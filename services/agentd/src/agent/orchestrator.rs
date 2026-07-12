//! Orchestrator agent — routes events to specialists via pattern matching + LLM triage.
//!
//! Routing logic (in order):
//! 1. **Direct match**: if an event type matches a specialist's `trigger_patterns`, skip
//!    orchestrator and run specialist directly.
//! 2. **LLM triage**: orchestrator analyses the event and calls a `transfer_to_{specialist}`
//!    tool to route to the correct agent.
//! 3. **Fallback**: if no specialist is invoked within `max_turns`, orchestrator handles
//!    it directly.

use std::sync::Arc;

use serde_json::Value;
use tracing::{info, instrument, warn};

use super::registry::AgentRegistry;
use super::session::{AgentDecision, AgentSession};
use crate::config::{AgentdConfig, OrchestratorConfig};
use crate::llm::{
    CompletionConfig, CompletionResult, LlmProvider, Message, build_provider, handoff_tools,
    parse_handoff,
};
use crate::mcp::McpPool;
use crate::rag::RagEngine;

pub struct OrchestratorAgent {
    provider: Arc<dyn LlmProvider>,
    cfg: OrchestratorConfig,
    completion_cfg: CompletionConfig,
}

impl OrchestratorAgent {
    pub fn new(cfg: &AgentdConfig) -> anyhow::Result<Self> {
        let provider_cfg = cfg
            .providers
            .get(&cfg.orchestrator.provider)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "orchestrator references unknown provider '{}'",
                    cfg.orchestrator.provider
                )
            })?;
        let provider = build_provider(&cfg.orchestrator.provider, provider_cfg);
        let completion_cfg = CompletionConfig {
            model: cfg.orchestrator.model.clone(),
            max_tokens: 2048,
        };
        Ok(Self {
            provider,
            cfg: cfg.orchestrator.clone(),
            completion_cfg,
        })
    }

    /// Entry point: route an event to the correct agent and run it.
    /// Follows handoffs up to 3 hops before giving up.
    #[allow(clippy::too_many_arguments)]
    #[instrument(skip(self, registry, mcp, rag))]
    pub async fn dispatch(
        &self,
        event_id: String,
        event_type: String,
        event_data: Value,
        registry: &AgentRegistry,
        mcp: &McpPool,
        rag: Option<&RagEngine>,
        tenant: &str,
    ) -> AgentDecision {
        // ── 1. Direct match ──────────────────────────────────────────────────
        if let Some(specialist) = registry.find_specialist(&event_type) {
            info!(agent = %specialist.name, "direct route (trigger_patterns match)");
            return self
                .run_with_handoff(
                    specialist, event_id, event_type, event_data, registry, mcp, rag, 0,
                )
                .await;
        }

        // ── 2. LLM triage ────────────────────────────────────────────────────
        let agent_descriptions: Vec<String> = registry
            .agent_names
            .iter()
            .filter_map(|n| registry.get(n))
            .map(|a| format!("- `{}`: {}", a.name, a.specialty))
            .collect();

        let handoff_tools = handoff_tools(&registry.agent_names);
        let system = self.cfg.system_prompt.clone().unwrap_or_else(|| {
            format!(
                "You are the orchestrator of the mako multi-agent system.\n\
                 Your job is to route incoming CloudEvents to the right specialist agent.\n\n\
                 Available specialists:\n{}\n\n\
                 Always transfer to a specialist. If no specialist is suitable, \
                 handle it yourself with a brief explanation.",
                agent_descriptions.join("\n")
            )
        });

        let user_msg = format!(
            "Route this event:\n- Type: `{event_type}`\n- Payload:\n```json\n{}\n```",
            serde_json::to_string_pretty(&event_data).unwrap_or_default()
        );

        let mut messages = vec![Message::system(&system), Message::user(&user_msg)];

        for _ in 0..self.cfg.max_turns {
            match self
                .provider
                .complete(&messages, &handoff_tools, &self.completion_cfg)
                .await
            {
                Err(e) => {
                    warn!(error = %e, "orchestrator LLM error");
                    break;
                }
                Ok(CompletionResult::Text(text)) => {
                    // Orchestrator answered directly
                    return AgentDecision {
                        agent_name: "orchestrator".into(),
                        event_id,
                        event_type,
                        outcome: "completed".into(),
                        summary: text,
                        tool_calls: 0,
                        turns: 1,
                        handoff_to: None,
                    };
                }
                Ok(CompletionResult::ToolCalls(calls)) => {
                    for call in &calls {
                        if let Some((to_agent, reason, _ctx)) =
                            parse_handoff(call, &registry.agent_names)
                            && let Some(specialist) = registry.get(&to_agent)
                        {
                            info!(to = %to_agent, reason = %reason, "orchestrator routing");
                            return self
                                .run_with_handoff(
                                    specialist, event_id, event_type, event_data, registry, mcp,
                                    rag, 0,
                                )
                                .await;
                        }
                        messages.push(Message::tool_result(
                            &call.id,
                            serde_json::json!({"ok": true}),
                        ));
                    }
                    let assistant_content: Vec<Value> = calls
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "type":"tool_use","id":c.id,"name":c.name,"input":c.arguments
                            })
                        })
                        .collect();
                    messages.push(Message {
                        role: "assistant".into(),
                        content: Value::Array(assistant_content),
                    });
                }
                Ok(CompletionResult::Handoff {
                    to_agent, reason, ..
                }) => {
                    if let Some(specialist) = registry.get(&to_agent) {
                        return self
                            .run_with_handoff(
                                specialist, event_id, event_type, event_data, registry, mcp, rag, 0,
                            )
                            .await;
                    }
                    warn!(to = %to_agent, reason = %reason, "orchestrator handoff to unknown agent");
                    break;
                }
            }
        }

        // Fallback: no routing achieved
        AgentDecision {
            agent_name: "orchestrator".into(),
            event_id,
            event_type,
            outcome: "no_route".into(),
            summary: "Orchestrator could not route the event to a specialist.".into(),
            tool_calls: 0,
            turns: self.cfg.max_turns,
            handoff_to: None,
        }
    }

    /// Run a specialist with chained handoff support (max 3 hops).
    #[allow(clippy::too_many_arguments)]
    async fn run_with_handoff(
        &self,
        agent: Arc<super::registry::Agent>,
        event_id: String,
        event_type: String,
        event_data: Value,
        registry: &AgentRegistry,
        mcp: &McpPool,
        rag: Option<&RagEngine>,
        hop: u32,
    ) -> AgentDecision {
        if hop > 3 {
            return AgentDecision {
                agent_name: agent.name.clone(),
                event_id,
                event_type,
                outcome: "max_hops".into(),
                summary: "Exceeded maximum handoff hops (3).".into(),
                tool_calls: 0,
                turns: 0,
                handoff_to: None,
            };
        }
        // Build peer list: all agents except the current one
        let peers: Vec<String> = registry
            .agent_names
            .iter()
            .filter(|n| *n != &agent.name)
            .cloned()
            .collect();

        let decision = AgentSession::new(Arc::clone(&agent), event_id.clone(), event_type.clone())
            .run(event_data.clone(), mcp, &peers, rag)
            .await;

        // Follow handoff if requested
        if let Some(ref next_name) = decision.handoff_to
            && let Some(next_agent) = registry.get(next_name)
        {
            info!(from = %agent.name, to = %next_name, hop, "following handoff");
            return Box::pin(self.run_with_handoff(
                next_agent,
                event_id,
                event_type,
                event_data,
                registry,
                mcp,
                rag,
                hop + 1,
            ))
            .await;
        }
        decision
    }
}
