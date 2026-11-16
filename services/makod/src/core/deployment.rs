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

use crate::config::PartyConfig;

/// One deployment role: a Marktrolle in one Sparte. The setup interview asks
/// for the same type, so every role makod loads is one it can propose.
pub use mako_service::einrichtung::befragung::Rolle;

/// Check `rollen` against the `[[party]]` table and return the declared set,
/// refusing with every finding joined.
///
/// # Errors
///
/// Refuses, naming the role:
///
/// - an empty declaration;
/// - a declared role with no MP-ID, or with more than one;
/// - a `[[party]]` role whose deployment role is not declared;
/// - a `[[party]]` role no engine module answers for.
///
/// The tests' view of [`pruefe_rollen`]; startup reads the findings.
#[cfg(test)]
pub fn validate_rollen(
    rollen: &[Rolle],
    parties: &[PartyConfig],
) -> anyhow::Result<BTreeSet<Rolle>> {
    let (declared, befunde) = pruefe_rollen(rollen, parties);
    if !befunde.is_empty() {
        anyhow::bail!(
            "{}",
            befunde
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    Ok(declared)
}

/// The source of the one-MP-ID-per-role rule.
const EINE_MP_ID: &str = "BDEW-AWH Identifikatoren V1.2, Kap. 2.1 (S. 4)";

/// The deployment's role checks as findings — every defect, each at its path —
/// and the declared set.
#[must_use]
pub fn pruefe_rollen(
    rollen: &[Rolle],
    parties: &[PartyConfig],
) -> (BTreeSet<Rolle>, Vec<mako_service::Befund>) {
    use mako_service::befund::{Befund, Bereich};
    let mut out = Vec::new();
    if rollen.is_empty() {
        out.push(
            Befund::blockiert(
                "deployment.rollen",
                format!(
                    "makod.toml declares no deployment role. Name the roles this instance plays:\n\
                 \n\
                 [deployment]\n\
                 rollen = [\"nb-strom\"]  # {}\n\
                 \n\
                 A makod that registers no engine module routes nothing and dead-letters \
                 every inbound message.",
                    Rolle::ALLE.map(Rolle::token).join(", ")
                ),
            )
            .bereich(Bereich::Identitaeten),
        );
        return (BTreeSet::new(), out);
    }
    let declared: BTreeSet<Rolle> = rollen.iter().copied().collect();

    // Each deployment role → the distinct MP-IDs the party table gives it.
    let mut mp_ids: BTreeMap<Rolle, BTreeSet<&str>> = BTreeMap::new();
    for (i, party) in parties.iter().enumerate() {
        for role in &party.roles {
            let Some(rolle) = Rolle::von_kuerzel(role) else {
                out.push(
                    Befund::blockiert(
                        format!("party[{i}].roles"),
                        format!(
                            "[[party]] mp_id = {:?} declares the role {role:?}, which no engine \
                             module answers for. Remove it from `roles`.",
                            party.mp_id
                        ),
                    )
                    .bereich(Bereich::Identitaeten),
                );
                continue;
            };
            if !declared.contains(&rolle) {
                out.push(
                    Befund::blockiert(
                        format!("party[{i}].roles"),
                        format!(
                            "[[party]] mp_id = {:?} holds an MP-ID for {role:?} ({rolle}), but \
                             [deployment] rollen does not declare {rolle}. Declare the role or \
                             remove the MP-ID — a deployment names itself only for the roles it plays.",
                            party.mp_id
                        ),
                    )
                    .bereich(Bereich::Identitaeten),
                );
                continue;
            }
            mp_ids
                .entry(rolle)
                .or_default()
                .insert(party.mp_id.as_str());
        }
    }

    for rolle in &declared {
        let ids = mp_ids.get(rolle).map(BTreeSet::len).unwrap_or(0);
        if ids != 1 {
            out.push(
                Befund::blockiert(
                    "deployment.rollen",
                    format!(
                        "[deployment] rollen declares {rolle}, which has {ids} MP-IDs in [[party]] \
                         ({:?}); it needs exactly one (BDEW-AWH Identifikatoren V1.2 Kap. 2.1: „Für \
                         jede Sparte (Gas bzw. Strom) und jede Rolle hat ein Markteilnehmer genau \
                         eine MP-ID.\"). Party roles for {rolle}: {:?}.",
                        mp_ids.get(rolle).cloned().unwrap_or_default(),
                        rolle.alle_kuerzel(),
                    ),
                )
                .bereich(Bereich::Identitaeten)
                .quelle(EINE_MP_ID),
            );
        }
    }

    (declared, out)
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
            as4: None,
        }
    }

    fn err(rollen: &[Rolle], parties: &[PartyConfig]) -> String {
        validate_rollen(rollen, parties)
            .expect_err("the declaration must be refused")
            .to_string()
    }

    #[test]
    fn tokens_parse_and_print_the_same() {
        for rolle in Rolle::ALLE {
            let parsed: Rolle = serde_json::from_value(serde_json::json!(rolle.token()))
                .expect("every token parses");
            assert_eq!(parsed, rolle);
        }
        assert!(serde_json::from_value::<Rolle>(serde_json::json!("lf")).is_err());
    }

    #[test]
    fn every_party_role_maps_to_one_deployment_role() {
        for rolle in Rolle::ALLE {
            for abbrev in rolle.alle_kuerzel() {
                assert_eq!(Rolle::von_kuerzel(abbrev), Some(rolle), "{abbrev}");
            }
        }
        assert_eq!(Rolle::von_kuerzel("gmsb"), Some(Rolle::MsbGas));
        assert_eq!(Rolle::von_kuerzel("BKV"), Some(Rolle::BkvStrom));
        assert_eq!(Rolle::von_kuerzel("BIKO"), None);
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
        let msg = err(
            &[Rolle::LfStrom],
            &[party("9900001000001", &["LF", "BIKO"])],
        );
        assert!(msg.contains("BIKO"), "{msg}");
    }

    /// Two defects are two findings, each at its path.
    #[test]
    fn every_defect_is_its_own_finding() {
        let (_, befunde) = pruefe_rollen(
            &[Rolle::LfStrom, Rolle::LfGas],
            &[party("9900001000001", &["LF", "NB"])],
        );
        let pfade: Vec<&str> = befunde.iter().map(|b| b.pfad.as_str()).collect();
        assert_eq!(
            pfade,
            ["party[0].roles", "deployment.rollen"],
            "{befunde:?}"
        );
        assert!(
            befunde[1]
                .quelle
                .as_deref()
                .is_some_and(|q| q.contains("Kap. 2.1"))
        );
    }

    /// Every MaBiS command only a BKV may send is reachable under exactly one
    /// deployment role, the BKV's.
    ///
    /// Each role is declared the way an operator does it — `rollen` plus one
    /// `[[party]]` MP-ID — and must pass the boot's role checks; the command is
    /// then licensed against the engine roles that `[[party]]` table yields. A
    /// command no declarable role reaches is catalogued and unsendable.
    #[test]
    fn every_bkv_only_command_is_reachable_under_the_bkv_role_alone() {
        use mako_engine::marktrolle::Marktrolle;

        let bkv_only: Vec<&str> = crate::commands_api::COMMAND_REGISTRY
            .iter()
            .filter(|d| d.permitted_roles == [Marktrolle::Bkv])
            .filter(|d| d.workflow.is_some_and(|w| w.starts_with("mabis-")))
            .map(|d| d.name)
            .collect();
        assert!(
            bkv_only.contains(&"mabis.abrechnung.einleiten"),
            "{bkv_only:?}"
        );
        for command in bkv_only {
            let mut reaching = Vec::new();
            for rolle in Rolle::ALLE {
                let mp_id = if rolle.token().ends_with("-gas") {
                    "9800001000001"
                } else {
                    "9900001000001"
                };
                let parties = [party(mp_id, &[rolle.alle_kuerzel()[0]])];
                validate_rollen(&[rolle], &parties)
                    .unwrap_or_else(|e| panic!("{rolle} must be declarable: {e}"));
                let registry = crate::party_registry::MpIdRegistry::from_config(&parties)
                    .expect("one MP-ID per role is a valid party table");
                let engine_roles = registry.deployment_role_strings();
                if crate::commands_api::validate_command(command, None, &engine_roles).is_ok() {
                    reaching.push(rolle.token());
                }
            }
            assert_eq!(reaching, ["bkv-strom"], "{command}");
        }
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
