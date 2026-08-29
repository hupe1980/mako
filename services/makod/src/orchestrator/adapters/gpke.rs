//! GPKE (Strom) adapter registries.
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
// ── GPKE UTILMD Anfrage (PIDs 55001, 55002, 55016) ──────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSupplierChangeWorkflow`].
///
/// Registers one adapter covering all current BDEW format versions.
/// Extracts UTILMD S2.x fields to construct a
/// [`SupplierChangeCommand::ReceiveUtilmd`] for the 3 inbound ANFRAGE PIDs:
/// 55001–55002 (Lieferbeginn/Lieferende) and 55016 (Kündigung).
/// Outbound ANTWORT PIDs (55002/55003, 55005/55006, 55017, 55018) are handled separately.
/// ORDERS Sperrung (PIDs 17115/17116/17117) uses [`gpke_sperrung_registry`].
#[must_use]
pub fn gpke_registry() -> AdapterRegistry<GpkeSupplierChangeWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE adapter".into())
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                // IFTSTA Vollzugsmeldung (PIDs 21024–21033) are also routed to
                // the gpke-supplier-change workflow. Handle them here.
                if let AnyMessage::Iftsta(_) = msg {
                    return build_gpke_iftsta_command(msg);
                }
                return Err(EngineError::Deserialization(
                    "GPKE adapter: expected UTILMD or IFTSTA message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!("GPKE adapter: PID detection failed: {e}"))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(SupplierChangeCommand::ReceiveUtilmd {
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
                // `SG4 IDE+24` DE 7402 — echoed in `SG6 RFF+TN` on the NB's
                // 55036 Information über existierende Zuordnung, where the AHB
                // marks it Muss.
                vorgangsnummer: u
                    .transactions()
                    .first()
                    .and_then(|t| t.vorgangsnummer())
                    .map(ToOwned::to_owned),
                // `SG12 NAD+Z09` — copied verbatim onto the NB's 55010, where
                // Bedingung [279] marks it Muss on a verbrauchende oder ruhende
                // Marktlokation and [572] names it „Kundenname aus Anmeldung
                // Lieferant neu".
                kunde_name: u
                    .transactions()
                    .first()
                    .and_then(|t| t.kunde())
                    .and_then(edi_energy::messages::utilmd::UtilmdParty::name),
                kunde_namensformat: u
                    .transactions()
                    .first()
                    .and_then(|t| t.kunde())
                    .and_then(|k| k.nad.name_format.clone()),
                // Bilanzierungsgebiet EIC from UTILMD NAD+Z09 / LOC+237.
                // processd NB check 4 uses this field directly; when None,
                // it falls back to marktd malo.bilanzierungsgebiet instead.
                // TODO(L1/N2): call t.bilanzierungsgebiet_eic() once edi-energy
                // exposes the LOC+237 segment accessor on UtilmdTransaction.
                bilanzierungsgebiet: None,
                // Bilanzierungsmethode from UTILMD TM+EM segment (L1/N1).
                // TM qualifier Z01 = SLP, Z02 = RLM, Z04 = IMS.
                // Extracted from the message-level raw segments: TM segment
                // immediately after the first IDE in the SG4 transaction group.
                bilanzierungsmethode: extract_bilanzierungsmethode(u.segments()),
                // Gas GaBi RLM Fallgruppe from UTILMD TM+Z10 segment (L1/N1).
                // Only populated for Gas PIDs; Strom UTILMD has no TM+Z10.
                fallgruppe: extract_fallgruppe(u.segments()),
                // SG4 STS Transaktionsgrund (Statuskategorie 7) — drives the
                // `mako-pruefung` date-plausibility rules (retroactive Einzug).
                //
                // The value is DE 9013 in C556 (`STS+7++E01'`), not DE 4405 in
                // C555: the UTILMD MIG marks C555 *nicht benutzt* for this
                // Statuskategorie, so reading it yields `None` for every
                // conformant message.
                // `STS+7++<grund>:<ergaenzung>:<befristet>` — read from the
                // positionally parsed `UtilmdTransaction::transaktionsgrund`,
                // not from the `Sts` list. `Sts::reason_code` deserializes DE
                // 9013's *first* C556 only, so a code-addressed scan for the
                // Ergänzung at element 3 could never match it.
                transaktionsgrund: u
                    .transactions()
                    .first()
                    .and_then(|t| t.transaktionsgrund())
                    .map(|g| g.grund),
                // DE 9013 element 3 — `ZW4` verbrauchende, `ZW3` erzeugende,
                // `ZAP` ruhende Marktlokation. `processd` maps it onto the
                // `mako_pruefung::Marktlokationsart` that decides which of
                // `E_0622`'s two disjoint code spaces answers.
                transaktionsgrund_ergaenzung: u
                    .transactions()
                    .first()
                    .and_then(|t| t.transaktionsgrund())
                    .and_then(|g| g.ergaenzung),
                // SG10 `CCI+Z22` DE 7037 — the Veräußerungsform of an
                // erzeugende Marktlokation.
                veraeusserungsform: extract_veraeusserungsform(u.segments()),
                message_ref: MessageRef::new(msg.message_ref()),
                received_at: time::OffsetDateTime::now_utc(),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GPKE ORDERS Sperrung (PIDs 17115, 17116, 17117) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungWorkflow`].
///
/// Extracts ORDERS 1.4b fields from an inbound Sperrauftrag / Entsperrauftrag
/// (PIDs 17115/17116/17117) to construct a [`SperrungCommand::ReceiveSperrung`].
///
/// **Message format**: ORDERS (AWH Sperrprozesse Strom, BK6-22-024).
/// The Marktlokation is carried in the LOC segment (element 1, component 0).
///
/// **PID 55555** ("Anfrage Daten der individuellen Bestellung", GPKE Teil 4)
/// is a completely separate UTILMD-based data-request process and must NOT
/// be routed to this adapter.
#[must_use]
pub fn gpke_sperrung_registry() -> AdapterRegistry<GpkeSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE Sperrung adapter".into())
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung adapter: expected ORDERS message (PIDs 17115/17116/17117)"
                        .into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // Marktlokation from the LOC segment (element 1, component 0).
            // LOC+7+<MaLo>::Z13 — element 0 = qualifier, element 1 = location composite.
            let location_id = mako_engine::types::MaLo::new(
                o.segments()
                    .iter()
                    .find(|s| s.tag == "LOC")
                    .and_then(|s| s.component_str(1, 0))
                    .unwrap_or(""),
            );

            Ok(SperrungCommand::ReceiveSperrung {
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

// ── GPKE INVOIC billing (PIDs 31001, 31002, 31004–31008) ─────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeAbrechnungWorkflow`].
///
/// Extracts INVOIC 2.x fields to construct an
/// [`InvoicCommand::ReceiveInvoic`] for any of the INVOIC-based GPKE
/// billing PIDs (31001–31008: Netznutzungsabrechnung, Mehr-/Mindermengen Strom).
#[must_use]
pub fn gpke_abrechnung_registry() -> AdapterRegistry<GpkeAbrechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Abrechnung adapter".into(),
                )
            })?;

            let AnyMessage::Invoic(inv) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Abrechnung adapter: expected INVOIC message".into(),
                ));
            };

            let pid = inv
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GPKE Abrechnung adapter: PID not found in INVOIC BGM".into(),
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

// ── GPKE billing — REMADV payment advice (PIDs 33001–33004) ──────────────────

/// Build an [`AdapterRegistry`] for REMADV 33001–33004 routed to [`GpkeAbrechnungWorkflow`].
///
/// After the NB sends an INVOIC to the LF, the LF (payer) responds with a REMADV
/// confirming or partially disputing the payment.  `makod` resumes the billing
/// process with [`InvoicCommand::ReceiveRemadv`].
///
/// **Correlation**: `extract_invoice_ref_from_remadv` reads `RFF+Z13:<invoice_ref>` to
/// map back to the spawned billing process.
///
/// Source: REMADV AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
#[must_use]
pub fn gpke_abrechnung_remadv_registry() -> AdapterRegistry<GpkeAbrechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE REMADV adapter".into())
            })?;
            let AnyMessage::Remadv(r) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE REMADV adapter: expected REMADV message (PIDs 33001–33004)".into(),
                ));
            };
            let pid = r
                .bgm()
                .and_then(|b| b.pruefidentifikator())
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "GPKE REMADV adapter: PID not found in REMADV BGM".into(),
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

// ── GPKE billing — COMDIS payment rejection (PID 29001) ──────────────────────

/// Build an [`AdapterRegistry`] for COMDIS 29001 routed to [`GpkeAbrechnungWorkflow`].
///
/// After the LF (payer) sends a REMADV, the NB (invoicer) may reject it via
/// COMDIS 29001 (Ablehnung der Zahlung).  `makod` resumes the billing process
/// with [`InvoicCommand::ReceiveComdis`].
///
/// **Correlation**: `extract_invoice_ref_from_comdis` reads `RFF+Z13:<invoice_ref>`.
///
/// Source: COMDIS AHB 1.0, GPKE Teil 2/Teil 3, BK6-24-174.
#[must_use]
pub fn gpke_abrechnung_comdis_registry() -> AdapterRegistry<GpkeAbrechnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE COMDIS adapter".into())
            })?;
            let AnyMessage::Comdis(_) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE COMDIS adapter: expected COMDIS message (PID 29001)".into(),
                ));
            };
            Ok(InvoicCommand::ReceiveComdis {
                comdis_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Konfigurationseinrichtung (PIDs 19001/19002 — ORDRSP from MSB) ───────

/// Build an [`AdapterRegistry`] for [`GpkeKonfigurationWorkflow`].
///
/// Registers one adapter covering all known BDEW format versions.
/// Extracts ORDRSP fields to construct a [`KonfigurationCommand::ReceiveOrdrsp`]
/// for inbound ORDRSP 19001 (Bestätigung) and 19002 (Ablehnung der Bestellung)
/// from the MSB in response to an outbound ORDERS 17134.
#[must_use]
pub fn gpke_konfiguration_registry() -> AdapterRegistry<GpkeKonfigurationWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Konfiguration adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Konfiguration adapter: expected ORDRSP message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Konfiguration adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // ORDRSP 19001 = Bestätigung (accept), 19002 = Ablehnung (reject).
            let accepted = pid.as_u32() == 19001;

            // For ORDRSP 19002 (Ablehnung), extract the rejection reason from
            // the first FTX segment (DE 4440 / element 3, component 0).
            let reason: Option<String> = if accepted {
                None
            } else {
                o.ftx().first().and_then(|f| f.text.clone())
            };

            Ok(KonfigurationCommand::ReceiveOrdrsp {
                pid,
                accepted,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE UTILMD Antwort (PIDs 55002/55003, 55005/55006, 55017, 55018) — LF role ──

/// Build an [`AdapterRegistry`] for [`GpkeLfAnmeldungWorkflow`].
///
/// Handles inbound NB/LFA response PIDs (55002/55003, 55005/55006, 55017/55018,
/// 55078/55080) when `makod` acts as the **Lieferant** — i.e. we previously sent
/// the ANFRAGE outbound and are now receiving the NB/LFA acknowledgement.
///
/// The AHB lays each Anwendungsfall out as the triple
/// `(Anfrage, Bestätigung, Ablehnung)`, so `accepted` is derived from the PID:
/// - 55002 (Bestätigung Anmeldung), 55005 (Bestätigung Abmeldung),
///   55017 (Bestätigung Kündigung), 55078 (Bestätigung Anmeldung erz. MaLo)
///   → `accepted = true`
/// - 55003 (Ablehnung Anmeldung), 55006 (Ablehnung Abmeldung),
///   55018 (Ablehnung Kündigung), 55080 (Ablehnung erz. MaLo)
///   → `accepted = false`
///
/// An optional rejection reason is extracted from the first `STS` segment's
/// free-text description when present.
#[must_use]
pub fn gpke_lf_anmeldung_registry() -> AdapterRegistry<GpkeLfAnmeldungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE LF-Anmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE LF-Anmeldung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE LF-Anmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // Acceptance is determined by the PID alone per BDEW GPKE AHB.
            let accepted = matches!(pid.as_u32(), 55002 | 55005 | 55017 | 55078);

            // Extract the rejection reason from the first transaction's FTX
            // segment (typically qualifier AAI or ZZZ in 55004/55006).
            // For acceptance PIDs this is typically absent; returns None.
            let reason = u
                .transactions()
                .first()
                .and_then(|tx| tx.ftx.first())
                .and_then(|f| f.text.clone());

            Ok(LfAnmeldungCommand::HandleAntwort {
                response_pid: pid,
                accepted,
                reason,
                response_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Neuanlage (PIDs 55600, 55601) ───────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeNeuanlageWorkflow`].
///
/// Adapts inbound UTILMD PIDs 55600 (neue verbrauchende MaLo) and 55601
/// (neue erzeugende MaLo) from the Lieferant to a
/// [`NeuanlageCommand::ReceiveAnmeldung`].
///
/// AHB validation is performed inline; `validation_passed` is set accordingly.
/// Acceptance/rejection is decided by the NB ERP via a subsequent
/// `SendAntwort` command.
///
#[must_use]
pub fn gpke_neuanlage_registry() -> AdapterRegistry<GpkeNeuanlageWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Neuanlage adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Neuanlage adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Neuanlage adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(NeuanlageCommand::ReceiveAnmeldung {
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
                    .and_then(|t| t.dtm.iter().find(|d| d.qualifier == "92"))
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                received_at: time::OffsetDateTime::now_utc(),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GPKE Abrechnungsdaten (PIDs 55156/55220/55673) ───────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeAbrechnungsdatenWorkflow`].
///
/// Adapts an inbound UTILMD Rückmeldung / Bestellung Abrechnungsdaten (LF → NB,
/// GPKE Teil 2 § 3.1) to a [`GpkeAbrechnungsdatenCommand::ReceiveRueckmeldung`].
/// The NB answers with an IFTSTA 21047 via a later `SendBearbeitungsstand`.
#[must_use]
pub fn gpke_abrechnungsdaten_registry() -> AdapterRegistry<GpkeAbrechnungsdatenWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Abrechnungsdaten adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Abrechnungsdaten adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Abrechnungsdaten adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (mut validation_passed, validation_errors) = super::ahb_verdict(msg);

            // 55156/55220/55673 have no imported AHB rules yet, so a pass here
            // means nothing was checked. Fail closed rather than answer a
            // message whose Bestellung was never validated.
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
                        message_ref = msg.message_ref(),
                        "GPKE Abrechnungsdaten adapter: no UTILMD AHB profile compiled for \
                         this PID — vacuous validation forced to failed."
                    );
                    validation_passed = false;
                }
            }

            Ok(GpkeAbrechnungsdatenCommand::ReceiveRueckmeldung {
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

// ── GPKE NB-initiated Lieferende (PID 55007) ─────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeLfAbmeldungWorkflow`].
///
/// Adapts inbound UTILMD PID 55007 (Ankündigung Lieferende, NB → LF) to a
/// [`LfAbmeldungCommand::ReceiveAnkuendigung`].
///
/// AHB validation is performed inline; `validation_passed` is set accordingly.
/// The LF ERP responds with a subsequent `SendAntwort` command (within 24h).
#[must_use]
pub fn gpke_lf_abmeldung_registry() -> AdapterRegistry<GpkeLfAbmeldungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE LF-Abmeldung adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE LF-Abmeldung adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE LF-Abmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (mut validation_passed, validation_errors) = super::ahb_verdict(msg);

            // ── PID 55007 — vacuous-validation guard ──────────────────────
            // PID 55007 (Ankündigung NB-seitiges Lieferende, NB → LFN) is
            // present in UTILMD AHB Strom 2.1 (FV2025-10-01) but has no
            // compiled AHB profile in the edi-energy profile set (import gap).
            // Without AHB rules, validate() returns is_valid()=true (zero
            // rules checked). Guard against that false positive by forcing
            // validation_passed=false until a proper profile is imported.
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
                        message_ref = msg.message_ref(),
                        "GPKE LF-Abmeldung adapter: PID {} (NB-seitiges Lieferende) \
                         has no UTILMD AHB profile in the compiled profile set — \
                         import gap; vacuous validation forced to failed.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

            Ok(LfAbmeldungCommand::ReceiveAnkuendigung {
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
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                // 55007 announces a Zuordnungs**ende**: `DTM+93` „Ende zum",
                // the qualifier the UTILMD AHB marks Muss on this
                // Anwendungsfall. `DTM+92` is the Beginn of an Anmeldung and is
                // absent here, so reading it left every `E_0609` walk without
                // the date its Prüfschritte 30, 40, 85 and 120 compare against.
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.date(edi_energy::utilmd_codes::dtm::ENDE_ZUM))
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                vorgang: Box::new(super::lf_vorgangsdaten(u)),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`GpkeBeendigungZuordnungWorkflow`].
///
/// Adapts inbound UTILMD PID 55010 (Anfrage zur Beendigung der Zuordnung,
/// NB → LFA) to a [`BeendigungZuordnungCommand::ReceiveAnfrage`]. The 55010 AHB
/// rulepack is authored, so `validate()` is a real MIG/AHB check.
#[must_use]
pub fn gpke_beendigung_zuordnung_registry() -> AdapterRegistry<GpkeBeendigungZuordnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Beendigung-Zuordnung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Beendigung-Zuordnung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Beendigung-Zuordnung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(BeendigungZuordnungCommand::ReceiveAnfrage {
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
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                // 55010 names a Zuordnungs**ende** — `DTM+93` Ende zum, the
                // qualifier the AHB marks Muss on this Anwendungsfall.
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.date(edi_energy::utilmd_codes::dtm::ENDE_ZUM))
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                vorgang: Box::new(super::lf_vorgangsdaten(u)),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`mako_gpke::GpkeZuordnungsmeldungWorkflow`].
///
/// Adapts an inbound UTILMD 55036 / 55037 / 55038 (NB → LFN/LFA/LFZ) to
/// [`mako_gpke::ZuordnungsmeldungCommand::Empfangen`].
///
/// A Zuordnungs-Meldung has **no Antwortnachricht**, so nothing here derives a
/// response PID or a business deadline. What the adapter does have to carry is
/// the `SG4 STS+7` Grund: it is the whole content of a 55037/55038 — which of
/// `ZC8`/`ZD9`/`ZG6` ended the Zuordnung, which of `ZG5`/`ZG9`/`ZH0`/`ZH1`
/// cancelled it — and a supplier that drops it learns only that something
/// happened.
#[must_use]
pub fn gpke_zuordnungsmeldung_registry() -> AdapterRegistry<mako_gpke::GpkeZuordnungsmeldungWorkflow>
{
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Zuordnungsmeldung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Zuordnungsmeldung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Zuordnungsmeldung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(mako_gpke::ZuordnungsmeldungCommand::Empfangen {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                // `SG5 LOC+Z16` Marktlokation or `LOC+Z21` Tranche — both carry
                // a MaLo-ID, so one accessor covers the pair.
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
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`mako_gpke::GpkeKuendigungWorkflow`].
///
/// Adapts inbound UTILMD PID 55016 (Kündigung, LFN → LFA) to a
/// [`mako_gpke::KuendigungCommand::ReceiveKuendigung`]. The Kündigungstermin is `DTM+93`.
#[must_use]
pub fn gpke_kuendigung_registry() -> AdapterRegistry<mako_gpke::GpkeKuendigungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Kündigung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Kündigung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Kündigung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            Ok(mako_gpke::KuendigungCommand::ReceiveKuendigung {
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
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                // The Kündigungstermin: `DTM+93` Ende zum.
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.date(edi_energy::utilmd_codes::dtm::ENDE_ZUM))
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                vorgang: Box::new(super::lf_vorgangsdaten(u)),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`GpkeAnkuendigungZuordnungLfWorkflow`].
///
/// Adapts inbound UTILMD PID 55607 (Ankündigung Zuordnung LF, NB → LFN) to an
/// [`AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung`].
///
/// AHB validation is performed inline; `validation_passed` is set accordingly.
/// The LFN ERP responds with a subsequent `SendAntwort` command (within 24h,
/// BK6-22-024 §4).
#[must_use]
pub fn gpke_ankuendigung_zuordnung_lf_registry()
-> AdapterRegistry<GpkeAnkuendigungZuordnungLfWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Ankündigung Zuordnung LF adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Ankündigung Zuordnung LF adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Ankündigung Zuordnung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (mut validation_passed, validation_errors) = super::ahb_verdict(msg);

            // ── PID 55607 — vacuous-validation guard ──────────────────────
            // PID 55607 (Ankündigung Zuordnung LF, NB → LFN) requires AHB
            // profile import before full validation is possible.
            // Guard against false positives from empty rule sets.
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
                        message_ref = msg.message_ref(),
                        "GPKE Ankündigung Zuordnung LF adapter: PID {} has no UTILMD AHB \
                         profile in the compiled profile set — import gap; \
                         vacuous validation forced to failed.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }

            Ok(AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
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
                document_date: u
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                // 55607 announces a Zuordnungs**beginn**: `DTM+92` „Beginn zum".
                process_date: u
                    .transactions()
                    .first()
                    .and_then(|t| t.date(edi_energy::utilmd_codes::dtm::BEGINN_ZUM))
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                vorgang: Box::new(super::lf_vorgangsdaten(u)),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── GPKE PARTIN Kommunikationsdaten (PIDs 37000–37006) ───────────────────────

/// Build an [`AdapterRegistry`] for [`GpkePartinWorkflow`].
///
/// Handles all inbound PARTIN messages with PIDs 37000–37006 (Strom
/// Kommunikationsdaten). Produces [`KommunikationsdatenCommand::ReceivePartin`].
#[must_use]
pub fn gpke_partin_registry() -> AdapterRegistry<GpkePartinWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE PARTIN adapter".into())
            })?;
            let AnyMessage::Partin(p) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE PARTIN adapter: expected PARTIN message (PIDs 37000–37006)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE PARTIN adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            Ok(KommunikationsdatenCommand::ReceivePartin {
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

// ── GPKE MSCONS Messwerte (PIDs 13002, 13005–13006) ──────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeMesswerteLieferungWorkflow`].
///
/// Handles inbound MSCONS metered-data messages from NB/MSB to LF. The
/// delivery location MaLo is extracted from the first SG5 NAD segment.
/// Produces [`MesswerteLieferungCommand::ReceiveMscons`].
#[must_use]
pub fn gpke_messwerte_registry() -> AdapterRegistry<GpkeMesswerteLieferungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Messwerte adapter".into(),
                )
            })?;
            let AnyMessage::Mscons(m) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Messwerte adapter: expected MSCONS message (PIDs 13002, 13005–13006)"
                        .into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Messwerte adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            // `SG6 LOC+172` is where the AHB puts the location — MSCONS AHB
            // 3.1g §11.2 gives `SG5 NAD` only DE 3035 = `DP`, with the
            // identifier in `LOC` DE 3225. Reading `NAD` first worked only
            // because mako's own renderer fills both; a conformant message
            // from a third party carries the MaLo in `LOC` alone, and the
            // empty location that produced is the field `edmd` refuses the
            // whole event on. `NAD` remains the fallback for the older use
            // cases that do carry it there.
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
            // The readings, not just the fact that a delivery arrived. They
            // used to be decoded into `m` and dropped here.
            let (reads, undated) = super::mscons_intervals(m);
            if undated > 0 {
                tracing::warn!(
                    pid = pid.as_u32(),
                    undated,
                    "MSCONS: readings skipped — no SG10 DTM+163/164 in format 303",
                );
            }
            Ok(MesswerteLieferungCommand::ReceiveMscons {
                pid,
                sender: MarktpartnerCode::new(
                    m.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                location_id,
                reads,
                document_date: m
                    .dtm()
                    .iter()
                    .find(|d| d.is_document_date())
                    .and_then(|d| d.value_str())
                    .unwrap_or("")
                    .to_owned(),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
                // `SG1 RFF+AGI` — Muss on a 13027 „Werte nach Typ 2" (MSCONS
                // AHB 3.2 §11.2 hint [574]) and the only thing on the delivery
                // that names the ESA subscription it belongs to. Absent on
                // every other MSCONS PID, which have no subscription.
                bestellung_ref: msg
                    .segments()
                    .iter()
                    .find(|s| s.tag == "RFF" && s.component_str(0, 0) == Some("AGI"))
                    .and_then(|s| s.component_str(0, 1))
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned),
            })
        },
    ));
    registry
}

// ── GPKE UTILTS Konfigurationsdaten (PIDs 11002, 11003, …) ───────────────────

/// Build an [`AdapterRegistry`] for [`GpkeUtiltsWorkflow`].
///
/// Handles inbound UTILTS configuration-data messages for GPKE Teil 3.
/// Produces [`UtiltsKonfigCommand::ReceiveUtilts`].
#[must_use]
pub fn gpke_utilts_registry() -> AdapterRegistry<GpkeUtiltsWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE UTILTS adapter".into())
            })?;
            let AnyMessage::Utilts(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE UTILTS adapter: expected UTILTS message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE UTILTS adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            Ok(UtiltsKonfigCommand::ReceiveUtilts {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
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

// ── GPKE Konfigurationsänderung ORDRSP (PIDs 17102, 17113) ───────────────────

/// Build an [`AdapterRegistry`] for [`GpkeKonfigurationAenderungWorkflow`].
///
/// Handles inbound ORDRSP messages (NB/MSB response to LF config-change
/// request). Produces [`KonfigurationAenderungCommand::ReceiveOrdrsp`].
///
/// The `accepted` flag is set to `true` when the ORDRSP BGM response code
/// indicates acceptance (`27` = accepted without amendment). Any other
/// response is treated as a rejection and `accepted` is `false`.
#[must_use]
pub fn gpke_konfiguration_aenderung_registry() -> AdapterRegistry<GpkeKonfigurationAenderungWorkflow>
{
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Konfigurationsänderung adapter".into(),
                )
            })?;
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Konfigurationsänderung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // IFTSTA 21043/21044 (Bestellungsantwort/-beendigung) — informational.
            if let AnyMessage::Iftsta(_) = msg {
                return Ok(KonfigurationAenderungCommand::ReceiveIftsta {
                    pid,
                    message_ref: MessageRef::new(msg.message_ref()),
                });
            }

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Konfigurationsänderung adapter: expected ORDRSP (17102/17113) or \
                     IFTSTA (21043/21044) message"
                        .into(),
                ));
            };
            let ordrsp_pid = pid;
            // BGM response code 27 = accepted without amendment; anything else = rejection.
            let (accepted, reason) = {
                let code = o
                    .segments()
                    .iter()
                    .find(|s| s.tag == "BGM")
                    .and_then(|s| s.component_str(2, 0));
                let accepted = code == Some("27");
                let reason = if accepted {
                    None
                } else {
                    Some(format!(
                        "ORDRSP response code: {}",
                        code.unwrap_or("unknown")
                    ))
                };
                (accepted, reason)
            };
            Ok(KonfigurationAenderungCommand::ReceiveOrdrsp {
                ordrsp_pid,
                accepted,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Datenabruf ORDRSP / Ablehnung (PIDs 17102, 17113) ───────────────────

/// Build an [`AdapterRegistry`] for [`GpkeDatanabrufWorkflow`].
///
/// The Datenabruf process is LF-initiated (outbound ORDERS); the only inbound
/// message is a rejection ORDRSP from NB/MSB. Produces
/// [`DatanabrufCommand::ReceiveAblehnung`].
#[must_use]
pub fn gpke_datenabruf_registry() -> AdapterRegistry<GpkeDatanabrufWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Datenabruf adapter".into(),
                )
            })?;
            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Datenabruf adapter: expected ORDRSP message (PIDs 17102, 17113)".into(),
                ));
            };
            let ordrsp_pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Datenabruf adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            // FTX C108 free text — the fourth element (4451, 4453, C107, C108),
            // index 3 zero-based.
            let reason = o
                .segments()
                .iter()
                .find(|s| s.tag == "FTX")
                .and_then(|s| s.component_str(3, 0))
                .map(|s| s.to_owned());
            Ok(DatanabrufCommand::ReceiveAblehnung {
                ordrsp_pid,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Stornierung (PIDs 55022–55024) ───────────────────────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeStornierungWorkflow`].
///
/// Routes UTILMD Strom messages with PIDs 55022–55024 (GPKE Stornierung):
/// - 55022 — Anfrage nach Stornierung (LFN → NB)
/// - 55023 — Bestätigung Stornierung  (NB response — accepted)
/// - 55024 — Ablehnung Stornierung    (NB response — rejected)
///
/// **APERAK Frist:** 45 Minuten für eine UTILMD (APERAK AHB 1.0 § 2.4.1).
#[must_use]
pub fn gpke_stornierung_registry() -> AdapterRegistry<GpkeStornierungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Stornierung adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Stornierung adapter: PID detection failed: {e}"
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
                        "GPKE Stornierung adapter: PID {} has no UTILMD AHB profile — \
                         validation was vacuous. Import profile with `cargo xtask import-xml-ahb`.",
                        pid.as_u32(),
                    );
                    validation_passed = false;
                }
            }
            Ok(GpkeStornierungCommand::ReceiveUtilmd {
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

// ── GPKE Anfrage Daten der individuellen Bestellung (PID 55555) ───────────────

/// Build an [`AdapterRegistry`] for [`GpkeAnfrageBestellungWorkflow`].
///
/// Routes UTILMD Strom messages with PID 55555 (GPKE Teil 4, BK6-22-024 Anlage 1d):
///
/// **Message format**: UTILMD Strom S2.x (`AnyMessage::Utilmd`).
/// **APERAK Frist:** 45 Minuten für eine UTILMD (APERAK AHB 1.0 § 2.4.1).
///
/// The key fields extracted from the UTILMD message are:
/// - `pid` — must be 55555
/// - `sender` / `receiver` — from NAD+MS / NAD+MR party identifiers
/// - `vorgang_id` — from `SG4 IDE+24` DE 7402 (the queried order)
/// - `bearbeitungsstatus` — from `STS` DE 9015 qualifier (`"E07"` or `"E08"`)
/// - `document_date` — from `DTM+137`
#[must_use]
pub fn gpke_anfrage_bestellung_registry() -> AdapterRegistry<GpkeAnfrageBestellungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE AnfrageBestellung adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE AnfrageBestellung adapter: expected UTILMD message (PID 55555)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE AnfrageBestellung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // Vorgangsnummer from `IDE+24` DE 7402 — this one really is a
            // Vorgang reference, not a Lokations-ID.
            let vorgang_id = MaLo::new(
                u.transactions()
                    .first()
                    .and_then(|t| t.vorgangsnummer())
                    .unwrap_or(""),
            );

            // Bearbeitungsstatus from the first STS segment, element 0 (DE 9015).
            // Expected values: "E07" (known/confirmed Vorgang) or "E08" (unconfirmed).
            let bearbeitungsstatus = u
                .transactions()
                .first()
                .and_then(|t| t.sts.first())
                .and_then(|s| s.category.as_deref())
                .unwrap_or("")
                .to_owned();

            Ok(AnfrageBestellungCommand::ReceiveAnfrage {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                vorgang_id,
                bearbeitungsstatus,
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

// ── GPKE Sperrung NB — MSB response (ORDRSP 19118/19119) ─────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungWorkflow`] (MSB → NB direction).
///
/// Routes ORDRSP 19118 (Bestätigung Anfrage Sperrung) and 19119 (Ablehnung
/// Anfrage Sperrung) from the MSB to the NB-side `gpke-sperrung` workflow via
/// [`SperrungCommand::ReceiveMsbAntwort`].
///
/// This is a **response adapter** — it is only used by the ingest dispatcher
/// to continue an existing NB-side process once the MSB answers the Anfrage
/// Sperrung (PID 17116).  It is distinct from [`gpke_sperrung_registry`] which
/// handles the inbound Sperrauftrag (PIDs 17115/17117).
///
/// **Loopback use**: in an integrated NB+MSB deployment (same MP-ID), the
/// outbox ORDRSP 19118/19119 emitted by the MSB side loops back via the
/// [`crate::ingest_dispatcher`] to complete the NB process.
#[must_use]
pub fn gpke_sperrung_msb_response_registry() -> AdapterRegistry<GpkeSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Sperrung MSB-response adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung MSB-response adapter: expected ORDRSP message \
                     (PIDs 19118/19119)"
                        .into(),
                ));
            };
            let _ = o; // sender extracted below

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung MSB-response adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // 19118 = Bestätigung (MSB confirms meter access).
            // 19119 = Ablehnung  (MSB cannot confirm meter access).
            let is_confirmed = pid.as_u32() == 19118;

            Ok(SperrungCommand::ReceiveMsbAntwort {
                pid,
                is_confirmed,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Sperrung NB — LF Stornierung (ORDCHG 39000) ─────────────────────────

/// Build an [`AdapterRegistry`] for ORDCHG 39000 routed to [`GpkeSperrungWorkflow`]
/// (NB side).
///
/// The LF cancels a pending Sperrauftrag with ORDCHG 39000 (Stornierung); `makod`
/// resumes the NB-side process with [`SperrungCommand::ReceiveStornierung`]. ORDCHG
/// carries no LOC — the process is correlated by the original order reference
/// (RFF+ON, the Belegnummer the 17115 spawn indexed the process under).
#[must_use]
pub fn gpke_sperrung_stornierung_registry() -> AdapterRegistry<GpkeSperrungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Sperrung Stornierung adapter".into(),
                )
            })?;
            let AnyMessage::Ordchg(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Sperrung Stornierung adapter: expected ORDCHG message (PID 39000)".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung Stornierung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            Ok(SperrungCommand::ReceiveStornierung {
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

// ── GPKE Sperrung LF side (ORDRSP 19116/19117 — NB → LF) ────────────────────

/// Build an [`AdapterRegistry`] for [`GpkeSperrungLfWorkflow`].
///
/// Routes ORDRSP 19116 (Bestätigung Sperr-/Entsperrauftrag, NB → LF) and
/// 19117 (Ablehnung) to [`SperrungLfCommand::ReceiveOrdrsp`].
///
/// This is a **response adapter** used by the ingest dispatcher to continue
/// the LF-side process once the NB responds to the Sperrauftrag.
///
/// **Loopback use**: in an integrated NB+LF deployment (same MP-ID), the
/// outbox ORDRSP 19116/19117 emitted by the NB side loops back via the
/// [`crate::ingest_dispatcher`] to complete the LF process.
#[must_use]
pub fn gpke_sperrung_lf_registry() -> AdapterRegistry<GpkeSperrungLfWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Sperrung LF adapter".into(),
                )
            })?;

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Sperrung LF adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let message_ref = MessageRef::new(msg.message_ref());

            match msg {
                AnyMessage::Ordrsp(o) => {
                    let sender = MarktpartnerCode::new(
                        o.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                    );
                    match pid.as_u32() {
                        // Bestätigung/Ablehnung of the LF's Sperrauftrag (ORDERS 17115).
                        19116 | 19117 => Ok(SperrungLfCommand::ReceiveOrdrsp {
                            pid,
                            is_confirmed: pid.as_u32() == 19116,
                            message_ref,
                            sender,
                            reason: None,
                        }),
                        // Bestätigung/Ablehnung of the LF's Stornierung (ORDCHG 39000).
                        19128 | 19129 => Ok(SperrungLfCommand::ReceiveStornoOrdrsp {
                            pid,
                            is_confirmed: pid.as_u32() == 19128,
                            message_ref,
                            sender,
                        }),
                        other => Err(EngineError::Deserialization(format!(
                            "GPKE Sperrung LF adapter: unexpected ORDRSP PID {other} \
                             (expected 19116/19117/19128/19129)"
                        ))),
                    }
                }
                // IFTSTA 21039: Auftragsstatus after Sperrung execution (NB → LF).
                AnyMessage::Iftsta(i) => Ok(SperrungLfCommand::ReceiveIftsta {
                    pid,
                    message_ref,
                    sender: MarktpartnerCode::new(
                        i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                    ),
                }),
                _ => Err(EngineError::Deserialization(
                    "GPKE Sperrung LF adapter: expected ORDRSP (19116/19117/19128/19129) or \
                     IFTSTA (21039)"
                        .into(),
                )),
            }
        },
    ));
    registry
}

// ── GPKE Allokationsliste — ORDRSP rejection (PIDs 19110/19115) ───────────────

/// Build an [`AdapterRegistry`] for [`GpkeAllokationslisteWorkflow`] (ORDRSP path).
///
/// Handles inbound ORDRSP 19110 (Ablehnung Allokationsliste) and 19115
/// (Ablehnung Anforderung bilanzierte Menge) from the NB. Both are negative
/// responses to an LF-initiated ORDERS 17110/17114 request.
///
/// **Regulatory basis**: GPKE / MMM Strom/Gas (BK6-22-024 §8).
#[must_use]
pub fn gpke_allokationsliste_ordrsp_registry() -> AdapterRegistry<GpkeAllokationslisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Allokationsliste ORDRSP adapter".into(),
                )
            })?;

            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Allokationsliste ORDRSP adapter: expected ORDRSP message (PIDs 19110/19115)".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Allokationsliste ORDRSP adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // Extract optional rejection reason from FTX free-text segment.
            let reason: Option<String> = o.ftx().first().and_then(|f| f.text.clone());

            Ok(AllokationslisteCommand::ReceiveAblehnung {
                ordrsp_pid: pid,
                reason,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Allokationsliste — MSCONS data delivery (PIDs 13013/13014) ───────────

/// Build an [`AdapterRegistry`] for [`GpkeAllokationslisteWorkflow`] (MSCONS path).
///
/// Handles inbound MSCONS 13013 (Marktlokationsscharfe Allokationsliste Gas)
/// and 13014 (Marktlokationsscharfe bilanzierte Menge) — the positive response
/// to an LF-initiated ORDERS 17110/17114 request.
///
/// These are **MMM Strom/Gas** PIDs, NOT GeLi Gas. They arrive at the LF
/// after the NB fulfils the allocation-list request.
///
/// **Regulatory basis**: GPKE / MMM Strom/Gas (BK6-22-024 §8).
#[must_use]
pub fn gpke_allokationsliste_mscons_registry() -> AdapterRegistry<GpkeAllokationslisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Allokationsliste MSCONS adapter".into(),
                )
            })?;

            // Accept MSCONS 13013/13014 only.
            if !matches!(msg, AnyMessage::Mscons(_)) {
                return Err(EngineError::Deserialization(
                    "GPKE Allokationsliste MSCONS adapter: expected MSCONS message (PIDs 13013/13014)".into(),
                ));
            }

            Ok(AllokationslisteCommand::NotifyDatenGeliefert {
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── GPKE Ersatz-/Grundversorgung (PIDs 55013–55015) ───────────────────────────

/// Build an [`AdapterRegistry`] for [`mako_gpke::GpkeEogWorkflow`].
///
/// Adapts inbound UTILMD:
///
/// - **PID 55013** (Anmeldung / Zuordnung EOG, NB → LF) to
///   [`mako_gpke::EogCommand::ReceiveAnmeldung`] — the E/G responder side.
/// - **PIDs 55014/55015** (Bestätigung / Ablehnung, LF → NB) to
///   [`mako_gpke::EogCommand::ReceiveAntwort`] — resumes the NB initiator.
///
/// The Transaktionsgrund is read from the first `SG4 STS` with category `7`,
/// the Haushaltskunde flag from SG10 `CCI` Z15/Z18, and the response
/// Versorgungsart from SG10 `CCI+Z36` (ZC9/ZD0/ZE3). When the compiled AHB
/// rulepack for 55013–55015 is present the message is validated against it;
/// otherwise structural (MIG) validation applies and the vacuous AHB pass is
/// accepted with a warning (like the Gas twin 44013).
#[must_use]
pub fn gpke_eog_registry() -> AdapterRegistry<mako_gpke::GpkeEogWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for GPKE EoG adapter".into())
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE EoG adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE EoG adapter: PID detection failed: {e}"
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
            let location_id = MaLo::new(first_tx.and_then(|t| t.lokation()).unwrap_or(""));

            match pid.as_u32() {
                55013 => {
                    let (validation_passed, validation_errors) = super::ahb_verdict(msg);
                    // 55013 now has an authored AHB rulepack (UTILMD AHB Strom 2.2
                    // Kap. 2 — BGM+E01, SG4 IDE 7495=24, STS 9013=E06
                    // Ersatzbelieferung), so `validate()` is a real MIG/AHB check.
                    let transaktionsgrund = first_tx
                        .and_then(|t| {
                            t.sts
                                .iter()
                                .find(|s| s.category.as_deref() == Some("7"))
                                .and_then(|s| s.status_code.clone())
                        })
                        .unwrap_or_default();
                    Ok(mako_gpke::EogCommand::ReceiveAnmeldung {
                        pid,
                        sender,
                        receiver,
                        location_id,
                        document_date: u
                            .dtm()
                            .iter()
                            .find(|d| d.is_document_date())
                            .and_then(|d| d.value_str())
                            .unwrap_or("")
                            .to_owned(),
                        process_date: first_tx
                            .and_then(|t| {
                                t.dtm
                                    .iter()
                                    .find(|d| d.qualifier == "92" || d.qualifier == "163")
                            })
                            .and_then(|d| d.value_str())
                            .unwrap_or("")
                            .to_owned(),
                        message_ref: MessageRef::new(msg.message_ref()),
                        transaktionsgrund,
                        // SG10 CCI Z15/Z18 household indicator (best-effort).
                        haushaltskunde: super::extract_haushaltskunde(u.segments()),
                        validation_passed,
                        validation_errors,
                        received_at: time::OffsetDateTime::now_utc(),
                    })
                }
                55014 | 55015 => Ok(mako_gpke::EogCommand::ReceiveAntwort {
                    response_pid: pid,
                    accepted: pid.as_u32() == 55014,
                    // SG10 CCI+Z36 Versorgungsart (ZC9/ZD0/ZE3). When absent
                    // marktd defaults to Ersatzversorgung (§38 ipso iure).
                    versorgungsart: super::extract_versorgungsart(u.segments())
                        .and_then(|c| mako_gpke::Versorgungsart::from_code(&c)),
                    // The E/G's Bilanzkreis is resolved from the deposited
                    // default-BK (GrundversorgerRecord) on the marktd side, not
                    // parsed here — the 55014 BK segment placement is AHB-version
                    // dependent.
                    bilanzkreis: None,
                    reason: None,
                }),
                other => Err(EngineError::Deserialization(format!(
                    "GPKE EoG adapter: unexpected PID {other} (expected 55013–55015)"
                ))),
            }
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for [`mako_gpke::GpkeStammdatenaenderungWorkflow`].
///
/// Adapts inbound UTILMD GPKE Teil 4 Stammdatenänderung:
/// - an **Änderung** PID → [`mako_gpke::StammdatenCommand::ReceiveAenderung`]
///   (the Berechtigter applies MaLo attribute changes and answers).
/// - a **Rückmeldung** PID → [`mako_gpke::StammdatenCommand::ReceiveRueckmeldung`]
///   (resumes a change we initiated).
///
/// No AHB rulepack exists for these PIDs (like EoG) — structural validation
/// only. The MaLo attribute patch is built from the `TM` segments the extract
/// helpers already read (`bilanzierungsmethode`, `fallgruppe`); netzebene /
/// energierichtung / regelzone extraction awaits the AHB-profile import
/// (roadmap).
#[must_use]
pub fn gpke_stammdaten_registry() -> AdapterRegistry<mako_gpke::GpkeStammdatenaenderungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Stammdaten adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Stammdaten adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Stammdaten adapter: PID detection failed: {e}"
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
            let location_id = MaLo::new(first_tx.and_then(|t| t.lokation()).unwrap_or(""));
            let aenderungsdatum = first_tx
                .and_then(|t| t.dtm.iter().find(|d| d.is_period_start()))
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();

            let pid_u32 = pid.as_u32();
            if mako_gpke::is_rueckmeldung_pid(pid_u32) {
                // Resume our initiated change — the Rückmeldung reports A01/A02.
                let qualitaet = if u
                    .transactions()
                    .first()
                    .and_then(|t| {
                        t.sts
                            .iter()
                            .find(|s| s.category.as_deref() == Some("E01"))
                            .and_then(|s| s.status_code.clone())
                    })
                    .as_deref()
                    == Some("A02")
                {
                    mako_gpke::Qualitaet::UebernommenMitKorrektur
                } else {
                    mako_gpke::Qualitaet::Uebernommen
                };
                return Ok(mako_gpke::StammdatenCommand::ReceiveRueckmeldung {
                    response_pid: pid,
                    qualitaet,
                });
            }

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // Build the MaLo attribute patch. `TM`-carried attributes
            // (Bilanzierungsmethode, Fallgruppe) plus the SG8 `SEQ`/SG10
            // `CCI`/`CAV`-carried attributes (Netzebene, Energierichtung,
            // Regelzone, Bilanzierungsgebiet). Each extractor is best-effort —
            // absent segments simply leave that column untouched (COALESCE).
            let segs = u.segments();
            let mut patch = serde_json::Map::new();
            if let Some(b) = extract_bilanzierungsmethode(segs) {
                patch.insert("bilanzierungsmethode".into(), b.into());
            }
            if let Some(f) = extract_fallgruppe(segs) {
                patch.insert("fallgruppe".into(), f.into());
            }
            if let Some(n) = super::extract_netzebene(segs) {
                patch.insert("netzebene".into(), n.into());
            }
            if let Some(e) = super::extract_energierichtung(segs) {
                patch.insert("energierichtung".into(), e.into());
            }
            if let Some(r) = super::extract_regelzone(segs) {
                patch.insert("regelzone".into(), r.into());
            }
            if let Some(bg) = super::extract_bilanzierungsgebiet(segs) {
                patch.insert("bilanzierungsgebiet".into(), bg.into());
            }
            if let Some(fs) = super::extract_fernsteuerbarkeit(segs) {
                patch.insert("fernsteuerbar".into(), fs.into());
            }
            if let Some(sk) = super::extract_steuerkanal(segs) {
                patch.insert("steuerkanal".into(), sk.into());
            }
            if let Some(fsb) = super::extract_ist_fernschaltbar(segs) {
                patch.insert("ist_fernschaltbar".into(), fsb.into());
            }
            if let Some(n) = super::extract_tr_nutzung(segs) {
                patch.insert("nutzung".into(), n.into());
            }
            if let Some(v) = super::extract_tr_verbrauchsart(segs) {
                patch.insert("verbrauchsart".into(), v.into());
            }
            if let Some(kp) = super::extract_sr_konfigurationsprodukte(segs) {
                patch.insert("konfigurationsprodukte".into(), kp);
            }
            if let Some(msb) = super::extract_zugeordneter_msb(segs) {
                patch.insert("zugeordneter_msb".into(), msb.into());
            }

            Ok(mako_gpke::StammdatenCommand::ReceiveAenderung {
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

/// Build an [`AdapterRegistry`] for the **LFA's answer** to the NB's Anfrage
/// zur Beendigung der Zuordnung (UTILMD 55011 / 55012).
///
/// The counterpart of [`gpke_beendigung_zuordnung_registry`], which adapts the
/// 55010 for the LFA. This one runs on the **NB** and produces
/// [`mako_gpke::BeendigungZuordnungCommand::ReceiveAntwort`].
///
/// The Antwortcode is what matters: `E_0623` Prüfschritt 50 asks whether the
/// Widerspruch was `A30`, and Prüfschritt 40 whether there was one at all. The
/// **Cluster** decides that, not the response PID — an LFA that answers 55012
/// with a Zustimmungscode has contradicted itself, and `mako-pruefung`
/// escalates rather than guessing.
#[must_use]
pub fn gpke_beendigung_zuordnung_antwort_registry()
-> AdapterRegistry<GpkeBeendigungZuordnungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for the Beendigung-Zuordnung answer adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "Beendigung-Zuordnung answer adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "Beendigung-Zuordnung answer adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let tx = u.transactions();
            let vorgang = tx.first();
            let antwort = vorgang.and_then(|t| t.antwort());
            let antwortcode = antwort.map(|a| a.code.clone()).ok_or_else(|| {
                EngineError::Deserialization(
                    "the answer to an Anfrage zur Beendigung der Zuordnung carries no \
                     SG4 STS+E01 — the AHB marks it Muss, and E_0624's Cluster is what \
                     decides whether the Zuordnung was released"
                        .into(),
                )
            })?;
            // The Cluster, resolved against `E_0624`, and not the response PID:
            // the code is the substance and the PID only its envelope.
            let zustimmung = mako_pruefung::codes::lookup(
                mako_pruefung::codes::EBD_BEENDIGUNG_ZUORDNUNG,
                &antwortcode,
            )
            .is_some_and(|c| c.cluster == mako_pruefung::codes::Cluster::Zustimmung);

            Ok(mako_gpke::BeendigungZuordnungCommand::ReceiveAntwort {
                response_pid: pid,
                antwortcode,
                zustimmung,
                // `FTX+ACB` — „Hierbei übermittelt der LFA eine Begründung für
                // den Widerspruch", which the NB restates on its own Ablehnung.
                grund: vorgang.and_then(|t| {
                    t.ftx
                        .iter()
                        .find(|f| f.qualifier == "ACB")
                        .and_then(|f| f.text.clone())
                }),
                // **Fall b** — the LFA agreed to an earlier Zuordnungsende and
                // „teilt sein Lieferendedatum in der Antwort mit" (`A34`).
                zuordnungsende: vorgang
                    .and_then(|t| t.date(edi_energy::utilmd_codes::dtm::ENDE_ZUM))
                    .map(ToOwned::to_owned),
            })
        },
    ));
    registry
}
