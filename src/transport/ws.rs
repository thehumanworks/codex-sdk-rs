use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use url::Url;

use super::TransportHandle;
use crate::error::ClientError;

pub async fn connect_ws_transport(url: &str) -> Result<TransportHandle, ClientError> {
    let parsed = Url::parse(url)
        .map_err(|err| ClientError::TransportSend(format!("invalid websocket URL: {err}")))?;

    let (stream, _) = tokio_tungstenite::connect_async(parsed.as_str())
        .await
        .map_err(|err| ClientError::TransportSend(format!("websocket connect failed: {err}")))?;

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
