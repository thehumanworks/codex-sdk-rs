pub mod stdio;
pub mod ws;
pub mod ws_daemon;

use serde_json::Value;
use tokio::sync::mpsc;

use crate::error::ClientError;

pub(crate) const OUTBOUND_CAPACITY: usize = 256;
pub(crate) const INBOUND_CAPACITY: usize = 1024;

pub(crate) type InboundSender = mpsc::Sender<Result<Value, ClientError>>;

pub struct TransportHandle {
    pub outbound: mpsc::Sender<Value>,
    pub inbound: mpsc::Receiver<Result<Value, ClientError>>,
}

/// Creates the outbound/inbound channel pairs shared by all transports.
pub(crate) fn transport_channels() -> (
    mpsc::Sender<Value>,
    mpsc::Receiver<Value>,
    InboundSender,
    mpsc::Receiver<Result<Value, ClientError>>,
) {
    let (outbound_tx, outbound_rx) = mpsc::channel::<Value>(OUTBOUND_CAPACITY);
    let (inbound_tx, inbound_rx) = mpsc::channel::<Result<Value, ClientError>>(INBOUND_CAPACITY);
    (outbound_tx, outbound_rx, inbound_tx, inbound_rx)
}

/// Writer loop shared by all transports: serializes each queued `Value` and
/// hands the JSON text to `send_frame`. On a serialization or send error the
/// error is pushed onto the inbound channel and the loop stops.
///
/// Spawn the returned future with `tokio::spawn`.
pub(crate) async fn write_json_frames<F>(
    mut outbound_rx: mpsc::Receiver<Value>,
    inbound_tx: InboundSender,
    mut send_frame: F,
) where
    F: AsyncFnMut(String) -> Result<(), ClientError>,
{
    while let Some(message) = outbound_rx.recv().await {
        let result = match serde_json::to_string(&message) {
            Ok(payload) => send_frame(payload).await,
            Err(err) => Err(ClientError::Serialization(err)),
        };
        if let Err(err) = result {
            let _ = inbound_tx.send(Err(err)).await;
            break;
        }
    }
}

/// One step of a transport's inbound stream, as consumed by
/// [`read_json_frames`].
pub(crate) enum RawFrame {
    /// A payload that should be parsed as a JSON value.
    Payload(Vec<u8>),
    /// A frame with no JSON payload (e.g. websocket ping/pong); keep reading.
    Skip,
    /// The transport terminated; forward the error and stop reading.
    Closed(ClientError),
    /// The underlying stream ended without an explicit close; stop silently.
    Eof,
}

/// Reader loop shared by all transports: pulls [`RawFrame`]s from
/// `next_frame`, parses payloads as JSON, and forwards `Ok(Value)` /
/// `Err(ClientError::InvalidMessage)` onto the inbound channel until the
/// stream ends. `context` names the frame kind in parse-error messages.
///
/// Spawn the returned future with `tokio::spawn`.
pub(crate) async fn read_json_frames<F>(
    inbound_tx: InboundSender,
    context: &'static str,
    mut next_frame: F,
) where
    F: AsyncFnMut() -> RawFrame,
{
    loop {
        match next_frame().await {
            RawFrame::Payload(bytes) => {
                let parsed = serde_json::from_slice::<Value>(&bytes).map_err(|err| {
                    ClientError::InvalidMessage(format!("failed to parse {context}: {err}"))
                });
                if inbound_tx.send(parsed).await.is_err() {
                    break;
                }
            }
            RawFrame::Skip => {}
            RawFrame::Closed(err) => {
                let _ = inbound_tx.send(Err(err)).await;
                break;
            }
            RawFrame::Eof => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn reader_reports_invalid_json_as_invalid_message() {
        let (inbound_tx, mut inbound_rx) = mpsc::channel(INBOUND_CAPACITY);
        let mut frames = VecDeque::from([
            RawFrame::Payload(b"not json".to_vec()),
            RawFrame::Skip,
            RawFrame::Payload(b"{\"ok\":true}".to_vec()),
            RawFrame::Closed(ClientError::TransportClosed),
        ]);

        read_json_frames(inbound_tx, "test frame", async move || {
            frames.pop_front().expect("reader should stop at Closed")
        })
        .await;

        match inbound_rx.recv().await {
            Some(Err(ClientError::InvalidMessage(message))) => {
                assert!(
                    message.contains("failed to parse test frame"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected InvalidMessage error, got {other:?}"),
        }

        match inbound_rx.recv().await {
            Some(Ok(value)) => assert_eq!(value, json!({ "ok": true })),
            other => panic!("expected parsed value, got {other:?}"),
        }

        assert!(matches!(
            inbound_rx.recv().await,
            Some(Err(ClientError::TransportClosed))
        ));
        assert!(inbound_rx.recv().await.is_none(), "reader should stop");
    }

    #[tokio::test]
    async fn writer_forwards_send_errors_to_inbound_channel() {
        let (outbound_tx, outbound_rx) = mpsc::channel::<Value>(OUTBOUND_CAPACITY);
        let (inbound_tx, mut inbound_rx) = mpsc::channel(INBOUND_CAPACITY);

        outbound_tx.send(json!({ "n": 1 })).await.expect("queue");
        drop(outbound_tx);

        write_json_frames(outbound_rx, inbound_tx, async move |_payload: String| {
            Err(ClientError::TransportSend("sink failed".to_string()))
        })
        .await;

        match inbound_rx.recv().await {
            Some(Err(ClientError::TransportSend(message))) => {
                assert_eq!(message, "sink failed");
            }
            other => panic!("expected TransportSend error, got {other:?}"),
        }
    }
}
