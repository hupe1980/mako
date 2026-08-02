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
//! 4. Derives `VersorgungsStatus` for PIDs 55001/44001 (announce), 55002/44002 (confirm), 55003/44003 (clear on Ablehnung), 55005/44005 (end + gap detection), 55013/44013 (begin Ersatz-/Grundversorgung)
//!
//! Idempotency: duplicate event IDs return `202 Accepted` without re-processing.

use std::sync::Arc;

use crate::pg::{
    PgDeviceRepository, PgNeLoRepository, PgTechnischeRessourceRepository, PgTrancheRepository,
    PgZaehlzeitRepository,
};
use axum::{
    Extension,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::repository::DeviceRepository;
use mako_markt::{
    cloudevents::{EventExtensions, InboundMakoEvent, MarktEvent},
    repository::{
        AppState, CorrelationIndex, MaloRepository, MeloRepository, PartnerRepository,
        SubscriptionRepository, VersorgungsStatusRepository,
    },
};
use sqlx::PgPool;
use tracing::{debug, error, warn};

/// Newtype wrapper for the inbound webhook secret so it can be used as an axum
/// Extension.  `None` means signature verification is disabled.
#[derive(Clone, Debug)]
pub struct InboundWebhookSecret(pub Option<String>);

/// `POST /api/v1/mako/events`
///
/// Request body: CloudEvents 1.0 JSON (`application/cloudevents+json`).
/// Signature header: `X-Mako-Signature: sha256=<hex>`.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_event<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(secret): Extension<InboundWebhookSecret>,
    Extension(pool): Extension<PgPool>,
    Extension(vs_repo): Extension<Arc<crate::pg::PgVersorgungsStatusRepository>>,
    Extension(device_repo): Extension<Arc<PgDeviceRepository>>,
    Extension(zaehzeit_repo): Extension<Arc<PgZaehlzeitRepository>>,
    Extension(nelo_repo): Extension<Arc<PgNeLoRepository>>,
    Extension(tranche_repo): Extension<Arc<PgTrancheRepository>>,
    Extension(tr_repo): Extension<Arc<PgTechnischeRessourceRepository>>,
    Extension(sr_repo): Extension<Arc<crate::pg::PgSteuerbareRessourceRepository>>,
    Extension(melo_msb_repo): Extension<Arc<crate::pg::PgMeloMsbRepository>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
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
            Some(hex) if mako_service::webhook::verify_hmac(secret_str.as_bytes(), &body, hex) => {}
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
    let ce_type_for_vs = event.ce_type.clone();
    let event_id_for_vs = event.id.clone();
    let pid_for_vs = event.makopid;
    let data_for_vs = event.data.clone();

    let marktrole = marktrole_from_workflow(event.makoworkflow.as_deref());
    let markt_event = MarktEvent::new(
        &state.tenant_gln,
        event.ce_type,
        event.subject.unwrap_or_else(|| event.id.clone()),
        event.data,
    )
    .with_extensions(EventExtensions {
        marktrole,
        makoconvid: event.makoconvid,
        makopid: event.makopid,
        makoworkflow: event.makoworkflow,
        // B10: forward W3C Trace Context unchanged so subscribers can continue
        // the distributed trace without re-sampling.
        traceparent: event.traceparent,
        tracestate: event.tracestate,
        ..Default::default()
    });

    // Durable, persist-before-fan-out: the enqueue INSERT is FATAL. If it fails
    // we roll back the idempotency marker so a client retry re-processes the
    // event (otherwise the duplicate guard at step 3 would swallow it and the
    // event would be lost).
    if let Err(e) = crate::outbox::enqueue(&pool, &markt_event, &state.notify).await {
        error!(event_id = %event_id_for_vs, error = %e, "event_ingest: durable enqueue failed; rolling back idempotency marker");
        let _ = sqlx::query("DELETE FROM processed_events WHERE event_id = $1")
            .bind(&event_id_for_vs)
            .execute(&pool)
            .await;
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // 5. Derive VersorgungsStatus from supply-state-changing CloudEvents.
    //
    // Event → action mapping (GPKE BK6-24-174 + GeLi Gas 3.0 (BK7-24-01-009)):
    //
    //   process.initiated  + PID 55001/44001
    //     → announce_lf_next: set lf_mp_id_next + lf_next_lieferbeginn
    //       (NB side: new_supplier + process_date from ProcessInitiated payload)
    //
    //   process.completed  + PID 55002/44002 (Bestätigung Anmeldung)
    //     → confirm_supply: promote lf_mp_id_next → lf_mp_id (atomic SQL)
    //
    //   process.completed  + PID 55003/44003 (Ablehnung Anmeldung)
    //     → clear_lf_next: drop the announced future Lieferant
    //
    //   process.completed  + PID 55005/44005 (Bestätigung Lieferende)
    //     → end_supply: lieferstatus = Unbeliefert, clear lf_mp_id
    //       (preserves lf_mp_id_next / lf_next_lieferbeginn for pending transition);
    //       when no successor is announced, emit de.markt.versorgung.gap-detected
    //       — the §38 EnWG gap-closure trigger consumed by processd
    //
    //   process.completed  + PID 55013/44013 (Anmeldung/Zuordnung EOG)
    //     → begin_eog_supply: the E/G becomes the supplier of record
    //       (lieferstatus = Ersatzversorgung/Grundversorgung per data.eog_art,
    //        eog_seit = data.process_date — anchors the §38 Abs. 2 3-month clock);
    //       emits de.markt.versorgung.eog-begonnen
    //
    // The CE subject is always the process UUID — malo_id is extracted from
    // the data payload.  All actions are idempotent under at-least-once delivery.
    {
        let is_initiated = ce_type_for_vs == mako_events::mako::PROCESS_INITIATED;
        let is_completed = ce_type_for_vs == mako_events::mako::PROCESS_COMPLETED;

        if let Some(pid) = pid_for_vs {
            // Extract malo_id from data payload — the CE subject is a process UUID.
            let malo_id_str = data_for_vs
                .get("malo_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);

            if let Some(malo_str) = malo_id_str {
                // GPKE Teil 4 / GeLi Gas Stammdatenänderung apply — object-generic.
                // Runs BEFORE the MaLo-ID parse gate below because non-MaLo object
                // IDs (MeLo DE+31, NeLo EIC, Tranche) are not valid MaLo-IDs. The
                // workflow tags the ProcessCompleted with the `objekt` marker; we
                // route it to the matching typed-column patch_stammdaten.
                if is_completed && let Some(patch_val) = data_for_vs.get("stammdaten_patch") {
                    let objekt = data_for_vs
                        .get("objekt")
                        .and_then(|v| v.as_str())
                        .unwrap_or("MARKTLOKATION");
                    let aenderungsdatum =
                        data_for_vs.get("aenderungsdatum").and_then(|v| v.as_str());
                    apply_object_stammdaten(
                        &state,
                        &pool,
                        nelo_repo.as_ref(),
                        tranche_repo.as_ref(),
                        tr_repo.as_ref(),
                        sr_repo.as_ref(),
                        melo_msb_repo.as_ref(),
                        objekt,
                        &malo_str,
                        pid,
                        aenderungsdatum,
                        patch_val,
                    )
                    .await;
                }

                let malo_id = malo_str.parse::<mako_markt::domain::MaloId>();
                let nb_mp_id = data_for_vs
                    .get("nb_mp_id")
                    .or_else(|| data_for_vs.get("grid_operator"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| state.tenant_gln.clone());
                let process_id = uuid::Uuid::parse_str(&event_id_for_vs).ok();

                if let Ok(malo_id) = malo_id {
                    let vs = Arc::clone(&vs_repo);

                    if is_initiated && matches!(pid, 55001 | 44001) {
                        // NB received Lieferbeginn Anfrage — record the pending transition.
                        let lf_mp_id_next = data_for_vs
                            .get("new_supplier")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        let lf_next_lieferbeginn = data_for_vs
                            .get("process_date")
                            .and_then(|v| v.as_str())
                            .and_then(|s| {
                                time::Date::parse(
                                    s,
                                    &time::format_description::well_known::Iso8601::DEFAULT,
                                )
                                .ok()
                            });
                        if let Some(lf_mp_id_next) = lf_mp_id_next
                            && let Err(e) = vs
                                .announce_lf_next(
                                    &malo_id,
                                    &state.tenant_gln,
                                    &lf_mp_id_next,
                                    lf_next_lieferbeginn,
                                    &nb_mp_id,
                                    process_id,
                                )
                                .await
                        {
                            tracing::warn!(
                                malo_id = %malo_str,
                                pid,
                                error = %e,
                                "event_ingest: failed to announce_lf_next"
                            );
                        }

                        // L1/N1: Patch malo.bilanzierungsmethode + malo.fallgruppe
                        // from the ProcessInitiated payload.  These are populated
                        // by the makod GPKE/GeLi Gas adapter from UTILMD TM+EM /
                        // TM+Z10 segments and propagated into the outbox event.
                        // Best-effort: failure is logged but does not affect the
                        // VersorgungsStatus update above.
                        let bilanzierungsmethode = data_for_vs
                            .get("bilanzierungsmethode")
                            .and_then(|v| v.as_str());
                        let fallgruppe = data_for_vs.get("fallgruppe").and_then(|v| v.as_str());
                        if bilanzierungsmethode.is_some() || fallgruppe.is_some() {
                            if let Err(e) = state
                                .malo_repo
                                .patch_typenmerkmal(&malo_id, bilanzierungsmethode, fallgruppe)
                                .await
                            {
                                tracing::warn!(
                                    malo_id = %malo_str,
                                    pid,
                                    error = %e,
                                    "event_ingest: patch_typenmerkmal failed (non-fatal)"
                                );
                            } else if bilanzierungsmethode.is_some() || fallgruppe.is_some() {
                                tracing::debug!(
                                    malo_id = %malo_str,
                                    bilanzierungsmethode,
                                    fallgruppe,
                                    "event_ingest: patched malo Typenmerkmale from ProcessInitiated"
                                );
                            }
                        }
                    } else if is_completed && matches!(pid, 55002 | 44002) {
                        // NB confirmed Lieferbeginn (Bestätigung Anmeldung) —
                        // promote the announced LF to active.
                        if let Err(e) = vs
                            .confirm_supply(&malo_id, &state.tenant_gln, process_id)
                            .await
                        {
                            tracing::warn!(
                                malo_id = %malo_str,
                                pid,
                                error = %e,
                                "event_ingest: failed to confirm_supply"
                            );
                        }
                    } else if is_completed && matches!(pid, 55005 | 44005) {
                        // Bestätigung Lieferende — active LF removed; preserve
                        // pending transition. When no successor is announced,
                        // the MaLo is in a §38 EnWG supply gap: emit the
                        // gap-detected trigger for the processd EoG automation.
                        if let Err(e) = vs
                            .end_supply(&malo_id, &state.tenant_gln, &nb_mp_id, process_id)
                            .await
                        {
                            tracing::warn!(
                                malo_id = %malo_str,
                                pid,
                                error = %e,
                                "event_ingest: failed to end_supply"
                            );
                        } else {
                            match vs.find(&malo_id, &state.tenant_gln).await {
                                Ok(Some(rec)) if rec.lf_mp_id_next.is_none() => {
                                    let gap_evt = MarktEvent::new(
                                        &state.tenant_gln,
                                        mako_events::markt::VERSORGUNG_GAP_DETECTED,
                                        malo_str.clone(),
                                        serde_json::json!({
                                            "malo_id":  malo_str,
                                            "nb_mp_id": rec.nb_mp_id,
                                            "pid":      pid,
                                            "sparte":   if pid == 55005 { "STROM" } else { "GAS" },
                                        }),
                                    )
                                    .with_extensions(EventExtensions {
                                        marktmaloid: Some(malo_str.clone()),
                                        makopid: Some(pid),
                                        ..Default::default()
                                    });
                                    if let Err(e) =
                                        crate::outbox::enqueue(&pool, &gap_evt, &state.notify).await
                                    {
                                        error!(error = %e, "event_ingest: gap-detected enqueue failed");
                                    }
                                }
                                Ok(_) => {}
                                Err(e) => tracing::warn!(
                                    malo_id = %malo_str,
                                    error = %e,
                                    "event_ingest: gap-detection read failed (non-fatal)"
                                ),
                            }
                        }
                    } else if is_completed && matches!(pid, 55013 | 44013) {
                        // Anmeldung/Zuordnung EOG completed — the E/G is now the
                        // supplier of record (GPKE Teil 2 Kap. 2.3, §36/§38 EnWG).
                        let gv_mp_id = data_for_vs
                            .get("new_supplier")
                            .and_then(|v| v.as_str())
                            .map(str::to_owned);
                        let eog_status = match data_for_vs.get("eog_art").and_then(|v| v.as_str()) {
                            Some("GRUNDVERSORGUNG") => {
                                Some(mako_markt::repository::LieferStatus::Grundversorgung)
                            }
                            // Default: §38 Abs. 1 EnWG applies ipso iure.
                            None | Some("ERSATZVERSORGUNG") => {
                                Some(mako_markt::repository::LieferStatus::Ersatzversorgung)
                            }
                            // Vertragliche Ersatzbelieferung (ZE3) and §38a
                            // Übergangsversorgung (ZZD) are contract regimes
                            // outside the statutory fallback states — the
                            // operator records them via the REST upsert.
                            Some(other) => {
                                tracing::warn!(
                                    malo_id = %malo_str,
                                    eog_art = other,
                                    "event_ingest: EoG completion with non-statutory \
                                     Versorgungsart — no automatic status transition"
                                );
                                None
                            }
                        };
                        let eog_seit = data_for_vs
                            .get("process_date")
                            .and_then(|v| v.as_str())
                            .and_then(parse_civil_date);
                        // Resolve the Bilanzkreis: the E/G's own BK from the
                        // completion payload when present, else the NB's
                        // pre-deposited default BK (GPKE Teil 4 „Übermittlung von
                        // Informationen") — consumed when the E/G answered late
                        // (`ohne_antwort`).
                        let bilanzkreis: Option<String> =
                            match data_for_vs.get("bilanzkreis").and_then(|v| v.as_str()) {
                                Some(bk) => Some(bk.to_owned()),
                                None => sqlx::query_scalar::<_, Option<String>>(
                                    r"SELECT default_bilanzkreis FROM grundversorger
                                      WHERE tenant = $1 AND nb_mp_id = $2 AND sparte = $3",
                                )
                                .bind(&state.tenant_gln)
                                .bind(&nb_mp_id)
                                .bind(if pid == 44013 { "GAS" } else { "STROM" })
                                .fetch_optional(&pool)
                                .await
                                .ok()
                                .flatten()
                                .flatten(),
                            };
                        if let (Some(gv), Some(status)) = (gv_mp_id, eog_status) {
                            if let Err(e) = vs
                                .begin_eog_supply(
                                    &malo_id,
                                    &state.tenant_gln,
                                    &gv,
                                    &nb_mp_id,
                                    status,
                                    eog_seit,
                                    process_id,
                                )
                                .await
                            {
                                tracing::warn!(
                                    malo_id = %malo_str,
                                    pid,
                                    error = %e,
                                    "event_ingest: failed to begin_eog_supply"
                                );
                            } else {
                                let eog_evt = MarktEvent::new(
                                    &state.tenant_gln,
                                    mako_events::markt::VERSORGUNG_EOG_BEGONNEN,
                                    malo_str.clone(),
                                    serde_json::json!({
                                        "malo_id":  malo_str,
                                        "gv_mp_id": gv,
                                        "nb_mp_id": nb_mp_id,
                                        "eog_art":  status.to_string(),
                                        "eog_seit": eog_seit.map(|d| d.to_string()),
                                        "bilanzkreis": bilanzkreis,
                                        "haushaltskunde":
                                            data_for_vs.get("haushaltskunde").cloned(),
                                    }),
                                )
                                .with_extensions(EventExtensions {
                                    marktmaloid: Some(malo_str.clone()),
                                    makopid: Some(pid),
                                    ..Default::default()
                                });
                                if let Err(e) =
                                    crate::outbox::enqueue(&pool, &eog_evt, &state.notify).await
                                {
                                    error!(error = %e, "event_ingest: eog-begonnen enqueue failed");
                                }
                            }
                        } else {
                            tracing::warn!(
                                malo_id = %malo_str,
                                pid,
                                "event_ingest: EoG completion without new_supplier — skipped"
                            );
                        }
                    } else if is_completed && matches!(pid, 55003 | 44003) {
                        // Lieferbeginn rejected (Ablehnung Anmeldung — GPKE
                        // 55003 / GeLi Gas 44003): reset the announced future
                        // Lieferant so no consumer acts on a supplier switch
                        // that will not happen. Without it `lf_mp_id_next` was
                        // stale forever.
                        if let Err(e) = vs
                            .clear_lf_next(&malo_id, &state.tenant_gln, process_id)
                            .await
                        {
                            tracing::warn!(
                                malo_id = %malo_str,
                                pid,
                                error = %e,
                                "event_ingest: failed to clear_lf_next"
                            );
                        }
                    }
                }
            }
        }
    }

    // WiM Stammdaten Übermittlung (PIDs 17102–17133) — auto-update ZaehlzeitRegister.
    //
    // When the MSB transmits register definitions via ORDERS 17102–17133, `makod`
    // emits a ProcessCompleted outbox entry carrying `melo_id` + `zaehlwerke`
    // (ZAK+ZE parsed JSON).  We look up the Zähler for the MeLo and upsert all
    // ZaehlzeitRegister + ZaehlzeitSaison records, giving `billingd` and `edmd`
    // accurate TOU information for future reads.
    //
    // Non-fatal: errors are logged but never block the 202 response.
    {
        let is_wim_stammdaten_completed = ce_type_for_vs == mako_events::mako::PROCESS_COMPLETED
            && pid_for_vs.is_some_and(|p| (17102u32..=17133).contains(&p));

        if is_wim_stammdaten_completed {
            let melo_id_str = data_for_vs
                .get("melo_id")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            let zaehlwerke = data_for_vs
                .get("zaehlwerke")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if let Some(melo_str) = melo_id_str
                && !zaehlwerke.is_empty()
            {
                // Look up the Zähler associated with this MeLo.
                match device_repo
                    .list_zaehler_by_melo(&melo_str, &state.tenant_gln)
                    .await
                {
                    Ok(zaehler_list) => {
                        if let Some(zaehler) = zaehler_list.first() {
                            let zaehler_id = zaehler.zaehler_id.clone();
                            upsert_zaehlzeitregister_from_zaehlwerke(
                                &zaehzeit_repo,
                                &zaehler_id,
                                &state.tenant_gln,
                                &zaehlwerke,
                            )
                            .await;
                        } else {
                            tracing::debug!(
                                melo_id = %melo_str,
                                "event_ingest: no Zaehler found for MeLo; \
                                 ZaehlzeitRegister update skipped"
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            melo_id = %melo_str,
                            error = %e,
                            "event_ingest: list_zaehler_by_melo failed (non-fatal)"
                        );
                    }
                }
            }
        }
    }

    StatusCode::ACCEPTED.into_response()
}

// ── ZaehlzeitRegister auto-update (WiM Stammdaten) ───────────────────────────

/// Upsert `ZaehlzeitRegister` + `ZaehlzeitSaison` records from parsed ZAK+ZE
/// JSON objects extracted from WiM ORDERS 17102–17133.
///
/// Called after receiving a `de.mako.process.completed` CloudEvent with
/// `pid` in the 17102–17133 range and a non-empty `zaehlwerke` array.
///
/// Each entry in `zaehlwerke` has the shape produced by
/// `makod::adapters::extract_zak_ze_zaehlwerke`:
/// ```json
/// {
///   "obis_kennzahl": "1-1:1.8.0",
///   "zaehlerauspraegung": "HT",
///   "bezeichnung": "HT Tarif",
///   "saisons": [
///     { "saison": "GESAMT", "tagtypen": [
///       { "tagtyp": "WERKTAG", "wochentage": [1,2,3,4,5],
///         "fenster": [{"von": "07:00","bis":"22:00"},{"von":"22:00","bis":"07:00"}] }
///     ]}
///   ]
/// }
/// ```
///
/// Saison UUIDs are derived deterministically from
/// `(register_id, saison, tagtyp, zeit_von)` so repeated deliveries are
/// idempotent even with the `ON CONFLICT (id)` constraint in `zaehler_saisons`.
async fn upsert_zaehlzeitregister_from_zaehlwerke(
    repo: &Arc<crate::pg::PgZaehlzeitRepository>,
    zaehler_id: &str,
    tenant: &str,
    zaehlwerke: &[serde_json::Value],
) {
    use mako_markt::repository::{
        ZaehlzeitRegisterRecord, ZaehlzeitRepository, ZaehlzeitSaisonRecord,
    };

    let today = time::OffsetDateTime::now_utc().date();

    for zw in zaehlwerke {
        let obis_kennzahl = zw
            .get("obis_kennzahl")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let zaehlerauspraegung = zw
            .get("zaehlerauspraegung")
            .and_then(|v| v.as_str())
            .unwrap_or("EINZEL")
            .to_owned();
        let bezeichnung = zw
            .get("bezeichnung")
            .and_then(|v| v.as_str())
            .unwrap_or(&zaehlerauspraegung)
            .to_owned();

        let reg = ZaehlzeitRegisterRecord {
            id: uuid::Uuid::new_v4(),
            zaehler_id: zaehler_id.to_owned(),
            tenant: tenant.to_owned(),
            bezeichnung: bezeichnung.clone(),
            zaehlerauspraegung: zaehlerauspraegung.clone(),
            obis_kennzahl,
            einheit: "KWH".to_owned(),
            valid_from: today,
            valid_to: None,
            updated_at: time::OffsetDateTime::now_utc(),
        };

        if let Err(e) = repo.upsert_register(&reg).await {
            tracing::warn!(
                zaehler_id,
                bezeichnung = %bezeichnung,
                error = %e,
                "event_ingest: upsert_register failed (non-fatal)"
            );
            continue;
        }

        // Re-read the register to get the stable ID (upsert uses ON CONFLICT,
        // so the server-assigned ID may differ from reg.id).
        let register_id = match repo.list_registers_by_zaehler(zaehler_id, tenant).await {
            Ok(regs) => regs
                .into_iter()
                .find(|r| {
                    r.bezeichnung == bezeichnung
                        && r.zaehlerauspraegung == zaehlerauspraegung
                        && r.valid_from == today
                })
                .map(|r| r.id)
                .unwrap_or(reg.id),
            Err(_) => reg.id,
        };

        // Upsert seasonal TOU windows.
        if let Some(saisons) = zw.get("saisons").and_then(|v| v.as_array()) {
            for saison_val in saisons {
                let saison = saison_val
                    .get("saison")
                    .and_then(|v| v.as_str())
                    .unwrap_or("GESAMT")
                    .to_owned();

                if let Some(tagtypen) = saison_val.get("tagtypen").and_then(|v| v.as_array()) {
                    for tt_val in tagtypen {
                        let tagtyp = tt_val
                            .get("tagtyp")
                            .and_then(|v| v.as_str())
                            .unwrap_or("WERKTAG");
                        let wochentage = tt_val
                            .get("wochentage")
                            .cloned()
                            .unwrap_or(serde_json::json!([1, 2, 3, 4, 5]));

                        if let Some(fenster) = tt_val.get("fenster").and_then(|v| v.as_array()) {
                            for f in fenster {
                                let zeit_von = f
                                    .get("von")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("00:00")
                                    .to_owned();
                                let zeit_bis = f
                                    .get("bis")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("00:00")
                                    .to_owned();

                                // Deterministic UUID so repeated deliveries are idempotent.
                                let saison_id = uuid::Uuid::new_v5(
                                    &uuid::Uuid::NAMESPACE_URL,
                                    format!("zaehlzeit:{register_id}:{saison}:{tagtyp}:{zeit_von}")
                                        .as_bytes(),
                                );

                                let saison_rec = ZaehlzeitSaisonRecord {
                                    id: saison_id,
                                    register_id,
                                    saison: saison.clone(),
                                    wochentage: wochentage.clone(),
                                    zeit_von: zeit_von.clone(),
                                    zeit_bis,
                                    updated_at: time::OffsetDateTime::now_utc(),
                                };

                                if let Err(e) = repo.upsert_saison(&saison_rec).await {
                                    tracing::warn!(
                                        zaehler_id,
                                        %register_id,
                                        saison = %saison,
                                        tagtyp,
                                        zeit_von = %zeit_von,
                                        error = %e,
                                        "event_ingest: upsert_saison failed (non-fatal)"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    tracing::debug!(
        zaehler_id,
        count = zaehlwerke.len(),
        "event_ingest: ZaehlzeitRegister upserted from WiM Stammdaten"
    );
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
/// Parse a civil date from either ISO extended (`YYYY-MM-DD`) or the EDIFACT
/// DTM basic form (`YYYYMMDD`).
fn parse_civil_date(s: &str) -> Option<time::Date> {
    if let Ok(d) = time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT) {
        return Some(d);
    }
    let fmt = time::macros::format_description!("[year][month][day]");
    time::Date::parse(s, &fmt).ok()
}

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

/// Apply a GPKE Teil 4 / GeLi Gas Stammdatenänderung to the typed columns of the
/// target master-data object, dispatching by the `objekt` marker.
///
/// Object-generic counterpart of the MaLo-only path: the workflow tags the
/// `ProcessCompleted` with `objekt` (`MARKTLOKATION` / `MESSLOKATION` /
/// `NETZLOKATION` / `TRANCHE`) and the object's own location id, and we route to
/// the matching `patch_stammdaten`. §14a SR/TR objects carry no grounded generic
/// attributes (source-gated) and fall through to an acknowledged-only log.
///
/// Non-fatal by contract: the CloudEvent is already acknowledged, so every
/// failure or unknown object is logged, never propagated.
#[allow(clippy::too_many_arguments)]
async fn apply_object_stammdaten<Ma, Me, Su, Ci, Pa>(
    state: &AppState<Ma, Me, Su, Ci, Pa>,
    pool: &sqlx::PgPool,
    nelo_repo: &PgNeLoRepository,
    tranche_repo: &PgTrancheRepository,
    tr_repo: &PgTechnischeRessourceRepository,
    sr_repo: &crate::pg::PgSteuerbareRessourceRepository,
    melo_msb_repo: &crate::pg::PgMeloMsbRepository,
    objekt: &str,
    object_id: &str,
    pid: u32,
    aenderungsdatum: Option<&str>,
    patch_val: &serde_json::Value,
) where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    use mako_markt::repository::{
        MaloStammdatenPatch, MeloMsbRepository, MeloStammdatenPatch, NeLoRepository,
        NeloStammdatenPatch, SteuerbareRessourceRepository, SteuerbareRessourceStammdatenPatch,
        TechnischeRessourceRepository, TechnischeRessourceStammdatenPatch, TrancheRepository,
        TrancheStammdatenPatch,
    };

    // Emit a stammdaten-changed CloudEvent after a successful typed patch —
    // durable enqueue to the outbox (best-effort logging: these are secondary
    // events derived from an already-persisted primary ingest).
    let notify: &tokio::sync::Notify = &state.notify;
    let emit = |ce_type: &'static str, is_malo: bool| {
        let evt = MarktEvent::new(
            &state.tenant_gln,
            ce_type,
            object_id.to_owned(),
            serde_json::json!({
                "object_id": object_id,
                "objekt":    objekt,
                "pid":       pid,
                "patch":     patch_val,
            }),
        )
        .with_extensions(EventExtensions {
            marktmaloid: if is_malo {
                Some(object_id.to_owned())
            } else {
                None
            },
            makopid: Some(pid),
            ..Default::default()
        });
        async move {
            if let Err(e) = crate::outbox::enqueue(pool, &evt, notify).await {
                error!(error = %e, ce_type, "event_ingest: stammdaten enqueue failed");
            }
        }
    };

    match objekt {
        // The Paket-ID change is carried on the MaLo (LOC+Z16).
        "MARKTLOKATION" | "PAKET_ID" => {
            let Ok(malo_id) = object_id.parse::<mako_markt::domain::MaloId>() else {
                debug!(
                    object_id,
                    "event_ingest: Stammdatenänderung with invalid MaLo-ID — skipped"
                );
                return;
            };
            let patch: MaloStammdatenPatch =
                serde_json::from_value(patch_val.clone()).unwrap_or_default();
            if patch.is_empty() {
                return;
            }
            match state.malo_repo.patch_stammdaten(&malo_id, &patch).await {
                Ok(true) => emit(mako_events::markt::MALO_STAMMDATEN_GEAENDERT, true).await,
                Ok(false) => {
                    debug!(
                        object_id,
                        pid, "event_ingest: MaLo Stammdatenänderung for unknown MaLo — no-op"
                    )
                }
                Err(e) => {
                    warn!(object_id, pid, error = %e, "event_ingest: MaLo patch_stammdaten failed (non-fatal)")
                }
            }
        }
        "MESSLOKATION" => {
            let Ok(melo_id) = object_id.parse::<mako_markt::domain::MeloId>() else {
                debug!(
                    object_id,
                    "event_ingest: Stammdatenänderung with invalid MeLo-ID — skipped"
                );
                return;
            };
            // The real MeLo Änderungsmeldung payload is the MSB-Zuordnung
            // (zugeordneter Messstellenbetreiber); record it on the dated
            // `melo_msb_zuordnungen` timeline effective the Änderungsdatum.
            if let Some(msb) = patch_val.get("zugeordneter_msb").and_then(|v| v.as_str())
                && let Some(valid_from) = aenderungsdatum.and_then(parse_civil_date)
            {
                match melo_msb_repo
                    .assign_msb(&state.tenant_gln, object_id, msb, valid_from)
                    .await
                {
                    Ok(()) => emit(mako_events::markt::STAMMDATEN_GEAENDERT, false).await,
                    Err(e) => {
                        warn!(object_id, pid, error = %e, "event_ingest: MeLo assign_msb failed (non-fatal)")
                    }
                }
            }
            // Defensive typed-column patch (Netzebene/Regelzone are not carried by
            // the MeLo Änderungsmeldung today, so this is a rarely-firing no-op).
            let patch: MeloStammdatenPatch =
                serde_json::from_value(patch_val.clone()).unwrap_or_default();
            if patch.is_empty() {
                return;
            }
            match state.melo_repo.patch_stammdaten(&melo_id, &patch).await {
                Ok(true) => emit(mako_events::markt::STAMMDATEN_GEAENDERT, false).await,
                Ok(false) => {
                    debug!(
                        object_id,
                        pid, "event_ingest: MeLo Stammdatenänderung for unknown MeLo — no-op"
                    )
                }
                Err(e) => {
                    warn!(object_id, pid, error = %e, "event_ingest: MeLo patch_stammdaten failed (non-fatal)")
                }
            }
        }
        "NETZLOKATION" => {
            let patch: NeloStammdatenPatch =
                serde_json::from_value(patch_val.clone()).unwrap_or_default();
            if patch.is_empty() {
                return;
            }
            match nelo_repo
                .patch_stammdaten(object_id, &state.tenant_gln, &patch)
                .await
            {
                Ok(true) => emit(mako_events::markt::STAMMDATEN_GEAENDERT, false).await,
                Ok(false) => {
                    debug!(
                        object_id,
                        pid, "event_ingest: NeLo Stammdatenänderung for unknown NeLo — no-op"
                    )
                }
                Err(e) => {
                    warn!(object_id, pid, error = %e, "event_ingest: NeLo patch_stammdaten failed (non-fatal)")
                }
            }
        }
        "TRANCHE" => {
            let patch: TrancheStammdatenPatch =
                serde_json::from_value(patch_val.clone()).unwrap_or_default();
            if patch.is_empty() {
                return;
            }
            match tranche_repo
                .patch_stammdaten(object_id, &state.tenant_gln, &patch)
                .await
            {
                Ok(true) => emit(mako_events::markt::STAMMDATEN_GEAENDERT, false).await,
                Ok(false) => {
                    debug!(
                        object_id,
                        pid, "event_ingest: Tranche Stammdatenänderung for unknown Tranche — no-op"
                    )
                }
                Err(e) => {
                    warn!(object_id, pid, error = %e, "event_ingest: Tranche patch_stammdaten failed (non-fatal)")
                }
            }
        }
        "TECHNISCHE_RESSOURCE" => {
            let patch: TechnischeRessourceStammdatenPatch =
                serde_json::from_value(patch_val.clone()).unwrap_or_default();
            if patch.is_empty() {
                return;
            }
            match tr_repo
                .patch_stammdaten(object_id, &state.tenant_gln, &patch)
                .await
            {
                Ok(true) => emit(mako_events::markt::STAMMDATEN_GEAENDERT, false).await,
                Ok(false) => {
                    debug!(
                        object_id,
                        pid, "event_ingest: TR Stammdatenänderung for unknown TR — no-op"
                    )
                }
                Err(e) => {
                    warn!(object_id, pid, error = %e, "event_ingest: TR patch_stammdaten failed (non-fatal)")
                }
            }
        }
        "STEUERBARE_RESSOURCE" => {
            let patch: SteuerbareRessourceStammdatenPatch =
                serde_json::from_value(patch_val.clone()).unwrap_or_default();
            let Some(kp) = patch.konfigurationsprodukte else {
                return;
            };
            match sr_repo
                .replace_sr_konfigurationsprodukte(object_id, &state.tenant_gln, kp)
                .await
            {
                Ok(true) => emit(mako_events::markt::STAMMDATEN_GEAENDERT, false).await,
                Ok(false) => {
                    debug!(
                        object_id,
                        pid, "event_ingest: SR Stammdatenänderung for unknown SR — no-op"
                    )
                }
                Err(e) => {
                    warn!(object_id, pid, error = %e, "event_ingest: SR replace_sr_konfigurationsprodukte failed (non-fatal)")
                }
            }
        }
        // MeLo standorteigenschaften deep attributes still travel in
        // characteristic groups whose per-attribute mapping is gated on the
        // §14a UTILMD AHB (roadmap). Acknowledged without a typed apply.
        other => debug!(
            objekt = other,
            object_id,
            pid,
            "event_ingest: Stammdatenänderung apply for this object is source-gated (§14a AHB) — acknowledged only"
        ),
    }
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
