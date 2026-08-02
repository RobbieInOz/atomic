//! Tool registry — what a run is allowed to do.
//!
//! A tool owns its own dependencies (storage, settings, event sinks) at
//! construction time and receives only per-run state when it executes, so
//! conditional capabilities are conditional *registrations* rather than
//! branches inside a dispatch match.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::providers::types::ToolDefinition;

use super::citations::CitationLedger;

/// Outcome of a single tool execution.
///
/// `failed` is tracked separately from `output` because the error text still
/// has to reach the model (and the UI's tool card), while the recorded
/// status must say what actually happened.
pub struct ToolResult {
    pub output: String,
    /// How many things the tool found, for the caller's progress reporting.
    pub results_count: i32,
    pub failed: bool,
}

impl ToolResult {
    pub fn ok(output: impl Into<String>, results_count: i32) -> Self {
        Self {
            output: output.into(),
            results_count,
            failed: false,
        }
    }

    pub fn failed(error: impl std::fmt::Display) -> Self {
        Self {
            output: format!("Error: {}", error),
            results_count: 0,
            failed: true,
        }
    }
}

/// Per-run state a tool may reach into. Everything else a tool needs is
/// captured when it is constructed.
pub struct ToolContext<'a> {
    /// Register surfaced evidence here to make it citable, and put the
    /// returned number in the tool's output so the model can cite it.
    pub citations: &'a CitationLedger,
}

/// A callable capability. Implementors are cheap handles over shared state
/// (`StorageBackend`, `Arc` callbacks) — one instance serves the whole run.
#[async_trait]
pub trait AgentTool: Send + Sync {
    /// Name, description, and JSON schema sent to the model. Called once,
    /// at registration.
    fn definition(&self) -> ToolDefinition;

    /// Run the tool. Argument validation is the tool's job: a malformed
    /// call is a [`ToolResult::failed`], never a panic.
    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext<'_>) -> ToolResult;
}

/// The tools one run may call, in the order they were registered (which is
/// the order the model sees them).
#[derive(Default)]
pub struct ToolRegistry {
    definitions: Vec<ToolDefinition>,
    by_name: HashMap<String, Arc<dyn AgentTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool, replacing any earlier registration of the same name so a
    /// caller can override a default capability without reordering.
    pub fn register(&mut self, tool: impl AgentTool + 'static) -> &mut Self {
        let definition = tool.definition();
        match self
            .definitions
            .iter()
            .position(|existing| existing.name == definition.name)
        {
            Some(index) => self.definitions[index] = definition.clone(),
            None => self.definitions.push(definition.clone()),
        }
        self.by_name.insert(definition.name, Arc::new(tool));
        self
    }

    /// Chaining form of [`Self::register`], for conditional assembly.
    pub fn with(mut self, tool: impl AgentTool + 'static) -> Self {
        self.register(tool);
        self
    }

    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    pub fn get(&self, name: &str) -> Option<&dyn AgentTool> {
        self.by_name.get(name).map(|tool| tool.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Echo {
        name: &'static str,
        reply: &'static str,
    }

    #[async_trait]
    impl AgentTool for Echo {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(self.name, "echo", json!({ "type": "object" }))
        }

        async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext<'_>) -> ToolResult {
            ToolResult::ok(self.reply, 1)
        }
    }

    #[tokio::test]
    async fn registration_order_is_preserved_and_names_are_unique() {
        let registry = ToolRegistry::new()
            .with(Echo {
                name: "first",
                reply: "a",
            })
            .with(Echo {
                name: "second",
                reply: "b",
            })
            .with(Echo {
                name: "first",
                reply: "replaced",
            });

        let names: Vec<&str> = registry
            .definitions()
            .iter()
            .map(|definition| definition.name.as_str())
            .collect();
        assert_eq!(names, vec!["first", "second"]);

        let ledger = CitationLedger::new(super::super::CitationAdmission::Open);
        let ctx = ToolContext { citations: &ledger };
        let result = registry
            .get("first")
            .expect("registered tool")
            .execute(&json!({}), &ctx)
            .await;
        assert_eq!(result.output, "replaced");
        assert!(registry.get("missing").is_none());
    }
}
