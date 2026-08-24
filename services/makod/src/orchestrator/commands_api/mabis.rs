//! MaBiS billing command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

pub(super) fn cmd_mabis_abrechnung_einleiten<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_billing_einleiten(s, p))
}

pub(super) fn cmd_mabis_summenzeitreihe_uebermitteln<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_summenzeitreihe_uebermitteln(s, p))
}

pub(super) fn cmd_mabis_abrechnung_daten_einreichen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_billing_daten_einreichen(s, p))
}

pub(super) fn cmd_mabis_abrechnung_begleichen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_billing_begleichen(s, p))
}

/// Dispatch `mabis.abrechnung.einleiten` — BIKO sends Abrechnungssummenzeitreihe;
/// open the billing period from the BKV's perspective (MaBiS BK6-24-174 §13).
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `billing_period` | Yes | Billing period in `"YYYY-MM"` format (e.g. `"2025-09"`) |
/// | `bkv_id` | Yes | BKV MP-ID (13-digit) |
/// | `biko_id` | Yes | BIKO EIC code (16-char) |
/// | `version` | Yes | `"vorlaeufig"` or `"endgueltig"` |
/// | `message_ref` | Yes | MSCONS message reference |
///
/// # Deadline
///
/// Registers a 1-Werktag Prüfmitteilung deadline (BK6-24-174 §13.8) immediately
/// after spawning. The deadline scheduler fires `PruefmitteilungDeadlineExpired`
/// if the BKV does not issue `daten-einreichen` within 1 Werktag.
pub(super) async fn dispatch_mabis_billing_einleiten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let billing_period_str = payload
        .get("billing_period")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"billing_period\" (e.g. \"2025-09\")".into(),
            )
        })?
        .to_owned();
    let bkv_id_str = payload
        .get("bkv_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"bkv_id\" (BKV MP-ID, 13 digits)".into(),
            )
        })?
        .to_owned();
    let biko_id_str = payload
        .get("biko_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload("payload must contain \"biko_id\" (BIKO EIC code)".into())
        })?
        .to_owned();
    let version_str = payload
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"version\" (\"vorlaeufig\" or \"endgueltig\")".into(),
            )
        })?;
    let version = match version_str {
        "vorlaeufig" => BillingVersion::Vorlaeufig,
        "endgueltig" => BillingVersion::Endgueltig,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "\"version\" must be \"vorlaeufig\" or \"endgueltig\", got \"{other}\""
            )));
        }
    };
    let message_ref_str = payload
        .get("message_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let pid = Pruefidentifikator::new(13_003).map_err(DispatchError::InvalidPayload)?;

    // Business key: unique per BKV + billing period combination.
    let business_key = format!("{bkv_id_str}|{billing_period_str}");

    // ── Idempotency guard ─────────────────────────────────────────────────────
    let existing = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, &business_key)
        .await
        .map_err(|e| {
            DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string()))
        })?;
    let already_active: Vec<_> = existing
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == "mabis-billing")
        .collect();
    if let Some(dup) = already_active.into_iter().next() {
        tracing::warn!(
            business_key = %business_key,
            process_id   = %dup.process_id,
            "mabis.abrechnung.einleiten refused: active billing process already registered for this BKV/period",
        );
        return Ok(DispatchOutcome::Dispatched {
            process_id: dup.process_id,
        });
    }

    // ── Spawn process and execute command ─────────────────────────────────────
    let workflow_id = WorkflowId::new("mabis-billing", latest_format_version());
    let process = mako_engine::process::Process::<
        MabisBillingWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();
    let domain_cmd = BillingCommand::ReceiveSummenzeitreihe {
        pid,
        billing_period: BillingPeriod::new(billing_period_str.clone()),
        bkv_id: BkvId::new(bkv_id_str.clone()),
        biko_id: BikoId::new(biko_id_str),
        version,
        message_ref: MessageRef::new(message_ref_str),
    };

    // ── Build 1-Werktag Prüfmitteilung deadline before the atomic write ───────
    //
    // Due at 17:00 Europe/Berlin on the first Werktag following receipt
    // (BK6-24-174 §13.8).  The deadline is passed to
    // `execute_and_enqueue_with_deadlines` so that the event, outbox entry,
    // and the deadline all land in a single SSI transaction (F-009).
    let due_at = mako_fristen::deadline_at_werktage(
        time::OffsetDateTime::now_utc(),
        1,
        mako_fristen::HolidayCalendar::BdewMaKo,
    );
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id.clone(),
        PRUEFMITTEILUNG_DEADLINE_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await
        .map_err(|e| {
            tracing::error!(
                process_id = %process_id,
                error      = %e,
                "MABIS billing: atomic event+deadline write failed — \
                 process not spawned, BKV deadline enforcement inactive",
            );
            DispatchError::Engine(e)
        })?;

    // ── Register process under BKV|billing_period business key ────────────────
    let identity = process.identity();
    if let Err(e) = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, &business_key, process_id, identity)
        .await
    {
        tracing::error!(
            process_id   = %process_id,
            business_key = %business_key,
            error        = %e,
            "MABIS billing: process registry registration failed — \
             follow-up commands (daten-einreichen/begleichen) may not route correctly",
        );
    }

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `mabis.summenzeitreihe.uebermitteln` — NB/ÜNB sends a BG-SZR to the
/// BIKO as MSCONS Prüfidentifikator 13003.
///
/// This is the outbound half of MaBiS, distinct from `mabis.abrechnung.*`, which
/// models the BKV receiving an Abrechnungssummenzeitreihe and answering with a
/// Prüfmitteilung.
///
/// No workflow is spawned. A Summenzeitreihe is a statement of fact, not a
/// request: the BIKO answers asynchronously with a Datenstatus (IFTSTA
/// 21003/21004) or a Prüfmitteilung (21000/21001), and those land back in
/// `mabis-syncd`, which owns the version history. Spawning a process here would
/// create a second place tracking the same state.
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `mabis_zp_id` | Yes | MaBiS-Zählpunkt — SG6 `LOC+172` Meldepunkt (33-char Zählpunktbezeichnung) |
/// | `bilanzierungsgebiet_id` | Yes | Bilanzierungsgebiet EIC — SG6 `LOC+107` |
/// | `balancing_period` | Yes | Bilanzierungsmonat, `CCYYMM` |
/// | `version` | Yes | Versionsangabe, `CCYYMMDDHHMMSSZZZ`, ascending per §3.8.2 |
/// | `receiver_mp_id` | Yes | BIKO code |
/// | `sender_mp_id` | No | defaults to this operator's MP-ID |
/// | `intervals` | Yes | one entry per settlement slot: `from`, `to`, `quantity_kwh` |
pub(super) async fn dispatch_mabis_summenzeitreihe_uebermitteln(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_engine::ids::{ConversationId, CorrelationId, EventId, OutboxMessageId, StreamId};
    use mako_engine::outbox::{OutboxMessage, OutboxStore as _};

    let require = |field: &str| -> Result<String, DispatchError> {
        payload
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| DispatchError::InvalidPayload(format!("\"{field}\" is required")))
    };

    // Two distinct SG6 LOC qualifiers, not one: `172` is the MaBiS-Zählpunkt
    // and `107` the Bilanzierungsgebiet. Both are required so neither can
    // silently stand in for the other on the wire.
    let mabis_zp_id = require("mabis_zp_id")?;
    let bilanzierungsgebiet_id = require("bilanzierungsgebiet_id")?;
    if mabis_zp_id == bilanzierungsgebiet_id {
        return Err(DispatchError::InvalidPayload(
            "\"mabis_zp_id\" and \"bilanzierungsgebiet_id\" identify different things \
             (SG6 LOC+172 vs LOC+107) and must differ"
                .into(),
        ));
    }
    let balancing_period = require("balancing_period")?;
    let version = require("version")?;
    let receiver_mp_id = require("receiver_mp_id")?;

    // An empty Summenzeitreihe would settle a Bilanzierungsgebiet at zero. The
    // BIKO cannot distinguish that from a territory that genuinely drew nothing.
    let intervals = payload
        .get("intervals")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "\"intervals\" must contain at least one settlement slot".into(),
            )
        })?;

    let sender_mp_id = payload
        .get("sender_mp_id")
        .and_then(|v| v.as_str())
        .unwrap_or(state.sender_party_id.as_str())
        .to_owned();

    let causation = EventId::new();
    let message = OutboxMessage {
        message_id: OutboxMessageId::new(),
        stream_id: StreamId::new(format!(
            "mabis-szr|{bilanzierungsgebiet_id}|{balancing_period}"
        )),
        process_id: ProcessId::new(),
        tenant_id: state.tenant_id,
        correlation_id: CorrelationId::new(),
        conversation_id: ConversationId::new(),
        causation_event_id: causation,
        message_type: "MSCONS".into(),
        recipient: receiver_mp_id.as_str().into(),
        payload: serde_json::json!({
            "pid": 13003,
            "sender_mp_id": sender_mp_id,
            "receiver_mp_id": receiver_mp_id,
            "mabis_zp_id": mabis_zp_id,
            "bilanzierungsgebiet_id": bilanzierungsgebiet_id,
            "balancing_period": balancing_period,
            "version": version,
            "intervals": intervals,
        }),
        payload_schema: None,
        created_at: time::OffsetDateTime::now_utc(),
        deliver_after: None,
        attempt_count: 0,
        workflow_name: "".into(),
        trace_context: mako_engine::trace_ctx::current().map(Into::into),
    };
    let process_id = message.process_id;

    state
        .store
        .enqueue(std::slice::from_ref(&message))
        .await
        .map_err(DispatchError::Engine)?;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `mabis.abrechnung.daten-einreichen` — BKV sends Prüfmitteilung.
///
/// The BKV must respond to the Abrechnungssummenzeitreihe within 1 Werktag
/// (BK6-24-174 §13.8) with either a positive or negative Prüfmitteilung.
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `bkv_id` | Yes | BKV MP-ID (same as used in `einleiten`) |
/// | `billing_period` | Yes | Billing period `"YYYY-MM"` (same as used in `einleiten`) |
/// | `message_ref` | No | Message reference for the outbound Prüfmitteilung |
/// | `reject` | No | `true` to send a negative Prüfmitteilung (default: `false`) |
/// | `reason` | Conditional | Dispute reason; required when `reject = true` |
pub(super) async fn dispatch_mabis_billing_daten_einreichen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let bkv_id = payload
        .get("bkv_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"bkv_id\" is required".into()))?;
    let billing_period = payload
        .get("billing_period")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"billing_period\" is required".into()))?;
    let business_key = format!("{bkv_id}|{billing_period}");
    let message_ref = MessageRef::new(
        payload
            .get("message_ref")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    let reject = payload
        .get("reject")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if reject {
        let reason = payload
            .get("reason")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                DispatchError::InvalidPayload(
                    "\"reason\" is required when \"reject\" is true".into(),
                )
            })?
            .to_owned();
        dispatch_to_process::<MabisBillingWorkflow, _>(
            state,
            &business_key,
            "mabis-billing",
            move || BillingCommand::SendPruefmitteilungNegativ {
                message_ref,
                reason,
            },
        )
        .await
    } else {
        dispatch_to_process::<MabisBillingWorkflow, _>(
            state,
            &business_key,
            "mabis-billing",
            move || BillingCommand::SendPruefmitteilungPositiv { message_ref },
        )
        .await
    }
}

/// Dispatch `mabis.abrechnung.begleichen` — BIKO sends Datenstatus; BKV marks settled.
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `bkv_id` | Yes | BKV MP-ID (same as used in `einleiten`) |
/// | `billing_period` | Yes | Billing period `"YYYY-MM"` (same as used in `einleiten`) |
/// | `data_status` | Yes | `"abrechnungsdaten"`, `"abgerechnete_daten"`, or `"abgerechnete_daten_kbka"` |
pub(super) async fn dispatch_mabis_billing_begleichen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let bkv_id = payload
        .get("bkv_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"bkv_id\" is required".into()))?;
    let billing_period = payload
        .get("billing_period")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"billing_period\" is required".into()))?;
    let business_key = format!("{bkv_id}|{billing_period}");
    let code = payload
        .get("data_status")
        .and_then(|v| v.as_str())
        .ok_or_else(|| DispatchError::InvalidPayload("\"data_status\" is required".into()))?;
    let data_status = match code {
        "abrechnungsdaten" => DataStatus::Abrechnungsdaten,
        "abgerechnete_daten" => DataStatus::AbgerechtneteDaten,
        "abgerechnete_daten_kbka" => DataStatus::AbgerechtneteDatenKbka,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "unknown data_status \"{other}\"; valid values: \
                 abrechnungsdaten, abgerechnete_daten, abgerechnete_daten_kbka"
            )));
        }
    };
    dispatch_to_process::<MabisBillingWorkflow, _>(
        state,
        &business_key,
        "mabis-billing",
        move || BillingCommand::ReceiveDatastatus { data_status },
    )
    .await
}
