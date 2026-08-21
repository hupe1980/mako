//! GPKE (Strom) command wrappers and dispatchers.
//!
//! Split out of the flat `commands_api` module; shared state, types, and
//! process-dispatch helpers live in `super`.

use super::*;
use mako_gpke::{GpkeBeendigungZuordnungWorkflow, GpkeEogWorkflow};

// ── Per-command wrapper functions ─────────────────────────────────────────────
//
// Each is a named `fn` item (not a closure) so it satisfies the `for<'a> fn(...)`
// bound of `DispatchFn` and can be stored in a `static CommandDescriptor`.

pub(super) fn cmd_gpke_lieferbeginn_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_lf_anmeldung(s, p, 55001, "lieferbeginn_datum"))
}

pub(super) fn cmd_gpke_lieferende_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_lf_anmeldung(s, p, 55004, "lieferende_datum"))
}

pub(super) fn cmd_gpke_kuendigung_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_lf_anmeldung(s, p, 55016, "kuendigung_datum"))
}

pub(super) fn cmd_maloid_lieferbeginn_fortsetzen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_maloid_lieferbeginn_fortsetzen(s, p))
}

pub(super) fn cmd_gpke_nb_lieferende_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_nb_lieferende_antwort(s, p))
}

pub(super) fn cmd_gpke_nb_lieferende_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_nb_lieferende_antwort(s, p))
}

pub(super) fn cmd_gpke_beendigung_zuordnung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_beendigung_zuordnung_antwort(s, p))
}

pub(super) fn cmd_gpke_beendigung_zuordnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_beendigung_zuordnung_antwort(s, p))
}

pub(super) fn cmd_gpke_zuordnung_lf_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_zuordnung_lf_antwort(s, p))
}

pub(super) fn cmd_gpke_zuordnung_lf_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_zuordnung_lf_antwort(s, p))
}

/// `gpke.kuendigung.bestaetigen` — the LFA agrees to an inbound Kündigung (55017).
pub(super) fn cmd_gpke_kuendigung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_kuendigung_antwort(s, p))
}

/// `gpke.kuendigung.ablehnen` — the LFA refuses an inbound Kündigung (55018).
pub(super) fn cmd_gpke_kuendigung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_kuendigung_antwort(s, p))
}

pub(super) fn cmd_gpke_lieferbeginn_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_supplier_change_antwort(s, p, true))
}

pub(super) fn cmd_gpke_lieferbeginn_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_supplier_change_antwort(s, p, false))
}

pub(super) fn cmd_gpke_lieferende_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_supplier_change_antwort(s, p, true))
}

pub(super) fn cmd_gpke_lieferende_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_supplier_change_antwort(s, p, false))
}

pub(super) fn cmd_gpke_lieferbeginn_aktivieren<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_lf_activate(s, p))
}

pub(super) fn cmd_gpke_sperrung_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_sperrung_ausfuehrung(s, p, true))
}

pub(super) fn cmd_gpke_sperrung_fehlgeschlagen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_sperrung_ausfuehrung(s, p, false))
}

pub(super) fn cmd_gpke_sperrung_beauftragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_sperrung_lf_beauftragen(s, p, 17115))
}

pub(super) fn cmd_gpke_entsperrung_beauftragen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_sperrung_lf_beauftragen(s, p, 17117))
}

pub(super) fn cmd_gpke_sperrung_stornieren<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_sperrung_lf_stornieren(s, p))
}

pub(super) fn cmd_gpke_abrechnung_annehmen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        // The ERP (or invoicd) must supply the original INVOIC message-ref so
        // we can route to the correct billing process.
        let invoice_ref = extract_invoice_ref(p)?;
        dispatch_to_process::<GpkeAbrechnungWorkflow, _>(s, &invoice_ref, "gpke-abrechnung", || {
            AbrechnungCommand::SettleInvoice
        })
        .await
    })
}

pub(super) fn cmd_gpke_abrechnung_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        let invoice_ref = extract_invoice_ref(p)?;
        let reason = p
            .get("ablehnungsgrund")
            .and_then(|v| v.as_str())
            .unwrap_or("Automatisch ermittelte Abweichung")
            .to_owned();
        dispatch_to_process::<GpkeAbrechnungWorkflow, _>(s, &invoice_ref, "gpke-abrechnung", || {
            AbrechnungCommand::DisputeInvoice { reason }
        })
        .await
    })
}

pub(super) fn cmd_gpke_abrechnung_selbstausstellen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(async move {
        // `invoicd` calls this after generating the BO4E Rechnung for PID 31006.
        // The invoice_ref is the unique process ID generated by invoicd; it becomes
        // the process business key so future REMADV responses can be routed back.
        let invoice_ref_str = extract_invoice_ref(p)?;
        let nb_mp_id = p
            .get("nb_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let sender_mp_id = p
            .get("sender_mp_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let invoice_ref_clone = invoice_ref_str.clone();
        dispatch_to_process::<GpkeAbrechnungWorkflow, _>(
            s,
            &invoice_ref_str,
            "gpke-abrechnung",
            move || AbrechnungCommand::SendInvoic {
                pid: mako_engine::types::Pruefidentifikator::new(31006)
                    .expect("31006 is a valid PID"),
                sender: mako_engine::types::MarktpartnerCode::new(sender_mp_id.as_str()),
                recipient: mako_engine::types::MarktpartnerCode::new(nb_mp_id.as_str()),
                invoice_ref: mako_engine::types::MessageRef::new(invoice_ref_clone.clone()),
                document_date: time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Iso8601::DEFAULT)
                    .unwrap_or_default(),
            },
        )
        .await
    })
}

/// Dispatch a GPKE LF-side Anmeldung (Lieferbeginn / Lieferende / Kündigung).
///
/// 1. Extract `malo_id` and `process_date_field` from the ERP payload.
/// 2. Resolve the NB GLN from the MaLo cache.
/// 3. Spawn a new `GpkeLfAnmeldungWorkflow` process.
/// 4. Execute `InitiateAnmeldung` — writes the `Initiated` event and enqueues
///    the outbound UTILMD outbox entry atomically.
/// 5. Register a 24h NB-response deadline (GPKE BK6-22-024).
pub(super) async fn dispatch_lf_anmeldung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    pid_code: u32,
    process_date_key: &str,
) -> Result<DispatchOutcome, DispatchError> {
    // ── Extract ERP-supplied fields ───────────────────────────────────────────
    let malo_id = extract_malo_id(payload)?;

    let process_date = payload
        .get(process_date_key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "payload must contain \"{process_date_key}\" (ISO-8601 date string)",
            ))
        })?
        .to_owned();

    // Optional: ERP may supply `alter_lf_mp_id` for Kündigung (PID 55016) when
    // the old supplier is a different legal entity.  Not yet used in the domain
    // command but extracted here for future use.
    let _alter_lf_mp_id = payload
        .get("alter_lf_mp_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    // ── Resolve NB GLN from MaLo cache ───────────────────────────────────────
    //
    // `data_market_location_network_operators` is a time-sliced list of NBs.
    // We take the entry with the latest `execution_time_from` that has no
    // `execution_time_until` (i.e. the currently active NB).
    let malo_record = state
        .malo_cache
        .get(&state.tenant_id.to_string(), malo_id.as_str())
        .await
        .map_err(|e| DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string())))?
        .ok_or_else(|| DispatchError::MaloNotFound(malo_id.to_string()))?;

    let nb_mp_id = malo_record
        .data_market_location
        .data_market_location_network_operators
        .iter()
        // Prefer currently open time slice (no execution_time_until) or latest.
        .max_by_key(|p| (p.execution_time_until.is_none(), &p.execution_time_from))
        .map(|p| MarktpartnerCode::new(format!("{:013}", p.market_partner_id)))
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "MaLo {malo_id} in cache has no network_operator entry — \
             the NB GLN cannot be resolved",
            ))
        })?;

    // ── Build typed domain command ────────────────────────────────────────────
    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;

    let domain_cmd = LfAnmeldungCommand::InitiateAnmeldung {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: nb_mp_id,
        location_id: malo_id.clone(),
        process_date,
        // SG4 STS Transaktionsgrund (E01 Ein-/Auszug, E03 Wechsel) —
        // rendered as the outbound STS segment when supplied by the ERP.
        transaktionsgrund: payload
            .get("transaktionsgrund")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    };

    // ── Duplicate guard ───────────────────────────────────────────────────────
    //
    // Blocks only while an Anmeldung is still running (`Pending`/`Active`); a
    // `Rejected` one does not retire the MaLo, so the LF's corrected Anmeldung
    // goes through. See `find_occupying_process` for why presence in the
    // correlation index is not the question, and `LfAnmeldungState`'s
    // `OccupiesBusinessKey` impl for the per-state decision.
    if let Some(dup_id) = find_occupying_process::<GpkeLfAnmeldungWorkflow>(
        state,
        malo_id.as_str(),
        mako_gpke::lf_anmeldung::WORKFLOW_NAME,
    )
    .await?
    {
        tracing::warn!(
            malo_id    = %malo_id,
            process_id = %dup_id,
            "anmelden refused: an Anmeldung is still running for this MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    // ── Spawn process and execute command ─────────────────────────────────────
    //
    // `Process::new` generates a fresh ProcessId + StreamId.
    // `execute_and_enqueue_with_snapshot_and_retry` atomically appends the
    // `Initiated` event AND enqueues the outbound UTILMD outbox entry in a
    // single SlateDB WriteBatch (dual-write atomicity — no lost APERAKs on
    // crash).  Retries up to 3 times on VersionConflict and takes a snapshot
    // every 100 events so future replay is bounded to at most 100 tail events.
    let workflow_id = current_workflow_id();
    let process = mako_engine::process::Process::<
        GpkeLfAnmeldungWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );

    let process_id = process.process_id();

    // ── Build 24h NB-response deadline before the atomic write ───────────────
    //
    // The deadline is built from `now_utc()` here so the regulatory window
    // starts at the moment the ERP request is received, not when the store
    // write completes.  It is passed to `execute_and_enqueue_with_deadlines`
    // so that events, outbox entries, and the deadline land in a single SSI
    // transaction (F-009).
    let due_at = mako_fristen::add_hours(time::OffsetDateTime::now_utc(), 24);
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id.clone(),
        "nb-response-window-24h",
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    // ── Register process under MaLo business key ──────────────────────────────
    //
    // This enables Phase 3 response commands (`gpke.lieferbeginn.bestaetigen`,
    // `gpke.lieferbeginn.aktivieren`, etc.) to look up the active process by
    // `malo_id` without an exhaustive scan of all process streams.
    //
    // Registration is non-fatal: if it fails (e.g. transient storage error)
    // the process has already been spawned and the Anmeldung is underway.
    // The ERP must re-issue the command to trigger retry — the 202 is not sent
    // here so the ERP is not misled, but we log prominently for ops.
    let identity = process.identity();
    if let Err(e) = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await
    {
        tracing::warn!(
            process_id = %process_id,
            malo_id    = %malo_id,
            error      = %e,
            "ERP dispatch: business-key registration failed (non-fatal — Anmeldung was spawned); \
             response commands (bestaetigen/ablehnen/aktivieren) will return process_not_found \
             until the next successful anmelden call re-registers the process",
        );
    }

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch `maloid.lieferbeginn.fortsetzen` — auto-continue to GPKE PID 55001
/// after a MaLo-ID identification has completed.
///
/// ## Flow
///
/// 1. ERP receives `MaloIdentified` ERP event with `tx_id`.
/// 2. ERP calls `POST /api/v1/commands`:
///    ```json
///    {
///      "command": "maloid.lieferbeginn.fortsetzen",
///      "marktrolle": "LF",
///      "payload": {
///        "tx_id":              "<tx_id from the maloId request>",
///        "lieferbeginn_datum": "2026-10-01"
///      }
///    }
///    ```
/// 3. `makod` looks up `(malo_id, nb_mp_id)` from the `mc_txres/` cache using
///    the `tx_id` — no need for the ERP to track the MaLo-ID.
/// 4. Dispatches `LfAnmeldungCommand::InitiateAnmeldung` (PID 55001) with the
///    resolved data, exactly like `gpke.lieferbeginn.anmelden`.
///
/// ## Error cases
///
/// - `tx_id` unknown or result not yet cached → `422 tx_id_not_resolved`.
/// - `tx_id` resolved but `nb_mp_id` is empty → falls back to MaLo cache lookup.
pub(super) async fn dispatch_maloid_lieferbeginn_fortsetzen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    // ── Extract tx_id and lieferbeginn_datum ──────────────────────────────────
    let tx_id = payload
        .get("tx_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"tx_id\" (the MaLo-ID request transaction ID)".into(),
            )
        })?
        .to_owned();

    let lieferbeginn_datum = payload
        .get("lieferbeginn_datum")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"lieferbeginn_datum\" (ISO-8601 date, e.g. \"2026-10-01\")"
                    .into(),
            )
        })?
        .to_owned();

    // ── Look up resolved result from tx_id cache ──────────────────────────────
    let resolved = state
        .maloid_result_cache
        .get_result(&state.tenant_id.to_string(), &tx_id)
        .await
        .map_err(|e| DispatchError::Engine(
            mako_engine::error::EngineError::store(e.to_string()),
        ))?
        .ok_or_else(|| DispatchError::InvalidPayload(format!(
            "tx_id {tx_id:?} not found in result cache — \
             the MaLo-ID identification may not yet have completed (positive callback not yet delivered). \
             Retry after receiving the MaloIdentified ERP event, or use \
             gpke.lieferbeginn.anmelden with an explicit malo_id.",
        )))?;

    // ── Build the synthetic ERP payload for dispatch_lf_anmeldung ────────────
    let synthetic_payload = serde_json::json!({
        "malo_id":           resolved.malo_id,
        "lieferbeginn_datum": lieferbeginn_datum,
    });

    tracing::info!(
        tx_id            = %tx_id,
        malo_id          = %resolved.malo_id,
        nb_mp_id           = %resolved.nb_mp_id,
        lieferbeginn_dat = %lieferbeginn_datum,
        "maloid.lieferbeginn.fortsetzen: resolved tx_id — dispatching PID 55001"
    );

    dispatch_lf_anmeldung(state, &synthetic_payload, 55001, "lieferbeginn_datum").await
}

/// Read an LF answer out of an ERP command payload.
///
/// | Field | Required | Wire slot |
/// |---|---|---|
/// | `antwort_code` | ✓ | `SG4 STS+E01` DE 9013 |
/// | `antwort_ebd` | — | `SG4 STS+E01` DE 1131 (absent on the Gas Codelisten) |
/// | `bemerkung` | — | `FTX+ACB` Erläuterung |
/// | `termin` | — | `SG4 DTM+93`, `YYYYMMDD`, when the answer states its own date |
///
/// The code is **validated against its EBD** before it goes anywhere: a code
/// the named tree does not publish is a rejected command, not a message the
/// counterparty gets to refuse. Whether the answer is a Bestätigung or an
/// Ablehnung is read from the code's published Cluster — the caller does not
/// get a separate say, which is what kept `A35` „Vertragsbindung" from ever
/// riding a Bestätigungs-PID.
fn extract_lf_antwort(
    payload: &serde_json::Value,
    default_ebd: Option<&str>,
) -> Result<mako_gpke::LfAntwort, DispatchError> {
    let antwort_code = payload
        .get("antwort_code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "payload must contain \"antwort_code\" — the EBD Antwortcode for \
                 SG4 STS+E01. The AHB marks that segment Muss on every \
                 Antwortnachricht{}",
                default_ebd
                    .map(|e| format!(", and restricts the code to the {e} cluster"))
                    .unwrap_or_default()
            ))
        })?
        .to_owned();

    let ebd = payload
        .get("antwort_ebd")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
        .or_else(|| default_ebd.map(ToOwned::to_owned));

    // `antwort_tree` is the **lookup key**, `antwort_ebd` the DE 1131 wire
    // value. They differ for Gas, whose Codelisten the MIG does not name in the
    // segment: without the key a Gas answer could carry any string at all.
    let tree = payload
        .get("antwort_tree")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
        .or_else(|| ebd.clone());

    // Resolve the code inside its own tree. `A02` means three different things
    // across E_0607, E_0622 and E_0609, so a bare code is not checkable.
    let zustimmung = match tree.as_deref() {
        Some(tree) => {
            let entry = mako_pruefung::codes::lookup(tree, &antwort_code).ok_or_else(|| {
                DispatchError::InvalidPayload(format!(
                    "Antwortcode {antwort_code:?} is not published by {tree}. \
                     Only codes from that Entscheidungsbaum's Codeliste are admissible."
                ))
            })?;
            entry.ist_zustimmung()
        }
        None => {
            // Neither key nor wire value: the caller must state the cluster,
            // and nothing validates the code.
            payload
                .get("zustimmung")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| {
                    DispatchError::InvalidPayload(
                        "an answer without an \"antwort_tree\" or \"antwort_ebd\" must \
                         state \"zustimmung\": the cluster decides the response PID and \
                         cannot be derived from an unresolved code"
                            .into(),
                    )
                })?
        }
    };

    Ok(mako_gpke::LfAntwort {
        antwort_code,
        ebd,
        zustimmung,
        bemerkung: payload
            .get("bemerkung")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned),
        termin: payload
            .get("termin")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned),
    })
}

/// Dispatch the LFA's answer to an inbound **Kündigung** (55016, EBD `E_0614`).
///
/// Both `gpke.kuendigung.bestaetigen` and `.ablehnen` land here: which of
/// 55017 / 55018 goes out is decided by the Antwortcode's published Cluster,
/// not by which command name the ERP happened to call. `A03` „Vertrag wurde
/// bereits zum angefragten Kündigungstermin gekündigt" is a **Zustimmung**
/// despite reading like a complaint, and a command-name-driven split would send
/// it as an Ablehnung.
///
/// The inbound 55016 runs on its own `gpke-kuendigung` workflow, so an LFA
/// answer can never resume the grid operator's Anmeldung on the same MaLo.
pub(super) async fn dispatch_kuendigung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    let antwort = extract_lf_antwort(payload, Some(mako_pruefung::codes::EBD_KUENDIGUNG))?;

    dispatch_to_process::<mako_gpke::GpkeKuendigungWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_gpke::kuendigung::WORKFLOW_NAME,
        move || mako_gpke::KuendigungCommand::SendAntwort { antwort },
    )
    .await
}

/// Dispatch LF's response to a NB-initiated Lieferende (PIDs 55008/55009).
///
/// Called for `gpke.nb-lieferende.bestaetigen` (→ 55008 Bestätigung) and
/// `gpke.nb-lieferende.ablehnen` (→ 55009 Ablehnung).
///
/// The LF receives PID 55007 Ankündigung via AS4 (auto-spawned by the ingest
/// dispatcher). After review, the ERP operator calls this command to send the
/// formal response. APERAK Frist: 24h wall-clock (BK6-22-024 §4).
///
/// ## Required payload fields
///
/// | Field | Type | Notes |
/// |---|---|---|
/// | `malo_id` | string | Marktlokations-ID identifying the process |
/// | `reason` | string (opt.) | Rejection reason — mandatory when `accepted = false` |
pub(super) async fn dispatch_gpke_nb_lieferende_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    let antwort = extract_lf_antwort(payload, Some(mako_pruefung::codes::EBD_ABMELDUNG))?;

    dispatch_to_process::<GpkeLfAbmeldungWorkflow, _>(
        state,
        malo_id.as_str(),
        "gpke-lf-abmeldung",
        move || mako_gpke::LfAbmeldungCommand::SendAntwort { antwort },
    )
    .await
}

/// Dispatch the LFA's response to an `Anfrage zur Beendigung der Zuordnung`
/// (inbound PID 55010, EBD **E_0624**).
///
/// Called for `gpke.beendigung-zuordnung.bestaetigen` (→ 55011 Bestätigung) and
/// `gpke.beendigung-zuordnung.ablehnen` (→ 55012 Ablehnung).
///
/// The ingest dispatcher spawns `gpke-beendigung-zuordnung` on an inbound 55010
/// and registers the 24 h business Frist (BK6-22-024 § 4). Until these two
/// commands existed the spawned process had no way to be answered at all — it
/// could only run out its deadline.
///
/// ## Required payload fields
///
/// | Field | Type | Notes |
/// |---|---|---|
/// | `malo_id` | string | Marktlokations-ID identifying the process |
/// | `reason` | string (opt.) | Rejection reason — mandatory when `accepted = false` |
pub(super) async fn dispatch_gpke_beendigung_zuordnung_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    let antwort = extract_lf_antwort(
        payload,
        Some(mako_pruefung::codes::EBD_BEENDIGUNG_ZUORDNUNG),
    )?;

    dispatch_to_process::<GpkeBeendigungZuordnungWorkflow, _>(
        state,
        malo_id.as_str(),
        "gpke-beendigung-zuordnung",
        move || mako_gpke::BeendigungZuordnungCommand::SendAntwort { antwort },
    )
    .await
}

/// Dispatch LFN's response to a NB-initiated Ankündigung Zuordnung LF (PIDs 55608/55609).
///
/// Called for `gpke.zuordnung-lf.bestaetigen` (→ 55608 Bestätigung) and
/// `gpke.zuordnung-lf.ablehnen` (→ 55609 Ablehnung).
///
/// The LFN receives PID 55607 Ankündigung via AS4 (auto-spawned by the ingest
/// dispatcher). After review, the ERP operator calls this command to send the
/// formal response. APERAK Frist: 24h wall-clock (BK6-22-024 §4).
///
/// ## Required payload fields
///
/// | Field | Type | Notes |
/// |---|---|---|
/// | `malo_id` | string | Marktlokations-ID identifying the process |
/// | `reason` | string (opt.) | Rejection reason — mandatory when `accepted = false` |
pub(super) async fn dispatch_gpke_zuordnung_lf_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    // 55607–55609 is governed by **four** trees, one per Anwendungsfall of the
    // NB's Ankündigung — `E_0603` EEG, `E_0604` EEG mit DV-Pflicht, `E_0605`
    // KWKG, `E_0606`. They publish the same two codes (`A01` Zustimmung, `A99`
    // Sonstiges) and differ only in which case they belong to, so the caller
    // names the one the inbound message carried in `SG4 STS+E01` DE 1131.
    //
    // This previously passed `None` on the belief that the family had no
    // published EBD, which sent every answer with an empty DE 1131.
    let antwort = extract_lf_antwort(payload, Some(mako_pruefung::codes::EBD_ZUORDNUNG_LF[0]))?;

    dispatch_to_process::<GpkeAnkuendigungZuordnungLfWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_gpke::ankuendigung_zuordnung_lf::WORKFLOW_NAME,
        move || AnkuendigungZuordnungLfCommand::SendAntwort { antwort },
    )
    .await
}

/// Dispatch the NB's decision (Bestätigung or Ablehnung) to an existing
/// `GpkeSupplierChangeWorkflow` process looked up by `malo_id`.
///
/// Called for `gpke.lieferbeginn.bestaetigen`, `gpke.lieferbeginn.ablehnen`,
/// `gpke.lieferende.bestaetigen`, `gpke.lieferende.ablehnen`.
///
/// ## Required payload fields
///
/// | Field | Required | Wire slot |
/// |---|---|---|
/// | `malo_id` | ✓ | Marktlokations-ID identifying the NB-side process |
/// | `antwort_code` | ✓ | `SG4 STS+E01` DE 9013 |
/// | `antwort_ebd` | — | `SG4 STS+E01` DE 1131 (absent on the Gas Codelisten) |
/// | `zustimmung` | Gas only | the cluster, which the Gas code alone cannot give |
/// | `bemerkung` | — | `FTX+ACB` Erläuterung |
///
/// The NB's own answer is validated exactly like the LF's: the code is resolved
/// inside its tree and the **published Cluster decides the response PID**, so
/// `gpke.lieferbeginn.bestaetigen` carrying an Ablehnungscode is a rejected
/// command rather than a 55002 stating a refusal. The `accepted` flag the
/// command name implies is checked against that cluster, not trusted.
pub(super) async fn dispatch_supplier_change_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    accepted: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    let antwort = extract_lf_antwort(payload, None)?;

    if antwort.zustimmung != accepted {
        return Err(DispatchError::InvalidPayload(format!(
            "Antwortcode {:?} is a {} in {}, but the command asks for a{}. The published              Cluster decides the response PID; the two may not disagree.",
            antwort.antwort_code,
            if antwort.zustimmung {
                "Zustimmung"
            } else {
                "Ablehnung"
            },
            antwort.ebd.as_deref().unwrap_or("its Gas Codeliste"),
            if accepted {
                " Bestätigung"
            } else {
                "n Ablehnung"
            },
        )));
    }

    dispatch_to_process::<GpkeSupplierChangeWorkflow, _>(
        state,
        malo_id.as_str(),
        "gpke-supplier-change",
        move || SupplierChangeCommand::SendAntwort {
            antwort,
            obligations: vec![],
        },
    )
    .await
}

/// Dispatch `LfAnmeldungCommand::Activate` to an existing `GpkeLfAnmeldungWorkflow`
/// process looked up by `malo_id`.
///
/// Called for `gpke.lieferbeginn.aktivieren`.
pub(super) async fn dispatch_lf_activate(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;

    dispatch_to_process::<GpkeLfAnmeldungWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_gpke::lf_anmeldung::WORKFLOW_NAME,
        || LfAnmeldungCommand::Activate,
    )
    .await
}

/// Report physical Sperrung/Entsperrung execution to the NB-role
/// `GpkeSperrungWorkflow` (ORDERS 17115/17117 → IFTSTA 21039).
///
/// Called by `sperrd` after a field technician confirms (or fails to carry out)
/// the disconnection, and by ERP operators driving `sperrd` manually.
///
/// **Role: NB.** This is the *grid operator* confirming execution of an order it
/// received from the Lieferant — not the LF issuing one. The LF-side lifecycle is
/// [`dispatch_gpke_sperrung_lf_beauftragen`].
///
/// Business key = `malo_id`; the inbound ORDERS 17115/17117 spawned the process
/// and registered the correlation.
///
/// Regulatory basis: GPKE BK6-22-024 §5 — the NB must dispatch IFTSTA 21039 after
/// physical execution. `durchgefuehrt = false` reports a failed execution with a
/// reason (meter access denied, safety block, …) so the LF is not left waiting.
pub(super) async fn dispatch_gpke_sperrung_ausfuehrung(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    durchgefuehrt: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;

    // `note` is what sperrd sends; `reason` is the ERP-facing alias.
    let reason = payload
        .get("reason")
        .or_else(|| payload.get("note"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    if !durchgefuehrt && reason.is_none() {
        return Err(DispatchError::InvalidPayload(
            "gpke.sperrung.fehlgeschlagen requires \"reason\" (or \"note\") \
             explaining why execution failed"
                .to_owned(),
        ));
    }

    dispatch_to_process::<GpkeSperrungWorkflow, _>(
        state,
        malo_id.as_str(),
        SPERRUNG_WORKFLOW_NAME,
        move || SperrungCommand::BestaetigueSperrung {
            durchgefuehrt,
            reason,
        },
    )
    .await
}

/// Spawn an LF-initiated Sperrauftrag (17115) or Entsperrauftrag (17117).
///
/// **Role: LF.** The Lieferant instructs the Netzbetreiber to disconnect or
/// reconnect a delivery point (typically after a dunning escalation reaches
/// Mahnstufe 3 in `accountingd`). The NB answers with ORDRSP 19116/19117 and
/// later reports execution via IFTSTA 21039 — both routed back to this process
/// by `PidRouter`.
///
/// Deadline: 24 wall-clock hours for the NB's ORDRSP (BK6-22-024).
pub(super) async fn dispatch_gpke_sperrung_lf_beauftragen(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    pid_code: u32,
) -> Result<DispatchOutcome, DispatchError> {
    if !SPERRUNG_LF_ANFRAGE_PIDS.contains(&pid_code) {
        return Err(DispatchError::InvalidPayload(format!(
            "unsupported Sperrung PID {pid_code}; expected 17115 or 17117"
        )));
    }

    let malo_id = extract_malo_id(payload)?;

    // Resolve the NB GLN from the MaLo cache — the LF addresses its own grid operator.
    let malo_record = state
        .malo_cache
        .get(&state.tenant_id.to_string(), malo_id.as_str())
        .await
        .map_err(|e| DispatchError::Engine(mako_engine::error::EngineError::store(e.to_string())))?
        .ok_or_else(|| DispatchError::MaloNotFound(malo_id.to_string()))?;

    let nb_mp_id = malo_record
        .data_market_location
        .data_market_location_network_operators
        .iter()
        .max_by_key(|p| (p.execution_time_until.is_none(), &p.execution_time_from))
        .map(|p| MarktpartnerCode::new(format!("{:013}", p.market_partner_id)))
        .ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "MaLo {malo_id} has no network_operator — NB GLN cannot be resolved",
            ))
        })?;

    let pid = Pruefidentifikator::new(pid_code).map_err(DispatchError::InvalidPayload)?;
    let message_ref = MessageRef::new(format!("SPERR-{}", uuid::Uuid::new_v4()));

    let domain_cmd = SperrungLfCommand::InitiateSperrung {
        pid,
        nb_mp_id,
        location_id: malo_id.clone(),
        message_ref,
    };

    // Duplicate guard — only a Sperrung still awaiting the NB blocks. An
    // executed one (`Ausgefuehrt`) is terminal, so the Entsperrung that follows
    // — which reuses this workflow — goes through. See `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<GpkeSperrungLfWorkflow>(
        state,
        malo_id.as_str(),
        SPERRUNG_LF_WORKFLOW_NAME,
    )
    .await?
    {
        tracing::warn!(
            malo_id = %malo_id,
            process_id = %dup_id,
            pid = pid_code,
            "gpke.sperrung.beauftragen refused: active Sperrung process already exists for this MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    let workflow_id = WorkflowId::new(SPERRUNG_LF_WORKFLOW_NAME, latest_format_version());
    let process = mako_engine::process::Process::<
        GpkeSperrungLfWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    // 24 wall-clock hours for the NB's ORDRSP (BK6-22-024) — not Werktage.
    let due_at = mako_fristen::add_hours(time::OffsetDateTime::now_utc(), 24);
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        SPERRUNG_LF_ANTWORT_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    let _ = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await;

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Cancel a pending LF-initiated Sperrauftrag via ORDCHG 39000.
///
/// **Role: LF.** Valid while the process is in `AuftragGesendet` or
/// `OrdrsepBestaetigt` — i.e. until the NB has physically executed. The NB
/// answers with ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung).
///
/// Typical trigger: the customer pays before the disconnection is carried out.
pub(super) async fn dispatch_gpke_sperrung_lf_stornieren(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;
    let message_ref = MessageRef::new(format!("SPERR-STORNO-{}", uuid::Uuid::new_v4()));

    dispatch_to_process::<GpkeSperrungLfWorkflow, _>(
        state,
        malo_id.as_str(),
        SPERRUNG_LF_WORKFLOW_NAME,
        move || SperrungLfCommand::SendStornierung { message_ref },
    )
    .await
}

// ── EoG (Ersatz-/Grundversorgung, §36/§38 EnWG) ───────────────────────────────

pub(super) fn cmd_gpke_eog_anmelden<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_eog_anmelden(s, p))
}

pub(super) fn cmd_gpke_eog_bestaetigen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_eog_antwort(s, p, true))
}

pub(super) fn cmd_gpke_eog_ablehnen<'a>(
    s: &'a CommandsApiState,
    p: &'a serde_json::Value,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<DispatchOutcome, DispatchError>> + Send + 'a>,
> {
    Box::pin(dispatch_gpke_eog_antwort(s, p, false))
}

/// Dispatch the NB's UTILMD 55013 Anmeldung / Zuordnung EOG (§36/§38 EnWG).
///
/// Called for `gpke.eog.anmelden` — typically by the `processd` gap-closure
/// automation after `de.markt.versorgung.gap-detected`, or manually by the
/// operator. GPKE Teil 2 Kap. 2.3: the Zuordnung is sent "unverzüglich" and
/// may be retroactive.
///
/// ## Required payload fields
///
/// | Field | Type | Notes |
/// |---|---|---|
/// | `malo_id` | string | Marktlokations-ID |
/// | `gv_mp_id` | string | MP-ID of the Grundversorger (§36 Abs. 2 Feststellung) |
/// | `process_date` | string | Zuordnungsbeginn, ISO-8601 or `YYYYMMDD` (retroactive allowed) |
/// | `transaktionsgrund` | string | SG4 STS DE9013 (Z02/Z36/Z37/Z39/ZC6/ZC7/ZT6/ZT7/E06/ZZD) |
/// | `haushaltskunde` | bool (opt.) | CCI Z15/Z18, when known |
pub(super) async fn dispatch_gpke_eog_anmelden(
    state: &CommandsApiState,
    payload: &serde_json::Value,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;

    let gv_mp_id = payload
        .get("gv_mp_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"gv_mp_id\" (MP-ID of the Grundversorger)".into(),
            )
        })?
        .to_owned();
    let process_date = payload
        .get("process_date")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"process_date\" (Zuordnungsbeginn, ISO-8601)".into(),
            )
        })?
        .to_owned();
    let transaktionsgrund = payload
        .get("transaktionsgrund")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"transaktionsgrund\" (SG4 STS DE9013, e.g. \"ZT6\")".into(),
            )
        })?
        .to_owned();
    let haushaltskunde = payload.get("haushaltskunde").and_then(|v| v.as_bool());

    let pid = Pruefidentifikator::new(mako_gpke::EOG_ANMELDUNG_PID)
        .map_err(DispatchError::InvalidPayload)?;
    let domain_cmd = mako_gpke::EogCommand::Anmelden {
        pid,
        sender: MarktpartnerCode::new(state.sender_party_id.clone()),
        receiver: MarktpartnerCode::new(gv_mp_id),
        location_id: malo_id.clone(),
        process_date,
        transaktionsgrund,
        haushaltskunde,
    };

    // ── Duplicate guard (workflow-scoped) ─────────────────────────────────────
    // Only a still-running EoG process for this MaLo blocks; a concurrent
    // Lieferbeginn or Sperrung on the same MaLo is legitimate, and an EoG that
    // was rejected or ended is terminal. See `find_occupying_process`.
    if let Some(dup_id) = find_occupying_process::<GpkeEogWorkflow>(
        state,
        malo_id.as_str(),
        mako_gpke::EOG_WORKFLOW_NAME,
    )
    .await?
    {
        tracing::warn!(
            malo_id    = %malo_id,
            process_id = %dup_id,
            "gpke.eog.anmelden refused: an EoG process is still running for this MaLo",
        );
        return Err(DispatchError::DuplicateProcess {
            process_id: dup_id,
            malo_id: malo_id.into(),
        });
    }

    // ── Spawn process ─────────────────────────────────────────────────────────
    let workflow_id = WorkflowId::new(mako_gpke::EOG_WORKFLOW_NAME, latest_format_version());
    let process = mako_engine::process::Process::<
        mako_gpke::GpkeEogWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(
        Arc::clone(&state.store),
        state.tenant_id,
        workflow_id.clone(),
    );
    let process_id = process.process_id();

    // E/G answer window (GPKE Teil 2 Kap. 2.3 SD Schritt 2): 15:00 windows,
    // outer envelope = next Werktag. On expiry the workflow assigns anyway
    // with the pre-deposited default Bilanzkreis (SD Schritt 3).
    let due_at = mako_gpke::eog_antwort_due_at(time::OffsetDateTime::now_utc());
    let deadline = Deadline::new(
        process.stream_id().clone(),
        process_id,
        state.tenant_id,
        workflow_id,
        mako_gpke::EOG_RESPONSE_WINDOW_LABEL,
        due_at,
    );
    process
        .execute_and_enqueue_with_deadlines(domain_cmd, &[deadline])
        .await?;

    let identity = process.identity();
    if let Err(e) = state
        .store
        .as_process_registry()
        .register_correlated(state.tenant_id, malo_id.as_str(), process_id, identity)
        .await
    {
        tracing::warn!(
            process_id = %process_id,
            malo_id    = %malo_id,
            error      = %e,
            "gpke.eog.anmelden: business-key registration failed (non-fatal)",
        );
    }

    Ok(DispatchOutcome::Spawned { process_id })
}

/// Dispatch the E/G's response to an inbound EoG Zuordnung (PIDs 55014/55015).
///
/// Called for `gpke.eog.bestaetigen` (→ 55014, requires `versorgungsart`) and
/// `gpke.eog.ablehnen` (→ 55015, requires `reason` — EBD E_0615 A02/A04/A05).
///
/// ## Payload fields
///
/// | Field | Type | Notes |
/// |---|---|---|
/// | `malo_id` | string | Marktlokations-ID identifying the process |
/// | `versorgungsart` | string | `ZC9`/`ZD0`/`ZE3`/`ZZD` — mandatory for Bestätigung |
/// | `bilanzkreis` | string (opt.) | EIC of the Bilanzkreis (Bestätigung) |
/// | `reason` | string (opt.) | mandatory for Ablehnung |
pub(super) async fn dispatch_gpke_eog_antwort(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    accepted: bool,
) -> Result<DispatchOutcome, DispatchError> {
    let malo_id = extract_malo_id(payload)?;

    let versorgungsart = match payload.get("versorgungsart").and_then(|v| v.as_str()) {
        Some(code) => Some(mako_gpke::Versorgungsart::from_code(code).ok_or_else(|| {
            DispatchError::InvalidPayload(format!(
                "invalid \"versorgungsart\" {code:?} — expected ZC9, ZD0, ZE3, or ZZD",
            ))
        })?),
        None => None,
    };
    let bilanzkreis = payload
        .get("bilanzkreis")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    dispatch_to_process::<mako_gpke::GpkeEogWorkflow, _>(
        state,
        malo_id.as_str(),
        mako_gpke::EOG_WORKFLOW_NAME,
        move || mako_gpke::EogCommand::SendAntwort {
            accepted,
            versorgungsart,
            bilanzkreis,
            reason,
        },
    )
    .await
}
