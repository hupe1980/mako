//! A Bedingung that predicates a Marktrolle of an MP-ID — „Wenn MP-ID in SG2
//! NAD+MS mit Rolle MSB nicht vorhanden" — is not a presence test of the
//! `NAD` segment. The role an MP-ID holds is not on the wire: the validator
//! decides it from the caller's `Parteirollen` and leaves it undecided where
//! the caller does not know the role. Read as „`NAD+MS` (not) present" it
//! decided every such Bedingung the same way for every sender.
//!
//! ORDERS AHB 1.1b Kap. 4.13 (S. 91) puts `SG29 IMD` code `Z46` on
//! `X [45] ∧ [492]` and `Z47` on `X [15] ∧ [492]`, with `[45]` „Wenn MP-ID in
//! SG2 NAD+MS mit Rolle MSB nicht vorhanden" and `[15]` „… mit Rolle MSB
//! vorhanden": Z46 is the code a LF or NB sends, Z47 the one a MSB sends.

#![cfg(feature = "orders")]

use edi_energy::profile::conditions::Voraussetzung;
use edi_energy::profile::{Parteirollen, SkeletonParties};
use edi_energy::{EdiEnergyMessage, MessageType, Profile, ReleaseRegistry};
use time::macros::date;

/// A LF's BDEW-Codenummer, as the other ORDERS tests use it.
const LF: &str = "9900000000002";
/// A MSB's BDEW-Codenummer.
const MSB: &str = "9900000000003";

/// The two shipped ORDERS releases and a day each is in force.
const RELEASES: [(&str, time::Date); 2] = [
    ("1.4b", date!(2026 - 06 - 01)),
    ("1.4c", date!(2026 - 11 - 13)),
];

fn orders(release: &str) -> &'static Profile {
    ReleaseRegistry::global()
        .profiles_for(MessageType::Orders)
        .find(|p| p.release().as_str() == release)
        .unwrap_or_else(|| panic!("ORDERS {release} is shipped"))
}

fn rolle(absender: Option<&str>) -> Parteirollen {
    Parteirollen {
        absender: absender.map(str::to_owned),
        empfaenger: Some("MSB".to_owned()),
    }
}

/// The 17004 skeleton from `sender` with its `SG29 IMD` Grund der Anforderung
/// set to `grund`, and the issues that bar sending it, judged with `rollen`.
fn anfrage_17004(
    (release, tag): (&str, time::Date),
    sender: &str,
    grund: &str,
    rollen: &Parteirollen,
) -> Vec<String> {
    let profile = orders(release);
    let af = profile.anwendungsfall(17004).expect("17004 is a column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: sender.to_owned(),
                receiver: MSB.to_owned(),
            },
        )
        .expect("skeleton");
    let wire = String::from_utf8(bytes).expect("utf-8");
    let lin = wire
        .find("LIN+")
        .expect("the 17004 skeleton carries SG29 LIN");
    let at = lin
        + wire[lin..]
            .find("IMD++")
            .expect("the 17004 skeleton carries SG29 IMD")
        + 5;
    let wire = format!("{}{grund}{}", &wire[..at], &wire[at + 3..]);
    let msg = edi_energy::parse(wire.as_bytes()).unwrap_or_else(|e| panic!("parses: {e}\n{wire}"));
    let report = msg
        .validate_on_date_with_rollen(tag, rollen)
        .expect("validates");
    report
        .iter_issues()
        .filter(|i| edi_energy::profile::validate::bars_sending(i))
        .map(|i| format!("[{}] {}", i.rule_id.as_deref().unwrap_or("-"), i.message))
        .collect()
}

#[test]
fn a_17004_with_z46_from_a_lf_passes() {
    for release in RELEASES {
        let issues = anfrage_17004(release, LF, "Z46", &rolle(Some("LF")));
        assert!(issues.is_empty(), "ORDERS {}: {issues:#?}", release.0);
    }
}

/// `[45]` does not hold for a MSB, so `Z46` is not a code its 17004 may carry.
#[test]
fn a_17004_with_z46_from_a_msb_is_refused() {
    for release in RELEASES {
        let issues = anfrage_17004(release, MSB, "Z46", &rolle(Some("MSB")));
        assert!(
            issues
                .iter()
                .any(|i| i.contains("7081") && i.contains("Z46")),
            "ORDERS {}: {issues:#?}",
            release.0
        );
        // …and the MSB's own code passes.
        let own = anfrage_17004(release, MSB, "Z47", &rolle(Some("MSB")));
        assert!(own.is_empty(), "ORDERS {}: {own:#?}", release.0);
        // A LF may not send the MSB's code.
        let lf = anfrage_17004(release, LF, "Z47", &rolle(Some("LF")));
        assert!(!lf.is_empty(), "ORDERS {}: Z47 from a LF", release.0);
    }
}

/// Without the sender's role neither code is decided, so neither is refused.
#[test]
fn an_unknown_sender_role_leaves_the_code_undecided() {
    for release in RELEASES {
        for grund in ["Z46", "Z47"] {
            let issues = anfrage_17004(release, MSB, grund, &Parteirollen::unbekannt());
            assert!(
                issues.is_empty(),
                "ORDERS {} {grund}: {issues:#?}",
                release.0
            );
        }
    }
}

/// Every role predicate of every shipped AHB reads as a role Bedingung or
/// not at all — never as a segment presence test.
#[test]
fn no_role_predicate_is_read_as_a_presence_test() {
    let mut read = Vec::new();
    for profile in ReleaseRegistry::global().all_profiles() {
        for (nr, text) in &profile.ahb.conditions {
            if edi_energy::profile::conditions::predicates_a_role(text)
                && let Some(v) = Voraussetzung::parse(text)
                && !matches!(v, Voraussetzung::Rolle { .. })
            {
                read.push(format!(
                    "{} {} [{nr}] {text}",
                    profile.message_type(),
                    profile.release()
                ));
            }
        }
    }
    assert!(read.is_empty(), "read as a presence test: {read:#?}");
}

/// The shapes: „mit Rolle X (nicht) vorhanden", „in der Rolle X/Y",
/// „nicht in der Rolle X oder Y"; a Sparte qualifier stays unread.
#[test]
fn role_bedingungen_read_their_party_roles_and_negation() {
    use edi_energy::profile::Partei;
    let rolle = |t: &str| match Voraussetzung::parse(t) {
        Some(Voraussetzung::Rolle {
            partei,
            rollen,
            negate,
        }) => Some((partei, rollen, negate)),
        _ => None,
    };
    assert_eq!(
        rolle("Wenn MP-ID in SG2 NAD+MS mit Rolle MSB nicht vorhanden"),
        Some((Partei::Absender, vec!["MSB".to_owned()], true))
    );
    assert_eq!(
        rolle("Wenn MP-ID in SG2 NAD+MR (Nachrichtenempfänger) in der Rolle LF/MSB/ÜNB"),
        Some((
            Partei::Empfaenger,
            vec!["LF".to_owned(), "MSB".to_owned(), "ÜNB".to_owned()],
            false
        ))
    );
    assert_eq!(
        rolle("Wenn MP-ID in NAD+MR nicht in der Rolle MGV oder KN"),
        Some((
            Partei::Empfaenger,
            vec!["MGV".to_owned(), "KN".to_owned()],
            true
        ))
    );
    assert_eq!(
        rolle("Wenn MP-ID in SG2 NAD+MR mit Rolle MSB in der Sparte Gas nicht vorhanden"),
        None
    );
}
