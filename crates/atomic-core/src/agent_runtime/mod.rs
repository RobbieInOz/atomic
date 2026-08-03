//! The agent runtime: one tool-calling loop, one citation system, shared by
//! every agentic surface in Atomic.
//!
//! A caller assembles three things and runs them:
//!
//! - a [`ToolRegistry`] — the capabilities this run has, registered
//!   conditionally rather than branched on inside a dispatch match;
//! - a [`CitationLedger`] — numbered evidence, where tools register what
//!   they surface and the final text's `[N]` markers decide what is stored;
//! - a [`RunConfig`] — model, budget, termination mode, and whether text
//!   streams back as it is produced.
//!
//! The runtime knows nothing about transports or storage. Progress leaves it
//! through a [`RunEventSink`] callback (the pattern `EmbeddingEvent` uses),
//! and hosts map those events onto their own systems — `ChatEvent` for chat,
//! nothing at all for a scheduled report.

pub mod citations;
mod context;
pub mod run;
pub mod tools;

pub use citations::{Citable, CitationAdmission, CitationLedger, CitationSource};
pub use context::truncate_messages_to_context;
pub use run::{
    messages_are_balanced, AgentRun, CancelFlag, RunConfig, RunError, RunEvent, RunEventSink,
    RunOutcome, StopReason, Termination, ToolCallRecord,
};
pub use tools::{AgentTool, ToolContext, ToolRegistry, ToolResult};
