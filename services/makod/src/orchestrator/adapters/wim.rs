//! WiM Strom adapter registries (Messstellenbetrieb, ESA Wertebestellung, INSRPT, Preise).
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use mako_engine::types::Sparte;
use time::OffsetDateTime;

use super::*;
// ── WiM INVOIC billing (PID 31009 MSB-Rechnung) ───────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimInvoicWorkflow`].
///
/// Extracts INVOIC fields to construct a [`InvoicCommand::ReceiveInvoic`]
/// for the WiM Strom MSB-Rechnung (PID 31009). This PID is explicitly excluded
/// from `mako-gpke`'s GPKE_INVOIC_PIDS. (The Gas WiM-Rechnung 31003 lives in
/// `mako-wim-gas`, duplicated per Sparte.)
#[must_use]
pub fn wim_invoic_registry() -> AdapterRegistry<WimInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for WiM Rechnung adapter".into())
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Rechnung adapter: expected INVOIC message".into(),
                ));
            };

            let pid = edi_energy::EdiEnergyMessage::detect_pruefidentifikator(inv)
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Rechnung adapter: no Prüfidentifikator in the INVOIC ({e})"
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

// ── WiM billing — REMADV payment advice (PIDs 33001–33004) ────────────────────

/// Build an [`AdapterRegistry`] for REMADV 33001–33004 routed to
/// [`WimInvoicWorkflow`] (MSB invoicer role).
///
/// After the MSB sends INVOIC 31009, the payer (NB/LF/ESA) returns a REMADV:
/// 33001 confirms payment; 33002 non-itemized Abweisung; 33003/33004 the itemized
/// Strom Abweisungen (Kopf+Summe / Position). `makod` resumes the billing process
/// with [`InvoicCommand::ReceiveRemadv`]. Mirrors `gpke_abrechnung_remadv_registry`.
#[must_use]
pub fn wim_invoic_remadv_registry() -> AdapterRegistry<WimInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for WiM REMADV adapter".into())
            })?;
            let AnyMessage::Remadv(r) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM REMADV adapter: expected REMADV message (PIDs 33001–33004)".into(),
                ));
            };
            let pid = r
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM REMADV adapter: PID not found in REMADV BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            Ok(InvoicCommand::ReceiveRemadv {
                pid,
                remadv_ref: MessageRef::new(msg.message_ref()),
                sender: MarktpartnerCode::new(
                    r.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
            })
        },
    ));
    registry
}

// ── WiM billing — COMDIS payment rejection (PID 29001) ────────────────────────

/// Build an [`AdapterRegistry`] for COMDIS 29001 routed to [`WimInvoicWorkflow`].
///
/// After the payer sends a REMADV, the MSB (invoicer) may reject it via COMDIS
/// 29001 (Ablehnung der Zahlung); `makod` resumes with
/// [`InvoicCommand::ReceiveComdis`]. Mirrors `gpke_abrechnung_comdis_registry`.
#[must_use]
pub fn wim_invoic_comdis_registry() -> AdapterRegistry<WimInvoicWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for WiM COMDIS adapter".into())
            })?;
            let AnyMessage::Comdis(_) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM COMDIS adapter: expected COMDIS message (PID 29001)".into(),
                ));
            };
            Ok(InvoicCommand::ReceiveComdis {
                comdis_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── WiM Ersteinbau eines iMS (PIDs 21029, 21030, 21031) ──────────────────────

/// Build an [`AdapterRegistry`] for [`mako_wim::ersteinbau::WimErsteinbauWorkflow`].
///
/// WiM Strom Teil 1 Kap. 3.5. Three IFTSTA Prüfidentifikatoren, and the
/// direction is what separates them: 21029 is the gMSB's Vorabinformation and
/// opens the Vorgang, 21030/21031 are the wMSB's answer under `E_0233` and
/// close it.
///
/// The Antwortcode rides `SG4 STS` on the answer legs. It is read here rather
/// than defaulted, because `E_0233` `A04` („noch keine Aussage möglich") and
/// `A01`/`A02` are all Ablehnungen on the same PID and only the code says
/// which — an answer whose code we cannot read is a `ReceiveAntwort` with the
/// PID's own meaning and nothing more.
#[must_use]
pub fn wim_ersteinbau_registry() -> AdapterRegistry<mako_wim::ersteinbau::WimErsteinbauWorkflow> {
    use mako_wim::ersteinbau::{ABLEHNUNG_PID, ErsteinbauCommand, VORABINFORMATION_PID};

    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Ersteinbau adapter".into(),
                )
            })?;
            let AnyMessage::Iftsta(i) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Ersteinbau adapter: expected IFTSTA (PIDs 21029/21030/21031)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Ersteinbau adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            let sender =
                MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""));
            let receiver = MarktpartnerCode::new(
                i.receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );

            if pid.as_u32() == VORABINFORMATION_PID {
                return Ok(ErsteinbauCommand::ReceiveVorabinformation {
                    pid,
                    gmsb: sender,
                    wmsb: receiver,
                    melo_id: mako_engine::types::MeLo::new(
                        super::iftsta_location(i).unwrap_or_default(),
                    ),
                    // `SG15 DTM+2380` — the planned Umstellungszeitpunkt the
                    // 3-Monats-Vorlauffrist of Kap. 3.5.2 Nr. 1 is measured
                    // against.
                    umstellungszeitpunkt: super::iftsta_zuordnungsbeginn(i).unwrap_or_default(),
                    message_ref: MessageRef::new(msg.message_ref()),
                    validation_passed,
                    validation_errors,
                });
            }

            // 21030/21031 — the wMSB's answer. The code is what the process
            // records; the PID only says which side of the axis it sits on, and
            // `DispatchAntwort` re-derives the PID from the code's cluster so
            // the two cannot disagree.
            let antwort_code = super::iftsta_antwortcode(i).unwrap_or_else(|| {
                if pid.as_u32() == ABLEHNUNG_PID {
                    "A04"
                } else {
                    "A03"
                }
                .to_owned()
            });
            Ok(ErsteinbauCommand::DispatchAntwort { antwort_code })
        },
    ));
    registry
}

// ── WiM Messstellenbetrieb (PIDs 55039, 55042, 55051, 55168) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`WimDeviceChangeWorkflow`].
#[must_use]
pub fn wim_registry() -> AdapterRegistry<WimDeviceChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for WiM adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                // IFTSTA status messages (PIDs 21009–21018) are also routed to
                // the wim-device-change workflow. Handle them here.
                if let AnyMessage::Iftsta(_) = msg {
                    return build_wim_iftsta_command(msg);
                }
                return Err(EngineError::Deserialization(
                    "WiM adapter: expected UTILMD or IFTSTA message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!("WiM adapter: PID detection failed: {e}"))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // Antwort PIDs close an order we sent. They carry no UTILMD AHB
            // Anwendungsfall, so `validate()` yields `ProfileNotFound` and the
            // validation flags above are meaningless here — the Bestätigung /
            // Ablehnung decision comes from the PID itself.
            if mako_wim::antwort_pid_meaning(pid.as_u32()).is_some() {
                return Ok(DeviceChangeCommand::ReceiveAntwort {
                    pid,
                    sender: MarktpartnerCode::new(
                        u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                    ),
                    message_ref: MessageRef::new(msg.message_ref()),
                    reason: Some(validation_errors.join("; ")).filter(|s| !s.is_empty()),
                    // `SG4 DTM` on an answer that moves the date — `Z01` on a
                    // Bestätigung, `Z12` on a Kündigungsablehnung. It replaces
                    // the requested Zuordnungsbeginn for everything downstream.
                    bestaetigter_termin: u
                        .transactions()
                        .first()
                        .and_then(|t| {
                            t.dtm
                                .iter()
                                .find(|d| {
                                    d.qualifier
                                        == edi_energy::utilmd_codes::dtm::LEISTUNGSBEGINN_GEPLANT
                                })
                                .and_then(|d| d.value_str())
                        })
                        .map(ToOwned::to_owned),
                });
            }

            // UTILMD 44183 „Ende MSB von NB" informs and asks nothing: no
            // Status der Antwort, no answer PID (AWH WiM Gas 2.0 Kap. 3.7,
            // UTILMD AHB Gas 1.2 Kap. 6.4). It takes the same path as the
            // IFTSTA Statusmeldungen rather than the MSB-Wechsel one, which
            // would look for an Antwortfrist the Anwendungsfall does not state.
            if pid.as_u32() == mako_wim::geraetewechsel::ENDE_MSB_VOM_NB_PID {
                return Ok(DeviceChangeCommand::ReceiveInformation {
                    pid,
                    sender: MarktpartnerCode::new(
                        u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                    ),
                    receiver: MarktpartnerCode::new(
                        u.receiver()
                            .and_then(|n| n.party_id.as_deref())
                            .unwrap_or(""),
                    ),
                    message_ref: MessageRef::new(msg.message_ref()),
                    validation_passed,
                    validation_errors,
                });
            }

            // WiM uses MeLo (Messlokation) as the object ID.
            let melo_id = MeLo::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.messlokation().or_else(|| t.marktlokation()))
                    .unwrap_or(""),
            );
            // Device ID from the first transaction reference (EIC / AGS).
            let device_id = DeviceId::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.references.first())
                    .and_then(|r| r.reference.as_deref())
                    .unwrap_or(""),
            );

            Ok(DeviceChangeCommand::ReceiveUtilmd {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                melo_id,
                device_id,
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                // `IDE+24` DE 7402 — the Vorgangsnummer the answer must echo.
                // Not `msg.message_ref()`: one interchange can carry several
                // Vorgänge, so the UNH reference would correlate a Bestätigung
                // to whichever of them happened to be first.
                vorgangsnummer: u
                    .transactions()
                    .first()
                    .and_then(|t| t.vorgangsnummer())
                    .map(ToOwned::to_owned),
                // The requested Zuordnungsbeginn resp. -ende. Which `SG4 DTM`
                // qualifier carries it depends on the Anwendungsfall: `76`
                // (Lieferdatum/-zeit, geplant) on an Anmeldung and a
                // Verpflichtungsanfrage, `93` (Datum Vertragsende) XOR `471`
                // (Ende zum nächstmöglichem Termin) on a Kündigung and an Ende
                // MSB. Looking only for `76` left every Kündigung and every
                // Abmeldung with no process date, and with it no
                // Vorlauffrist-Prüfung and no date for the answer to confirm.
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| {
                        use edi_energy::utilmd_codes::dtm;
                        [
                            dtm::LEISTUNGSBEGINN_GEPLANT,
                            dtm::ENDE_ZUM,
                            "471",
                            dtm::BEGINN_ZUM,
                        ]
                        .into_iter()
                        .find_map(|q| {
                            t.dtm
                                .iter()
                                .find(|d| d.qualifier == q)
                                .and_then(|d| d.value_str())
                        })
                    })
                    .map(ToOwned::to_owned),
                // `SG4 STS+7` DE 9013 — Muss on every WiM MSB-Wechsel PID, and
                // the answer echoes it.
                transaktionsgrund: u
                    .transactions()
                    .first()
                    .and_then(|t| t.transaktionsgrund())
                    .map(|g| g.grund),
                validation_passed,
                validation_errors,
                received_at: time::OffsetDateTime::now_utc(),
            })
        },
    ));
    registry
}

// ── WiM Steuerungsauftrag — REST-only, no EDIFACT adapter ───────────────────
//
// `WimSteuerungsauftragWorkflow` is driven exclusively through the
// BDEW API-Webdienste Strom `controlMeasuresV1` REST channel. It has no
// EDIFACT Prüfidentifikator and receives no AS4 inbound messages.
// No `AdapterRegistry` is registered for this workflow; commands are
// constructed in `energy-api` and submitted directly.

// ── WiM Geräteübernahme (PIDs 17001, 17002, 17009) ───────────────────────────

/// Build an [`AdapterRegistry`] for [`WimGeraeteubernahmeWorkflow`].
///
/// Handles the two ORDERS PIDs this workflow answers, in both Sparten:
/// `17001` (Bestellung Geräteübernahme) and `17009` (Anzeige
/// Gerätewechselabsicht), both MSBN → MSBA.
///
/// **17002 is not one of them.** „Weiterverpflichtung" is NB → MSBA with its
/// own Frist and Entscheidungsbaum — [`wim_weiterverpflichtung_registry`].
///
/// `sparte` comes from the interchange recipient's MP-ID: ORDERS and ORDRSP are
/// Sparte-neutral AHBs, so it is the only thing that tells `E_0247` from
/// `E_2011` and `S_0067` from `G_0061`.
///
/// The MeLo ID is extracted from the `IDE` segment (element 1, component 0),
/// the Gerätenummer from the first `RFF` segment's reference value.
#[must_use]
pub fn wim_geraeteubernahme_registry(
    sparte: mako_engine::types::Sparte,
) -> AdapterRegistry<WimGeraeteubernahmeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Geräteübernahme adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Geräteübernahme adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Geräteübernahme adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            let sender =
                MarktpartnerCode::new(o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""));
            let receiver = MarktpartnerCode::new(
                o.receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );
            let document_date = o
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();
            let message_ref = MessageRef::new(msg.message_ref());

            // MeLo from the IDE segment (element 1, component 0 = object ID).
            let melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            // `SG10 CAV+Z30` carries the Gerätenummer; the first `RFF` value
            // is the fallback the ORDERS AHB allows where the order names the
            // device by reference instead.
            let device_id = DeviceId::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "RFF")
                    .and_then(|s| s.component_str(0, 1))
                    .unwrap_or(""),
            );
            // The date the order turns on: the Übernahmezeitpunkt on a 17001,
            // the Gerätewechseltermin on a 17009. Both ride `DTM+76`
            // („Lieferdatum/-zeit, geplant"); `DTM+137` is the document date and
            // is never the process date.
            let termin = o
                .dtm()
                .iter()
                .find(|d| !d.is_document_date())
                .and_then(|d| d.value_str())
                .map(str::to_owned);
            Ok(GeraeteubernahmeCommand::ReceiveOrders {
                pid,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                termin,
                message_ref,
                validation_passed,
                validation_errors,
                sparte,
                received_at: time::OffsetDateTime::now_utc(),
            })
        },
    ));
    registry
}

// ── WiM Stammdaten (PID 17101) ───────────────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimStammdatenWorkflow`].
///
/// Extract ZAK+ZE+ZD register definitions from WiM ORDERS Stammdaten segments.
///
/// Parses the flat list of `OwnedSegment`s and groups them into per-register
/// JSON objects following the nesting: **ZAK → ZE → ZD**.
///
/// # Segment mapping (BDEW ORDERS AHB fv20251001 — WiM Stammdatenübermittlung)
///
/// | Segment | Field | Description |
/// |---|---|---|
/// | `ZAK` element 0 | `obis_kennzahl` | OBIS code (e.g. `"1-1:1.8.0"`) |
/// | `ZAK` element 1 | `zaehlerauspraegung` | `Z01`→`HT`, `Z02`→`NT`, `Z03`→`EINZEL` |
/// | `ZAK` element 2 | `bezeichnung` | Human-readable register label |
/// | `ZE` element 0 | `saison` | `Z01`→`SOMMER`, `Z02`→`WINTER`, `Z03`→`GESAMT` |
/// | `ZD` element 0 | `tagtyp` | `Z01`→`WERKTAG`, `Z02`→`SAMSTAG`, `Z03`→`SONNTAG_FEIERTAG` |
/// | `ZD` elements 1..N | `fenster` | `"HHMM:code"` switch-point pairs |
///
/// # Output shape per register
///
/// ```json
/// {
///   "obis_kennzahl": "1-1:1.8.0",
///   "zaehlerauspraegung": "HT",
///   "bezeichnung": "HT Tarif",
///   "saisons": [
///     {
///       "saison": "GESAMT",
///       "tagtypen": [
///         {
///           "tagtyp": "WERKTAG",
///           "wochentage": [1, 2, 3, 4, 5],
///           "fenster": [
///             {"von": "07:00", "bis": "22:00"},
///             {"von": "22:00", "bis": "07:00"}
///           ]
///         }
///       ]
///     }
///   ]
/// }
/// ```
///
/// The `fenster` windows are derived from consecutive switch points: the `von`
/// time of window `i` equals the switch time, and `bis` equals the next switch
/// time (wrapping to the first for the last entry).
pub fn extract_zak_ze_zaehlwerke(segs: &[OwnedSegment]) -> Vec<serde_json::Value> {
    let mut result: Vec<serde_json::Value> = Vec::new();

    // --- mutable accumulator state ---
    let mut cur_zw: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut cur_saisons: Vec<serde_json::Value> = Vec::new();
    let mut cur_saison: Option<serde_json::Map<String, serde_json::Value>> = None;
    let mut cur_tagtypen: Vec<serde_json::Value> = Vec::new();

    /// Flush accumulated `tagtypen` into `cur_saison`, then push saison into `cur_saisons`.
    fn flush_saison(
        cur_saison: &mut Option<serde_json::Map<String, serde_json::Value>>,
        cur_tagtypen: &mut Vec<serde_json::Value>,
        cur_saisons: &mut Vec<serde_json::Value>,
    ) {
        if let Some(mut s) = cur_saison.take() {
            s.insert(
                "tagtypen".into(),
                serde_json::Value::Array(std::mem::take(cur_tagtypen)),
            );
            cur_saisons.push(serde_json::Value::Object(s));
        } else {
            cur_tagtypen.clear();
        }
    }

    /// Flush accumulated `saisons` into `cur_zw`, then push zaehlwerk into `result`.
    fn flush_zaehlwerk(
        cur_zw: &mut Option<serde_json::Map<String, serde_json::Value>>,
        cur_saisons: &mut Vec<serde_json::Value>,
        result: &mut Vec<serde_json::Value>,
    ) {
        if let Some(mut zw) = cur_zw.take() {
            zw.insert(
                "saisons".into(),
                serde_json::Value::Array(std::mem::take(cur_saisons)),
            );
            result.push(serde_json::Value::Object(zw));
        } else {
            cur_saisons.clear();
        }
    }

    for seg in segs {
        match &*seg.tag {
            "ZAK" => {
                // Flush any in-progress register.
                flush_saison(&mut cur_saison, &mut cur_tagtypen, &mut cur_saisons);
                flush_zaehlwerk(&mut cur_zw, &mut cur_saisons, &mut result);

                let obis = seg.element_str(0).unwrap_or("").to_owned();
                let zaehlerauspraegung = match seg.element_str(1).unwrap_or("Z03") {
                    "Z01" => "HT",
                    "Z02" => "NT",
                    _ => "EINZEL",
                };
                let bezeichnung = seg.element_str(2).unwrap_or("").to_owned();

                let mut zw = serde_json::Map::new();
                zw.insert("obis_kennzahl".into(), serde_json::Value::String(obis));
                zw.insert(
                    "zaehlerauspraegung".into(),
                    serde_json::Value::String(zaehlerauspraegung.to_owned()),
                );
                zw.insert("bezeichnung".into(), serde_json::Value::String(bezeichnung));
                cur_zw = Some(zw);
            }
            "ZE" if cur_zw.is_some() => {
                // Flush any in-progress saison.
                flush_saison(&mut cur_saison, &mut cur_tagtypen, &mut cur_saisons);

                let saison = match seg.element_str(0).unwrap_or("Z03") {
                    "Z01" => "SOMMER",
                    "Z02" => "WINTER",
                    _ => "GESAMT",
                };
                let mut s = serde_json::Map::new();
                s.insert(
                    "saison".into(),
                    serde_json::Value::String(saison.to_owned()),
                );
                cur_saison = Some(s);
            }
            "ZD" if cur_saison.is_some() => {
                let (tagtyp, wochentage) = match seg.element_str(0).unwrap_or("Z01") {
                    "Z02" => ("SAMSTAG", serde_json::json!([6])),
                    "Z03" => ("SONNTAG_FEIERTAG", serde_json::json!([7])),
                    _ => ("WERKTAG", serde_json::json!([1, 2, 3, 4, 5])),
                };

                // Collect all "HHMM:code" switch-point pairs from elements 1..N.
                let mut switches: Vec<String> = Vec::new();
                let mut idx = 1usize;
                while let Some(pair) = seg.element_str(idx) {
                    if !pair.is_empty() {
                        switches.push(pair.to_owned());
                    }
                    idx += 1;
                }

                // Build time windows: window i = [switch[i].time, switch[i+1].time).
                // The last window wraps around to switch[0].time.
                let times: Vec<String> = switches
                    .iter()
                    .map(|p| {
                        // "HHMM:code" → "HH:MM"
                        let raw = p.split(':').next().unwrap_or(p);
                        if raw.len() == 4 {
                            format!("{}:{}", &raw[..2], &raw[2..])
                        } else {
                            raw.to_owned()
                        }
                    })
                    .collect();

                let mut fenster: Vec<serde_json::Value> = Vec::with_capacity(times.len());
                for i in 0..times.len() {
                    let von = &times[i];
                    let bis = if i + 1 < times.len() {
                        times[i + 1].clone()
                    } else if !times.is_empty() {
                        times[0].clone()
                    } else {
                        "00:00".to_owned()
                    };
                    fenster.push(serde_json::json!({ "von": von, "bis": bis }));
                }

                cur_tagtypen.push(serde_json::json!({
                    "tagtyp":     tagtyp,
                    "wochentage": wochentage,
                    "fenster":    fenster,
                }));
            }
            _ => {}
        }
    }

    // Flush any remaining accumulated state.
    flush_saison(&mut cur_saison, &mut cur_tagtypen, &mut cur_saisons);
    flush_zaehlwerk(&mut cur_zw, &mut cur_saisons, &mut result);

    result
}

/// Build the inbound ORDERS adapter for WiM Stammdaten **Übermittlung** (PIDs 17102–17133).
///
/// This is the **responding-party** adapter (NB receiving MSB's master-data
/// response). It extracts:
/// - ZAK+ZE+ZD register definitions → `zaehlwerke`  
/// - LOC/QTY/MEA Standorteigenschaften → `standorteigenschaften` (if present)
/// - MeLo ID from the IDE segment
///
/// The resulting [`StammdatenCommand::TransmitStammdaten`] resumes (or starts)
/// the existing `wim-stammdaten` workflow on the NB side.
#[must_use]
pub fn wim_stammdaten_uebermittlung_registry() -> AdapterRegistry<WimStammdatenWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Stammdaten Übermittlung adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Stammdaten Übermittlung adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Stammdaten Übermittlung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let message_ref = MessageRef::new(msg.message_ref());

            // MeLo from the IDE segment (element 1, component 0).
            let _melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            // ZAK+ZE+ZD → typed register definitions.
            let zaehlwerke = extract_zak_ze_zaehlwerke(o.segments());

            // Standorteigenschaften is carried by UTILMD, not ORDERS 17102–17133.
            // Future: extend if LOC/QTY segments appear in Stammdaten ORDERS.
            let standorteigenschaften: Option<serde_json::Value> = None;

            Ok(StammdatenCommand::TransmitStammdaten {
                response_pid: pid,
                response_ref: message_ref,
                standorteigenschaften,
                zaehlwerke,
            })
        },
    ));
    registry
}

/// Handles the single inbound ORDERS PID 17101 (Anforderung Stammdaten) and
/// produces a [`StammdatenCommand::ReceiveAnforderung`].
///
/// The MeLo ID is extracted from the `IDE` segment (element 1, component 0).
#[must_use]
pub fn wim_stammdaten_registry() -> AdapterRegistry<WimStammdatenWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Stammdaten adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Stammdaten adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Stammdaten adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            let sender =
                MarktpartnerCode::new(o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""));
            let receiver = MarktpartnerCode::new(
                o.receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );
            let document_date = o
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();
            let message_ref = MessageRef::new(msg.message_ref());

            // MeLo from the IDE segment (element 1, component 0 = object ID).
            let melo_id = MeLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "IDE")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            Ok(StammdatenCommand::ReceiveAnforderung {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM Preisanfrage REQOTE (PIDs 35001/35002/35004/35005) ────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimPreisanfrageWorkflow`].
///
/// Handles **both** legs of the WiM Preisanfrage exchange:
/// - inbound **REQOTE** 35001/35002/35004/35005 (nMSB → MSB) → [`PreisanfrageCommand::ReceiveReqote`];
/// - inbound **QUOTES** 15001/15002/15004/15005 (MSB → nMSB, the Angebot answering our REQOTE)
///   → [`PreisanfrageCommand::ReceiveAngebot`], which resumes the process the
///   REQOTE opened.
#[must_use]
pub fn wim_preisanfrage_registry() -> AdapterRegistry<WimPreisanfrageWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Preisanfrage adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Preisanfrage adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            match msg {
                AnyMessage::Reqote(r) => {
                    let (validation_passed, validation_errors) = super::ahb_verdict(msg);
                    Ok(PreisanfrageCommand::ReceiveReqote {
                        pid,
                        sender: MarktpartnerCode::new(
                            r.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                        ),
                        receiver: MarktpartnerCode::new(
                            r.receiver()
                                .and_then(|n| n.party_id.as_deref())
                                .unwrap_or(""),
                        ),
                        message_ref: MessageRef::new(msg.message_ref()),
                        validation_passed,
                        validation_errors,
                    })
                }
                // QUOTES 15001/15002/15004/15005: the MSB's Angebot answering our REQOTE.
                AnyMessage::Quotes(_) => Ok(PreisanfrageCommand::ReceiveAngebot {
                    pid,
                    message_ref: MessageRef::new(msg.message_ref()),
                }),
                _ => Err(EngineError::Deserialization(
                    "WiM Preisanfrage adapter: expected REQOTE (35001/35002/35004/35005) or QUOTES \
                     (15001/15002/15004/15005) message"
                        .into(),
                )),
            }
        },
    ));
    registry
}

// ── WiM ESA Wertebestellung (REQOTE 35003 / ORDERS 17007 / ORDCHG 39002) ─────

/// Build an [`AdapterRegistry`] for the ESA Wertebestellung workflow.
///
/// Covers the inbound ESA→MSB leg of WiM Teil 2 Kapitel 4:
///
/// | PID | Message | Command |
/// |---|---|---|
/// | 35003 | REQOTE | `ReceiveAnfrage` (UC 4.1 Nr. 1) |
/// | 17007 | ORDERS | `ReceiveBestellung` (UC 4.1 Nr. 3) |
/// | 39002 | ORDCHG | `ReceiveStornierung` (UC 4.1 Nr. 5) |
///
/// Every command carries a [`Zustellquittung`]. makod issues its positive AS4
/// Receipt for an inbound message in the same request, so the adapter stamps the
/// acknowledgement at parse time — the ÜT that GPKE Teil 1 requires the Frist to
/// be counted from.
///
/// [`Zustellquittung`]: mako_wim::wertebestellung::Zustellquittung
#[must_use]
/// Read `SG27 PIA+5+<Produkt-Code>:Z11` — the ordered Messprodukt.
///
/// Also the second half of the subscription's business key: several
/// Kapitel-4.6 products exist for one Marktlokation.
///
/// REQOTE AHB 1.2 §4.3 condition `[41]` restricts DE 7140 to the codes of
/// *Codeliste der Konfigurationen* Kapitel 4.6, so a code outside it is not a
/// wrong product but an undefined one.
pub fn extract_messprodukt(msg: &AnyMessage) -> Option<String> {
    msg.segments()
        .iter()
        .filter(|s| s.tag == "PIA" && s.component_str(0, 0) == Some("5"))
        .find_map(|s| {
            // C212: 7140 Produkt-Code, 7143 Produktart. `Z11` = Produkt;
            // `SRW` on the same qualifier is an OBIS-Kennzahl, not a product.
            let code = s.component_str(1, 0)?;
            (s.component_str(1, 1) == Some("Z11")).then(|| code.to_owned())
        })
}

/// Read `SG2 AJT` — the published Antwortcode and the EBD it came from.
///
/// **Muss** on every ORDRSP that answers an ESA order (ORDRSP AHB 1.1b §4.15,
/// PIDs 19011–19014), and the only structured statement of what the answer
/// means: those four use cases publish no free-text segment at all. The one
/// `FTX` a conformant 19011 may carry is `SG27 FTX+Z27`, which holds the MSB's
/// **IP address** — reading that as a rejection reason recorded an IP where the
/// Antwortcode belonged, and left every 19012 with no reason at all.
///
/// `None` means the counterparty omitted a Muss segment, which the workflow
/// records rather than papering over.
fn extract_antwort(msg: &AnyMessage) -> Option<mako_wim::esa::Antwort> {
    let AnyMessage::Ordrsp(o) = msg else {
        return None;
    };
    o.ajt()
        .map(|a| mako_wim::esa::Antwort::new(a.antwortcode.clone(), a.ebd.clone()))
}

/// Read `SG27 FTX+Z27` (IP-Adresse) or `FTX+Z28` (IP-Range) off a confirming
/// ORDRSP 19011.
///
/// ORDRSP AHB 1.1b §4.15 conditions `[76]`/`[77]`: exactly one of the two is
/// **Muss** when the confirmed order named a Kapitel-4.6.2 product. It is the
/// source the ESA has to admit before the iMS can reach it, so a confirmed
/// SMGW subscription without it can never deliver.
fn extract_smgw_quelle(msg: &AnyMessage) -> Option<mako_wim::esa::SmgwQuelle> {
    msg.segments()
        .iter()
        .filter(|s| s.tag == "FTX")
        .find_map(|s| match s.component_str(0, 0) {
            // C108 sits at element 3 (`FTX+Z27+++<Adresse>`), and its
            // components are the repeated DE 4440 lines.
            Some("Z27") => Some(mako_wim::esa::SmgwQuelle::Adresse(
                s.component_str(3, 0)?.to_owned(),
            )),
            Some("Z28") => Some(mako_wim::esa::SmgwQuelle::Range {
                von: s.component_str(3, 0)?.to_owned(),
                bis: s.component_str(3, 1)?.to_owned(),
            }),
            _ => None,
        })
}

/// Read the free text of a given `FTX` DE 4451 qualifier.
///
/// C108 is the **fourth** element of `FTX` (4451, 4453, C107, C108) and
/// `component_str` indexes from zero, so the text is at 3; reading 4 addresses
/// DE 3453 (the language code) and yields `None` for every conformant message.
///
/// Qualified rather than „the first FTX": on a 19011 the first `FTX` is
/// `SG27 FTX+Z27`, the MSB's IP address.
fn esa_freitext(msg: &AnyMessage, qualifier: &str) -> Option<String> {
    msg.segments()
        .iter()
        .filter(|s| s.tag == "FTX" && s.component_str(0, 0) == Some(qualifier))
        .find_map(|s| s.component_str(3, 0))
        .map(str::to_owned)
}

/// Read a `DTM` whose DE 2380 is a **duration** and resolve it against the
/// document date of the message that states it.
///
/// `DTM+273` (Gültigkeitsdauer/Bindungsfrist) and `DTM+279` (Einrichtungs-
/// zeitspanne) both carry a count in DE 2380 and its unit in DE 2379 — `802`
/// Monat, `803` Woche, `804` Tag (QUOTES AHB 1.1a condition `[908]`). Parsed
/// as `CCYYMMDD` they find nothing at all.
///
/// The span runs from the MSB's `DTM+137` Dokumentendatum, so the same message
/// resolves to the same instant however long it waited in a queue and however
/// often it is replayed. Anchoring on arrival would hand a stale offer a fresh
/// Bindungsfrist.
fn extract_dauer(msg: &AnyMessage, qualifier: &str) -> Option<time::OffsetDateTime> {
    let anchor = extract_dtm_date(msg, "137").map_or_else(time::OffsetDateTime::now_utc, |d| {
        mako_fristen::berlin_midnight(d)
    });
    msg.segments()
        .iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| {
            let count: i64 = s.component_str(0, 1)?.trim().parse().ok()?;
            let einheit = edi_energy::builders::DauerEinheit::from_code(s.component_str(0, 2)?)?;
            einheit.resolve_from(anchor, count)
        })
}

/// Read the commercial substance of a QUOTES 15003 Angebot.
///
/// UC 4.1.1: the ESA „fragt die Übermittlung von Werten **und die damit
/// verbundenen Kosten**" beim MSB an. QUOTES AHB 1.1a §4.3 makes `SG4 CUX`,
/// the `SG27 PIA+Z02` Artikel-IDs, the `SG31 PRI+CAL` prices and one to 23
/// `PIA+5 …:SRW` OBIS-Kennzahlen all **Muss** — keeping only the Bindungsfrist
/// reduced an offer to its expiry date. The ESA needs the prices to order
/// deliberately and to check the MSB's INVOIC 31009 (UC 4.5) against what it
/// accepted, and the OBIS list to know which registers the subscription owes.
fn extract_angebot(msg: &AnyMessage) -> mako_wim::esa::Angebot {
    use mako_wim::esa::{Angebot, Preisposition, Preistyp};

    let segs = msg.segments();
    // `SG4 CUX+2:EUR:4` — DE 6345 is the currency, component 1 of C504.
    let waehrung = segs
        .iter()
        .find(|s| s.tag == "CUX")
        .and_then(|s| s.component_str(0, 1))
        .map(str::to_owned);

    // `SG27 PIA+5+<OBIS>:SRW` — the registers the subscription will carry.
    let obis_kennzahlen: Vec<String> = segs
        .iter()
        .filter(|s| s.tag == "PIA" && s.component_str(0, 0) == Some("5"))
        .filter_map(|s| {
            let code = s.component_str(1, 0)?;
            (s.component_str(1, 1) == Some("SRW")).then(|| code.to_owned())
        })
        .collect();

    // `SG27 PIA+Z02+<Artikel-ID>:Z09` and the `SG31 PRI+CAL:<Betrag>:<Typ>::<Menge>:<Einheit>`
    // that follows it. The AHB repeats `SG31` once per Artikel-ID inside the
    // same `SG27 LIN`, in that order, so a price is attributed to the most
    // recent Artikel-ID seen — the wire carries no back-reference.
    let mut preise = Vec::new();
    let mut artikel: Vec<String> = Vec::new();
    let mut naechster = 0usize;
    for seg in segs {
        match &*seg.tag {
            "LIN" => {
                artikel.clear();
                naechster = 0;
            }
            "PIA" if seg.component_str(0, 0) == Some("Z02") => {
                if let Some(id) = seg.component_str(1, 0) {
                    artikel.push(id.to_owned());
                }
            }
            "PRI" => {
                let Some(betrag) = seg.component_str(0, 1) else {
                    continue;
                };
                // C509: 5125, 5118, 5375, 5387, 5284, 6411 — the Preisart
                // code is component 3, the unit component 5.
                let Some(preistyp) = seg.component_str(0, 3).and_then(Preistyp::from_pri_code)
                else {
                    continue;
                };
                let artikel_id = artikel.get(naechster).or_else(|| artikel.last());
                preise.push(Preisposition {
                    artikel_id: artikel_id.cloned().unwrap_or_default(),
                    preistyp,
                    betrag: betrag.to_owned(),
                    einheit: seg.component_str(0, 5).unwrap_or_default().to_owned(),
                });
                naechster += 1;
            }
            _ => {}
        }
    }

    Angebot {
        waehrung,
        preise,
        obis_kennzahlen,
        // `DTM+279` — „Erforderliche Zeitspanne zur Einrichtung der
        // Übermittlung von Werten ab Bestellung", a duration (Kann).
        einrichtung_bis: extract_dauer(msg, "279"),
    }
}

/// Read a `DTM` of the given DE 2005 qualifier as a date.
///
/// Kapitel 4 uses format `303` (`CCYYMMDDHHMMZZZ`) throughout, so only the
/// leading eight digits are the date.
fn extract_dtm_date(msg: &AnyMessage, qualifier: &str) -> Option<time::Date> {
    msg.segments()
        .iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|v| {
            let d: String = v.chars().filter(char::is_ascii_digit).take(8).collect();
            (d.len() == 8).then_some(d)
        })
        .and_then(|d| {
            time::Date::from_calendar_date(
                d[0..4].parse().ok()?,
                time::Month::try_from(d[4..6].parse::<u8>().ok()?).ok()?,
                d[6..8].parse().ok()?,
            )
            .ok()
        })
}

/// Read `IMD++<7081>` — the Abonnement mode.
///
/// **Muss** on ORDERS 17007/17008 (ORDERS AHB 1.1b §4.15). DE 7081 sits in
/// C272, i.e. element 2 of the segment, with DE 7077 (element 1) empty.
fn extract_abonnement(msg: &AnyMessage) -> Option<mako_wim::esa::Abonnement> {
    msg.segments()
        .iter()
        .filter(|s| s.tag == "IMD")
        .find_map(|s| mako_wim::esa::Abonnement::from_imd_code(s.component_str(1, 0)?))
}

pub fn wim_wertebestellung_registry() -> AdapterRegistry<WimWertebestellungWorkflow> {
    use mako_wim::wertebestellung::{
        ABBESTELLUNG_PID, ANFRAGE_PID, BESTELLUNG_PID, Lokationsebene, STORNIERUNG_PID,
        WertebestellungCommand, Zustellquittung,
    };
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Wertebestellung adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Wertebestellung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let message_ref = MessageRef::new(msg.message_ref());
            // The positive AS4 Receipt for this message is issued in the same
            // request, so "now" is the ÜT.
            let quittung = Zustellquittung::positive(time::OffsetDateTime::now_utc());

            match pid {
                ANFRAGE_PID => {
                    let AnyMessage::Reqote(r) = msg else {
                        return Err(EngineError::Deserialization(
                            "WiM Wertebestellung adapter: PID 35003 expects a REQOTE".into(),
                        ));
                    };
                    // UC 4.1: the request names a MaLo-ID, a ZPB or a NeLo-ID
                    // depending on the level it is addressed to.
                    let lokations_id = r
                        .segments()
                        .iter()
                        .find(|s| s.tag == "LOC")
                        .and_then(|s| s.component_str(1, 0))
                        .unwrap_or("")
                        .to_owned();
                    // The Messprodukt itself says which level it is for, so
                    // the identifier length only has to disambiguate when the
                    // product is unknown — and an unknown product is refused
                    // by the workflow's own catalogue check anyway.
                    let messprodukt = extract_messprodukt(msg).unwrap_or_default();
                    let ebene = mako_wim::esa::messprodukt(&messprodukt).map_or_else(
                        || match lokations_id.len() {
                            33 => Lokationsebene::Messlokation,
                            11 => Lokationsebene::Marktlokation,
                            _ => Lokationsebene::Netzlokation,
                        },
                        |p| p.ebene,
                    );
                    // `DTM+76` — der Wunschtermin für die erstmalige
                    // Übermittlung. Muss on 35003; absent only on a
                    // non-conformant message, where "today" is the honest
                    // reading of "as soon as possible".
                    let wunschtermin =
                        extract_dtm_date(msg, "76").unwrap_or_else(mako_fristen::heute);
                    let gegenstand = Box::new(mako_wim::esa::Bestellgegenstand {
                        messprodukt,
                        wunschtermin,
                        zeitraum_bis: None,
                        // The REQOTE carries no `IMD`; the Abo mode is stated
                        // on the ORDERS that follows.
                        abonnement: mako_wim::esa::Abonnement::StartAbo,
                        smgw: None,
                    });
                    Ok(WertebestellungCommand::ReceiveAnfrage {
                        pid,
                        esa: MarktpartnerCode::new(
                            r.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                        ),
                        msb: MarktpartnerCode::new(
                            r.receiver()
                                .and_then(|n| n.party_id.as_deref())
                                .unwrap_or(""),
                        ),
                        ebene,
                        lokations_id,
                        gegenstand,
                        message_ref,
                        quittung,
                        // Filled by the makod ingest consent gate before spawn.
                        consent_block: None,
                    })
                }
                BESTELLUNG_PID => Ok(WertebestellungCommand::ReceiveBestellung {
                    pid,
                    message_ref,
                    // `IMD+7081` is Muss on 17007; `Z01` (Abo) is the reading
                    // that keeps a running subscription terminable, and the
                    // AHB profile validator rejects a message without it.
                    abonnement: extract_abonnement(msg)
                        .unwrap_or(mako_wim::esa::Abonnement::StartAbo),
                    quittung,
                    // Filled by the makod ingest consent gate before spawn.
                    consent_block: None,
                }),
                ABBESTELLUNG_PID => {
                    // UC 4.3 Nr. 1: the ESA ends a running delivery. The stop
                    // date is `DTM+203` Ausführungsdatum — **Muss** on 17008
                    // (ORDERS AHB 1.1b §4.15); `Z11` is not a qualifier this
                    // message uses.
                    let beendigung_zum = extract_dtm_date(msg, "203")
                        .map(|d| d.midnight().assume_utc())
                        .unwrap_or_else(time::OffsetDateTime::now_utc);
                    Ok(WertebestellungCommand::ReceiveAbbestellung {
                        pid,
                        message_ref,
                        beendigung_zum,
                        quittung,
                    })
                }
                STORNIERUNG_PID => Ok(WertebestellungCommand::ReceiveStornierung {
                    pid,
                    message_ref,
                    quittung,
                }),
                other => Err(EngineError::Deserialization(format!(
                    "WiM Wertebestellung adapter: PID {other} is not an ESA inbound PID \
                     (expected 35003, 17007, 17008 or 39002)"
                ))),
            }
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`EsaWertebestellungWorkflow`].
///
/// The ESA-origination side: it *receives* the MSB's answers and resumes the
/// process it started. Maps the inbound MSB→ESA responses to `Receive*`:
///
/// | PID | Message | Command |
/// |---|---|---|
/// | 15003 | QUOTES | `ReceiveAngebot` (UC 4.1 Nr. 2) |
/// | 19011 | ORDRSP | `ReceiveBestaetigung` (Ab-/Bestellung) |
/// | 19012 | ORDRSP | `ReceiveAblehnung` (Ab-/Bestellung) |
/// | 19013 | ORDRSP | `ReceiveStornierungAntwort` (Bestätigung) |
/// | 19014 | ORDRSP | `ReceiveStornierungAntwort` (Ablehnung) |
#[must_use]
pub fn esa_wertebestellung_registry() -> AdapterRegistry<EsaWertebestellungWorkflow> {
    use mako_wim::esa_wertebestellung::{
        ABLEHNUNG_PID, ANGEBOT_PID, BEENDIGUNG_MSB_PID, BESTAETIGUNG_PID,
        EsaWertebestellungCommand, STORNO_ABLEHNUNG_PID, STORNO_BESTAETIGUNG_PID,
    };
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for ESA Wertebestellung adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "ESA Wertebestellung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let message_ref = MessageRef::new(msg.message_ref());

            match pid {
                ANGEBOT_PID => {
                    // QUOTES 15003 covers both outcomes of UC 4.1 Nr. 2
                    // („Angebot zur / Ablehnung der Anfrage"), but the QUOTES
                    // AHB 1.1a publishes only the Angebot — and makes
                    // `DTM+273` **Muss** on it. The Bindungsfrist therefore
                    // cannot tell the two apart: a refusal carries one too,
                    // and reading its absence as a rejection turned every
                    // conformant offer into one.
                    //
                    // What an offer *is* is a priced position: `SG31 PRI` is
                    // Muss inside the `SG27 LIN` block, one per Artikel-ID.
                    // A 15003 that prices nothing is the MSB declining, with
                    // its grounds in `FTX+ACB` — the only free text the
                    // message has.
                    let angebot = extract_angebot(msg);
                    if angebot.ist_leer() {
                        return Ok(EsaWertebestellungCommand::ReceiveAnfrageAblehnung {
                            message_ref,
                            reason: esa_freitext(msg, "ACB"),
                        });
                    }
                    // `DTM+273` is a **duration**, not a date: QUOTES AHB 1.1a
                    // §4.3 gives DE 2380 as „Zeitraum" with condition [908]
                    // („Mögliche Werte: 1 bis n") and DE 2379 as `802` Monat /
                    // `803` Woche / `804` Tag. Read as `CCYYMMDD` it finds
                    // nothing in a conformant message.
                    let bindungsfrist =
                        extract_dauer(msg, "273").unwrap_or_else(time::OffsetDateTime::now_utc);
                    Ok(EsaWertebestellungCommand::ReceiveAngebot {
                        message_ref,
                        bindungsfrist,
                        // `DTM+469` — the earliest start the MSB offers, Muss.
                        fruehester_start: extract_dtm_date(msg, "469")
                            .map(|d| d.midnight().assume_utc()),
                        angebot: Box::new(angebot),
                    })
                }
                BESTAETIGUNG_PID => Ok(EsaWertebestellungCommand::ReceiveBestaetigung {
                    message_ref,
                    antwort: extract_antwort(msg),
                    smgw_quelle: extract_smgw_quelle(msg),
                }),
                ABLEHNUNG_PID => Ok(EsaWertebestellungCommand::ReceiveAblehnung {
                    message_ref,
                    antwort: extract_antwort(msg),
                }),
                STORNO_BESTAETIGUNG_PID | STORNO_ABLEHNUNG_PID => {
                    Ok(EsaWertebestellungCommand::ReceiveStornierungAntwort {
                        pid,
                        message_ref,
                        antwort: extract_antwort(msg),
                    })
                }
                BEENDIGUNG_MSB_PID => {
                    // IFTSTA 21042 (WiM Umsetzungsstatus, MSB → ESA, UC 4.4).
                    //
                    // The Beendigung date is `SG15 DTM+93` „Datum
                    // Vertragsende" (IFTSTA AHB 2.1 §6.9) — **not** the first
                    // DTM in the message, which is `DTM+137`, the day the MSB
                    // wrote the notice.
                    let beendigung_zum = extract_dtm_date(msg, "93")
                        .map(|d| d.midnight().assume_utc())
                        .unwrap_or_else(time::OffsetDateTime::now_utc);
                    Ok(EsaWertebestellungCommand::ReceiveBeendigungDurchMsb {
                        message_ref,
                        beendigung_zum,
                        reason: esa_freitext(msg, "ACB"),
                    })
                }
                other => Err(EngineError::Deserialization(format!(
                    "ESA Wertebestellung adapter: PID {other} is not an ESA inbound response \
                     (expected 15003, 19011, 19012, 19013, 19014 or 21042)"
                ))),
            }
        },
    ));
    registry
}

// ── WiM Preisliste PRICAT (PIDs 27001–27003) ──────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimPreislisteWorkflow`].
///
/// Handles inbound PRICAT price-list messages from MSB to nMSB.
/// Produces [`PreislisteCommand::ReceivePricat`].
#[must_use]
pub fn wim_preisliste_registry() -> AdapterRegistry<WimPreislisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Preisliste adapter".into(),
                )
            })?;
            let AnyMessage::Pricat(p) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Preisliste adapter: expected PRICAT message (PIDs 27001–27003)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Preisliste adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            Ok(PreislisteCommand::ReceivePricat {
                pid,
                sender: MarktpartnerCode::new(
                    p.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    p.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── WiM INSRPT (PIDs 23001/23003/23004/23005/23008/23009/23011/23012) ────────

/// Build an [`AdapterRegistry`] for [`WimInsrptWorkflow`], **in both Sparten**.
///
/// Handles every inbound INSRPT of the Störungsbehebung in der Messlokation,
/// on both sides of it: 23001 opens the process at the **MSB**, the rest are
/// the MSB's answers arriving at the **Störungsmelder**.
///
/// The INSRPT AHB is Sparte-neutral, so `sparte` comes from the recipient
/// MP-ID. `messtechnik` sizes the Antwort- and the Ergebnisfrist; neither is in
/// the message, and the MSB's own device registry is the only source. Passing
/// the fastest branch keeps an alert early rather than late.
#[must_use]
pub fn wim_insrpt_registry(
    sparte: Sparte,
    messtechnik: mako_fristen::antwort::Messtechnik,
) -> AdapterRegistry<WimInsrptWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Strom INSRPT adapter".into(),
                )
            })?;
            let AnyMessage::Insrpt(insrpt) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Strom INSRPT adapter: expected INSRPT message".into(),
                ));
            };
            let pid = insrpt
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Strom INSRPT adapter: PID not found in INSRPT BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            let sender = MarktpartnerCode::new(
                insrpt
                    .sender()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or(""),
            );
            let message_ref = MessageRef::new(
                insrpt
                    .bgm()
                    .and_then(|b| b.document_id.as_deref())
                    .unwrap_or(msg.message_ref()),
            );
            match pid.as_u32() {
                // 23001 arrives at the MSB and opens the process there. The
                // Melder side never ingests it — it sent it.
                23_001 => Ok(StoerungsmeldungCommand::ReceiveStoerungsmeldung {
                    pid,
                    melder_mp_id: sender,
                    msb_mp_id: MarktpartnerCode::new(
                        insrpt
                            .receiver()
                            .and_then(|n| n.party_id.as_deref())
                            .unwrap_or(""),
                    ),
                    melo_id: MeLo::new(
                        insrpt
                            .segments()
                            .iter()
                            .find(|s| s.tag == "LOC" && s.component_str(0, 0) == Some("172"))
                            .and_then(|s| s.component_str(1, 0))
                            .unwrap_or_default(),
                    ),
                    sparte,
                    document_date: insrpt
                        .dtm()
                        .iter()
                        .find(|d| d.qualifier == "137")
                        .and_then(|d| d.value.clone())
                        .unwrap_or_default(),
                    message_ref,
                    received_at: OffsetDateTime::now_utc(),
                    messtechnik,
                }),
                23_005 | 23_009 | 23_011 | 23_012 => {
                    Ok(StoerungsmeldungCommand::ReceiveInformationsmeldung {
                        pid,
                        sender,
                        message_ref,
                    })
                }
                _ => Ok(StoerungsmeldungCommand::ReceiveAntwort {
                    pid,
                    sender,
                    message_ref,
                }),
            }
        },
    ));
    registry
}

// ── WiM Technikänderung ORDRSP (PIDs 19003–19007) ────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimTechnikAenderungWorkflow`].
///
/// The Technikänderung process is **requester-initiated**: mako sends ORDERS
/// 17011 (Änderung der Technik, LF/NB → MSB) or 17118 (Konfigurationsänderung,
/// MSB → MSB) and the counterparty answers with an ORDRSP. Only that answer is
/// ingested here — it resumes the open process, never spawns one.
///
/// The accepted/rejected split is carried by the ORDRSP **PID** rather than the
/// BGM response code: 19003 (Fortführungsbestätigung) and 19005
/// (Auftragsbestätigung) are confirmations, 19004/19006/19007 are rejections.
/// `TechnikAenderungCommand::ReceiveOrdrsp` re-derives that split from the PID,
/// so the adapter only supplies the human-readable rejection reason.
///
/// # Segment mapping (BDEW ORDRSP AHB — WiM Strom Teil 1)
///
/// | Segment | Field | Description |
/// |---|---|---|
/// | `BGM` element 2 comp. 0 | response code | `27` = accepted; anything else is quoted into `reason` |
/// | `FTX` element 3 comp. 0 | free text | Preferred rejection reason when present |
/// | `BGM` element 0 comp. 0 | `message_ref` | Falls back to the interchange message reference |
#[must_use]
pub fn wim_technik_aenderung_registry() -> AdapterRegistry<WimTechnikAenderungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Technikänderung adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Technikänderung adapter: expected ORDRSP message".into(),
                ));
            };

            let ordrsp_pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Technikänderung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // A rejection reason is only meaningful for the negative PIDs; the
            // workflow discards it for 19003/19005. Prefer the FTX free text and
            // fall back to quoting the BGM response code.
            let reason = o
                .segments()
                .iter()
                .find(|s| s.tag == "FTX")
                .and_then(|s| s.component_str(3, 0))
                .map(str::to_owned)
                .or_else(|| {
                    o.segments()
                        .iter()
                        .find(|s| s.tag == "BGM")
                        .and_then(|s| s.component_str(2, 0))
                        .map(|code| format!("ORDRSP response code: {code}"))
                });

            Ok(TechnikAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── WiM Weiterverpflichtung (ORDERS 17002 → ORDRSP 19003/19004) ─────────────

/// Build an [`AdapterRegistry`] for [`mako_wim::WimWeiterverpflichtungWorkflow`].
///
/// Only the inbound leg: ORDERS 17002, the NB ordering this MSB to keep
/// operating a Messlokation whose Abmeldung has no successor yet (WiM Teil 1
/// Kap. 2.4.2 Nr. 5). The ORDRSP answer is an outbox entry the workflow
/// renders, so it needs no adapter.
///
/// The „verschobenes Zuordnungsende" comes from the ORDERS `DTM` — it is the
/// date the whole decision turns on, because the Weiterverpflichtungszeitraum
/// is capped at three months or one from the confirmed Zuordnungsende.
#[must_use]
pub fn wim_weiterverpflichtung_registry(
    sparte: mako_engine::types::Sparte,
) -> AdapterRegistry<WimWeiterverpflichtungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Weiterverpflichtung adapter".into(),
                )
            })?;
            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Weiterverpflichtung adapter: expected ORDERS message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Weiterverpflichtung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(WeiterverpflichtungCommand::ReceiveAuftrag {
                pid,
                sparte,
                nb: MarktpartnerCode::new(
                    o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                msba: MarktpartnerCode::new(
                    o.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                // MeLo from the IDE segment (element 1, component 0 = object ID).
                melo_id: MeLo::new(
                    o.segments()
                        .iter()
                        .find(|s| s.tag == "IDE")
                        .and_then(|s| s.component_str(1, 0))
                        .unwrap_or(""),
                ),
                // The date the NB wants the Messstellenbetrieb continued to —
                // the „verschobenes Zuordnungsende" the whole decision turns on.
                verschobenes_zuordnungsende: o
                    .segments()
                    .iter()
                    .find(|s| s.tag == "DTM")
                    .and_then(|s| s.component_str(0, 1))
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

// ── WiM Rechnungsabwicklung MSB über LF (ORDERS 17005/17006, ORDRSP 19009/19010) ──

/// Build an [`AdapterRegistry`] for
/// [`mako_wim::WimRechnungsabwicklungWorkflow`].
///
/// One registry, two message types, because the process has two entries:
///
/// - **ORDERS** 17005 (Bestellung — the LF accepting the quote; terminal on
///   receipt, nothing answers it) or 17006 (Beendigung — either side may send
///   it) → [`RechnungsabwicklungCommand::ReceiveOrders`], spawning a process.
/// - **ORDRSP** 19009/19010 (Bestätigung/Ablehnung der Beendigung) →
///   [`RechnungsabwicklungCommand::ReceiveAntwort`], resuming the process the
///   outbound 17006 opened.
///
/// Directions verified against the BDEW PID overview 4.0 and AWH
/// Aktivitätsdiagramme WiM V1.3 §§2.8–2.11 (EBDs `E_0206`/`E_0209`).
///
/// [`RechnungsabwicklungCommand::ReceiveOrders`]: mako_wim::RechnungsabwicklungCommand::ReceiveOrders
/// [`RechnungsabwicklungCommand::ReceiveAntwort`]: mako_wim::RechnungsabwicklungCommand::ReceiveAntwort
#[must_use]
pub fn wim_rechnungsabwicklung_registry()
-> AdapterRegistry<mako_wim::WimRechnungsabwicklungWorkflow> {
    use mako_wim::RechnungsabwicklungCommand;
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Rechnungsabwicklung adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Rechnungsabwicklung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            match msg {
                AnyMessage::Orders(o) => {
                    let (validation_passed, validation_errors) = super::ahb_verdict(msg);
                    Ok(RechnungsabwicklungCommand::ReceiveOrders {
                        pid,
                        sender: MarktpartnerCode::new(
                            o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                        ),
                        receiver: MarktpartnerCode::new(
                            o.receiver()
                                .and_then(|n| n.party_id.as_deref())
                                .unwrap_or(""),
                        ),
                        message_ref: MessageRef::new(msg.message_ref()),
                        validation_passed,
                        validation_errors,
                    })
                }
                AnyMessage::Ordrsp(_) => Ok(RechnungsabwicklungCommand::ReceiveAntwort {
                    pid,
                    message_ref: MessageRef::new(msg.message_ref()),
                }),
                // IFTSTA 21032 — the LF refuses the Angebot. Which tree its
                // code belongs to follows the *sequence*, and both sequences
                // end in this PID: `E_0205` when the MSB offered unprompted,
                // `E_0208` when the LF asked with a REQOTE 35002 first.
                //
                // An adapter sees one message and not the Vorgang, so it states
                // **no** Herkunft. Guessing would not fail closed: the two
                // trees share `A01`–`A03` with different Bedeutungen, so a
                // wrong guess passes the workflow's code check and records the
                // wrong reason. The refusal is recorded verbatim instead and an
                // operator resolves the tree — until the Herkunft is carried on
                // the process from the REQOTE/QUOTES leg.
                AnyMessage::Iftsta(i) => Ok(RechnungsabwicklungCommand::ReceiveAngebotAblehnung {
                    sender: MarktpartnerCode::new(
                        i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                    ),
                    receiver: MarktpartnerCode::new(
                        i.receiver()
                            .and_then(|n| n.party_id.as_deref())
                            .unwrap_or(""),
                    ),
                    herkunft: None,
                    antwort_code: super::iftsta_antwortcode(i),
                    message_ref: MessageRef::new(msg.message_ref()),
                }),
                _ => Err(EngineError::Deserialization(
                    "WiM Rechnungsabwicklung adapter: expected ORDERS, ORDRSP or IFTSTA".into(),
                )),
            }
        },
    ));
    registry
}
