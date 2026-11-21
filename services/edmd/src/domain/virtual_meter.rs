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

/// One `SEQ+Z37` Bestandteil eines Rechenschritts, in the UTILTS terms a
/// Berechnungsformel (Prüfidentifikator 25001) travels in — the shape makod
/// renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bestandteil {
    /// `SEQ+Z37` DE 1050 — the Rechenschrittidentifikator.
    pub schritt: u32,
    /// `CAV` under `CCI+++Z86` — `Z69` … `Z83`.
    pub operator: &'static str,
    /// `RFF+Z19` — the Messlokation it reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub messlokation: Option<String>,
    /// `RFF+Z23` — the Rechenschritt it reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schritt_ref: Option<u32>,
    /// `CAV` under `CCI+++Z87` — `Z71` Verbrauch, `Z72` Erzeugung.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub richtung: Option<&'static str>,
    /// `CCI+++Z16` Verlustfaktor Trafo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verlust_trafo: Option<String>,
    /// `CCI+++ZB2` Verlustfaktor Leitung.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verlust_leitung: Option<String>,
    /// `CCI+++ZG6` Aufteilungsfaktor Energiemenge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aufteilung: Option<String>,
}

/// The formula as UTILTS writes it: every Rechenschritt's components in step
/// order (UTILTS AHB 1.1, MIG 1.1e `SEQ+Z37`) — the same formula
/// [`Formula::eval`] computes the virtual Marktlokation with, so what the
/// market is told and what edmd computes cannot differ.
#[must_use]
pub fn bestandteile(formula: &Formula) -> Vec<Bestandteil> {
    use metering::allocation::formula::{Operand, Operator, Sign, Step};
    let leer = |schritt: u32, operator: Operator| Bestandteil {
        schritt,
        operator: operator.as_str(),
        messlokation: None,
        schritt_ref: None,
        richtung: None,
        verlust_trafo: None,
        verlust_leitung: None,
        aufteilung: None,
    };
    let mit = |schritt: u32, operator: Operator, operand: &Operand| match operand {
        Operand::Melo(m) => Bestandteil {
            messlokation: Some(m.melo.as_str().to_owned()),
            richtung: Some(m.direction.as_str()),
            verlust_trafo: m.loss_trafo.map(|f| f.get().to_string()),
            verlust_leitung: m.loss_line.map(|f| f.get().to_string()),
            aufteilung: m.split.map(|f| f.get().to_string()),
            ..leer(schritt, operator)
        },
        Operand::Step(s) => Bestandteil {
            schritt_ref: Some(s.get()),
            ..leer(schritt, operator)
        },
    };
    let mut out = Vec::new();
    for (id, step) in formula.steps() {
        let schritt = id.get();
        match step {
            Step::Sum(terms) => out.extend(terms.iter().map(|(sign, operand)| {
                let op = match sign {
                    Sign::Plus => Operator::Addition,
                    Sign::Minus => Operator::Subtraktion,
                };
                mit(schritt, op, operand)
            })),
            Step::Quotient { dividend, divisor } => {
                out.push(mit(schritt, Operator::Dividend, dividend));
                out.push(mit(schritt, Operator::Divisor, divisor));
            }
            Step::Product(factors) => {
                out.extend(factors.iter().map(|f| mit(schritt, Operator::Faktor, f)));
            }
            Step::Positive(s) => out.push(Bestandteil {
                schritt_ref: Some(s.get()),
                ..leer(schritt, Operator::Positivwert)
            }),
        }
    }
    out
}

/// `Z40` „Berechnungsformel besitzt keine Rechenoperation" for a 1:1 Lokation
/// — one step adding one Messlokation with no factor — else `Z33`
/// „Berechnungsformel angefügt" (UTILTS MIG 1.1e `STS+Z23` DE 4405).
#[must_use]
pub fn formelstatus(bestandteile: &[Bestandteil]) -> &'static str {
    match bestandteile {
        [b] if b.operator == "Z69"
            && b.messlokation.is_some()
            && b.verlust_trafo.is_none()
            && b.verlust_leitung.is_none()
            && b.aufteilung.is_none() =>
        {
            "Z40"
        }
        _ => "Z33",
    }
}

#[cfg(test)]
mod bestandteile_tests {
    use super::*;
    use metering::allocation::formula::{FlowDirection, MeloOperand, SplitFactor};

    fn melo(id: &str) -> MeloOperand {
        MeloOperand::new(id.parse().expect("a MeLo-ID"), FlowDirection::Verbrauch)
    }

    /// A Summenmessung's formula becomes one Z69 Bestandteil per Messlokation
    /// of step 1, with its register — what UTILTS writes in `SEQ+Z37`.
    #[test]
    fn a_sum_is_one_addition_per_messlokation() {
        let a = "DE0000011000000000000000012345678";
        let b = "DE0000011000000000000000087654321";
        let f = Formula::sum([melo(a), melo(b)]).expect("valid");
        let out = bestandteile(&f);
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .all(|x| x.schritt == f.result().get() && x.operator == "Z69")
        );
        assert_eq!(out[0].richtung, Some("Z71"));
        assert_eq!(formelstatus(&out), "Z33");
    }

    /// One Messlokation and nothing else is a 1:1 Lokation: `Z40`, „keine
    /// Rechenoperation"; with a split factor it is a formula again.
    #[test]
    fn one_messlokation_has_no_rechenoperation_until_it_carries_a_factor() {
        let a = "DE0000011000000000000000012345678";
        let eins = bestandteile(&Formula::melo(melo(a)).expect("valid"));
        assert_eq!(formelstatus(&eins), "Z40");
        let anteil = melo(a).split(SplitFactor::new(rust_decimal::dec!(0.25)).expect("a share"));
        let geteilt = bestandteile(&Formula::melo(anteil).expect("valid"));
        assert_eq!(geteilt[0].aufteilung.as_deref(), Some("0.25"));
        assert_eq!(formelstatus(&geteilt), "Z33");
    }
}
