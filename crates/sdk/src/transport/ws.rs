use futures_util::{SinkExt, StreamExt};
use http::{HeaderValue, header::AUTHORIZATION};
use tokio_tungstenite::{
    Connector,
    tungstenite::{Message, client::IntoClientRequest},
};
use url::Url;

use super::{RawFrame, TransportHandle, read_json_frames, transport_channels, write_json_frames};
use crate::error::ClientError;

pub async fn connect_ws_transport(
    url: &str,
    auth_token: Option<&str>,
) -> Result<TransportHandle, ClientError> {
    connect_ws_transport_with_connector(url, auth_token, None).await
}

async fn connect_ws_transport_with_connector(
    url: &str,
    auth_token: Option<&str>,
    connector: Option<Connector>,
) -> Result<TransportHandle, ClientError> {
    let parsed = Url::parse(url)
        .map_err(|err| ClientError::Config(format!("invalid websocket URL: {err}")))?;

    if parsed.scheme() == "wss" {
        ensure_rustls_crypto_provider();
    }

    let request = build_ws_request(parsed.as_str(), auth_token)?;
    let (stream, _) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector)
            .await
            .map_err(|err| {
                ClientError::TransportSend(format!("websocket connect failed: {err}"))
            })?;

    let (mut ws_write, mut ws_read) = stream.split();

    let (outbound_tx, outbound_rx, inbound_tx, inbound_rx) = transport_channels();

    tokio::spawn(write_json_frames(
        outbound_rx,
        inbound_tx.clone(),
        async move |payload: String| {
            ws_write
                .send(Message::Text(payload.into()))
                .await
                .map_err(|err| ClientError::TransportSend(format!("websocket send failed: {err}")))
        },
    ));

    tokio::spawn(read_json_frames(
        inbound_tx,
        "websocket frame as JSON",
        async move || match ws_read.next().await {
            Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                RawFrame::Payload(message.into_data().to_vec())
            }
            Some(Ok(Message::Close(_))) => RawFrame::Closed(ClientError::TransportClosed),
            Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => RawFrame::Skip,
            Some(Err(err)) => RawFrame::Closed(ClientError::TransportSend(format!(
                "websocket receive failed: {err}"
            ))),
            None => RawFrame::Eof,
        },
    ));

    Ok(TransportHandle {
        outbound: outbound_tx,
        inbound: inbound_rx,
    })
}

pub(crate) fn build_ws_request(
    url: &str,
    auth_token: Option<&str>,
) -> Result<http::Request<()>, ClientError> {
    if auth_token.is_some() && !websocket_url_allows_auth_token(url) {
        return Err(ClientError::Config(
            "websocket auth tokens require `wss://` or a loopback `ws://` URL".to_string(),
        ));
    }

    let mut request = url.into_client_request().map_err(|err| {
        ClientError::Config(format!("invalid websocket request for `{url}`: {err}"))
    })?;

    if let Some(token) = auth_token {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|err| ClientError::Config(format!("invalid websocket auth token: {err}")))?;
        request.headers_mut().insert(AUTHORIZATION, value);
    }

    Ok(request)
}

/// Bearer auth is only allowed for `wss://` or loopback `ws://` endpoints.
pub fn websocket_url_allows_auth_token(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("wss://") {
        return true;
    }
    if !lower.starts_with("ws://") {
        return false;
    }
    let authority = lower[5..].split(['/', '?', '#']).next().unwrap_or_default();
    let hostport = authority
        .rsplit_once('@')
        .map(|(_, hostport)| hostport)
        .unwrap_or(authority);
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        rest.split(']').next().unwrap_or_default()
    } else {
        hostport.split(':').next().unwrap_or_default()
    };
    matches!(host, "127.0.0.1" | "localhost" | "::1")
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
    use futures_util::{SinkExt, StreamExt};
    use http::header::AUTHORIZATION;
    use rcgen::generate_simple_self_signed;
    use rustls::{ClientConfig, RootCertStore, ServerConfig, pki_types::CertificateDer};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;
    use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

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
            None,
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

    #[tokio::test]
    async fn connect_ws_transport_sends_authorization_bearer_header() -> anyhow::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let expected = Arc::new("secret-token".to_string());
        let expected_for_server = Arc::clone(&expected);

        let server = tokio::spawn(async move {
            let (tcp_stream, _) = listener.accept().await?;
            let callback =
                |request: &Request, response: Response| -> Result<Response, ErrorResponse> {
                    let auth = request
                        .headers()
                        .get(AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default();
                    if auth == format!("Bearer {}", expected_for_server.as_str()) {
                        Ok(response)
                    } else {
                        Err(http::Response::builder()
                            .status(http::StatusCode::UNAUTHORIZED)
                            .body(None)
                            .expect("unauthorized response"))
                    }
                };
            let mut ws_stream = tokio_tungstenite::accept_hdr_async(tcp_stream, callback).await?;
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

        let mut handle = connect_ws_transport(
            &format!("ws://127.0.0.1:{}", addr.port()),
            Some(expected.as_str()),
        )
        .await?;

        handle
            .outbound
            .send(json!({ "kind": "auth-ping" }))
            .await
            .context("send outbound transport message")?;
        let received = handle
            .inbound
            .recv()
            .await
            .context("expected inbound transport message")??;
        assert_eq!(received, json!({ "kind": "auth-ping" }));

        server.await??;
        Ok(())
    }

    #[test]
    fn build_ws_request_rejects_control_characters_in_token() {
        let error = build_ws_request("ws://127.0.0.1:4222", Some("bad\ntoken"))
            .expect_err("newline token must fail");
        assert!(matches!(error, ClientError::Config(_)));
    }

    #[test]
    fn build_ws_request_rejects_auth_on_non_loopback_ws() {
        let error = build_ws_request("ws://example.com:4222", Some("secret"))
            .expect_err("remote ws auth must fail");
        assert!(matches!(error, ClientError::Config(_)));
        assert!(websocket_url_allows_auth_token("wss://example.com"));
        assert!(websocket_url_allows_auth_token("ws://127.0.0.1:4222"));
        assert!(!websocket_url_allows_auth_token("ws://192.168.1.10:4222"));
    }
}
