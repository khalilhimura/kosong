//! `kosong mcp` — Model Context Protocol server over stdio.
//!
//! Exposes three read-only tools:
//! - `kosong_status`  — workspace & document state
//! - `kosong_doctor`  — prerequisite checks
//! - `kosong_show`    — document content
//!
//! Lesson: software can describe its own capabilities to other software.

use crate::exit::{CliError, CliResult};
use super::show;
use super::status::status_json;
use super::doctor::doctor_json;
use super::Context;
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::tool::ToolRouter,
    model::*,
    tool, tool_handler, tool_router,
};
use std::sync::Arc;

/// MCP server that exposes kosong's read-only introspection tools.
pub struct KosongMcp {
    tool_router: ToolRouter<KosongMcp>,
    context: Arc<Context>,
}

#[tool_router]
impl KosongMcp {
    fn new(context: Arc<Context>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            context,
        }
    }

    /// Report the current state of your kosong workspace and page.
    #[tool(description = "Report the current state of your kosong workspace and page.")]
    async fn kosong_status(&self) -> String {
        match status_json(&self.context) {
            Ok(json) => json,
            Err(e) => format!("{{\"error\":\"{}\"}}", e.message.replace('"', r#"\""#)),
        }
    }

    /// Check that everything kosong needs is in place.
    #[tool(description = "Check that everything kosong needs is in place.")]
    async fn kosong_doctor(&self) -> String {
        match doctor_json(&self.context) {
            Ok(json) => json,
            Err(e) => format!("{{\"error\":\"{}\"}}", e.message.replace('"', r#"\""#)),
        }
    }

    /// Return the document content.
    #[tool(description = "Return the document content.")]
    async fn kosong_show(&self) -> String {
        match show::show_raw(&self.context) {
            Ok(text) => text,
            Err(e) => format!("error: {}", e.message),
        }
    }
}

#[tool_handler]
impl ServerHandler for KosongMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Run the MCP server over stdio transport.
///
/// The server processes JSON-RPC messages on stdin and writes responses to
/// stdout. It runs until stdin is closed.
pub fn run(context: &Context) -> CliResult<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::internal("MCP_RUNTIME", format!("could not build runtime: {e}")))?;

    runtime.block_on(async {
        let transport = (tokio::io::stdin(), tokio::io::stdout());
        let service = KosongMcp::new(Arc::new(context.clone()))
            .serve(transport)
            .await
            .map_err(|e| CliError::internal("MCP_SERVE", format!("MCP server error: {e}")))?;
        service.waiting().await.map_err(|e| {
            CliError::internal("MCP_WAIT", format!("MCP server finished: {e}"))
        })?;
        Ok(())
    })
}
