//! Adapters for ORDERS whose recipient owes an answer: the GNB's Anfrage
//! Sperrung Gas (17116, `geli-gas-sperrung-msb`) and the GPKE Teil 3
//! Konfiguration ORDERS, the Einrichtung 17134 / 17135 included
//! (`gpke-konfiguration-aenderung`, receiving side).

use super::*;

/// The `NAD+MS` and `NAD+MR` MP-IDs, the first `SG2 LOC` DE 3225 and the
/// `BGM` DE 1004 of an inbound ORDERS.
pub(super) struct OrdersKopf {
    /// `NAD+MS`.
    pub sender: String,
    /// `NAD+MR`.
    pub empfaenger: String,
    /// The first `LOC` DE 3225.
    pub lokation: String,
    /// `BGM` DE 1004.
    pub dokumentennummer: String,
    /// `BGM` DE 1001.
    pub bgm: String,
    /// `IMD` DE 7081 (C272, element 2), when present.
    pub leistung: Option<String>,
}

pub(super) fn orders_kopf(o: &edi_energy::messages::orders::OrdersMessage) -> OrdersKopf {
    let party = |n: Option<&edi_energy::messages::segments::Nad>| {
        n.and_then(|n| n.party_id.as_deref())
            .unwrap_or("")
            .to_owned()
    };
    let lokation = o
        .segments()
        .iter()
        .find(|s| s.tag == "LOC")
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned();
    let bgm_segment = o.segments().iter().find(|s| s.tag == "BGM");
    let dokumentennummer = bgm_segment
        .and_then(|s| s.component_str(1, 0))
        .unwrap_or("")
        .to_owned();
    let bgm = bgm_segment
        .and_then(|s| s.component_str(0, 0))
        .unwrap_or("")
        .to_owned();
    let leistung = o
        .segments()
        .iter()
        .find(|s| s.tag == "IMD")
        .and_then(|s| s.component_str(1, 0))
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
    OrdersKopf {
        bgm,
        leistung,
        sender: party(o.sender()),
        empfaenger: party(o.receiver()),
        lokation,
        dokumentennummer,
    }
}

/// Build an [`AdapterRegistry`] for
/// [`mako_geli_gas::sperrung_msb::GeliGasSperrungMsbWorkflow`]: the gMSB's copy
/// of the GNB's Anfrage Sperrung (ORDERS 17116) becomes `ReceiveAnfrage`.
#[must_use]
pub fn geli_gas_sperrung_msb_registry()
-> AdapterRegistry<mako_geli_gas::sperrung_msb::GeliGasSperrungMsbWorkflow> {
    use mako_geli_gas::sperrung_msb::{AnfrageSperrungGas, SperrungMsbGasCommand};
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GeLi Gas Sperrung gMSB adapter".into(),
                )
            })?;
            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GeLi Gas Sperrung gMSB adapter: expected ORDERS 17116".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GeLi Gas Sperrung gMSB adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            let kopf = orders_kopf(o);
            Ok(SperrungMsbGasCommand::ReceiveAnfrage {
                pid,
                anfrage: AnfrageSperrungGas {
                    netzbetreiber: mako_engine::types::MarktpartnerCode::new(kopf.sender),
                    msb: mako_engine::types::MarktpartnerCode::new(kopf.empfaenger),
                    malo: mako_engine::types::MaLo::new(kopf.lokation),
                    dokumentennummer: kopf.dokumentennummer,
                    message_ref: MessageRef::new(msg.message_ref()),
                },
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for the receiving side of
/// [`mako_gpke::GpkeKonfigurationAenderungWorkflow`]: an inbound GPKE Teil 3
/// ORDERS (17118, 17120–17123, 17128–17131, 17133–17135) becomes
/// `ReceiveAuftrag`.
#[must_use]
pub fn gpke_konfiguration_auftrag_registry()
-> AdapterRegistry<mako_gpke::GpkeKonfigurationAenderungWorkflow> {
    use mako_gpke::KonfigurationAenderungCommand;
    use mako_gpke::konfiguration_aenderung::EingangAuftrag;
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for GPKE Konfiguration Auftrag adapter".into(),
                )
            })?;
            let AnyMessage::Orders(o) = msg else {
                return Err(EngineError::Deserialization(
                    "GPKE Konfiguration Auftrag adapter: expected ORDERS".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "GPKE Konfiguration Auftrag adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            let kopf = orders_kopf(o);
            Ok(KonfigurationAenderungCommand::ReceiveAuftrag {
                pid,
                auftrag: EingangAuftrag {
                    absender: mako_engine::types::MarktpartnerCode::new(kopf.sender),
                    empfaenger: mako_engine::types::MarktpartnerCode::new(kopf.empfaenger),
                    lokation: Some(kopf.lokation).filter(|l| !l.is_empty()),
                    dokumentennummer: kopf.dokumentennummer,
                    bgm: kopf.bgm,
                    message_ref: MessageRef::new(msg.message_ref()),
                },
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build an [`AdapterRegistry`] for the MSBN's side of the Geräteübernahme
/// ([`mako_wim::geraeteubernahme_msbn::WimGeraeteubernahmeMsbnWorkflow`]): an
/// inbound ORDRSP 19001 / 19002 / 19015 / 19016 becomes `ReceiveAntwort`, with
/// its `SG2 AJT` code and Codeliste and the `SG3 NAD+VY` Eigentümer.
#[must_use]
pub fn wim_geraeteubernahme_msbn_registry()
-> AdapterRegistry<mako_wim::geraeteubernahme_msbn::WimGeraeteubernahmeMsbnWorkflow> {
    use mako_wim::geraeteubernahme_msbn::{MsbaAntwort, MsbnCommand};
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for WiM Geräteübernahme MSBN adapter".into(),
                )
            })?;
            let AnyMessage::Ordrsp(o) = msg else {
                return Err(EngineError::Deserialization(
                    "WiM Geräteübernahme MSBN adapter: expected ORDRSP".into(),
                ));
            };
            let pid = msg
                .detect_pruefidentifikator()
                .map_err(|e| {
                    EngineError::Deserialization(format!(
                        "WiM Geräteübernahme MSBN adapter: PID detection failed: {e}"
                    ))
                })
                .and_then(convert_pid)?;
            let ajt = o.segments().iter().find(|s| s.tag == "AJT");
            let eigentuemer = o
                .segments()
                .iter()
                .find(|s| s.tag == "NAD" && s.component_str(0, 0) == Some("VY"))
                .and_then(|s| s.component_str(1, 0))
                .filter(|v| !v.is_empty())
                .map(mako_engine::types::MarktpartnerCode::new);
            Ok(MsbnCommand::ReceiveAntwort {
                antwort: MsbaAntwort {
                    pid,
                    antwortcode: ajt
                        .and_then(|s| s.component_str(0, 0))
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned),
                    codeliste: ajt
                        .and_then(|s| s.component_str(1, 0))
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned),
                    eigentuemer,
                    message_ref: MessageRef::new(msg.message_ref()),
                },
            })
        },
    ));
    registry
}
