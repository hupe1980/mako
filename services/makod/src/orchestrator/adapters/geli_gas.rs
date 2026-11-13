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
/// **Message format**: ORDERS (AWH Sperrprozesse Gas; GeLi Gas 3.0
/// BK7-24-01-009 holds no Sperrprozess). The GNB's answer window is the
/// 17115 / 17117 row of `mako_fristen::antwort`; the APERAK is the Gas
/// Verarbeitbarkeitsfehlermeldung of APERAK AHB 1.1 Kap. 2.3.1.
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
pub fn geli_gas_registry(
    received_at: time::OffsetDateTime,
) -> AdapterRegistry<GeliGasSupplierChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
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
            // 44021 answers the LF's Änderungsmeldung zur Bestandsliste 44020
            // (UTILMD AHB Gas 1.2 Kap. 5.1, S. 15, NB an LF). mako sends no 44020, so
            // no LF-side process waits for it; refuse it by name rather than
            // open it as an Anfrage.
            if pid.as_u32() == 44_021 {
                return Err(EngineError::Deserialization(
                    "GeLi Gas adapter: PID 44021 answers an Änderungsmeldung zur \
                     Bestandsliste 44020; mako implements no LF-side Änderungsmeldung, so \
                     there is no open 44020 for it to answer"
                        .into(),
                ));
            }
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
                        // Meldepunkt, not in the Strom `Z16`/`Z17` pair.
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
                // `SG4 DTM+92` „Beginn zum" — the Lieferbeginn of an
                // Anmeldung — or `DTM+93` „Ende zum", the Lieferende of an
                // Abmeldung (55004 / 44004), which carries no `92`. `163` is
                // an MSCONS qualifier. Reading the wrong one yields an empty
                // `process_date`, which `processd` cannot decide on and the
                // answer cannot restate.
                process_date: {
                    let first = u.transactions().first();
                    super::prozesstag(
                        super::sg4_dtm(first, edi_energy::utilmd_codes::dtm::BEGINN_ZUM).or_else(
                            || super::sg4_dtm(first, edi_energy::utilmd_codes::dtm::ENDE_ZUM),
                        ),
                        super::utilmd_sparte(u),
                    )?
                    .unwrap_or_default()
                },
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
                received_at,
                // L1/N1: extract Bilanzierungsmethode (TM+EM) and Fallgruppe (TM+Z10)
                bilanzierungsmethode: extract_bilanzierungsmethode(u.segments()),
                fallgruppe: extract_fallgruppe(u.segments()),
                // The Sparte-neutral `SG4` contract `processd` walks: the
                // Transaktionsgrund *and* its Ergänzung, the Vorgangsnummer,
                // `DTM+154` and `DTM+471`. Reading the Grund alone escalated
                // every Gas answer at Prüfschritt 10.
                vorgang: Box::new(super::lf_vorgangsdaten(u)?),
                // No gas-quality characteristic exists in UTILMD G; see
                // `extract_gasqualitaet`. Always `None` until an AHB defines one.
                gasqualitaet: extract_gasqualitaet(u.segments()).map(str::to_owned),
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for the LF's answer to an Anfrage the GNB sent.
///
/// Adapts an inbound UTILMD G 44008/44009, 44011/44012 or 44014/44015 (LF → NB,
/// answering the GNB's 44007, 44010 or 44013) to
/// [`GasSupplierChangeCommand::ReceiveGnbAntwort`]. The Prüfidentifikator says
/// Bestätigung or Ablehnung ([`mako_geli_gas::gnb_antwort_accepted`]); the
/// `SG4 STS+E01` DE 9013 is kept raw, so a code its tree does not publish is
/// still recorded — the workflow names the tree, `E_3002` / `E_3020` /
/// `E_3008`, from the Anfrage it sent, because the Gas MIG carries no DE 1131.
/// The `FTX+ACB` Bemerkung rides as the reason.
///
/// Built as an Anfrage instead (`ReceiveUtilmd`), the answer is refused inside
/// the workflow and the GNB's process never records what the LF said.
#[must_use]
pub fn geli_gas_gnb_antwort_registry() -> AdapterRegistry<GeliGasSupplierChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas GNB-Antwort adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas GNB-Antwort adapter: expected UTILMD G message \
                     (PIDs 44008/44009, 44011/44012, 44014/44015)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas GNB-Antwort adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let accepted = mako_geli_gas::gnb_antwort_accepted(pid.as_u32()).ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "GeLi Gas GNB-Antwort adapter: unexpected PID {pid} \
                     (expected 44008/44009, 44011/44012, 44014/44015)"
                ))
            })?;
            let first_tx = u.transactions().first();
            Ok(GasSupplierChangeCommand::ReceiveGnbAntwort {
                response_pid: pid,
                accepted,
                reason: first_tx
                    .and_then(|tx| tx.ftx.first())
                    .and_then(|f| f.text.clone()),
                antwort_code: first_tx.and_then(|tx| tx.antwort()).map(|a| a.code.clone()),
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
/// Build an [`AdapterRegistry`] for `GeliGasDatenabrufWorkflow` — ORDERS 17103/17104 receive.
///
/// NB-side: receives inbound ORDERS from LF requesting Brennwert/Zustandszahl.
#[must_use]
pub fn geli_gas_datenabruf_receive_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasDatenabrufWorkflow> {
    use mako_geli_gas::datenabruf::{GeliGasDatenabrufCommand, ORDERS_ANFRAGE_PIDS};

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
            Ok(GeliGasDatenabrufCommand::ReceiveAnfrage {
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

/// Build an [`AdapterRegistry`] for `GeliGasDatenabrufWorkflow` — ORDRSP 19103/19104.
///
/// LF-side: receives ORDRSP rejection from NB after sending ORDERS 17103.
#[must_use]
pub fn geli_gas_datenabruf_ablehnung_registry()
-> AdapterRegistry<mako_geli_gas::GeliGasDatenabrufWorkflow> {
    use mako_geli_gas::datenabruf::{GeliGasDatenabrufCommand, ORDRSP_ABLEHNUNG_PIDS};

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
            Ok(GeliGasDatenabrufCommand::ReceiveAblehnung {
                pid,
                sender: MarktpartnerCode::new(sender_mp_id),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// Acceptance is determined by PID alone: the Bestätigungen 44002, 44005 and
/// 44017 accept, the Ablehnungen 44003, 44006 and 44018 refuse
/// (`LF_ANMELDUNG_BESTAETIGUNG_PIDS`). The rejection reason is extracted from the
/// first transaction's FTX segment, the answer code from its `STS+E01`.
/// Regulatory basis: BK7-24-01-009, GeLi Gas (Lf-only deployment role).
#[must_use]
pub fn geli_gas_lf_anmeldung_registry() -> AdapterRegistry<mako_geli_gas::GeliGasLfAnmeldungWorkflow>
{
    use mako_geli_gas::{
        GeliGasLfAnmeldungCommand, LF_ANMELDUNG_ANTWORT_PIDS, LF_ANMELDUNG_BESTAETIGUNG_PIDS,
    };

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
                    "GeLi Gas LF-Anmeldung adapter: expected UTILMD G message (PIDs 44002/44003, 44005/44006, 44017/44018)"
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
                    "GeLi Gas LF-Anmeldung adapter: unexpected PID {pid} (expected 44002/44003, 44005/44006, 44017/44018)"
                )));
            }

            let accepted = LF_ANMELDUNG_BESTAETIGUNG_PIDS.contains(&pid.as_u32());
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());

            // `SG4 STS+E01`: the code is what the answer means, the
            // Codeliste which tree defines it.
            let antwort = u
                .transactions()
                .first()
                .and_then(|tx| tx.antwort())
                .map(|a| mako_engine::envelope::Antwortcode::new(a.code.clone(), a.codeliste.clone()));

            Ok(GeliGasLfAnmeldungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
                antwort,
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
/// Routes UTILMD G 44022 „Anfrage nach Stornierung" (LFN/LFA → GNB). The
/// GNB's answer is a UTILMD of its own — 44023 Bestätigung / 44024 Ablehnung
/// Anfrage Stornierung, „zurück an den Absender" (UTILMD AHB Gas 1.2 Kap. 5.2)
/// — which the workflow queues and the UTILMD renderer writes.
///
/// `BGM` DE 1001 is read into the command because the answer repeats it
/// (Bedingung `[500]`: the category of the cancelled message).
///
/// **APERAK:** only a Verarbeitbarkeitsfehlermeldung, nächster Werktag 12 Uhr
/// for a Folgeprozess (APERAK AHB 1.1 Kap. 2.3 / Kap. 2.3.1).
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
                         validation was vacuous. Import the profile with `cargo xtask import-profiles`.",
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
                document_code: u
                    .bgm()
                    .map(|b| b.document_code.clone())
                    .unwrap_or_default(),
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

            // The Prüfidentifikator is `SG1 RFF+Z13`; `BGM` DE 1004 is the
            // Rechnungsnummer.
            let pid = edi_energy::EdiEnergyMessage::detect_pruefidentifikator(inv)
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrprozesse INVOIC adapter: no Prüfidentifikator in the \
                         INVOIC ({e})"
                    ))
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
            let (zaehlerstaende, _) = super::mscons_zaehlerstaende(m);
            if undated > 0 {
                tracing::warn!(
                    pid = pid.as_u32(),
                    undated,
                    "Gas MSCONS: readings skipped — neither SG10 DTM+163/164 nor DTM+7 in format 303",
                );
            }

            Ok(GasMsconsDatenCommand::ReceiveMscons {
                pid,
                sender,
                location_id,
                message_ref,
                reads,
                zaehlerstaende,
                validation_passed,
                validation_errors,
                brennwert_kwh_per_m3: extract_qty_z08(m),
                zustandszahl: extract_qty_z10(m),
                // The MSCONS AHB carries no gas quality type.
                gasqualitaet: None,
                nachrichtendatum: m
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
            })
        },
    ));
    registry
}

/// The master-data groups an Anfrage zur Stammdatenänderung filled, read off
/// its Vorgang by the qualifiers UTILMD AHB Gas 1.2 Kap. 5.12 gives each
/// group: in the 44139/44140 columns (Kap. 5.12.1) `SG6 DTM+Z21`/`Z22`/`752`,
/// the `SG10 CCI` of `SEQ+Z01` and `SEQ+Z18`, the `SEQ+Z02`/`Z07`/`Z12`
/// groups, `SG12 NAD+DP`; in the 44137 (Kap. 5.12.4) `SG6 DTM+Z20`, `SG10
/// CCI+++Z15`/`Z18`, `SG12 NAD+Z09`/`Z04`; in the 44150 (Kap. 5.12.5) `SG10
/// CCI+Z19`. The Antwort carries exactly these (Bedingung `[336]`).
fn angefragte_gruppen(
    tx: &edi_energy::messages::utilmd::UtilmdTransaction,
) -> Vec<mako_geli_gas::AngefragteGruppe> {
    use mako_geli_gas::AngefragteGruppe as G;
    let mut out = Vec::new();
    let mut push = |g: G| {
        if !out.contains(&g) {
            out.push(g);
        }
    };
    for d in &tx.dtm {
        match d.qualifier.as_str() {
            "Z21" => push(G::TerminNetznutzungsabrechnung),
            "Z22" => push(G::Netznutzungsabrechnungsintervall),
            "752" => push(G::Turnusablesung),
            "Z20" => push(G::AbrechnungsintervallLf),
            _ => {}
        }
    }
    for seq in &tx.sequences {
        match seq.seq.action.as_deref().unwrap_or_default() {
            "Z02" => push(G::ObisDaten),
            "Z07" => push(G::Konzessionsabgabe),
            "Z12" => push(G::Gemeinderabatt),
            _ => {}
        }
        for m in &seq.characteristics {
            match m.cci.category.as_deref() {
                Some("Z21") => push(G::Marktgebiet),
                Some("Z19") => push(G::Bilanzkreis),
                _ => {}
            }
            match m.cci.characteristic_id.as_deref() {
                Some("Y01") => push(G::Druckebene),
                Some("Z15" | "Z18") => push(G::Gruppenzuordnung),
                Some("Z88") => push(G::Netznutzung),
                Some("ZB3") => push(G::ZugeordneteMarktpartner),
                Some("Y02") => push(G::Gasqualitaet),
                _ => {}
            }
        }
    }
    for p in &tx.parties {
        match p.nad.qualifier.as_str() {
            "DP" => push(G::Marktlokationsanschrift),
            "Z09" => push(G::KundeDesLf),
            "Z04" => push(G::KorrespondenzanschriftKundeLf),
            _ => {}
        }
    }
    out
}

/// Build an [`AdapterRegistry`] for [`mako_geli_gas::GeliGasStammdatenaenderungWorkflow`].
///
/// Adapts inbound UTILMD G GeLi Gas Stammdatenänderung: an **Änderung** PID →
/// [`mako_geli_gas::GasStammdatenCommand::ReceiveAenderung`] (apply on
/// Zustimmung, respecting the Monatserster rule for bila.rel. changes); an
/// **Antwort** PID → [`mako_geli_gas::GasStammdatenCommand::ReceiveAntwort`].
/// Structural validation only (no AHB rulepack).
#[must_use]
pub fn geli_gas_stammdaten_registry(
    received_at: time::OffsetDateTime,
) -> AdapterRegistry<mako_geli_gas::GeliGasStammdatenaenderungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
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
            // `SG4 DTM+157` „Änderung zum" — the Stammdatenänderung's own date,
            // and what makod's renderer writes for this PID family. `163` is an
            // MSCONS qualifier and appears in no UTILMD Anwendungsfall.
            let aenderungsdatum = super::prozesstag(
                super::sg4_dtm(first_tx, edi_energy::utilmd_codes::dtm::AENDERUNG_ZUM),
                super::utilmd_sparte(u),
            )?
            .unwrap_or_default();

            let pid_u32 = pid.as_u32();
            if mako_geli_gas::stammdatenaenderung::is_antwort_pid(pid_u32)
                && !mako_geli_gas::stammdatenaenderung::is_aenderung_pid(pid_u32)
            {
                // `SG4 STS+E01` — the code and the Codeliste the Antwort column
                // names, as they arrived. An Antwort without one is not read as
                // a Zustimmung: the code is what the counterparty said.
                let antwort = first_tx.and_then(|t| t.antwort()).ok_or_else(|| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Stammdaten adapter: Antwort {} carries no SG4 STS+E01 \
                         (UTILMD AHB Gas 1.2 Kap. 5.10–5.11, Muss)",
                        pid.as_u32()
                    ))
                })?;
                return Ok(mako_geli_gas::GasStammdatenCommand::ReceiveAntwort {
                    response_pid: pid,
                    antwort_code: antwort.code.clone(),
                    antwort_codeliste: antwort.codeliste.clone(),
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

            // Anfrage zur Stammdatenänderung (G8–G10): we are the
            // Verantwortlicher. The process keeps what the Antwort restates —
            // both SG4 dates, the Grund, the Vorgangsnummer — and which groups
            // the Anfrage filled (UTILMD AHB Gas 1.2 Kap. 5.12.1, `[528]`,
            // `[336]`).
            if mako_geli_gas::stammdatenaenderung::is_anfrage_request_pid(pid_u32) {
                let vertragsbeginn = super::prozesstag(
                    super::sg4_dtm(first_tx, edi_energy::utilmd_codes::dtm::BEGINN_ZUM),
                    super::utilmd_sparte(u),
                )?;
                return Ok(mako_geli_gas::GasStammdatenCommand::ReceiveAnfrage {
                    pid,
                    sender,
                    receiver,
                    location_id,
                    aenderungsdatum,
                    vertragsbeginn,
                    transaktionsgrund: first_tx
                        .and_then(|t| t.transaktionsgrund())
                        .map(|g| g.grund),
                    vorgangsnummer: first_tx.and_then(|t| t.vorgangsnummer()).map(str::to_owned),
                    angefragt: first_tx.map(angefragte_gruppen).unwrap_or_default(),
                    message_ref: MessageRef::new(msg.message_ref()),
                    validation_passed,
                    validation_errors,
                    received_at,
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

            // `SG4 DTM+92` Datum Vertragsbeginn, Muss beside the `157` on every
            // Änderung column, and the `IDE+24` the Antwort names in `SG6
            // RFF+TN` (UTILMD AHB Gas 1.2 Kap. 5.10–5.11).
            let vertragsbeginn = super::prozesstag(
                super::sg4_dtm(first_tx, edi_energy::utilmd_codes::dtm::BEGINN_ZUM),
                super::utilmd_sparte(u),
            )?;
            Ok(mako_geli_gas::GasStammdatenCommand::ReceiveAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                patch: serde_json::Value::Object(patch),
                vertragsbeginn,
                vorgangsnummer: first_tx.and_then(|t| t.vorgangsnummer()).map(str::to_owned),
                dokumentenart: u.bgm().map(|b| b.document_code.clone()),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
                received_at,
            })
        },
    ));
    registry
}

// ── Geschäftsdatenanfrage Gas (ORDERS 17101 / 17126 → UTILMD / ORDRSP 19101) ─

/// The correlation key a Gas Geschäftsdatenanfrage message reaches its process
/// by: an inbound ORDERS 17101 / 17126 opens the NB's side under the
/// Anfragender's `NAD+MS` with its Anfrage-Nummer; an answer resumes the
/// Anfrage this party sent, keyed on the number alone. The number is read as
/// on the Strom side ([`super::gda_anfrage_nummer`]): `BGM` DE 1004 of the
/// ORDERS, `SG6 RFF+AAV` of the UTILMD, `SG1 RFF+ON` of the ORDRSP.
#[must_use]
pub fn gda_gas_process_key(msg: &AnyMessage) -> String {
    use mako_geli_gas::geschaeftsdatenanfrage::{anfragender_key, netzbetreiber_key};
    let nummer = super::gda_anfrage_nummer(msg);
    if nummer.is_empty() {
        return nummer;
    }
    match msg {
        AnyMessage::Orders(o) => netzbetreiber_key(
            o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
            &nummer,
        ),
        _ => anfragender_key(&nummer),
    }
}

/// Build an [`AdapterRegistry`] for the Gas Geschäftsdatenanfrage.
///
/// - ORDERS **17101 / 17126** (at the NB) → `ReceiveAnfrage`, with the AHB
///   verdict, `NAD+MS`/`NAD+MR` and the `SG2 LOC+172` Lokation.
/// - UTILMD **44035 / 44060** (at the Anfragender) → `ReceiveAntwort`.
/// - ORDRSP **19101** (at the Anfragender) → `ReceiveAblehnung`, with the
///   `SG2 AJT` code and its DE 1082 Codeliste.
#[must_use]
pub fn geli_gas_geschaeftsdatenanfrage_registry()
-> AdapterRegistry<mako_geli_gas::geschaeftsdatenanfrage::GeliGasGeschaeftsdatenanfrageWorkflow> {
    use mako_geli_gas::geschaeftsdatenanfrage::GdaGasCommand;
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Geschäftsdatenanfrage adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Geschäftsdatenanfrage adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let message_ref = MessageRef::new(msg.message_ref());
            match msg {
                AnyMessage::Orders(o) => {
                    let (validation_passed, validation_errors) = super::ahb_verdict(msg);
                    let lokation_id = o
                        .segments()
                        .iter()
                        .find(|s| s.tag == "LOC" && s.component_str(0, 0) == Some("172"))
                        .and_then(|s| s.component_str(1, 0))
                        .unwrap_or("")
                        .to_owned();
                    Ok(GdaGasCommand::ReceiveAnfrage {
                        pid,
                        anfragender: MarktpartnerCode::new(
                            o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                        ),
                        netzbetreiber: MarktpartnerCode::new(
                            o.receiver()
                                .and_then(|n| n.party_id.as_deref())
                                .unwrap_or(""),
                        ),
                        lokation_id,
                        anfrage_nummer: MessageRef::new(super::gda_anfrage_nummer(msg)),
                        message_ref,
                        validation_passed,
                        validation_errors,
                    })
                }
                AnyMessage::Utilmd(_) => Ok(GdaGasCommand::ReceiveAntwort {
                    antwort_pid: pid,
                    message_ref,
                }),
                AnyMessage::Ordrsp(o) => {
                    let ajt = o.segments().iter().find(|s| s.tag == "AJT");
                    let element = |e: usize| {
                        ajt.and_then(|s| s.component_str(e, 0))
                            .filter(|c| !c.is_empty())
                            .map(str::to_owned)
                    };
                    Ok(GdaGasCommand::ReceiveAblehnung {
                        antwortcode: element(0),
                        codeliste: element(1),
                        message_ref,
                    })
                }
                _ => Err(EngineError::Deserialization(
                    "GeLi Gas Geschäftsdatenanfrage adapter: expected ORDERS, UTILMD or ORDRSP"
                        .into(),
                )),
            }
        },
    ));
    registry
}
