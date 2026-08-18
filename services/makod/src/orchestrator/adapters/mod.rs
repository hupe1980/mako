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
pub(crate) fn parse_ccyymmdd(v: &str) -> Option<time::OffsetDateTime> {
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
    AnkuendigungZuordnungLfCommand, BeendigungZuordnungCommand, DatanabrufCommand,
    GpkeAbrechnungWorkflow, GpkeAllokationslisteWorkflow, GpkeAnfrageBestellungWorkflow,
    GpkeAnkuendigungZuordnungLfWorkflow, GpkeBeendigungZuordnungWorkflow, GpkeDatanabrufWorkflow,
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
    StammdatenCommand, TechnikAenderungCommand, WimDeviceChangeWorkflow,
    WimGeraeteubernahmeWorkflow, WimInsrptWorkflow, WimInvoicCommand, WimInvoicWorkflow,
    WimPreisanfrageWorkflow, WimPreislisteWorkflow, WimStammdatenWorkflow,
    WimTechnikAenderungWorkflow, esa_wertebestellung::EsaWertebestellungWorkflow,
    insrpt::StorungsmeldungCommand, wertebestellung::WimWertebestellungWorkflow,
};
use mako_wim_gas::{
    GasGeraeteubernahmeCommand, WimGasAnmeldungCommand, WimGasAnmeldungWorkflow,
    WimGasGeraeteubernahmeWorkflow, WimGasInsrptWorkflow, WimGasInvoicCommand,
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

/// Accept-predicate shared by every adapter in this module.
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

// ── Adapter coverage ─────────────────────────────────────────────────────────

/// Coverage verdict for one adapter registry.
pub struct RegistryCoverage {
    /// Name of the `*_registry` constructor this verdict describes.
    pub registry: &'static str,
    /// Number of adapters the registry holds.
    pub adapters: usize,
    /// Format versions no adapter in the registry accepts.
    pub uncovered: Vec<FormatVersion>,
}

/// Build one coverage verdict per registry.
///
/// The table is the single enumeration of the module's registries. The
/// `every_registry_is_in_the_coverage_table` test below scans this module's
/// submodules for `pub fn …_registry` and fails when one is missing, so a new
/// registry cannot be added without also being checked at startup — which is
/// exactly how twenty of them silently escaped the previous hand-maintained
/// list in `startup.rs`.
macro_rules! coverage_table {
    ($($name:ident),+ $(,)?) => {
        /// Report adapter coverage for every registry in this module.
        ///
        /// A registry is covered when some adapter in it accepts every format
        /// version in [`known_fvs`]. Consumed by
        /// `startup::validate_adapter_coverage`, which refuses to boot on a gap.
        #[must_use]
        pub fn coverage() -> Vec<RegistryCoverage> {
            let known = known_fvs();
            vec![$({
                let registry = $name();
                RegistryCoverage {
                    registry: stringify!($name),
                    adapters: registry.len(),
                    uncovered: registry
                        .validate_policy(
                            &mako_engine::version::WorkflowVersionPolicy::ForwardCompatible,
                            &known,
                        )
                        .err()
                        .unwrap_or_default(),
                }
            }),+]
        }
    };
}

coverage_table! {
    esa_wertebestellung_registry,
    gabi_gas_allocation_registry,
    gabi_gas_comdis_registry,
    gabi_gas_invoic_registry,
    gabi_gas_nomination_registry,
    gabi_gas_remadv_registry,
    geli_gas_datenabruf_ablehnung_registry,
    geli_gas_datenabruf_receive_registry,
    geli_gas_lf_anmeldung_registry,
    geli_gas_mscons_registry,
    geli_gas_partin_registry,
    geli_gas_registry,
    geli_gas_sperrprozesse_invoic_registry,
    geli_gas_sperrung_lf_registry,
    geli_gas_sperrung_nb_registry,
    geli_gas_sperrung_nb_response_registry,
    geli_gas_sperrung_nb_stornierung_registry,
    geli_gas_stammdaten_registry,
    geli_gas_stornierung_lf_registry,
    geli_gas_stornierung_registry,
    gpke_abrechnung_comdis_registry,
    gpke_abrechnung_registry,
    gpke_abrechnung_remadv_registry,
    gpke_allokationsliste_mscons_registry,
    gpke_allokationsliste_ordrsp_registry,
    gpke_anfrage_bestellung_registry,
    gpke_ankuendigung_zuordnung_lf_registry,
    gpke_beendigung_zuordnung_registry,
    gpke_datenabruf_registry,
    gpke_eog_registry,
    gpke_konfiguration_aenderung_registry,
    gpke_konfiguration_registry,
    gpke_lf_abmeldung_registry,
    gpke_lf_anmeldung_registry,
    gpke_messwerte_registry,
    gpke_neuanlage_registry,
    gpke_partin_registry,
    gpke_registry,
    gpke_sperrung_lf_registry,
    gpke_sperrung_msb_response_registry,
    gpke_sperrung_registry,
    gpke_sperrung_stornierung_registry,
    gpke_stammdaten_registry,
    gpke_stornierung_registry,
    gpke_utilts_registry,
    mabis_anforderung_registry,
    mabis_clearingliste_registry,
    mabis_listenabgleich_registry,
    mabis_registry,
    mabis_zp_lifecycle_registry,
    wim_gas_anmeldung_registry,
    wim_gas_geraeteubernahme_registry,
    wim_gas_insrpt_registry,
    wim_gas_invoic_comdis_registry,
    wim_gas_invoic_registry,
    wim_gas_invoic_remadv_registry,
    wim_gas_kuendigung_registry,
    wim_gas_stornierung_registry,
    wim_gas_verpflichtungsanfrage_registry,
    wim_geraeteubernahme_registry,
    wim_insrpt_registry,
    wim_invoic_comdis_registry,
    wim_invoic_registry,
    wim_invoic_remadv_registry,
    wim_preisanfrage_registry,
    wim_preisliste_registry,
    wim_rechnungsabwicklung_registry,
    wim_registry,
    wim_stammdaten_registry,
    wim_stammdaten_uebermittlung_registry,
    wim_technik_aenderung_registry,
    wim_wertebestellung_registry,
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

/// Extract the EoG **Versorgungsart** from a UTILMD segment list.
///
/// The Versorgungsart travels in a SG10 `CCI+Z36` segment, DE7037 =
/// `ZC9` (Ersatzversorgung) / `ZD0` (Grundversorgung) / `ZE3`
/// (Ersatzbelieferung) / `ZZD` (Übergangsversorgung). These four codes are
/// distinctive, so the extractor scans the CCI segment elements rather than
/// hard-coding a component index that varies between AHB revisions.
///
/// Returns the raw AHB code (e.g. `"ZD0"`); the caller maps it to
/// `mako_gpke::Versorgungsart` via `from_code`.
pub fn extract_versorgungsart(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter().filter(|s| s.tag == "CCI").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find(|v| matches!(*v, "ZC9" | "ZD0" | "ZE3" | "ZZD"))
            .map(str::to_owned)
    })
}

/// Extract the **Haushaltskunde** flag from a UTILMD segment list.
///
/// The household indicator travels in a SG10 `CCI` segment, DE7037 =
/// `Z15` (Haushaltskunde → `true`) / `Z18` (Nicht-Haushaltskunde → `false`).
/// `None` when neither code is present. The Z15/Z18 direction is
/// operator-verifiable against the current UTILMD AHB Strom.
pub fn extract_haushaltskunde(segs: &[OwnedSegment]) -> Option<bool> {
    segs.iter().filter(|s| s.tag == "CCI").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find_map(|v| match v {
                "Z15" => Some(true),
                "Z18" => Some(false),
                _ => None,
            })
    })
}

/// Extract the **Netzebene** from a UTILMD segment list (Stammdatenänderung).
///
/// The Netzebene (voltage/pressure level) travels in a SG8 `SEQ+Z27` group's
/// `CCI`/`CAV` characteristic in newer AHBs. The codes (`E03`..`E06` Strom,
/// gas pressure levels) are read from the CAV value following a Netzebene CCI.
/// Returns the raw code when present. Best-effort: `None` when absent.
pub fn extract_netzebene(segs: &[OwnedSegment]) -> Option<String> {
    extract_cav_after_cci(segs, "Z27")
}

/// Extract the **Energierichtung** (Einspeisung / Entnahme) from a UTILMD
/// segment list. Travels in a SG10 `CCI` characteristic; `Z50`/`Z51` (or the
/// `CAV` value under the Energierichtung CCI). Best-effort.
pub fn extract_energierichtung(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter().filter(|s| s.tag == "CCI").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find_map(|v| match v {
                "Z50" => Some("EINSPEISUNG".to_owned()),
                "Z51" => Some("ENTNAHME".to_owned()),
                _ => None,
            })
    })
}

/// Extract the **Regelzone** (EIC) from a UTILMD segment list. Travels in a
/// SG10 `CAV` value under the Regelzone `CCI` class, or a `LOC+Z28`. Reads the
/// CAV value that follows a Regelzone CCI. Best-effort.
pub fn extract_regelzone(segs: &[OwnedSegment]) -> Option<String> {
    extract_cav_after_cci(segs, "Z28")
}

/// Extract the §14a EnWG **Status der Fernsteuerbarkeit** of the Marktlokation.
///
/// Travels in a SG10 `CCI` characteristic (class `7059 = Z24`) whose `7037`
/// value is `Z97` (technisch fernsteuerbar → `true`) or `Z96` (technisch nicht
/// fernsteuerbar → `false`), per UTILMD AHB Strom 2.2 Kap. 9 (Änderung Daten
/// der MaLo). `None` when the characteristic is absent. The Z96/Z97 codes are
/// operator-verifiable against the current UTILMD AHB Strom.
pub fn extract_fernsteuerbarkeit(segs: &[OwnedSegment]) -> Option<bool> {
    segs.iter().filter(|s| s.tag == "CCI").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find_map(|v| match v {
                "Z97" => Some(true),
                "Z96" => Some(false),
                _ => None,
            })
    })
}

/// Extract the §14a EnWG **Steuerkanal** presence (NeLo, Redispatch 2.0).
///
/// Travels in a SG10 `CCI` characteristic (class `7059 = Z49`, Steuerkanal)
/// whose `7037` value is `ZF3` (Steuerkanal vorhanden → `true`) or `ZF2` (Kein
/// Steuerkanal vorhanden → `false`), per UTILMD AHB Strom 2.2 Kap. 9.
/// Operator-verifiable against the current UTILMD AHB Strom.
pub fn extract_steuerkanal(segs: &[OwnedSegment]) -> Option<bool> {
    segs.iter().filter(|s| s.tag == "CCI").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find_map(|v| match v {
                "ZF3" => Some(true),
                "ZF2" => Some(false),
                _ => None,
            })
    })
}

/// Extract the **zugeordneter Messstellenbetreiber** (serving MSB) of a
/// Messlokation from a UTILMD MeLo Stammdatenänderung.
///
/// Travels in the SG10 `CCI 7037=ZB3` (Zugeordneter Marktpartner) group as a
/// `CAV` whose `7111` qualifier is `Z91` (MSB), per UTILMD AHB Strom 2.2
/// Kap. 9.1.5. The MSB's MP-ID (13-digit BDEW Codenummer) is the 13-digit
/// numeric component of that `CAV` — the other components are Z-coded qualifiers
/// (`Z91`, and the `Z39` grundzuständig / `Z19` vertraglich eigenschaft), so the
/// 13-digit-numeric test identifies the MP-ID unambiguously without depending on
/// the exact C889 component index. Returns the `ZF0` gMSB entry only if no `Z91`
/// serving MSB is present. `None` when absent.
pub fn extract_zugeordneter_msb(segs: &[OwnedSegment]) -> Option<String> {
    let mp_id_of = |s: &OwnedSegment| -> Option<String> {
        (0..5)
            .filter_map(|c| s.component_str(0, c))
            .find(|v| v.len() == 13 && v.bytes().all(|b| b.is_ascii_digit()))
            .map(str::to_owned)
    };
    // Prefer the Z91 serving MSB; fall back to the ZF0 gMSB.
    segs.iter()
        .filter(|s| s.tag == "CAV")
        .find(|s| s.component_str(0, 0) == Some("Z91"))
        .and_then(mp_id_of)
        .or_else(|| {
            segs.iter()
                .filter(|s| s.tag == "CAV")
                .find(|s| s.component_str(0, 0) == Some("ZF0"))
                .and_then(mp_id_of)
        })
}

/// Extract the **Fernschaltbarkeit** of a Technische Ressource (Redispatch 2.0).
///
/// Travels in a SG10 `CAV` carrying the Fernschaltung class (`7111 = Z58`) plus
/// the value (`7110`): `Z06` (vorhanden → `true`) / `Z07` (nicht vorhanden →
/// `false`), per UTILMD AHB Strom 2.2 Kap. 9. Conservative: only fires when the
/// `Z58` class marker co-occurs with a `Z06`/`Z07` value in the same `CAV`, so a
/// generic `Z06`/`Z07` elsewhere is not misread.
pub fn extract_ist_fernschaltbar(segs: &[OwnedSegment]) -> Option<bool> {
    segs.iter().filter(|s| s.tag == "CAV").find_map(|s| {
        let vals: Vec<&str> = (0..6).filter_map(|i| s.element_str(i)).collect();
        if !vals.contains(&"Z58") {
            return None;
        }
        if vals.contains(&"Z06") {
            Some(true)
        } else if vals.contains(&"Z07") {
            Some(false)
        } else {
            None
        }
    })
}

/// Extract the **Art und Nutzung der Technischen Ressource** — the TR object's
/// own classification, as the BO4E `TechnischeRessourceNutzung` wire code.
///
/// Travels in a SG10 `CCI` characteristic (class `7059`): `Z17`
/// (Stromverbrauchsart), `Z50` (Stromerzeugungsart), or `Z56` (Speicher), per
/// UTILMD AHB Strom 2.2 Kap. 9 (Daten der technischen Ressource — PIDs
/// 55617/55623/55629/55635). `None` when absent.
///
/// **Not** the MaLo `CCI+7059=Z69` „Art und Nutzung der technischen Einrichtung"
/// (a MaLo Verbrauchsart, `CAV+7111=Z64` Kraft/Licht) — a different object; the
/// TR nutzung uses the disjoint `Z17`/`Z50`/`Z56` class codes, so the two never
/// collide.
pub fn extract_tr_nutzung(segs: &[OwnedSegment]) -> Option<&'static str> {
    segs.iter().filter(|s| s.tag == "CCI").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find_map(|v| match v {
                "Z17" => Some("STROMVERBRAUCHSART"),
                "Z50" => Some("STROMERZEUGUNGSART"),
                "Z56" => Some("SPEICHER"),
                _ => None,
            })
    })
}

/// Extract the TR **Verbrauchsart** (BO4E `TechnischeRessourceVerbrauchsart`),
/// present when the Nutzung is Stromverbrauchsart.
///
/// Travels in a SG10 `CAV` (class `7111`): `Z64` (Kraft/Licht), `Z65`
/// (Wärme/Kälte), `ZE5` (E-Mobilität), or `ZA8` (Straßenbeleuchtung), per UTILMD
/// AHB Strom 2.2 Kap. 9. Returns the BO4E wire code. `None` when absent.
pub fn extract_tr_verbrauchsart(segs: &[OwnedSegment]) -> Option<&'static str> {
    segs.iter().filter(|s| s.tag == "CAV").find_map(|s| {
        (0..4)
            .filter_map(|i| s.element_str(i))
            .find_map(|v| match v {
                "Z64" => Some("KRAFT_LICHT"),
                "Z65" => Some("WAERME"),
                "ZE5" => Some("E_MOBILITAET"),
                "ZA8" => Some("STRASSENBELEUCHTUNG"),
                _ => None,
            })
    })
}

/// Extract the SteuerbareRessource **Konfigurationsprodukte** as a BO4E
/// `Vec<Konfigurationsprodukt>` (serialised to a JSONB array).
///
/// Each contracted product is one SG8 `SEQ+Z79` („Bestandteil eines
/// Produktpakets") group; within it, per UTILMD AHB Strom 2.2 Kap. 9 (SR):
/// - `produktcode` — SG8 `PIA+5`, DE7140 (Produkt-Code);
/// - `marktpartner` — the zugeordneter Marktpartner in a SG10 `CAV` whose
///   `7111` qualifier is `Z91` (MSB) or `ZF0` (gMSB), keyed by its 13-digit
///   BDEW Codenummer;
/// - `leistungskurvendefinition` — the Produkteigenschaft value (SG10
///   `CCI+7059=Z66` → `CAV+7111=ZH9`/`ZV4`), best-effort.
///
/// A produktcode-only projection would drop the produktcode↔MSB pairing, so this
/// walks each group and preserves all three BO4E fields. Returns `None` when the
/// message carries no `SEQ+Z79` product group.
pub fn extract_sr_konfigurationsprodukte(segs: &[OwnedSegment]) -> Option<serde_json::Value> {
    let mut produkte = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if segs[i].tag == "SEQ" && segs[i].element_str(0) == Some("Z79") {
            let start = i + 1;
            let mut end = start;
            while end < segs.len() && segs[end].tag != "SEQ" {
                end += 1;
            }
            if let Some(kp) = konfigurationsprodukt_from_group(&segs[start..end]) {
                produkte.push(kp);
            }
            i = end;
        } else {
            i += 1;
        }
    }
    (!produkte.is_empty()).then(|| serde_json::Value::Array(produkte))
}

/// Build one BO4E `Konfigurationsprodukt` JSON object from a `SEQ+Z79` group.
fn konfigurationsprodukt_from_group(group: &[OwnedSegment]) -> Option<serde_json::Value> {
    // produktcode: SG8 PIA+5 (4347=5, Produktidentifikation), DE7140 in element 1.
    let produktcode = group
        .iter()
        .find(|s| s.tag == "PIA" && s.element_str(0) == Some("5"))
        .and_then(|s| s.component_str(1, 0))
        .map(str::to_owned)?;
    let mut kp = serde_json::Map::new();
    kp.insert("_typ".into(), "KONFIGURATIONSPRODUKT".into());
    kp.insert("produktcode".into(), produktcode.into());
    // zugeordneter Marktpartner: reuse the Z91/ZF0 13-digit resolver on the group.
    if let Some(mp) = extract_zugeordneter_msb(group) {
        kp.insert(
            "marktpartner".into(),
            serde_json::json!({ "_typ": "MARKTTEILNEHMER", "rollencodenummer": mp }),
        );
    }
    // Leistungskurvendefinition: the Produkteigenschaft value carried in a
    // CAV+ZH9 (Code der Produkteigenschaft) or CAV+ZV4 (Wertedetails).
    if let Some(lk) = group
        .iter()
        .filter(|s| s.tag == "CAV")
        .filter(|s| matches!(s.component_str(0, 0), Some("ZH9") | Some("ZV4")))
        .find_map(|s| {
            (1..6)
                .filter_map(|c| s.component_str(0, c))
                .find(|v| !v.is_empty())
        })
    {
        kp.insert("leistungskurvendefinition".into(), lk.into());
    }
    Some(serde_json::Value::Object(kp))
}

/// Extract the **Bilanzierungsgebiet** EIC from a UTILMD segment list.
///
/// The Bilanzierungsgebiet travels in `LOC+237` (DE3227 = `237`, DE3225 =
/// the EIC). Best-effort: `None` when the segment is absent.
pub fn extract_bilanzierungsgebiet(segs: &[OwnedSegment]) -> Option<String> {
    segs.iter()
        .find(|s| s.tag == "LOC" && s.element_str(0).is_some_and(|q| q == "237"))
        .and_then(|s| s.element_str(1))
        .map(str::to_owned)
}

/// Read the first `CAV` value that immediately follows a `CCI` segment whose
/// class code (any element) equals `cci_class`.
///
/// UTILMD SG8/SG10 characteristic groups are ordered `CCI` then one or more
/// `CAV`; this pairs them positionally, which is robust across AHB revisions
/// that shuffle the CCI component layout.
fn extract_cav_after_cci(segs: &[OwnedSegment], cci_class: &str) -> Option<String> {
    let mut it = segs.iter().peekable();
    while let Some(s) = it.next() {
        let is_target_cci = s.tag == "CCI"
            && (0..4)
                .filter_map(|i| s.element_str(i))
                .any(|v| v == cci_class);
        if is_target_cci {
            while let Some(next) = it.peek() {
                if next.tag == "CAV" {
                    return next.element_str(0).map(str::to_owned);
                }
                if next.tag == "CCI" {
                    break; // next characteristic group — no CAV for this CCI
                }
                it.next();
            }
        }
    }
    None
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

#[cfg(test)]
mod fernsteuerbarkeit_tests {
    use super::*;
    use edifact_rs::OwnedElement;

    fn seg(tag: &str, elements: Vec<Vec<&str>>) -> OwnedSegment {
        OwnedSegment::new(
            tag,
            elements
                .into_iter()
                .map(|comps| OwnedElement::of(&comps))
                .collect(),
        )
    }

    #[test]
    fn extracts_the_14a_fernsteuerbarkeit_status() {
        // SG10 CCI+7059=Z24 (Status der Fernsteuerbarkeit) with 7037=Z97/Z96.
        let fernsteuerbar = vec![seg("CCI", vec![vec!["Z24"], vec!["Z97"]])];
        assert_eq!(extract_fernsteuerbarkeit(&fernsteuerbar), Some(true));

        let nicht = vec![seg("CCI", vec![vec!["Z24"], vec!["Z96"]])];
        assert_eq!(extract_fernsteuerbarkeit(&nicht), Some(false));

        // Absent characteristic → None (COALESCE leaves the column unchanged).
        let other = vec![seg("CCI", vec![vec!["Z50"]])];
        assert_eq!(extract_fernsteuerbarkeit(&other), None);
    }

    #[test]
    fn extracts_the_14a_steuerkanal() {
        // SG10 CCI+7059=Z49 (Steuerkanal) with 7037=ZF3/ZF2.
        assert_eq!(
            extract_steuerkanal(&[seg("CCI", vec![vec!["Z49"], vec!["ZF3"]])]),
            Some(true)
        );
        assert_eq!(
            extract_steuerkanal(&[seg("CCI", vec![vec!["Z49"], vec!["ZF2"]])]),
            Some(false)
        );
        assert_eq!(extract_steuerkanal(&[seg("CCI", vec![vec!["Z18"]])]), None);
    }

    #[test]
    fn extracts_the_tr_fernschaltbarkeit() {
        // SG10 CAV carrying the Z58 Fernschaltung class + Z06/Z07 value.
        assert_eq!(
            extract_ist_fernschaltbar(&[seg("CAV", vec![vec!["Z06"], vec!["Z58"]])]),
            Some(true)
        );
        assert_eq!(
            extract_ist_fernschaltbar(&[seg("CAV", vec![vec!["Z07"], vec!["Z58"]])]),
            Some(false)
        );
        // A generic Z06 without the Z58 Fernschaltung class is not misread.
        assert_eq!(
            extract_ist_fernschaltbar(&[seg("CAV", vec![vec!["Z06"], vec!["ZXX"]])]),
            None
        );
    }

    #[test]
    fn extracts_the_tr_nutzung() {
        // SG10 CCI+7059 = Z17/Z50/Z56 → BO4E TechnischeRessourceNutzung.
        assert_eq!(
            extract_tr_nutzung(&[seg("CCI", vec![vec!["Z17"]])]),
            Some("STROMVERBRAUCHSART")
        );
        assert_eq!(
            extract_tr_nutzung(&[seg("CCI", vec![vec!["Z50"]])]),
            Some("STROMERZEUGUNGSART")
        );
        assert_eq!(
            extract_tr_nutzung(&[seg("CCI", vec![vec!["Z56"]])]),
            Some("SPEICHER")
        );
        // The MaLo „technische Einrichtung" Z69 is NOT a TR nutzung code.
        assert_eq!(extract_tr_nutzung(&[seg("CCI", vec![vec!["Z69"]])]), None);
    }

    #[test]
    fn extracts_the_tr_verbrauchsart() {
        // SG10 CAV+7111 = Z64/Z65/ZE5/ZA8 → BO4E TechnischeRessourceVerbrauchsart.
        assert_eq!(
            extract_tr_verbrauchsart(&[seg("CAV", vec![vec!["Z64"]])]),
            Some("KRAFT_LICHT")
        );
        assert_eq!(
            extract_tr_verbrauchsart(&[seg("CAV", vec![vec!["ZE5"]])]),
            Some("E_MOBILITAET")
        );
        assert_eq!(
            extract_tr_verbrauchsart(&[seg("CAV", vec![vec!["ZA8"]])]),
            Some("STRASSENBELEUCHTUNG")
        );
        assert_eq!(
            extract_tr_verbrauchsart(&[seg("CAV", vec![vec!["ZXX"]])]),
            None
        );
    }

    #[test]
    fn extracts_sr_konfigurationsprodukte_per_product() {
        let segs = vec![
            // Product A — SEQ+Z79 group.
            seg("SEQ", vec![vec!["Z79"]]),
            seg("PIA", vec![vec!["5"], vec!["PRODUKT_A"]]),
            seg("CCI", vec![vec!["Z66"]]),
            seg("CAV", vec![vec!["ZH9", "LK001"]]),
            seg("CAV", vec![vec!["Z91", "9900123456789"]]),
            // Product B — SEQ+Z79 group with a gMSB (ZF0).
            seg("SEQ", vec![vec!["Z79"]]),
            seg("PIA", vec![vec!["5"], vec!["PRODUKT_B"]]),
            seg("CAV", vec![vec!["ZF0", "9988776655443"]]),
            // A non-Z79 SEQ group is not a konfigurationsprodukt.
            seg("SEQ", vec![vec!["Z01"]]),
            seg("PIA", vec![vec!["5"], vec!["IGNORE_ME"]]),
        ];
        let kp = extract_sr_konfigurationsprodukte(&segs).expect("products");
        let arr = kp.as_array().unwrap();
        assert_eq!(arr.len(), 2, "only the two SEQ+Z79 groups are products");
        assert_eq!(arr[0]["produktcode"], "PRODUKT_A");
        assert_eq!(arr[0]["marktpartner"]["rollencodenummer"], "9900123456789");
        assert_eq!(arr[0]["leistungskurvendefinition"], "LK001");
        assert_eq!(arr[1]["produktcode"], "PRODUKT_B");
        assert_eq!(arr[1]["marktpartner"]["rollencodenummer"], "9988776655443");
        // No SEQ+Z79 group → None (COALESCE leaves the column unchanged).
        assert!(extract_sr_konfigurationsprodukte(&[seg("SEQ", vec![vec!["Z01"]])]).is_none());
    }

    #[test]
    fn extracts_the_zugeordneter_msb() {
        // SG10 CAV C889: comp0=Z91 (MSB), comp1=MP-ID (13-digit), comp3=Z39.
        let serving = vec![seg("CAV", vec![vec!["Z91", "9900123456789", "", "Z39"]])];
        assert_eq!(
            extract_zugeordneter_msb(&serving).as_deref(),
            Some("9900123456789")
        );

        // Falls back to the ZF0 gMSB when no Z91 serving MSB is present.
        let gmsb = vec![seg("CAV", vec![vec!["ZF0", "9988776655443"]])];
        assert_eq!(
            extract_zugeordneter_msb(&gmsb).as_deref(),
            Some("9988776655443")
        );

        // Z91 is preferred over ZF0 when both are present.
        let both = vec![
            seg("CAV", vec![vec!["ZF0", "9988776655443"]]),
            seg("CAV", vec![vec!["Z91", "9900123456789", "", "Z19"]]),
        ];
        assert_eq!(
            extract_zugeordneter_msb(&both).as_deref(),
            Some("9900123456789")
        );

        // No MSB CAV → None.
        assert_eq!(
            extract_zugeordneter_msb(&[seg("CAV", vec![vec!["Z28"]])]),
            None
        );
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod coverage_table_tests {
    /// Every `pub fn …_registry` in this module must appear in the coverage
    /// table, so startup validation cannot silently skip one.
    ///
    /// This is the guard for a gap that was live: `startup.rs` carried a
    /// hand-written list of registries to validate, and twenty registries —
    /// among them both Wertebestellung families, the GPKE EoG workflow, the
    /// WiM Technik-Änderung, and every REMADV/COMDIS resume path — had simply
    /// never been added to it.
    #[test]
    fn every_registry_is_in_the_coverage_table() {
        const SOURCES: &[(&str, &str)] = &[
            ("gabi_gas.rs", include_str!("gabi_gas.rs")),
            ("geli_gas.rs", include_str!("geli_gas.rs")),
            ("gpke.rs", include_str!("gpke.rs")),
            ("mabis.rs", include_str!("mabis.rs")),
            ("wim.rs", include_str!("wim.rs")),
            ("wim_gas.rs", include_str!("wim_gas.rs")),
        ];
        let table: std::collections::HashSet<&str> =
            super::coverage().iter().map(|c| c.registry).collect();

        let mut missing: Vec<String> = Vec::new();
        for (file, src) in SOURCES {
            for line in src.lines() {
                let Some(rest) = line.strip_prefix("pub fn ") else {
                    continue;
                };
                let Some((name, _)) = rest.split_once('(') else {
                    continue;
                };
                if name.ends_with("_registry") && !table.contains(name) {
                    missing.push(format!("{file}: {name}"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "these adapter registries are not in the coverage table and are \
             therefore never validated at startup:\n  {}\n\
             Add each name to the `coverage_table!` invocation in adapters/mod.rs.",
            missing.join("\n  ")
        );
    }

    /// Coverage is meaningless without format versions to cover: an empty
    /// `known_fvs()` makes every registry vacuously complete while no message
    /// would parse at all.
    #[test]
    fn the_known_format_versions_are_not_empty() {
        assert!(
            !super::known_fvs().is_empty(),
            "no BDEW format version is registered in the compiled edi-energy \
             profile registry — every adapter would reject every message"
        );
    }
}
