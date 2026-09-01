//! Missing final ALOCAT triage is event translation, not inference.

use agentplane::core::{
    DeadlineSpec, Justification, Outcome, Priority, Skill, SkillDescriptor, SkillError, Tainted,
    TaskSpec,
};
use agentplane::runtime::StepCtx;
use serde_json::{Value, json};

const TRIAGE_OBLIGATION: &str = "gabi-final-allocation-review";
const TRIAGE_AUDIENCE: &str = "gas-operations";
const TRIAGE_ESCALATION: &str = "mako-operations";

/// Opens operator clearing work for a deterministic final-allocation breach.
#[derive(Debug, Default)]
pub struct GabiAllocationTriage;

impl GabiAllocationTriage {
    pub const CAPABILITY: &'static str = "gabi.gas.balancing";
    pub const NAME: &'static str = "gabi-gas-agent";
    pub const TRIAGE_AUDIENCE: &'static str = TRIAGE_AUDIENCE;
    pub const TRIAGE_ESCALATION: &'static str = TRIAGE_ESCALATION;

    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn required<'a>(payload: &'a Value, field: &str) -> Result<&'a str, SkillError> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| SkillError::Other(format!("de.gabi.alocat.missing lacks `{field}`")))
}

#[async_trait::async_trait]
impl Skill for GabiAllocationTriage {
    fn descriptor(&self) -> SkillDescriptor {
        SkillDescriptor::new(Self::NAME).provides(Self::CAPABILITY)
    }

    async fn invoke(
        &self,
        cx: &mut StepCtx<'_>,
        input: Tainted<Value>,
    ) -> Result<Outcome, SkillError> {
        let payload = input.peek();
        let gas_day = required(payload, "gas_day")?.to_owned();
        let sender_eic = required(payload, "sender_eic")?.to_owned();
        let receiver_eic = required(payload, "receiver_eic")?.to_owned();
        let deadline_label = required(payload, "deadline_label")?.to_owned();
        let synthetic_pid = required(payload, "synthetic_pid")?.to_owned();

        cx.note(format!(
            "final ALOCAT missing for GasDay {gas_day}; deadline={deadline_label}; sender={sender_eic}; receiver={receiver_eic}"
        ))
        .await
        .map_err(|error| SkillError::Other(format!("record GaBi triage note: {error}")))?;

        cx.deadline(
            TRIAGE_OBLIGATION,
            &DeadlineSpec::new("working-days", json!({ "n": 1 })),
            None,
        )
        .await
        .map_err(|error| SkillError::Other(format!("register GaBi triage deadline: {error}")))?;

        let justification = json!({
            "gas_day": gas_day,
            "sender_eic": sender_eic,
            "receiver_eic": receiver_eic,
            "deadline_label": deadline_label,
            "synthetic_pid": synthetic_pid,
        });
        let task = TaskSpec::new(
            "gabi.final-allocation.missing",
            Justification::new(
                "A binding final ALOCAT is missing. Open and track the clearing case.",
                justification,
            ),
            TRIAGE_OBLIGATION,
        )
        .role(TRIAGE_AUDIENCE)
        .priority(Priority::Urgent)
        .on_expiry(agentplane::core::OnExpiry::Escalate)
        .escalate_to(TRIAGE_ESCALATION);

        cx.open_task(&task)
            .await
            .map_err(|error| SkillError::Other(format!("open GaBi triage task: {error}")))?;

        let answer = json!({
            "gas_day": gas_day,
            "status": "MISSING_FINAL_ALLOCATION",
            "sender_eic": sender_eic,
            "receiver_eic": receiver_eic,
            "action": "OPEN_CLEARING_CASE",
            "legal_basis": "KoV §6.4",
        });
        Ok(Outcome::done(input.map(|_| answer)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skill_identity_matches_the_manifest() {
        assert_eq!(GabiAllocationTriage::NAME, "gabi-gas-agent");
        assert_eq!(GabiAllocationTriage::CAPABILITY, "gabi.gas.balancing");
    }
}
