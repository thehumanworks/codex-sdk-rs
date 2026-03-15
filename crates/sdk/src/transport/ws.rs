use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::{Connector, tungstenite::Message};
use url::Url;

use super::TransportHandle;
use crate::error::ClientError;

pub async fn connect_ws_transport(url: &str) -> Result<TransportHandle, ClientError> {
    connect_ws_transport_with_connector(url, None).await
}

async fn connect_ws_transport_with_connector(
    url: &str,
    connector: Option<Connector>,
) -> Result<TransportHandle, ClientError> {
    let parsed = Url::parse(url)
        .map_err(|err| ClientError::TransportSend(format!("invalid websocket URL: {err}")))?;

    if parsed.scheme() == "wss" {
        ensure_rustls_crypto_provider();
    }

    let (stream, _) =
        tokio_tungstenite::connect_async_tls_with_config(parsed.as_str(), None, false, connector)
            .await
            .map_err(|err| {
                ClientError::TransportSend(format!("websocket connect failed: {err}"))
            })?;

    let (mut ws_write, mut ws_read) = stream.split();

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Value>(256);
    let (inbound_tx, inbound_rx) = mpsc::channel::<Result<Value, ClientError>>(1024);

    let inbound_for_writer = inbound_tx.clone();
    tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            match serde_json::to_string(&message) {
                Ok(payload) => {
                    if let Err(err) = ws_write.send(Message::Text(payload.into())).await {
                        let _ = inbound_for_writer
                            .send(Err(ClientError::TransportSend(format!(
                                "websocket send failed: {err}"
                            ))))
                            .await;
                        break;
                    }
                }
                Err(err) => {
                    let _ = inbound_for_writer
                        .send(Err(ClientError::Serialization(err)))
                        .await;
                    break;
                }
            }
        }
    });

    tokio::spawn(async move {
        while let Some(frame) = ws_read.next().await {
            match frame {
                Ok(Message::Text(text)) => match serde_json::from_str::<Value>(&text) {
                    Ok(value) => {
                        if inbound_tx.send(Ok(value)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        if inbound_tx
                            .send(Err(ClientError::InvalidMessage(format!(
                                "failed to parse websocket frame as JSON: {err}"
                            ))))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                },
                Ok(Message::Binary(bin)) => match serde_json::from_slice::<Value>(&bin) {
                    Ok(value) => {
                        if inbound_tx.send(Ok(value)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        if inbound_tx
                            .send(Err(ClientError::InvalidMessage(format!(
                                "failed to parse websocket binary frame as JSON: {err}"
                            ))))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                },
                Ok(Message::Close(_)) => {
                    let _ = inbound_tx.send(Err(ClientError::TransportClosed)).await;
                    break;
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Err(err) => {
                    let _ = inbound_tx
                        .send(Err(ClientError::TransportSend(format!(
                            "websocket receive failed: {err}"
                        ))))
                        .await;
                    break;
                }
            }
        }
    });

    Ok(TransportHandle {
        outbound: outbound_tx,
        inbound: inbound_rx,
    })
}

fn ensure_rustls_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Context;
    use rcgen::generate_simple_self_signed;
    use rustls::{ClientConfig, RootCertStore, ServerConfig, pki_types::CertificateDer};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    use super::*;

    #[tokio::test]
    async fn connect_ws_transport_supports_wss_urls() -> anyhow::Result<()> {
        let _ = rustls::crypto::ring::default_provider().install_default();

        let generated = generate_simple_self_signed(vec!["localhost".to_string()])?;
        let cert_der = CertificateDer::from(generated.cert.der().to_vec());
        let key_der = generated.key_pair.serialize_der();

        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert_der.clone()],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()),
            )?;
        let acceptor = TlsAcceptor::from(Arc::new(server_config));

        let mut roots = RootCertStore::empty();
        roots
            .add(cert_der)
            .context("add test certificate to root store")?;
        let client_config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;

        let server = tokio::spawn(async move {
            let (tcp_stream, _) = listener.accept().await?;
            let tls_stream = acceptor.accept(tcp_stream).await?;
            let mut ws_stream = tokio_tungstenite::accept_async(tls_stream).await?;

            let frame = ws_stream
                .next()
                .await
                .context("expected websocket frame from client")??;
            let Message::Text(text) = frame else {
                anyhow::bail!("expected text frame from client, got {frame:?}");
            };

            ws_stream.send(Message::Text(text)).await?;
            anyhow::Ok(())
        });

        let mut handle = connect_ws_transport_with_connector(
            &format!("wss://localhost:{}", addr.port()),
            Some(Connector::Rustls(Arc::new(client_config))),
        )
        .await?;

        handle
            .outbound
            .send(json!({ "kind": "ping" }))
            .await
            .context("send outbound transport message")?;

        let received = handle
            .inbound
            .recv()
            .await
            .context("expected inbound transport message")??;
        assert_eq!(received, json!({ "kind": "ping" }));

        server.await??;
        Ok(())
    }
}
