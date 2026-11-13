//! The pre-send AHB gate: a rendered Übertragungsdatei is parsed back and held
//! to its release profile before it leaves — over AS4 or in-process loopback.
//!
//! An AHB Bedingung on the Marktrolle of an MP-ID („Wenn MP-ID in SG2 NAD+MS
//! mit Rolle MSB nicht vorhanden", ORDERS AHB 1.1b Kap. 4.13, S. 91) is
//! decided from the roles the deployment knows: the sender is always one of
//! its own MP-IDs, and the receiver is known when it is one too (a
//! combined-role loopback). A role the registry does not hold leaves the
//! Bedingung undecided.

use crate::party_registry::MpIdRegistry;

/// Why a rendered message may not be sent.
#[derive(Debug)]
pub enum Sperre {
    /// It does not parse.
    Parse(String),
    /// Its validation could not run.
    Validierung(String),
    /// It violates its AHB profile: the refusals.
    Profil(Vec<String>),
}

/// Hold `bytes` to its profile on `tag`, with the [`MpIdRegistry`]'s roles of
/// its `UNB` sender and receiver.
///
/// # Errors
///
/// The [`Sperre`] that keeps it from being sent.
pub fn pruefe(
    platform: &edi_energy::Platform,
    bytes: &[u8],
    registry: &MpIdRegistry,
    tag: time::Date,
) -> Result<(), Sperre> {
    use edi_energy::EdiEnergyMessage as _;
    let parsed = platform
        .parse(bytes)
        .map_err(|e| Sperre::Parse(e.to_string()))?;
    let rollen = platform
        .parse_interchange_header(bytes)
        .map(|h| registry.parteirollen(&h.sender_id, &h.receiver_id))
        .unwrap_or_default();
    let report = parsed
        .validate_on_date_with_rollen(tag, &rollen)
        .map_err(|e| Sperre::Validierung(e.to_string()))?;
    if report.is_sendable() {
        Ok(())
    } else {
        Err(Sperre::Profil(
            report.sending_refusals().map(|e| e.to_string()).collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edi_energy::profile::SkeletonParties;
    use edi_energy::{MessageType, ReleaseRegistry};

    const SENDER: &str = "9900001000002";
    const MSB: &str = "9900001000003";

    fn registry(rolle: &str) -> MpIdRegistry {
        MpIdRegistry::from_config(&[crate::config::PartyConfig {
            mp_id: SENDER.to_owned(),
            roles: vec![rolle.to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .unwrap()
    }

    /// A 17004 from `SENDER` with `SG29 IMD` Grund `Z46`, rendered from the
    /// ORDERS 1.4c skeleton.
    fn anfrage_17004_z46() -> Vec<u8> {
        let profile = ReleaseRegistry::global()
            .profiles_for(MessageType::Orders)
            .find(|p| p.release().as_str() == "1.4c")
            .expect("ORDERS 1.4c is shipped");
        let af = profile.anwendungsfall(17004).expect("17004 is a column");
        let wire = String::from_utf8(
            profile
                .skeleton_interchange(
                    af,
                    &SkeletonParties {
                        sender: SENDER.to_owned(),
                        receiver: MSB.to_owned(),
                    },
                )
                .expect("skeleton"),
        )
        .unwrap();
        let lin = wire.find("LIN+").expect("SG29 LIN");
        let at = lin + wire[lin..].find("IMD++").expect("SG29 IMD") + 5;
        format!("{}Z46{}", &wire[..at], &wire[at + 3..]).into_bytes()
    }

    /// `Z46` is the code a LF or NB sends (ORDERS AHB 1.1b Kap. 4.13, S. 91:
    /// `X [45] ∧ [492]`, `[45]` „Wenn MP-ID in SG2 NAD+MS mit Rolle MSB nicht
    /// vorhanden"): from a MSB deployment it is refused before sending, from a
    /// LF deployment it passes.
    #[test]
    fn a_17004_with_z46_is_refused_from_a_msb_deployment_only() {
        let platform = edi_energy::Platform::with_all_profiles();
        let tag = time::macros::date!(2026 - 11 - 13);
        let bytes = anfrage_17004_z46();
        match pruefe(&platform, &bytes, &registry("MSB"), tag) {
            Err(Sperre::Profil(refusals)) => assert!(!refusals.is_empty()),
            other => panic!("a MSB's Z46 must be refused: {other:?}"),
        }
        pruefe(&platform, &bytes, &registry("LF"), tag)
            .unwrap_or_else(|e| panic!("a LF's Z46 is sendable: {e:?}"));
    }

    /// The registry speaks for its own MP-IDs only, and for one declared with a
    /// single AHB role.
    #[test]
    fn only_an_own_single_role_mp_id_has_a_role() {
        let reg = registry("GMSB");
        assert_eq!(reg.ahb_rolle(SENDER), Some("MSB"));
        assert_eq!(reg.ahb_rolle(MSB), None);
        let beide = MpIdRegistry::from_config(&[crate::config::PartyConfig {
            mp_id: SENDER.to_owned(),
            roles: vec!["NB".to_owned(), "MSB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .unwrap();
        assert_eq!(beide.ahb_rolle(SENDER), None);
    }

    /// Inbound, the sender's role is not known to the receiving deployment, so
    /// the same 17004 leaves `[45]` undecided and is not refused for it.
    #[test]
    fn an_inbound_z46_from_a_sender_of_unknown_role_stays_undecided() {
        let msg = edi_energy::parse(&anfrage_17004_z46()).expect("parses");
        let report = crate::adapters::ahb_pruefung(&msg).expect("ORDERS has a profile");
        assert!(
            report.sending_refusals().next().is_none(),
            "{:?}",
            report.sending_refusals().collect::<Vec<_>>()
        );
    }
}
