//! Inbound makod CloudEvents handler.
//!
//! Route: `POST /api/v1/mako/events`
//!
//! Receives CloudEvents 1.0 payloads from `makod`'s outbound webhook channel,
//! verifies the `X-Mako-Signature` HMAC-SHA256 header, deduplicates via the
//! `processed_events` table, and emits the event onto the internal MPSC channel
//! for the fan-out worker.
//!
//! # Architecture
//!
//! `marktd` is a **pure data hub** — it does not make Anmeldung decisions.
//! Automated STP decisions (NB role, PIDs 55001/55016/44001) are handled by
//! `processd` via the EventBus subscription.  `marktd` simply:
//!
//! 1. Verifies the HMAC signature
//! 2. Deduplicates via `processed_events`
//! 3. Enriches the event with `marktrole` and emits to all subscribers
//! 4. Derives `VersorgungsStatus` for PIDs 55003/44003 (Beliefert) and 55013/44013 (Unbeliefert)
//!
//! Idempotency: duplicate event IDs return `202 Accepted` without re-processing.

use std::sync::Arc;

use axum::{
    Extension,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::{EventExtensions, InboundMakoEvent, MarktEvent, verify_signature},
    repository::{
        AppState, ContractRepository, CorrelationIndex, LieferStatus, MaloRepository,
        MeloRepository, PartnerRepository, SubscriptionRepository, VersorgungsStatusRecord,
        VersorgungsStatusRepository,
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
    Extension(vs_repo): Extension<Arc<crate::pg::PgVersorgungsStatusRepository>>,
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
            .map(|v| v.strip_prefix("sha256=").unwrap_or(v));

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

    // 4. Re-emit as MarktEvent enriched with the tenant GLN as source.
    //
    // Phase 1 — capture values needed for VersorgungsStatus derivation before
    // event fields are moved into MarktEvent.
    let process_completed = event.ce_type == "de.mako.process.completed";
    let event_id_for_vs = event.id.clone();
    let pid_for_vs = event.makopid;
    let subject_for_vs = event.subject.clone();
    let data_for_vs = if process_completed {
        Some(event.data.clone())
    } else {
        None
    };

    let marktrole = marktrole_from_workflow(event.makoworkflow.as_deref());
    let markt_event = MarktEvent::new(
        &state.tenant_gln,
        event.ce_type,
        event.subject.unwrap_or_else(|| event.id.clone()),
        event.data,
    )
    .with_extensions(EventExtensions {
        marktrole,
        makopid: event.makopid,
        makoworkflow: event.makoworkflow,
        ..Default::default()
    });

    if let Ok(payload) = serde_json::to_value(&markt_event) {
        let _ = state.event_tx.send(payload);
    }

    // 5. Phase 1 — derive VersorgungsStatus from de.mako.process.completed.
    //
    // PID-to-state mapping (GPKE BK6-22-024 + GeLi Gas BK7-24-01-009):
    //   55003 → Beliefert   (NB confirms Lieferbeginn; LFN assigned)
    //   44003 → Beliefert   (GeLi Gas: NB confirms Gas-Lieferbeginn)
    //   55013 → Unbeliefert (Abmeldung-Bestätigung received; LF removed)
    //   44013 → Unbeliefert (GeLi Gas: Abmeldung-Bestätigung)
    //
    // Upsert uses if_version=None (blind) so concurrent updates to unrelated
    // fields converge without version conflicts.  At-least-once delivery from
    // EventBus guarantees eventual convergence even when the spawn races.
    if process_completed && let Some(pid) = pid_for_vs {
        let lieferstatus: Option<LieferStatus> = match pid {
            55003 | 44003 => Some(LieferStatus::Beliefert),
            55013 | 44013 => Some(LieferStatus::Unbeliefert),
            _ => None,
        };

        if let (Some(lieferstatus), Some(subject)) = (lieferstatus, subject_for_vs)
            && !subject.is_empty()
        {
            let data = data_for_vs.unwrap_or(serde_json::Value::Null);
            // Parse as MaloId — if the subject is not a valid 11-digit MaLo-ID
            // (e.g. it's a process UUID on non-MaLo events), skip silently.
            let malo_id = match subject.parse::<mako_markt::domain::MaloId>() {
                Ok(id) => id,
                Err(_) => return StatusCode::ACCEPTED.into_response(),
            };
            let lf_mp_id = if lieferstatus == LieferStatus::Beliefert {
                data.get("lieferant_gln")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            } else {
                None
            };
            let nb_mp_id = data
                .get("nb_mp_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| state.tenant_gln.clone());
            let process_id = uuid::Uuid::parse_str(&event_id_for_vs).ok();
            let rec = VersorgungsStatusRecord {
                malo_id,
                lieferstatus,
                lf_mp_id,
                lf_gln_next: None,
                lieferbeginn: None,
                lieferende: None,
                msb_mp_id: None,
                nb_mp_id,
                last_process_id: process_id,
                updated_at: time::OffsetDateTime::now_utc(),
                tenant: state.tenant_gln.clone(),
                version: 0,
            };
            let vs = Arc::clone(&vs_repo);
            tokio::spawn(async move {
                if let Err(e) = vs.upsert(rec, None).await {
                    tracing::warn!(
                        malo_id = %subject,
                        pid,
                        error = %e,
                        "event_ingest: failed to upsert VersorgungsStatus"
                    );
                }
            });
        }
    }

    StatusCode::ACCEPTED.into_response()
}

/// Derive the canonical `marktrole` value from the `makoworkflow` CE extension.
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
pub(crate) fn marktrole_from_workflow(workflow_name: Option<&str>) -> Option<String> {
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
    use super::marktrole_from_workflow;

    #[test]
    fn lf_suffix_maps_to_lf() {
        assert_eq!(
            marktrole_from_workflow(Some("gpke-sperrung-lf")),
            Some("LF".into())
        );
        assert_eq!(
            marktrole_from_workflow(Some("geli-gas-stornierung-lf")),
            Some("LF".into())
        );
        assert_eq!(
            marktrole_from_workflow(Some("gpke-ankuendigung-zuordnung-lf")),
            Some("LF".into())
        );
    }

    #[test]
    fn lf_infix_maps_to_lf() {
        assert_eq!(
            marktrole_from_workflow(Some("gpke-lf-anmeldung")),
            Some("LF".into())
        );
        assert_eq!(
            marktrole_from_workflow(Some("gpke-lf-abmeldung")),
            Some("LF".into())
        );
    }

    #[test]
    fn wim_prefix_maps_to_msb() {
        assert_eq!(
            marktrole_from_workflow(Some("wim-device-change")),
            Some("MSB".into())
        );
        assert_eq!(
            marktrole_from_workflow(Some("wim-gas-anmeldung")),
            Some("MSB".into())
        );
        assert_eq!(
            marktrole_from_workflow(Some("wim-insrpt")),
            Some("MSB".into())
        );
    }

    #[test]
    fn mabis_prefix_maps_to_biko() {
        assert_eq!(
            marktrole_from_workflow(Some("mabis-bilanzkreisabrechnung")),
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
                marktrole_from_workflow(Some(name)),
                Some("NB".into()),
                "expected NB for {name}"
            );
        }
    }

    #[test]
    fn none_and_empty_return_none() {
        assert_eq!(marktrole_from_workflow(None), None);
        assert_eq!(marktrole_from_workflow(Some("")), None);
    }
}
