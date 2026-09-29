//! The MCP Interface: every registry Operation as a tool, added at runtime
//! (docs/spec/interfaces.md, errors.md).
//!
//! Walking skeleton: stdio only, tools only. Resources, `watch` notifications,
//! `--read-only` and Streamable HTTP come in later slices.

use std::sync::Arc;

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, InitializeResult,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool, ToolAnnotations,
    },
    service::RequestContext,
};
use serde_json::Value;
use wikirs_core::{Kind, OpInfo, Wiki};

#[derive(Clone)]
pub struct WikiServer {
    wiki: Arc<Wiki>,
}

impl WikiServer {
    #[must_use]
    pub fn new(wiki: Wiki) -> Self {
        Self {
            wiki: Arc::new(wiki),
        }
    }
}

fn tool(op: &OpInfo) -> Tool {
    let Value::Object(schema) = op.input_schema() else {
        unreachable!("input schemas are objects")
    };
    let annotations = match op.kind {
        Kind::Query => ToolAnnotations::new().read_only(true),
        _ => ToolAnnotations::new().read_only(false),
    };
    Tool::new(op.name, op.description, Arc::new(schema)).annotate(annotations)
}

impl ServerHandler for WikiServer {
    fn get_info(&self) -> InitializeResult {
        let mut info = InitializeResult::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("wikirs", env!("CARGO_PKG_VERSION"));
        info
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = wikirs_core::registry()
            .iter()
            .filter(|op| op.kind != Kind::Subscription)
            .map(tool)
            .collect();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        wikirs_core::find(name).map(|op| tool(&op))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some(op) = wikirs_core::find(&request.name) else {
            return Err(McpError::invalid_params(
                format!("unknown tool `{}`", request.name),
                None,
            ));
        };
        let wiki = self.wiki.clone();
        let args = Value::Object(request.arguments.unwrap_or_default());
        let outcome = tokio::task::spawn_blocking(move || op.call(&wiki, args))
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Operation errors are tool results, so the model can see them and correct itself.
        Ok(match outcome {
            Ok(envelope) => CallToolResult::structured(envelope),
            Err(err) => CallToolResult::structured_error(err.to_json()),
        }
        .into())
    }
}

/// Serves `wiki` over MCP on stdio until the client disconnects.
pub fn run_stdio(wiki: Wiki) -> anyhow::Result<()> {
    // A long-lived process keeps the Index fresh by watching (process-model.md).
    // Without a watcher, queries still work: `index_status` reports `none`.
    let _ = wiki.start_watcher();
    tokio::runtime::Runtime::new()?.block_on(async {
        let service = WikiServer::new(wiki)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok(())
    })
}
