//! `MessageAdapter` registries for all domain workflows.
//!
//! This module is the **wiring layer** between raw parsed `AnyMessage` values
//! (produced by `edi-energy`) and typed domain commands (consumed by each
//! workflow). It is the only place in the codebase that knows about both sides.
//!
//! # Design rationale
//!
//! Domain crates (`mako-gpke`, `mako-wim`, …) contain **pure domain logic**
//! and must never import `edi-energy`. The field-extraction code that maps
//! wire-format EDIFACT segments to domain command fields lives here, where
//! both `edi-energy` and the domain crates are visible.
//!
//! # Cross-FV behaviour
//!
//! Each registry registers a single `FnAdapter` that accepts **all known BDEW
//! format versions** (FV ≥ `FV2024-10-01`). The UTILMD S2.x wire format for
//! the fields used by the current workflows has been stable across all current
//! BDEW format versions. When a future release changes field layout, add an
//! internal branch on `fv` inside the adapter closure.
//!
//! # Adding a new format version
//!
//! 1. Add the new `FormatVersion` to `known_fvs()` below.
//! 2. If the wire format changed, branch inside the adapter closure.
//! 3. Rebuild and verify the startup `validate_policy` check passes.

use std::any::Any;

use dvgw_edi::AnyDvgwMessage;
use edi_energy::{AnyMessage, EdiEnergyMessage};
use edifact_rs::OwnedSegment;
use mako_engine::{
    error::EngineError,
    message_adapter::{AdapterRegistry, FnAdapter},
    types::{
        BillingPeriod, DeviceId, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator,
    },
    version::FormatVersion,
};
use rubo4e::current as bo4e;
use rust_decimal::Decimal;

/// Convert an `edi_energy::Pruefidentifikator` to the domain `Pruefidentifikator`.
///
/// This is the only permitted crossing point between the two crates for PID values.
/// The `edi-energy` type guarantees the code is already in the 10 000–99 999 range,
/// so the conversion must always succeed.
#[inline]
fn convert_pid(p: edi_energy::Pruefidentifikator) -> Result<Pruefidentifikator, EngineError> {
    Pruefidentifikator::new(p.as_u32())
        .map_err(|e| EngineError::Deserialization(format!("PID out of range: {e}")))
}

/// Parse a `CCYYMMDD` DTM value (format 102) into a UTC datetime at midnight.
fn parse_ccyymmdd(v: &str) -> Option<time::OffsetDateTime> {
    if v.len() != 8 || !v.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let year: i32 = v[0..4].parse().ok()?;
    let month = time::Month::try_from(v[4..6].parse::<u8>().ok()?).ok()?;
    let day: u8 = v[6..8].parse().ok()?;
    time::Date::from_calendar_date(year, month, day)
        .ok()
        .map(|d| d.midnight().assume_utc())
}
use mako_gabi_gas::{
    AllocationCommand, GaBiGasAllocationWorkflow, GaBiGasInvoicCommand, GaBiGasInvoicWorkflow,
    GaBiGasNominationWorkflow, NominationCommand, NomresAcceptance,
};
use mako_geli_gas::{
    GasKommunikationsdatenCommand, GasMsconsDatenCommand, GasSperrungLfCommand,
    GasSperrungNbCommand, GasSupplierChangeCommand, GeliGasLfStornierungWorkflow,
    GeliGasMsconsWorkflow, GeliGasPartinWorkflow, GeliGasSperrprozesseInvoicCommand,
    GeliGasSperrprozesseInvoicWorkflow, GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow,
    GeliGasStornierungCommand, GeliGasStornierungWorkflow, GeliGasSupplierChangeWorkflow,
    LfStornierungCommand,
};
use mako_gpke::{
    AbrechnungCommand, AllokationslisteCommand, AnfrageBestellungCommand,
    AnkuendigungZuordnungLfCommand, DatanabrufCommand, GpkeAbrechnungWorkflow,
    GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeDatanabrufWorkflow,
    GpkeKonfigurationAenderungWorkflow, GpkeKonfigurationWorkflow, GpkeLfAbmeldungWorkflow,
    GpkeLfAnmeldungWorkflow, GpkeMesswerteLieferungWorkflow, GpkeNeuanlageWorkflow,
    GpkePartinWorkflow, GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, GpkeStornierungCommand,
    GpkeStornierungWorkflow, GpkeSupplierChangeWorkflow, GpkeUtiltsWorkflow,
    KommunikationsdatenCommand, KonfigurationAenderungCommand, KonfigurationCommand,
    LfAbmeldungCommand, LfAnmeldungCommand, MesswerteLieferungCommand, NeuanlageCommand,
    SperrungCommand, SperrungLfCommand, SupplierChangeCommand, UtiltsKonfigCommand,
};
use mako_mabis::{
    BillingCommand, ClearinglisteCommand, DataStatus, IFTSTA_DATENSTATUS_PID, MabisBillingWorkflow,
    MabisClearinglisteWorkflow,
};
use mako_wim::{
    DeviceChangeCommand, GeraeteubernahmeCommand, PreisanfrageCommand, PreislisteCommand,
    StammdatenCommand, WimDeviceChangeWorkflow, WimGeraeteubernahmeWorkflow, WimInsrptWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimRechnungCommand, WimRechnungWorkflow,
    WimStammdatenWorkflow, esa_wertebestellung::EsaWertebestellungWorkflow,
    insrpt::StorungsmeldungCommand, wertebestellung::WimWertebestellungWorkflow,
};
use mako_wim_gas::{
    WimGasAnmeldungCommand, WimGasAnmeldungWorkflow, WimGasInsrptWorkflow, WimGasInvoicCommand,
    WimGasInvoicWorkflow, WimGasKuendigungCommand, WimGasKuendigungWorkflow,
    WimGasStornierungCommand, WimGasStornierungWorkflow, WimGasVerpflichtungsanfrageCommand,
    WimGasVerpflichtungsanfrageWorkflow, insrpt::GasStorungsmeldungCommand,
};

// ── Per-domain registry submodules ───────────────────────────────────────────
//
// One file per market-communication domain. Every registry fn is re-exported
// here so the flat `crate::adapters::*_registry` paths keep working.

mod gabi_gas;
mod geli_gas;
mod gpke;
mod mabis;
mod wim;
mod wim_gas;

pub use gabi_gas::*;
pub use geli_gas::*;
pub use gpke::*;
pub use mabis::*;
pub use wim::*;
pub use wim_gas::*;
// ── Known format versions ─────────────────────────────────────────────────────

// The set of BDEW format versions for which all active domain workflows must
// have registered adapters.
// ── IFTSTA extraction helpers ─────────────────────────────────────────────────

/// Extract a [`SupplierChangeCommand::ReceiveVollzugsmeldung`] from an IFTSTA
/// message routed to the GPKE supplier-change workflow (PIDs 21024–21033).
///
/// Called from `gpke_registry()` when `msg` is `AnyMessage::Iftsta`.
fn build_gpke_iftsta_command(msg: &AnyMessage) -> Result<SupplierChangeCommand, EngineError> {
    let AnyMessage::Iftsta(i) = msg else {
        return Err(EngineError::Deserialization(
            "GPKE IFTSTA adapter: expected AnyMessage::Iftsta".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("GPKE IFTSTA adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;
    let validation_result = msg.validate().ok();
    let validation_passed = validation_result
        .as_ref()
        .map(|r| r.is_valid())
        .unwrap_or(false);
    let validation_errors: Vec<String> = validation_result
        .as_ref()
        .map(|r| r.errors().iter().map(|e| format!("{e}")).collect())
        .unwrap_or_default();
    Ok(SupplierChangeCommand::ReceiveVollzugsmeldung {
        pid,
        sender: MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            i.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        message_ref: MessageRef::new(msg.message_ref()),
        validation_passed,
        validation_errors,
    })
}

/// Extract a [`DeviceChangeCommand::ReceiveIftsta`] from an IFTSTA message
/// routed to the WiM device-change workflow (PIDs 21009–21018).
///
/// Called from `wim_registry()` when `msg` is `AnyMessage::Iftsta`.
fn build_wim_iftsta_command(msg: &AnyMessage) -> Result<DeviceChangeCommand, EngineError> {
    let AnyMessage::Iftsta(i) = msg else {
        return Err(EngineError::Deserialization(
            "WiM IFTSTA adapter: expected AnyMessage::Iftsta".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("WiM IFTSTA adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;
    let validation_result = msg.validate().ok();
    let validation_passed = validation_result
        .as_ref()
        .map(|r| r.is_valid())
        .unwrap_or(false);
    let validation_errors: Vec<String> = validation_result
        .as_ref()
        .map(|r| r.errors().iter().map(|e| format!("{e}")).collect())
        .unwrap_or_default();
    Ok(DeviceChangeCommand::ReceiveIftsta {
        pid,
        sender: MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            i.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        message_ref: MessageRef::new(msg.message_ref()),
        validation_passed,
        validation_errors,
    })
}

/// Extract a [`BillingCommand::ReceiveIftsta`] from an IFTSTA message routed
/// to the MABIS billing workflow (PIDs 21000–21007).
///
/// Called from `mabis_registry()` when `msg` is `AnyMessage::Iftsta`.
///
/// For PID 21004 (Datenstatus vom BIKO), the `DataStatus` value is extracted
/// from the STS segment element 2 (DE 9013, status reason):
///
/// | STS element 2 | `DataStatus` variant |
/// |---------------|----------------------|
/// | `Z03`         | `Abrechnungsdaten`   |
/// | `Z49`         | `AbgerechtneteDaten` |
/// | `Z86`         | `AbgerechtneteDatenKbka` |
///
/// Codes are per BDEW MaBiS IFTSTA AHB 2.0g. All other MaBiS PIDs set
/// `data_status = None`.
fn build_mabis_iftsta_command(msg: &AnyMessage) -> Result<BillingCommand, EngineError> {
    let AnyMessage::Iftsta(i) = msg else {
        return Err(EngineError::Deserialization(
            "MABIS IFTSTA adapter: expected AnyMessage::Iftsta".into(),
        ));
    };
    let pid = msg
        .detect_pruefidentifikator()
        .map_err(|e| {
            EngineError::Deserialization(format!("MABIS IFTSTA adapter: PID detection failed: {e}"))
        })
        .and_then(convert_pid)?;
    let validation_result = msg.validate().ok();
    let validation_passed = validation_result
        .as_ref()
        .map(|r| r.is_valid())
        .unwrap_or(false);
    let validation_errors: Vec<String> = validation_result
        .as_ref()
        .map(|r| r.errors().iter().map(|e| format!("{e}")).collect())
        .unwrap_or_default();

    // For PID 21004 (Statusmeldung vom BIKO an BKV/NB), extract the Datenstatus
    // code from the first STS segment at element index 2 (DE 9013, status reason).
    //
    // IFTSTA MaBiS AHB 2.0g STS segment: STS+status_category+status_code+status_reason
    // The BDEW Datenstatus qualifier occupies element position 2 (0-based).
    let data_status = if pid == IFTSTA_DATENSTATUS_PID {
        i.segments()
            .iter()
            .find(|s| s.tag == "STS")
            .and_then(|s| s.element_str(2))
            .and_then(|code| match code {
                "Z03" => Some(DataStatus::Abrechnungsdaten),
                "Z49" => Some(DataStatus::AbgerechtneteDaten),
                "Z86" => Some(DataStatus::AbgerechtneteDatenKbka),
                _ => None,
            })
    } else {
        None
    };

    Ok(BillingCommand::ReceiveIftsta {
        pid,
        sender: MarktpartnerCode::new(i.sender().and_then(|n| n.party_id.as_deref()).unwrap_or("")),
        receiver: MarktpartnerCode::new(
            i.receiver()
                .and_then(|n| n.party_id.as_deref())
                .unwrap_or(""),
        ),
        message_ref: MessageRef::new(msg.message_ref()),
        validation_passed,
        validation_errors,
        data_status,
    })
}

///
/// Returns `true` when `fv` is in the set of format versions derived from the
/// compiled `edi-energy` profile registry.
///
/// Prefer this over lexicographic `>=` comparisons — a new FV is not
/// automatically supported until `edi-energy` has a profile with that
/// `valid_from` date and the adapter closures above have been verified.
fn is_known_fv(fv: &FormatVersion) -> bool {
    known_fvs().iter().any(|k| k.as_str() == fv.as_str())
}

/// Return all BDEW format versions registered in the compiled `edi-energy`
/// profile registry, sorted chronologically.
///
/// This replaces the previously hand-maintained allowlist. Adding a new BDEW
/// format version now only requires shipping a new `edi-energy` profile;
/// `makod` picks it up automatically on the next rebuild.
///
/// If the wire-format *changed* in the new format version, add a branch on
/// `fv` inside the relevant adapter closure above before deploying.
#[must_use]
pub fn known_fvs() -> Vec<FormatVersion> {
    edi_energy::registry::ReleaseRegistry::global()
        .format_versions()
        .into_iter()
        .filter_map(|s| FormatVersion::parse(&s).ok())
        .collect()
}

// ── EDIFACT → BO4E anti-corruption layer ─────────────────────────────────────

/// Convert raw INVOIC EDIFACT segments into a [`bo4e::Rechnung`].
///
/// This is the **only** place in the codebase where EDIFACT segment parsing
/// knowledge about the INVOIC message structure is combined with BO4E object
/// construction.  All downstream domain logic and the `invoic-checker` engine
/// work exclusively with the resulting [`bo4e::Rechnung`].
///
/// # Date types
///
/// In rubo4e, `Zeitraum.startdatum / enddatum` are `time::Date` (date-only).
/// EDIFACT DTM `YYYYMMDD` values are parsed to `time::Date` for all period
/// fields. Delivery periods are wrapped in a `Zeitraum` and stored in
/// `Rechnungsposition.lieferungszeitraum` (v202607 schema).
#[must_use]
fn build_rechnung(segs: &[OwnedSegment]) -> bo4e::Rechnung {
    // Split at the first LIN segment: header vs. detail sections.
    let lin_start = segs
        .iter()
        .position(|s| s.tag == "LIN")
        .unwrap_or(segs.len());
    let header = &segs[..lin_start];

    // Zeitraum.startdatum/enddatum are time::Date in rubo4e.
    let period_start = dtm(header, "163").and_then(edifact_date_to_date);
    let period_end = dtm(header, "164").and_then(edifact_date_to_date);
    // Rechnung.rechnungsdatum is still OffsetDateTime.
    // rechnungsdatum is time::Date in rubo4e (follows *datum convention).
    let invoice_date = dtm(header, "137").and_then(edifact_date_to_date);

    let gesamtnetto = moa_betrag(header, "79");
    let gesamtbrutto = moa_betrag(header, "9");

    let rechnungsnummer = segs
        .iter()
        .find(|s| s.tag == "BGM")
        .and_then(|s| s.component_str(1, 0))
        .map(str::to_owned);

    let rechnungsperiode = match (period_start, period_end) {
        (Some(s), Some(e)) => Some(bo4e::Zeitraum {
            startdatum: Some(s),
            enddatum: Some(e),
            ..Default::default()
        }),
        _ => None,
    };
    let rechnungspositionen = {
        let p = build_positions(segs);
        if p.is_empty() { None } else { Some(p) }
    };
    bo4e::Rechnung {
        rechnungsnummer,
        rechnungsdatum: invoice_date,
        rechnungsperiode,
        gesamtnetto,
        gesamtbrutto,
        rechnungspositionen,
        ..Default::default()
    }
}

/// Build a `Vec<Rechnungsposition>` by splitting on `LIN` segment boundaries.
fn build_positions(segs: &[OwnedSegment]) -> Vec<bo4e::Rechnungsposition> {
    let mut result = Vec::new();
    let mut group: Vec<&OwnedSegment> = Vec::new();
    let mut in_detail = false;

    for seg in segs {
        if seg.tag == "LIN" {
            if in_detail && !group.is_empty() {
                result.push(build_position(&group));
            }
            group.clear();
            in_detail = true;
        }
        if in_detail {
            group.push(seg);
        }
    }
    if in_detail && !group.is_empty() {
        result.push(build_position(&group));
    }
    result
}

/// Build a single `Rechnungsposition` from one LIN group.
fn build_position(group: &[&OwnedSegment]) -> bo4e::Rechnungsposition {
    let positionsnummer = group
        .first()
        .and_then(|s| s.component_str(0, 0))
        .and_then(|s| s.parse::<i64>().ok());

    // `lokations_id` was removed in BO4E v202607; store it as positionstext.
    let positionstext = group
        .iter()
        .find(|s| s.tag == "LOC" && s.component_str(0, 0) == Some("172"))
        .and_then(|s| s.component_str(1, 0))
        .map(str::to_owned);

    // Delivery period now lives in lieferungszeitraum (v202607).
    let lieferung_von = dtm_in_group(group, "163").and_then(edifact_date_to_date);
    let lieferung_bis = dtm_in_group(group, "164").and_then(edifact_date_to_date);
    let lieferungszeitraum = if lieferung_von.is_some() || lieferung_bis.is_some() {
        Some(bo4e::Zeitraum {
            startdatum: lieferung_von,
            enddatum: lieferung_bis,
            ..Default::default()
        })
    } else {
        None
    };

    let positions_menge = group
        .iter()
        .find(|s| s.tag == "QTY" && s.component_str(0, 0) == Some("46"))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|wert| {
            let normalized = wert.replace(',', ".");
            normalized.parse::<Decimal>().ok().map(|d| bo4e::Menge {
                wert: Some(d),
                einheit: Some(bo4e::Mengeneinheit::Kwh),
                ..Default::default()
            })
        });

    let einzelpreis = group
        .iter()
        .find(|s| s.tag == "PRI" && s.component_str(0, 0) == Some("AAB"))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|p| {
            let normalized = p.replace(',', ".");
            normalized.parse::<Decimal>().ok().map(|d| bo4e::Preis {
                wert: Some(d),
                ..Default::default()
            })
        });

    let gesamtpreis = moa_betrag_in_group(group, "77");

    bo4e::Rechnungsposition {
        positionsnummer,
        positionstext,
        lieferungszeitraum,
        positions_menge,
        einzelpreis,
        gesamtpreis,
        ..Default::default()
    }
}

// ── EDIFACT segment accessor helpers ─────────────────────────────────────────

/// Find the value of a `DTM` segment with a given qualifier in a slice.
fn dtm<'a>(segs: &'a [OwnedSegment], qualifier: &str) -> Option<&'a str> {
    segs.iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
}

/// Find the value of a `DTM` segment within a LIN group (slices of references).
fn dtm_in_group<'a>(group: &[&'a OwnedSegment], qualifier: &str) -> Option<&'a str> {
    group
        .iter()
        .find(|s| s.tag == "DTM" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
}

/// Build a [`bo4e::Betrag`] from a `MOA` segment with a given qualifier.
fn moa_betrag(segs: &[OwnedSegment], qualifier: &str) -> Option<bo4e::Betrag> {
    segs.iter()
        .find(|s| s.tag == "MOA" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|wert| {
            wert.replace(',', ".")
                .parse::<Decimal>()
                .ok()
                .map(|d| bo4e::Betrag {
                    wert: Some(d),
                    ..Default::default()
                })
        })
}

/// Build a [`bo4e::Betrag`] from a `MOA` segment within a LIN group.
fn moa_betrag_in_group(group: &[&OwnedSegment], qualifier: &str) -> Option<bo4e::Betrag> {
    group
        .iter()
        .find(|s| s.tag == "MOA" && s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .and_then(|wert| {
            wert.replace(',', ".")
                .parse::<Decimal>()
                .ok()
                .map(|d| bo4e::Betrag {
                    wert: Some(d),
                    ..Default::default()
                })
        })
}

/// Convert an EDIFACT date (`YYYYMMDD`) to ISO 8601 (`YYYY-MM-DD`).
///
/// Lexicographic comparison of ISO dates is correct — required by
/// `invoic_checker`'s period-validity check (string comparison on
/// `Zeitraum.startdatum` / `enddatum`).
/// Parse an EDIFACT date string (`YYYYMMDD`) to a `time::Date`.
///
/// Used for BO4E fields typed as `Option<time::Date>` in rubo4e
/// (e.g. `Zeitraum.startdatum`, `Zeitraum.enddatum`).
///
/// Returns `None` if the string is not exactly 8 digits or cannot be parsed as a
/// valid calendar date.
fn edifact_date_to_date(yyyymmdd: &str) -> Option<time::Date> {
    use time::{Date, Month};
    if yyyymmdd.len() != 8 {
        return None;
    }
    let year: i32 = yyyymmdd[..4].parse().ok()?;
    let month: u8 = yyyymmdd[4..6].parse().ok()?;
    let day: u8 = yyyymmdd[6..8].parse().ok()?;
    let month = Month::try_from(month).ok()?;
    Date::from_calendar_date(year, month, day).ok()
}

// ── Gas quality helpers (PID 13007 Gasbeschaffenheitsdaten) ──────────────────
//
// PID 13007 MSCONS carries Brennwert and Zustandszahl in QTY segments:
//   QTY+Z08:{value} — Abrechnungsbrennwert (kWh/m³)
//   QTY+Z10:{value} — Zustandszahl (dimensionless compressibility factor)
//
// Source: Allgemeine Festlegungen V6.1d §6 / MSCONS AHB Gas 1.x.

/// Extract Abrechnungsbrennwert from `QTY+Z08` in a Gas MSCONS.
///
/// Scans all delivery-point → time-series → line-item → quantity leaves
/// for the first quantity with qualifier `Z08` and returns its value.
fn extract_qty_z08(m: &edi_energy::messages::mscons::MsconsMessage) -> Option<String> {
    for dp in m.delivery_points() {
        for ts in &dp.time_series {
            for item in &ts.items {
                for qty in &item.quantities {
                    if qty.qty.qualifier == "Z08" {
                        let normalized = qty
                            .qty
                            .value
                            .as_deref()
                            .map(|v| v.replace(',', "."))
                            .unwrap_or_default();
                        if !normalized.is_empty() {
                            return Some(normalized);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Extract Zustandszahl from `QTY+Z10` in a Gas MSCONS.
///
/// Scans all delivery-point → time-series → line-item → quantity leaves
/// for the first quantity with qualifier `Z10` and returns its value.
fn extract_qty_z10(m: &edi_energy::messages::mscons::MsconsMessage) -> Option<String> {
    for dp in m.delivery_points() {
        for ts in &dp.time_series {
            for item in &ts.items {
                for qty in &item.quantities {
                    if qty.qty.qualifier == "Z10" {
                        let normalized = qty
                            .qty
                            .value
                            .as_deref()
                            .map(|v| v.replace(',', "."))
                            .unwrap_or_default();
                        if !normalized.is_empty() {
                            return Some(normalized);
                        }
                    }
                }
            }
        }
    }
    None
}

// ── UTILMD Typenmerkmale (TM) segment extractors ─────────────────────────────
//
// The BDEW UTILMD S2.x TM segment encodes energy classification metadata:
//   TM+EM+<qualifier>  — Energiemenge / Bilanzierungsmethode
//   TM+Z10+<code>      — Gas GaBi Fallgruppe (RLM only)
//
// These extractors scan the raw UTILMD segment list. They are best-effort:
// if the segment is absent or malformed, they return `None`.

/// Extract `bilanzierungsmethode` from a UTILMD segment list.
///
/// Maps `TM+EM` qualifier to BO4E `Bilanzierungsmethode`:
/// - Z01 → `"SLP"` (Standardlastprofil)
/// - Z02 → `"RLM"` (Registrierende Leistungsmessung)
/// - Z04 → `"IMS"` (Intelligentes Messsystem / iMSys)
pub fn extract_bilanzierungsmethode(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter()
        .find(|s| s.tag == "TM" && s.element_str(0).is_some_and(|q| q == "EM"))
        .and_then(|s| s.element_str(1))
        .and_then(|qualifier| match qualifier {
            "Z01" => Some("SLP".to_owned()),
            "Z02" => Some("RLM".to_owned()),
            "Z04" => Some("IMS".to_owned()),
            _ => None,
        })
}

/// Extract Gas GaBi `Fallgruppe` from a UTILMD segment list.
///
/// The `TM+Z10` segment in Gas UTILMD encodes the GaBi RLM Fallgruppe,
/// which determines whether the Gas MMM uses the `differenzierter` or
/// `pauschalierter` Abwicklungsweg (§ 4 GaBi-Strom / §5 KoV IX).
///
/// Returns the raw DE 7065 value (e.g. `"Z01"`, `"Z02"`) when present.
pub fn extract_fallgruppe(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter()
        .find(|s| s.tag == "TM" && s.element_str(0).is_some_and(|q| q == "Z10"))
        .and_then(|s| s.element_str(1))
        .map(str::to_owned)
}

/// Extract gas quality type from a UTILMD G segment list.
///
/// ## Current status — placeholder for H2-blend AHBs (2026–2028)
///
/// The DVGW and BNetzA have not yet standardized an EDIFACT qualifier for gas
/// quality type (`H_GAS` | `L_GAS` | `H2_BLEND`) in UTILMD G messages.
///
/// When the 2026–2028 H2-blend AHB wave is published, add the UTILMD G segment
/// code here — e.g. `TM+Z20` or a `CAV`/`ALC` characteristic.  The
/// `mako_geli_gas::gas_quality::GasQualitaet::from_raw()` normalization function
/// will convert whatever raw qualifier is used to the canonical `H_GAS` / `L_GAS` /
/// `H2_BLEND` form before storage in `marktd.malo.gasqualitaet`.
///
/// ## DVGW G 260 background
///
/// German gas quality types are defined in DVGW G 260 §3.2:
/// - **H-Gas** (high calorific): Wobbe index 12.4–15.7 kWh/m³
/// - **L-Gas** (low calorific): Wobbe index 10.5–13.0 kWh/m³
///
/// ## H2-blend EDIFACT pilot observation
///
/// In GET H2 and GASCADE H2 pilot messages (2025), some implementations carry
/// gas quality information in the `MKT+Z10` or `CAV+Z20` characteristic segment.
/// These are NOT standardized in the BDEW AHB yet. A monitoring adapter
/// that logs unknown `TM`/`CAV` qualifiers would help detect new codes before
/// the formal AHB publication.
#[allow(unused_variables)]
pub fn extract_gasqualitaet(segs: &[OwnedSegment]) -> Option<String> {
    use mako_geli_gas::gas_quality::normalize_gasqualitaet;
    // Placeholder: scan for any TM segment with a gas-quality-like qualifier.
    // Currently returns None for all standard UTILMD G messages.
    // TODO: add the canonical BDEW AHB segment code when published (2026-2028 wave).
    // Example future implementation:
    //   segs.iter()
    //       .find(|s| s.tag == "TM" && s.element_str(0).is_some_and(|q| q == "Z20"))
    //       .and_then(|s| s.element_str(1))
    //       .map(|raw| normalize_gasqualitaet(raw).to_owned())
    let _ = normalize_gasqualitaet; // suppress unused warning until real mapping is added
    None
}

// ── DVGW gas-day conversion helper ───────────────────────────────────────────

/// Parse a DVGW reference-date string (`YYYY-MM-DD` or `YYYYMMDD`) into a
/// typed [`mako_gabi_gas::GasDay`].
///
/// DVGW messages encode the gas day in DTM qualifier 137.  The format is
/// `YYYYMMDD` in older versions and `YYYY-MM-DD` in current NOMINT/ALOCAT.
/// Both are accepted here; an invalid or absent date falls back to today.
fn parse_dvgw_gas_day(raw: Option<&str>) -> mako_gabi_gas::GasDay {
    let fallback = || mako_gabi_gas::GasDay::new(time::OffsetDateTime::now_utc().date());
    let Some(s) = raw else { return fallback() };
    // Try ISO 8601 first (`YYYY-MM-DD`), then compact form (`YYYYMMDD`).
    if let Ok(d) = mako_gabi_gas::GasDay::parse(s) {
        return d;
    }
    // Compact `YYYYMMDD` → insert dashes and retry.
    if s.len() == 8 {
        let iso = format!("{}-{}-{}", &s[..4], &s[4..6], &s[6..8]);
        if let Ok(d) = mako_gabi_gas::GasDay::parse(&iso) {
            return d;
        }
    }
    tracing::warn!(
        raw = s,
        "adapters: could not parse DVGW gas day — using today as fallback"
    );
    fallback()
}
