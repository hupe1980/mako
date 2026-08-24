//! How a received message finds the object or process it belongs to.
//!
//! ALOCAT 5.11a §3.3 publishes this per Prüfidentifikator — which
//! *Zuordnungstupel* the receiver applies, and the exact segments each element
//! comes from:
//!
//! | Tuple | Elements | Segments |
//! |---|---|---|
//! | `ZO-T1` | Bilanzkreis, Netzbetreiber, Zeitreihentyp | `SG39 NAD+ZEU`, `SG39 NAD+ZSO`, `SG36 SG37 STS` |
//! | `ZO-T2` | Verantwortlicher Absender, vorgelagerter NB, nachgelagerter NB | `SG3 NAD+MS`, `SG39 NAD+ZET`, `SG39 NAD+ZSZ` |
//! | `ZO-T3` | Bilanzkreis, Netzkontonummer, Zeitreihentyp | `SG39 NAD+ZEU`, `SG39 NAD+ZSH`, `SG36 SG37 STS` |
//! | `ZO-T4` | Bilanzkreis, Virtueller Handelspunkt, Zeitreihentyp | `SG39 NAD+ZEU`, `SG39 NAD+VHP`, `SG36 SG37 STS` |
//! | `ZG-T1` | Clearingnummer | `SG1 RFF+ANX` |
//!
//! `ZO-T*` assigns the message to an **object**, `ZG-T1` to an existing
//! **Geschäftsvorfall** (an open Clearingfall) — keying both the same way merges
//! a clearing correction into the stream it corrects.
//!
//! Nominations carry no published tuple: a NOMRES has one `RFF`, and it is the
//! Prüfidentifikator, so a NOMRES cannot be paired with its NOMINT by reference
//! — only by the business key both carry.

use std::fmt;

use crate::{
    message::DvgwMessage,
    model::{nad, rff},
    pruefidentifikator::Pruefidentifikator,
};

/// The Zuordnungstupel a Prüfidentifikator is assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Zuordnung {
    /// `ZO-T1` — (Bilanzkreis, Netzbetreiber, Zeitreihentyp).
    ZoT1,
    /// `ZO-T2` — (Verantwortlicher Absender, vorgelagerter NB, nachgelagerter NB).
    ZoT2,
    /// `ZO-T3` — (Bilanzkreis, Netzkontonummer, Zeitreihentyp).
    ZoT3,
    /// `ZO-T4` — (Bilanzkreis, Virtueller Handelspunkt, Zeitreihentyp).
    ZoT4,
    /// `ZG-T1` — (Clearingnummer). Assigns to an open Geschäftsvorfall.
    ZgT1,
    /// Nomination pairing: (Gastag, Ort, Bilanzkreis intern, Bilanzkreis extern).
    ///
    /// Not a DVGW-published tuple — NOMINT/NOMRES publish none, because a NOMRES
    /// carries no reference to the nomination it answers. This is the business
    /// key both messages do carry, and it is the only thing that pairs them.
    Nominierung,
}

impl Zuordnung {
    /// The published label, or `"Nominierung"` for the derived nomination key.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ZoT1 => "ZO-T1",
            Self::ZoT2 => "ZO-T2",
            Self::ZoT3 => "ZO-T3",
            Self::ZoT4 => "ZO-T4",
            Self::ZgT1 => "ZG-T1",
            Self::Nominierung => "Nominierung",
        }
    }

    /// `true` when the tuple assigns to an existing Geschäftsvorfall rather than
    /// to an object — i.e. the message continues a case instead of extending a
    /// stream.
    #[must_use]
    pub fn assigns_to_geschaeftsvorfall(self) -> bool {
        matches!(self, Self::ZgT1)
    }

    /// The tuple DVGW assigns to a Prüfidentifikator.
    ///
    /// Source: ALOCAT 5.11a §3.3. Returns `None` for a code with no published
    /// assignment — including any ALOCAT code outside the shipped package, which
    /// must not be guessed at.
    #[must_use]
    pub fn for_pid(pid: Pruefidentifikator) -> Option<Self> {
        let zuordnung = match pid.as_u32() {
            // Allokationsabgabe (NB an MGV, 70001/70004–70007) and the optional
            // tägliche SLP-Allokation (NB an BKV, 70022).
            70001 | 70004..=70007 | 70022 => Self::ZoT3,
            // Allokationsabgabe NKP — NB an MGV (70002/70003), ENB/ANB an NB
            // (70011/70012), MGV an NB (70023).
            70002 | 70003 | 70011 | 70012 | 70023 => Self::ZoT2,
            // Allokationsabgabe (MGV an BKV, 70013–70017) and Ersatzwertversand
            // (MGV an NB, 70021).
            //
            // The published row for 70013–70017 names ZO-T1 *and* ZO-T4; they
            // differ only in whether the counterparty is a Netzbetreiber or the
            // Virtueller Handelspunkt, which the message states by which `NAD`
            // role it carries rather than by its Prüfidentifikator.
            70013..=70017 | 70021 => Self::ZoT1,
            // Allokationsabgabe Clearing — assigns to an open Clearingfall.
            70008..=70010 | 70018..=70020 => Self::ZgT1,
            // NOMINT / NOMRES.
            70030..=70039 => Self::Nominierung,
            _ => return None,
        };
        Some(zuordnung)
    }
}

impl fmt::Display for Zuordnung {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A resolved Zuordnungstupel — the tuple and the values read for it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct CorrelationKey {
    /// Which tuple was applied.
    pub zuordnung: Zuordnung,
    /// The tuple's elements, in the order the specification lists them.
    ///
    /// An element the message did not carry is an empty string rather than a
    /// dropped position, so two keys never collide by shifting.
    pub elements: Vec<String>,
}

impl CorrelationKey {
    /// `true` when every element carries a value.
    ///
    /// A partial key still identifies *something*, but on fewer facts than DVGW
    /// specified.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.elements.is_empty() && self.elements.iter().all(|e| !e.is_empty())
    }
}

impl fmt::Display for CorrelationKey {
    /// A stable, flat rendering for use as a process-registry key.
    ///
    /// The tuple label is part of the string: the same Bilanzkreis under `ZO-T1`
    /// and `ZO-T3` names two different objects — a Netzbetreiber's stream and a
    /// Netzkonto's.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.zuordnung.as_str())?;
        for element in &self.elements {
            write!(f, "|{element}")?;
        }
        Ok(())
    }
}

impl DvgwMessage {
    /// The Zuordnungstupel this message is assigned by, with its values read.
    ///
    /// Returns `None` when the Prüfidentifikator is absent or has no published
    /// assignment — the message then has no defined way to reach a process, and
    /// inventing one would attach it to the wrong stream.
    #[must_use]
    pub fn correlation_key(&self) -> Option<CorrelationKey> {
        let zuordnung = Zuordnung::for_pid(self.pruefidentifikator?)?;
        // Every ZO-T* element outside the header is read from the first position:
        // the tuple identifies the message, and a conformant message states one
        // object per message.
        let item = self.items.first();
        let item_party = |role: &str| {
            item.and_then(|i| i.party(role))
                .map(|p| p.id.clone())
                .unwrap_or_default()
        };
        let zeitreihentyp = || item.and_then(|i| i.item_type.clone()).unwrap_or_default();
        let gas_day = || {
            self.validity_period
                .map(|p| p.start.date().to_string())
                .unwrap_or_default()
        };

        let elements = match zuordnung {
            Zuordnung::ZoT1 => vec![
                item_party(nad::BILANZKREIS_INTERN),
                item_party(nad::NETZBETREIBER),
                zeitreihentyp(),
            ],
            Zuordnung::ZoT2 => vec![
                self.sender().map(|p| p.id.clone()).unwrap_or_default(),
                item_party(nad::VORGELAGERTER_NETZBETREIBER),
                item_party(nad::NETZKONTO),
            ],
            Zuordnung::ZoT3 => vec![
                item_party(nad::BILANZKREIS_INTERN),
                item_party(nad::NETZKONTO_ZO_T3),
                zeitreihentyp(),
            ],
            Zuordnung::ZoT4 => vec![
                item_party(nad::BILANZKREIS_INTERN),
                item_party(nad::VIRTUELLER_HANDELSPUNKT),
                zeitreihentyp(),
            ],
            Zuordnung::ZgT1 => vec![
                self.reference(rff::CLEARINGNUMMER)
                    .unwrap_or_default()
                    .to_owned(),
            ],
            Zuordnung::Nominierung => vec![
                gas_day(),
                item.and_then(|i| i.locations.first())
                    .and_then(|l| l.code.clone())
                    .unwrap_or_default(),
                item_party(nad::BILANZKREIS_INTERN),
                item_party(nad::BILANZKREIS_EXTERN),
            ],
        };
        Some(CorrelationKey {
            zuordnung,
            elements,
        })
    }

    /// The gas day this message reports on, as `YYYY-MM-DD`.
    ///
    /// Read from `DTM+Z01`, never from `DTM+137`.
    #[must_use]
    pub fn gas_day(&self) -> Option<time::Date> {
        self.validity_period.map(|p| p.start.date())
    }

    /// The key identifying the *process* this message belongs to.
    ///
    /// The [`correlation_key`](Self::correlation_key) plus the gas day, which the
    /// published tuples leave out: a `ZO-T*` tuple identifies an **object** — an
    /// account, not one day of it — while a process is one gas day of that
    /// object, holding that day's record and its `KoV` §6.4 deadline.
    ///
    /// `ZG-T1` is returned unchanged: a Clearingnummer already identifies one
    /// Geschäftsvorfall, which may span several days.
    ///
    /// Returns `None` when the message has no published Zuordnung, or when a
    /// tuple that needs a gas day has none to read.
    #[must_use]
    pub fn process_key(&self) -> Option<String> {
        let key = self.correlation_key()?;
        if key.zuordnung.assigns_to_geschaeftsvorfall() {
            return Some(key.to_string());
        }
        let gas_day = self.gas_day()?;
        Some(format!("{key}|{gas_day}"))
    }
}

/// Every Prüfidentifikator the shipped catalogue assigns a Zuordnung.
///
/// Used by the routing layer to refuse, at startup, to register a PID it has no
/// defined way to correlate.
pub fn assigned_pids() -> impl Iterator<Item = (Pruefidentifikator, Zuordnung)> {
    crate::pruefidentifikator::catalogue()
        .iter()
        .filter_map(|info| {
            let pid = Pruefidentifikator::new(info.pid)?;
            Zuordnung::for_pid(pid).map(|z| (pid, z))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every catalogued Prüfidentifikator must have a published Zuordnung, or a
    /// message carrying it has no defined way to reach a process.
    #[test]
    fn every_catalogued_pid_has_a_zuordnung() {
        let catalogued = crate::pruefidentifikator::catalogue().len();
        assert_eq!(
            assigned_pids().count(),
            catalogued,
            "a catalogued PID has no Zuordnung assignment"
        );
    }

    /// The assignments must match ALOCAT 5.11a §3.3 exactly.
    #[test]
    fn the_assignments_match_the_published_table() {
        let z = |pid: u32| Zuordnung::for_pid(Pruefidentifikator::new(pid).unwrap()).unwrap();

        // Allokationsabgabe (NB an MGV) — ZO-T3.
        for pid in [70001, 70004, 70005, 70006, 70007] {
            assert_eq!(z(pid), Zuordnung::ZoT3, "{pid}");
        }
        // Allokationsabgabe NKP — ZO-T2.
        for pid in [70002, 70003, 70011, 70012, 70023] {
            assert_eq!(z(pid), Zuordnung::ZoT2, "{pid}");
        }
        // Allokationsabgabe (MGV an BKV) and Ersatzwertversand — ZO-T1.
        for pid in [70013, 70014, 70015, 70016, 70017, 70021] {
            assert_eq!(z(pid), Zuordnung::ZoT1, "{pid}");
        }
        // Optional tägliche SLP-Allokation — ZO-T3.
        assert_eq!(z(70022), Zuordnung::ZoT3);
        // Clearing — assigns to a Geschäftsvorfall, not an object.
        for pid in [70008, 70009, 70010, 70018, 70019, 70020] {
            assert_eq!(z(pid), Zuordnung::ZgT1, "{pid}");
            assert!(z(pid).assigns_to_geschaeftsvorfall(), "{pid}");
        }
        // Nominations pair on the business key.
        for pid in 70030..=70039 {
            assert_eq!(z(pid), Zuordnung::Nominierung, "{pid}");
        }
        // An uncatalogued code in range has no assignment to guess at.
        assert_eq!(
            Zuordnung::for_pid(Pruefidentifikator::new(70500).unwrap()),
            None
        );
    }

    /// The tuple label has to be part of the key: the same Bilanzkreis under two
    /// different tuples is two different objects.
    #[test]
    fn the_rendered_key_carries_its_tuple() {
        let key = CorrelationKey {
            zuordnung: Zuordnung::ZoT1,
            elements: vec!["BK1".into(), "NB1".into(), "Z01".into()],
        };
        assert_eq!(key.to_string(), "ZO-T1|BK1|NB1|Z01");
        assert!(key.is_complete());

        let same_values_other_tuple = CorrelationKey {
            zuordnung: Zuordnung::ZoT3,
            elements: vec!["BK1".into(), "NB1".into(), "Z01".into()],
        };
        assert_ne!(key.to_string(), same_values_other_tuple.to_string());
    }

    /// A missing element must hold its position rather than shift the rest.
    #[test]
    fn an_absent_element_keeps_its_slot() {
        let key = CorrelationKey {
            zuordnung: Zuordnung::ZoT1,
            elements: vec!["BK1".into(), String::new(), "Z01".into()],
        };
        assert_eq!(key.to_string(), "ZO-T1|BK1||Z01");
        assert!(!key.is_complete());
        // …and must not collide with a two-element key that happens to match.
        assert_ne!(
            key.to_string(),
            CorrelationKey {
                zuordnung: Zuordnung::ZoT1,
                elements: vec!["BK1".into(), "Z01".into()],
            }
            .to_string()
        );
    }
}

#[cfg(test)]
mod process_key_tests {
    use crate::{DvgwDocument, DvgwPeriod, DvgwPlatform, MessageBuilder, Position, model::nad};
    use time::macros::datetime;

    fn alocat(pid: u32, day: u8, clearing: &str) -> Vec<u8> {
        let gas_day = DvgwPeriod {
            start: datetime!(2026-03-01 05:00 UTC) + time::Duration::days(i64::from(day)),
            end: datetime!(2026-03-02 05:00 UTC) + time::Duration::days(i64::from(day)),
        };
        MessageBuilder::new(DvgwDocument::AllokationSlp)
            .document_number("ALOCAT1")
            .version("5.11a")
            .pruefidentifikator(pid)
            .message_datetime(datetime!(2026-03-01 04:00 UTC))
            .validity_period(gas_day)
            .clearingnummer(clearing)
            .sender("A")
            .receiver("B")
            .position(
                Position::new()
                    .item_type("Z01")
                    .location("Z99", None)
                    .quantity("Z03", "4000", gas_day)
                    .party(nad::BILANZKREIS_INTERN, "BK1")
                    .party(nad::NETZKONTO_ZO_T3, "NK1"),
            )
            .build()
            .expect("builds")
    }

    /// Two gas days of the same object are two processes.
    ///
    /// `ZO-T3` names (Bilanzkreis, Netzkonto, Zeitreihentyp) and stops there, so
    /// the tuple alone is the same for every day of the month. An allocation
    /// process holds one gas day's record and one §6.4 deadline, so keying on the
    /// tuple would let day two overwrite both of day one's.
    #[test]
    fn two_gas_days_of_one_object_are_two_processes() {
        let platform = DvgwPlatform::default();
        let day_one = platform.parse(&alocat(70_001, 0, "CLR-A")).unwrap();
        let day_two = platform.parse(&alocat(70_001, 1, "CLR-A")).unwrap();

        // The published tuple is identical — as specified.
        assert_eq!(day_one.correlation_key(), day_two.correlation_key());
        // The process key is not.
        assert_ne!(day_one.process_key(), day_two.process_key());
        assert_eq!(
            day_one.process_key().as_deref(),
            Some("ZO-T3|BK1|NK1|Z01|2026-03-01")
        );
    }

    /// A clearing case keeps one key across the days it spans.
    #[test]
    fn a_clearing_case_is_not_split_by_gas_day() {
        let platform = DvgwPlatform::default();
        let day_one = platform.parse(&alocat(70_008, 0, "CLR-A")).unwrap();
        let day_two = platform.parse(&alocat(70_008, 1, "CLR-A")).unwrap();
        assert_eq!(day_one.process_key(), day_two.process_key());
        assert_eq!(day_one.process_key().as_deref(), Some("ZG-T1|CLR-A"));
        // A different Clearingfall is a different case.
        let other = platform.parse(&alocat(70_008, 0, "CLR-B")).unwrap();
        assert_ne!(day_one.process_key(), other.process_key());
    }

    /// A tuple that needs a gas day and has none yields no process key, rather
    /// than one that silently merges with every other dateless message.
    #[test]
    fn a_missing_gas_day_yields_no_process_key() {
        let mut wire = String::from_utf8(alocat(70_001, 0, "CLR-A")).unwrap();
        wire = wire.replace("DTM+Z01:202603010500202603020500:719'", "");
        let msg = DvgwPlatform::default().parse(wire.as_bytes()).unwrap();
        assert!(msg.correlation_key().is_some(), "the tuple still resolves");
        assert_eq!(msg.process_key(), None, "but the process does not");
    }
}
