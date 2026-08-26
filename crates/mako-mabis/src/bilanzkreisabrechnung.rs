//! Bilanzkreisabrechnung Strom — the settlement of **one Summenzeitreihe** over
//! one Bilanzierungsmonat (BNetzA BK6-24-174 Anlage 3, Kap. 3.8 and 3.10).
//!
//! # The stream is a Summenzeitreihe, not a message
//!
//! A MaBiS settlement is not request/response. One MaBiS-Zählpunkt in one
//! Bilanzierungsmonat receives a **sequence of versions** of the same
//! Summenzeitreihe, each of which may be checked, corrected and superseded
//! until its clearing window closes. Modelling a single inbound message as the
//! whole process makes the second version unrepresentable, and the second
//! version is the entire point of the Clearingphase.
//!
//! ```text
//! Version 1 ──► (Datenstatus)          ◄── Prüfmitteilung ± ── ►
//! Version 2 ──► (Datenstatus)          ◄── Prüfmitteilung ± ── ►   … until the
//! Version n ──► Datenstatus „abgerechnete Daten"                    window closes
//! ```
//!
//! # Three rules that are easy to get backwards
//!
//! **The Datenstatus is assigned exclusively by the BIKO** (Kap. 3.8.3). No
//! other party derives one, which is why [`BillingCommand::ReceiveIftsta`]
//! is the only way a [`Datenstatus`] enters the state.
//!
//! **Which Datenstatus a version gets depends on *when* it arrived, not on what
//! it says.** Inside the Erstaufschlag window it is „Abrechnungsdaten"
//! automatically; after it, „Prüfdaten", and only a **positive** Prüfmitteilung
//! promotes it (Kap. 3.8.3). The two windows differ per Summenzeitreihe — 10 WT
//! for the BG-SZR, 12 WT for the BK-SZR — so the phase comes from
//! [`crate::fristen::Bilanzierungsmonat::phase`] and never from a flag on the
//! message.
//!
//! **A negative Prüfmitteilung does not change the Datenstatus** (Kap. 3.8.3,
//! explicit sentence). It opens a correction obligation on the responsible
//! party; the version keeps whatever status it had.
//!
//! # There is no 1-Werktag Prüfmitteilung deadline
//!
//! Every Prüfmitteilung use case in the Festlegung carries an empty Frist cell
//! („–", e.g. Kap. 9.8.2 Nr. 1: „Der NB **kann** … eine positive oder eine
//! negative Prüfmitteilung übermitteln"). What bounds it is the clearing window
//! of Tabelle 2. The two genuine 1-Werktag Fristen belong to the **BIKO** —
//! forwarding a Prüfmitteilung (Kap. 9.8.2 Nr. 3) and dispatching the
//! Datenstatus (Kap. 9.9.2 Nr. 1) — and live in [`crate::fristen`].
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt*.
//!
//! | PID | Nachricht | Von → An | Rolle in this workflow |
//! |----:|-----------|----------|------------------------|
//! | 13003 | MSCONS Summenzeitreihe | ÜNB/NB/BIKO → … | the version itself |
//! | 21000 | IFTSTA Prüfmitteilung | LF → NB/ÜNB | outbound, LF side |
//! | 21001 | IFTSTA Prüfmitteilung | NB (benachbart) → NB (verantwortlich) | outbound, NZR |
//! | 21002 | IFTSTA **Abweisung** einer Prüfmitteilung | BIKO → NB/ÜNB | inbound |
//! | 21003 | IFTSTA Datenstatus **und** Weiterleitung Prüfmitteilung | BIKO → NB/ÜNB | inbound |
//! | 21004 | IFTSTA Datenstatus **und** Weiterleitung Prüfmitteilung | BIKO → BKV/NB | inbound |
//! | 21005 | IFTSTA Prüfmitteilung | BKV/NB → BIKO | outbound |
//!
//! Two consequences the previous single-PID model could not express:
//!
//! - **21003 carries a Datenstatus just as 21004 does.** Which of the two you
//!   receive follows from your role — 21003 addresses the NB/ÜNB, 21004 the
//!   BKV (and the NB for the DZÜ). Treating 21004 as "the Datenstatus PID"
//!   drops every Datenstatus an NB or ÜNB is sent.
//! - **21000, 21001 and 21005 are outbound.** They are *this* participant's
//!   Prüfmitteilung, not something that arrives. Registering them as inbound
//!   status notifications silently discards the obligation to send one.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 (MaBiS)** — Kap. 2 (Tabelle 1), Kap. 3.8
//!   (Bildung, Versionierung, Prüfmitteilung und Datenstatus), Kap. 3.10
//!   (Tabelle 2 Fristenkalender), Kap. 9–13 (the use cases).
//! - **MSCONS AHB 3.1g / 3.2** — Summenzeitreihen (PID 13003).
//! - **IFTSTA AHB 2.0h / 2.1** — MaBiS Statusmeldungen (PIDs 21000–21005).

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    projection::Projection,
    types::{BikoId, BillingPeriod, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use mako_pruefung::mabis::MabisAntwort;

use crate::ids::MabisZaehlpunktId;
use crate::zeitreihen::Zeitreihe;

// ── Constants ────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "mabis-billing";

/// MSCONS PID carrying a Summenzeitreihe („Summenzeitreihen und
/// Ausfallarbeitssummen", MSCONS AHB 3.1g §5).
///
/// Covers every BG-/BK-/LF-SZR, the DZÜ, the NZR and the
/// Abrechnungssummenzeitreihe.
pub const SUMMENZEITREIHE_PID: u32 = 13_003;

/// MSCONS PIDs carrying the MaBiS Ausfallarbeit series (Kap. 17).
///
/// | PID | Serie | Von → An |
/// |----:|-------|----------|
/// | 13020 | Ausfallarbeitsüberführungszeitreihe | NB (ANB) → BIKO → BKV; ÜNB → BIKO |
/// | 13023 | Lieferantenausfallarbeitssummenzeitreihe | NB (ANB) → LF |
///
/// Both are **MaBiS**, not Redispatch: the PID overview files them under the
/// MaBiS Prozessbeschreibung and both carry the full Prüfmitteilung/Datenstatus
/// cycle. 13021 (meteorologische Ex-post-Daten), 13022 (Einzelzeitreihe
/// Ausfallarbeit, TR-scharf) and 13026 (EEG-Überführungszeitreihe) are not.
pub const AUSFALLARBEIT_PIDS: &[u32] = &[13_020, 13_023];

/// Whether `pid` may carry a version into this workflow.
#[must_use]
pub fn ist_zeitreihen_pid(pid: u32) -> bool {
    pid == SUMMENZEITREIHE_PID || AUSFALLARBEIT_PIDS.contains(&pid)
}

/// Every MaBiS IFTSTA Prüfidentifikator (21000–21005).
///
/// PID 21006 does not exist; PID 21007 is WiM Strom Teil 1 / WiM Gas and is
/// registered in `mako-wim`.
pub const IFTSTA_PIDS: &[u32] = &[21_000, 21_001, 21_002, 21_003, 21_004, 21_005];

/// The IFTSTA PIDs that carry a [`Datenstatus`] (both are BIKO-originated).
///
/// 21003 addresses the NB/ÜNB, 21004 the BKV — and the NB for the DZÜ. A
/// participant sees one or the other depending on its role, never neither.
pub const IFTSTA_DATENSTATUS_PIDS: &[u32] = &[21_003, 21_004];

/// The IFTSTA PIDs on which **this** participant sends a Prüfmitteilung.
///
/// 21000 LF → NB/ÜNB, 21001 benachbarter NB → verantwortlicher NB (NZR),
/// 21005 BKV/NB → BIKO.
pub const IFTSTA_PRUEFMITTEILUNG_PIDS: &[u32] = &[21_000, 21_001, 21_005];

/// IFTSTA PID 21002 — the BIKO **rejects** a Prüfmitteilung.
///
/// Kap. 9.8.2 Nr. 2: a rejected Prüfmitteilung is not forwarded to the
/// responsible party at all, so the check it carried never lands.
pub const IFTSTA_ABWEISUNG_PID: u32 = 21_002;

// ── Version ──────────────────────────────────────────────────────────────────

/// Version of a Summenzeitreihe — its **Erstellungszeitpunkt**.
///
/// Kap. 3.8.2 requires versions to ascend („jeweils aufsteigend zu vergeben …
/// über die gesamte BKA"), and the wire implements that with a timestamp rather
/// than a counter:
///
/// > „Über dieses Segment erfolgt die Referenzierung auf die Version der
/// > betrachteten Summenzeitreihe. **Die Versionsangabe erfolgt über den
/// > Erstellungszeitpunkt**, der in der MSCONS übermittelt wurde.
/// > Beispiel: `RFF+AUU:20110503121544?+00`"
/// >
/// > — IFTSTA MIG 2.1, SG4 `RFF+AUU`, DE 1154 (`an17`)
///
/// So a version is 17 characters of `CCYYMMDDHHMMSSZZZ`, and it is what both
/// ends match on: the sender puts it in MSCONS SG6 `DTM+293`, the BIKO echoes
/// it back in the IFTSTA. Modelling it as an integer would make every inbound
/// reference unmatchable, because there is no integer on the wire to match.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct SzrVersion(String);

/// A Versionsangabe that is not a `CCYYMMDDHHMMSSZZZ` Erstellungszeitpunkt.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSzrVersion {
    /// Not 17 characters (IFTSTA MIG 2.1 DE 1154 `an17`).
    #[error("Versionsangabe hat {0} Zeichen, erwartet 17 (CCYYMMDDHHMMSSZZZ)")]
    FalscheLaenge(usize),
    /// The first 14 characters are not all digits.
    #[error("Versionsangabe '{0}': die ersten 14 Zeichen sind kein Zeitstempel")]
    KeinZeitstempel(String),
}

impl SzrVersion {
    /// Build a version from an Erstellungszeitpunkt as it arrived on the wire.
    ///
    /// # Errors
    ///
    /// [`InvalidSzrVersion`] when the value is not 17 characters or does not
    /// open with a 14-digit timestamp. Refusing rather than normalising is
    /// deliberate: the string is a **matching key** against the counterparty's
    /// records, and a repaired one no longer matches what they hold.
    pub fn new(erstellungszeitpunkt: impl Into<String>) -> Result<Self, InvalidSzrVersion> {
        let v: String = erstellungszeitpunkt.into();
        let n = v.chars().count();
        if n != 17 {
            return Err(InvalidSzrVersion::FalscheLaenge(n));
        }
        if !v.chars().take(14).all(|c| c.is_ascii_digit()) {
            return Err(InvalidSzrVersion::KeinZeitstempel(v));
        }
        Ok(Self(v))
    }

    /// The Erstellungszeitpunkt as it travels in `RFF+AUU` and `DTM+293`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SzrVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `RFF` qualifier carrying the Versionsangabe (IFTSTA MIG 2.1, DE 1153).
pub const RFF_QUALIFIER_VERSION: &str = "AUU";

// ── Datenstatus ──────────────────────────────────────────────────────────────

/// Datenstatus of one version of a Summenzeitreihe (Kap. 3.8.3).
///
/// Assigned **exclusively by the BIKO**. The five values are the complete set
/// the Festlegung names, and they are exactly the five codes the Datenstatus
/// Entscheidungsbäume emit — „Prüfdaten" in particular is the ordinary status
/// of every version filed after the Erstaufschlag, so a model without it cannot
/// represent the common case.
///
/// # On the wire
///
/// `STS+Z04+<code>:<EBD>` — Statuskategorie `Z04` „Datenstatus zur
/// Summenzeitreihe", DE 4405 the code, DE 1131 the EBD it was read against
/// (IFTSTA MIG 2.1, SG7 `STS`; example `STS+Z04+A03:E_0026`).
///
/// | Code | Datenstatus |
/// |------|-------------|
/// | `A01` | Abrechnungsdaten |
/// | `A02` | Prüfdaten |
/// | `A03` | Abgerechnete Daten |
/// | `A04` | Abrechnungsdaten KBKA |
/// | `A06` | Abgerechnete Daten KBKA |
///
/// Source: *Entscheidungsbaum-Diagramme und Codelisten* 4.3 (01.04.2026),
/// `E_0026` / `E_0042` / `E_0043` and the parallel triples.
///
/// The EBD triples are the three **occasions** a Datenstatus is assigned, and
/// they line up one-to-one with Kap. 3.8.3:
///
/// | EBD name | Occasion |
/// |---|---|
/// | „…nach Eingang einer Summenzeitreihe vergeben" | arrival — Erstaufschlag → `A01`, otherwise `A02` |
/// | „…nach Vorliegen einer Prüfmitteilung vergeben" | a positive check promotes `A02` → `A01` |
/// | „…nach erfolgter Bilanzkreisabrechnung vergeben" | the Abrechnungsstichtag → `A03` / `A06` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Datenstatus {
    /// `A02` „Prüfdaten" — filed after the Erstaufschlag; needs a positive
    /// Prüfmitteilung to be promoted.
    Pruefdaten,
    /// `A01` „Abrechnungsdaten" — will settle in the BKA if it is the highest
    /// such version.
    Abrechnungsdaten,
    /// `A04` „Abrechnungsdaten KBKA" — the same, in the Korrekturlauf.
    AbrechnungsdatenKbka,
    /// `A03` „Abgerechnete Daten" — used in the completed BKA.
    AbgerechneteDaten,
    /// `A06` „Abgerechnete Daten KBKA" — used in the completed KBKA.
    AbgerechneteDatenKbka,
}

impl Datenstatus {
    /// Resolve the `STS+Z04` DE 4405 code.
    ///
    /// The code space is shared by every Datenstatus EBD, so — unlike the GPKE
    /// Antwortcodes — it does **not** have to be read against its tree. The EBD
    /// number still travels alongside it and is worth recording, because it
    /// says *which occasion* assigned the status.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "A01" => Self::Abrechnungsdaten,
            "A02" => Self::Pruefdaten,
            "A03" => Self::AbgerechneteDaten,
            "A04" => Self::AbrechnungsdatenKbka,
            "A06" => Self::AbgerechneteDatenKbka,
            _ => return None,
        })
    }

    /// The `STS+Z04` DE 4405 code for this status.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Abrechnungsdaten => "A01",
            Self::Pruefdaten => "A02",
            Self::AbgerechneteDaten => "A03",
            Self::AbrechnungsdatenKbka => "A04",
            Self::AbgerechneteDatenKbka => "A06",
        }
    }

    /// Whether this status marks the version as settled (Abrechnungsstichtag
    /// reached, Kap. 3.10 Tabelle 2).
    #[must_use]
    pub fn ist_abgerechnet(self) -> bool {
        matches!(self, Self::AbgerechneteDaten | Self::AbgerechneteDatenKbka)
    }

    /// Whether this status makes the version eligible to settle — the highest
    /// such version comes to Abrechnung (Kap. 3.8.3).
    #[must_use]
    pub fn ist_abrechnungsrelevant(self) -> bool {
        matches!(self, Self::Abrechnungsdaten | Self::AbrechnungsdatenKbka)
    }
}

/// `STS` Statuskategorie for a Datenstatus zur Summenzeitreihe
/// (IFTSTA MIG 2.1, DE 9015).
pub const STS_KATEGORIE_DATENSTATUS: &str = "Z04";

// ── Prüfergebnis ─────────────────────────────────────────────────────────────

/// Outcome of checking one version of a Summenzeitreihe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "ergebnis", rename_all = "snake_case")]
pub enum Pruefergebnis {
    /// Positive Prüfmitteilung — after the Erstaufschlag this promotes the
    /// version from „Prüfdaten" to „Abrechnungsdaten" (Kap. 3.8.3). The
    /// promotion itself is the BIKO's; this only states the check passed.
    Positiv {
        /// The tree's own Zustimmungscode.
        antwort: MabisAntwort,
    },
    /// Negative Prüfmitteilung. Kap. 3.8.3: this **does not** change the
    /// Datenstatus — it opens a correction obligation on the responsible party.
    Negativ {
        /// The published Antwortcode, resolved within the tree that decides
        /// this Summenzeitreihe.
        antwort: MabisAntwort,
        /// Free-text Erläuterung for the counterparty and the audit log.
        grund: String,
    },
}

/// A Prüfmitteilung could not be built because the code is not published by
/// the tree that decides the series.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PruefergebnisError {
    /// Checking this Summenzeitreihe is not mako's obligation — see
    /// [`Zeitreihe::pruef_ebd`].
    #[error("für {zeitreihe} führt mako keinen Entscheidungsbaum")]
    KeinPruefbaum {
        /// The series that was asked about.
        zeitreihe: Zeitreihe,
    },
    /// The tree does not publish that code.
    #[error("{ebd} veröffentlicht den Antwortcode {code} nicht")]
    UnbekannterCode {
        /// The tree that decides the series.
        ebd: &'static str,
        /// The code that was asked for.
        code: String,
    },
    /// The code is published but sits on the wrong side of the tree.
    #[error("{code} aus {ebd} ist {cluster} und trägt keine {erwartet}e Prüfmitteilung")]
    FalschesCluster {
        /// The tree.
        ebd: &'static str,
        /// The code.
        code: String,
        /// The cluster it actually sits in.
        cluster: &'static str,
        /// What was asked for.
        erwartet: &'static str,
    },
}

impl Pruefergebnis {
    /// A positive Prüfmitteilung, drawing the Zustimmungscode of the tree that
    /// decides `zeitreihe`.
    ///
    /// The code is never passed in: „Zeitreihe akzeptiert" is `A06` in the
    /// LF-SZR trees, `A03` in `E_0062` and `A04` in `E_0065`, and a caller that
    /// picks it puts an undefined code on the wire.
    ///
    /// # Errors
    ///
    /// [`PruefergebnisError::KeinPruefbaum`] when checking the series is not
    /// mako's obligation.
    pub fn positiv(zeitreihe: Zeitreihe) -> Result<Self, PruefergebnisError> {
        let ebd = zeitreihe
            .pruef_ebd()
            .ok_or(PruefergebnisError::KeinPruefbaum { zeitreihe })?;
        let code =
            mako_pruefung::mabis::zustimmung(ebd).ok_or(PruefergebnisError::UnbekannterCode {
                ebd,
                code: "<Zustimmung>".to_owned(),
            })?;
        Ok(Self::Positiv {
            antwort: MabisAntwort::from_code(ebd, code, 0, None),
        })
    }

    /// A negative Prüfmitteilung carrying a published Ablehnungs- or
    /// Abweisungscode.
    ///
    /// # Errors
    ///
    /// When the series has no tree, when the tree does not publish `code`, or
    /// when `code` is that tree's Zustimmung.
    pub fn negativ(
        zeitreihe: Zeitreihe,
        code: &str,
        grund: impl Into<String>,
    ) -> Result<Self, PruefergebnisError> {
        let ebd = zeitreihe
            .pruef_ebd()
            .ok_or(PruefergebnisError::KeinPruefbaum { zeitreihe })?;
        let entry = mako_pruefung::mabis::lookup(ebd, code).ok_or_else(|| {
            PruefergebnisError::UnbekannterCode {
                ebd,
                code: code.to_owned(),
            }
        })?;
        if entry.ist_zustimmung() == Some(true) {
            return Err(PruefergebnisError::FalschesCluster {
                ebd,
                code: code.to_owned(),
                cluster: entry.cluster.label(),
                erwartet: "negativ",
            });
        }
        Ok(Self::Negativ {
            antwort: MabisAntwort::from_code(ebd, entry, 0, None),
            grund: grund.into(),
        })
    }

    /// Resolve `code` against the tree that decides `zeitreihe` and build the
    /// matching Prüfmitteilung.
    ///
    /// The cluster — not the caller — decides whether the result is positive:
    /// only the tree's Zustimmung is.
    ///
    /// # Errors
    ///
    /// When the series has no tree mako runs, or the tree does not publish the
    /// code.
    pub fn aus_code(
        zeitreihe: Zeitreihe,
        code: &str,
        grund: impl Into<String>,
    ) -> Result<Self, PruefergebnisError> {
        let ebd = zeitreihe
            .pruef_ebd()
            .ok_or(PruefergebnisError::KeinPruefbaum { zeitreihe })?;
        let entry = mako_pruefung::mabis::lookup(ebd, code).ok_or_else(|| {
            PruefergebnisError::UnbekannterCode {
                ebd,
                code: code.to_owned(),
            }
        })?;
        let antwort = MabisAntwort::from_code(ebd, entry, 0, None);
        Ok(if entry.ist_zustimmung() == Some(true) {
            Self::Positiv { antwort }
        } else {
            Self::Negativ {
                antwort,
                grund: grund.into(),
            }
        })
    }

    /// Whether this is a positive check.
    #[must_use]
    pub fn ist_positiv(&self) -> bool {
        matches!(self, Self::Positiv { .. })
    }

    /// The resolved Antwortcode either way.
    #[must_use]
    pub fn antwort(&self) -> &MabisAntwort {
        match self {
            Self::Positiv { antwort } | Self::Negativ { antwort, .. } => antwort,
        }
    }

    /// Whether the BIKO forwards this Prüfmitteilung to the next market partner.
    ///
    /// MaBiS Kap. 9.8.2 Nr. 2: an **abgewiesene** Prüfmitteilung is not
    /// forwarded. A negative one carrying an Ablehnungscode is — so this is not
    /// the negation of [`Self::ist_positiv`].
    #[must_use]
    pub fn wird_weitergeleitet(&self) -> bool {
        self.antwort().wird_weitergeleitet()
    }
}

// ── Version record ───────────────────────────────────────────────────────────

/// One version of the Summenzeitreihe, and what has happened to it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VersionRecord {
    /// Ascending version number (Kap. 3.8.2).
    pub version: SzrVersion,
    /// Message reference of the MSCONS that carried it.
    pub message_ref: MessageRef,
    /// Whether it arrived inside the Erstaufschlag window (Kap. 3.8.3). The
    /// caller derives this from [`crate::fristen::Phase::ist_erstaufschlag`];
    /// it decides whether the version starts as „Abrechnungsdaten" or needs a
    /// positive Prüfmitteilung to get there.
    pub im_erstaufschlag: bool,
    /// Prüfmitteilung this participant has sent for this version, if any.
    pub pruefergebnis: Option<Pruefergebnis>,
    /// Datenstatus the BIKO last assigned to this version, if any.
    pub datenstatus: Option<Datenstatus>,
    /// Set when the BIKO rejected the Prüfmitteilung for this version
    /// (IFTSTA 21002). A rejected Prüfmitteilung is never forwarded, so the
    /// check has to be redone.
    pub pruefmitteilung_abgewiesen: Option<String>,
}

// ── Domain events ────────────────────────────────────────────────────────────

/// Events emitted by the MaBiS Bilanzkreisabrechnung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum BillingEvent {
    /// A version of the Summenzeitreihe arrived (MSCONS 13003).
    SummenzeitreiheReceived {
        /// Which Summenzeitreihe of Tabelle 1 this stream settles.
        zeitreihe: Zeitreihe,
        /// The MaBiS-Zählpunkt the series is filed against.
        mabis_zp: MabisZaehlpunktId,
        /// Bilanzierungsmonat, `YYYY-MM`.
        bilanzierungsmonat: BillingPeriod,
        /// Ascending version number.
        version: SzrVersion,
        /// Whether it arrived inside the Erstaufschlag window.
        im_erstaufschlag: bool,
        /// Party that sent it.
        absender: MarktpartnerCode,
        /// Bilanzkoordinator of this settlement.
        biko_id: BikoId,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// This participant sent a Prüfmitteilung for one version.
    PruefmitteilungSent {
        /// Version the check refers to (Kap. 3.8.3: „Eine Prüfmitteilung
        /// bezieht sich immer auf eine Version").
        version: SzrVersion,
        /// IFTSTA PID it went out on (21000 / 21001 / 21005).
        pid: Pruefidentifikator,
        /// Outcome.
        ergebnis: Pruefergebnis,
        /// Message reference of the dispatched IFTSTA.
        message_ref: MessageRef,
    },
    /// The BIKO rejected a Prüfmitteilung (IFTSTA 21002); it was never
    /// forwarded to the responsible party.
    PruefmitteilungAbgewiesen {
        /// Version the rejected check referred to.
        version: SzrVersion,
        /// Rejection reason from the BIKO.
        grund: String,
        /// Message reference of the inbound IFTSTA.
        message_ref: MessageRef,
    },
    /// The BIKO assigned a Datenstatus to one version (IFTSTA 21003 / 21004).
    DatenstatusReceived {
        /// Version the status refers to.
        version: SzrVersion,
        /// The status assigned.
        datenstatus: Datenstatus,
        /// IFTSTA PID it arrived on.
        pid: Pruefidentifikator,
        /// Message reference of the inbound IFTSTA.
        message_ref: MessageRef,
    },
    /// The clearing window for this stream closed; no further version can
    /// change the settlement (Kap. 3.10 Tabelle 2).
    ClearingGeschlossen {
        /// Which run closed.
        lauf: crate::fristen::Abrechnungslauf,
    },
}

impl EventPayload for BillingEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::SummenzeitreiheReceived { .. } => "MabisSummenzeitreiheReceived",
            Self::PruefmitteilungSent { .. } => "MabisPruefmitteilungSent",
            Self::PruefmitteilungAbgewiesen { .. } => "MabisPruefmitteilungAbgewiesen",
            Self::DatenstatusReceived { .. } => "MabisDatenstatusReceived",
            Self::ClearingGeschlossen { .. } => "MabisClearingGeschlossen",
        }
    }
}

// ── Domain state ─────────────────────────────────────────────────────────────

/// Settlement facts shared by every version of one stream.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingData {
    /// Which Summenzeitreihe of Tabelle 1 this stream settles.
    pub zeitreihe: Zeitreihe,
    /// MaBiS-Zählpunkt the series is filed against.
    pub mabis_zp: MabisZaehlpunktId,
    /// Bilanzierungsmonat, `YYYY-MM`.
    pub bilanzierungsmonat: BillingPeriod,
    /// Bilanzkoordinator of this settlement.
    pub biko_id: BikoId,
    /// Every version received, in arrival order.
    pub versionen: Vec<VersionRecord>,
}

impl BillingData {
    /// The record for `version`, if it was received.
    #[must_use]
    pub fn version(&self, version: &SzrVersion) -> Option<&VersionRecord> {
        self.versionen.iter().find(|v| &v.version == version)
    }

    /// Highest version received so far.
    #[must_use]
    pub fn hoechste_version(&self) -> Option<&SzrVersion> {
        self.versionen.iter().map(|v| &v.version).max()
    }

    /// The version that settles: the highest one carrying an
    /// abrechnungsrelevanter or already-settled Datenstatus (Kap. 3.8.3).
    #[must_use]
    pub fn abrechnungsrelevante_version(&self) -> Option<&VersionRecord> {
        self.versionen
            .iter()
            .filter(|v| {
                v.datenstatus
                    .is_some_and(|d| d.ist_abrechnungsrelevant() || d.ist_abgerechnet())
            })
            .max_by_key(|v| &v.version)
    }

    /// Versions this participant checked negatively and has not seen corrected
    /// — the open Korrekturbedarf.
    #[must_use]
    pub fn offener_korrekturbedarf(&self) -> Vec<&SzrVersion> {
        let hoechste = self.hoechste_version();
        self.versionen
            .iter()
            .filter(|v| {
                matches!(v.pruefergebnis, Some(Pruefergebnis::Negativ { .. }))
                    && Some(&v.version) == hoechste
            })
            .map(|v| &v.version)
            .collect()
    }
}

/// Lifecycle of one Summenzeitreihen-Settlement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum BillingState {
    /// No version received yet.
    #[default]
    New,
    /// At least one version received; versions may still arrive and be checked.
    Offen(BillingData),
    /// The clearing window closed. Kap. 3.10: „Danach dürfen die Prozesse …
    /// nicht erneut gestartet werden."
    Geschlossen(BillingData),
}

impl BillingState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Offen(_) => "Offen",
            Self::Geschlossen(_) => "Geschlossen",
        }
    }

    /// Settlement facts, once the first version has arrived.
    #[must_use]
    pub fn data(&self) -> Option<&BillingData> {
        match self {
            Self::New => None,
            Self::Offen(d) | Self::Geschlossen(d) => Some(d),
        }
    }
}

// ── Domain commands ──────────────────────────────────────────────────────────

/// Commands for the MaBiS Bilanzkreisabrechnung workflow.
///
/// `Workflow::handle()` is pure — no I/O, no EDIFACT parsing, no clock.
#[derive(Clone)]
pub enum BillingCommand {
    /// A version of the Summenzeitreihe arrived (MSCONS 13003).
    ///
    /// `im_erstaufschlag` comes from
    /// [`crate::fristen::Bilanzierungsmonat::phase`] evaluated on the arrival
    /// date — it is a calendar fact about this Zeitreihe, not something the
    /// message states.
    ReceiveSummenzeitreihe {
        /// MSCONS PID; must be 13003.
        pid: Pruefidentifikator,
        /// Which Summenzeitreihe of Tabelle 1 this stream settles.
        zeitreihe: Zeitreihe,
        /// MaBiS-Zählpunkt the series is filed against.
        mabis_zp: MabisZaehlpunktId,
        /// Bilanzierungsmonat, `YYYY-MM`.
        bilanzierungsmonat: BillingPeriod,
        /// Ascending version number.
        version: SzrVersion,
        /// Whether the arrival date fell inside the Erstaufschlag window.
        im_erstaufschlag: bool,
        /// Party that sent it.
        absender: MarktpartnerCode,
        /// Bilanzkoordinator of this settlement.
        biko_id: BikoId,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Send a Prüfmitteilung for one version (IFTSTA 21000 / 21001 / 21005).
    ///
    /// There is no Frist on this — see the module docs. It is refused once the
    /// clearing window has closed, because a check that can no longer change
    /// the settlement is not a check.
    SendPruefmitteilung {
        /// Version the check refers to.
        version: SzrVersion,
        /// IFTSTA PID to send on.
        pid: Pruefidentifikator,
        /// The **Antwortcode** the check landed on, as published by the tree
        /// that decides this Summenzeitreihe (`A03` in `E_0062`, `A06` in
        /// `E_0041`, …).
        ///
        /// A bare code and not a `Pruefergebnis`: which tree applies follows
        /// from the series the stream settles, which only the workflow knows.
        /// Resolving it here is what makes „positive check" and „negative
        /// check" impossible to state without naming a code that exists.
        antwortcode: String,
        /// Free-text Erläuterung. Required for every code that is not the
        /// tree's Zustimmung, and for any code whose Codeliste marks it as
        /// needing one.
        grund: Option<String>,
        /// Message reference assigned to the outbound IFTSTA.
        message_ref: MessageRef,
    },
    /// An inbound MaBiS IFTSTA arrived (21002 / 21003 / 21004).
    ///
    /// The PID decides what it means: 21002 rejects a Prüfmitteilung,
    /// 21003 and 21004 both carry a Datenstatus.
    ReceiveIftsta {
        /// IFTSTA Prüfidentifikator.
        pid: Pruefidentifikator,
        /// Version the message refers to.
        version: SzrVersion,
        /// Datenstatus, required for 21003 and 21004.
        datenstatus: Option<Datenstatus>,
        /// Rejection reason, required for 21002.
        abweisungsgrund: Option<String>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// The clearing window closed (Kap. 3.10 Tabelle 2). Emitted by the caller
    /// from [`crate::fristen::Phase`], typically off a
    /// [`crate::fristen::CLEARING_ENDE_LABEL`] deadline.
    CloseClearing {
        /// Which run closed.
        lauf: crate::fristen::Abrechnungslauf,
    },
}

impl CommandPayload for BillingCommand {}

// ── Workflow ─────────────────────────────────────────────────────────────────

/// MaBiS Bilanzkreisabrechnung Strom workflow.
pub struct MabisBillingWorkflow;

impl Workflow for MabisBillingWorkflow {
    type State = BillingState;
    type Event = BillingEvent;
    type Command = BillingCommand;

    /// Close the settlement when the clearing window lapses.
    ///
    /// This is the only deadline the workflow owns. It is not a response
    /// window: nothing is owed *by* this participant when it fires, the
    /// settlement simply stops accepting versions.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (crate::fristen::CLEARING_ENDE_LABEL, BillingState::Offen(_)) => {
                Some(BillingCommand::CloseClearing {
                    lauf: crate::fristen::Abrechnungslauf::Bka,
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            BillingEvent::SummenzeitreiheReceived {
                zeitreihe,
                mabis_zp,
                bilanzierungsmonat,
                version,
                im_erstaufschlag,
                biko_id,
                message_ref,
                ..
            } => {
                let record = VersionRecord {
                    version: version.clone(),
                    message_ref: message_ref.clone(),
                    im_erstaufschlag: *im_erstaufschlag,
                    pruefergebnis: None,
                    datenstatus: None,
                    pruefmitteilung_abgewiesen: None,
                };
                match state {
                    BillingState::Offen(mut d) => {
                        d.versionen.push(record);
                        BillingState::Offen(d)
                    }
                    BillingState::New => BillingState::Offen(BillingData {
                        zeitreihe: *zeitreihe,
                        mabis_zp: mabis_zp.clone(),
                        bilanzierungsmonat: bilanzierungsmonat.clone(),
                        biko_id: biko_id.clone(),
                        versionen: vec![record],
                    }),
                    // A closed settlement takes no further version.
                    other @ BillingState::Geschlossen(_) => other,
                }
            }

            BillingEvent::PruefmitteilungSent {
                version, ergebnis, ..
            } => mutate_version(state, version, |v| {
                v.pruefergebnis = Some(ergebnis.clone());
                // A fresh check supersedes an earlier Abweisung.
                v.pruefmitteilung_abgewiesen = None;
            }),

            BillingEvent::PruefmitteilungAbgewiesen { version, grund, .. } => {
                mutate_version(state, version, |v| {
                    v.pruefergebnis = None;
                    v.pruefmitteilung_abgewiesen = Some(grund.clone());
                })
            }

            BillingEvent::DatenstatusReceived {
                version,
                datenstatus,
                ..
            } => mutate_version(state, version, |v| v.datenstatus = Some(*datenstatus)),

            BillingEvent::ClearingGeschlossen { .. } => match state {
                BillingState::Offen(d) => BillingState::Geschlossen(d),
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            BillingCommand::ReceiveSummenzeitreihe {
                pid,
                zeitreihe,
                mabis_zp,
                bilanzierungsmonat,
                version,
                im_erstaufschlag,
                absender,
                biko_id,
                message_ref,
            } => {
                if !ist_zeitreihen_pid(pid.as_u32()) {
                    return Err(WorkflowError::not_implemented(pid.as_u32()));
                }
                if !zeitreihe.hat_pruefmitteilung_und_datenstatus() {
                    // Kap. 3.8.3: BG-SZR and BK-SZR of Kategorie C carry
                    // neither, so they do not settle and have no stream here.
                    return Err(WorkflowError::validation(format!(
                        "{zeitreihe} sendet weder Prüfmitteilung noch Datenstatus \
                         (BK6-24-174 Anlage 3 Kap. 3.8.3) und wird nicht abgerechnet"
                    )));
                }
                match state {
                    BillingState::Geschlossen(_) => {
                        return Err(WorkflowError::invalid_state("Offen", "Geschlossen"));
                    }
                    BillingState::Offen(d) => {
                        // Kap. 3.8.2: versions ascend across the whole BKA.
                        if let Some(h) = d.hoechste_version()
                            && &version <= h
                        {
                            return Err(WorkflowError::validation(format!(
                                "Version {version} ist nicht aufsteigend — \
                                 zuletzt empfangen {h} (BK6-24-174 Anlage 3 Kap. 3.8.2)"
                            )));
                        }
                        if d.zeitreihe != zeitreihe {
                            return Err(WorkflowError::validation(format!(
                                "Stream führt {} — {zeitreihe} gehört in einen eigenen Stream",
                                d.zeitreihe
                            )));
                        }
                    }
                    BillingState::New => {}
                }
                Ok(vec![BillingEvent::SummenzeitreiheReceived {
                    zeitreihe,
                    mabis_zp,
                    bilanzierungsmonat,
                    version,
                    im_erstaufschlag,
                    absender,
                    biko_id,
                    message_ref,
                }]
                .into())
            }

            BillingCommand::SendPruefmitteilung {
                version,
                pid,
                antwortcode,
                grund,
                message_ref,
            } => {
                let data = open_data(state)?;
                if !IFTSTA_PRUEFMITTEILUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::validation(format!(
                        "PID {} trägt keine Prüfmitteilung dieses Teilnehmers — \
                         erwartet 21000, 21001 oder 21005",
                        pid.as_u32()
                    )));
                }
                if data.version(&version).is_none() {
                    return Err(WorkflowError::validation(format!(
                        "Prüfmitteilung zu {version}, die nie empfangen wurde \
                         (Kap. 3.8.3: eine Prüfmitteilung bezieht sich immer auf eine Version)"
                    )));
                }
                // The code is resolved against the tree that decides *this*
                // series, so a code from another Codeliste cannot be stated at
                // all: `A02` is „Energiemenge falsch" in `E_0062` and
                // „Gewählter Zeitraum nicht zulässig" in `E_0041`.
                let ergebnis = Pruefergebnis::aus_code(
                    data.zeitreihe,
                    &antwortcode,
                    grund.as_deref().unwrap_or_default(),
                )
                .map_err(|e| WorkflowError::validation(e.to_string()))?;
                if !ergebnis.ist_positiv() && grund.as_deref().unwrap_or_default().trim().is_empty()
                {
                    return Err(WorkflowError::validation(
                        "eine negative Prüfmitteilung ohne Grund ist nicht zustellbar",
                    ));
                }
                if ergebnis.antwort().braucht_bemerkung
                    && grund.as_deref().unwrap_or_default().trim().is_empty()
                {
                    return Err(WorkflowError::validation(format!(
                        "{} verlangt eine Erläuterung (FTX+ACB)",
                        ergebnis.antwort().code
                    )));
                }
                Ok(vec![BillingEvent::PruefmitteilungSent {
                    version,
                    pid,
                    ergebnis,
                    message_ref,
                }]
                .into())
            }

            BillingCommand::ReceiveIftsta {
                pid,
                version,
                datenstatus,
                abweisungsgrund,
                message_ref,
            } => {
                let data = open_data(state)?;
                if data.version(&version).is_none() {
                    return Err(WorkflowError::validation(format!(
                        "IFTSTA {} verweist auf {version}, die nie empfangen wurde",
                        pid.as_u32()
                    )));
                }
                let raw = pid.as_u32();
                if raw == IFTSTA_ABWEISUNG_PID {
                    let grund = abweisungsgrund.ok_or_else(|| {
                        WorkflowError::validation(
                            "IFTSTA 21002 (Abweisung der Prüfmitteilung): Grund ist erforderlich",
                        )
                    })?;
                    return Ok(vec![BillingEvent::PruefmitteilungAbgewiesen {
                        version,
                        grund,
                        message_ref,
                    }]
                    .into());
                }
                if IFTSTA_DATENSTATUS_PIDS.contains(&raw) {
                    let ds = datenstatus.ok_or_else(|| {
                        WorkflowError::validation(format!(
                            "IFTSTA {raw} (Datenstatus): der Datenstatus-Code ist erforderlich"
                        ))
                    })?;
                    return Ok(vec![BillingEvent::DatenstatusReceived {
                        version,
                        datenstatus: ds,
                        pid,
                        message_ref,
                    }]
                    .into());
                }
                // 21000/21001/21005 are this participant's own outbound
                // Prüfmitteilungen. Accepting one as inbound would record a
                // check nobody performed.
                Err(WorkflowError::validation(format!(
                    "IFTSTA {raw} ist eine ausgehende Prüfmitteilung dieses Teilnehmers \
                     und kein Eingang — erwartet 21002, 21003 oder 21004"
                )))
            }

            BillingCommand::CloseClearing { lauf } => {
                if !matches!(state, BillingState::Offen(_)) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![BillingEvent::ClearingGeschlossen { lauf }].into())
            }
        }
    }
}

/// Apply `f` to the record of `version`, leaving every other state untouched.
fn mutate_version(
    state: BillingState,
    version: &SzrVersion,
    f: impl FnOnce(&mut VersionRecord),
) -> BillingState {
    match state {
        BillingState::Offen(mut d) => {
            if let Some(v) = d.versionen.iter_mut().find(|v| &v.version == version) {
                f(v);
            }
            BillingState::Offen(d)
        }
        BillingState::Geschlossen(mut d) => {
            // A Datenstatus („abgerechnete Daten") still lands after the window
            // closes — it is the BIKO recording what settled.
            if let Some(v) = d.versionen.iter_mut().find(|v| &v.version == version) {
                f(v);
            }
            BillingState::Geschlossen(d)
        }
        new @ BillingState::New => new,
    }
}

/// The settlement facts of a stream that is still open, or an error naming the
/// state that blocked.
fn open_data(state: &BillingState) -> Result<&BillingData, WorkflowError> {
    match state {
        BillingState::Offen(d) => Ok(d),
        other => Err(WorkflowError::invalid_state("Offen", other.status_str())),
    }
}

// ── Read-model projection ────────────────────────────────────────────────────

/// Read-model record for one MaBiS settlement stream.
#[derive(Debug, Default)]
pub struct BillingRecord {
    /// Current lifecycle stage.
    pub status: &'static str,
    /// Which Summenzeitreihe the stream settles, once known.
    pub zeitreihe: Option<Zeitreihe>,
    /// Bilanzierungsmonat, once known.
    pub bilanzierungsmonat: Option<BillingPeriod>,
    /// Highest version received.
    pub hoechste_version: Option<SzrVersion>,
    /// Datenstatus of the highest version, as last assigned by the BIKO.
    pub datenstatus: Option<Datenstatus>,
    /// Versions for which this participant sent a negative Prüfmitteilung that
    /// has not been superseded.
    pub offene_korrekturen: Vec<SzrVersion>,
    /// Total events applied.
    pub event_count: usize,
}

/// In-process read model across MaBiS settlement streams.
#[derive(Debug, Default)]
pub struct BillingProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, BillingRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for BillingProjection {
    fn name(&self) -> &'static str {
        "BillingProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let Ok(event) = envelope.decode::<BillingEvent>() else {
            return;
        };
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        record.event_count += 1;

        match event {
            BillingEvent::SummenzeitreiheReceived {
                zeitreihe,
                bilanzierungsmonat,
                version,
                ..
            } => {
                record.status = "Offen";
                record.zeitreihe = Some(zeitreihe);
                record.bilanzierungsmonat = Some(bilanzierungsmonat);
                if record
                    .hoechste_version
                    .as_ref()
                    .is_none_or(|h| &version > h)
                {
                    record.hoechste_version = Some(version);
                    // A new highest version carries no status until the BIKO
                    // assigns one, and supersedes the previous correction need.
                    record.datenstatus = None;
                    record.offene_korrekturen.clear();
                }
            }
            BillingEvent::PruefmitteilungSent {
                version, ergebnis, ..
            } => {
                record.offene_korrekturen.retain(|v| *v != version);
                if !ergebnis.ist_positiv() {
                    record.offene_korrekturen.push(version);
                }
            }
            BillingEvent::PruefmitteilungAbgewiesen { version, .. } => {
                record.offene_korrekturen.retain(|v| *v != version);
            }
            BillingEvent::DatenstatusReceived {
                version,
                datenstatus,
                ..
            } => {
                if record.hoechste_version.as_ref() == Some(&version) {
                    record.datenstatus = Some(datenstatus);
                }
            }
            BillingEvent::ClearingGeschlossen { .. } => {
                record.status = "Geschlossen";
            }
        }
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zeitreihen::{Familie, Kategorie};

    fn bg() -> Zeitreihe {
        Zeitreihe::new(Familie::BgSzr, Some(Kategorie::B)).expect("Tabelle-1 row")
    }

    /// Build a version from an ordinal — the wire form is an
    /// Erstellungszeitpunkt, so the test ordinals become ascending seconds.
    fn v(n: u32) -> SzrVersion {
        SzrVersion::new(format!("202601011200{n:02}+00")).expect("17 chars")
    }

    fn zp() -> MabisZaehlpunktId {
        MabisZaehlpunktId::new("DE0001111222233334444555566667777").expect("33 chars")
    }

    fn receive(version: u32, im_erstaufschlag: bool) -> BillingCommand {
        receive_as(SUMMENZEITREIHE_PID, bg(), version, im_erstaufschlag)
    }

    fn receive_as(
        pid: u32,
        zeitreihe: Zeitreihe,
        version: u32,
        im_erstaufschlag: bool,
    ) -> BillingCommand {
        BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            zeitreihe,
            mabis_zp: zp(),
            bilanzierungsmonat: BillingPeriod::new("2026-01"),
            version: v(version),
            im_erstaufschlag,
            absender: MarktpartnerCode::new("9900357000004"),
            biko_id: BikoId::new("10YDE-VE-TRANSMIX"),
            message_ref: MessageRef::new(format!("MSCONS-{version}")),
        }
    }

    fn apply_all(state: BillingState, events: &[BillingEvent]) -> BillingState {
        events.iter().fold(state, MabisBillingWorkflow::apply)
    }

    fn run(cmds: Vec<BillingCommand>) -> BillingState {
        cmds.into_iter().fold(BillingState::default(), |s, c| {
            let out = MabisBillingWorkflow::handle(&s, c).expect("command accepted");
            apply_all(s, &out.events)
        })
    }

    #[test]
    fn a_stream_accumulates_versions() {
        let state = run(vec![receive(1, true), receive(2, false), receive(7, false)]);
        let d = state.data().expect("open");
        assert_eq!(d.versionen.len(), 3);
        assert_eq!(d.hoechste_version(), Some(&v(7)));
        assert!(d.versionen[0].im_erstaufschlag);
        assert!(!d.versionen[1].im_erstaufschlag);
    }

    #[test]
    fn versions_must_ascend() {
        // Kap. 3.8.2 — a repeated or lower version is a filing error, not a
        // second attempt at the same one.
        let state = run(vec![receive(3, true)]);
        for v in [1, 3] {
            assert!(
                MabisBillingWorkflow::handle(&state, receive(v, false)).is_err(),
                "version {v} must be refused after 3"
            );
        }
        assert!(MabisBillingWorkflow::handle(&state, receive(4, false)).is_ok());
    }

    #[test]
    fn a_kategorie_c_series_has_no_settlement_stream() {
        // Kap. 3.8.3 — neither Prüfmitteilung nor Datenstatus, so nothing to
        // settle and nothing this workflow can do with it.
        let cmd = receive_as(
            SUMMENZEITREIHE_PID,
            Zeitreihe::new(Familie::BgSzr, Some(Kategorie::C)).unwrap(),
            1,
            true,
        );
        assert!(MabisBillingWorkflow::handle(&BillingState::New, cmd).is_err());
    }

    #[test]
    fn only_the_mabis_zeitreihen_pids_open_a_settlement() {
        for pid in [SUMMENZEITREIHE_PID, 13_020, 13_023] {
            assert!(ist_zeitreihen_pid(pid), "{pid}");
            let cmd = receive_as(pid, bg(), 1, true);
            assert!(MabisBillingWorkflow::handle(&BillingState::New, cmd).is_ok());
        }
        // 13021 meteorologische Daten, 13022 TR-scharfe Einzelzeitreihe and
        // 13026 EEG-Überführungszeitreihe are not MaBiS Summenzeitreihen.
        for pid in [13_021_u32, 13_022, 13_026] {
            assert!(!ist_zeitreihen_pid(pid), "{pid}");
            let cmd = receive_as(pid, bg(), 1, true);
            assert!(MabisBillingWorkflow::handle(&BillingState::New, cmd).is_err());
        }
    }

    #[test]
    fn pruefmitteilung_needs_a_version_that_arrived() {
        let state = run(vec![receive(1, true)]);
        let cmd = BillingCommand::SendPruefmitteilung {
            version: v(2),
            pid: Pruefidentifikator::new(21_005).expect("valid"),
            antwortcode: "A03".into(),
            grund: None,
            message_ref: MessageRef::new("PM-1"),
        };
        assert!(MabisBillingWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn an_inbound_pid_cannot_be_sent_as_a_pruefmitteilung() {
        let state = run(vec![receive(1, true)]);
        for pid in [21_002_u32, 21_003, 21_004] {
            let cmd = BillingCommand::SendPruefmitteilung {
                version: v(1),
                pid: Pruefidentifikator::new(pid).expect("valid"),
                antwortcode: "A03".into(),
                grund: None,
                message_ref: MessageRef::new("PM-1"),
            };
            assert!(
                MabisBillingWorkflow::handle(&state, cmd).is_err(),
                "{pid} is inbound"
            );
        }
    }

    #[test]
    fn an_outbound_pid_cannot_arrive_as_an_inbound_iftsta() {
        // 21000/21001/21005 are this participant's own Prüfmitteilungen.
        let state = run(vec![receive(1, true)]);
        for pid in IFTSTA_PRUEFMITTEILUNG_PIDS {
            let cmd = BillingCommand::ReceiveIftsta {
                pid: Pruefidentifikator::new(*pid).expect("valid"),
                version: v(1),
                datenstatus: Some(Datenstatus::Abrechnungsdaten),
                abweisungsgrund: None,
                message_ref: MessageRef::new("IN-1"),
            };
            assert!(
                MabisBillingWorkflow::handle(&state, cmd).is_err(),
                "{pid} is outbound"
            );
        }
    }

    #[test]
    fn both_21003_and_21004_carry_a_datenstatus() {
        for pid in IFTSTA_DATENSTATUS_PIDS {
            let state = run(vec![
                receive(1, false),
                BillingCommand::ReceiveIftsta {
                    pid: Pruefidentifikator::new(*pid).expect("valid"),
                    version: v(1),
                    datenstatus: Some(Datenstatus::Pruefdaten),
                    abweisungsgrund: None,
                    message_ref: MessageRef::new("DS-1"),
                },
            ]);
            assert_eq!(
                state.data().unwrap().version(&v(1)).unwrap().datenstatus,
                Some(Datenstatus::Pruefdaten),
                "PID {pid} must set the Datenstatus"
            );
        }
    }

    #[test]
    fn a_datenstatus_message_without_a_code_is_refused() {
        let state = run(vec![receive(1, false)]);
        let cmd = BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_004).expect("valid"),
            version: v(1),
            datenstatus: None,
            abweisungsgrund: None,
            message_ref: MessageRef::new("DS-1"),
        };
        assert!(MabisBillingWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn a_negative_pruefmitteilung_leaves_the_datenstatus_alone() {
        // Kap. 3.8.3: „Eine negative Prüfmitteilung verändert nicht den
        // Datenstatus einer Summenzeitreihe."
        let state = run(vec![
            receive(1, true),
            BillingCommand::ReceiveIftsta {
                pid: Pruefidentifikator::new(21_004).expect("valid"),
                version: v(1),
                datenstatus: Some(Datenstatus::Abrechnungsdaten),
                abweisungsgrund: None,
                message_ref: MessageRef::new("DS-1"),
            },
            BillingCommand::SendPruefmitteilung {
                version: v(1),
                pid: Pruefidentifikator::new(21_005).expect("valid"),
                antwortcode: "A02".into(),
                grund: Some("Summe weicht um 12 kWh ab".into()),
                message_ref: MessageRef::new("PM-1"),
            },
        ]);
        let v = state.data().unwrap().version(&v(1)).unwrap();
        assert_eq!(v.datenstatus, Some(Datenstatus::Abrechnungsdaten));
        assert!(matches!(
            v.pruefergebnis,
            Some(Pruefergebnis::Negativ { .. })
        ));
    }

    #[test]
    fn a_negative_pruefmitteilung_needs_a_reason() {
        let state = run(vec![receive(1, true)]);
        let cmd = BillingCommand::SendPruefmitteilung {
            version: v(1),
            pid: Pruefidentifikator::new(21_005).expect("valid"),
            antwortcode: "A02".into(),
            grund: Some("   ".into()),
            message_ref: MessageRef::new("PM-1"),
        };
        assert!(MabisBillingWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn an_abgewiesene_pruefmitteilung_clears_the_check() {
        // Kap. 9.8.2 Nr. 2: a rejected Prüfmitteilung is never forwarded, so
        // the responsible party never saw it — the check has to be redone.
        let state = run(vec![
            receive(1, true),
            BillingCommand::SendPruefmitteilung {
                version: v(1),
                pid: Pruefidentifikator::new(21_005).expect("valid"),
                antwortcode: "A03".into(),
                grund: None,
                message_ref: MessageRef::new("PM-1"),
            },
            BillingCommand::ReceiveIftsta {
                pid: Pruefidentifikator::new(IFTSTA_ABWEISUNG_PID).expect("valid"),
                version: v(1),
                datenstatus: None,
                abweisungsgrund: Some("MaBiS-ZP nicht aktiv".into()),
                message_ref: MessageRef::new("AB-1"),
            },
        ]);
        let v = state.data().unwrap().version(&v(1)).unwrap();
        assert!(v.pruefergebnis.is_none(), "the check no longer stands");
        assert_eq!(
            v.pruefmitteilung_abgewiesen.as_deref(),
            Some("MaBiS-ZP nicht aktiv")
        );
    }

    #[test]
    fn an_abweisung_without_a_reason_is_refused() {
        let state = run(vec![receive(1, true)]);
        let cmd = BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(IFTSTA_ABWEISUNG_PID).expect("valid"),
            version: v(1),
            datenstatus: None,
            abweisungsgrund: None,
            message_ref: MessageRef::new("AB-1"),
        };
        assert!(MabisBillingWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn the_highest_abrechnungsrelevante_version_settles() {
        let ds = |n: u32, s: Datenstatus| BillingCommand::ReceiveIftsta {
            pid: Pruefidentifikator::new(21_004).expect("valid"),
            version: v(n),
            datenstatus: Some(s),
            abweisungsgrund: None,
            message_ref: MessageRef::new(format!("DS-{n}")),
        };
        let state = run(vec![
            receive(1, true),
            ds(1, Datenstatus::Abrechnungsdaten),
            receive(2, false),
            ds(2, Datenstatus::Pruefdaten),
            receive(3, false),
            ds(3, Datenstatus::Abrechnungsdaten),
        ]);
        let d = state.data().unwrap();
        assert_eq!(
            d.abrechnungsrelevante_version().map(|r| &r.version),
            Some(&v(3)),
            "V2 is only Prüfdaten, so V3 settles — not the highest version overall"
        );
    }

    #[test]
    fn a_closed_window_takes_no_further_version() {
        let state = run(vec![
            receive(1, true),
            BillingCommand::CloseClearing {
                lauf: crate::fristen::Abrechnungslauf::Bka,
            },
        ]);
        assert_eq!(state.status_str(), "Geschlossen");
        assert!(MabisBillingWorkflow::handle(&state, receive(2, false)).is_err());
    }

    #[test]
    fn a_datenstatus_still_lands_after_the_window_closed() {
        // The Abrechnungsstichtag is *after* the clearing window: the BIKO
        // assigns „abgerechnete Daten" on the 42. WT, long after the 30. WT.
        let state = run(vec![
            receive(1, true),
            BillingCommand::CloseClearing {
                lauf: crate::fristen::Abrechnungslauf::Bka,
            },
        ]);
        let evt = BillingEvent::DatenstatusReceived {
            version: v(1),
            datenstatus: Datenstatus::AbgerechneteDaten,
            pid: Pruefidentifikator::new(21_004).expect("valid"),
            message_ref: MessageRef::new("DS-final"),
        };
        let state = MabisBillingWorkflow::apply(state, &evt);
        assert!(
            state
                .data()
                .unwrap()
                .version(&v(1))
                .unwrap()
                .datenstatus
                .unwrap()
                .ist_abgerechnet()
        );
    }

    #[test]
    fn closing_twice_is_a_no_op() {
        let state = run(vec![
            receive(1, true),
            BillingCommand::CloseClearing {
                lauf: crate::fristen::Abrechnungslauf::Bka,
            },
        ]);
        let out = MabisBillingWorkflow::handle(
            &state,
            BillingCommand::CloseClearing {
                lauf: crate::fristen::Abrechnungslauf::Kbka,
            },
        )
        .expect("idempotent");
        assert!(out.events.is_empty());
    }

    #[test]
    fn korrekturbedarf_tracks_only_the_highest_version() {
        let state = run(vec![
            receive(1, true),
            BillingCommand::SendPruefmitteilung {
                version: v(1),
                pid: Pruefidentifikator::new(21_005).expect("valid"),
                antwortcode: "A02".into(),
                grund: Some("Abweichung".into()),
                message_ref: MessageRef::new("PM-1"),
            },
        ]);
        assert_eq!(state.data().unwrap().offener_korrekturbedarf(), vec![&v(1)]);
        // The correction arrives as a new version: the need is met.
        let state = {
            let out = MabisBillingWorkflow::handle(&state, receive(2, false)).unwrap();
            apply_all(state, &out.events)
        };
        assert!(state.data().unwrap().offener_korrekturbedarf().is_empty());
    }

    #[test]
    fn no_command_is_accepted_before_the_first_version() {
        for cmd in [
            BillingCommand::SendPruefmitteilung {
                version: v(1),
                pid: Pruefidentifikator::new(21_005).expect("valid"),
                antwortcode: "A03".into(),
                grund: None,
                message_ref: MessageRef::new("PM-1"),
            },
            BillingCommand::ReceiveIftsta {
                pid: Pruefidentifikator::new(21_004).expect("valid"),
                version: v(1),
                datenstatus: Some(Datenstatus::Pruefdaten),
                abweisungsgrund: None,
                message_ref: MessageRef::new("DS-1"),
            },
        ] {
            assert!(MabisBillingWorkflow::handle(&BillingState::New, cmd).is_err());
        }
    }
}
