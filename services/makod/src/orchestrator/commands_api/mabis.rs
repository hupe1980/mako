//! MaBiS billing command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;

use mako_mabis::{ListenabgleichCommand, MabisListenabgleichWorkflow};

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

/// Dispatch `mabis.abrechnung.einleiten` — a version of a Summenzeitreihe
/// arrived; open or extend its settlement (BK6-24-174 Anlage 3 Kap. 3.8).
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `zeitreihe` | Yes | `SG10 CAV` DE 7111 code — `Z95`…`ZA6` (see `mako_mabis::zeitreihe_aus_cav`) |
/// | `mabis_zp_id` | Yes | MaBiS-Zählpunkt, 33 characters |
/// | `bilanzierungsmonat` | Yes | `"YYYY-MM"` |
/// | `version` | Yes | Erstellungszeitpunkt, `CCYYMMDDHHMMSSZZZ` (`RFF+AUU`) |
/// | `biko_id` | Yes | BIKO EIC |
/// | `absender_mp_id` | Yes | party that sent the Summenzeitreihe |
/// | `pid` | No | MSCONS PID; defaults to 13003 |
/// | `message_ref` | No | MSCONS message reference |
///
/// # No deadline is registered
///
/// The Prüfmitteilung has **no Frist**: Kap. 9.8.2 Nr. 1 leaves the cell empty
/// and says the receiving party „kann" answer, and Kap. 13.8.2 — which the
/// previous 1-Werktag deadline cited — defines no answer at all; its two rows
/// are the BIKO's own dispatch dates. What bounds a Prüfmitteilung is the
/// clearing window of Tabelle 2, and that is a date range anchored on the
/// Bilanzierungsmonat rather than a countdown from this arrival. The window is
/// derived here so the caller sees it, and a version that arrives after it is
/// refused rather than accepted into a settlement it can no longer change.
pub(super) async fn dispatch_mabis_billing_einleiten(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let require = |field: &str| -> Result<String, DispatchError> {
        payload
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| DispatchError::InvalidPayload(format!("\"{field}\" is required")))
    };

    let cav = require("zeitreihe")?;
    let (zeitreihe, _ebene) = mako_mabis::zeitreihe_aus_cav(&cav).ok_or_else(|| {
        DispatchError::InvalidPayload(format!(
            "\"zeitreihe\" \"{cav}\" is not an SG10 CAV Summenzeitreihen code \
                 (Z95…ZA6, UTILMD AHB Strom 2.2 Kap. 13.1)"
        ))
    })?;
    let mabis_zp_id = require("mabis_zp_id")?;
    let mabis_zp = mako_mabis::MabisZaehlpunktId::new(&mabis_zp_id)
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
    let bilanzierungsmonat_str = require("bilanzierungsmonat")?;
    let version = mako_mabis::SzrVersion::new(require("version")?)
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;
    let biko_id_str = require("biko_id")?;
    let absender_str = require("absender_mp_id")?;
    let message_ref_str = payload
        .get("message_ref")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let pid_code = payload
        .get("pid")
        .and_then(|v| v.as_u64())
        .map_or(mako_mabis::SUMMENZEITREIHE_PID, |n| {
            u32::try_from(n).unwrap_or(u32::MAX)
        });
    if !mako_mabis::ist_zeitreihen_pid(pid_code) {
        return Err(DispatchError::InvalidPayload(format!(
            "pid {pid_code} carries no MaBiS Summenzeitreihe"
        )));
    }
    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;

    // ── Which phase the arrival falls in (Tabelle 2, Kap. 3.10) ──────────────
    //
    // This is what decides the Datenstatus the BIKO will assign, so it is
    // derived from the calendar rather than taken from the payload.
    let monat = parse_bilanzierungsmonat(&bilanzierungsmonat_str)?;
    let heute = time::OffsetDateTime::now_utc().date();
    let phase = monat.phase(zeitreihe, heute);
    if !phase.nimmt_versionen_an() {
        return Err(DispatchError::InvalidPayload(format!(
            "das Abrechnungsfenster für {zeitreihe} im Bilanzierungsmonat \
             {bilanzierungsmonat_str} nimmt am {heute} keine Version mehr an (Phase {phase:?}, \
             BK6-24-174 Anlage 3 Kap. 3.10)"
        )));
    }

    // Business key: one settlement per MaBiS-Zählpunkt and Bilanzierungsmonat.
    let business_key = format!("{mabis_zp_id}|{bilanzierungsmonat_str}");

    let domain_cmd = BillingCommand::ReceiveSummenzeitreihe {
        pid,
        zeitreihe,
        mabis_zp,
        bilanzierungsmonat: BillingPeriod::new(bilanzierungsmonat_str.clone()),
        version,
        im_erstaufschlag: phase.ist_erstaufschlag(),
        absender: MarktpartnerCode::new(absender_str),
        biko_id: BikoId::new(biko_id_str),
        message_ref: MessageRef::new(message_ref_str),
    };

    // A settlement accumulates versions, so a second one **resumes** the
    // existing process rather than being refused as a duplicate — the whole
    // point of the Clearingphase is that more versions arrive.
    let existing = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, &business_key)
        .await
        .map_err(|e| {
            DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string()))
        })?;
    if existing
        .iter()
        .any(|id| id.workflow_id.name.as_ref() == "mabis-billing")
    {
        return dispatch_to_process::<MabisBillingWorkflow, _>(
            state,
            &business_key,
            "mabis-billing",
            move || domain_cmd,
        )
        .await;
    }

    // ── Spawn the settlement ─────────────────────────────────────────────────
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
    process.execute_and_enqueue(domain_cmd).await.map_err(|e| {
        tracing::error!(
            process_id = %process_id,
            error      = %e,
            "MaBiS billing: spawn failed",
        );
        DispatchError::Engine(e)
    })?;

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
            "MaBiS billing: process registry registration failed — \
             follow-up commands may not route correctly",
        );
    }

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Parse a `"YYYY-MM"` Bilanzierungsmonat into its Fristenkalender.
fn parse_bilanzierungsmonat(s: &str) -> Result<mako_mabis::Bilanzierungsmonat, DispatchError> {
    let invalid = || {
        DispatchError::InvalidPayload(format!("\"bilanzierungsmonat\" \"{s}\" is not \"YYYY-MM\""))
    };
    let (y, m) = s.split_once('-').ok_or_else(invalid)?;
    let year: i32 = y.parse().map_err(|_| invalid())?;
    let month: u8 = m.parse().map_err(|_| invalid())?;
    let month = time::Month::try_from(month).map_err(|_| invalid())?;
    let letzter = time::util::days_in_month(month, year);
    let ende = time::Date::from_calendar_date(year, month, letzter).map_err(|_| invalid())?;
    Ok(mako_mabis::Bilanzierungsmonat::new(ende))
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

/// Dispatch `mabis.abrechnung.daten-einreichen` — send a Prüfmitteilung.
///
/// # There is no deadline on this
///
/// Kap. 9.8.2 Nr. 1 gives the Prüfmitteilung an empty Frist cell; the receiving
/// party „kann" answer positively or negatively. What bounds it is the clearing
/// window of Tabelle 2 (Kap. 3.10), which the workflow enforces by refusing once
/// the settlement is closed.
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `mabis_zp_id` | Yes | MaBiS-Zählpunkt (same as used in `einleiten`) |
/// | `bilanzierungsmonat` | Yes | `"YYYY-MM"` (same as used in `einleiten`) |
/// | `version` | Yes | which version is being checked — a Prüfmitteilung always refers to one (Kap. 3.8.3) |
/// | `pid` | Yes | 21000 (LF → NB/ÜNB), 21001 (NB → NB) or 21005 (BKV/NB → BIKO) |
/// | `message_ref` | No | reference for the outbound IFTSTA |
/// | `reject` | No | `true` for a negative Prüfmitteilung (default `false`) |
/// | `reason` | Conditional | required when `reject = true` |
pub(super) async fn dispatch_mabis_billing_daten_einreichen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let require = |field: &str| -> Result<String, DispatchError> {
        payload
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| DispatchError::InvalidPayload(format!("\"{field}\" is required")))
    };
    let mabis_zp_id = require("mabis_zp_id")?;
    let bilanzierungsmonat = require("bilanzierungsmonat")?;
    let business_key = format!("{mabis_zp_id}|{bilanzierungsmonat}");
    let version = mako_mabis::SzrVersion::new(require("version")?)
        .map_err(|e| DispatchError::InvalidPayload(e.to_string()))?;

    let pid_code = payload
        .get("pid")
        .and_then(|v| v.as_u64())
        .map(|n| u32::try_from(n).unwrap_or(u32::MAX))
        .ok_or_else(|| DispatchError::InvalidPayload("\"pid\" (u32) is required".into()))?;
    if !MABIS_IFTSTA_PRUEFMITTEILUNG_PIDS.contains(&pid_code) {
        return Err(DispatchError::InvalidPayload(format!(
            "pid {pid_code} carries no Prüfmitteilung of this participant; \
             valid: {MABIS_IFTSTA_PRUEFMITTEILUNG_PIDS:?}"
        )));
    }
    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;

    let message_ref = MessageRef::new(
        payload
            .get("message_ref")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
    );
    // The caller names a published Antwortcode, not a verdict: whether the
    // Prüfmitteilung is positive follows from the code's Cluster, and which
    // codes exist follows from the Entscheidungsbaum that decides the
    // Summenzeitreihe this stream settles. The workflow resolves both.
    let antwortcode = payload
        .get("antwortcode")
        .and_then(|v| v.as_str())
        .filter(|c| !c.trim().is_empty())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "\"antwortcode\" (the EBD code, e.g. \"A03\") is required".into(),
            )
        })?
        .to_owned();
    let grund = payload
        .get("grund")
        .and_then(|v| v.as_str())
        .filter(|r| !r.trim().is_empty())
        .map(ToOwned::to_owned);

    dispatch_to_process::<MabisBillingWorkflow, _>(
        state,
        &business_key,
        "mabis-billing",
        move || BillingCommand::SendPruefmitteilung {
            version,
            pid,
            antwortcode,
            grund,
            message_ref,
        },
    )
    .await
}

/// Dispatch `mabis.abrechnung.begleichen` — close the clearing window.
///
/// The Datenstatus itself is **not** set here: it is assigned exclusively by the
/// BIKO (Kap. 3.8.3) and arrives inbound as IFTSTA 21003 or 21004, which
/// `mabis.datenstatus.empfangen` records. What this command does is mark the
/// settlement closed once the clearing window of Tabelle 2 has lapsed, after
/// which no further version can change it.
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `mabis_zp_id` | Yes | MaBiS-Zählpunkt (same as used in `einleiten`) |
/// | `bilanzierungsmonat` | Yes | `"YYYY-MM"` (same as used in `einleiten`) |
/// | `lauf` | No | `"bka"` (default) or `"kbka"` |
pub(super) async fn dispatch_mabis_billing_begleichen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let require = |field: &str| -> Result<String, DispatchError> {
        payload
            .get(field)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| DispatchError::InvalidPayload(format!("\"{field}\" is required")))
    };
    let mabis_zp_id = require("mabis_zp_id")?;
    let bilanzierungsmonat = require("bilanzierungsmonat")?;
    let business_key = format!("{mabis_zp_id}|{bilanzierungsmonat}");

    let lauf = match payload
        .get("lauf")
        .and_then(|v| v.as_str())
        .unwrap_or("bka")
    {
        "bka" => mako_mabis::Abrechnungslauf::Bka,
        "kbka" => mako_mabis::Abrechnungslauf::Kbka,
        other => {
            return Err(DispatchError::InvalidPayload(format!(
                "unknown lauf \"{other}\"; valid: bka, kbka"
            )));
        }
    };

    dispatch_to_process::<MabisBillingWorkflow, _>(
        state,
        &business_key,
        "mabis-billing",
        move || BillingCommand::CloseClearing { lauf },
    )
    .await
}

// ── MaBiS Listenabgleich — the two reply legs ─────────────────────────────────

pub(super) fn cmd_mabis_liste_korrigieren<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_liste_korrigieren(s, p))
}

pub(super) fn cmd_mabis_liste_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_mabis_liste_ablehnen(s, p))
}

/// The business key a Clearingliste's process runs under — the Marktlokation the
/// ingest spawned it with, so a reply reaches the list it answers.
fn liste_business_key(payload: &serde_json::Value) -> Result<String, DispatchError> {
    payload
        .get("malo_id")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| DispatchError::InvalidPayload("\"malo_id\" is required".to_owned()))
}

/// Which Marktrolle distributed the list, which is what selects the tree.
///
/// 55066 is answered out of `E_0047` when the NB sent the list and out of
/// `E_0004` when the ÜNB did — one PID, two disjoint code spaces — so this is
/// required rather than defaulted. A default would answer half the lists out of
/// the wrong tree with codes the counterparty's Codeliste does not contain.
fn liste_sender_rolle(payload: &serde_json::Value) -> Result<String, DispatchError> {
    payload
        .get("sender_rolle")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "\"sender_rolle\" is required — it selects the Entscheidungsbaum (NB or ÜNB), \
                 and the two publish different codes for the same Korrekturgrund"
                    .to_owned(),
            )
        })
}

/// Dispatch `mabis.liste.korrigieren` — the **Korrekturliste** leg of a MaBiS
/// Clearingliste (BK6-24-174 Anlage 3, Clearingverfahren).
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `malo_id` | Yes | the business key the list's process runs under |
/// | `sender_rolle` | Yes | `"NB"` or `"ÜNB"` — selects the Entscheidungsbaum |
/// | `positionen` | No | disputed positions; **absent means none**, which is still a reply |
///
/// Each position is `{ "malo": "…", "grund": "ergaenzt" \| "falsche_zuordnung" \|
/// "entfallen" \| "daten_fehlerhaft" \| "malo_unbekannt" }`.
///
/// An **empty** list is the ordinary „reconciled, nothing to correct" answer and
/// is dispatched, not refused: silence would read as acceptance of whatever the
/// distributor filed, which is the same statement with none of the evidence.
pub(super) async fn dispatch_mabis_liste_korrigieren(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    use mako_pruefung::mabis::{Korrekturgrund, Korrekturposition};

    let business_key = liste_business_key(payload)?;
    let sender_rolle = liste_sender_rolle(payload)?;

    let mut positionen: Vec<Korrekturposition> = Vec::new();
    for entry in payload
        .get("positionen")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let malo = entry
            .get("malo")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                DispatchError::InvalidPayload("every Position needs a \"malo\"".to_owned())
            })?;
        let grund = match entry.get("grund").and_then(|v| v.as_str()) {
            Some("ergaenzt") => Korrekturgrund::Ergaenzt,
            Some("falsche_zuordnung") => Korrekturgrund::FalscheZuordnung,
            Some("entfallen") => Korrekturgrund::Entfallen,
            Some("daten_fehlerhaft") => Korrekturgrund::DatenFehlerhaft,
            Some("malo_unbekannt") => Korrekturgrund::MaloUnbekannt,
            other => {
                return Err(DispatchError::InvalidPayload(format!(
                    "unknown Korrekturgrund {other:?}; valid: ergaenzt, falsche_zuordnung, \
                     entfallen, daten_fehlerhaft, malo_unbekannt"
                )));
            }
        };
        positionen.push(Korrekturposition {
            malo: malo.to_owned(),
            grund,
        });
    }

    dispatch_to_process::<MabisListenabgleichWorkflow, _>(
        state,
        &business_key,
        "mabis-listenabgleich",
        move || ListenabgleichCommand::SendKorrektur {
            sender_rolle,
            positionen,
        },
    )
    .await
}

/// Dispatch `mabis.liste.ablehnen` — refuse the **whole list** and demand a new
/// one (`Cluster::AblehnungDerGesamtenListe`).
///
/// # Payload fields
///
/// | Field | Required | Description |
/// |-------|----------|-------------|
/// | `malo_id` | Yes | the business key the list's process runs under |
/// | `sender_rolle` | Yes | `"NB"` or `"ÜNB"` — selects the Entscheidungsbaum |
/// | `abonnement_bestellt` | No | „Wurde das Abonnement bestellt?" |
/// | `zeitraum_plausibel` | No | „Ist der Zeitraum plausibel?" |
/// | `mabis_zp_passt` | No | „Entspricht der MaBiS-ZP dem angefragten?" |
/// | `version_zugelassen` | No | „Ist die Version zugelassen?" |
/// | `innerhalb_clearingphase` | No | „Eingang innerhalb der Clearingphase DZÜ?" (`E_0070`) |
///
/// Each fact is a **tri-state**: `true` passes the Prüfschritt, `false` refuses
/// the list on it, and *absent* means the caller cannot answer it — which the
/// tree escalates rather than guessing, because a guess here refuses a list the
/// receiver could have reconciled. The tree also decides: a request to refuse a
/// list whose whole-list Prüfschritte all pass is rejected, since what that list
/// is owed is a Korrekturliste.
pub(super) async fn dispatch_mabis_liste_ablehnen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let business_key = liste_business_key(payload)?;
    let sender_rolle = liste_sender_rolle(payload)?;
    let fact = |name: &str| payload.get(name).and_then(serde_json::Value::as_bool);

    let (abonnement_bestellt, zeitraum_plausibel) =
        (fact("abonnement_bestellt"), fact("zeitraum_plausibel"));
    let (mabis_zp_passt, version_zugelassen) = (fact("mabis_zp_passt"), fact("version_zugelassen"));
    let innerhalb_clearingphase = fact("innerhalb_clearingphase");

    dispatch_to_process::<MabisListenabgleichWorkflow, _>(
        state,
        &business_key,
        "mabis-listenabgleich",
        move || ListenabgleichCommand::SendGesamtAblehnung {
            sender_rolle,
            abonnement_bestellt,
            zeitraum_plausibel,
            mabis_zp_passt,
            version_zugelassen,
            innerhalb_clearingphase,
        },
    )
    .await
}
