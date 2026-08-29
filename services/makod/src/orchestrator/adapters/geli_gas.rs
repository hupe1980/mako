//! GeLi Gas adapter registries.
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
// ── GeLi Gas ORDERS Sperrung NB-side (PIDs 17115, 17116, 17117) ──────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrungNbWorkflow`].
///
/// Extracts ORDERS fields from an inbound Gas-Sperrauftrag / Gas-Entsperrauftrag
/// (PIDs 17115/17116/17117) to construct a [`GasSperrungNbCommand::ReceiveSperrung`].
///
/// **Message format**: ORDERS (AWH Sperrprozesse Gas, BK7-24-01-009).
/// **APERAK Frist:** 10 Werktage.
#[must_use]
pub fn geli_gas_sperrung_nb_registry() -> AdapterRegistry<GeliGasSperrungNbWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung NB adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung NB adapter: expected ORDERS message (PIDs 17115/17116/17117)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung NB adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // Marktlokation from the LOC segment (element 1, component 0).
            let location_id = mako_engine::types::MaLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "LOC")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            Ok(GasSperrungNbCommand::ReceiveSperrung {
                pid,
                sender: mako_engine::types::MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: mako_engine::types::MarktpartnerCode::new(
                    o.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                location_id,
                document_date: o
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: mako_engine::types::MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GeLi Gas Sperrung NB — gMSB response (ORDRSP 19118/19119) ─────────────────

/// Build an [`AdapterRegistry`] for ORDRSP 19118/19119 routed to
/// [`GeliGasSperrungNbWorkflow`] (GNB side).
///
/// After the GNB forwards the Anfrage Sperrung to the gMSB, the gMSB answers with
/// ORDRSP 19118 (Bestätigung) or 19119 (Ablehnung); `makod` resumes the process
/// with [`GasSperrungNbCommand::ReceiveMsbAntwort`]. Per-Sparte duplicate of
/// `gpke_sperrung_msb_response_registry`.
#[must_use]
pub fn geli_gas_sperrung_nb_response_registry() -> AdapterRegistry<GeliGasSperrungNbWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung NB response adapter".into(),
                )
            })?;
            let AnyMessage::Ordrsp(_) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung NB response adapter: expected ORDRSP message \
                     (PIDs 19118/19119)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung NB response adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            // 19118 = Bestätigung (gMSB confirms meter access), 19119 = Ablehnung.
            Ok(GasSperrungNbCommand::ReceiveMsbAntwort {
                pid,
                is_confirmed: pid.as_u32() == 19118,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GeLi Gas Sperrung NB — LF Stornierung (ORDCHG 39000) ─────────────────────

/// Build an [`AdapterRegistry`] for ORDCHG 39000 routed to
/// [`GeliGasSperrungNbWorkflow`] (GNB side).
///
/// The LFG cancels a pending Gas-Sperrauftrag with ORDCHG 39000; `makod` resumes
/// the GNB-side process with [`GasSperrungNbCommand::ReceiveStornierung`]. ORDCHG
/// carries no LOC — correlated by the original order reference (RFF+ON). Per-Sparte
/// duplicate of `gpke_sperrung_stornierung_registry`.
#[must_use]
pub fn geli_gas_sperrung_nb_stornierung_registry() -> AdapterRegistry<GeliGasSperrungNbWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Ordchg(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung Stornierung adapter: expected ORDCHG message (PID 39000)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung Stornierung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            Ok(GasSperrungNbCommand::ReceiveStornierung {
                pid,
                sender: MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GeLi Gas Lieferantenwechsel (PIDs 44001–44006, 44017–44018) ──────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSupplierChangeWorkflow`].
#[must_use]
pub fn geli_gas_registry() -> AdapterRegistry<GeliGasSupplierChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GeLi Gas adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(GasSupplierChangeCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                malo_id: MaLo::new(
                    u.transactions()
                        .first()
                        // UTILMD Gas names every Lokation in `SG5 LOC+172`
                        // Meldepunkt, not in the Strom `Z16`/`Z17` pair —
                        // reading only those left `malo_id` empty on every
                        // message a real Gas counterparty sends.
                        .and_then(|t| t.lokation())
                        .unwrap_or(""),
                ),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.dtm.iter().find(|d| d.is_period_start()))
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
                received_at: time::OffsetDateTime::now_utc(),
                // L1/N1: extract Bilanzierungsmethode (TM+EM) and Fallgruppe (TM+Z10)
                bilanzierungsmethode: extract_bilanzierungsmethode(u.segments()),
                fallgruppe: extract_fallgruppe(u.segments()),
                // The Sparte-neutral `SG4` contract `processd` walks: the
                // Transaktionsgrund *and* its Ergänzung, the Vorgangsnummer,
                // `DTM+154` and `DTM+471`. Reading the Grund alone escalated
                // every Gas answer at Prüfschritt 10.
                vorgang: super::lf_vorgangsdaten(u),
                // No gas-quality characteristic exists in UTILMD G; see
                // `extract_gasqualitaet`. Always `None` until an AHB defines one.
                gasqualitaet: extract_gasqualitaet(u.segments()).map(str::to_owned),
            })
        },
    ));
    registry
}

// ── GeLi Gas Informationsmeldungen (PIDs 44036–44038) ────────────────────────

/// Build an [`AdapterRegistry`] for
/// [`mako_geli_gas::GeliGasZuordnungsmeldungWorkflow`].
///
/// Adapts an inbound UTILMD 44036 / 44037 / 44038 (NB → LFN/LFA/LFZ) to
/// [`mako_geli_gas::zuordnungsmeldung::ZuordnungsmeldungCommand::Empfangen`].
///
/// „Eine Informationsmeldung ist eine Nachricht, für die keine Antwort
/// vorgesehen ist" (UTILMD AHB Gas Kap. 5.8), so nothing here derives a
/// response PID or a business deadline — only the `SG4 STS+7` Grund, which is
/// the whole content of the message.
#[must_use]
pub fn geli_gas_zuordnungsmeldung_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasZuordnungsmeldungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Informationsmeldung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Informationsmeldung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Informationsmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(
                mako_geli_gas::zuordnungsmeldung::ZuordnungsmeldungCommand::Empfangen {
                    pid,
                    sender: MarktpartnerCode::new(
                        u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                    ),
                    receiver: MarktpartnerCode::new(
                        u.receiver()
                            .and_then(|n| n.party_id.as_deref())
                            .unwrap_or(""),
                    ),
                    location_id: MaLo::new(
                        u.transactions()
                            .first()
                            .and_then(|t| t.lokation())
                            .unwrap_or(""),
                    ),
                    transaktionsgrund: u
                        .transactions()
                        .first()
                        .and_then(|t| t.transaktionsgrund())
                        .map(|g| g.grund)
                        .unwrap_or_default(),
                    message_ref: MessageRef::new(msg.message_ref()),
                    validation_passed,
                    validation_errors,
                },
            )
        },
    ));
    registry
}

// ── GeLi Gas PARTIN Kommunikationsdaten (PIDs 37008–37014) ───────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasPartinWorkflow`].
///
/// Handles all inbound Gas PARTIN messages with PIDs 37008–37014
/// (Gas Kommunikationsdaten). Produces
/// [`GasKommunikationsdatenCommand::ReceivePartin`].
#[must_use]
pub fn geli_gas_partin_registry() -> AdapterRegistry<GeliGasPartinWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas PARTIN adapter".into(),
                )
            })?;
            let AnyMessage::Partin(p) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas PARTIN adapter: expected PARTIN message (PIDs 37008–37014)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas PARTIN adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            Ok(GasKommunikationsdatenCommand::ReceivePartin {
                pid,
                sender: MarktpartnerCode::new(
                    p.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                document_date: p
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GeLi Gas Stornierung LF side (PIDs 44023–44024) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasLfStornierungWorkflow`].
///
/// Handles inbound PIDs 44023/44024 (Bestätigung / Ablehnung Stornierung)
/// from GNB → LF. Produces [`LfStornierungCommand::HandleAntwort`].
///
/// Build an [`AdapterRegistry`] for `GeliGasDatanabrufWorkflow` — ORDERS 17103/17104 receive.
///
/// NB-side: receives inbound ORDERS from LF requesting Brennwert/Zustandszahl.
#[must_use]
pub fn geli_gas_datenabruf_receive_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasDatanabrufWorkflow> {
    use mako_geli_gas::datenabruf::{GeliGasDatanabrufCommand, ORDERS_ANFRAGE_PIDS};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Datenabruf receive adapter".into(),
                )
            })?;
            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Datenabruf receive adapter: expected ORDERS message (PIDs 17103/17104)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Datenabruf receive adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            if !ORDERS_ANFRAGE_PIDS.contains(&pid.as_u32()) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas Datenabruf receive adapter: unexpected PID {pid}"
                )));
            }
            Ok(GeliGasDatanabrufCommand::ReceiveAnfrage {
                pid,
                sender: MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    o.receiver().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for `GeliGasDatanabrufWorkflow` — ORDRSP 19103/19104.
///
/// LF-side: receives ORDRSP rejection from NB after sending ORDERS 17103.
#[must_use]
pub fn geli_gas_datenabruf_ablehnung_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasDatanabrufWorkflow> {
    use mako_geli_gas::datenabruf::{GeliGasDatanabrufCommand, ORDRSP_ABLEHNUNG_PIDS};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Datenabruf Ablehnung adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Datenabruf Ablehnung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            if !ORDRSP_ABLEHNUNG_PIDS.contains(&pid.as_u32()) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas Datenabruf Ablehnung adapter: unexpected PID {pid}"
                )));
            }
            // ORDRSP sender is the NB/MSB rejecting our request.
            let sender_mp_id = match msg {
                AnyMessage::Orders(o) => o
                    .sender()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or("")
                    .to_owned(),
                _ => String::new(),
            };
            Ok(GeliGasDatanabrufCommand::ReceiveAblehnung {
                pid,
                sender: MarktpartnerCode::new(sender_mp_id),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// Acceptance is determined by PID alone: 44023 = accepted, 44024 = rejected.
/// The rejection reason is extracted from the first transaction's FTX segment.
/// Regulatory basis: BK7-24-01-009, GeLi Gas (Lf-only deployment role).
#[must_use]
pub fn geli_gas_lf_anmeldung_registry() -> AdapterRegistry<mako_geli_gas::GeliGasLfAnmeldungWorkflow>
{
    use mako_geli_gas::{GeliGasLfAnmeldungCommand, LF_ANMELDUNG_ANTWORT_PIDS};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas LF-Anmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas LF-Anmeldung adapter: expected UTILMD G message (PIDs 44003–44006)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas LF-Anmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            if !LF_ANMELDUNG_ANTWORT_PIDS.contains(&pid.as_u32()) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas LF-Anmeldung adapter: unexpected PID {pid} (expected 44003–44006)"
                )));
            }

            // PID 44003/44005 = Bestätigung (accepted), 44004/44006 = Ablehnung (rejected).
            let accepted = matches!(pid.as_u32(), 44003 | 44005);
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());

            Ok(GeliGasLfAnmeldungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// The rejection reason is extracted from the first transaction's FTX segment.
/// Regulatory basis: BK7-24-01-009, GeLi Gas (Lf-only deployment role).
#[must_use]
pub fn geli_gas_stornierung_lf_registry() -> AdapterRegistry<GeliGasLfStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Stornierung LF adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Stornierung LF adapter: expected UTILMD message (PIDs 44023–44024)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Stornierung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            // PID 44023 = Bestätigung (accepted), 44024 = Ablehnung (rejected).
            let accepted = pid.as_u32() == 44023;
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());
            Ok(LfStornierungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GeLi Gas Stornierung (PIDs 44022–44024) ────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasStornierungWorkflow`].
///
/// Routes UTILMD G messages with PIDs 44022–44024 (GeLi Gas Stornierung):
/// - 44022 — Anfrage nach Stornierung (LFN/LFA → GNB)
/// - 44023 — Bestätigung Stornierung  (GNB response — accepted)
/// - 44024 — Ablehnung Stornierung    (GNB response — rejected)
///
/// **APERAK Frist:** 10 Werktage (BNetzA BK7-24-01-009).
#[must_use]
pub fn geli_gas_stornierung_registry() -> AdapterRegistry<GeliGasStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Stornierung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Stornierung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (mut validation_passed, validation_errors) = super::ahb_verdict(msg);
            // Vacuous-validation guard: warn if AHB profile not yet imported.
            if validation_passed {
                let has_ahb_rules = edi_energy::Pruefidentifikator::new(pid.as_u32())
                    .map(|edi_pid| {
                        edi_energy::registry::ReleaseRegistry::global()
                            .pid_has_ahb_rules(edi_energy::MessageType::Utilmd, edi_pid)
                    })
                    .unwrap_or(false);
                if !has_ahb_rules {
                    tracing::warn!(
                        pid = pid.as_u32(),
                        "GeLi Gas Stornierung adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }
            Ok(GeliGasStornierungCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                vorgang_id: MaLo::new(
                    u.transactions()
                        .first()
                        .and_then(|t| t.vorgangsnummer())
                        .unwrap_or(""),
                ),
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GeLi Gas INVOIC billing (PID 31011 Rechnung sonstige Leistung) ───────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrprozesseInvoicWorkflow`].
///
/// Extracts INVOIC fields to construct a
/// [`InvoicCommand::ReceiveInvoic`] for GeLi Gas AWH
/// billing PID 31011 (Rechnung sonstige Leistung, VNB → LFN/LFA).
///
/// Regulatory basis: BK7-24-01-009 (GeLi Gas 3.0).
#[must_use]
pub fn geli_gas_sperrprozesse_invoic_registry()
-> AdapterRegistry<GeliGasSperrprozesseInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrprozesse INVOIC adapter".into(),
                )
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrprozesse INVOIC adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GeLi Gas Sperrprozesse INVOIC adapter: PID not found in BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            let invoice_ref = inv
                .bgm()
                .and_then(|b| b.document_id.as_deref())
                .unwrap_or(msg.message_ref());

            Ok(InvoicCommand::ReceiveInvoic {
                pid,
                sender: MarktpartnerCode::new(
                    inv.sender()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                recipient: MarktpartnerCode::new(
                    inv.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                invoice_ref: MessageRef::new(invoice_ref),
                document_date: inv
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                validation_passed,
                validation_errors,
                // The BO4E `Rechnung` rides along so `invoicd` can run its
                // plausibility checks straight off the ProcessInitiated
                // payload, as it already does for GPKE and WiM.
                rechnung: Some(Box::new(build_rechnung(inv.segments()))),
                // `SG1 RFF+ACE` and `IMD+7081` — Muss on the wire, and BO4E
                // has no field for either. The first is what `E_0264`
                // Prüfschritt 40 compares against the order on record; the
                // second states which Use-Case a shared PID belongs to.
                bestellung_ref: rff_ace(inv.segments()),
                rechnungstyp: imd_rechnungstyp(inv.segments()),
            })
        },
    ));
    registry
}

// ── GeLi Gas Sperrung LF side (ORDRSP 19116/19117 — GNB → LFG) ──────────────

/// Build an [`AdapterRegistry`] for [`GeliGasSperrungLfWorkflow`].
///
/// Routes ORDRSP 19116 (Bestätigung Gas-Sperr-/Entsperrauftrag, GNB → LFG)
/// and 19117 (Ablehnung) to [`GasSperrungLfCommand::ReceiveOrdrsp`].
///
/// This is a **response adapter** used by the ingest dispatcher to continue
/// the LFG-side process once the GNB responds to the Gas-Sperrauftrag.
///
/// **Loopback use**: in an integrated GNB+LFG deployment (same MP-ID), the
/// outbox ORDRSP 19116/19117 emitted by the GNB side loops back via the
/// [`crate::ingest_dispatcher`] to complete the LFG process.
#[must_use]
pub fn geli_gas_sperrung_lf_registry() -> AdapterRegistry<GeliGasSperrungLfWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung LF adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung LF adapter: expected ORDRSP message \
                     (PIDs 19116/19117/19128/19129)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let sender =
                MarktpartnerCode::new(o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""));
            let message_ref = MessageRef::new(msg.message_ref());

            match pid.as_u32() {
                // Bestätigung/Ablehnung of the LFG's Gas-Sperrauftrag (ORDERS 17115).
                19116 | 19117 => Ok(GasSperrungLfCommand::ReceiveOrdrsp {
                    pid,
                    is_confirmed: pid.as_u32() == 19116,
                    message_ref,
                    sender,
                    reason: None,
                }),
                // Bestätigung/Ablehnung of the LFG's Stornierung (ORDCHG 39000).
                19128 | 19129 => Ok(GasSperrungLfCommand::ReceiveStornoOrdrsp {
                    pid,
                    is_confirmed: pid.as_u32() == 19128,
                    message_ref,
                    sender,
                }),
                other => Err(EngineError::Deserialization(format!(
                    "GeLi Gas Sperrung LF adapter: unexpected ORDRSP PID {other} \
                     (expected 19116/19117/19128/19129)"
                ))),
            }
        },
    ));
    registry
}

// ── GeLi Gas MSCONS data delivery (PIDs 13002, 13007–13009) ─────────────────

/// Build an [`AdapterRegistry`] for [`GeliGasMsconsWorkflow`].
///
/// Handles inbound Gas MSCONS metered-data messages from NB/MSB to LFG.
/// PIDs 13002, 13007–13009 (GeLi Gas 2.0 + WiM Gas data delivery per GeLi Gas 3.0).
///
/// Note: PIDs 13013/13014 (MMM Strom/Gas Allokationsliste) are NOT handled here —
/// they belong to `gpke-allokationsliste` and have their own registry.
#[must_use]
pub fn geli_gas_mscons_registry() -> AdapterRegistry<GeliGasMsconsWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas MSCONS adapter".into(),
                )
            })?;

            let AnyMessage::Mscons(m) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas MSCONS adapter: expected MSCONS message (PIDs 13002, 13007–13009)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas MSCONS adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            let sender = mako_engine::types::MarktpartnerCode::new(
                m.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
            );
            let message_ref = mako_engine::types::MessageRef::new(msg.message_ref());
            // `SG5 NAD` carries the MaLo. Without it `edmd` refuses the event
            // before it reaches either the interval parser or the PID 13007
            // Brennwert branch, so the Gasbeschaffenheit this adapter already
            // extracted never arrived either.
            let location_id = mako_engine::types::MaLo::new(
                m.delivery_points()
                    .first()
                    .and_then(|dp| {
                        dp.time_series
                            .iter()
                            .find(|ts| ts.loc.qualifier == "172")
                            .and_then(|ts| ts.loc.location_id.as_deref())
                            .or(dp.nad.party_id.as_deref())
                    })
                    .unwrap_or(""),
            );
            let (reads, undated) = super::mscons_intervals(m);
            if undated > 0 {
                tracing::warn!(
                    pid = pid.as_u32(),
                    undated,
                    "Gas MSCONS: readings skipped — no SG10 DTM+163/164 in format 303",
                );
            }

            Ok(GasMsconsDatenCommand::ReceiveMscons {
                pid,
                sender,
                location_id,
                message_ref,
                reads,
                validation_passed,
                validation_errors,
                brennwert_kwh_per_m3: extract_qty_z08(m),
                zustandszahl: extract_qty_z10(m),
                // H2-readiness: gas quality type not yet in MSCONS AHB — None until standardized
                gasqualitaet: None,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`mako_geli_gas::GeliGasStammdatenaenderungWorkflow`].
///
/// Adapts inbound UTILMD G GeLi Gas Stammdatenänderung: an **Änderung** PID →
/// [`mako_geli_gas::GasStammdatenCommand::ReceiveAenderung`] (apply on
/// Zustimmung, respecting the Monatserster rule for bila.rel. changes); an
/// **Antwort** PID → [`mako_geli_gas::GasStammdatenCommand::ReceiveAntwort`].
/// Structural validation only (no AHB rulepack).
#[must_use]
pub fn geli_gas_stammdaten_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasStammdatenaenderungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Stammdaten adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Stammdaten adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Stammdaten adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let sender =
                MarktpartnerCode::new(u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""));
            let receiver = MarktpartnerCode::new(
                u.receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );
            let first_tx = u.transactions().first();
            // `SG5 LOC+172` Meldepunkt — the Gas qualifier; see the
            // Lieferantenwechsel adapter above.
            let location_id = MaLo::new(first_tx.and_then(|t| t.lokation()).unwrap_or(""));
            let aenderungsdatum = first_tx
                .and_then(|t| t.dtm.iter().find(|d| d.is_period_start()))
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();

            let pid_u32 = pid.as_u32();
            if mako_geli_gas::stammdatenaenderung::is_antwort_pid(pid_u32)
                && !mako_geli_gas::stammdatenaenderung::is_aenderung_pid(pid_u32)
            {
                let antwort = match u
                    .transactions()
                    .first()
                    .and_then(|t| {
                        t.sts
                            .iter()
                            .find(|s| s.category.as_deref() == Some("E01"))
                            .and_then(|s| s.status_code.clone())
                    })
                    .as_deref()
                {
                    Some("E13") => mako_geli_gas::GasAntwort::AblehnungBilanzierung,
                    Some("E17") => mako_geli_gas::GasAntwort::AblehnungFrist,
                    _ => mako_geli_gas::GasAntwort::Zustimmung,
                };
                return Ok(mako_geli_gas::GasStammdatenCommand::ReceiveAntwort {
                    response_pid: pid,
                    antwort,
                });
            }

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // A Stammdatenanfrage **data-return** is an answer, not a change.
            // mako implements only the answering side of the G8–G10 round-trip,
            // so nothing should send us one — and the fall-through below would
            // otherwise apply it as a master-data Änderung. Refuse it loudly:
            // an audited rejection beats a silent wrong write.
            if mako_geli_gas::stammdatenaenderung::is_anfrage_response_pid(pid_u32) {
                return Err(EngineError::Deserialization(format!(
                    "GeLi Gas Stammdaten adapter: PID {pid_u32} is a Stammdatenanfrage \
                     data-return; mako implements no requester side, so there is no open \
                     Anfrage for it to answer"
                )));
            }

            // Stammdatenanfrage (G8–G10): we are the data owner — answer with the
            // requested master data (auto data-return).
            if mako_geli_gas::stammdatenaenderung::is_anfrage_request_pid(pid_u32) {
                return Ok(mako_geli_gas::GasStammdatenCommand::ReceiveAnfrage {
                    pid,
                    sender,
                    receiver,
                    location_id,
                    message_ref: MessageRef::new(msg.message_ref()),
                    validation_passed,
                    validation_errors,
                    received_at: time::OffsetDateTime::now_utc(),
                });
            }

            let segs = u.segments();
            let mut patch = serde_json::Map::new();
            if let Some(b) = extract_bilanzierungsmethode(segs) {
                patch.insert("bilanzierungsmethode".into(), b.into());
            }
            if let Some(q) = extract_gasqualitaet(segs) {
                patch.insert("gasqualitaet".into(), q.into());
            }
            if let Some(n) = super::extract_netzebene(segs) {
                patch.insert("netzebene".into(), n.into());
            }
            if let Some(r) = super::extract_regelzone(segs) {
                patch.insert("regelzone".into(), r.into());
            }
            if let Some(bg) = super::extract_bilanzierungsgebiet(segs) {
                patch.insert("bilanzierungsgebiet".into(), bg.into());
            }

            Ok(mako_geli_gas::GasStammdatenCommand::ReceiveAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                patch: serde_json::Value::Object(patch),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
                received_at: time::OffsetDateTime::now_utc(),
            })
        },
    ));
    registry
}
