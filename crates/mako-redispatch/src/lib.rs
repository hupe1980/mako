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
//! | `BNetzA` decision | Topic |
//! |---|---|
//! | BK6-20-059 | `AcknowledgementDocument` (6h), `StatusRequest` (24h) |
//! | BK6-20-060 | `Stammdaten` (1 Werktag), Activation (5 min) |
//! | BK6-20-061 | `Kostenblatt` (15th of following month) |
//!
//! # Regulatory deadlines
//!
//! | Obligation | Deadline | Clock |
//! |---|---|---|
//! | `AcknowledgementDocument` | 6 wall-clock hours | **UTC** |
//! | `StatusRequest` response | 24 wall-clock hours | **UTC** |
//! | Stammdaten forward (VNB→ÜNB) | 1 Werktag | German local time |
//! | Activation (ACO) response | **5 minutes** | **UTC** |
//! | Kostenblatt submission | 15th of following month | German local time |
//!
//! > **Clock semantics differ from GPKE/WiM.** Redispatch 2.0 uses UTC
//! > wall-clock hours for the acknowledgement and activation deadlines.
//! > Only the Stammdaten-forwarding and Kostenblatt obligations follow
//! > German local time (CET/CEST) + Werktag rules.
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
//! | [`ack_forward`] (Kaskade) | `redispatch-kaskade` | `Kaskade` |
//! | [`ack_forward`] (Planungsdaten) | `redispatch-planungsdaten` | `PlannedResourceScheduleDocument` |
//! | [`ack_forward`] (Statusanfrage) | `redispatch-statusanfrage` | `StatusRequest_MarketDocument` |
//! | [`ack_forward`] (Kostenblatt) | `redispatch-kostenblatt` | `Kostenblatt` |

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]

pub mod ack_forward;
pub mod aktivierung;
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
        // EDIFACT IFTSTA PIDs carry Redispatch status messages:
        //
        // PID 21035 — Redispatch / Statusmeldung
        // PID 21036 — Redispatch / Statusmeldung Aktivierungsauftrag
        // PID 21037 — Redispatch / Statusmeldung Einspeisemanagement
        // PID 21038 — Redispatch / Statusmeldung Abrechnungsinformation
        // PID 21040 — Redispatch / Statusmeldung Bilanzkreiszuordnung
        //
        // Source: IFTSTA AHB 2.1 + PID 4.0 (01.04.2026).
        // These route to the Aktivierung workflow via conversation-ID lookup.
        for &pid in aktivierung::IFTSTA_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }

        // Redispatch 2.0 MSCONS time-series data (PIDs 13020–13026).
        //
        // These carry Ausfallarbeit, meteorological data, and EEG
        // transfer time-series correlated to the Aktivierung process.
        for &pid in aktivierung::MSCONS_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }

        // Redispatch 2.0 ORDERS and ORDRSP PIDs (Ausfallarbeit / Abo-Verwaltung).
        //
        // ORDERS 17209/17210/17211: anfNB requests Ausfallarbeit or
        // Lieferantenausfallarbeitsclearingliste, or files a Reklamation.
        // ORDRSP 19204/19301/19302: BTR/ÜNB responds to subscription/aggregation requests.
        for &pid in aktivierung::ORDERS_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }
        for &pid in aktivierung::ORDRSP_PIDS {
            router.register(pid, aktivierung::WORKFLOW_NAME);
        }
    }

    fn profile_requirements(&self) -> &'static [ProfileRequirement] {
        &[
            ProfileRequirement {
                message_type: "IFTSTA",
                label: "IFTSTA (Redispatch 2.0 Statusmeldungen — PIDs 21035/21036/21037/21038/21040)",
            },
            ProfileRequirement {
                message_type: "MSCONS",
                label: "MSCONS Redispatch Ausfallarbeit/EEG (13020–13023, 13026)",
            },
            ProfileRequirement {
                message_type: "ORDERS",
                label: "ORDERS Redispatch Ausfallarbeit (17209–17211)",
            },
            ProfileRequirement {
                message_type: "ORDRSP",
                label: "ORDRSP Redispatch Abo/Aggregation (19204, 19301–19302)",
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
        // are not Redispatch PIDs — see docs/pid-reference.md.
        assert_eq!(aktivierung::IFTSTA_PIDS, &[21_037, 21_038]);
    }

    #[test]
    fn mscons_pids_are_correct() {
        // Confirmed from MSCONS AHB (Redispatch 2.0 Annex) + PID 4.0.
        assert_eq!(
            aktivierung::MSCONS_PIDS,
            &[13_020, 13_021, 13_022, 13_023, 13_026]
        );
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
