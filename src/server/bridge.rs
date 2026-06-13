//! Thin stdio↔daemon bridge.
//!
//! Each editor still spawns `axon serve` as a stdio child (so `.mcp.json` is
//! unchanged). When a [`super::daemon`] is running, `serve` forwards the MCP
//! session to it over the Unix socket — tagged with this session's workspace —
//! instead of holding its own in-process model. If no daemon is reachable, the
//! caller decides whether to retry or abort.

use anyhow::Result;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

use super::stdio;

/// Forward this process's stdio MCP session to the daemon at `socket_path`,
/// scoped to `workspace`.
///
/// Returns `Ok(false)` if the daemon isn't reachable. `Ok(true)` means the
/// bridge ran until the editor or daemon closed the connection.
pub async fn try_bridge(socket_path: &Path, workspace: &str) -> Result<bool> {
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        // No daemon listening: let the caller retry or abort.
        Err(_) => return Ok(false),
    };
    tracing::info!("Axon bridging to daemon at {}", socket_path.display());

    let (read_half, mut write_half) = stream.into_split();

    // Handshake: scope the session to this workspace.
    let handshake = json!({ "workspace": workspace }).to_string();
    write_half.write_all(handshake.as_bytes()).await?;
    write_half.write_all(b"\n").await?;
    write_half.flush().await?;

    let client_format = Arc::new(Mutex::new(stdio::StdioFormat::Framed));

    // Pump editor stdin → daemon and daemon → editor stdout concurrently.
    let stdin_client_format = Arc::clone(&client_format);
    let stdin_to_daemon = async move {
        let mut stdin = BufReader::new(io::stdin());
        while let Some(message) = stdio::read_message(&mut stdin).await? {
            *stdin_client_format.lock().await = message.format;
            let line = match serde_json::from_str::<serde_json::Value>(&message.body) {
                Ok(value) => serde_json::to_string(&value)?,
                Err(_) => message.body.trim().to_string(),
            };
            write_half.write_all(line.as_bytes()).await?;
            write_half.write_all(b"\n").await?;
            write_half.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    let stdout_client_format = Arc::clone(&client_format);
    let daemon_to_stdout = async move {
        let mut stdout = io::stdout();
        let mut lines = BufReader::new(read_half).lines();
        while let Some(line) = lines.next_line().await? {
            let format = *stdout_client_format.lock().await;
            stdio::write_message(&mut stdout, &line, format).await?;
        }
        Ok::<(), anyhow::Error>(())
    };

    // If stdin closes first (for example a one-shot validation pipe), keep
    // draining daemon output so the response is not lost. If the daemon closes
    // first, end the session and drop stdin forwarding.
    tokio::pin!(stdin_to_daemon);
    tokio::pin!(daemon_to_stdout);
    tokio::select! {
        r = &mut stdin_to_daemon => {
            r?;
            daemon_to_stdout.await?;
        }
        r = &mut daemon_to_stdout => r?,
    }

    Ok(true)
}
