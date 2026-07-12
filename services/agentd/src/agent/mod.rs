//! Multi-agent orchestration — Orchestrator + Specialist mesh.

pub mod orchestrator;
pub mod registry;
pub mod session;

pub use orchestrator::OrchestratorAgent;
pub use registry::AgentRegistry;
pub use session::AgentDecision;
