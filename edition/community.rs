//! The community build's contribution: none.
//!
//! Every function here has the signature of its counterpart in `ee/app.rs`, which is what `super::active` resolves to in an enterprise build.

use loco_rs::{Result, app::AppContext, task::Tasks};

use crate::app::RouteMounts;
use crate::controllers::mcp::McpToolSet;
use crate::workers::registry::WorkerRegistry;

pub(crate) fn compose_context(_ctx: &AppContext) {}

pub(crate) fn workers(registry: WorkerRegistry) -> WorkerRegistry {
    registry
}

pub(crate) fn mount_routes(_mounts: &mut RouteMounts, _ctx: &AppContext) {}

pub(crate) fn mcp_tools(tools: McpToolSet) -> McpToolSet {
    tools
}

pub(crate) fn register_tasks(_tasks: &mut Tasks) {}

#[allow(clippy::unused_async)]
pub(crate) async fn seed(_ctx: &AppContext) -> Result<()> {
    Ok(())
}
