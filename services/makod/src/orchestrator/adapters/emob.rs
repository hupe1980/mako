//! NZR-EMob / Modell 2 adapters — UTILMD 55238–55243.
//!
//! Split out of the flat `adapters` module. One registry per direction, shared
//! by all three legs: the request adapter builds
//! [`ModellwechselCommand::ReceiveAnfrage`], the answer adapter
//! [`ModellwechselCommand::ReceiveAntwort`]. Which leg a message belongs to is
//! the router's decision, not the adapter's — the adapter only reads the wire.
//!
//! # The date qualifier follows the leg
//!
//! An Anmeldung names a Vertrags**beginn** (`DTM+92`) and everything that ends
//! something a Vertrags**ende** (`DTM+93`). Reading the wrong one yields an
//! empty process date, which the workflow then puts on the answer, so both are
//! tried in the order the leg's AHB column fixes.

use std::any::Any;

use edi_energy::{AnyMessage, EdiEnergyMessage};
use mako_emob::modellwechsel::{EmobAntwort, ModellwechselCommand, Modellwechseldaten};
use mako_emob::{EmobAbmeldungWorkflow, EmobAnmeldungWorkflow, EmobZuordnungsendeWorkflow};
use mako_engine::{
    error::EngineError,
    message_adapter::{AdapterRegistry, FnAdapter},
    types::{MaLo, MarktpartnerCode, MessageRef},
    version::FormatVersion,
    workflow::Workflow,
};

use super::{convert_pid, is_known_fv};

/// The `SG4` facts every Modell-2 leg carries, read once for both directions.
fn daten(msg: &AnyMessage) -> Result<Modellwechseldaten, EngineError> {
    let AnyMessage::Utilmd(u) = msg else {
        return Err(EngineError::Deserialization(
            "Modell-2 adapter: expected UTILMD message".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("Modell-2 adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;

    let tx = u.transactions();
    let vorgang = tx.first();

    // `DTM+92` on the Anmeldung pair, `DTM+93` on the four that end something
    // (UTILMD AHB Strom 2.2 Kap. 11). `DTM+158`/`DTM+159` carry the same value
    // by Bedingung `[317]`, so either answers the question when the first is
    // absent — a Modellwechseltermin read as `""` would ride the answer back.
    use edi_energy::utilmd_codes::dtm;
    let qualifiers: [&str; 2] = match pid.as_u32() {
        55_238 | 55_239 => [dtm::BEGINN_ZUM, dtm::BILANZIERUNGSBEGINN],
        _ => [dtm::ENDE_ZUM, dtm::BILANZIERUNGSENDE],
    };
    let process_date = qualifiers
        .iter()
        .find_map(|q| vorgang.and_then(|t| t.date(q)))
        .unwrap_or_default()
        .to_owned();

    Ok(Modellwechseldaten {
        // `SG5 LOC+Z16` explicitly. Modell 2 is Strom-only and the 55239
        // carries a `Z15` in the same SG5, so the „first Lokation of any type"
        // reading is one wire-order change away from keying the process on a
        // Zählpunktbezeichnung.
        malo: MaLo::new(vorgang.and_then(|t| t.marktlokation()).unwrap_or("")),
        sender: MarktpartnerCode::new(u.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            u.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        process_date,
        pruefidentifikator: pid,
        // `SG4 IDE+24` DE 7402. The answer has to echo it in `SG4 RFF+TN`, and
        // it is the only thing that ties the two messages together.
        vorgangsnummer: vorgang
            .and_then(|t| t.vorgangsnummer())
            .map(ToOwned::to_owned),
    })
}

/// Build the request-side registry for one leg.
fn anfrage_registry<W>() -> AdapterRegistry<W>
where
    W: Workflow<Command = ModellwechselCommand> + 'static,
{
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization("expected AnyMessage for the Modell-2 adapter".into())
            })?;
            let (validation_passed, validation_errors) = super::ahb_verdict(msg);
            Ok(ModellwechselCommand::ReceiveAnfrage {
                data: Box::new(daten(msg)?),
                message_ref: MessageRef::new(msg.message_ref()),
                validation_passed,
                validation_errors,
            })
        },
    ));
    registry
}

/// Build the answer-side registry for one leg.
///
/// `tree` is the Entscheidungsbaum the Cluster is resolved against. It cannot
/// be derived from the code: `A01` is an **Ablehnung** in `E_0510` and a
/// **Zustimmung** in `E_0511` and `E_0512`, so a `zustimmung` read from the
/// code alone would invert two of the three legs.
fn antwort_registry<W>(tree: &'static str) -> AdapterRegistry<W>
where
    W: Workflow<Command = ModellwechselCommand> + 'static,
{
    let mut registry = AdapterRegistry::new();
    registry.register(FnAdapter::new(
        is_known_fv,
        move |raw: &dyn Any, _fv: &FormatVersion| {
            let msg = raw.downcast_ref::<AnyMessage>().ok_or_else(|| {
                EngineError::Deserialization(
                    "expected AnyMessage for the Modell-2 answer adapter".into(),
                )
            })?;
            let AnyMessage::Utilmd(u) = msg else {
                return Err(EngineError::Deserialization(
                    "Modell-2 answer adapter: expected UTILMD message".into(),
                ));
            };
            let tx = u.transactions();
            let vorgang = tx.first();
            let code = vorgang
                .and_then(|t| t.antwort())
                .map(|a| a.code.clone())
                .ok_or_else(|| {
                    EngineError::Deserialization(format!(
                        "the Modell-2 answer carries no SG4 STS+E01 — the AHB marks it Muss \
                         and {tree}'s Cluster is what decides the outcome"
                    ))
                })?;
            let zustimmung = mako_pruefung::codes::lookup(tree, &code)
                .is_some_and(|c| c.cluster == mako_pruefung::codes::Cluster::Zustimmung);

            let mut antwort = EmobAntwort {
                antwort_code: code,
                codeliste: tree.to_owned(),
                zustimmung,
                bemerkung: vorgang.and_then(|t| {
                    t.ftx
                        .iter()
                        .find(|f| f.qualifier == "ACB")
                        .and_then(|f| f.text.clone())
                }),
                zp_ngz: None,
            };
            // `SG5 LOC+Z15` — the ZPB des ZP der NGZ the 55239 Bestätigung
            // names beside the MaLo (AHB Bedingung `[663]`). Without it the LPB
            // cannot subscribe to the Netzgangzeitreihe it just won.
            if let Some(zp) =
                vorgang.and_then(|t| t.location(edi_energy::Lokationstyp::MabisZaehlpunkt))
            {
                antwort.zp_ngz = Some(zp.to_owned());
            }

            Ok(ModellwechselCommand::ReceiveAntwort {
                antwort: Box::new(antwort),
            })
        },
    ));
    registry
}

/// UTILMD 55238 — Anmeldung in Modell 2, inbound at the VNB.
#[must_use]
pub fn emob_anmeldung_registry() -> AdapterRegistry<EmobAnmeldungWorkflow> {
    anfrage_registry()
}

/// UTILMD 55239 — the VNB's answer, inbound at the LPB.
///
/// Resolved against `E_0510`. `E_0513` shares the PID and publishes only
/// `A99`, which is an Ablehnung in both, so the Cluster is the same either way.
#[must_use]
pub fn emob_anmeldung_antwort_registry() -> AdapterRegistry<EmobAnmeldungWorkflow> {
    antwort_registry(mako_pruefung::emob::EBD_ANMELDUNG)
}

/// UTILMD 55240 — Beendigung der Zuordnung, inbound at the LF.
#[must_use]
pub fn emob_zuordnungsende_registry() -> AdapterRegistry<EmobZuordnungsendeWorkflow> {
    anfrage_registry()
}

/// UTILMD 55241 — the LF's answer, inbound at the VNB. Resolved against `E_0511`.
#[must_use]
pub fn emob_zuordnungsende_antwort_registry() -> AdapterRegistry<EmobZuordnungsendeWorkflow> {
    antwort_registry(mako_pruefung::emob::EBD_BEENDIGUNG)
}

/// UTILMD 55242 — Abmeldung aus dem Modell 2, inbound at the VNB.
#[must_use]
pub fn emob_abmeldung_registry() -> AdapterRegistry<EmobAbmeldungWorkflow> {
    anfrage_registry()
}

/// UTILMD 55243 — the VNB's answer, inbound at the LPB. Resolved against `E_0512`.
#[must_use]
pub fn emob_abmeldung_antwort_registry() -> AdapterRegistry<EmobAbmeldungWorkflow> {
    antwort_registry(mako_pruefung::emob::EBD_ABMELDUNG)
}
