//! Thin stdio↔daemon bridge.
//!
//! Each editor still spawns `axon serve` as a stdio child (so `.mcp.json` is
//! unchanged). When a [`super::daemon`] is running, `serve` forwards the MCP
//! session to it over the Unix socket — tagged with this session's workspace —
//! instead of holding its own in-process model. If no daemon is reachable, the
//! caller falls back to the standalone in-process server.

use anyhow::Result;
use serde_json::json;
use std::path::Path;
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Forward this process's stdio MCP session to the daemon at `socket_path`,
/// scoped to `workspace`.
///
/// Returns `Ok(false)` if the daemon isn't reachable (the caller should fall
/// back to standalone). `Ok(true)` means the bridge ran until the editor or the
/// daemon closed the connection.
pub async fn try_bridge(socket_path: &Path, workspace: &str) -> Result<bool> {
    let stream = match UnixStream::connect(socket_path).await {
        Ok(s) => s,
        // No daemon listening → signal the caller to run standalone.
        Err(_) => return Ok(false),
    };
    tracing::info!("Axon bridging to daemon at {}", socket_path.display());

    let (read_half, mut write_half) = stream.into_split();

    // Handshake: scope the session to this workspace.
    let handshake = json!({ "workspace": workspace }).to_string();
    write_half.write_all(handshake.as_bytes()).await?;
    write_half.write_all(b"\n").await?;
    write_half.flush().await?;

    // Pump editor stdin → daemon and daemon → editor stdout concurrently.
    let stdin_to_daemon = async move {
        let mut lines = BufReader::new(io::stdin()).lines();
        while let Some(line) = lines.next_line().await? {
            write_half.write_all(line.as_bytes()).await?;
            write_half.write_all(b"\n").await?;
            write_half.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    let daemon_to_stdout = async move {
        let mut stdout = io::stdout();
        let mut lines = BufReader::new(read_half).lines();
        while let Some(line) = lines.next_line().await? {
            stdout.write_all(line.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
        Ok::<(), anyhow::Error>(())
    };

    // Whichever side closes first ends the session.
    tokio::select! {
        r = stdin_to_daemon => r?,
        r = daemon_to_stdout => r?,
    }

    Ok(true)
}
