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
            // If document date is absent or shorter than 6 chars, store empty.
            let billing_period = if document_date_str.len() >= 6 {
                BillingPeriod::new(&document_date_str[..6])
            } else {
                BillingPeriod::new("")
            };

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
pub fn mabis_zp_lifecycle_registry() -> AdapterRegistry<MabisZpLifecycleWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
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

            let billing_period = if document_date.len() >= 6 {
                BillingPeriod::new(&document_date[..6])
            } else {
                BillingPeriod::new("")
            };

            // 55062/55063 are shared by eleven Summenzeitreihen, so the PID
            // alone does not say what was activated. The UTILMD carries the
            // discriminator: `SG10 CCI+++ZB4 / CAV` DE 7111 names the
            // Summenzeitreihe and `SG10 CCI+6` DE 7037 the responsible role
            // (UTILMD AHB Strom 2.2 Kap. 13.1). Both are read here rather than
            // guessed, because there are eleven wrong answers.
            let vorgang = zp_vorgang_for_pid(pid.as_u32()).ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS-ZP lifecycle adapter: PID {pid} is not a lifecycle Anfrage"
                ))
            })?;
            let serie = zp_serie_from_utilmd(msg, pid.as_u32())?;

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
            let billing_period = if document_date.len() >= 6 {
                BillingPeriod::new(&document_date[..6])
            } else {
                BillingPeriod::new("")
            };

            Ok(ListenabgleichCommand::ReceiveListe {
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
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
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
/// - **Which Summenzeitreihe** — `SG10 CCI+++ZB4` / `CAV` DE 7111, the same
///   codelist the ZP-lifecycle adapter reads. Without it 13003 is ambiguous
///   across every row of Tabelle 1.
/// - **The version** — the Erstellungszeitpunkt in `SG6 DTM+293`, which the
///   BIKO echoes back in IFTSTA `RFF+AUU`. It is the key both ends match on.
/// - **Whether the arrival is inside the Erstaufschlag window** — a calendar
///   fact about the Bilanzierungsmonat (Kap. 3.10 Tabelle 2), not something the
///   message states, and the thing that decides whether the BIKO will assign
///   „Abrechnungsdaten" or „Prüfdaten".
#[must_use]
pub fn mabis_summenzeitreihe_registry() -> AdapterRegistry<MabisBillingWorkflow> {
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
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

            let cav =
                mscons_cav(m, mako_mabis::CCI_BEZEICHNUNG_SUMMENZEITREIHE).ok_or_else(|| {
                    EngineError::Deserialization(
                        "MaBiS Summenzeitreihe adapter: SG10 CCI+++ZB4 (Bezeichnung der \
                         Summenzeitreihe) carries no CAV code, so the Summenzeitreihe is \
                         ambiguous"
                            .into(),
                    )
                })?;
            let (zeitreihe, _ebene) = mako_mabis::zeitreihe_aus_cav(&cav).ok_or_else(|| {
                EngineError::Deserialization(format!(
                    "MaBiS Summenzeitreihe adapter: CAV '{cav}' names no MaBiS \
                         Summenzeitreihe"
                ))
            })?;

            let mabis_zp = mako_mabis::MabisZaehlpunktId::new(extract_malo_from_msg(msg).as_str())
                .map_err(|e| {
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

            let document_date = m
                .dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value_str())
                .unwrap_or("")
                .to_owned();
            let bilanzierungsmonat = if document_date.len() >= 6 {
                BillingPeriod::new(&document_date[..6])
            } else {
                BillingPeriod::new("")
            };

            // Which phase the arrival falls in — the calendar decides, not the
            // message (Kap. 3.10 Tabelle 2).
            let im_erstaufschlag = bilanzierungsmonat_aus(&document_date).is_some_and(|monat| {
                monat
                    .phase(zeitreihe, mako_fristen::heute())
                    .ist_erstaufschlag()
            });

            Ok(BillingCommand::ReceiveSummenzeitreihe {
                pid,
                zeitreihe,
                mabis_zp,
                bilanzierungsmonat,
                version,
                im_erstaufschlag,
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

/// The Fristenkalender of the Bilanzierungsmonat a `CCYYMMDD` document date
/// falls in.
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

/// The `CAV` DE 7111 code under the MSCONS `SG10` characteristic whose DE 7037
/// Merkmal is `merkmal`.
fn mscons_cav(m: &edi_energy::messages::mscons::MsconsMessage, merkmal: &str) -> Option<String> {
    super::cav_codes_under_merkmal(m.segments(), merkmal)
        .next()
        .map(str::to_owned)
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

            // `STS+E01+<code>:<EBD>` — the Antwortcode and the tree it belongs to.
            let sts = o
                .segments()
                .iter()
                .find(|s| s.tag == "STS")
                .ok_or_else(|| {
                    EngineError::Deserialization(
                        "MaBiS Ablehnung adapter: no STS segment — the Ablehnung carries \
                         no Antwortcode"
                            .into(),
                    )
                })?;
            let code = sts.component_str(1, 0).unwrap_or_default().to_owned();
            let ebd = sts.component_str(1, 1).unwrap_or_default().to_owned();

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
/// [`ProfilCommand::ReceiveProfile`]. The Reklamation (ORDERS 17211) is
/// **outbound** — this participant sends it — so no inbound arm exists for it.
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
            let bilanzierungsmonat = if document_date.len() >= 6 {
                BillingPeriod::new(&document_date[..6])
            } else {
                BillingPeriod::new("")
            };

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
/// 55197–55214 AAÜZ/LF-AASZR) the PID is unambiguous and the `SG10` pair is not
/// needed. For the **generic** 55062/55063 it is the only discriminator there
/// is, and a missing or unknown one is refused rather than defaulted: eleven
/// series share the code and five of them owe no answer, so a wrong guess either
/// invents an obligation or drops one.
fn zp_serie_from_utilmd(msg: &AnyMessage, pid: u32) -> Result<ZpSerie, EngineError> {
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

    let cav = utilmd_cav(u, CCI_BEZEICHNUNG_SUMMENZEITREIHE).ok_or_else(|| {
        EngineError::Deserialization(format!(
            "MaBiS-ZP lifecycle adapter: PID {pid} is shared by {} Summenzeitreihen, but \
             SG10 CCI+++{CCI_BEZEICHNUNG_SUMMENZEITREIHE} (Bezeichnung der Summenzeitreihe) \
             carries no CAV code",
            kandidaten.len()
        ))
    })?;
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
