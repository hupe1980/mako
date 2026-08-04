//! ESA consent gating — the Einwilligungs-Vorbedingung of WiM Strom Teil 2
//! Kapitel 4, expressed as pure domain policy.
//!
//! An ESA's access to values rests on the Anschlussnutzer's consent
//! (§49 Abs. 2 Nr. 9 MsbG) plus an established bilateral framework agreement
//! with the MSB. This module owns the **policy** of how a consent lookup
//! affects the ESA-Wertebestellung message flow; the lookup itself is behind
//! the [`ConsentGate`] port so the domain stays free of any registry client.
//!
//! # Asymmetric force
//!
//! The consent has different weight on the two sides of the relationship:
//!
//! - **MSB inbound** ([`gate_inbound`]) is **fail-open**. The MSB holds only
//!   the ESA's self-assertion, and BNetzA Mitteilung Nr. 3 (07.02.2024)
//!   forbids rejecting on consent *form*, so a missing record or a failed
//!   lookup never blocks. Only an explicit negative signal — a revoked
//!   consent or an unestablished framework agreement — turns the command into
//!   its Ablehnung by setting `consent_block`. The durable stop signal
//!   remains the 17008 Abbestellung fired on revocation.
//! - **ESA outbound** ([`gate_outbound`]) is **fail-closed**. The ESA is the
//!   data controller that obtained the Einwilligung; without a confirmed,
//!   non-revoked consent it has no lawful basis (GDPR Art. 7) to originate a
//!   Werteanfrage or Bestellung, so a block *and* a failed lookup both reject.

use std::future::Future;

use mako_engine::types::{MaLo, MarktpartnerCode};

use crate::wertebestellung::WertebestellungCommand;

// ── Port types ────────────────────────────────────────────────────────────────

/// Which side of the ESA relationship is gating a message.
///
/// See the module docs for why the two perspectives carry asymmetric force.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConsentPerspective {
    /// MSB *receiving* an inbound ESA order. Lenient: a missing consent record
    /// is self-assertion and never blocks (BNetzA forbids form-based rejection).
    #[default]
    MsbInbound,
    /// ESA *originating* an outbound request (Werteanfrage/Bestellung).
    /// Strict: the ESA must hold a recorded, non-revoked consent — a missing
    /// record is no lawful basis (GDPR Art. 7), so it blocks.
    EsaOutbound,
}

/// Why an ESA-message consent check allowed or blocked delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentCode {
    /// An active, non-revoked consent covers the location — deliver.
    Active,
    /// No consent record for the location, seen from the **MSB** side. The
    /// ESA's self-assertion stands and BNetzA forbids rejecting on consent
    /// *form*, so absence alone never blocks — deliver.
    SelfAssertion,
    /// No consent record for the location, seen from the **ESA** side. The ESA
    /// holds no lawful basis (GDPR Art. 7) and must not originate the request —
    /// block.
    NoConsent,
    /// A recorded consent for the location has been revoked (GDPR Art. 7(3))
    /// and no active consent superseded it — block (the Widerruf clearing case).
    Revoked,
    /// A framework agreement exists but is not established (no EDI agreement or
    /// a negative cert state) — the UC 4.1.1 Vorbedingung is unmet, so block.
    FrameworkRejected,
}

impl ConsentCode {
    /// Whether this outcome permits the message.
    #[must_use]
    pub const fn allowed(self) -> bool {
        matches!(self, Self::Active | Self::SelfAssertion)
    }
}

/// Outcome of a consent-registry lookup for one ESA/MSB/location triple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentDecision {
    /// `true` when the ESA message may be processed.
    pub allowed: bool,
    /// Machine-readable reason.
    pub code: ConsentCode,
    /// Human-readable reason (used verbatim as the Ablehnung Begründung).
    pub reason: String,
}

/// The lookup failed — network, registry outage, malformed response.
///
/// Carries a display string only; the *policy* consequence (pass inbound,
/// reject outbound) is decided by [`gate_inbound`] / [`gate_outbound`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentGateError(pub String);

impl std::fmt::Display for ConsentGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConsentGateError {}

/// Port to the consent registry.
///
/// Implemented by the application layer (makod adapts its `marktd` client);
/// the domain only ever sees a [`ConsentDecision`] or a [`ConsentGateError`].
pub trait ConsentGate {
    /// Check whether `esa_mp_id` may exchange values for `location_id` with
    /// `msb_mp_id`, judged from `perspective`.
    ///
    /// `location_id` is deliberately a plain string slice: the location may be
    /// a MaLo, a Zählpunktbezeichnung (MeLo) or a NeLo-ID, depending on the
    /// [`Lokationsebene`](crate::wertebestellung::Lokationsebene).
    fn check(
        &self,
        esa_mp_id: &MarktpartnerCode,
        msb_mp_id: &MarktpartnerCode,
        location_id: &str,
        perspective: ConsentPerspective,
    ) -> impl Future<Output = Result<ConsentDecision, ConsentGateError>> + Send;
}

// ── Gating policy ─────────────────────────────────────────────────────────────

/// Gate an inbound ESA [`WertebestellungCommand`] against the consent registry.
///
/// **Fail-open** (MSB perspective). Sets `consent_block` on
/// [`WertebestellungCommand::ReceiveAnfrage`] /
/// [`WertebestellungCommand::ReceiveBestellung`] when the registry reports the
/// delivery blocked (a revoked consent or an unestablished framework agreement
/// — the clearing case). Everything else leaves the command untouched: a
/// non-gated command variant, an active consent, self-assertion, an empty
/// sender or location (nothing to check), or a lookup error.
///
/// `location` may be empty; for a `ReceiveAnfrage` the command's own
/// `lokations_id` is the fallback (REQOTE carries the location in LOC, not
/// IDE, so the transport-level extraction can come up empty).
pub async fn gate_inbound<G: ConsentGate>(
    cmd: WertebestellungCommand,
    esa: &MarktpartnerCode,
    msb: &MarktpartnerCode,
    location: &MaLo,
    gate: &G,
) -> WertebestellungCommand {
    use WertebestellungCommand as C;

    // Only the two inbound-order commands are gated.
    if !matches!(cmd, C::ReceiveAnfrage { .. } | C::ReceiveBestellung { .. }) {
        return cmd;
    }

    // REQOTE carries the location in LOC, not IDE — fall back for the Anfrage.
    let location: &str = if location.as_str().is_empty() {
        if let C::ReceiveAnfrage { lokations_id, .. } = &cmd {
            lokations_id.as_str()
        } else {
            location.as_str()
        }
    } else {
        location.as_str()
    };
    if esa.as_str().is_empty() || location.is_empty() {
        return cmd;
    }
    let location = location.to_owned();

    // The ingest boundary is the MSB *receiving* an ESA order — lenient: a
    // missing consent record is self-assertion, never a rejection; a failed
    // lookup fails open (the 17008 Abbestellung remains the stop signal).
    let decision = match gate
        .check(esa, msb, &location, ConsentPerspective::MsbInbound)
        .await
    {
        Ok(d) => d,
        Err(_) => return cmd,
    };
    if decision.allowed {
        return cmd;
    }

    match cmd {
        C::ReceiveAnfrage {
            pid,
            esa,
            msb,
            ebene,
            lokations_id,
            message_ref,
            quittung,
            ..
        } => C::ReceiveAnfrage {
            pid,
            esa,
            msb,
            ebene,
            lokations_id,
            message_ref,
            quittung,
            consent_block: Some(decision.reason),
        },
        C::ReceiveBestellung {
            pid,
            message_ref,
            quittung,
            ..
        } => C::ReceiveBestellung {
            pid,
            message_ref,
            quittung,
            consent_block: Some(decision.reason),
        },
        other => other,
    }
}

/// Enforce the strict outbound consent gate before an ESA originates a
/// Werteanfrage or Bestellung.
///
/// **Fail-closed** (ESA perspective): a blocked decision *and* a failed lookup
/// both reject — the ESA must not request values it cannot confirm a lawful
/// basis for.
///
/// # Errors
///
/// Returns the rejection Begründung (used verbatim by the caller, e.g. as an
/// HTTP 422 detail) when the consent is missing/blocked or could not be
/// checked.
pub async fn gate_outbound<G: ConsentGate>(
    esa: &MarktpartnerCode,
    msb: &MarktpartnerCode,
    location: &MaLo,
    gate: &G,
) -> Result<(), String> {
    match gate
        .check(esa, msb, location.as_str(), ConsentPerspective::EsaOutbound)
        .await
    {
        Ok(d) if d.allowed => Ok(()),
        Ok(d) => Err(format!(
            "ESA-Einwilligung fehlt für {location}: {} (Rechtsgrundlage nach GDPR Art. 7 \
             erforderlich, bevor der ESA Werte anfragt)",
            d.reason
        )),
        Err(e) => Err(format!(
            "ESA-Einwilligung konnte nicht geprüft werden für {location}: {e}"
        )),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wertebestellung::{Lokationsebene, Zustellquittung};
    use mako_engine::types::{MessageRef, Pruefidentifikator};
    use time::OffsetDateTime;

    /// In-memory [`ConsentGate`] fake returning a fixed outcome.
    struct FakeGate(Result<ConsentDecision, ConsentGateError>);

    impl FakeGate {
        fn allowing() -> Self {
            Self(Ok(ConsentDecision {
                allowed: true,
                code: ConsentCode::Active,
                reason: "aktive Einwilligung".to_owned(),
            }))
        }

        fn blocking(reason: &str) -> Self {
            Self(Ok(ConsentDecision {
                allowed: false,
                code: ConsentCode::Revoked,
                reason: reason.to_owned(),
            }))
        }

        fn failing() -> Self {
            Self(Err(ConsentGateError("marktd unreachable".to_owned())))
        }
    }

    impl ConsentGate for FakeGate {
        async fn check(
            &self,
            _esa: &MarktpartnerCode,
            _msb: &MarktpartnerCode,
            _location: &str,
            _perspective: ConsentPerspective,
        ) -> Result<ConsentDecision, ConsentGateError> {
            self.0.clone()
        }
    }

    fn anfrage() -> WertebestellungCommand {
        WertebestellungCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::const_new(35003),
            esa: MarktpartnerCode::new("9900000000001"),
            msb: MarktpartnerCode::new("9900000000002"),
            ebene: Lokationsebene::Marktlokation,
            lokations_id: "57685676748".to_owned(),
            message_ref: MessageRef::new("MSG-1"),
            quittung: Zustellquittung::positive(OffsetDateTime::UNIX_EPOCH),
            consent_block: None,
        }
    }

    fn bestellung() -> WertebestellungCommand {
        WertebestellungCommand::ReceiveBestellung {
            pid: Pruefidentifikator::const_new(17007),
            message_ref: MessageRef::new("MSG-2"),
            quittung: Zustellquittung::positive(OffsetDateTime::UNIX_EPOCH),
            consent_block: None,
        }
    }

    fn ids() -> (MarktpartnerCode, MarktpartnerCode, MaLo) {
        (
            MarktpartnerCode::new("9900000000001"),
            MarktpartnerCode::new("9900000000002"),
            MaLo::new("57685676748"),
        )
    }

    fn consent_block(cmd: &WertebestellungCommand) -> Option<&str> {
        match cmd {
            WertebestellungCommand::ReceiveAnfrage { consent_block, .. }
            | WertebestellungCommand::ReceiveBestellung { consent_block, .. } => {
                consent_block.as_deref()
            }
            _ => None,
        }
    }

    #[tokio::test]
    async fn inbound_active_consent_passes_untouched() {
        let (esa, msb, loc) = ids();
        let cmd = gate_inbound(anfrage(), &esa, &msb, &loc, &FakeGate::allowing()).await;
        assert_eq!(consent_block(&cmd), None);
    }

    #[tokio::test]
    async fn inbound_block_rewrites_consent_block_on_anfrage_and_bestellung() {
        let (esa, msb, loc) = ids();
        let gate = FakeGate::blocking("Einwilligung widerrufen");
        let cmd = gate_inbound(anfrage(), &esa, &msb, &loc, &gate).await;
        assert_eq!(consent_block(&cmd), Some("Einwilligung widerrufen"));
        let cmd = gate_inbound(bestellung(), &esa, &msb, &loc, &gate).await;
        assert_eq!(consent_block(&cmd), Some("Einwilligung widerrufen"));
    }

    #[tokio::test]
    async fn inbound_lookup_error_fails_open() {
        let (esa, msb, loc) = ids();
        let cmd = gate_inbound(bestellung(), &esa, &msb, &loc, &FakeGate::failing()).await;
        assert_eq!(consent_block(&cmd), None);
    }

    #[tokio::test]
    async fn inbound_empty_location_falls_back_to_anfrage_lokations_id() {
        let (esa, msb, _) = ids();
        // Empty extracted location: the Anfrage's own lokations_id kicks in,
        // so the gate still runs and blocks.
        let cmd = gate_inbound(
            anfrage(),
            &esa,
            &msb,
            &MaLo::new(""),
            &FakeGate::blocking("widerrufen"),
        )
        .await;
        assert_eq!(consent_block(&cmd), Some("widerrufen"));
        // The Bestellung has no location of its own → nothing to check → pass.
        let cmd = gate_inbound(
            bestellung(),
            &esa,
            &msb,
            &MaLo::new(""),
            &FakeGate::blocking("widerrufen"),
        )
        .await;
        assert_eq!(consent_block(&cmd), None);
    }

    #[tokio::test]
    async fn inbound_non_gated_variant_is_untouched_even_when_blocked() {
        let (esa, msb, loc) = ids();
        let storno = WertebestellungCommand::ReceiveStornierung {
            pid: Pruefidentifikator::const_new(39002),
            message_ref: MessageRef::new("MSG-3"),
            quittung: Zustellquittung::positive(OffsetDateTime::UNIX_EPOCH),
        };
        let cmd = gate_inbound(
            storno.clone(),
            &esa,
            &msb,
            &loc,
            &FakeGate::blocking("widerrufen"),
        )
        .await;
        assert_eq!(cmd, storno);
    }

    #[tokio::test]
    async fn outbound_active_consent_allows() {
        let (esa, msb, loc) = ids();
        assert_eq!(
            gate_outbound(&esa, &msb, &loc, &FakeGate::allowing()).await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn outbound_block_rejects_with_begruendung() {
        let (esa, msb, loc) = ids();
        let err = gate_outbound(&esa, &msb, &loc, &FakeGate::blocking("keine Einwilligung"))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "ESA-Einwilligung fehlt für 57685676748: keine Einwilligung (Rechtsgrundlage \
             nach GDPR Art. 7 erforderlich, bevor der ESA Werte anfragt)"
        );
    }

    #[tokio::test]
    async fn outbound_lookup_error_fails_closed() {
        let (esa, msb, loc) = ids();
        let err = gate_outbound(&esa, &msb, &loc, &FakeGate::failing())
            .await
            .unwrap_err();
        assert_eq!(
            err,
            "ESA-Einwilligung konnte nicht geprüft werden für 57685676748: marktd unreachable"
        );
    }
}
