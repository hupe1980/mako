//! Agent registry — holds all named specialists + the orchestrator.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::config::{AgentConfig, AgentdConfig};
use crate::llm::{CompletionConfig, LlmProvider, build_provider};

/// A fully resolved, ready-to-run agent definition.
pub struct Agent {
    pub name: String,
    pub specialty: String,
    pub system_prompt: String,
    pub provider: Arc<dyn LlmProvider>,
    pub completion_cfg: CompletionConfig,
    pub mcp_servers: Vec<String>,
    pub trigger_patterns: Vec<String>,
    pub max_turns: u32,
    pub use_rag: bool,
}

impl Agent {
    pub fn matches_trigger(&self, ce_type: &str) -> bool {
        if self.trigger_patterns.is_empty() {
            return false;
        }
        self.trigger_patterns.iter().any(|p| glob_match(p, ce_type))
    }
}

/// Registry of all agents, keyed by name.
pub struct AgentRegistry {
    pub agents: HashMap<String, Arc<Agent>>,
    /// Ordered list of agent names for routing / handoff tools.
    pub agent_names: Vec<String>,
}

impl AgentRegistry {
    pub fn build(cfg: &AgentdConfig) -> Result<Self> {
        let mut agents = HashMap::new();
        let mut agent_names = Vec::new();

        for ac in &cfg.agents {
            let agent = build_agent(ac, cfg)?;
            agent_names.push(ac.name.clone());
            agents.insert(ac.name.clone(), Arc::new(agent));
        }

        Ok(Self {
            agents,
            agent_names,
        })
    }

    pub fn get(&self, name: &str) -> Option<Arc<Agent>> {
        self.agents.get(name).cloned()
    }

    /// Find the best specialist for `ce_type` via trigger pattern matching.
    /// Returns `None` if no specialist matches (use orchestrator).
    pub fn find_specialist(&self, ce_type: &str) -> Option<Arc<Agent>> {
        for name in &self.agent_names {
            if let Some(a) = self.agents.get(name)
                && a.matches_trigger(ce_type)
            {
                return Some(Arc::clone(a));
            }
        }
        None
    }
}

fn build_agent(ac: &AgentConfig, cfg: &AgentdConfig) -> Result<Agent> {
    let provider_cfg = cfg.providers.get(&ac.provider).ok_or_else(|| {
        anyhow::anyhow!(
            "agent '{}' references unknown provider '{}'",
            ac.name,
            ac.provider
        )
    })?;
    let provider = build_provider(&ac.provider, provider_cfg);

    let default_prompt = format!(
        "You are the `{}` specialist agent for the mako German energy market platform.\n\
         Specialty: {}\n\n\
         Always reason step-by-step before taking action. Explain your reasoning.\n\
         You may call `transfer_to_orchestrator` to escalate cases outside your specialty.",
        ac.name, ac.specialty
    );

    Ok(Agent {
        name: ac.name.clone(),
        specialty: ac.specialty.clone(),
        system_prompt: ac.system_prompt.clone().unwrap_or(default_prompt),
        provider,
        completion_cfg: CompletionConfig {
            model: ac.model.clone(),
            max_tokens: 4096,
        },
        mcp_servers: ac.mcp_servers.clone(),
        trigger_patterns: ac.trigger_patterns.clone(),
        max_turns: ac.max_turns,
        use_rag: ac.use_rag,
    })
}

/// Simple glob matching: `*` matches any sequence, `?` matches any single char.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut pi = 0usize;
    let mut vi = 0usize;
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    let mut star_pi: Option<usize> = None;
    let mut star_vi = 0usize;

    while vi < v.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_exact_match() {
        assert!(glob_match(
            "de.mako.process.initiated",
            "de.mako.process.initiated"
        ));
        assert!(!glob_match(
            "de.mako.process.initiated",
            "de.mako.process.completed"
        ));
    }

    #[test]
    fn glob_trailing_wildcard() {
        assert!(glob_match("de.mako.process.*", "de.mako.process.initiated"));
        assert!(glob_match("de.mako.process.*", "de.mako.process.completed"));
        assert!(!glob_match(
            "de.mako.process.*",
            "de.invoic.receipt.disputed"
        ));
    }

    #[test]
    fn glob_mid_wildcard() {
        assert!(glob_match("de.mako.*", "de.mako.process.initiated"));
        assert!(glob_match("de.mako.*", "de.mako.aperak.sent"));
        assert!(!glob_match("de.mako.*", "de.invoic.receipt.disputed"));
    }

    #[test]
    fn glob_star_matches_everything() {
        assert!(glob_match("*", "de.mako.process.initiated"));
        assert!(glob_match("*", ""));
    }

    #[test]
    fn glob_empty_pattern_matches_only_empty() {
        assert!(glob_match("", ""));
        assert!(!glob_match("", "de.mako.something"));
    }

    #[test]
    fn agent_matches_trigger_empty_patterns() {
        // An agent with no trigger_patterns should never match
        let agent = Agent {
            name: "test".into(),
            specialty: "test".into(),
            system_prompt: "test".into(),
            provider: crate::llm::build_provider(
                "openai",
                &crate::config::ProviderConfig {
                    backend: "openai".into(),
                    api_base: None,
                    api_key: String::new(),
                    aws_region: None,
                    aws_access_key_id: None,
                    aws_secret_access_key: None,
                },
            ),
            completion_cfg: crate::llm::CompletionConfig {
                model: "gpt-4o".into(),
                max_tokens: 100,
            },
            mcp_servers: vec![],
            trigger_patterns: vec![],
            max_turns: 5,
            use_rag: false,
        };
        assert!(!agent.matches_trigger("de.mako.process.initiated"));
        assert!(!agent.matches_trigger("de.invoic.receipt.disputed"));
    }

    #[test]
    fn agent_matches_trigger_with_glob() {
        let agent = Agent {
            name: "eeg-agent".into(),
            specialty: "EEG".into(),
            system_prompt: "test".into(),
            provider: crate::llm::build_provider(
                "openai",
                &crate::config::ProviderConfig {
                    backend: "openai".into(),
                    api_base: None,
                    api_key: String::new(),
                    aws_region: None,
                    aws_access_key_id: None,
                    aws_secret_access_key: None,
                },
            ),
            completion_cfg: crate::llm::CompletionConfig {
                model: "gpt-4o".into(),
                max_tokens: 100,
            },
            mcp_servers: vec![],
            trigger_patterns: vec!["de.eeg.*".into(), "de.mako.process.initiated".into()],
            max_turns: 10,
            use_rag: false,
        };
        assert!(agent.matches_trigger("de.eeg.anlage.foerderung_auslaufend"));
        assert!(agent.matches_trigger("de.mako.process.initiated"));
        assert!(!agent.matches_trigger("de.invoic.receipt.disputed"));
    }
}
