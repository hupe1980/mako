//! MaBiS-Zählpunkt lifecycle — activation and deactivation of MaBiS-ZP,
//! Zuordnungsermächtigung, and the Ausfallarbeitsüberführungszeitreihen (AAÜZ)
//! series.
//!
//! # Process overview
//!
//! Every process in this family has the same shape: one party sends an
//! **Anfrage** that activates or deactivates a MaBiS-Zählpunkt for a given
//! series, and — depending on the family — the counterparty returns an
//! **Antwort**, after which the receiving party may forward a
//! **Weiterleitung** to a third party.
//!
//! ```text
//! Anfrage ──→ (Antwort) ──→ (Weiterleitung)
//!  step 1        step 2          step 4
//! ```
//!
//! Only three of the six families carry an Antwort PID, and only two carry a
//! Weiterleitung. A family without an Antwort is **record-only**: the message
//! is validated and stored, and the process is terminal on arrival. Modelling
//! those as request/response would manufacture a deadline the AHB never
//! defines.
//!
//! # Prüfidentifikatoren
//!
//! Verified against the BDEW *Anwendungsübersicht Prüfidentifikatoren 4.0*
//! (01.04.2026), sheet *Prüf-ID Prozessschritt* — the Prozessschritt column is
//! what distinguishes an Anfrage (1) from an Antwort (2) and a Weiterleitung
//! (4).
//!
//! ## 55062 / 55063 / 55064 are generic codes, not one process
//!
//! This is the trap the family table exists to close. **55062 „Aktivierung von
//! ZP" and 55063 „Deaktivierung von ZP" are used for eleven different
//! Summenzeitreihen**, and 55064 „Antwort" answers all of them — out of
//! **twelve different Entscheidungsbäume**:
//!
//! | Serie | Achse | Antwort | EBD Aktivierung | EBD Deaktivierung |
//! |-------|-------|--------:|-----------------|-------------------|
//! | Netzzeitreihe | NB (verantw.) → NB (benachbart) | 55064 | `E_0020` | `E_0010` |
//! | Netzzeitreihe | NB (verantw.) → BIKO | 55064 | `E_0024` | `E_0009` |
//! | Lieferantensummenzeitreihe | NB → LF | — | — | — |
//! | Lieferantensummenzeitreihe | ÜNB → LF | — | — | — |
//! | Bilanzierungsgebietssummenzeitreihe | ÜNB → BIKO | 55064 | `E_0015` | `E_0035` |
//! | Bilanzkreissummenzeitreihe | NB → BIKO | 55064 | `E_0034` | `E_0018` |
//! | Bilanzkreissummenzeitreihe | ÜNB → BIKO | 55064 | `E_0011` | `E_0012` |
//! | Deltazeitreihenübertrag | ÜNB → BIKO | 55064 | `E_0027` | `E_0028` |
//! | Abrechnungssummenzeitreihe | BIKO → NB / BKV / ÜNB | — | — | — |
//! | tägliche Bilanzierungsgebietssummenzeitreihe | ÜNB → NB | — | — | — |
//! | tägliche Bilanzkreissummenzeitreihe | ÜNB → BKV | — | — | — |
//!
//! Three consequences follow, and none of them is derivable from the PID:
//!
//! - **Whether an Antwort is owed at all** varies by series. Six of the eleven
//!   owe a 55064 and five are record-only. A model that answers "55062 → 55064"
//!   invents five obligations; one that never answers drops six real ones.
//! - **Which Codeliste the answer comes from** varies by series *and*
//!   direction. A code read against the wrong tree means something else there —
//!   the same trap `A02` sets across the GPKE trees.
//! - **The Weiterleitung re-uses the request code.** For the four series that
//!   have one, Prozessschritt 4 is another 55062/55063 addressed to the
//!   downstream party, not a distinct PID.
//!
//! [`ZpSerie`] therefore carries the series *and* its axis, and it is an
//! explicit input: the MaBiS-Zählpunkt is created **for** one Summenzeitreihe,
//! so the caller always knows which.
//!
//! ## The series with their own codes
//!
//! | Serie | Anfrage | Antwort | EBD | Weiterleitung |
//! |-------|--------:|--------:|-----|--------------:|
//! | Zuordnungsermächtigung (BKV → NB) | 55071 / 55072 | — | — | — |
//! | tägliche AAÜZ (NB (ANB) → ÜNB) | 55197 / 55198 | — | — | — |
//! | LF-AASZR (NB (ANB) → LF) | 55199 / 55200 | — | — | — |
//! | monatliche AAÜZ, BKV des LF (NB (ANB) → BIKO) | 55203 / 55206 | 55204 / 55207 | `E_0071` / `E_0072` | 55205 / 55208 |
//! | monatliche AAÜZ, BKV des anfNB (NB (ANB) → BIKO) | 55209 / 55212 | 55210 / 55213 | `E_0078` / `E_0079` | 55211 / 55214 |
//!
//! ## The tägliche AAÜZ expires on 30.09.2026
//!
//! 55197/55198 implement MaBiS Anlage 1 **Kapitel 17.2**, which BK6-23-241
//! Tenorziffer 5 repeals with the end of **30.09.2026**. Unlike the rest of
//! Kapitel 17 it is *not* republished as the Anlage zur BilAReM: 17.2 and
//! 17.3.2.1 are the two parts that simply stop. [`ZpSerie::endet_am`] carries
//! the date so a deployment can refuse to activate a Zählpunkt for a series
//! that will not exist when the month it settles is due.
//!
//! # Not in this family
//!
//! 55218 and 55220 (Abr.-Daten NNA) sit in the same numeric neighbourhood but
//! belong to **GPKE Teil 2**, not MaBiS. 55215–55217, 55219, 55221 and 55222
//! are unassigned. Neither group is routed here.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 3 (MaBiS)** — Bilanzkreisabrechnung, ZP
//!   activation and the AAÜZ series
//! - **UTILMD AHB Strom S2.1 / S2.2** — message format
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ AnfrageErhalten ─┬─ (validation failed) ─→ ValidationFailed  (terminal)
//!                      ├─ (no Antwort PID)    ─→ Erfasst           (terminal)
//!                      └─ AntwortGesendet ────┬─ (abgelehnt) ──────→ Abgelehnt (terminal)
//!                                             └─ (bestätigt) ──────→ Bestaetigt
//!                                                  └─ WeiterleitungGesendet → Weitergeleitet (terminal)
//! ```

//! # On the wire
//!
//! `BGM+Z07` „Aktivierung/Deaktivierung von MaBiS-ZP" — not the `E01` an
//! Anmeldung uses, because a Zählpunkt is activated rather than angemeldet.
//! The object is a **MaBiS-Zählpunkt** in `SG5 LOC+Z15` and no Marktlokation;
//! the date is `SG4 DTM+158` Bilanzierungsbeginn on an Aktivierung and
//! `DTM+159` Bilanzierungsende on a Deaktivierung, never a Vertragsdatum
//! (UTILMD AHB Strom 2.2 Kap. 13.3).
//!
//! The 55064 answer carries `SG4 STS+E01` DE 1131 — which of the twelve
//! Entscheidungsbäume decided it — but **not** DE 9013. Only `E_0010` and
//! `E_0020` have walks in `mako_pruefung::mabis::zp`; the other ten publish
//! codes this workspace has not catalogued, and a fabricated Prüfschritt on a
//! message that settles a Bilanzkreisabrechnung is worse than an absent one.
//!

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Family table ──────────────────────────────────────────────────────────────

/// Whether the Anfrage activates or deactivates the series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZpVorgang {
    /// Aktivierung — the MaBiS-ZP starts contributing to the series.
    Aktivierung,
    /// Deaktivierung — the MaBiS-ZP stops contributing.
    Deaktivierung,
}

/// Which MaBiS series — and on which axis — the Anfrage activates or
/// deactivates.
///
/// The axis is part of the identity, not decoration: the Netzzeitreihe is
/// activated twice, once toward the neighbouring NB and once toward the BIKO,
/// and the two are answered out of different Entscheidungsbäume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZpSerie {
    /// Netzzeitreihe, verantwortlicher NB → benachbarter NB.
    NetzzeitreiheNachbarNb,
    /// Netzzeitreihe, verantwortlicher NB → BIKO.
    NetzzeitreiheBiko,
    /// Lieferantensummenzeitreihe (Kategorie A), NB → LF.
    LieferantensummenzeitreiheNb,
    /// Lieferantensummenzeitreihe (Kategorie B), ÜNB → LF.
    LieferantensummenzeitreiheUenb,
    /// Bilanzierungsgebietssummenzeitreihe, ÜNB → BIKO, weitergeleitet an den NB.
    Bilanzierungsgebietssummenzeitreihe,
    /// Bilanzkreissummenzeitreihe (Kategorie A), NB → BIKO, weitergeleitet an den BKV.
    BilanzkreissummenzeitreiheNb,
    /// Bilanzkreissummenzeitreihe (Kategorie B), ÜNB → BIKO, weitergeleitet an den BKV.
    BilanzkreissummenzeitreiheUenb,
    /// Deltazeitreihenübertrag, ÜNB → BIKO, weitergeleitet an den NB.
    Deltazeitreihenuebertrag,
    /// Abrechnungssummenzeitreihe, BIKO → NB / BKV / ÜNB.
    Abrechnungssummenzeitreihe,
    /// Tägliche Bilanzierungsgebietssummenzeitreihe, ÜNB → NB.
    TaeglicheBgSzr,
    /// Tägliche Bilanzkreissummenzeitreihe, ÜNB → BKV.
    TaeglicheBkSzr,
    /// Zuordnungsermächtigung des BKV beim NB (55071/55072).
    Zuordnungsermaechtigung,
    /// Tägliche Ausfallarbeitsüberführungszeitreihe (55197/55198), NB (ANB) → ÜNB.
    ///
    /// MaBiS Kap. 17.2 — repealed with the end of 30.09.2026 and **not**
    /// republished as the Anlage zur BilAReM.
    TaeglicheAauez,
    /// Lieferantenausfallarbeitssummenzeitreihe (55199/55200), NB (ANB) → LF.
    LfAaszr,
    /// Monatliche AAÜZ, forwarded to the BKV of the Lieferant (55203–55208).
    MonatlicheAauezBkvLf,
    /// Monatliche AAÜZ, forwarded to the BKV of the anfordernder NB (55209–55214).
    MonatlicheAauezBkvAnfNb,
    /// Zuordnung des Zählpunkts der **Netzgangzeitreihe** zur Netzzeitreihe
    /// (55235/55236/55237), verantwortlicher NB → benachbarter NB, informiert
    /// an den ÜNB.
    ///
    /// The NZR-EMob leg: a Modell-2 Übergabestelle's Netzgangzeitreihe has to
    /// be assigned to the receiving NB's Netzzeitreihe before any value flows
    /// (BDEW AWH Ergänzung MaBiS Netzgangzeitreihe Kap. 1.8.2). It is **MaBiS
    /// rather than Modell 2** — UTILMD AHB Strom 2.2 Kap. 13.16, answered from
    /// `E_0102`/`E_0103` — which is why it lives here and not in `mako-emob`.
    ///
    /// Unlike the 55062/55063 families this one has its own Anfrage codes, so
    /// [`Self::from_wire`] never returns it: there is nothing to disambiguate.
    NetzgangzeitreiheNzr,
}

/// Last day the tägliche AAÜZ process exists — BK6-23-241 Tenorziffer 5 repeals
/// MaBiS Anlage 1 Kap. 17.2 with the end of this day.
///
/// The tägliche AAÜZ **is** Kap. 17.2, so this is
/// [`crate::zeitreihen::KAPITEL_17_2_ENDE`] under the name the Zählpunkt side
/// asks for it by, not a second reading of the Tenor.
pub const TAEGLICHE_AAUEZ_ENDE: time::Date = crate::zeitreihen::KAPITEL_17_2_ENDE;

impl ZpSerie {
    /// Canonical BDEW name of the series, including its axis.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NetzzeitreiheNachbarNb => "Netzzeitreihe (NB → benachbarter NB)",
            Self::NetzzeitreiheBiko => "Netzzeitreihe (NB → BIKO)",
            Self::LieferantensummenzeitreiheNb => "Lieferantensummenzeitreihe (NB → LF)",
            Self::LieferantensummenzeitreiheUenb => "Lieferantensummenzeitreihe (ÜNB → LF)",
            Self::Bilanzierungsgebietssummenzeitreihe => "Bilanzierungsgebietssummenzeitreihe",
            Self::BilanzkreissummenzeitreiheNb => "Bilanzkreissummenzeitreihe (NB → BIKO)",
            Self::BilanzkreissummenzeitreiheUenb => "Bilanzkreissummenzeitreihe (ÜNB → BIKO)",
            Self::Deltazeitreihenuebertrag => "Deltazeitreihenübertrag",
            Self::Abrechnungssummenzeitreihe => "Abrechnungssummenzeitreihe",
            Self::TaeglicheBgSzr => "tägliche Bilanzierungsgebietssummenzeitreihe",
            Self::TaeglicheBkSzr => "tägliche Bilanzkreissummenzeitreihe",
            Self::Zuordnungsermaechtigung => "Zuordnungsermächtigung",
            Self::TaeglicheAauez => "tägliche AAÜZ",
            Self::LfAaszr => "LF-AASZR",
            Self::MonatlicheAauezBkvLf => "monatliche AAÜZ (BKV des LF)",
            Self::MonatlicheAauezBkvAnfNb => "monatliche AAÜZ (BKV des anfordernden NB)",
            Self::NetzgangzeitreiheNzr => "Zuordnung ZP der NGZ zur NZR",
        }
    }

    /// The last day this series exists, where a Festlegung ends it.
    ///
    /// Only the tägliche AAÜZ has one: BK6-23-241 Tenorziffer 5 repeals MaBiS
    /// Anlage 1 Kap. 17.2 with the end of 30.09.2026, and — unlike Kap. 17.1
    /// and 17.3 — it is not republished as the Anlage zur BilAReM.
    #[must_use]
    pub fn endet_am(self) -> Option<time::Date> {
        match self {
            Self::TaeglicheAauez => Some(TAEGLICHE_AAUEZ_ENDE),
            _ => None,
        }
    }

    /// Whether the series still exists on `date`.
    #[must_use]
    pub fn gilt_am(self, date: time::Date) -> bool {
        self.endet_am().is_none_or(|ende| date <= ende)
    }

    /// Resolve the series from what the UTILMD actually carries.
    ///
    /// The two codes together are the discriminator 55062/55063 lack:
    ///
    /// - `cav` — `SG10 CCI+++ZB4` / `CAV` DE 7111 „Bezeichnung der
    ///   Summenzeitreihe" ([`crate::zeitreihen::zeitreihe_aus_cav`]).
    /// - `verantwortlicher` — `SG10 CCI+6` DE 7037, the role responsible for
    ///   the series ([`crate::zeitreihen::rolle_aus_cci`]).
    ///
    /// The Verantwortliche is needed because two pairs of families share a CAV
    /// code and differ only in who owns the series: the BK-SZR is `Z97`/`Z99`
    /// whether the NB or the ÜNB aggregates it, and the LF-SZR likewise. Those
    /// pairs answer out of different Entscheidungsbäume, so collapsing them
    /// would send a code from the wrong tree.
    ///
    /// Returns `None` when the pair names no family here — including every
    /// series with its own Anfrage PID (Zuordnungsermächtigung, AAÜZ, LF-AASZR),
    /// which is not activated with 55062/55063 at all.
    #[must_use]
    pub fn from_wire(cav: &str, verantwortlicher: &str) -> Option<Self> {
        use crate::zeitreihen::{Aggregationsebene as E, Familie as F, Kategorie as K, Rolle};
        let (zeitreihe, ebene) = crate::zeitreihen::zeitreihe_aus_cav(cav)?;
        let rolle = crate::zeitreihen::rolle_aus_cci(verantwortlicher)?;
        Some(
            match (zeitreihe.familie(), zeitreihe.kategorie(), ebene, rolle) {
                (F::Nzr, _, _, Rolle::Nb) => {
                    // Both Netzzeitreihe legs are the verantwortlicher NB's, and
                    // the AHB does not distinguish them here — the recipient
                    // does. `from_wire` therefore returns the BIKO leg, and a
                    // caller that knows it is answering a neighbouring NB names
                    // `NetzzeitreiheNachbarNb` explicitly.
                    Self::NetzzeitreiheBiko
                }
                (F::LfSzr, Some(K::A), _, _) => Self::LieferantensummenzeitreiheNb,
                (F::LfSzr, Some(K::B), _, _) => Self::LieferantensummenzeitreiheUenb,
                (F::BgSzr, Some(K::B), _, _) => Self::Bilanzierungsgebietssummenzeitreihe,
                (F::BgSzr, Some(K::C), _, _) => Self::TaeglicheBgSzr,
                (F::BkSzr, Some(K::A), _, _) => Self::BilanzkreissummenzeitreiheNb,
                (F::BkSzr, Some(K::B), Some(E::Bilanzierungsgebiet), _) => {
                    Self::BilanzkreissummenzeitreiheUenb
                }
                (F::BkSzr, Some(K::C), _, _) => Self::TaeglicheBkSzr,
                (F::Dzue, _, _, _) => Self::Deltazeitreihenuebertrag,
                (F::Abrechnungssummenzeitreihe, _, _, _) => Self::Abrechnungssummenzeitreihe,
                _ => return None,
            },
        )
    }
}

/// One row of the Anfrage → Antwort → Weiterleitung table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZpFamilie {
    /// Series and axis this row describes.
    pub serie: ZpSerie,
    /// Whether this row activates or deactivates.
    pub vorgang: ZpVorgang,
    /// Inbound Anfrage Prüfidentifikator (Prozessschritt 1).
    pub anfrage: u32,
    /// Outbound Antwort PID (Prozessschritt 2), when the AHB defines one.
    pub antwort: Option<u32>,
    /// EBD the answering party runs to build that Antwort.
    ///
    /// Always `Some` exactly when [`Self::antwort`] is: an answer PID without a
    /// decision tree would be a code with no Codeliste to read it against.
    pub antwort_ebd: Option<&'static str>,
    /// Outbound Weiterleitung PID (Prozessschritt 4), when the AHB defines one.
    ///
    /// For the series that share 55062/55063 this is the **same code again**,
    /// re-addressed to the downstream party.
    pub weiterleitung: Option<u32>,
}

/// Every Anfrage this workflow accepts, with its answer tree and forwarding PID.
///
/// This table is the single source of truth: the workflow never computes an
/// answer PID or an EBD from the request. BDEW does not number these `+1/+2` —
/// 55062 and 55063 share the Antwort 55064 across eleven series, and each
/// (series, axis, direction) reads it out of a different tree.
pub const ZP_FAMILIEN: &[ZpFamilie] = &[
    // ── Series sharing the generic 55062 / 55063 / 55064 codes ──────────────
    ZpFamilie {
        serie: ZpSerie::NetzzeitreiheNachbarNb,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: Some(55064),
        antwort_ebd: Some("E_0020"),
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::NetzzeitreiheNachbarNb,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: Some(55064),
        antwort_ebd: Some("E_0010"),
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::NetzzeitreiheBiko,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: Some(55064),
        antwort_ebd: Some("E_0024"),
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::NetzzeitreiheBiko,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: Some(55064),
        antwort_ebd: Some("E_0009"),
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::LieferantensummenzeitreiheNb,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::LieferantensummenzeitreiheNb,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::LieferantensummenzeitreiheUenb,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::LieferantensummenzeitreiheUenb,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::Bilanzierungsgebietssummenzeitreihe,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: Some(55064),
        antwort_ebd: Some("E_0015"),
        // Prozessschritt 4: the BIKO re-sends 55062 to the NB.
        weiterleitung: Some(55062),
    },
    ZpFamilie {
        serie: ZpSerie::Bilanzierungsgebietssummenzeitreihe,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: Some(55064),
        antwort_ebd: Some("E_0035"),
        weiterleitung: Some(55063),
    },
    ZpFamilie {
        serie: ZpSerie::BilanzkreissummenzeitreiheNb,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: Some(55064),
        antwort_ebd: Some("E_0034"),
        weiterleitung: Some(55062),
    },
    ZpFamilie {
        serie: ZpSerie::BilanzkreissummenzeitreiheNb,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: Some(55064),
        antwort_ebd: Some("E_0018"),
        weiterleitung: Some(55063),
    },
    ZpFamilie {
        serie: ZpSerie::BilanzkreissummenzeitreiheUenb,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: Some(55064),
        antwort_ebd: Some("E_0011"),
        weiterleitung: Some(55062),
    },
    ZpFamilie {
        serie: ZpSerie::BilanzkreissummenzeitreiheUenb,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: Some(55064),
        antwort_ebd: Some("E_0012"),
        weiterleitung: Some(55063),
    },
    ZpFamilie {
        serie: ZpSerie::Deltazeitreihenuebertrag,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: Some(55064),
        antwort_ebd: Some("E_0027"),
        weiterleitung: Some(55062),
    },
    ZpFamilie {
        serie: ZpSerie::Deltazeitreihenuebertrag,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: Some(55064),
        antwort_ebd: Some("E_0028"),
        weiterleitung: Some(55063),
    },
    ZpFamilie {
        serie: ZpSerie::Abrechnungssummenzeitreihe,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::Abrechnungssummenzeitreihe,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::TaeglicheBgSzr,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::TaeglicheBgSzr,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::TaeglicheBkSzr,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55062,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::TaeglicheBkSzr,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55063,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    // ── Series with their own codes ─────────────────────────────────────────
    ZpFamilie {
        serie: ZpSerie::Zuordnungsermaechtigung,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55071,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::Zuordnungsermaechtigung,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55072,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::TaeglicheAauez,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55197,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::TaeglicheAauez,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55198,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::LfAaszr,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55199,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::LfAaszr,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55200,
        antwort: None,
        antwort_ebd: None,
        weiterleitung: None,
    },
    ZpFamilie {
        serie: ZpSerie::MonatlicheAauezBkvLf,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55203,
        antwort: Some(55204),
        antwort_ebd: Some("E_0071"),
        weiterleitung: Some(55205),
    },
    ZpFamilie {
        serie: ZpSerie::MonatlicheAauezBkvLf,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55206,
        antwort: Some(55207),
        antwort_ebd: Some("E_0072"),
        weiterleitung: Some(55208),
    },
    ZpFamilie {
        serie: ZpSerie::MonatlicheAauezBkvAnfNb,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55209,
        antwort: Some(55210),
        antwort_ebd: Some("E_0078"),
        weiterleitung: Some(55211),
    },
    ZpFamilie {
        serie: ZpSerie::MonatlicheAauezBkvAnfNb,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55212,
        antwort: Some(55213),
        antwort_ebd: Some("E_0079"),
        weiterleitung: Some(55214),
    },
    // ── NZR-EMob: Zuordnung des ZP der NGZ zur NZR (AHB Strom 2.2 Kap. 13.16) ─
    //
    // One Antwort code for both directions — 55237 answers the Zuordnung out of
    // `E_0102` and the Beendigung out of `E_0103`, which is exactly why the
    // Antwort PID and the EBD are separate columns in this table.
    //
    // The Weiterleitung is the **same code re-addressed to the ÜNB**: the AHB
    // gives 55235/55236 two recipients („NB an NB" and „NB an ÜNB") while 55237
    // is „NB an NB" only, and the AWH sequences the ÜNB copy *after* the answer
    // (Lfd 19150 follows 19130). `SendWeiterleitung` requires `Bestaetigt`, so
    // that ordering is the state machine's rather than a convention.
    ZpFamilie {
        serie: ZpSerie::NetzgangzeitreiheNzr,
        vorgang: ZpVorgang::Aktivierung,
        anfrage: 55235,
        antwort: Some(55237),
        antwort_ebd: Some("E_0102"),
        weiterleitung: Some(55235),
    },
    ZpFamilie {
        serie: ZpSerie::NetzgangzeitreiheNzr,
        vorgang: ZpVorgang::Deaktivierung,
        anfrage: 55236,
        antwort: Some(55237),
        antwort_ebd: Some("E_0103"),
        weiterleitung: Some(55236),
    },
];

/// Look up the family for one series and Vorgang.
///
/// This is the only lookup: the Anfrage PID alone does **not** identify a
/// family, because 55062/55063 are shared by eleven series with five different
/// answer obligations between them.
#[must_use]
pub fn familie_for(serie: ZpSerie, vorgang: ZpVorgang) -> Option<&'static ZpFamilie> {
    ZP_FAMILIEN
        .iter()
        .find(|f| f.serie == serie && f.vorgang == vorgang)
}

/// Every series that uses `anfrage` as its Anfrage PID.
///
/// Useful for diagnostics — an inbound 55062 is ambiguous until the caller says
/// which Summenzeitreihe its MaBiS-Zählpunkt belongs to.
#[must_use]
pub fn serien_fuer_pid(anfrage: u32) -> Vec<ZpSerie> {
    ZP_FAMILIEN
        .iter()
        .filter(|f| f.anfrage == anfrage)
        .map(|f| f.serie)
        .collect()
}

/// Every PID this workflow is registered for — Anfragen, Antworten and
/// Weiterleitungen alike.
///
/// The Antwort and Weiterleitung PIDs are registered because mako may sit on
/// either side: as the answering party it *emits* them, and as the requesting
/// party it *receives* them.
#[must_use]
pub fn all_pids() -> Vec<u32> {
    let mut v: Vec<u32> = ZP_FAMILIEN
        .iter()
        .flat_map(|f| [Some(f.anfrage), f.antwort, f.weiterleitung])
        .flatten()
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "mabis-zp-lifecycle";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a lifecycle Anfrage is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZpLifecycleData {
    /// Prüfidentifikator of the inbound Anfrage.
    pub pruefidentifikator: Pruefidentifikator,
    /// Activation or deactivation.
    pub vorgang: ZpVorgang,
    /// Series affected.
    pub serie: ZpSerie,
    /// MaBiS-Zählpunkt the Anfrage refers to.
    pub mabis_zp_id: String,
    /// GLN of the requesting party.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving party.
    pub receiver: MarktpartnerCode,
    /// Billing period the activation takes effect in.
    pub billing_period: BillingPeriod,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// EDIFACT message reference of the Anfrage.
    pub message_ref: MessageRef,
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the MaBiS-ZP lifecycle workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ZpLifecycleEvent {
    /// Inbound Anfrage received and recorded.
    AnfrageErhalten {
        /// Prüfidentifikator of the Anfrage.
        pruefidentifikator: Pruefidentifikator,
        /// Activation or deactivation.
        vorgang: ZpVorgang,
        /// Series affected.
        serie: ZpSerie,
        /// MaBiS-Zählpunkt the Anfrage refers to.
        mabis_zp_id: String,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        receiver: MarktpartnerCode,
        /// Billing period the activation takes effect in.
        billing_period: BillingPeriod,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Anfrage recorded with no Antwort obligation (terminal for that family).
    Erfasst {
        /// Reference of the recorded message.
        message_ref: MessageRef,
    },
    /// Outbound Antwort dispatched.
    AntwortGesendet {
        /// Antwort Prüfidentifikator actually sent.
        antwort_pid: Pruefidentifikator,
        /// EBD the Antwortcode was read against — recorded because 55064 is
        /// answered out of twelve different trees.
        ebd: String,
        /// `true` when the Anfrage was confirmed.
        bestaetigt: bool,
        /// Rejection reason, when `bestaetigt` is `false`.
        grund: Option<String>,
    },
    /// Outbound Weiterleitung dispatched to the downstream BKV.
    WeiterleitungGesendet {
        /// Weiterleitung Prüfidentifikator actually sent.
        weiterleitung_pid: Pruefidentifikator,
        /// GLN of the BKV the Weiterleitung was addressed to.
        empfaenger: MarktpartnerCode,
    },
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Human-readable summary of validation errors.
        reason: String,
    },
}

impl EventPayload for ZpLifecycleEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "MabisZpAnfrageErhalten",
            Self::Erfasst { .. } => "MabisZpErfasst",
            Self::AntwortGesendet { .. } => "MabisZpAntwortGesendet",
            Self::WeiterleitungGesendet { .. } => "MabisZpWeiterleitungGesendet",
            Self::ValidationFailed { .. } => "MabisZpValidationFailed",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a MaBiS-ZP lifecycle process stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum ZpLifecycleState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received; an Antwort is owed.
    AnfrageErhalten(Box<ZpLifecycleData>),
    /// Anfrage recorded; the family defines no Antwort (terminal).
    Erfasst(Box<ZpLifecycleData>),
    /// Antwort sent confirming the Anfrage.
    Bestaetigt(Box<ZpLifecycleData>),
    /// Antwort sent rejecting the Anfrage (terminal).
    Abgelehnt {
        /// Rejection reason.
        grund: String,
    },
    /// Weiterleitung dispatched to the downstream BKV (terminal).
    Weitergeleitet(Box<ZpLifecycleData>),
    /// Inbound message failed AHB validation (terminal).
    ValidationFailed {
        /// Validation error summary.
        reason: String,
    },
}

impl ZpLifecycleState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageErhalten(_) => "AnfrageErhalten",
            Self::Erfasst(_) => "Erfasst",
            Self::Bestaetigt(_) => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Weitergeleitet(_) => "Weitergeleitet",
            Self::ValidationFailed { .. } => "ValidationFailed",
        }
    }

    /// The recorded Anfrage data, when the state carries any.
    #[must_use]
    pub fn data(&self) -> Option<&ZpLifecycleData> {
        match self {
            Self::AnfrageErhalten(d)
            | Self::Erfasst(d)
            | Self::Bestaetigt(d)
            | Self::Weitergeleitet(d) => Some(d),
            Self::New | Self::Abgelehnt { .. } | Self::ValidationFailed { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the MaBiS-ZP lifecycle workflow.
///
/// `Workflow::handle()` is pure — no I/O, no EDIFACT parsing, no store access.
#[derive(Clone)]
pub enum ZpLifecycleCommand {
    /// Inbound Anfrage received from the AS4 layer.
    ReceiveAnfrage {
        /// Prüfidentifikator of the inbound UTILMD.
        ///
        /// Checked against the family, not used to find it: 55062/55063 are
        /// shared by eleven series.
        pid: Pruefidentifikator,
        /// Which Summenzeitreihe — and axis — the MaBiS-Zählpunkt belongs to.
        ///
        /// An explicit input because the PID does not carry it. A MaBiS-ZP is
        /// created **for** one Summenzeitreihe, so the adapter always knows.
        serie: ZpSerie,
        /// Activation or deactivation, from the message content.
        vorgang: ZpVorgang,
        /// MaBiS-Zählpunkt the Anfrage refers to, as it arrived.
        ///
        /// Deliberately a `String` and not
        /// [`MabisZaehlpunktId`](crate::MabisZaehlpunktId): this is a
        /// counterparty's value. Requiring the validated type would make a
        /// malformed Meldepunkt unconstructible, and the workflow could then
        /// neither record what arrived nor answer it with a proper Ablehnung.
        /// The outbound side — [`crate::Summenzeitreihe`] — uses the type,
        /// because that value is ours to get right.
        mabis_zp_id: String,
        /// GLN of the requesting party.
        sender: MarktpartnerCode,
        /// GLN of the receiving party.
        receiver: MarktpartnerCode,
        /// Billing period the activation takes effect in.
        billing_period: BillingPeriod,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
    /// Send the Antwort for a received Anfrage.
    SendAntwort {
        /// `true` to confirm, `false` to reject.
        ///
        /// A cluster, not a code. `SG4 STS+E01` DE 9013 stays unstated until
        /// the twelve Entscheidungsbäume a 55064 is answered out of are
        /// catalogued in `mako_pruefung`: only `E_0010` and `E_0020` have
        /// walks today, and inventing a code for the other ten would put a
        /// fabricated Prüfschritt on a message that settles a
        /// Bilanzkreisabrechnung.
        bestaetigt: bool,
        /// Rejection reason — required when `bestaetigt` is `false`.
        grund: Option<String>,
    },
    /// Forward the confirmed activation to the downstream BKV.
    SendWeiterleitung {
        /// GLN of the BKV to forward to.
        empfaenger: MarktpartnerCode,
    },
}

impl CommandPayload for ZpLifecycleCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// MaBiS-ZP lifecycle workflow.
///
/// Handles activation and deactivation of the MaBiS-Zählpunkt, the
/// Zuordnungsermächtigung, and the AAÜZ/LF-AASZR series. See the module
/// documentation for the PID table and the state machine.
pub struct MabisZpLifecycleWorkflow;

impl Workflow for MabisZpLifecycleWorkflow {
    type State = ZpLifecycleState;
    type Event = ZpLifecycleEvent;
    type Command = ZpLifecycleCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ZpLifecycleEvent::AnfrageErhalten {
                pruefidentifikator,
                vorgang,
                serie,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
            } => ZpLifecycleState::AnfrageErhalten(Box::new(ZpLifecycleData {
                pruefidentifikator: *pruefidentifikator,
                vorgang: *vorgang,
                serie: *serie,
                mabis_zp_id: mabis_zp_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                billing_period: billing_period.clone(),
                document_date: document_date.clone(),
                message_ref: message_ref.clone(),
            })),

            ZpLifecycleEvent::Erfasst { .. } => match state {
                ZpLifecycleState::AnfrageErhalten(d) => ZpLifecycleState::Erfasst(d),
                other => other,
            },

            ZpLifecycleEvent::AntwortGesendet {
                bestaetigt, grund, ..
            } => match state {
                ZpLifecycleState::AnfrageErhalten(d) => {
                    if *bestaetigt {
                        ZpLifecycleState::Bestaetigt(d)
                    } else {
                        ZpLifecycleState::Abgelehnt {
                            grund: grund.clone().unwrap_or_default(),
                        }
                    }
                }
                other => other,
            },

            ZpLifecycleEvent::WeiterleitungGesendet { .. } => match state {
                ZpLifecycleState::Bestaetigt(d) => ZpLifecycleState::Weitergeleitet(d),
                other => other,
            },

            ZpLifecycleEvent::ValidationFailed { reason } => ZpLifecycleState::ValidationFailed {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                serie,
                vorgang,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ZpLifecycleState::New) {
                    // Idempotent: a redelivered Anfrage is a no-op.
                    return Ok(vec![].into());
                }

                let Some(familie) = familie_for(serie, vorgang) else {
                    return Err(WorkflowError::rejected(format!(
                        "{} kennt keinen Vorgang {vorgang:?}",
                        serie.label()
                    )));
                };

                // 55062/55063 are shared by eleven series, so the PID cannot
                // identify the family — but it can still contradict it, and a
                // 55197 filed against the Netzzeitreihe is a routing error, not
                // a variant.
                if familie.anfrage != pid.as_u32() {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} passt nicht zu {} / {vorgang:?} — erwartet {}",
                        serie.label(),
                        familie.anfrage
                    )));
                }

                // A series a Festlegung has repealed cannot be activated for a
                // Bilanzierungsmonat that starts after it ends: BK6-23-241
                // Tenorziffer 5 repeals the tägliche AAÜZ with the end of
                // 30.09.2026, and a MaBiS-ZP activated into October contributes
                // to a Summenzeitreihe that no longer exists — the Abrechnung
                // never arrives and nothing else says why.
                if vorgang == ZpVorgang::Aktivierung
                    && let Some(beginn) = abrechnungszeitraum_beginn(billing_period.as_str())
                    && !serie.gilt_am(beginn)
                {
                    return Err(WorkflowError::rejected(format!(
                        "{} endet am {} und kann für den Abrechnungszeitraum {} \
                         nicht mehr aktiviert werden",
                        serie.label(),
                        serie.endet_am().expect("gilt_am was false, so there is an end date"),
                        billing_period.as_str()
                    )));
                }

                if !validation_passed {
                    return Ok(vec![ZpLifecycleEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }

                let erhalten = ZpLifecycleEvent::AnfrageErhalten {
                    pruefidentifikator: pid,
                    vorgang: familie.vorgang,
                    serie: familie.serie,
                    mabis_zp_id,
                    sender,
                    receiver,
                    billing_period,
                    document_date,
                    message_ref: message_ref.clone(),
                };

                // A family with no Antwort PID is terminal on arrival. Leaving
                // it in `AnfrageErhalten` would model an obligation the AHB
                // does not define.
                if familie.antwort.is_none() {
                    return Ok(vec![erhalten, ZpLifecycleEvent::Erfasst { message_ref }].into());
                }

                Ok(vec![erhalten].into())
            }

            ZpLifecycleCommand::SendAntwort { bestaetigt, grund } => {
                let ZpLifecycleState::AnfrageErhalten(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendAntwort requires state AnfrageErhalten, got {}",
                        state.label()
                    )));
                };

                let familie = familie_for(data.serie, data.vorgang).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "keine Familie für {} / {:?}",
                        data.serie.label(),
                        data.vorgang
                    ))
                })?;

                let (Some(antwort_pid_code), Some(ebd)) = (familie.antwort, familie.antwort_ebd)
                else {
                    return Err(WorkflowError::rejected(format!(
                        "{} (Anfrage {}) definiert keine Antwort",
                        familie.serie.label(),
                        familie.anfrage
                    )));
                };

                if !bestaetigt && grund.as_ref().is_none_or(|g| g.trim().is_empty()) {
                    return Err(WorkflowError::rejected(
                        "a rejecting Antwort requires a reason".to_owned(),
                    ));
                }

                let antwort_pid = Pruefidentifikator::new(antwort_pid_code).map_err(|e| {
                    WorkflowError::rejected(format!("invalid Antwort PID {antwort_pid_code}: {e}"))
                })?;

                // The keys are the UTILMD renderer's. A MaBiS Vorgang names a
                // **MaBiS-Zählpunkt** and no Marktlokation, so the ZP is the
                // primary `SG5 LOC+Z15`; the answer travels back the way the
                // Anfrage came, so the parties swap.
                let mut payload = serde_json::json!({
                    "pid": antwort_pid_code,
                    "sender": data.receiver.as_str(),
                    "receiver": data.sender.as_str(),
                    "mabis_zaehlpunkt": data.mabis_zp_id,
                    // `SG4 STS+E01` DE 1131 — the tree this answer belongs to.
                    // 55064 is answered out of twelve of them, so the Antwort
                    // is unreadable without it. DE 9013 is not stated: see
                    // `SendAntwort`.
                    "antwort_codeliste": ebd,
                });
                // `SG4 DTM+158` on an answer to an Aktivierung, `DTM+159` on
                // one to a Deaktivierung (UTILMD AHB Strom 2.2 Kap. 13.3,
                // Bedingungen `[30]`/`[34]`). The lifecycle has no
                // Vertragsdatum: a Zählpunkt has no contract.
                let datum_key = match data.vorgang {
                    ZpVorgang::Aktivierung => "bilanzierungsbeginn",
                    ZpVorgang::Deaktivierung => "bilanzierungsende",
                };
                payload[datum_key] = serde_json::Value::String(data.document_date.clone());
                if let Some(ref text) = grund {
                    payload["bemerkung"] = serde_json::Value::String(text.clone());
                }
                let outbox = PendingOutbox::new("UTILMD", data.sender.as_str(), payload);

                Ok(WorkflowOutput {
                    events: vec![ZpLifecycleEvent::AntwortGesendet {
                        antwort_pid,
                        ebd: ebd.to_owned(),
                        bestaetigt,
                        grund,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }

            ZpLifecycleCommand::SendWeiterleitung { empfaenger } => {
                let ZpLifecycleState::Bestaetigt(data) = state else {
                    return Err(WorkflowError::rejected(format!(
                        "SendWeiterleitung requires state Bestaetigt, got {}",
                        state.label()
                    )));
                };

                let familie = familie_for(data.serie, data.vorgang).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "keine Familie für {} / {:?}",
                        data.serie.label(),
                        data.vorgang
                    ))
                })?;

                let Some(weiterleitung) = familie.weiterleitung else {
                    return Err(WorkflowError::rejected(format!(
                        "{} (Anfrage {}) definiert keine Weiterleitung",
                        familie.serie.label(),
                        familie.anfrage
                    )));
                };

                let weiterleitung_pid = Pruefidentifikator::new(weiterleitung).map_err(|e| {
                    WorkflowError::rejected(format!(
                        "invalid Weiterleitung PID {weiterleitung}: {e}"
                    ))
                })?;

                let outbox = PendingOutbox::new(
                    "UTILMD",
                    empfaenger.as_str(),
                    serde_json::json!({
                        "pid": weiterleitung,
                        "sender": data.receiver.as_str(),
                        "receiver": empfaenger.as_str(),
                        "mabis_zaehlpunkt": data.mabis_zp_id,
                        "bilanzierungsbeginn": data.document_date,
                    }),
                );

                Ok(WorkflowOutput {
                    events: vec![ZpLifecycleEvent::WeiterleitungGesendet {
                        weiterleitung_pid,
                        empfaenger,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![],
                })
            }
        }
    }
}


/// First day of the month a [`BillingPeriod`] names, where it names one.
///
/// The value is a counterparty's and its shape is AHB-dependent — `YYYYMM` or
/// `YYYYMMDD-YYYYMMDD` — so only the leading `YYYYMM` is read, and anything else
/// answers `None`. A period that cannot be read is not evidence of a period out
/// of range.
fn abrechnungszeitraum_beginn(period: &str) -> Option<time::Date> {
    // `YYYYMM`, `YYYY-MM` and `YYYYMMDD-YYYYMMDD` all appear across AHB
    // versions, so the separator is ignored and the leading six digits are read.
    let digits: String = period.chars().filter(char::is_ascii_digit).take(6).collect();
    if digits.len() != 6 {
        return None;
    }
    let year: i32 = digits[..4].parse().ok()?;
    let month = time::Month::try_from(digits[4..6].parse::<u8>().ok()?).ok()?;
    time::Date::from_calendar_date(year, month, 1).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn receive(serie: ZpSerie, vorgang: ZpVorgang) -> ZpLifecycleCommand {
        let pid = familie_for(serie, vorgang).expect("in the table").anfrage;
        receive_with_pid(serie, vorgang, pid)
    }

    fn receive_with_pid(serie: ZpSerie, vorgang: ZpVorgang, pid: u32) -> ZpLifecycleCommand {
        ZpLifecycleCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            serie,
            vorgang,
            mabis_zp_id: "DE0001112223334445556667778889990".to_owned(),
            sender: mp("9900123456789"),
            receiver: mp("9900987654321"),
            billing_period: BillingPeriod::new("2026-07"),
            document_date: "20260701".to_owned(),
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }


    fn receive_for_period(
        serie: ZpSerie,
        vorgang: ZpVorgang,
        period: &str,
    ) -> ZpLifecycleCommand {
        let mut cmd = receive(serie, vorgang);
        if let ZpLifecycleCommand::ReceiveAnfrage {
            ref mut billing_period,
            ..
        } = cmd
        {
            *billing_period = BillingPeriod::new(period);
        }
        cmd
    }

    fn fold(events: &[ZpLifecycleEvent]) -> ZpLifecycleState {
        events.iter().fold(ZpLifecycleState::default(), |s, e| {
            MabisZpLifecycleWorkflow::apply(s, e)
        })
    }

    // ── Table integrity ─────────────────────────────────────────────────────

    #[test]
    fn every_series_has_exactly_one_row_per_vorgang() {
        for f in ZP_FAMILIEN {
            for vorgang in [ZpVorgang::Aktivierung, ZpVorgang::Deaktivierung] {
                let rows = ZP_FAMILIEN
                    .iter()
                    .filter(|r| r.serie == f.serie && r.vorgang == vorgang)
                    .count();
                assert_eq!(rows, 1, "{} / {vorgang:?}", f.serie.label());
            }
        }
    }

    #[test]
    fn an_antwort_pid_always_comes_with_its_tree() {
        // An answer code without a Codeliste to read it against is not an
        // answer — 55064 alone says nothing.
        for f in ZP_FAMILIEN {
            assert_eq!(
                f.antwort.is_some(),
                f.antwort_ebd.is_some(),
                "{} / {:?}",
                f.serie.label(),
                f.vorgang
            );
        }
    }

    #[test]
    fn the_generic_codes_are_shared_by_eleven_series() {
        // This is the fact the whole module is shaped around: 55062/55063 do
        // not identify a process.
        let akt = serien_fuer_pid(55062);
        let deakt = serien_fuer_pid(55063);
        assert_eq!(akt.len(), 11, "55062 is shared: {akt:?}");
        assert_eq!(deakt.len(), 11, "55063 is shared: {deakt:?}");
    }

    #[test]
    fn the_shared_antwort_pid_reads_out_of_twelve_different_trees() {
        let mut ebds: Vec<&str> = ZP_FAMILIEN
            .iter()
            .filter(|f| f.antwort == Some(55064))
            .map(|f| f.antwort_ebd.expect("paired"))
            .collect();
        let total = ebds.len();
        ebds.sort_unstable();
        ebds.dedup();
        assert_eq!(
            total, 12,
            "twelve (series, direction) pairs answer with 55064"
        );
        assert_eq!(ebds.len(), 12, "and no two of them share a tree: {ebds:?}");
    }

    #[test]
    fn six_of_the_eleven_generic_series_answer_and_five_do_not() {
        let generic = |with_antwort: bool| {
            ZP_FAMILIEN
                .iter()
                .filter(|f| f.anfrage == 55062 && f.antwort.is_some() == with_antwort)
                .count()
        };
        assert_eq!(
            generic(true),
            6,
            "an implementation that never answers 55062 drops six obligations"
        );
        assert_eq!(
            generic(false),
            5,
            "modelling 55062 → 55064 invents five obligations"
        );
    }

    #[test]
    fn the_generic_weiterleitung_re_uses_the_request_code() {
        // Prozessschritt 4 is another 55062/55063 to the downstream party, not
        // a distinct PID.
        for f in ZP_FAMILIEN.iter().filter(|f| f.anfrage == 55062) {
            if let Some(w) = f.weiterleitung {
                assert_eq!(w, 55062, "{}", f.serie.label());
            }
        }
    }

    #[test]
    fn all_pids_covers_anfragen_answers_and_weiterleitungen() {
        let pids = all_pids();
        for f in ZP_FAMILIEN {
            assert!(pids.contains(&f.anfrage));
            for p in [f.antwort, f.weiterleitung].into_iter().flatten() {
                assert!(pids.contains(&p), "{p} missing from all_pids()");
            }
        }
        let expected: Vec<u32> = vec![
            55062, 55063, 55064, 55071, 55072, 55197, 55198, 55199, 55200, 55203, 55204, 55205,
            55206, 55207, 55208, 55209, 55210, 55211, 55212, 55213, 55214,
            // NZR-EMob Zuordnung des ZP der NGZ zur NZR (AHB Kap. 13.16).
            55235, 55236, 55237,
        ];
        assert_eq!(pids, expected);
    }

    /// The NZR-EMob Zuordnung des ZP der NGZ zur NZR — its own Anfrage codes,
    /// one shared Antwort code, two different trees.
    #[test]
    fn the_ngz_zuordnung_answers_one_pid_out_of_two_trees() {
        let auf = familie_for(ZpSerie::NetzgangzeitreiheNzr, ZpVorgang::Aktivierung)
            .expect("Zuordnung is a family");
        let ab = familie_for(ZpSerie::NetzgangzeitreiheNzr, ZpVorgang::Deaktivierung)
            .expect("Beendigung is a family");

        assert_eq!((auf.anfrage, ab.anfrage), (55235, 55236));

        // One Antwort code for both directions. Reading the tree off the
        // Antwort PID would therefore be impossible — which is exactly why the
        // EBD is its own column and the workflow never derives it.
        assert_eq!(auf.antwort, ab.antwort, "55237 answers both");
        assert_eq!(auf.antwort, Some(55237));
        assert_eq!(auf.antwort_ebd, Some("E_0102"));
        assert_eq!(ab.antwort_ebd, Some("E_0103"));
        assert_ne!(auf.antwort_ebd, ab.antwort_ebd);

        // The ÜNB copy is the same code re-addressed, sent only once the
        // neighbouring NB has confirmed (AHB Kap. 13.16 gives 55235/55236 two
        // recipients and 55237 one).
        assert_eq!(auf.weiterleitung, Some(55235));
        assert_eq!(ab.weiterleitung, Some(55236));

        // It has its own Anfrage codes, so it is never resolved off the
        // 55062/55063 SG10 discriminator.
        assert!(!serien_fuer_pid(55062).contains(&ZpSerie::NetzgangzeitreiheNzr));
        assert_eq!(serien_fuer_pid(55235), vec![ZpSerie::NetzgangzeitreiheNzr]);

        // MaBiS, not Modell 2 — it outlives no Festlegung end date.
        assert_eq!(ZpSerie::NetzgangzeitreiheNzr.endet_am(), None);
    }

    #[test]
    fn the_two_monatliche_families_forward_to_different_recipients() {
        // Identical process, different Weiterleitung code and a different EBD —
        // the only things separating them, and the reason they are not merged.
        let lf = familie_for(ZpSerie::MonatlicheAauezBkvLf, ZpVorgang::Aktivierung).unwrap();
        let nb = familie_for(ZpSerie::MonatlicheAauezBkvAnfNb, ZpVorgang::Aktivierung).unwrap();
        assert_eq!(lf.weiterleitung, Some(55205));
        assert_eq!(nb.weiterleitung, Some(55211));
        assert_eq!(lf.antwort_ebd, Some("E_0071"));
        assert_eq!(nb.antwort_ebd, Some("E_0078"));
    }

    // ── The 30.09.2026 cut ──────────────────────────────────────────────────

    #[test]
    fn only_the_taegliche_aauez_expires() {
        for f in ZP_FAMILIEN {
            let expected = f.serie == ZpSerie::TaeglicheAauez;
            assert_eq!(
                f.serie.endet_am().is_some(),
                expected,
                "{}",
                f.serie.label()
            );
        }
        let ende = TAEGLICHE_AAUEZ_ENDE;
        assert!(ZpSerie::TaeglicheAauez.gilt_am(ende));
        assert!(!ZpSerie::TaeglicheAauez.gilt_am(ende.next_day().unwrap()));
        // Everything else is unaffected by the Kap.-17 repeal.
        assert!(ZpSerie::LfAaszr.gilt_am(ende.next_day().unwrap()));
    }

    // ── Behaviour ───────────────────────────────────────────────────────────

    #[test]
    fn a_family_without_an_antwort_is_terminal_on_arrival() {
        let out = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive(ZpSerie::Zuordnungsermaechtigung, ZpVorgang::Aktivierung),
        )
        .expect("accepted");
        let state = fold(&out.events);
        assert_eq!(state.label(), "Erfasst");
        assert!(out.outbox.is_empty(), "record-only family must not emit");

        let err = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: true,
                grund: None,
            },
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("Antwort"), "got: {err}");
    }

    #[test]
    fn the_same_pid_answers_or_does_not_depending_on_the_series() {
        // Both arrive as 55062. One owes a 55064, the other is terminal — the
        // single fact a PID-keyed table cannot represent.
        let owes = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive(
                ZpSerie::Bilanzierungsgebietssummenzeitreihe,
                ZpVorgang::Aktivierung,
            ),
        )
        .expect("accepted");
        assert_eq!(fold(&owes.events).label(), "AnfrageErhalten");

        let terminal = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive(ZpSerie::TaeglicheBkSzr, ZpVorgang::Aktivierung),
        )
        .expect("accepted");
        assert_eq!(fold(&terminal.events).label(), "Erfasst");
    }

    #[test]
    fn the_antwort_carries_the_tree_it_was_read_against() {
        let out = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive(ZpSerie::Deltazeitreihenuebertrag, ZpVorgang::Deaktivierung),
        )
        .expect("accepted");
        let state = fold(&out.events);
        let antwort = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: true,
                grund: None,
            },
        )
        .expect("answered");
        assert_eq!(antwort.outbox[0].payload["pid"], 55064);
        assert_eq!(antwort.outbox[0].payload["antwort_codeliste"], "E_0028");
    }

    #[test]
    fn anfrage_antwort_weiterleitung_happy_path() {
        let out = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive(ZpSerie::MonatlicheAauezBkvLf, ZpVorgang::Aktivierung),
        )
        .expect("accepted");
        let state = fold(&out.events);
        assert_eq!(state.label(), "AnfrageErhalten");

        let antwort = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: true,
                grund: None,
            },
        )
        .expect("pruefung");
        assert_eq!(antwort.outbox.len(), 1);
        assert_eq!(antwort.outbox[0].payload["pid"], 55204);
        assert_eq!(antwort.outbox[0].payload["antwort_codeliste"], "E_0071");
        assert_eq!(
            antwort.outbox[0].recipient.as_ref(),
            "9900123456789",
            "the Antwort goes back to the requesting party"
        );

        let state = antwort
            .events
            .iter()
            .fold(state, MabisZpLifecycleWorkflow::apply);
        assert_eq!(state.label(), "Bestaetigt");

        let out = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendWeiterleitung {
                empfaenger: mp("9900555555555"),
            },
        )
        .expect("weiterleitung");
        assert_eq!(out.outbox[0].payload["pid"], 55205);
    }

    #[test]
    fn a_rejecting_antwort_requires_a_reason() {
        let out = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung),
        )
        .expect("accepted");
        let state = fold(&out.events);
        let err = MabisZpLifecycleWorkflow::handle(
            &state,
            ZpLifecycleCommand::SendAntwort {
                bestaetigt: false,
                grund: None,
            },
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("reason"), "got: {err}");
    }

    #[test]
    fn a_pid_that_contradicts_the_series_is_rejected() {
        // 55197 is the tägliche AAÜZ; filing it against the Netzzeitreihe is a
        // routing error, not a variant.
        let err = MabisZpLifecycleWorkflow::handle(
            &ZpLifecycleState::New,
            receive_with_pid(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung, 55197),
        )
        .expect_err("must reject");
        assert!(format!("{err}").contains("55062"), "got: {err}");
    }

    #[test]
    fn validation_failure_is_terminal_and_emits_nothing() {
        let cmd = match receive(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung) {
            ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                serie,
                vorgang,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                ..
            } => ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                serie,
                vorgang,
                mabis_zp_id,
                sender,
                receiver,
                billing_period,
                document_date,
                message_ref,
                validation_passed: false,
                validation_errors: vec!["SG6 LOC missing".to_owned()],
            },
            other => other,
        };
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd).expect("accepted");
        assert!(out.outbox.is_empty());
        assert_eq!(fold(&out.events).label(), "ValidationFailed");
    }

    #[test]
    fn a_redelivered_anfrage_is_a_no_op() {
        let cmd = receive(ZpSerie::NetzzeitreiheBiko, ZpVorgang::Aktivierung);
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd.clone())
            .expect("accepted");
        let state = fold(&out.events);
        let again = MabisZpLifecycleWorkflow::handle(&state, cmd).expect("idempotent");
        assert!(again.events.is_empty());
        assert!(again.outbox.is_empty());
    }

    /// BK6-23-241 Tenorziffer 5 repeals MaBiS Anlage 1 Kap. 17.2 with the end of
    /// 30.09.2026, so a MaBiS-ZP cannot be activated for a Bilanzierungsmonat
    /// that starts after it. Accepting one books a Zählpunkt into a
    /// Summenzeitreihe that never settles, and nothing downstream says why.
    #[test]
    fn a_repealed_series_cannot_be_activated_after_its_end() {
        let cmd = receive_for_period(
            ZpSerie::TaeglicheAauez,
            ZpVorgang::Aktivierung,
            "202610",
        );
        let out = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd);
        let err = out.expect_err("an activation past the repeal is refused");
        assert!(
            format!("{err}").contains("endet am 2026-09-30"),
            "the refusal names the date: {err}"
        );
    }

    /// The last month the series exists still activates.
    #[test]
    fn the_final_month_of_a_repealed_series_still_activates() {
        let cmd = receive_for_period(
            ZpSerie::TaeglicheAauez,
            ZpVorgang::Aktivierung,
            "202609",
        );
        assert!(
            MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd).is_ok()
        );
    }

    /// A Deaktivierung is how a repealed series is wound down, so the guard
    /// must not refuse one.
    #[test]
    fn a_deaktivierung_is_not_bound_by_the_end_date() {
        let cmd = receive_for_period(
            ZpSerie::TaeglicheAauez,
            ZpVorgang::Deaktivierung,
            "202610",
        );
        assert!(
            MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd).is_ok()
        );
    }

    /// Every other series is open-ended and unaffected.
    #[test]
    fn a_series_with_no_end_date_activates_in_any_period() {
        let cmd = receive_for_period(
            ZpSerie::TaeglicheBkSzr,
            ZpVorgang::Aktivierung,
            "209912",
        );
        assert!(
            MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, cmd).is_ok()
        );
    }

    /// A period whose shape the AHB version changed is not evidence of a period
    /// out of range, so the guard stands down rather than inventing a refusal.
    #[test]
    fn an_unreadable_abrechnungszeitraum_does_not_refuse() {
        let okt = time::Date::from_calendar_date(2026, time::Month::October, 1).unwrap();
        for shape in ["202610", "2026-10", "20261001-20261031"] {
            assert_eq!(super::abrechnungszeitraum_beginn(shape), Some(okt), "{shape}");
        }
        for bad in ["2026", "", "202613"] {
            assert_eq!(super::abrechnungszeitraum_beginn(bad), None, "{bad:?}");
        }
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::zeitreihen::{
        Aggregationsebene, Familie, Kategorie, Rolle, Zeitreihe, cav_aus_zeitreihe, cci_aus_rolle,
    };

    #[test]
    fn the_wire_codes_resolve_the_series_a_shared_pid_cannot() {
        /// One row: the Tabelle-1 identity, its Aggregationsebene where it has
        /// one, the responsible role, and the family it must resolve to.
        type Fall = (
            Familie,
            Option<Kategorie>,
            Option<Aggregationsebene>,
            Rolle,
            ZpSerie,
        );
        let cases: &[Fall] = &[
            (
                Familie::BgSzr,
                Some(Kategorie::B),
                None,
                Rolle::Uenb,
                ZpSerie::Bilanzierungsgebietssummenzeitreihe,
            ),
            (
                Familie::BgSzr,
                Some(Kategorie::C),
                None,
                Rolle::Uenb,
                ZpSerie::TaeglicheBgSzr,
            ),
            (
                Familie::BkSzr,
                Some(Kategorie::A),
                None,
                Rolle::Nb,
                ZpSerie::BilanzkreissummenzeitreiheNb,
            ),
            (
                Familie::BkSzr,
                Some(Kategorie::B),
                Some(Aggregationsebene::Bilanzierungsgebiet),
                Rolle::Uenb,
                ZpSerie::BilanzkreissummenzeitreiheUenb,
            ),
            (
                Familie::BkSzr,
                Some(Kategorie::C),
                None,
                Rolle::Uenb,
                ZpSerie::TaeglicheBkSzr,
            ),
            (
                Familie::LfSzr,
                Some(Kategorie::A),
                None,
                Rolle::Nb,
                ZpSerie::LieferantensummenzeitreiheNb,
            ),
            (
                Familie::LfSzr,
                Some(Kategorie::B),
                Some(Aggregationsebene::Bilanzierungsgebiet),
                Rolle::Uenb,
                ZpSerie::LieferantensummenzeitreiheUenb,
            ),
            (
                Familie::Dzue,
                None,
                None,
                Rolle::Uenb,
                ZpSerie::Deltazeitreihenuebertrag,
            ),
            (
                Familie::Nzr,
                None,
                None,
                Rolle::Nb,
                ZpSerie::NetzzeitreiheBiko,
            ),
            (
                Familie::Abrechnungssummenzeitreihe,
                None,
                None,
                Rolle::Biko,
                ZpSerie::Abrechnungssummenzeitreihe,
            ),
        ];
        for &(familie, kategorie, ebene, rolle, expected) in cases {
            let z = Zeitreihe::new(familie, kategorie).expect("Tabelle-1 row");
            let cav = cav_aus_zeitreihe(z, ebene).expect("has a CAV code");
            let cci = cci_aus_rolle(rolle).expect("has a CCI code");
            assert_eq!(
                ZpSerie::from_wire(cav, cci),
                Some(expected),
                "CAV {cav} / CCI {cci}"
            );
        }
    }

    #[test]
    fn every_resolved_series_has_a_family_row() {
        for cav in [
            "Z95", "Z96", "Z97", "Z99", "ZA0", "ZA1", "ZA3", "ZA4", "ZA5", "ZA6",
        ] {
            for cci in ["ZA8", "ZA9", "ZB7"] {
                if let Some(serie) = ZpSerie::from_wire(cav, cci) {
                    for vorgang in [ZpVorgang::Aktivierung, ZpVorgang::Deaktivierung] {
                        assert!(
                            familie_for(serie, vorgang).is_some(),
                            "{cav}/{cci} → {serie:?} / {vorgang:?} has no family row"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_unknown_code_resolves_to_nothing_rather_than_a_neighbour() {
        assert_eq!(
            ZpSerie::from_wire("ZG7", "ZA9"),
            None,
            "eMob is not MaBiS Tabelle 1"
        );
        assert_eq!(ZpSerie::from_wire("ZZZ", "ZA9"), None);
        assert_eq!(ZpSerie::from_wire("Z95", "ZZZ"), None);
    }

    #[test]
    fn the_series_with_their_own_pids_are_not_reachable_from_the_generic_codes() {
        // The Zuordnungsermächtigung, the AAÜZ families and the LF-AASZR are
        // activated with 55071/55072 and 55197–55214, not with 55062/55063, so
        // no CAV code names them.
        let unreachable = [
            ZpSerie::Zuordnungsermaechtigung,
            ZpSerie::TaeglicheAauez,
            ZpSerie::LfAaszr,
            ZpSerie::MonatlicheAauezBkvLf,
            ZpSerie::MonatlicheAauezBkvAnfNb,
        ];
        for cav in [
            "Z95", "Z96", "Z97", "Z98", "Z99", "ZA0", "ZA1", "ZA2", "ZA3", "ZA4", "ZA5", "ZA6",
        ] {
            for cci in ["ZA8", "ZA9", "ZB7"] {
                if let Some(s) = ZpSerie::from_wire(cav, cci) {
                    assert!(!unreachable.contains(&s), "{cav}/{cci} → {s:?}");
                }
            }
        }
    }
}
