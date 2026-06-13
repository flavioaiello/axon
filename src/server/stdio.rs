use anyhow::Result;
use tokio::io::{self, BufReader};

use crate::mcp::handle_request_with_registry;
use crate::mcp::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::server::transport;
use crate::store::CrateRegistry;

/// Run the MCP server over stdio (stdin/stdout), the standard transport for
/// VS Code / GitHub Copilot MCP integration.
///
/// The registry holds per-crate stores. The primary crate's store and workspace
/// key are extracted and threaded through the request handling unchanged.
pub async fn run(registry: std::sync::Arc<CrateRegistry>) -> Result<()> {
    let stdin = BufReader::new(io::stdin());
    let mut stdout = io::stdout();
    let mut messages = stdin;

    tracing::info!("Axon stdio transport ready");

    while let Some(message) = transport::read_message(&mut messages).await? {
        let message = message.trim().to_string();
        if message.is_empty() {
            continue;
        }

        tracing::debug!("← {}", message);

        let request: JsonRpcRequest = match serde_json::from_str(&message) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                send(&mut stdout, &resp).await?;
                continue;
            }
        };

        let response = handle_request_with_registry(&registry, &request);

        // Notifications (no id) don't get a response
        if request.id.is_some() {
            send(&mut stdout, &response).await?;
        }
    }

    Ok(())
}

async fn send(stdout: &mut io::Stdout, resp: &JsonRpcResponse) -> Result<()> {
    tracing::debug!("→ {}", serde_json::to_string(resp)?);
    transport::write_json(stdout, resp).await
}
