//! Inbound makod CloudEvents handler.
//!
//! Route: `POST /api/v1/mako/events`
//!
//! Receives CloudEvents 1.0 payloads from `makod`'s outbound webhook channel,
//! verifies the Standard Webhooks signature and timestamp, deduplicates via the
//! `processed_events` table, and emits the event onto the internal MPSC channel
//! for the fan-out worker.
//!
//! # Architecture
//!
//! `marktd` is a **pure data hub** — it does not make Anmeldung decisions.
//! Automated STP decisions (NB role) are handled by `processd` via the fan-out
//! subscription.  `marktd` simply:
//!
//! 1. Verifies the HMAC signature
//! 2. Deduplicates via `processed_events`
//! 3. Enriches the event with `marktrole` and emits to all subscribers
//! 4. Derives `VersorgungsStatus` for the Anmeldung PIDs 55001/55077/44001
//!    (announce), their confirmations 55002/55078/44002, their rejections
//!    55003/55080/44003 (clear the announcement), 55005/44005 (end + gap
//!    detection) and 55013/44013 (begin Ersatz-/Grundversorgung)
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
        SubscriptionRepository,
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
/// Signature headers: `webhook-id`, `webhook-timestamp`, `webhook-signature`.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_event<Ma, Me, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Su, Ci, Pa>>>,
    Extension(secret): Extension<InboundWebhookSecret>,
    Extension(pool): Extension<PgPool>,
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
    // 1. Verify the inbound signature through the shared verifier, which also
    //    refuses a stale `webhook-timestamp` — this is the ingest every other
    //    service's events arrive on, so a replay here re-enters the fan-out.
    if let Err(err) = mako_service::webhook::verify_request(
        secret.0.as_deref().map(str::as_bytes),
        &headers,
        &body,
    ) {
        warn!(%err, "event_ingest: refused");
        return StatusCode::from(err).into_response();
    }
    if secret.0.is_none() {
        warn!("event_ingest: no inbound secret configured — accepting unverified (dev mode)");
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

    // 3. One transaction for the whole ingest: idempotency marker, business
    //    derivation and the durable fan-out enqueue commit together or not at
    //    all.  A DB error must surface as 5xx so makod's durable webhook channel
    //    redelivers — only a genuine unique violation means "already processed".
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            error!(event_id = %event.id, error = %e, "event_ingest: begin failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    match sqlx::query("INSERT INTO processed_events (event_id) VALUES ($1)")
        .bind(&event.id)
        .execute(&mut *tx)
        .await
    {
        Ok(_) => {}
        Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
            debug!(event_id = %event.id, "event_ingest: duplicate, skipping");
            return StatusCode::ACCEPTED.into_response();
        }
        Err(e) => {
            error!(event_id = %event.id, error = %e, "event_ingest: idempotency insert failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // 4. Re-emit as MarktEvent enriched with the tenant GLN as source.
    //
    // Phase 1 — capture values needed for VersorgungsStatus derivation before
    // event fields are moved into MarktEvent.
    let ce_type_for_vs = event.ce_type.clone();
    let event_id_for_vs = event.id.clone();
    let pid_for_vs = event.makopid;
    let data_for_vs = event.data.clone();
    // The makod process UUID is the CloudEvent *subject*, not its `id`: the id
    // names a delivery envelope, `last_process_id` names the market process.
    let process_id_for_vs = event.process_id();

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

    // 5. Derive VersorgungsStatus from supply-state-changing CloudEvents.
    //
    // Event → action mapping (GPKE BK6-24-174 + GeLi Gas 3.0 (BK7-24-01-009)):
    //
    //   process.initiated  + PID 55001/55077/44001
    //     → announce_lf_next: set lf_mp_id_next + lf_next_lieferbeginn
    //       (NB side: new_supplier + process_date from ProcessInitiated payload).
    //       The *first* announcement wins — see announce_lf_next_tx.
    //
    //   process.completed  + PID 55002/55078/44002 (Bestätigung Anmeldung)
    //     → confirm_supply: promote lf_mp_id_next → lf_mp_id (atomic SQL)
    //
    //   process.completed  + PID 55003/55080/44003 (Ablehnung Anmeldung)
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
    // Every failure rolls the whole transaction back — including the idempotency
    // marker — so makod redelivers instead of leaving the projection behind.
    // The WiM MSB Zuordnung is keyed on the MeLo, so it runs beside the MaLo
    // supply-state derivation rather than inside it — on the same transaction,
    // so it commits with the idempotency marker or not at all.
    if let Err(e) = derive_msb_zuordnung(
        &mut tx,
        &state.tenant_gln,
        &ce_type_for_vs,
        pid_for_vs,
        &data_for_vs,
    )
    .await
    {
        error!(
            event_id = %event_id_for_vs,
            error = %e,
            "event_ingest: MSB-Zuordnung derivation failed — rolling back"
        );
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    let derived = match derive_supply_state(
        &mut tx,
        &state.tenant_gln,
        &ce_type_for_vs,
        pid_for_vs,
        &data_for_vs,
        process_id_for_vs,
    )
    .await
    {
        Ok(evts) => evts,
        Err(e) => {
            error!(
                event_id = %event_id_for_vs,
                error = %e,
                "event_ingest: supply-state derivation failed — rolling back"
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // 6. Durable, persist-before-fan-out: the relayed event and every derived
    //    event are enqueued on the same transaction as the idempotency marker,
    //    so a crash can never leave the marker committed without the events.
    for evt in std::iter::once(&markt_event).chain(derived.iter()) {
        if let Err(e) = crate::outbox::enqueue(&mut *tx, evt, &state.notify).await {
            error!(event_id = %event_id_for_vs, error = %e, "event_ingest: durable enqueue failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        error!(event_id = %event_id_for_vs, error = %e, "event_ingest: commit failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // 7. Secondary applies on other master-data objects. These run after the
    //    commit and stay best-effort by contract — the object repositories own
    //    their own pools and cannot join the ingest transaction.
    //
    // GPKE Teil 4 / GeLi Gas Stammdatenänderung apply — object-generic. The
    // workflow tags the ProcessCompleted with the `objekt` marker; we route it to
    // the matching typed-column patch_stammdaten. Non-MaLo object IDs (MeLo
    // DE+31, NeLo EIC, Tranche) are not valid MaLo-IDs, so this runs off the raw
    // `malo_id` string.
    if ce_type_for_vs == mako_events::mako::PROCESS_COMPLETED
        && let Some(pid) = pid_for_vs
        && let Some(object_id) = data_for_vs.get("malo_id").and_then(|v| v.as_str())
        && let Some(patch_val) = data_for_vs.get("stammdaten_patch")
    {
        let objekt = data_for_vs
            .get("objekt")
            .and_then(|v| v.as_str())
            .unwrap_or("MARKTLOKATION");
        let aenderungsdatum = data_for_vs.get("aenderungsdatum").and_then(|v| v.as_str());
        apply_object_stammdaten(
            &state,
            &pool,
            nelo_repo.as_ref(),
            tranche_repo.as_ref(),
            tr_repo.as_ref(),
            sr_repo.as_ref(),
            melo_msb_repo.as_ref(),
            objekt,
            object_id,
            pid,
            aenderungsdatum,
            patch_val,
        )
        .await;
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

// ── VersorgungsStatus derivation ─────────────────────────────────────────────

/// Inbound Anmeldung PIDs that announce a future Lieferant.
///
/// **55077** „Anmeldung erz. MaLo" is the erzeugende-Marktlokation twin of
/// 55001 (Anwendungsübersicht 4.0 lfd. Nr. 20080, LFN → NB) and drives the
/// identical projection — an EEG-/KWKG-MaLo's supplier change is a supplier
/// change.
/// IFTSTA 21012 — the NB's „Statusmeldung (erfolgreich)".
///
/// The one message that makes a WiM MSB-Wechsel constitutive. WiM Strom Teil 1
/// Kap. 2.1.1: „Der NB ordnet den MSBN/gMSB der Messlokation … zu dem Tag des
/// vom MSBN/gMSB mitgeteilten Termins des erfolgreichen Abschlusses des
/// Gesamtvorgangs … mit dem Zeitpunkt 00:00 Uhr zu. Die Zuordnung des MSBA
/// endet entsprechend zu diesem Zeitpunkt."
///
/// The Anmeldebestätigung 55043 is explicitly *vorläufig*, so deriving the
/// assignment from it would move the Messlokation up to nine Werktage early —
/// and would move it at all in the case where the Gesamtvorgang later fails,
/// which the Festlegung answers by leaving the MSBA in place.
const ZUORDNUNG_ERFOLG_PID: u32 = 21_012;

/// Apply a completed WiM Zuordnung to the per-Messlokation MSB timeline.
///
/// Keyed on the **MeLo**: the Messstellenbetrieb is assigned per Messlokation,
/// while [`derive_supply_state`] works on the MaLo and returns early without
/// one. `valid_from` is the Zuordnungsbeginn the MSBN reported and the NB
/// confirmed — not the date the message arrived, and not the vorläufig
/// bestätigter Zuordnungsbeginn the Anmeldung asked for.
///
/// # Errors
///
/// Propagates any SQL failure so the whole ingest transaction rolls back and
/// `makod` redelivers.
pub async fn derive_msb_zuordnung(
    conn: &mut sqlx::PgConnection,
    tenant_gln: &str,
    ce_type: &str,
    pid: Option<u32>,
    data: &serde_json::Value,
) -> Result<(), mako_markt::error::MdmError> {
    if ce_type != mako_events::mako::PROCESS_COMPLETED || pid != Some(ZUORDNUNG_ERFOLG_PID) {
        return Ok(());
    }
    let (Some(melo_id), Some(msb_mp_id)) = (
        data.get("melo_id").and_then(|v| v.as_str()),
        data.get("msb_mp_id")
            .or_else(|| data.get("new_msb"))
            .and_then(|v| v.as_str()),
    ) else {
        warn!(
            "event_ingest: IFTSTA {ZUORDNUNG_ERFOLG_PID} without melo_id/msb_mp_id — \
             no Zuordnung applied"
        );
        return Ok(());
    };
    let Some(valid_from) = data
        .get("zuordnungsbeginn")
        .and_then(|v| v.as_str())
        .and_then(parse_civil_date)
    else {
        // Without the date there is no day to assign from, and picking today
        // would silently disagree with the market by up to nine Werktage.
        warn!(
            %melo_id,
            "event_ingest: IFTSTA {ZUORDNUNG_ERFOLG_PID} without zuordnungsbeginn — \
             no Zuordnung applied"
        );
        return Ok(());
    };
    // `melo_msb_zuordnungen.melo_id` is a foreign key. An unknown Messlokation
    // would abort the whole ingest transaction, and `makod` would redeliver the
    // same event forever — so the missing master data is logged and the event
    // is acked instead of poisoning the queue.
    let melo_known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM melo WHERE melo_id = $1)")
            .bind(melo_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| mako_markt::error::MdmError::Internal(e.to_string()))?;
    if !melo_known {
        warn!(
            %melo_id, %msb_mp_id,
            "event_ingest: IFTSTA {ZUORDNUNG_ERFOLG_PID} for an unknown Messlokation — \
             no Zuordnung applied; import the MeLo and replay"
        );
        return Ok(());
    }

    debug!(%melo_id, %msb_mp_id, %valid_from, "event_ingest: applying WiM MSB-Zuordnung");
    crate::pg::PgMeloMsbRepository::assign_msb_tx(conn, tenant_gln, melo_id, msb_mp_id, valid_from)
        .await
}

const ANMELDUNG_PIDS: &[u32] = &[55_001, 55_077, 44_001];

/// Outbound answers that confirm an Anmeldung — 55002 for 55001, **55078** for
/// 55077, 44002 for 44001.
const ANMELDUNG_BESTAETIGT_PIDS: &[u32] = &[55_002, 55_078, 44_002];

/// Outbound answers that reject one — 55003 for 55001, **55080** for 55077
/// (55079 is unassigned), 44003 for 44001.
const ANMELDUNG_ABGELEHNT_PIDS: &[u32] = &[55_003, 55_080, 44_003];

/// Apply the supply-state transition an inbound makod CloudEvent implies, on the
/// caller's transaction.
///
/// Returns the `MarktEvent`s the transition produces; the caller enqueues them
/// on the same transaction. Every DB error propagates — the projection that
/// processd's automated LFA answers read must never silently diverge from the
/// acknowledged event.
///
/// Every transition emits `de.markt.versorgung.changed` carrying the resulting
/// state, plus the specific trigger event where one exists
/// (`versorgung.gap-detected`, `versorgung.eog-begonnen`). Subscribers track the
/// supply lifecycle from that one event type, so no transition may be silent.
///
/// # Errors
///
/// Any DB failure; the caller must roll the ingest transaction back.
pub async fn derive_supply_state(
    conn: &mut sqlx::PgConnection,
    tenant_gln: &str,
    ce_type: &str,
    pid: Option<u32>,
    data: &serde_json::Value,
    process_id: Option<uuid::Uuid>,
) -> Result<Vec<MarktEvent>, mako_markt::error::MdmError> {
    use crate::pg::{PgMaloRepository, PgVersorgungsStatusRepository as Vs};

    let is_initiated = ce_type == mako_events::mako::PROCESS_INITIATED;
    let is_completed = ce_type == mako_events::mako::PROCESS_COMPLETED;

    let (Some(pid), Some(malo_str)) = (pid, data.get("malo_id").and_then(|v| v.as_str())) else {
        return Ok(Vec::new());
    };
    // Non-MaLo objects (MeLo, NeLo, Tranche) carry no supply state.
    let Ok(malo_id) = malo_str.parse::<mako_markt::domain::MaloId>() else {
        return Ok(Vec::new());
    };
    let nb_mp_id = data
        .get("nb_mp_id")
        .or_else(|| data.get("grid_operator"))
        .and_then(|v| v.as_str())
        .map_or_else(|| tenant_gln.to_owned(), str::to_owned);
    // The Sparte is carried by the PID itself: GPKE Strom processes are 55xxx,
    // the GeLi Gas twins 44xxx. Subscribers filter on it and processd keys its
    // EoG case log on it, so it goes on every event this function emits.
    let sparte = sparte_of_pid(pid);
    let mut events = Vec::new();

    // Set by every branch that actually wrote a new state, so the
    // `versorgung.changed` announcement below is emitted exactly when one happened.
    let mut transitioned = false;

    if is_initiated && ANMELDUNG_PIDS.contains(&pid) {
        // NB received Lieferbeginn Anfrage — record the pending transition.
        if let Some(lf_mp_id_next) = data.get("new_supplier").and_then(|v| v.as_str()) {
            let lf_next_lieferbeginn = data
                .get("process_date")
                .and_then(|v| v.as_str())
                .and_then(parse_civil_date);
            transitioned = Vs::announce_lf_next_tx(
                conn,
                &malo_id,
                tenant_gln,
                lf_mp_id_next,
                lf_next_lieferbeginn,
                &nb_mp_id,
                process_id,
            )
            .await?;
            if !transitioned {
                // A different supplier already holds the announcement. Keeping
                // it is what lets `mako-pruefung` reject the second Anmeldung
                // with A06 „Andere Anmeldung in Bearbeitung"; overwriting it
                // made that check compare the new Anmeldung against itself.
                warn!(
                    malo_id = %malo_str, pid, lf_mp_id_next,
                    "event_ingest: competing Anmeldung — the pending announcement is kept"
                );
            }
        }

        // L1/N1: patch malo.bilanzierungsmethode + malo.fallgruppe from the
        // ProcessInitiated payload — populated by the makod GPKE/GeLi Gas
        // adapter from UTILMD TM+EM / TM+Z10 segments.
        let bilanzierungsmethode = data.get("bilanzierungsmethode").and_then(|v| v.as_str());
        let fallgruppe = data.get("fallgruppe").and_then(|v| v.as_str());
        PgMaloRepository::patch_typenmerkmal_tx(conn, &malo_id, bilanzierungsmethode, fallgruppe)
            .await?;
    } else if is_completed && ANMELDUNG_BESTAETIGT_PIDS.contains(&pid) {
        // Bestätigung Anmeldung — promote the announced LF to active.
        transitioned = Vs::confirm_supply_tx(conn, &malo_id, tenant_gln, process_id).await?;
    } else if is_completed && matches!(pid, 55005 | 44005) {
        // Bestätigung Lieferende — active LF removed; the pending transition is
        // preserved. The Lieferende is the *contractual* end date carried by the
        // process, not the day the confirmation happened to be ingested; billing
        // period boundaries and the EoG Zuordnungsbeginn are both derived from it.
        let lieferende = data
            .get("process_date")
            .and_then(|v| v.as_str())
            .and_then(parse_civil_date);
        Vs::end_supply_tx(
            conn, &malo_id, tenant_gln, &nb_mp_id, lieferende, process_id,
        )
        .await?;
        transitioned = true;

        let row: Option<(
            Option<String>,
            Option<time::Date>,
            Option<time::Date>,
            String,
        )> = sqlx::query_as(
            "SELECT lf_mp_id_next, lf_next_lieferbeginn, lieferende, nb_mp_id
                   FROM versorgungsstatus
                  WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(&malo_id)
        .bind(tenant_gln)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| mako_markt::error::MdmError::Internal(e.to_string()))?;

        // A supply gap is an uncovered *interval*, not merely "no successor at
        // all": an announced Lieferbeginn later than the day after the Lieferende
        // leaves the MaLo unversorgt in between, and §38 Abs. 1 EnWG attaches to
        // that interval exactly as it does to an open-ended gap.
        let gap = row.as_ref().and_then(|(next, next_beginn, ende, nb)| {
            let ende = (*ende)?;
            let gap_from = ende.next_day()?;
            match (next, next_beginn) {
                (None, _) => Some((gap_from, None, nb.clone())),
                (Some(_), Some(beginn)) if *beginn > gap_from => {
                    Some((gap_from, Some(*beginn), nb.clone()))
                }
                // A successor announced without a date cannot be shown to leave a
                // gap; the 55002 confirmation settles it either way.
                (Some(_), _) => None,
            }
        });

        if let Some((gap_from, gap_until, row_nb_mp_id)) = gap {
            events.push(
                MarktEvent::new(
                    tenant_gln,
                    mako_events::markt::VERSORGUNG_GAP_DETECTED,
                    malo_str.to_owned(),
                    serde_json::json!({
                        "malo_id":   malo_str,
                        "nb_mp_id":  row_nb_mp_id,
                        "pid":       pid,
                        "sparte":    sparte,
                        // The uncovered interval. `gap_until` is null for an
                        // open-ended gap (no successor announced at all).
                        "gap_from":  gap_from.to_string(),
                        "gap_until": gap_until.map(|d| d.to_string()),
                    }),
                )
                .with_extensions(EventExtensions {
                    marktmaloid: Some(malo_str.to_owned()),
                    makopid: Some(pid),
                    marktsparte: Some(sparte.to_owned()),
                    ..Default::default()
                }),
            );
        }
    } else if is_completed && matches!(pid, 55013 | 44013) {
        // Anmeldung/Zuordnung EOG completed — the E/G is now the supplier of
        // record (GPKE Teil 2 Kap. 2.3, §36/§38 EnWG).
        let gv_mp_id = data.get("new_supplier").and_then(|v| v.as_str());
        let eog_status = match data.get("eog_art").and_then(|v| v.as_str()) {
            Some("GRUNDVERSORGUNG") => Some(mako_markt::repository::LieferStatus::Grundversorgung),
            // Default: §38 Abs. 1 EnWG applies ipso iure.
            None | Some("ERSATZVERSORGUNG") => {
                Some(mako_markt::repository::LieferStatus::Ersatzversorgung)
            }
            // Vertragliche Ersatzbelieferung (ZE3) and §38a Übergangsversorgung
            // (ZZD) are contract regimes outside the statutory fallback states —
            // the operator records them via the REST upsert.
            Some(other) => {
                warn!(
                    malo_id = %malo_str,
                    eog_art = other,
                    "event_ingest: EoG completion with non-statutory \
                     Versorgungsart — no automatic status transition"
                );
                None
            }
        };
        let eog_seit = data
            .get("process_date")
            .and_then(|v| v.as_str())
            .and_then(parse_civil_date);

        let (Some(gv), Some(status)) = (gv_mp_id, eog_status) else {
            warn!(
                malo_id = %malo_str,
                pid,
                "event_ingest: EoG completion without new_supplier — skipped"
            );
            return Ok(events);
        };

        // Resolve the Bilanzkreis: the E/G's own BK from the completion payload
        // when present, else the NB's pre-deposited default BK (GPKE Teil 4
        // „Übermittlung von Informationen") — consumed when the E/G answered
        // late (`ohne_antwort`). A lookup error is fatal: emitting
        // `bilanzkreis: null` would be indistinguishable from "none deposited".
        let bilanzkreis: Option<String> = match data.get("bilanzkreis").and_then(|v| v.as_str()) {
            Some(bk) => Some(bk.to_owned()),
            None => sqlx::query_scalar::<_, Option<String>>(
                r"SELECT default_bilanzkreis FROM grundversorger
                  WHERE tenant = $1 AND nb_mp_id = $2 AND sparte = $3",
            )
            .bind(tenant_gln)
            .bind(&nb_mp_id)
            .bind(sparte)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| mako_markt::error::MdmError::Internal(e.to_string()))?
            .flatten(),
        };

        Vs::begin_eog_supply_tx(
            conn, &malo_id, tenant_gln, gv, &nb_mp_id, status, eog_seit, process_id,
        )
        .await?;

        transitioned = true;
        events.push(
            MarktEvent::new(
                tenant_gln,
                mako_events::markt::VERSORGUNG_EOG_BEGONNEN,
                malo_str.to_owned(),
                serde_json::json!({
                    "malo_id":     malo_str,
                    "gv_mp_id":    gv,
                    "nb_mp_id":    nb_mp_id,
                    // processd keys its EoG case log on this.
                    "sparte":      sparte,
                    "eog_art":     status.to_string(),
                    "eog_seit":    eog_seit.map(|d| d.to_string()),
                    "bilanzkreis": bilanzkreis,
                    "haushaltskunde": data.get("haushaltskunde").cloned(),
                }),
            )
            .with_extensions(EventExtensions {
                marktmaloid: Some(malo_str.to_owned()),
                makopid: Some(pid),
                marktsparte: Some(sparte.to_owned()),
                ..Default::default()
            }),
        );
    } else if is_completed && ANMELDUNG_ABGELEHNT_PIDS.contains(&pid) {
        // Ablehnung Anmeldung: reset the announced future Lieferant so no
        // consumer acts on a switch that will not happen — and so the next
        // supplier's Anmeldung is not rejected against a stale announcement.
        transitioned = Vs::clear_lf_next_tx(conn, &malo_id, tenant_gln, process_id).await?;
    }

    if transitioned {
        events.push(versorgung_changed(conn, tenant_gln, &malo_id, malo_str, sparte, pid).await?);
    }

    Ok(events)
}

/// The Sparte a GPKE / GeLi Gas Prüfidentifikator belongs to.
///
/// The two process families are numbered disjointly — GPKE Strom in the 55xxx
/// band, its GeLi Gas twin in 44xxx — so the PID alone decides it.
const fn sparte_of_pid(pid: u32) -> &'static str {
    if pid >= 44_000 && pid < 45_000 {
        "GAS"
    } else {
        "STROM"
    }
}

/// Build `de.markt.versorgung.changed` from the state the transition left behind.
///
/// Read back rather than reconstructed: the transitions are `ON CONFLICT` upserts
/// whose result depends on the prior row, so only the row itself describes the
/// state that actually resulted.
async fn versorgung_changed(
    conn: &mut sqlx::PgConnection,
    tenant_gln: &str,
    malo_id: &mako_markt::domain::MaloId,
    malo_str: &str,
    sparte: &str,
    pid: u32,
) -> Result<MarktEvent, mako_markt::error::MdmError> {
    #[allow(clippy::type_complexity)]
    let row: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<time::Date>,
        Option<time::Date>,
        Option<time::Date>,
        Option<time::Date>,
        i64,
    )> = sqlx::query_as(
        "SELECT lieferstatus, lf_mp_id, lf_mp_id_next, lf_next_lieferbeginn,
                lieferbeginn, lieferende, eog_seit, version
           FROM versorgungsstatus
          WHERE malo_id = $1 AND tenant = $2",
    )
    .bind(malo_id)
    .bind(tenant_gln)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| mako_markt::error::MdmError::Internal(e.to_string()))?;

    let data = row.map_or_else(
        || serde_json::json!({ "malo_id": malo_str, "sparte": sparte, "pid": pid }),
        |(
            lieferstatus,
            lf_mp_id,
            lf_mp_id_next,
            lf_next_lieferbeginn,
            lieferbeginn,
            lieferende,
            eog_seit,
            version,
        )| {
            serde_json::json!({
                "malo_id":              malo_str,
                "sparte":               sparte,
                "pid":                  pid,
                "lieferstatus":         lieferstatus,
                "lf_mp_id":             lf_mp_id,
                "lf_mp_id_next":        lf_mp_id_next,
                "lf_next_lieferbeginn": lf_next_lieferbeginn.map(|d| d.to_string()),
                "lieferbeginn":         lieferbeginn.map(|d| d.to_string()),
                "lieferende":           lieferende.map(|d| d.to_string()),
                "eog_seit":             eog_seit.map(|d| d.to_string()),
                "version":              version,
            })
        },
    );

    Ok(MarktEvent::new(
        tenant_gln,
        mako_events::markt::VERSORGUNG_CHANGED,
        malo_str.to_owned(),
        data,
    )
    .with_extensions(EventExtensions {
        marktmaloid: Some(malo_str.to_owned()),
        makopid: Some(pid),
        marktsparte: Some(sparte.to_owned()),
        ..Default::default()
    }))
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

    let today = crate::handlers::malo::today_berlin();

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
                        // ISO weekdays; anything outside 1..=7 is dropped
                        // rather than stored, since the column rejects it and a
                        // failed upsert would lose the whole window.
                        let wochentage: Vec<i16> = tt_val
                            .get("wochentage")
                            .and_then(serde_json::Value::as_array)
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(serde_json::Value::as_i64)
                                    .filter_map(|d| i16::try_from(d).ok())
                                    .filter(|d| (1..=7).contains(d))
                                    .collect()
                            })
                            .filter(|v: &Vec<i16>| !v.is_empty())
                            .unwrap_or_else(|| vec![1, 2, 3, 4, 5]);

                        if let Some(fenster) = tt_val.get("fenster").and_then(|v| v.as_array()) {
                            for f in fenster {
                                let Some(zeit_von) = f
                                    .get("von")
                                    .and_then(serde_json::Value::as_str)
                                    .and_then(parse_hhmm)
                                else {
                                    tracing::warn!(
                                        zaehler_id,
                                        %register_id,
                                        "event_ingest: ZaehlzeitSaison window has no parsable `von` — skipped"
                                    );
                                    continue;
                                };
                                let Some(zeit_bis) = f
                                    .get("bis")
                                    .and_then(serde_json::Value::as_str)
                                    .and_then(parse_hhmm)
                                else {
                                    tracing::warn!(
                                        zaehler_id,
                                        %register_id,
                                        "event_ingest: ZaehlzeitSaison window has no parsable `bis` — skipped"
                                    );
                                    continue;
                                };
                                // A zero-length or inverted window classifies no
                                // reading at all; the table refuses it, so drop
                                // it here with a reason rather than as a
                                // constraint violation deep in the upsert.
                                if zeit_von >= zeit_bis {
                                    tracing::warn!(
                                        zaehler_id,
                                        %register_id,
                                        von = %zeit_von,
                                        bis = %zeit_bis,
                                        "event_ingest: ZaehlzeitSaison window is empty or inverted — skipped"
                                    );
                                    continue;
                                }

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
                                    zeit_von,
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

/// Parse a `HH:MM` (or `HH:MM:SS`) wall-clock string from a market message.
///
/// Returns `None` rather than a default: a window that silently became
/// `00:00–00:00` classified no reading at all, and the caller can only report
/// that if it is told the value was unusable.
fn parse_hhmm(raw: &str) -> Option<time::Time> {
    let raw = raw.trim();
    let mut parts = raw.split(':');
    let h: u8 = parts.next()?.parse().ok()?;
    let m: u8 = parts.next()?.parse().ok()?;
    let sec: u8 = match parts.next() {
        Some(s) => s.parse().ok()?,
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    time::Time::from_hms(h, m, sec).ok()
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
