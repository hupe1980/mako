//! `mako-redispatch` — Redispatch 2.0 process engine for German grid
//! congestion management (§§ 13, 13a, 14 `EnWG`).
//!
//! # Three-crate architecture for Redispatch 2.0
//!
//! | Crate | Responsibility |
//! |---|---|
//! | `edi-energy` | IFTSTA status messages (EDIFACT, PIDs 21037/21038) |
//! | `redispatch-xml` | XML/XSD format parsing (`ActivationDocument`, `Stammdaten`, …) |
//! | `mako-redispatch` ← **this crate** | Process engine — workflows, routing, deadlines |
//!
//! # Domain background
//!
//! **Redispatch 2.0** entered into force on **1 October 2021** via the
//! Netzausbaubeschleunigungsgesetz (NABEG). It requires all German TSOs
//! (ÜNB) and DSOs (VNB) to coordinate congestion management across
//! transmission and distribution networks using CIM/IEC 62325 XML documents.
//!
//! Unlike GPKE/WiM/GeLi Gas (EDIFACT `RFF+Z13` Prüfidentifikatoren), routing
//! here is document-type-driven via [`RedispatchRouter`].
//!
//! # Regulatory basis
//!
//! **BK6-23-241 (Beschluss 07.05.2026) consolidated Redispatch 2.0.** Its
//! Anlage „Bilanzieller Ausgleich von Redispatch-Maßnahmen (`BilAReM`)" replaces
//! the three decisions this crate used to cite:
//!
//! | Repealed | By | With effect from |
//! |---|---|---|
//! | BK6-20-059 Tenorziffer 1 | Tenorziffer 1 | end of 30.06.2026 |
//! | BK6-20-060 (Netzbetreiberkoordinierung) | Tenorziffer 4 | 07.05.2026 |
//! | BK6-20-061 (Informationsbereitstellung) | Tenorziffer 3 | 07.05.2026 |
//! | BK6-20-059 Tenorziffer 2 · Anlage zur `BilAReM` | Tenorziffer 8 | first day the new EDI@Energy documents apply |
//! | `MaBiS` Anlage 1 Kap. 17 | Tenorziffer 5 | end of 30.09.2026 |
//!
//! What did **not** arrive with it is a new table of Fristen. Tenorziffer 7
//! obliges the ÜNB to develop bundesweit einheitliche Prozessbeschreibungen
//! together with the industry and submit them to the Beschlusskammer, which
//! then publishes them. Until that happens, most of the concrete windows are
//! the operator's own.
//!
//! # Deadlines
//!
//! [`fristen`] splits them by whether a published source still carries the
//! value, because the widely quoted figures no longer all have one:
//!
//! | Obligation | Value | Source |
//! |---|---|---|
//! | `AcknowledgementDocument` | **3 minutes**, unverzüglich | `AcknowledgementDocument` FB 1.0g |
//! | Vorab-Information, Prognosemodell | 30 minutes before validity | `BilAReM` Kap. 6.3.1 |
//! | Ausfallarbeit final or Dissens established | end of the **3rd** following month, no restart after | `BilAReM` Kap. 6.4.3 |
//! | Wetterdaten of the Anlagenbetreiber | 4th Werktag of the following month | `BilAReM` Kap. 3.2.1 |
//! | `Stammdaten` `gueltig_ab` | ≥ 5 or ≥ 10 Werktage ahead, ≤ 2 years | `Stammdaten` AWT 1.4b Fn. 27/31/32/33 |
//! | Überführung ins Planwertmodell | ≥ 6 months' notice, only on 01.01./04./07./10. | `BilAReM` Kap. 2.3.2 |
//! | Activation (ACO) response | **operator-configured** | — (was BK6-20-060) |
//! | `Kostenblatt` submission | **operator-configured** | — (was BK6-20-061) |
//! | `Stammdaten` forward (VNB→ÜNB) | **operator-configured** | — (was BK6-20-060) |
//!
//! > **The acknowledgement is three minutes, not six hours.** The 6-hour figure
//! > this crate carried had no published source; the `AcknowledgementDocument`
//! > Formatbeschreibung states „unverzüglich, jedoch spätestens 3 Minuten nach
//! > Erhalt der Übertragungsdatei". The difference is architectural: six hours
//! > is a batch job, three minutes has to be answered by the ingest path.
//!
//! > **`StatusRequest_MarketDocument` is not a request/response pair.** Its
//! > `type` codes are `A60` (status request for a position independently from a
//! > specific process) and `Z15` Erreichbarkeitsinformation, and its `status`
//! > carries `A03` Deactivated / `A04` Reactivated / `A13` Withdrawn. There is
//! > no 24-hour answer window and no answer document.
//!
//! # Deployment role gate
//!
//! `RedispatchModule` should only be registered when `DeploymentRoles` contains
//! at least one of `Marktrolle::Nb`, `Marktrolle::Unb`, or `Marktrolle::Anb`.
//! Lieferant (LF) and MSB deployments are out of scope for Redispatch 2.0.
//!
//! # IFTSTA PIDs (confirmed from IFTSTA AHB 2.1 + PID 4.0)
//!
//! | PID   | Perspective | Process |
//! |-------|-------------|---------|
//! | 21037 | NB (VNB)    | Kommunikationsprozesse Redispatch — Ansicht NB |
//! | 21038 | BTR         | Kommunikationsprozesse Redispatch — Ansicht BTR |
//!
//! These PIDs are registered into the `PidRouter` by [`RedispatchModule`] and
//! route to the [`aktivierung`] workflow via conversation-ID lookup.
//!
//! # Module overview
//!
//! | Module | Workflow name | Document type |
//! |---|---|---|
//! | [`stammdaten`] | `redispatch-stammdaten` | `Stammdaten` |
//! | [`aktivierung`] | `redispatch-aktivierung` | `ActivationDocument` |
//! | [`ack_forward`] (Verfügbarkeit) | `redispatch-verfuegbarkeit` | `UnavailabilityMarketDocument` |
//! | [`ack_forward`] (Netzengpass) | `redispatch-netzengpass` | `NetworkConstraintDocument` |
//! | [`ack_forward`] (`Kaskade`) | `redispatch-kaskade` | `Kaskade` |
//! | [`ack_forward`] (Planungsdaten) | `redispatch-planungsdaten` | `PlannedResourceScheduleDocument` |
//! | [`ack_forward`] (Statusanfrage) | `redispatch-statusanfrage` | `StatusRequest_MarketDocument` |
//! | [`ack_forward`] (`Kostenblatt`) | `redispatch-kostenblatt` | `Kostenblatt` |

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]

pub mod ack_forward;
pub mod aktivierung;
pub mod ausfallarbeit;
pub mod bilarem;
pub mod fristen;
pub mod router;
pub mod stammdaten;

pub use router::{RedispatchDocumentKind, RedispatchRouter};

use mako_engine::{builder::EngineModule, pid_router::PidRouter, profile::ProfileRequirement};

// ── RedispatchModule ──────────────────────────────────────────────────────────

/// Engine module for the Redispatch 2.0 process family.
///
/// Registers:
/// - All 8 Redispatch 2.0 workflows into the caller's `RedispatchRouter`
///   (XML document-type routing, not PID routing).
/// - IFTSTA PIDs 21037 and 21038 into the `PidRouter`
///   (EDIFACT-based Vollzugsmeldung, routes to `redispatch-aktivierung`).
///
/// # Deployment gate
///
/// Only register this module when `DeploymentRoles` contains at least one of
/// `Marktrolle::Nb`, `Marktrolle::Unb`, or `Marktrolle::Anb`:
///
/// ```rust,ignore
/// if roles.contains_any(&[Marktrolle::Nb, Marktrolle::Unb, Marktrolle::Anb]) {
///     builder.register(Box::new(RedispatchModule));
/// }
/// ```
pub struct RedispatchModule;

impl RedispatchModule {
    /// Build a fully-populated [`RedispatchRouter`] for `makod` inbound dispatch.
    ///
    /// Called once during daemon startup, before the HTTP/AS4 servers are bound.
    ///
    /// # Acknowledgement routing
    ///
    /// `AcknowledgementDocument` is intentionally **not** registered in this
    /// router. Inbound ACKs carry a `ReceivingDocumentIdentification` field that
    /// identifies the workflow instance they belong to. The `makod` dispatcher
    /// resolves that correlation key against the `ProcessRegistry` and delivers
    /// the ACK directly to the correct workflow instance — no document-type
    /// routing is needed.
    #[must_use]
    pub fn build_router() -> RedispatchRouter {
        let mut router = RedispatchRouter::new();
        router.register(
            RedispatchDocumentKind::Activation,
            aktivierung::WORKFLOW_NAME,
        );
        router.register(
            RedispatchDocumentKind::PlannedResourceSchedule,
            ack_forward::names::PLANUNGSDATEN,
        );
        // Acknowledgement is routed by correlation (ReceivingDocumentIdentification),
        // not by document kind — do NOT register it here.
        router.register(
            RedispatchDocumentKind::Stammdaten,
            stammdaten::WORKFLOW_NAME,
        );
        router.register(
            RedispatchDocumentKind::StatusRequest,
            ack_forward::names::STATUSANFRAGE,
        );
        router.register(
            RedispatchDocumentKind::Unavailability,
            ack_forward::names::VERFUEGBARKEIT,
        );
        router.register(RedispatchDocumentKind::Kaskade, ack_forward::names::KASKADE);
        router.register(
            RedispatchDocumentKind::NetworkConstraint,
            ack_forward::names::NETZENGPASS,
        );
        router.register(
            RedispatchDocumentKind::Kostenblatt,
            ack_forward::names::KOSTENBLATT,
        );
        router
    }
}

impl EngineModule for RedispatchModule {
    fn name(&self) -> &'static str {
        "redispatch"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            stammdaten::WORKFLOW_NAME,
            aktivierung::WORKFLOW_NAME,
            ack_forward::names::VERFUEGBARKEIT,
            ack_forward::names::NETZENGPASS,
            ack_forward::names::KASKADE,
            ack_forward::names::PLANUNGSDATEN,
            ack_forward::names::STATUSANFRAGE,
            ack_forward::names::KOSTENBLATT,
        ]
    }

    fn register_pids(&self, router: &mut PidRouter) {
        // Redispatch 2.0 uses XML document-type routing, not EDIFACT PIDs.
        // EDIFACT IFTSTA Vollzugsmeldungen for Redispatch 2.0:
        //
        // PID 21037 — Vollzugsmeldung (NB view)
        // PID 21038 — Vollzugsmeldung (BTR view)
        //
        // 21035/21036/21040 are NOT Redispatch PIDs (see aktivierung.rs).
        // Source: IFTSTA AHB 2.1 + PID 4.0 (01.04.2026).
        // These route to the Aktivierung workflow via conversation-ID lookup.
        for &pid in aktivierung::IFTSTA_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }

        // Redispatch 2.0 MSCONS data: 13021 meteorologische Ex-post-Daten,
        // 13022 TR-scharfe Einzelzeitreihe Ausfallarbeit.
        //
        // 13020 and 13023 are **MaBiS** Summenzeitreihen and are registered by
        // `MabisModule`; 13026 belongs to the EEG-Überführungszeitreihen family.
        // See `aktivierung::MSCONS_PIDS` for what routing them here cost.
        for &pid in aktivierung::MSCONS_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }

        // ORDERS 17209 — the anfNB requests the Ausfallarbeit from the ANB,
        // which answers with MSCONS 13022. There is no ORDRSP in this family:
        // 19204 is MaBiS, 19301/19302 belong to the Herkunftsnachweisregister.
        for &pid in aktivierung::ORDERS_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }
    }

    fn profile_requirements(&self) -> &'static [ProfileRequirement] {
        &[
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA Redispatch 2.0 (21037 Ansicht NB, 21038 Ansicht BTR)",
            },
            ProfileRequirement {
                message_type: "MSCONS",
                label: "MSCONS Redispatch (13021 meteorologische Daten, 13022 Einzelzeitreihe Ausfallarbeit)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS Redispatch (17209 Anforderung Ausfallarbeit)",
            },
        ]
    }

    fn configure(&self) -> Result<(), String> {
        // Verify that the router covers all document kinds that use kind-based routing.
        // Acknowledgement is excluded: it is routed by correlation key, not
        // by document kind (see build_router() doc comment).
        let router = Self::build_router();
        for dk in [
            RedispatchDocumentKind::Activation,
            RedispatchDocumentKind::PlannedResourceSchedule,
            RedispatchDocumentKind::Stammdaten,
            RedispatchDocumentKind::StatusRequest,
            RedispatchDocumentKind::Unavailability,
            RedispatchDocumentKind::NetworkConstraint,
            RedispatchDocumentKind::Kaskade,
            RedispatchDocumentKind::Kostenblatt,
        ] {
            router.route(dk).map_err(|e| format!("redispatch: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_router_covers_all_primary_doc_types() {
        let router = RedispatchModule::build_router();
        // All document kinds that use document-kind routing must be registered.
        // Acknowledgement is excluded: it uses correlation-key routing.
        for dk in [
            RedispatchDocumentKind::Activation,
            RedispatchDocumentKind::PlannedResourceSchedule,
            RedispatchDocumentKind::Stammdaten,
            RedispatchDocumentKind::StatusRequest,
            RedispatchDocumentKind::Unavailability,
            RedispatchDocumentKind::Kaskade,
            RedispatchDocumentKind::NetworkConstraint,
            RedispatchDocumentKind::Kostenblatt,
        ] {
            assert!(
                router.is_registered(dk),
                "RedispatchDocumentKind {dk:?} must be registered in RedispatchModule router"
            );
        }
        // Acknowledgement must NOT be registered — it is routed by correlation key.
        assert!(
            !router.is_registered(RedispatchDocumentKind::Acknowledgement),
            "Acknowledgement must not be in the document-kind router"
        );
    }

    #[test]
    fn configure_succeeds() {
        assert!(RedispatchModule.configure().is_ok());
    }

    #[test]
    fn iftsta_pids_are_correct() {
        // Confirmed from IFTSTA AHB 2.1 §8 and PID 4.0 (2026-04-01).
        // Only PIDs 21037 (Ansicht NB/VNB) and 21038 (Ansicht BTR) belong to
        // Redispatch 2.0. PIDs 21035 (GPKE Rückmeldung Lieferstelle → gpke-supplier-change),
        // 21036 (WiM Strom Teil 1, unassigned), and 21040 (AWH Sperrprozesse Gas, unassigned)
        // are not Redispatch PIDs — see site/content/docs/regulatory/pid-reference.md.
        assert_eq!(aktivierung::IFTSTA_PIDS, &[21_037, 21_038]);
    }

    #[test]
    fn mscons_pids_are_correct() {
        // PID 4.0, rows whose Prozessbeschreibung is "Kommunikationsprozesse
        // Redispatch". 13020/13023 are MaBiS and 13026 is the EEG-Überführungs-
        // zeitreihen family; all three used to be claimed here.
        assert_eq!(aktivierung::MSCONS_PIDS, &[13_021, 13_022]);
    }

    #[test]
    fn no_mabis_or_hkn_pid_is_claimed() {
        let claimed: Vec<u32> = aktivierung::IFTSTA_PIDS
            .iter()
            .chain(aktivierung::MSCONS_PIDS)
            .chain(aktivierung::ORDERS_PIDS)
            .copied()
            .collect();
        // MaBiS Summenzeitreihen and list requests.
        for pid in [13_020_u32, 13_023, 17_210, 17_211, 19_204] {
            assert!(!claimed.contains(&pid), "{pid} is a MaBiS PID");
        }
        // Herkunftsnachweisregister.
        for pid in [19_301_u32, 19_302] {
            assert!(!claimed.contains(&pid), "{pid} is an HKN-R PID");
        }
        // EEG-Überführungszeitreihen.
        assert!(!claimed.contains(&13_026));
    }

    #[test]
    fn the_ack_frist_is_three_minutes_everywhere() {
        // The 6-hour figure this crate carried had no published source.
        assert_eq!(fristen::ACK_FRIST, time::Duration::minutes(3));
    }

    #[test]
    fn workflow_names_are_non_empty() {
        assert!(!RedispatchModule.workflow_names().is_empty());
        for name in RedispatchModule.workflow_names() {
            assert!(
                name.starts_with("redispatch-"),
                "workflow name '{name}' must start with 'redispatch-'"
            );
        }
    }
}
