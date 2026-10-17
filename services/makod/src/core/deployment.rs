//! `[deployment] rollen` — the Marktrollen this `makod` instance plays.
//!
//! The declaration is the one place a deployment's role scope lives. It selects
//! the engine modules at startup ([`crate::startup::modules_for`]) and is held
//! against the `[[party]]` identity table: BDEW-AWH Identifikatoren V1.2
//! Kap. 2.1 (S. 4) — „Für jede Sparte (Gas bzw. Strom) und jede Rolle hat ein
//! Markteilnehmer genau eine MP-ID." So every declared role needs exactly one
//! MP-ID, and an MP-ID for a role the deployment does not declare is refused:
//! it would name the operator as a party to processes no module answers.
//!
//! A missing or empty `rollen` refuses startup. A daemon that registers no
//! module routes nothing and dead-letters every inbound message.
//!
//! The declaration selects which registrations run; every build links every
//! domain crate. Which principal may read which process is Cedar's decision
//! (`core/cedar_authz.rs`), not this one's.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::config::PartyConfig;

/// One deployment role: a Marktrolle in one Sparte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rolle {
    /// Lieferant Strom.
    LfStrom,
    /// Lieferant Gas.
    LfGas,
    /// Netzbetreiber Strom (VNB, ANB, and the LPB of NZR-EMob, whose wire role is NB).
    NbStrom,
    /// Gasnetzbetreiber.
    NbGas,
    /// Messstellenbetreiber Strom.
    MsbStrom,
    /// Messstellenbetreiber Gas.
    MsbGas,
    /// Energieserviceanbieter (WiM Strom Teil 2), Strom only.
    EsaStrom,
}

impl Rolle {
    /// Every deployment role, in declaration order.
    pub const ALL: [Self; 7] = [
        Self::LfStrom,
        Self::LfGas,
        Self::NbStrom,
        Self::NbGas,
        Self::MsbStrom,
        Self::MsbGas,
        Self::EsaStrom,
    ];

    /// The token `[deployment] rollen` spells this role with.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::LfStrom => "lf-strom",
            Self::LfGas => "lf-gas",
            Self::NbStrom => "nb-strom",
            Self::NbGas => "nb-gas",
            Self::MsbStrom => "msb-strom",
            Self::MsbGas => "msb-gas",
            Self::EsaStrom => "esa-strom",
        }
    }

    /// The `[[party]]` role abbreviations whose MP-ID is this role's MP-ID.
    ///
    /// The AHB sub-qualifiers (`ANB`/`VNB`, `NMSB`/`AMSB`) name the same
    /// Marktrolle in one Sparte, so they share its one MP-ID.
    #[must_use]
    pub const fn party_roles(self) -> &'static [&'static str] {
        match self {
            Self::LfStrom => &["LF"],
            Self::LfGas => &["LFG"],
            Self::NbStrom => &["NB", "ANB", "VNB"],
            Self::NbGas => &["GNB"],
            Self::MsbStrom => &["MSB", "NMSB", "AMSB"],
            Self::MsbGas => &["GMSB"],
            Self::EsaStrom => &["ESA"],
        }
    }

    /// The deployment role a `[[party]]` role abbreviation belongs to.
    ///
    /// `None` for a Marktrolle no engine module answers for (`BKV`, `UNB`,
    /// `BIKO`, `FNB`, `MGV`, `KN`, `DP`, `EIV`, `RB`).
    #[must_use]
    pub fn of_party_role(abbrev: &str) -> Option<Self> {
        let upper = abbrev.to_uppercase();
        Self::ALL
            .into_iter()
            .find(|r| r.party_roles().contains(&upper.as_str()))
    }
}

impl std::fmt::Display for Rolle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.token())
    }
}

/// Check `rollen` against the `[[party]]` table and return the declared set.
///
/// # Errors
///
/// Refuses, naming the role:
///
/// - an empty declaration;
/// - a declared role with no MP-ID, or with more than one;
/// - a `[[party]]` role whose deployment role is not declared;
/// - a `[[party]]` role no engine module answers for.
pub fn validate_rollen(
    rollen: &[Rolle],
    parties: &[PartyConfig],
) -> anyhow::Result<BTreeSet<Rolle>> {
    anyhow::ensure!(
        !rollen.is_empty(),
        "makod.toml declares no deployment role. Name the roles this instance plays:\n\
         \n\
         [deployment]\n\
         rollen = [\"nb-strom\"]  # lf-strom, lf-gas, nb-strom, nb-gas, msb-strom, msb-gas, esa-strom\n\
         \n\
         A makod that registers no engine module routes nothing and dead-letters \
         every inbound message."
    );
    let declared: BTreeSet<Rolle> = rollen.iter().copied().collect();

    // Each deployment role → the distinct MP-IDs the party table gives it.
    let mut mp_ids: BTreeMap<Rolle, BTreeSet<&str>> = BTreeMap::new();
    for party in parties {
        for role in &party.roles {
            let Some(rolle) = Rolle::of_party_role(role) else {
                anyhow::bail!(
                    "[[party]] mp_id = {:?} declares the role {role:?}, which no engine \
                     module answers for. Remove it from `roles`.",
                    party.mp_id
                );
            };
            anyhow::ensure!(
                declared.contains(&rolle),
                "[[party]] mp_id = {:?} holds an MP-ID for {role:?} ({rolle}), but \
                 [deployment] rollen does not declare {rolle}. Declare the role or \
                 remove the MP-ID — a deployment names itself only for the roles it plays.",
                party.mp_id
            );
            mp_ids
                .entry(rolle)
                .or_default()
                .insert(party.mp_id.as_str());
        }
    }

    for rolle in &declared {
        let ids = mp_ids.get(rolle).map(BTreeSet::len).unwrap_or(0);
        anyhow::ensure!(
            ids == 1,
            "[deployment] rollen declares {rolle}, which has {ids} MP-IDs in [[party]] \
             ({:?}); it needs exactly one (BDEW-AWH Identifikatoren V1.2 Kap. 2.1: „Für \
             jede Sparte (Gas bzw. Strom) und jede Rolle hat ein Markteilnehmer genau \
             eine MP-ID.\"). Party roles for {rolle}: {:?}.",
            mp_ids.get(rolle).cloned().unwrap_or_default(),
            rolle.party_roles(),
        );
    }

    Ok(declared)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn party(mp_id: &str, roles: &[&str]) -> PartyConfig {
        PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            primary: false,
            agency: None,
        }
    }

    fn err(rollen: &[Rolle], parties: &[PartyConfig]) -> String {
        validate_rollen(rollen, parties)
            .expect_err("the declaration must be refused")
            .to_string()
    }

    #[test]
    fn tokens_parse_and_print_the_same() {
        for rolle in Rolle::ALL {
            let parsed: Rolle = serde_json::from_value(serde_json::json!(rolle.token()))
                .expect("every token parses");
            assert_eq!(parsed, rolle);
        }
        assert!(serde_json::from_value::<Rolle>(serde_json::json!("lf")).is_err());
    }

    #[test]
    fn every_party_role_maps_to_one_deployment_role() {
        for rolle in Rolle::ALL {
            for abbrev in rolle.party_roles() {
                assert_eq!(Rolle::of_party_role(abbrev), Some(rolle), "{abbrev}");
            }
        }
        assert_eq!(Rolle::of_party_role("gmsb"), Some(Rolle::MsbGas));
        assert_eq!(Rolle::of_party_role("BKV"), None);
    }

    #[test]
    fn a_missing_declaration_is_refused() {
        assert!(err(&[], &[party("9900001000001", &["NB"])]).contains("no deployment role"));
    }

    #[test]
    fn a_declared_role_without_an_mp_id_is_refused() {
        let msg = err(
            &[Rolle::NbStrom, Rolle::LfStrom],
            &[party("9900001000001", &["NB"])],
        );
        assert!(
            msg.contains("lf-strom") && msg.contains("0 MP-IDs"),
            "{msg}"
        );
    }

    #[test]
    fn two_mp_ids_for_one_role_are_refused() {
        let msg = err(
            &[Rolle::NbStrom],
            &[
                party("9900001000001", &["NB"]),
                party("9900001000002", &["VNB"]),
            ],
        );
        assert!(
            msg.contains("nb-strom") && msg.contains("2 MP-IDs"),
            "{msg}"
        );
    }

    #[test]
    fn an_mp_id_for_an_undeclared_role_is_refused() {
        let msg = err(
            &[Rolle::LfStrom],
            &[
                party("9900001000001", &["LF"]),
                party("9900001000002", &["NB"]),
            ],
        );
        assert!(msg.contains("\"NB\"") && msg.contains("nb-strom"), "{msg}");
    }

    #[test]
    fn a_party_role_no_module_answers_for_is_refused() {
        let msg = err(&[Rolle::LfStrom], &[party("9900001000001", &["LF", "BKV"])]);
        assert!(msg.contains("BKV"), "{msg}");
    }

    #[test]
    fn one_mp_id_may_hold_several_roles() {
        let set = validate_rollen(
            &[Rolle::NbStrom, Rolle::LfStrom, Rolle::MsbStrom],
            &[party("9900001000001", &["NB", "LF", "MSB"])],
        )
        .expect("one Strom MP-ID covering three roles is valid");
        assert_eq!(set.len(), 3);
    }
}
