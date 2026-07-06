//! Inbound makod CloudEvents handler.
//!
//! Route: `POST /api/v1/mako/events`
//!
//! Receives CloudEvents 1.0 payloads from `makod`'s outbound webhook channel,
//! verifies the `X-Mako-Signature` HMAC-SHA256 header, deduplicates via the
//! `processed_events` table, and emits the event onto the internal MPSC channel
//! for the fan-out worker.
//!
//! Idempotency: duplicate event IDs return `202 Accepted` without re-processing.
//! The inbound HMAC secret is injected as an axum [`Extension`]:
//! `Extension<InboundWebhookSecret>` added via a layer in `main.rs`.

use std::sync::Arc;

use axum::{
    Extension,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_mdm::{
    cloudevents::{InboundMakoEvent, MdmEvent, verify_signature},
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRepository, SubscriptionRepository,
    },
};
use sqlx::PgPool;
use tracing::{debug, warn};

/// Newtype wrapper for the inbound webhook secret so it can be used as an axum
/// Extension.  `None` means signature verification is disabled.
#[derive(Clone, Debug)]
pub struct InboundWebhookSecret(pub Option<String>);

/// `POST /api/v1/mako/events`
///
/// Request body: CloudEvents 1.0 JSON (`application/cloudevents+json`).
/// Signature header: `X-Mako-Signature: sha256=<hex>`.
pub async fn ingest_event<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(secret): Extension<InboundWebhookSecret>,
    Extension(pool): Extension<PgPool>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    // 1. Verify HMAC signature if a shared secret is configured.
    if let Some(secret_str) = secret.0.as_deref() {
        let sig = headers
            .get("x-mako-signature")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("sha256="));

        match sig {
            Some(hex) if verify_signature(secret_str.as_bytes(), &body, hex) => {}
            Some(_) => {
                warn!("event_ingest: invalid HMAC signature");
                return StatusCode::UNAUTHORIZED.into_response();
            }
            None => {
                warn!("event_ingest: missing or malformed X-Mako-Signature header");
                return StatusCode::UNAUTHORIZED.into_response();
            }
        }
    }

    // 2. Deserialize.
    let event: InboundMakoEvent = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            warn!(%err, "event_ingest: failed to deserialize CloudEvent");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };

    debug!(
        event_id = %event.id,
        ce_type = %event.ce_type,
        "event_ingest: received"
    );

    // 3. Idempotency — INSERT ON CONFLICT returns true only for fresh inserts.
    let is_new: bool = sqlx::query_scalar(
        "INSERT INTO processed_events (event_id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING true",
    )
    .bind(&event.id)
    .fetch_optional(&pool)
    .await
    .unwrap_or(None) // treat DB error conservatively: don't re-process
    .unwrap_or(false);

    if !is_new {
        debug!(event_id = %event.id, "event_ingest: duplicate, skipping");
        return StatusCode::ACCEPTED.into_response();
    }

    // 4. Re-emit as MdmEvent enriched with the tenant GLN as source.
    let mdmrole = mdmrole_from_workflow(event.makoworkflow.as_deref());
    let mdm_event = MdmEvent::new(
        &state.tenant_gln,
        event.ce_type,
        event.subject.unwrap_or_else(|| event.id.clone()),
        event.data,
    )
    .with_extensions(mako_mdm::cloudevents::EventExtensions {
        mdmrole,
        ..Default::default()
    });

    let _ = state.event_tx.send(mdm_event);

    StatusCode::ACCEPTED.into_response()
}

/// Derive the canonical `mdmrole` value from the `makoworkflow` CE extension.
///
/// The mapping is based on the workflow naming convention (kebab-case).
///
/// | Pattern | Role | Example workflows |
/// |---|---|---|
/// | ends with `-lf` | `"LF"` | `gpke-sperrung-lf`, `geli-gas-stornierung-lf` |
/// | contains `-lf-` (infix) | `"LF"` | `gpke-lf-anmeldung`, `gpke-lf-abmeldung` |
/// | starts with `wim-` | `"MSB"` | `wim-device-change`, `wim-gas-anmeldung` |
/// | starts with `mabis-` | `"BIKO"` | `mabis-clearingliste` |
/// | everything else | `"NB"` | `gpke-supplier-change`, `geli-gas-sperrung-nb` |
///
/// Returns `None` when `workflow_name` is absent or empty (legacy outbox
/// messages that predate the `makoworkflow` extension).
fn mdmrole_from_workflow(workflow_name: Option<&str>) -> Option<String> {
    let name = workflow_name.filter(|s| !s.is_empty())?;
    let role = if name.ends_with("-lf") || name.contains("-lf-") {
        // "-lf" suffix:  gpke-sperrung-lf, geli-gas-stornierung-lf, …
        // "-lf-" infix:  gpke-lf-anmeldung, gpke-lf-abmeldung, …
        "LF"
    } else if name.starts_with("wim-") {
        "MSB"
    } else if name.starts_with("mabis-") {
        "BIKO"
    } else {
        // gpke-*, geli-gas-*, gabi-gas-*, dvgw-* — NB is the default
        "NB"
    };
    Some(role.to_owned())
}

#[cfg(test)]
mod tests {
    use super::mdmrole_from_workflow;

    #[test]
    fn lf_suffix_maps_to_lf() {
        assert_eq!(
            mdmrole_from_workflow(Some("gpke-sperrung-lf")),
            Some("LF".into())
        );
        assert_eq!(
            mdmrole_from_workflow(Some("geli-gas-stornierung-lf")),
            Some("LF".into())
        );
        assert_eq!(
            mdmrole_from_workflow(Some("gpke-ankuendigung-zuordnung-lf")),
            Some("LF".into())
        );
    }

    #[test]
    fn lf_infix_maps_to_lf() {
        // "gpke-lf-anmeldung" has "-lf-" in the middle — previously mapped to "NB" (bug)
        assert_eq!(
            mdmrole_from_workflow(Some("gpke-lf-anmeldung")),
            Some("LF".into())
        );
        assert_eq!(
            mdmrole_from_workflow(Some("gpke-lf-abmeldung")),
            Some("LF".into())
        );
    }

    #[test]
    fn wim_prefix_maps_to_msb() {
        assert_eq!(
            mdmrole_from_workflow(Some("wim-device-change")),
            Some("MSB".into())
        );
        assert_eq!(
            mdmrole_from_workflow(Some("wim-gas-anmeldung")),
            Some("MSB".into())
        );
        assert_eq!(
            mdmrole_from_workflow(Some("wim-insrpt")),
            Some("MSB".into())
        );
    }

    #[test]
    fn mabis_prefix_maps_to_biko() {
        assert_eq!(
            mdmrole_from_workflow(Some("mabis-bilanzkreisabrechnung")),
            Some("BIKO".into())
        );
    }

    #[test]
    fn gpke_and_gas_map_to_nb() {
        for name in &[
            "gpke-supplier-change",
            "gpke-sperrung",
            "gpke-konfiguration",
            "geli-gas-lieferbeginn",
            "geli-gas-sperrung-nb",
            "gabi-gas-mmma",
        ] {
            assert_eq!(
                mdmrole_from_workflow(Some(name)),
                Some("NB".into()),
                "expected NB for {name}"
            );
        }
    }

    #[test]
    fn absent_or_empty_returns_none() {
        assert_eq!(mdmrole_from_workflow(None), None);
        assert_eq!(mdmrole_from_workflow(Some("")), None);
    }
}
