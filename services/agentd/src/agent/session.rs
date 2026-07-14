//! ReAct agent session with handoff support.

use std::sync::Arc;

use serde_json::Value;
use tracing::{info, instrument, warn};

use super::registry::Agent;
use crate::llm::{CompletionResult, Message, ToolDef, handoff_tools, parse_handoff};
use crate::mcp::McpPool;
use crate::rag::RagEngine;

/// Result of an agent session.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentDecision {
    pub agent_name: String,
    pub event_id: String,
    pub event_type: String,
    pub outcome: String, // "completed" | "handoff:{target}" | "max_turns" | "error"
    pub summary: String,
    pub tool_calls: usize,
    pub turns: u32,
    pub handoff_to: Option<String>,
}

impl AgentDecision {
    pub fn to_cloud_event(&self, tenant: &str) -> Value {
        serde_json::json!({
            "specversion": "1.0",
            "type": "de.agent.decision.made",
            "source": format!("agentd/{tenant}/{}", self.agent_name),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            "data": self,
        })
    }
}

pub struct AgentSession {
    pub agent: Arc<Agent>,
    pub event_id: String,
    pub event_type: String,
}

impl AgentSession {
    pub fn new(agent: Arc<Agent>, event_id: String, event_type: String) -> Self {
        Self {
            agent,
            event_id,
            event_type,
        }
    }

    /// Run the ReAct loop.
    ///
    /// `peer_agents` — names of other agents this agent can hand off to.
    /// `rag` — optional RAG engine for background knowledge injection.
    #[instrument(skip(self, mcp, rag), fields(agent = %self.agent.name, event_id = %self.event_id))]
    pub async fn run(
        &self,
        event_data: Value,
        mcp: &McpPool,
        peer_agents: &[String],
        rag: Option<&RagEngine>,
    ) -> AgentDecision {
        // Build available tools = MCP tools + handoff tools
        let mcp_tools: Vec<ToolDef> = mcp.tools_for_servers(&self.agent.mcp_servers);
        let handoffs = handoff_tools(peer_agents);
        let all_tools: Vec<ToolDef> = mcp_tools.iter().chain(handoffs.iter()).cloned().collect();

        // Build system prompt (optionally prepend RAG context)
        let rag_context = match rag {
            Some(r) if self.agent.use_rag => {
                r.query(&format!(
                    "{} {}",
                    self.event_type,
                    event_data
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                ))
                .await
            }
            _ => String::new(),
        };

        let system = if rag_context.is_empty() {
            self.agent.system_prompt.clone()
        } else {
            format!("{}\n\n{}", rag_context, self.agent.system_prompt)
        };

        let user_msg = format!(
            "**Event trigger**\n- Type: `{}`\n- ID: `{}`\n- Payload:\n```json\n{}\n```\n\n\
             Analyse and take the appropriate action. Think step by step.",
            self.event_type,
            self.event_id,
            serde_json::to_string_pretty(&event_data).unwrap_or_default()
        );

        let mut messages = vec![Message::system(&system), Message::user(&user_msg)];
        let mut turns = 0u32;
        let mut tool_calls = 0usize;

        loop {
            if turns >= self.agent.max_turns {
                warn!(agent = %self.agent.name, turns, "max_turns reached");
                return AgentDecision {
                    agent_name: self.agent.name.clone(),
                    event_id: self.event_id.clone(),
                    event_type: self.event_type.clone(),
                    outcome: "max_turns".into(),
                    summary: format!(
                        "Reached max_turns ({}) without conclusion.",
                        self.agent.max_turns
                    ),
                    tool_calls,
                    turns,
                    handoff_to: None,
                };
            }

            match self
                .agent
                .provider
                .complete(&messages, &all_tools, &self.agent.completion_cfg)
                .await
            {
                Err(e) => {
                    warn!(error = %e, "LLM error");
                    return AgentDecision {
                        agent_name: self.agent.name.clone(),
                        event_id: self.event_id.clone(),
                        event_type: self.event_type.clone(),
                        outcome: "error".into(),
                        summary: format!("LLM error: {e}"),
                        tool_calls,
                        turns,
                        handoff_to: None,
                    };
                }

                Ok(CompletionResult::Text(answer)) => {
                    info!(agent = %self.agent.name, turns, tool_calls, "session completed");
                    return AgentDecision {
                        agent_name: self.agent.name.clone(),
                        event_id: self.event_id.clone(),
                        event_type: self.event_type.clone(),
                        outcome: "completed".into(),
                        summary: answer,
                        tool_calls,
                        turns,
                        handoff_to: None,
                    };
                }

                Ok(CompletionResult::Handoff {
                    to_agent,
                    reason,
                    context: _,
                }) => {
                    info!(agent = %self.agent.name, to = %to_agent, reason = %reason, "handoff");
                    return AgentDecision {
                        agent_name: self.agent.name.clone(),
                        event_id: self.event_id.clone(),
                        event_type: self.event_type.clone(),
                        outcome: format!("handoff:{to_agent}"),
                        summary: reason,
                        tool_calls,
                        turns,
                        handoff_to: Some(to_agent),
                    };
                }

                Ok(CompletionResult::ToolCalls(calls)) => {
                    // Add assistant's tool-use turn to conversation
                    let assistant_content: Vec<Value> = calls
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "type": "tool_use", "id": c.id, "name": c.name, "input": c.arguments
                            })
                        })
                        .collect();
                    messages.push(Message {
                        role: "assistant".into(),
                        content: Value::Array(assistant_content),
                    });

                    // Check for handoffs first
                    for call in &calls {
                        if let Some((to_agent, reason, _ctx)) = parse_handoff(call, peer_agents) {
                            messages.push(Message::tool_result(
                                &call.id,
                                serde_json::json!({"transferred": true}),
                            ));
                            info!(to = %to_agent, reason = %reason, "handoff via tool");
                            return AgentDecision {
                                agent_name: self.agent.name.clone(),
                                event_id: self.event_id.clone(),
                                event_type: self.event_type.clone(),
                                outcome: format!("handoff:{to_agent}"),
                                summary: reason,
                                tool_calls,
                                turns,
                                handoff_to: Some(to_agent),
                            };
                        }

                        // Execute MCP tool
                        let result = match mcp.call_tool(&call.name, call.arguments.clone()).await {
                            Ok(r) => r,
                            Err(e) => {
                                warn!(tool = %call.name, error = %e, "tool call failed");
                                serde_json::json!({"error": e.to_string()})
                            }
                        };
                        tool_calls += 1;
                        messages.push(Message::tool_result(&call.id, result));
                    }
                    turns += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_decision(agent_name: &str, outcome: &str) -> AgentDecision {
        AgentDecision {
            agent_name: agent_name.to_owned(),
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: "de.test.event".into(),
            outcome: outcome.to_owned(),
            summary: "test summary".into(),
            tool_calls: 3,
            turns: 2,
            handoff_to: None,
        }
    }

    #[test]
    fn to_cloud_event_required_fields() {
        let d = make_decision("mako-agent", "completed");
        let ce = d.to_cloud_event("9910000000002");
        assert_eq!(ce["specversion"], "1.0");
        assert_eq!(ce["type"], "de.agent.decision.made");
        assert!(ce["id"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(ce["time"].as_str().is_some_and(|s| s.contains('T')));
        let src = ce["source"].as_str().unwrap();
        assert!(src.contains("9910000000002"), "source must contain tenant");
        assert!(src.contains("mako-agent"), "source must contain agent name");
    }

    #[test]
    fn to_cloud_event_data_contains_outcome() {
        let d = make_decision("eeg-agent", "handoff:billing-agent");
        let ce = d.to_cloud_event("tenant-x");
        assert_eq!(ce["data"]["agent_name"], "eeg-agent");
        assert_eq!(ce["data"]["outcome"], "handoff:billing-agent");
        assert_eq!(ce["data"]["tool_calls"], 3);
        assert_eq!(ce["data"]["turns"], 2);
    }

    #[test]
    fn decision_is_clone() {
        let d = make_decision("mako-agent", "completed");
        let d2 = d.clone();
        assert_eq!(d.agent_name, d2.agent_name);
        assert_eq!(d.event_id, d2.event_id);
    }
}
