//! The one door every outbound INVOIC passes: build, validate, spawn.
//!
//! `invoic.nne-abschlag.stellen`, `invoic.nne.stellen`, `invoic.mmm.stellen`
//! (31005, or 31006 for a selbst ausgestellte Rechnung),
//! `wim.msb-rechnung.stellen` and `invoic.sonstige-leistung.stellen` each run
//! the § 14 / § 27 Abs. 38 UStG
//! check ([`super::e_rechnung::edi_pruefen`]) and then come here, where:
//!
//! 1. the payload's BO4E `rechnung` is built into the INVOIC of the PID and
//!    validated against its Prüfschablone
//!    ([`crate::orchestrator::invoic_ausgang`]) — a Rechnung that cannot go
//!    out is refused to the caller with the places the AHB names;
//! 2. the issuing process is **spawned** — an invoice starts its own process,
//!    nothing else does — with `invoice_ref` as its correlation key, in the one
//!    write that records `InvoicSent` and enqueues the INVOIC
//!    ([`super::spawn_process`]);
//! 3. a Stornorechnung — a `rechnung` with `ist_storno` — goes out as PID
//!    31004 (INVOIC AHB 1.0b Kap. 3.1.2) under `wim-invoic`, the workflow
//!    that owns that Sparte-neutral PID, whichever door it came through;
//! 4. a second command for the same `invoice_ref` reaches the process the
//!    first one spawned, as `409 duplicate_invoice`: a Rechnungsnummer names
//!    one document, so any process already holding it — whatever its state —
//!    is that document, and a second spawn would send a second INVOIC the
//!    counterparty cannot be made to forget.

use super::*;

/// What a door states beyond the payload.
pub(super) struct Ausstellung<'a> {
    /// The issuing workflow's name.
    pub workflow_name: &'static str,
    /// The PID of the invoice.
    pub pid: u32,
    /// `NAD+MS`.
    pub sender: &'a str,
    /// `NAD+MR`.
    pub recipient: &'a str,
}

/// Build the INVOIC of the payload's `rechnung` and spawn its issuing process.
///
/// # Errors
///
/// [`DispatchError::InvalidPayload`] for a payload without a `rechnung` or
/// whose `invoice_ref` is not its `rechnungsnummer`;
/// [`DispatchError::InvoicNichtDarstellbar`] for a Rechnung the profile
/// refuses; [`DispatchError::DuplicateInvoice`] when a process holds the
/// `invoice_ref` already; and the spawn's own failures.
pub(super) async fn invoic_stellen<W>(
    state: &CommandsApiState,
    payload: &serde_json::Value,
    a: Ausstellung<'_>,
) -> Result<DispatchOutcome, DispatchError>
where
    W: mako_engine::workflow::Workflow<Command = InvoicCommand>,
    Arc<mako_engine::store_slatedb::SlateDbStore>: mako_engine::event_store::AtomicAppend,
{
    let invoice_ref = extract_invoice_ref(payload)?;
    let rechnung: rubo4e::current::Rechnung = payload
        .get("rechnung")
        .cloned()
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "payload must contain \"rechnung\" (BO4E RECHNUNG)".into(),
            )
        })
        .and_then(|r| {
            serde_json::from_value(r).map_err(|e| {
                DispatchError::InvalidPayload(format!("rechnung is not a BO4E RECHNUNG: {e}"))
            })
        })?;
    // The REMADV answers by the number printed on the document (`SG5 DOC`),
    // and the process is found by `invoice_ref` — the two must be one value.
    if rechnung.rechnungsnummer.as_deref() != Some(invoice_ref.as_str()) {
        return Err(DispatchError::InvalidPayload(format!(
            "invoice_ref {invoice_ref:?} is not the rechnung's rechnungsnummer {:?} — the \
             REMADV answering BGM DE 1004 would find no process",
            rechnung.rechnungsnummer
        )));
    }

    let heute = mako_fristen::heute();
    let profile = crate::orchestrator::invoic_ausgang::profil(heute).ok_or_else(|| {
        DispatchError::InvoicNichtDarstellbar {
            message: crate::orchestrator::invoic_ausgang::InvoicAusgangError::KeinProfil(heute)
                .to_string(),
            places: Vec::new(),
        }
    })?;
    let message_ref = crate::orchestrator::edifact_renderer::msg_ref_from_uuid(&format!(
        "INVOIC{}",
        uuid::Uuid::new_v4()
    ));
    let text = |k: &str| {
        payload
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
    };
    let ausfuehrungsdatum = text("ausfuehrungsdatum")
        .map(|d| {
            time::Date::parse(d, &time::format_description::well_known::Iso8601::DATE).map_err(
                |_| {
                    DispatchError::InvalidPayload(
                        "ausfuehrungsdatum is not a YYYY-MM-DD date".into(),
                    )
                },
            )
        })
        .transpose()?;
    let kopf = crate::orchestrator::invoic_ausgang::Kopf {
        pid: a.pid,
        sender: a.sender,
        receiver: a.recipient,
        message_ref: &message_ref,
        // `DTM+9` is the Buchungsdatum of the receivable (MIG 2.8e 00006),
        // booked when it is issued — which is now.
        buchungsdatum: time::OffsetDateTime::now_utc(),
        zugehoeriges_dokument: text("zugehoeriges_dokument"),
        preise_mp_id: text("preise_mp_id"),
        rechnungstyp: text("rechnungstyp"),
        ausfuehrungsdatum,
    };
    let ist_storno = rechnung.ist_storno == Some(true);
    let built = if ist_storno {
        // `DTM+171` is the Ursprungsrechnung's `DTM+137` (MIG 2.8e 00019);
        // the Stornorechnung does not carry it, so the caller states it.
        let ursprungsdatum = match payload.get("original_rechnungsdatum") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => Some(
                serde_json::from_value::<String>(v.clone())
                    .ok()
                    .and_then(|s| {
                        time::OffsetDateTime::parse(
                            &s,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    })
                    .ok_or_else(|| {
                        DispatchError::InvalidPayload(
                            "original_rechnungsdatum is not an RFC 3339 timestamp".into(),
                        )
                    })?,
            ),
        };
        crate::orchestrator::invoic_ausgang::storno(profile, &rechnung, ursprungsdatum, &kopf)
    } else {
        crate::orchestrator::invoic_ausgang::rechnung(profile, &rechnung, &kopf)
    }
    .map_err(|e| DispatchError::InvoicNichtDarstellbar {
        places: e.places(),
        message: e.to_string(),
    })?;
    let (pid, workflow_name) = if ist_storno {
        (
            crate::orchestrator::invoic_ausgang::STORNO_PID,
            mako_wim::invoic::WORKFLOW_NAME,
        )
    } else {
        (a.pid, a.workflow_name)
    };

    let pid = mako_engine::types::Pruefidentifikator::new(pid).map_err(|e| {
        DispatchError::InvalidPayload(format!("{pid} is not a Prüfidentifikator: {e}"))
    })?;
    let command = InvoicCommand::SendInvoic {
        pid,
        sender: mako_engine::types::MarktpartnerCode::new(a.sender),
        recipient: mako_engine::types::MarktpartnerCode::new(a.recipient),
        document_date: rechnung
            .rechnungsdatum
            .map(|t| {
                let d = t.date();
                format!("{:04}{:02}{:02}", d.year(), u8::from(d.month()), d.day())
            })
            .unwrap_or_default(),
        invoice_ref: mako_engine::types::MessageRef::new(invoice_ref.clone()),
        nachricht: built.nachricht,
    };

    // Held across the lookup and the spawn: the ingest dispatcher spawns under
    // the same lock, so a concurrent command for the same invoice waits rather
    // than passing the lookup beside this one.
    let _guard = crate::ingest_dispatcher::business_key_locks()
        .acquire(&invoice_ref)
        .await;
    if let Some(existing) = state
        .store
        .as_process_registry()
        .lookup_correlated(state.tenant_id, &invoice_ref)
        .await
        .map_err(DispatchError::Engine)?
        .into_iter()
        .find(|id| id.workflow_id.name.as_ref() == workflow_name)
    {
        return Err(DispatchError::DuplicateInvoice {
            process_id: existing.process_id,
            invoice_ref,
        });
    }
    let workflow_id = WorkflowId::new(workflow_name, latest_format_version());
    let keys = [invoice_ref.as_str()];
    let process_id = if ist_storno {
        spawn_process::<WimInvoicWorkflow>(
            &state.store,
            state.tenant_id,
            workflow_id,
            command,
            &[],
            &keys,
        )
        .await?
    } else {
        spawn_process::<W>(
            &state.store,
            state.tenant_id,
            workflow_id,
            command,
            &[],
            &keys,
        )
        .await?
    };
    Ok(DispatchOutcome::Spawned { process_id })
}
