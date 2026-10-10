//! What a virtual Marktlokation is computed from: a UTILTS Berechnungsformel,
//! or one participant's share of a § 42b / § 42c community.
//!
//! Both read Messlokationen, and edmd stores readings under the Marktlokation
//! they were delivered for — so a rule names, for every Messlokation it reads,
//! the MaLo its readings are filed under (`sources`).
//!
//! | Kind | Arithmetic | Source |
//! |---|---|---|
//! | [`Formula`](VirtualMeterRule::Formula) | [`metering::allocation::formula::Formula::eval`] | UTILTS AHB 1.1 Berechnungsformel (Prüfidentifikator 25001) |
//! | [`CommunityShare`](VirtualMeterRule::CommunityShare) | [`metering::allocation::community::allocate`], the tenant's `net_grid_draw` | § 42b Abs. 5 EnWG; § 42c EnWG (BNetzA Mitteilung Nr. 73, BK6-06-009) |
//!
//! The rule is stored as externally tagged JSON (`rule_json`,
//! `{"FORMULA": {…}}`), and its tag is the `rule_type` column, so the column
//! and the document cannot disagree. Externally, because a formula keys its
//! Rechenschritte by number (`{"1": …}`): an internally tagged enum buffers its
//! body before it knows the variant, and a buffered map key is a string a
//! numeric key cannot be read from.

use std::collections::BTreeMap;

use metering::allocation::community::AllocationKey;
use metering::allocation::formula::Formula;
use metering::{MaloId, MeloId};
use serde::{Deserialize, Serialize};

/// A virtual Marktlokation's definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum VirtualMeterRule {
    /// A Berechnungsformel over Messlokationen: Summenmessung, a residual load,
    /// net grid exchange, loss factors.
    Formula {
        /// The formula, as `metering` validates it.
        formula: Formula,
        /// The Marktlokation each Messlokation's readings are filed under.
        sources: BTreeMap<MeloId, MaloId>,
    },
    /// One tenant of a community supplied from a shared plant: what the tenant
    /// still drew from the public grid after its share of the plant's
    /// generation.
    CommunityShare {
        /// The plant's Messlokation; it names the community.
        plant: MeloId,
        /// This participant's Messlokation.
        tenant: MeloId,
        /// How the generation is divided. Every participant of one plant
        /// carries the same key.
        key: AllocationKey,
        /// The Marktlokation each Messlokation's readings are filed under —
        /// at least the plant and this tenant.
        sources: BTreeMap<MeloId, MaloId>,
    },
}

impl VirtualMeterRule {
    /// Every `rule_type`, in declaration order — the `CHECK` list the DDL
    /// carries.
    pub const KINDS: [&'static str; 2] = ["FORMULA", "COMMUNITY_SHARE"];

    /// The `rule_type` column: the variant the JSON document is tagged with.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Formula { .. } => Self::KINDS[0],
            Self::CommunityShare { .. } => Self::KINDS[1],
        }
    }

    /// Read a rule from its stored or submitted JSON.
    ///
    /// # Errors
    ///
    /// The document is not a rule.
    pub fn from_json(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        Self::deserialize(value)
    }

    /// The Marktlokation each Messlokation's readings are filed under.
    #[must_use]
    pub const fn sources(&self) -> &BTreeMap<MeloId, MaloId> {
        match self {
            Self::Formula { sources, .. } | Self::CommunityShare { sources, .. } => sources,
        }
    }

    /// A Messlokation the rule reads but names no Marktlokation for.
    ///
    /// Checked when a rule is stored, so a rule that could never be evaluated
    /// is refused at the door rather than at the first read.
    #[must_use]
    pub fn unsourced(&self) -> Option<MeloId> {
        let read: Vec<MeloId> = match self {
            Self::Formula { formula, .. } => formula.inputs().into_iter().map(|(m, _)| m).collect(),
            Self::CommunityShare { plant, tenant, .. } => vec![*plant, *tenant],
        };
        read.into_iter().find(|m| !self.sources().contains_key(m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metering::allocation::formula::{FlowDirection, MeloOperand};

    fn melo(n: u8) -> MeloId {
        format!("DE00012345678900000000000000000{n:02}")
            .parse()
            .expect("a MeLo")
    }

    fn malo() -> MaloId {
        "51238696012".parse().expect("a MaLo")
    }

    /// The document's tag is the `rule_type` column, and a formula survives the
    /// round trip through the stored JSON.
    #[test]
    fn the_kind_is_the_tag_of_the_document() {
        let rule = VirtualMeterRule::Formula {
            formula: Formula::sum([MeloOperand::new(melo(1), FlowDirection::Verbrauch)])
                .expect("a formula"),
            sources: BTreeMap::from([(melo(1), malo())]),
        };
        let json = serde_json::to_value(&rule).expect("serialisable");
        assert!(
            json.get(rule.kind()).is_some(),
            "tagged {}: {json}",
            rule.kind()
        );
        let back = VirtualMeterRule::from_json(&json).expect("round trip");
        assert_eq!(back, rule);
    }

    /// A rule reading a Messlokation it names no Marktlokation for cannot be
    /// evaluated, and says which one.
    #[test]
    fn an_unsourced_messlokation_is_named() {
        let rule = VirtualMeterRule::CommunityShare {
            plant: melo(1),
            tenant: melo(2),
            key: AllocationKey::Proportional,
            sources: BTreeMap::from([(melo(1), malo())]),
        };
        assert_eq!(rule.unsourced(), Some(melo(2)));
    }
}
