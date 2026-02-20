use std::collections::HashMap;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::TransportHandle;
use crate::error::ClientError;

pub async fn spawn_stdio_transport(
    binary: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> Result<TransportHandle, ClientError> {
    let mut cmd = Command::new(binary);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    for (key, value) in env {
        cmd.env(key, value);
    }

    let mut child = cmd.spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ClientError::TransportSend("missing child stdin".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ClientError::TransportSend("missing child stdout".to_string()))?;

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<Value>(256);
    let (inbound_tx, inbound_rx) = mpsc::channel::<Result<Value, ClientError>>(1024);

    let inbound_for_writer = inbound_tx.clone();
    tokio::spawn(async move {
        while let Some(message) = outbound_rx.recv().await {
            match serde_json::to_string(&message) {
                Ok(line) => {
                    if let Err(err) = stdin.write_all(line.as_bytes()).await {
                        let _ = inbound_for_writer.send(Err(ClientError::Io(err))).await;
                        break;
                    }
                    if let Err(err) = stdin.write_all(b"\n").await {
                        let _ = inbound_for_writer.send(Err(ClientError::Io(err))).await;
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

    let inbound_for_reader = inbound_tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        if inbound_for_reader.send(Ok(value)).await.is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        if inbound_for_reader
                            .send(Err(ClientError::InvalidMessage(format!(
                                "failed to parse JSONL frame: {err}"
                            ))))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                },
                Ok(None) => {
                    let _ = inbound_for_reader
                        .send(Err(ClientError::TransportClosed))
                        .await;
                    break;
                }
                Err(err) => {
                    let _ = inbound_for_reader.send(Err(ClientError::Io(err))).await;
                    break;
                }
            }
        }
    });

    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    Ok(TransportHandle {
        outbound: outbound_tx,
        inbound: inbound_rx,
    })
}
