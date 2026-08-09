//! WiM Strom adapter registries (Messstellenbetrieb, ESA Wertebestellung, INSRPT, Preise).
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
// ── WiM INVOIC billing (PID 31009 MSB-Rechnung) ───────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimInvoicWorkflow`].
///
/// Extracts INVOIC fields to construct a [`WimInvoicCommand::ReceiveInvoic`]
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

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "WiM Rechnung adapter: PID not found in INVOIC BGM".into(),
                    )
                })
                .and_then(convert_pid)?;
            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
            let invoice_ref = inv
                .bgm()
                .and_then(|b| b.document_id.as_deref())
                .unwrap_or(msg.message_ref());

            Ok(WimInvoicCommand::ReceiveInvoic {
                pruefidentifikator: pid,
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
                rechnung: serde_json::to_value(build_rechnung(inv.segments()))
                    .unwrap_or(serde_json::Value::Null),
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
/// with [`WimInvoicCommand::ReceiveRemadv`]. Mirrors `gpke_abrechnung_remadv_registry`.
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
            Ok(WimInvoicCommand::ReceiveRemadv {
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
/// [`WimInvoicCommand::ReceiveComdis`]. Mirrors `gpke_abrechnung_comdis_registry`.
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
            Ok(WimInvoicCommand::ReceiveComdis {
                comdis_ref: MessageRef::new(msg.message_ref()),
            })
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
            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

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
                    reason: validation_result
                        .as_ref()
                        .map(|r| {
                            r.errors()
                                .iter()
                                .map(|i| format!("{i}"))
                                .collect::<Vec<_>>()
                                .join("; ")
                        })
                        .filter(|s| !s.is_empty()),
                });
            }

            // WiM uses MeLo (Messlokation) as the object ID.
            let melo_id = MeLo::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.ide.object_id.as_deref())
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
/// Handles the three ORDERS PIDs of the Geräteübernahme family:
/// - `17001` (Bestellung Geräteübernahmeangebot, MSBN → MSBA) and `17002`
///   (Weiterverpflichtung, NB → MSBA) → [`GeraeteubernahmeCommand::ReceiveAnfrage`]
/// - `17009` (Anzeige Gerätewechselabsicht, MSBN → MSBA) →
///   [`GeraeteubernahmeCommand::ReceiveGeraetewechselabsicht`]
///
/// The MeLo ID is extracted from the `IDE` segment (element 1, component 0).
/// The `DeviceId` (Anfrage only) is extracted from the first `RFF` segment's
/// reference value (element 0, component 1).
#[must_use]
pub fn wim_geraeteubernahme_registry() -> AdapterRegistry<WimGeraeteubernahmeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
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

            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

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

            let pid_u32 = pid.as_u32();
            if matches!(pid_u32, 17001 | 17002) {
                // Phase 1: Anfrage Geräteübernahmeangebot — extract DeviceId from
                // the first RFF reference value (element 0, component 1).
                let device_id = DeviceId::new(
                    o.segments()
                        .iter()
                        .find(|s| s.tag == "RFF")
                        .and_then(|s| s.component_str(0, 1))
                        .unwrap_or(""),
                );
                Ok(GeraeteubernahmeCommand::ReceiveAnfrage {
                    pid,
                    sender,
                    receiver,
                    melo_id,
                    device_id,
                    document_date,
                    message_ref,
                    validation_passed,
                    validation_errors,
                })
            } else {
                // 17009 — Anzeige Gerätewechselabsicht (MSBN → MSBA). Answered by
                // ORDRSP 19015/19016, not by a Bestellbestätigung.
                Ok(GeraeteubernahmeCommand::ReceiveGeraetewechselabsicht { pid, message_ref })
            }
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
        match seg.tag.as_str() {
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

            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();

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

// ── WiM Preisanfrage REQOTE (PIDs 35001–35005) ────────────────────────────────

/// Build an [`AdapterRegistry`] for [`WimPreisanfrageWorkflow`].
///
/// Handles **both** legs of the WiM Preisanfrage exchange:
/// - inbound **REQOTE** 35001–35005 (nMSB → MSB) → [`PreisanfrageCommand::ReceiveReqote`];
/// - inbound **QUOTES** 15001–15005 (MSB → nMSB, the Angebot answering our REQOTE)
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
                    let validation_result = msg.validate().ok();
                    let validation_passed = validation_result
                        .as_ref()
                        .map(|r| r.is_valid())
                        .unwrap_or(false);
                    let validation_errors: Vec<String> = validation_result
                        .as_ref()
                        .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                        .unwrap_or_default();
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
                // QUOTES 15001–15005: the MSB's Angebot answering our REQOTE.
                AnyMessage::Quotes(_) => Ok(PreisanfrageCommand::ReceiveAngebot {
                    pid,
                    message_ref: MessageRef::new(msg.message_ref()),
                }),
                _ => Err(EngineError::Deserialization(
                    "WiM Preisanfrage adapter: expected REQOTE (35001–35005) or QUOTES \
                     (15001–15005) message"
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
                    let ebene = match lokations_id.len() {
                        33 => Lokationsebene::Messlokation,
                        11 => Lokationsebene::Marktlokation,
                        _ => Lokationsebene::Netzlokation,
                    };
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
                        message_ref,
                        quittung,
                        // Filled by the makod ingest consent gate before spawn.
                        consent_block: None,
                    })
                }
                BESTELLUNG_PID => Ok(WertebestellungCommand::ReceiveBestellung {
                    pid,
                    message_ref,
                    quittung,
                    // Filled by the makod ingest consent gate before spawn.
                    consent_block: None,
                }),
                ABBESTELLUNG_PID => {
                    // UC 4.3 Nr. 1: the ESA ends a running delivery. The stop
                    // date travels in DTM+Z11 where present; else it stops now.
                    let beendigung_zum = msg
                        .segments()
                        .iter()
                        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some("Z11"))
                        .and_then(|s| s.component_str(0, 1))
                        .and_then(|v| {
                            time::Date::parse(
                                v,
                                &time::format_description::well_known::Iso8601::DEFAULT,
                            )
                            .ok()
                            .map(|d| d.midnight().assume_utc())
                        })
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
            // Rejection reason (ORDRSP 19012/19014) from the first FTX free text.
            // C108 is the *fourth* element of FTX (4451, 4453, C107, C108) and
            // `component_str` indexes elements from zero, so the free text is at
            // 3. Reading 4 addresses 3453 (language code) and yields `None` for
            // every conformant message.
            let reason = msg
                .segments()
                .iter()
                .find(|s| s.tag == "FTX")
                .and_then(|s| s.component_str(3, 0))
                .map(str::to_owned);

            match pid {
                ANGEBOT_PID => {
                    // QUOTES 15003 carries both the Angebot and the Ablehnung der
                    // Anfrage. They are told apart by the Bindungsfrist
                    // (DTM+273, offer validity): an Angebot has one, an
                    // Ablehnung does not.
                    let bindungsfrist = msg
                        .segments()
                        .iter()
                        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some("273"))
                        .and_then(|s| s.component_str(0, 1))
                        .and_then(parse_ccyymmdd);
                    if let Some(bindungsfrist) = bindungsfrist {
                        Ok(EsaWertebestellungCommand::ReceiveAngebot {
                            message_ref,
                            bindungsfrist,
                        })
                    } else {
                        // No Bindungsfrist → Ablehnung der Anfrage. Reason from
                        // the FTX free text (element 3, per the AHB).
                        let reason = msg
                            .segments()
                            .iter()
                            .find(|s| s.tag == "FTX")
                            .and_then(|s| s.component_str(3, 0))
                            .map(str::to_owned);
                        Ok(EsaWertebestellungCommand::ReceiveAnfrageAblehnung { reason })
                    }
                }
                BESTAETIGUNG_PID => {
                    Ok(EsaWertebestellungCommand::ReceiveBestaetigung { message_ref })
                }
                ABLEHNUNG_PID => Ok(EsaWertebestellungCommand::ReceiveAblehnung {
                    message_ref,
                    reason,
                }),
                p @ (STORNO_BESTAETIGUNG_PID | STORNO_ABLEHNUNG_PID) => {
                    Ok(EsaWertebestellungCommand::ReceiveStornierungAntwort {
                        pid,
                        message_ref,
                        reason: if p == STORNO_ABLEHNUNG_PID {
                            reason
                        } else {
                            None
                        },
                    })
                }
                BEENDIGUNG_MSB_PID => {
                    // IFTSTA 21042 (WiM Umsetzungsstatus, MSB → ESA, UC 4.4). The
                    // Beendigung date is the status DTM; STS 4405 = 105 „beendet"
                    // is asserted by the profile validator.
                    let beendigung_zum = msg
                        .segments()
                        .iter()
                        .find(|s| s.tag == "DTM")
                        .and_then(|s| s.component_str(0, 1))
                        .and_then(parse_ccyymmdd)
                        .unwrap_or_else(time::OffsetDateTime::now_utc);
                    Ok(EsaWertebestellungCommand::ReceiveBeendigungDurchMsb {
                        message_ref,
                        beendigung_zum,
                        reason,
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
            let validation_result = msg.validate().ok();
            let validation_passed = validation_result
                .as_ref()
                .map(|r| r.is_valid())
                .unwrap_or(false);
            let validation_errors: Vec<String> = validation_result
                .as_ref()
                .map(|r| r.errors().iter().map(|i| format!("{i}")).collect())
                .unwrap_or_default();
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

// ── WiM Strom INSRPT (PIDs 23001/23003/23004/23008/23011/23012) ──────────────

/// Build an [`AdapterRegistry`] for [`WimInsrptWorkflow`] (WiM Strom, 5 WT).
///
/// Handles inbound INSRPT messages for fault/inspection reporting between LF
/// and MSB in the WiM Strom Teil 2 process.  Covers both the outbound
/// Störungsmeldung (23001) and all inbound MSB responses
/// (23003/23004/23008/23011/23012).
///
/// In combined Strom+Gas deployments the ingest layer must supply
/// `Sparte::Strom` when calling [`PidRouter::route_with_sparte`] to reach this
/// workflow instead of [`wim_gas_insrpt_registry`].
///
/// [`PidRouter::route_with_sparte`]: mako_engine::pid_router::PidRouter::route_with_sparte
#[must_use]
pub fn wim_insrpt_registry() -> AdapterRegistry<WimInsrptWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
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
                23011 | 23012 => Ok(StorungsmeldungCommand::ReceiveInformationsmeldung {
                    pid,
                    sender,
                    message_ref,
                }),
                _ => Ok(StorungsmeldungCommand::ReceiveAntwort {
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
