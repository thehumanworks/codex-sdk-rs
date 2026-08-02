use std::collections::HashMap;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::{RawFrame, TransportHandle, read_json_frames, transport_channels, write_json_frames};
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

    let (outbound_tx, outbound_rx, inbound_tx, inbound_rx) = transport_channels();

    tokio::spawn(write_json_frames(
        outbound_rx,
        inbound_tx.clone(),
        async move |payload: String| {
            stdin
                .write_all(payload.as_bytes())
                .await
                .map_err(ClientError::Io)?;
            stdin.write_all(b"\n").await.map_err(ClientError::Io)
        },
    ));

    let mut lines = BufReader::new(stdout).lines();
    tokio::spawn(read_json_frames(
        inbound_tx,
        "JSONL frame",
        async move || match lines.next_line().await {
            Ok(Some(line)) => RawFrame::Payload(line.into_bytes()),
            Ok(None) => RawFrame::Closed(ClientError::TransportClosed),
            Err(err) => RawFrame::Closed(ClientError::Io(err)),
        },
    ));

    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    Ok(TransportHandle {
        outbound: outbound_tx,
        inbound: inbound_rx,
    })
}
