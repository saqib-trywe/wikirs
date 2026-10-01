//! The MCP Interface (docs/spec/interfaces.md#mcp, errors.md#mcp):
//! - every registry Operation as a tool, added at runtime;
//! - Pages and Attachments as resources;
//! - `watch` as resource notifications: on `subscriptions/listen` for
//!   2026-07-28 clients, and `resources/subscribe` for legacy ones.
//!
//! Streamable HTTP (`/mcp` under `serve`) comes with the HTTP slice.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, InitializeResult,
        ListResourceTemplatesResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
        ReadResourceRequestParams, ReadResourceResponse, ResourceUpdatedNotificationParam,
        ServerCapabilities, SubscribeRequestParams, SubscriptionFilter, Tool, ToolAnnotations,
        UnsubscribeRequestParams,
    },
    service::{NotificationContext, RequestContext, SubscriptionContext, SubscriptionSendError},
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;
use wikirs_core::{Kind, OpInfo, Wiki, index::Scope};

pub mod resources;

use resources::Notice;

/// Mutations that can lose or break content, marked `destructiveHint`.
const DESTRUCTIVE: [&str; 5] = [
    "delete_page",
    "delete_attachment",
    "move_page",
    "move_attachment",
    "rename_tag",
];

#[derive(Clone)]
pub struct WikiServer {
    wiki: Arc<Wiki>,
    /// `--read-only`: the adapter capability `mutations: false`.
    read_only: bool,
    /// URIs a legacy client subscribed to with `resources/subscribe`: one
    /// set per session (see [`WikiServer::session`]).
    subscribed: Arc<Mutex<HashSet<String>>>,
}

impl WikiServer {
    #[must_use]
    pub fn new(wiki: Wiki) -> Self {
        Self {
            wiki: Arc::new(wiki),
            read_only: false,
            subscribed: Arc::default(),
        }
    }

    /// Lists only queries and `rebuild_index` as tools (http-security.md#read-only-mode).
    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// A server for one more session (Streamable HTTP): same Wiki and
    /// policy, its own subscriptions.
    #[must_use]
    pub fn session(&self) -> Self {
        Self {
            subscribed: Arc::default(),
            ..self.clone()
        }
    }

    /// Whether `op` is one of this server's tools.
    fn exposes(&self, op: &OpInfo) -> bool {
        match op.kind {
            Kind::Subscription => false,
            Kind::Mutation => !self.read_only,
            Kind::Query | Kind::Maintenance => true,
        }
    }

    /// This Wiki's `watch` events as notices, relayed from a thread (the
    /// core's channel blocks) that ends at the first notice after the
    /// receiver is dropped.
    fn notices(&self) -> Result<UnboundedReceiver<Notice>, McpError> {
        let events = self.wiki.watch(Scope::default());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("wikirs-mcp-notices".into())
            .spawn(move || resources::relay(events, |n| tx.send(n).is_ok()))
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(rx)
    }

    fn find(&self, name: &str) -> Option<OpInfo> {
        wikirs_core::find(name).filter(|op| self.exposes(op))
    }
}

fn tool(op: &OpInfo) -> Tool {
    let Value::Object(schema) = op.input_schema() else {
        unreachable!("input schemas are objects")
    };
    // `rebuild_index` carries neither hint: it rewrites only the disposable Index.
    let annotations = match op.kind {
        Kind::Query => ToolAnnotations::new().read_only(true),
        Kind::Mutation => ToolAnnotations::new()
            .read_only(false)
            .destructive(DESTRUCTIVE.contains(&op.name)),
        Kind::Maintenance | Kind::Subscription => ToolAnnotations::new(),
    };
    Tool::new(op.name, op.description, Arc::new(schema)).annotate(annotations)
}

/// Runs blocking core work off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, McpError> + Send + 'static,
) -> Result<T, McpError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?
}

impl ServerHandler for WikiServer {
    fn get_info(&self) -> InitializeResult {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .enable_resources_subscribe()
            .enable_resources_list_changed()
            .build();
        let mut info = InitializeResult::new(capabilities);
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
            .filter(|op| self.exposes(op))
            .map(tool)
            .collect();
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.find(name).map(|op| tool(&op))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some(op) = self.find(&request.name) else {
            return Err(McpError::invalid_params(
                format!("unknown tool `{}`", request.name),
                None,
            ));
        };
        let wiki = self.wiki.clone();
        let args = Value::Object(request.arguments.unwrap_or_default());
        let outcome = blocking(move || Ok(op.call(&wiki, args))).await?;
        // Operation errors are tool results, so the model can see them and correct itself.
        Ok(match outcome {
            Ok(envelope) => CallToolResult::structured(envelope),
            Err(err) => CallToolResult::structured_error(err.to_json()),
        }
        .into())
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let wiki = self.wiki.clone();
        let cursor = request.and_then(|r| r.cursor);
        blocking(move || resources::list(&wiki, cursor.as_deref())).await
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, McpError>> + Send + '_ {
        std::future::ready(Ok(resources::templates()))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let wiki = self.wiki.clone();
        blocking(move || resources::read(&wiki, &request.uri))
            .await
            .map(Into::into)
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        let uris = requested
            .resource_subscriptions
            .iter()
            .flatten()
            .filter(|uri| resources::is_resource_uri(uri))
            .cloned();
        Some(
            SubscriptionFilter::builder()
                .resources_list_changed()
                .resource_subscriptions(uris)
                .build(),
        )
    }

    /// 2026-07-28 clients: `watch` as notifications on one listen request,
    /// until the client cancels it.
    async fn listen(&self, context: SubscriptionContext) -> Result<(), McpError> {
        let mut notices = self.notices()?;
        let sink = context.sink().clone();
        loop {
            let notice = tokio::select! {
                () = context.cancelled() => return Ok(()),
                notice = notices.recv() => match notice {
                    Some(notice) => notice,
                    None => return Ok(()),
                },
            };
            let sent = match notice {
                Notice::Updated(uri) => sink.notify_resource_updated(uri).await,
                Notice::ListChanged => sink.notify_resource_list_changed().await,
            };
            match sent {
                // The sink drops resources the client didn't subscribe to.
                Ok(()) | Err(SubscriptionSendError::NotificationNotAccepted(_)) => {}
                Err(_) => return Ok(()),
            }
        }
    }

    /// Legacy clients: `list_changed` from the start, `updated` for the
    /// resources they subscribe to.
    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let Ok(mut notices) = self.notices() else {
            return;
        };
        let subscribed = self.subscribed.clone();
        tokio::spawn(async move {
            while let Some(notice) = notices.recv().await {
                let sent = match notice {
                    Notice::Updated(uri) if lock(&subscribed).contains(&uri) => {
                        context
                            .peer
                            .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                            .await
                    }
                    Notice::Updated(_) => Ok(()),
                    Notice::ListChanged => context.peer.notify_resource_list_changed().await,
                };
                if sent.is_err() {
                    return;
                }
            }
        });
    }

    #[allow(deprecated)] // legacy-only by design: pre-2026-07-28 clients subscribe this way
    fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        let outcome = if resources::is_resource_uri(&request.uri) {
            lock(&self.subscribed).insert(request.uri);
            Ok(())
        } else {
            Err(McpError::resource_not_found(
                format!("`{}` is not a wiki:// Page or Attachment URI", request.uri),
                None,
            ))
        };
        std::future::ready(outcome)
    }

    #[allow(deprecated)]
    fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<(), McpError>> + Send + '_ {
        lock(&self.subscribed).remove(&request.uri);
        std::future::ready(Ok(()))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serves `wiki` over MCP on stdio until the client disconnects.
pub fn run_stdio(wiki: Wiki, read_only: bool) -> anyhow::Result<()> {
    // A long-lived process keeps the Index fresh by watching (process-model.md).
    // Without a watcher, queries still work: `index_status` reports `none`.
    let _ = wiki.start_watcher();
    let server = WikiServer::new(wiki);
    let server = if read_only {
        server.read_only()
    } else {
        server
    };
    tokio::runtime::Runtime::new()?.block_on(async {
        let service = server.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}
