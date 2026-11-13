//! MaBiS adapter registries.
//!
//! Split out of the flat `adapters` module; shared helpers live in `super`.

use super::*;
use crate::orchestrator::ingest_dispatcher::extract_malo_from_msg;
use mako_mabis::{
    AbonnementVorgang, AnforderungCommand, BillingCommand, CCI_BEZEICHNUNG_SUMMENZEITREIHE,
    CCI_KLASSENTYP_VERANTWORTLICHER, ListenabgleichCommand, MabisAnforderungWorkflow,
    MabisListenabgleichWorkflow, MabisProfilWorkflow, MabisZpLifecycleWorkflow, ProfilCommand,
    ZP_FAMILIEN, ZpLifecycleCommand, ZpSerie, ZpVorgang,
};
// ── MABIS Bilanzkreisabrechnung (PID 13003) ───────────────────────────────────

/// Build an [`AdapterRegistry`] for [`MabisBillingWorkflow`].
///
/// MaBiS billing commands (`ReceiveSummenzeitreihe`, `ReceivePruefmitteilung`,
/// …) for MSCONS PID 13003 are constructed by the billing aggregation layer,
/// not by direct EDIFACT downcast. However, inbound **IFTSTA** messages with
/// MaBiS PIDs 21000–21007 must be handled here so they are not dead-lettered.
///
/// The adapter matches on `AnyMessage::Iftsta` and constructs
/// [`BillingCommand::ReceiveIftsta`]. Any other message type is rejected with
/// an error directing callers to use the aggregation layer.
#[must_use]
/// The `CCYYMM` Bilanzierungsmonat at the head of a `DTM` value.
///
/// The value is a wire string a counterparty controls. `str::get` is what makes
/// this safe: a byte length of six says nothing about char boundaries, and a
/// UNOC:3 interchange can put a multi-byte character in the sixth position, so
/// a naive `&raw[..6]` panics the ingest task on a malformed message. An
/// unusable date yields the empty period, which is what the callers already
/// fall back to.
fn billing_period_head(raw: &str) -> BillingPeriod {
    BillingPeriod::new(raw.get(..6).unwrap_or(""))
}

pub fn mabis_registry() -> AdapterRegistry<MabisBillingWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        // Accept all FVs.
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for MABIS adapter".into())
            })?;
            match msg {
                AnyMessage::Iftsta(_) => build_mabis_iftsta_command(msg),
                _ => Err(EngineError::Deserialization(
                    "MABIS: MSCONS billing commands must be constructed via the \
                     aggregation layer, not directly from a single EDIFACT message"
                        .into(),
                )),
            }
        },
    ));
    registry
}

// ── MaBiS Clearingliste (PIDs 55065, 55069, 55070) ────────────────────────────

/// Build an [`AdapterRegistry`] for [`MabisClearinglisteWorkflow`].
///
/// Handles inbound UTILMD Clearingliste messages in the MaBiS settlement cycle:
///
/// | PID   | Process name (AHB)              | Direction       |
/// |-------|---------------------------------|-----------------|
/// | 55065 | Lieferantenclearingliste        | NB → LF         |
/// | 55069 | Clearingliste DZR               | BIKO → NB / ÜNB |
/// | 55070 | Clearingliste BAS               | BIKO → BKV      |
///
/// Extracts UTILMD header fields (sender, receiver, document date, message ref)
/// and constructs a [`ClearinglisteCommand::ReceiveClearingliste`].
///
/// The `billing_period` field is derived from the UTILMD document date
/// (DTM qualifier `"137"`) by truncating to `YYYYMM` format. If no document
/// date segment is present, the field is left empty.
///
/// **Regulatory basis**: BNetzA BK6-24-174 Anlage 3 MaBiS — Clearingverfahren.
/// No outbound APERAK deadline is associated with receiving these messages.
#[must_use]
pub fn mabis_clearingliste_registry() -> AdapterRegistry<MabisClearinglisteWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Clearingliste adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Clearingliste adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Clearingliste adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // ── Field extraction ──────────────────────────────────────────
            let document_date_str = u
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();

            // Derive billing period from document date: `YYYYMMDD` → `YYYYMM`.
            let billing_period = billing_period_head(&document_date_str);

            Ok(ClearinglisteCommand::ReceiveClearingliste {
                pid,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                billing_period,
                document_date: document_date_str,
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── MaBiS-ZP lifecycle (UTILMD Aktivierung/Deaktivierung) ─────────────────────

/// Build an [`AdapterRegistry`] for [`MabisZpLifecycleWorkflow`].
///
/// Converts an inbound UTILMD carrying one of the lifecycle Anfrage PIDs into
/// [`ZpLifecycleCommand::ReceiveAnfrage`]. The PID→family mapping lives in
/// `mako_mabis::zp_lifecycle`; this adapter only extracts fields.
///
/// The MaBiS-Zählpunkt is read from the `LOC` segment. It is the identifier the
/// whole family is keyed on, so an Anfrage without one cannot be correlated to
/// anything — the workflow records it as received and the empty value surfaces
/// in the read model rather than being silently substituted.
#[must_use]
pub fn mabis_zp_lifecycle_registry(
    aktiviert: Option<ZpSerie>,
) -> AdapterRegistry<MabisZpLifecycleWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS-ZP lifecycle adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS-ZP lifecycle adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS-ZP lifecycle adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            let document_date = u
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();

            let billing_period = billing_period_head(&document_date);

            // 55062/55063 are shared by eleven Summenzeitreihen, so the PID
            // alone does not say what was activated. The Aktivierung 55062
            // carries the discriminator: `SG10 CCI+++ZB4 / CAV` DE 7111 names
            // the Summenzeitreihe and `SG10 CCI+6` DE 7037 the responsible role
            // (UTILMD AHB Strom 2.2 Kap. 13.1, S. 1100). The Deaktivierung 55063
            // carries neither, so it takes the series the caller read off the
            // MaBiS-ZP's activation (`aktiviert`). Nothing is guessed: there are
            // eleven wrong answers.
            let vorgang = zp_vorgang_for_pid(pid.as_u32()).ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS-ZP lifecycle adapter: PID {pid} is not a lifecycle Anfrage"
                ))
            })?;
            let serie = zp_serie_from_utilmd(msg, pid.as_u32(), aktiviert)?;

            Ok(ZpLifecycleCommand::ReceiveAnfrage {
                pid,
                serie,
                vorgang,
                mabis_zp_id: extract_malo_from_msg(msg).as_str().to_owned(),
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                billing_period,
                document_date,
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for the **Antwort** leg of the MaBiS-ZP
/// lifecycle.
///
/// Prozessschritt 2 of every Use-Case in the family: the answering party — the
/// BIKO, or the benachbarter NB on the Netzzeitreihe axis — returns
/// „angenommen oder abgelehnt" to an Anfrage this participant sent
/// (BK6-24-174 Anlage 3 Kap. 5.2.2 / 10.4.2 / 17.3.3.1.2 Nr. 2). Separate from
/// [`mabis_zp_lifecycle_registry`] because it builds a different command from
/// different segments, the way the ORDRSP Ablehnung is separate from the ORDERS
/// Anforderung.
///
/// # What it reads
///
/// `SG4 STS+E01` carries both halves of the answer: DE 9013 the Antwortcode and
/// DE 1131 the **Codeliste** it is drawn from. Neither alone decides anything —
/// the same code means opposite things in two trees — so the Cluster is
/// resolved from the pair through `mako_mabis::zp_antwort_ist_zustimmung`, and
/// an unresolvable pair is refused rather than defaulted.
///
/// Defaulting is not the safe direction here: reading an uncatalogued code as
/// an Ablehnung closes a Bilanzkreisabrechnung's MaBiS-Zählpunkt as refused
/// when the BIKO in fact activated it, and the Summenzeitreihe that never
/// arrives is the only symptom. A refusal dead-letters, which is visible.
///
/// The Begründung an Ablehnung must carry travels in `FTX+ACB`.
#[must_use]
pub fn mabis_zp_lifecycle_antwort_registry() -> AdapterRegistry<MabisZpLifecycleWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for the MaBiS-ZP Antwort adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS-ZP Antwort adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS-ZP Antwort adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let tx = u.transactions();
            let vorgang = tx.first();
            let antwort = vorgang.and_then(|t| t.antwort()).ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS-ZP Antwort adapter: PID {pid} carries no SG4 STS+E01 — the \
                     Antwortcode and its Codeliste are what say whether the MaBiS-ZP \
                     was activated"
                ))
            })?;

            let ebd = antwort.codeliste.clone().ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS-ZP Antwort adapter: PID {pid} states no SG4 STS+E01 DE 1131 — \
                     the Antwortcode '{}' names no Entscheidungsbaum, and the MaBiS-ZP \
                     answers are read out of sixteen different ones",
                    antwort.code
                ))
            })?;

            let bestaetigt = mako_mabis::zp_antwort_ist_zustimmung(&ebd, &antwort.code)
                .ok_or_else(|| {
                    EngineError::Deserialization(format!(
                        "MaBiS-ZP Antwort adapter: '{}' is not a code {ebd} publishes in \
                         `mako_pruefung::mabis::codes`, so whether PID {pid} activated or \
                         refused the MaBiS-ZP cannot be read — and a guess settles a \
                         Bilanzkreisabrechnung either way",
                        antwort.code
                    ))
                })?;

            // `FTX+ACB` — the Begründung the Festlegung requires on an
            // Ablehnung („erfolgt diese mit einer Begründung").
            let grund = vorgang.and_then(|t| {
                t.ftx
                    .iter()
                    .find(|f| f.qualifier == "ACB")
                    .and_then(|f| f.text.clone())
            });

            Ok(ZpLifecycleCommand::ReceiveAntwort {
                pid,
                bestaetigt,
                grund,
                ebd,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── MaBiS Anforderungen (ORDERS 17201–17208) ─────────────────────────────────

/// Build an [`AdapterRegistry`] for [`MabisAnforderungWorkflow`].
///
/// Converts an inbound ORDERS carrying a MaBiS Anforderung PID into
/// [`AnforderungCommand::ReceiveAnforderung`].
///
/// **`vorgang` cannot be read from the PID.** Five of the eight codes carry both
/// the start and the end of an Abonnement, so the direction lives in the message
/// body. Until the ORDERS AHB entries for this band are curated there is no
/// qualifier to key on, and defaulting silently to `Bestellung` would turn every
/// unsubscribe into a subscribe. The adapter therefore reads the BGM message
/// function: `1` (Cancellation) marks an Abbestellung, anything else a
/// Bestellung — and the workflow still rejects an Abbestellung on a one-shot
/// code, so a wrong read fails loudly rather than flipping the meaning.
#[must_use]
pub fn mabis_anforderung_registry() -> AdapterRegistry<MabisAnforderungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Anforderung adapter".into(),
                )
            })?;

            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Anforderung adapter: expected ORDERS message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Anforderung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // BGM DE1225 message function `1` = Cancellation.
            let vorgang = if o.bgm().and_then(|b| b.function.as_deref()) == Some("1") {
                AbonnementVorgang::Abbestellung
            } else {
                AbonnementVorgang::Bestellung
            };

            Ok(AnforderungCommand::ReceiveAnforderung {
                pid,
                vorgang,
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
        },
    ));
    registry
}

// ── MaBiS Listenabgleich (UTILMD list + correction leg) ───────────────────────

/// Build an [`AdapterRegistry`] for [`MabisListenabgleichWorkflow`].
///
/// Converts an inbound UTILMD carrying one of the list PIDs (55195, 55201,
/// 55223) into [`ListenabgleichCommand::ReceiveListe`]. The list→reply mapping
/// lives in `mako_mabis::listenabgleich`; this adapter only extracts fields.
#[must_use]
pub fn mabis_listenabgleich_registry() -> AdapterRegistry<MabisListenabgleichWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Listenabgleich adapter".into(),
                )
            })?;

            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Listenabgleich adapter: expected UTILMD message".into(),
                ));
            };

            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Listenabgleich adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            let document_date = u
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();
            // `DTM+157` Gültigkeit, Beginndatum in `610` `CCYYMM` — the
            // Bilanzierungsmonat the list covers (UTILMD AHB Strom 2.2
            // Kap. 13.4). The Dokumentendatum is when the list was *made*: a
            // December list distributed in January reconciles December, and
            // reading `DTM+137` for it settles the wrong month.
            let billing_period = u
                .dtm()
                .iter()
                .find(|d| d.qualifier == "157")
                .and_then(|d| d.value_str())
                .map_or_else(|| billing_period_head(&document_date), billing_period_head);

            // The `IDE+Z01` head carries what the whole list is about: its
            // Listennummer, the MaBiS-Zählpunkt in `SG5 LOC+Z15`, and the
            // Version der Zeitreihe in `SG8 RFF+AUU`. The answer has to name
            // all three back.
            let transaktionen = u.transactions();
            let kopf = transaktionen
                .iter()
                .find(|t| t.ide.qualifier == edi_energy::utilmd_codes::IDE_LISTE);
            let listennummer = kopf
                .and_then(|t| t.vorgangsnummer())
                .unwrap_or_default()
                .to_owned();
            let mabis_zaehlpunkt = kopf
                .and_then(|t| t.location(edi_energy::Lokationstyp::MabisZaehlpunkt))
                .unwrap_or_default()
                .to_owned();
            let zeitreihen_version = kopf
                .and_then(|t| {
                    t.sequences.iter().find_map(|seq| {
                        seq.references
                            .iter()
                            .find(|r| r.qualifier == edi_energy::utilmd_codes::RFF_ZEITREIHE)
                            .and_then(|r| r.reference.clone())
                    })
                })
                .unwrap_or_default();

            Ok(ListenabgleichCommand::ReceiveListe {
                pid,
                mabis_zaehlpunkt,
                zeitreihen_version,
                listennummer,
                sender: MarktpartnerCode::new(
                    u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    u.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                billing_period,
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// The Listennummer a Korrekturliste / Prüfmitteilung answers: its head's
/// `SG6 RFF+TN` („Es ist die Listennummer aus der Lieferanten- bzw.
/// Bilanzierungsgebietsclearingliste zu verwenden", UTILMD AHB Strom 2.2
/// Kap. 13.2, Bedingung `[631]`, S. 1112). The distributor's process runs
/// under it ([`mako_mabis::listenabgleich::liste_key`]).
#[must_use]
pub fn mabis_korrekturliste_referenz(msg: &AnyMessage) -> Option<String> {
    let AnyMessage::Utilmd(u) = msg else {
        return None;
    };
    u.transactions()
        .iter()
        .find(|t| t.ide.qualifier == edi_energy::utilmd_codes::IDE_LISTE)?
        .references
        .iter()
        .find(|r| r.qualifier == edi_energy::utilmd_codes::rff::REFERENZ_VORGANGSNUMMER)
        .and_then(|r| r.reference.clone())
        .filter(|r| !r.is_empty())
}

/// Build an [`AdapterRegistry`] carrying a received Korrekturliste (55066 /
/// 55202) into [`ListenabgleichCommand::ReceiveKorrektur`].
///
/// The head's `STS+E01` and its `IDE+24` members are mutually exclusive
/// (Bedingungen `[238]` / `[630]`, UTILMD AHB Strom 2.2 Kap. 13.2, S. 1110 /
/// 1114): a head code is a whole-list refusal, members are a Korrekturliste —
/// none at all being the empty one. Which tree the codes must come from is the
/// workflow's decision, from the list it sent.
#[must_use]
pub fn mabis_listenabgleich_korrektur_registry() -> AdapterRegistry<MabisListenabgleichWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Korrekturliste adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Korrekturliste adapter: expected UTILMD message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Korrekturliste adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let transaktionen = u.transactions();
            let kopf = transaktionen
                .iter()
                .find(|t| t.ide.qualifier == edi_energy::utilmd_codes::IDE_LISTE);
            let mitglieder: Vec<_> = transaktionen
                .iter()
                .filter(|t| t.ide.qualifier != edi_energy::utilmd_codes::IDE_LISTE)
                .collect();

            let mut codeliste = kopf
                .and_then(|t| t.antwort())
                .and_then(|a| a.codeliste.clone());
            let antwort = match kopf.and_then(|t| t.antwort()) {
                Some(kopfstatus) if mitglieder.is_empty() => {
                    mako_mabis::listenabgleich::EingehendeAntwort::GesamtAblehnung {
                        antwortcode: kopfstatus.code.clone(),
                    }
                }
                _ => {
                    let mut positionen = Vec::with_capacity(mitglieder.len());
                    for (i, t) in mitglieder.iter().enumerate() {
                        let malo = t.marktlokation().unwrap_or_default().to_owned();
                        let status = t.antwort().ok_or_else(|| {
                            EngineError::Deserialization(format!(
                                "MaBiS Korrekturliste adapter: IDE+24 #{} has no STS+E01",
                                i + 1
                            ))
                        })?;
                        if codeliste.is_none() {
                            codeliste.clone_from(&status.codeliste);
                        }
                        positionen.push((malo, status.code.clone()));
                    }
                    mako_mabis::listenabgleich::EingehendeAntwort::Korrekturliste { positionen }
                }
            };

            Ok(ListenabgleichCommand::ReceiveKorrektur {
                pid,
                message_ref: MessageRef::new(msg.message_ref()),
                codeliste,
                antwort,
            })
        },
    ));
    registry
}

// ── MaBiS Summenzeitreihen (MSCONS 13003 / 13020 / 13023) ────────────────────

/// Build an [`AdapterRegistry`] carrying an inbound Summenzeitreihe version
/// into [`MabisBillingWorkflow`].
///
/// Three fields cannot come from the message alone and are resolved here:
///
/// - **Which Summenzeitreihe** — the 13003 / 13020 / 13023 columns carry no
///   `CCI` (MSCONS AHB 3.2 Kap. 8.3.3, S. 94–97). 13023 names the LF-AASZR by
///   its PID ([`mako_mabis::zeitreihe_aus_pid`]); every other version names its
///   series through the MaBiS-ZP in `SG6 LOC+172`, which was activated before
///   any version is sent (BK6-24-174 Anlage 3 Kap. 7.4.1, S. 83). The caller
///   reads that activation and passes it as `aktiviert`; without one the
///   version is refused by name rather than guessed across Tabelle 1.
/// - **The version** — the Erstellungszeitpunkt in `SG6 DTM+293`, which the
///   BIKO echoes back in IFTSTA `RFF+AUU`. It is the key both ends match on.
/// - **Whether the arrival is inside the Erstaufschlag window** — a calendar
///   fact about the Bilanzierungsmonat (Kap. 3.10 Tabelle 2), not something the
///   message states, and the thing that decides whether the BIKO will assign
///   „Abrechnungsdaten" or „Prüfdaten". It is judged on `received_at`, the
///   instant the door confirmed, as a Europe/Berlin date: the Fristen of
///   Tabelle 2 „beziehen sich auf den Zeitpunkt des Eingangs einer Meldung …
///   beim BIKO", so a version received on the last Erstaufschlag Werktag and
///   dispatched after midnight keeps its Erstaufschlag.
#[must_use]
pub fn mabis_summenzeitreihe_registry(
    received_at: time::OffsetDateTime,
    aktiviert: Option<mako_mabis::Zeitreihe>,
) -> AdapterRegistry<MabisBillingWorkflow> {
    let eingang = mako_fristen::berlin_date(received_at);
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Summenzeitreihe adapter".into(),
                )
            })?;
            let AnyMessage::Mscons(m) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Summenzeitreihe adapter: expected MSCONS message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Summenzeitreihe adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let mabis_zp_raw = extract_malo_from_msg(msg);
            let zeitreihe = mako_mabis::zeitreihe_aus_pid(pid.as_u32())
                .or(aktiviert)
                .ok_or_else(|| {
                    EngineError::Deserialization(format!(
                        "MaBiS Summenzeitreihe adapter: MaBiS-ZP '{}' has no recorded \
                         Aktivierung (55062) naming its Summenzeitreihe, and MSCONS {pid} \
                         does not name one — a version is sent only for an activated \
                         MaBiS-ZP (BK6-24-174 Anlage 3 Kap. 7.4.1)",
                        mabis_zp_raw.as_str()
                    ))
                })?;

            let mabis_zp =
                mako_mabis::MabisZaehlpunktId::new(mabis_zp_raw.as_str()).map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Summenzeitreihe adapter: SG6 LOC+172 Meldepunkt: {e}"
                    ))
                })?;

            // SG6 DTM+293 — the Erstellungszeitpunkt that *is* the version.
            let version = m
                .segments()
                .iter()
                .filter(|s| s.tag == "DTM")
                .find(|s| s.component_str(0, 0) == Some("293"))
                .and_then(|s| s.component_str(0, 1))
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "MaBiS Summenzeitreihe adapter: SG6 DTM+293 (Versionsangabe) is \
                         missing — the version cannot be matched against the BIKO's copy"
                            .into(),
                    )
                })
                .and_then(|v| {
                    mako_mabis::SzrVersion::new(v).map_err(|e| {
                        EngineError::Deserialization(format!(
                            "MaBiS Summenzeitreihe adapter: DTM+293 '{v}': {e}"
                        ))
                    })
                })?;

            // SG6 DTM+492 in `610` `CCYYMM` — the Bilanzierungsmonat the
            // version covers, beside the Versionsangabe in the same Versions-
            // Tupel (MSCONS AHB 3.2 Kap. 3, S. 14; Segmentliste S. 97). The
            // Nachrichtendatum `DTM+137` is when the version was *made*: a
            // September version sent in October belongs to September's windows.
            let monat_raw = m
                .segments()
                .iter()
                .filter(|s| s.tag == "DTM")
                .find(|s| s.component_str(0, 0) == Some("492"))
                .and_then(|s| s.component_str(0, 1))
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "MaBiS Summenzeitreihe adapter: SG6 DTM+492 (Bilanzierungsmonat) is \
                         missing — the version cannot be placed in a Clearingfenster"
                            .into(),
                    )
                })?;
            let bilanzierungsmonat = billing_period_head(monat_raw);

            // Which phase the arrival falls in — the calendar decides, not the
            // message (Kap. 3.10 Tabelle 2), on the day of receipt.
            let monat = Some(bilanzierungsmonat_aus(monat_raw).ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS Summenzeitreihe adapter: DTM+492 '{monat_raw}' is no `CCYYMM` \
                     Bilanzierungsmonat"
                ))
            })?);
            let im_erstaufschlag =
                monat.is_some_and(|monat| monat.phase(zeitreihe, eingang).ist_erstaufschlag());
            let clearing_ende = monat.and_then(|monat| {
                monat.clearing_ende(zeitreihe, mako_mabis::fristen::Abrechnungslauf::Bka)
            });
            let kbka_clearing_ende = monat.and_then(|monat| {
                monat.clearing_ende(zeitreihe, mako_mabis::fristen::Abrechnungslauf::Kbka)
            });

            Ok(BillingCommand::ReceiveSummenzeitreihe {
                pid,
                zeitreihe,
                mabis_zp,
                bilanzierungsmonat,
                version,
                im_erstaufschlag,
                clearing_ende,
                kbka_clearing_ende,
                absender: MarktpartnerCode::new(
                    m.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                biko_id: mako_engine::types::BikoId::new(
                    m.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

/// The Fristenkalender of the Bilanzierungsmonat a `CCYYMM` value names.
fn bilanzierungsmonat_aus(document_date: &str) -> Option<mako_mabis::Bilanzierungsmonat> {
    if document_date.len() < 6 {
        return None;
    }
    let year: i32 = document_date[..4].parse().ok()?;
    let month: u8 = document_date[4..6].parse().ok()?;
    let month = time::Month::try_from(month).ok()?;
    let letzter = time::util::days_in_month(month, year);
    time::Date::from_calendar_date(year, month, letzter)
        .ok()
        .map(mako_mabis::Bilanzierungsmonat::new)
}

// ── MaBiS Anforderung Ablehnung (ORDRSP 19204) ───────────────────────────────

/// Build an [`AdapterRegistry`] for the one Ablehnung a MaBiS Anforderung has.
///
/// ORDRSP 19204 „Ablehnung Ab-/Bestellung der Aggregationsebene" (ÜNB → BKV)
/// answers **17207 only**. The ÜNB reads it out of `E_0003` for a Bestellung and
/// `E_0022` for an Abbestellung, so the EBD travels with the code and the
/// workflow checks the pair — a code read against the wrong tree means something
/// else there.
#[must_use]
pub fn mabis_anforderung_ablehnung_registry() -> AdapterRegistry<MabisAnforderungWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Ablehnung adapter".into(),
                )
            })?;
            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Ablehnung adapter: expected ORDRSP message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Ablehnung adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            // `SG2 AJT+<code>+<EBD>` — the Antwortcode (DE 4465) and the tree it
            // belongs to (DE 1082), Muss on the 19204 (ORDRSP AHB, Nr 00015).
            // The ORDRSP MIG defines no `STS` segment at all.
            let ajt = o.ajt().ok_or_else(|| {
                EngineError::Deserialization(
                    "MaBiS Ablehnung adapter: no SG2 AJT — the Ablehnung carries no \
                     Antwortcode"
                        .into(),
                )
            })?;
            let code = ajt.antwortcode.clone();
            let ebd = ajt.ebd.clone().unwrap_or_default();

            Ok(AnforderungCommand::ReceiveAblehnung {
                pid,
                ebd,
                code,
                message_ref: MessageRef::new(msg.message_ref()),
            })
        },
    ));
    registry
}

// ── MaBiS normierte Profile (MSCONS 13010–13012, ORDERS 17211) ───────────────

/// Build an [`AdapterRegistry`] for [`MabisProfilWorkflow`].
///
/// Converts an inbound MSCONS profile delivery into
/// [`ProfilCommand::ReceiveProfile`]. The NB's copy of a Reklamation (ORDERS
/// 17211) goes through [`mabis_profil_reklamation_registry`].
/// Build an [`AdapterRegistry`] for the NB's side of the Profil-Reklamation:
/// the LF's ORDERS 17211 (ORDERS AHB 1.1b Kap. 5.5, S. 126–127) becomes
/// `ReceiveReklamation` — `FTX+AFH` DE 4441 the `E_0100` code and `SG1
/// RFF+ACW` the reclaimed MSCONS' `BGM` DE 1004.
#[must_use]
pub fn mabis_profil_reklamation_registry() -> AdapterRegistry<MabisProfilWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for MaBiS Profil-Reklamation adapter".into(),
                )
            })?;
            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Profil-Reklamation adapter: expected ORDERS 17211".into(),
                ));
            };
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            let kopf = super::msb_eingang::orders_kopf(o);
            let segs = o.segments();
            let antwortcode = segs
                .iter()
                .filter(|s| s.tag == "FTX")
                .find(|s| s.component_str(0, 0) == Some("AFH"))
                .and_then(|s| s.component_str(2, 0))
                .unwrap_or("")
                .to_owned();
            let reklamierte_nachricht = segs
                .iter()
                .filter(|s| s.tag == "RFF")
                .find(|s| s.component_str(0, 0) == Some("ACW"))
                .and_then(|s| s.component_str(0, 1))
                .unwrap_or("")
                .to_owned();
            Ok(ProfilCommand::ReceiveReklamation {
                reklamation: mako_mabis::ReklamationEingang {
                    lf: MarktpartnerCode::new(kopf.sender),
                    nb: MarktpartnerCode::new(kopf.empfaenger),
                    dokumentennummer: kopf.dokumentennummer,
                    reklamierte_nachricht,
                    antwortcode,
                    message_ref: MessageRef::new(msg.message_ref()),
                },
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

#[must_use]
pub fn mabis_profil_registry() -> AdapterRegistry<MabisProfilWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for MaBiS Profil adapter".into())
            })?;
            let AnyMessage::Mscons(m) = msg else {
                return Err(EngineError::Deserialization(
                    "MaBiS Profil adapter: expected MSCONS message".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "MaBiS Profil adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;

            let (validation_passed, validation_errors) = super::ahb_verdict(msg);

            // The Bilanzierungsmonat and the profile version both come from the
            // document date; the AHB carries the version as an ascending
            // Erstellungszeitpunkt, of which only the ordinal matters here.
            let document_date = m
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();
            let bilanzierungsmonat = billing_period_head(&document_date);

            Ok(ProfilCommand::ReceiveProfile {
                pid,
                sender: MarktpartnerCode::new(
                    m.sender().and_then(|n| n.party_id.as_deref()).unwrap_or(""),
                ),
                receiver: MarktpartnerCode::new(
                    m.receiver()
                        .and_then(|n| n.party_id.as_deref())
                        .unwrap_or(""),
                ),
                bilanzierungsmonat,
                version: 0,
                message_ref: MessageRef::new(msg.message_ref()),
                // `BGM` DE 1004 — what a Reklamation 17211 names in `SG1
                // RFF+ACW` (ORDERS AHB 1.1b Kap. 5.5, `[525]`).
                dokumentennummer: m
                    .bgm()
                    .and_then(|b| b.document_id.clone())
                    .unwrap_or_default(),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

// ── MaBiS-ZP series resolution ───────────────────────────────────────────────

/// Whether a lifecycle Anfrage PID activates or deactivates.
///
/// Unlike the series, this **is** derivable from the PID: every Anfrage code in
/// [`ZP_FAMILIEN`] belongs to exactly one direction.
fn zp_vorgang_for_pid(pid: u32) -> Option<ZpVorgang> {
    ZP_FAMILIEN
        .iter()
        .find(|f| f.anfrage == pid)
        .map(|f| f.vorgang)
}

/// Resolve which Summenzeitreihe a MaBiS-ZP lifecycle UTILMD is about.
///
/// For the codes that carry their own series (55071/55072 Zuordnungsermächtigung,
/// 55197–55200 · 55203–55214 AAÜZ/LF-AASZR) the PID is unambiguous and the `SG10` pair is not
/// needed. For the **generic** 55062/55063 it is the only discriminator there
/// is, and a missing or unknown one is refused rather than defaulted: eleven
/// series share the code and five of them owe no answer, so a wrong guess either
/// invents an obligation or drops one.
fn zp_serie_from_utilmd(
    msg: &AnyMessage,
    pid: u32,
    aktiviert: Option<ZpSerie>,
) -> Result<ZpSerie, EngineError> {
    // A PID used by exactly one series needs no lookup.
    let kandidaten: Vec<ZpSerie> = ZP_FAMILIEN
        .iter()
        .filter(|f| f.anfrage == pid)
        .map(|f| f.serie)
        .collect();
    if let [einzige] = kandidaten[..] {
        return Ok(einzige);
    }

    let AnyMessage::Utilmd(u) = msg else {
        return Err(EngineError::Deserialization(
            "MaBiS-ZP lifecycle adapter: expected UTILMD message".into(),
        ));
    };

    let Some(cav) = utilmd_cav(u, CCI_BEZEICHNUNG_SUMMENZEITREIHE) else {
        // The Deaktivierung names no series; its activation does.
        return aktiviert
            .filter(|serie| kandidaten.contains(serie))
            .ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS-ZP lifecycle adapter: PID {pid} is shared by {} Summenzeitreihen; \
                     it carries no SG10 CCI+++{CCI_BEZEICHNUNG_SUMMENZEITREIHE} and the \
                     MaBiS-ZP has no recorded Aktivierung naming its series",
                    kandidaten.len()
                ))
            });
    };
    let verantwortlicher =
        utilmd_merkmal_of_class(u, CCI_KLASSENTYP_VERANTWORTLICHER).ok_or_else(|| {
            EngineError::Deserialization(
                "MaBiS-ZP lifecycle adapter: SG10 CCI+6 (Verantwortlicher) is missing".into(),
            )
        })?;

    ZpSerie::from_wire(&cav, &verantwortlicher).ok_or_else(|| {
        EngineError::Deserialization(format!(
            "MaBiS-ZP lifecycle adapter: CAV '{cav}' / Verantwortlicher '{verantwortlicher}' \
             names no MaBiS Summenzeitreihe"
        ))
    })
}

/// The `CAV` DE 7111 code under the `SG10` characteristic whose DE 7037 Merkmal
/// is `merkmal` and whose DE 7059 Klassentyp is unused.
///
/// Both halves of the key matter: `SG10` repeats, one group per Merkmal, so
/// reading the first `CAV` in the message would take a value out of whichever
/// characteristic happened to come first.
fn utilmd_cav(u: &edi_energy::messages::utilmd::UtilmdMessage, merkmal: &str) -> Option<String> {
    super::cav_codes_under_merkmal(u.segments(), merkmal)
        .next()
        .map(str::to_owned)
}

/// The DE 7037 Merkmal of the `SG10` characteristic whose DE 7059 Klassentyp is
/// `klassentyp`.
fn utilmd_merkmal_of_class(
    u: &edi_energy::messages::utilmd::UtilmdMessage,
    klassentyp: &str,
) -> Option<String> {
    super::cci_merkmal_of_class(u.segments(), klassentyp).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 13003 version of the Bilanzkreissummenzeitreihe for September 2026,
    /// made on `nachrichtendatum` (`CCYYMMDD`).
    fn summenzeitreihe_vom(nachrichtendatum: &str, monat: Option<&str>) -> AnyMessage {
        let monat = monat.map_or_else(String::new, |m| format!("DTM+492:{m}:610'"));
        let raw = format!(
            "UNA:+.? 'UNB+UNOC:3+9900357000004:500+9900357000011:500+261016:2130+1'\
             UNH+1+MSCONS:D:04B:UN:2.4c'BGM+BK+DOK13003+9'\
             DTM+137:{nachrichtendatum}1200?+00:303'RFF+Z13:13003'\
             NAD+MS+9900357000004::293'NAD+MR+9900357000011::293'UNS+D'NAD+DP'\
             LOC+172+DE0004030099000000000000000012345'{monat}\
             DTM+293:20261016213000?+00:304'\
             LIN+1'QTY+220:100'UNT+17+1'UNZ+1+1'"
        );
        edi_energy::parse(raw.as_bytes()).expect("the Summenzeitreihe parses")
    }

    /// The series the MaBiS-ZP was activated for — the BK-SZR (Kategorie A).
    fn bk_szr_a() -> mako_mabis::Zeitreihe {
        mako_mabis::ZpSerie::BilanzkreissummenzeitreiheNb
            .zeitreihe()
            .expect("a Tabelle-1 row")
    }

    fn summenzeitreihe() -> AnyMessage {
        summenzeitreihe_vom("20260930", Some("202609"))
    }

    fn dispatch(msg: &AnyMessage, received_at: time::OffsetDateTime) -> BillingCommand {
        let fv = FormatVersion::parse("FV2026-04-01").expect("known");
        let raw: &dyn Any = msg;
        mabis_summenzeitreihe_registry(received_at, Some(bk_szr_a()))
            .dispatch(raw, &fv)
            .expect("dispatched")
    }

    fn im_erstaufschlag(received_at: time::OffsetDateTime) -> bool {
        match dispatch(&summenzeitreihe(), received_at) {
            BillingCommand::ReceiveSummenzeitreihe {
                im_erstaufschlag, ..
            } => im_erstaufschlag,
            _ => panic!("the adapter builds ReceiveSummenzeitreihe"),
        }
    }

    /// A version is placed in the Bilanzierungsmonat of its `SG6 DTM+492`,
    /// not of its Nachrichtendatum: a September version made on 02.10. is
    /// judged against September's windows.
    #[test]
    fn a_september_version_sent_in_october_is_judged_against_september() {
        let (zeitreihe, _) = mako_mabis::zeitreihe_aus_cav("Z97").expect("Z97");
        let september = mako_mabis::Bilanzierungsmonat::new(time::macros::date!(2026 - 09 - 30));
        let erwartet = september
            .clearing_ende(zeitreihe, mako_mabis::fristen::Abrechnungslauf::Bka)
            .expect("the BK-SZR has a BKA close");
        let erst = september
            .erstaufschlag(zeitreihe, mako_mabis::fristen::Abrechnungslauf::Bka)
            .expect("the BK-SZR has an Erstaufschlag")
            .von;
        let received = mako_fristen::berlin_at(erst, time::macros::time!(10:00));
        match dispatch(&summenzeitreihe_vom("20261002", Some("202609")), received) {
            BillingCommand::ReceiveSummenzeitreihe {
                bilanzierungsmonat,
                clearing_ende,
                im_erstaufschlag,
                ..
            } => {
                assert_eq!(bilanzierungsmonat.as_str(), "202609");
                assert_eq!(clearing_ende, Some(erwartet));
                assert!(
                    im_erstaufschlag,
                    "received inside September's Erstaufschlag"
                );
            }
            _ => panic!("the adapter builds ReceiveSummenzeitreihe"),
        }
    }

    /// A version without its Bilanzierungsmonat is refused rather than placed
    /// by its Nachrichtendatum.
    #[test]
    fn a_version_without_its_bilanzierungsmonat_is_refused() {
        let fv = FormatVersion::parse("FV2026-04-01").expect("known");
        let msg = summenzeitreihe_vom("20261002", None);
        let raw: &dyn Any = &msg;
        let Err(refused) =
            mabis_summenzeitreihe_registry(time::OffsetDateTime::UNIX_EPOCH, Some(bk_szr_a()))
                .dispatch(raw, &fv)
        else {
            panic!("a version without DTM+492 is refused");
        };
        assert!(refused.to_string().contains("DTM+492"), "{refused}");
    }

    /// The Erstaufschlag is judged on the day the version arrived, in
    /// gesetzlicher deutscher Zeit — „Zeitpunkt des Eingangs einer Meldung …
    /// beim BIKO" (MaBiS Kap. 3.10, S. 21) — never on the day it is dispatched,
    /// and never on its UTC date.
    #[test]
    fn the_erstaufschlag_is_judged_on_the_receipt_day() {
        let (zeitreihe, _) = mako_mabis::zeitreihe_aus_cav("Z97").expect("Z97");
        let letzter = mako_mabis::Bilanzierungsmonat::new(time::macros::date!(2026 - 09 - 30))
            .erstaufschlag(zeitreihe, mako_mabis::fristen::Abrechnungslauf::Bka)
            .expect("the BK-SZR has an Erstaufschlag")
            .bis;
        let berlin = |date: time::Date, h: u8, m: u8| {
            mako_fristen::berlin_at(date, time::Time::from_hms(h, m, 0).expect("valid"))
        };
        assert!(
            im_erstaufschlag(berlin(letzter, 23, 30)),
            "received 23:30 MESZ on the last Erstaufschlag day {letzter}"
        );
        let danach = letzter.next_day().expect("a next day");
        let spaeter = berlin(danach, 0, 30);
        assert_eq!(spaeter.to_offset(time::UtcOffset::UTC).date(), letzter);
        assert!(
            !im_erstaufschlag(spaeter),
            "00:30 MESZ the next day is past it, although its UTC date is not"
        );
    }
}
