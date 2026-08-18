//! Conversions from `edmd`'s domain types (`MeterRead`, `Sparte`, `QualityFlag`)
//! to their BO4E representations for API responses and Zeitreihen export.

#[allow(unused_imports)]
use super::*;

/// Convert an `edm::Sparte` to the BO4E `Sparte` enum.
pub(crate) fn edm_sparte_to_bo4e(s: EdmSparte) -> Bo4eSparte {
    match s {
        EdmSparte::Strom => Bo4eSparte::Strom,
        EdmSparte::Gas => Bo4eSparte::Gas,
        // BO4E splits heat into Fern-/Nahwärme; `edmd` does not carry that
        // distinction, and Fernwaerme is the billing-relevant default.
        EdmSparte::Waerme => Bo4eSparte::Fernwaerme,
        EdmSparte::Wasser => Bo4eSparte::Wasser,
    }
}

/// Map `edm::Sparte` to the BO4E `Medium` enum for `Zeitreihe`.
pub(crate) fn edm_sparte_to_medium(s: EdmSparte) -> Medium {
    match s {
        EdmSparte::Strom => Medium::Strom,
        EdmSparte::Gas => Medium::Gas,
        EdmSparte::Wasser => Medium::Wasser,
        // BO4E `Medium` has no heat variant (STROM/GAS/WASSER/DAMPF only), so a
        // Wärmemengenzähler has no faithful mapping. `Dampf` would be wrong —
        // district heat is hot water, not steam — so this reports Unknown rather
        // than asserting something false in an exported Zeitreihe.
        EdmSparte::Waerme => Medium::Unknown,
    }
}

/// The `Mengeneinheit` a stored quantity of this Sparte is expressed in.
///
/// Follows the storage convention (`store::stored_unit`): every reading is held
/// in its Sparte's **billing** unit. Gas registers m³ but is converted to kWh_Hs
/// at ingest (§ 25 Nr. 4 MessEV), so only water — whose measured and billed unit
/// are both m³ — is exported as a volume. Declaring gas as `Kubikmeter` here
/// understated an exported Energiemenge by the Brennwert factor, roughly
/// tenfold.
pub(crate) fn edm_sparte_to_einheit(s: EdmSparte) -> Mengeneinheit {
    match s {
        EdmSparte::Wasser => Mengeneinheit::Kubikmeter,
        EdmSparte::Strom | EdmSparte::Gas | EdmSparte::Waerme => Mengeneinheit::Kwh,
    }
}

/// Map a `QualityFlag` to the nearest `Messwertstatus` variant.
pub(crate) fn quality_to_messwertstatus(q: QualityFlag) -> Messwertstatus {
    match q {
        QualityFlag::Measured => Messwertstatus::Abgelesen,
        QualityFlag::Estimated => Messwertstatus::Prognosewert,
        QualityFlag::Substituted => Messwertstatus::Ersatzwert,
        QualityFlag::Calculated => Messwertstatus::Vorlaeufigerwert,
        QualityFlag::Corrected => Messwertstatus::Vorlaeufigerwert,
        QualityFlag::Preliminary => Messwertstatus::Prognosewert,
        QualityFlag::Faulty => Messwertstatus::Unknown,
        QualityFlag::Unknown => Messwertstatus::Unknown,
    }
}

/// Convert a `MeterRead` to a BO4E `Energiemenge`.
///
/// `Energiemenge` is the canonical BO4E Business Object for a metered energy
/// quantity at a location.  It carries the OBIS-Kennzahl, the measured `Menge`
/// in kWh, and the billing `Zeitraum` — exactly the triple that MSCONS
/// communicates per register per interval.
///
/// All timestamps are UTC (`startuhrzeit`/`enduhrzeit` format `HH:MM:SS+00:00`).
pub(crate) fn read_to_energiemenge(r: &crate::domain::MeterRead) -> Energiemenge {
    fn fmt_uhrzeit(dt: OffsetDateTime) -> String {
        format!(
            "{:02}:{:02}:{:02}+00:00",
            dt.hour(),
            dt.minute(),
            dt.second()
        )
    }
    Energiemenge {
        obis_kennzahl: r.obis_code.as_deref().and_then(|s| ObisCode::new(s).ok()),
        menge: Some(Menge {
            wert: Some(r.quantity_kwh),
            einheit: Some(edm_sparte_to_einheit(r.sparte)),
            ..Default::default()
        }),
        zeitraum: Some(Zeitraum {
            startdatum: Some(r.dtm_from.date()),
            startuhrzeit: Some(fmt_uhrzeit(r.dtm_from)),
            enddatum: Some(r.dtm_to.date()),
            enduhrzeit: Some(fmt_uhrzeit(r.dtm_to)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Convert a `MeterRead` to a BO4E `Zeitreihenwert`.
///
/// Timestamps are in UTC; `startuhrzeit`/`enduhrzeit` are formatted as
/// `HH:MM:SS+00:00` per Allgemeine Festlegungen V6.1d §3.
pub(crate) fn read_to_zeitreihenwert(r: &crate::domain::MeterRead) -> Zeitreihenwert {
    fn fmt_uhrzeit(dt: OffsetDateTime) -> String {
        format!(
            "{:02}:{:02}:{:02}+00:00",
            dt.hour(),
            dt.minute(),
            dt.second()
        )
    }
    Zeitreihenwert {
        wert: Some(r.quantity_kwh),
        status: Some(quality_to_messwertstatus(r.quality)),
        zeitraum: Some(Zeitraum {
            startdatum: Some(r.dtm_from.date()),
            startuhrzeit: Some(fmt_uhrzeit(r.dtm_from)),
            enddatum: Some(r.dtm_to.date()),
            enduhrzeit: Some(fmt_uhrzeit(r.dtm_to)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a `Menge` representing an interval length from whole minutes.
pub(crate) fn minutes_to_menge(minutes: u32) -> Menge {
    let (wert, einheit) = match minutes {
        15 => (Decimal::from(15u32), Mengeneinheit::ViertelStunde),
        60 => (Decimal::from(1u32), Mengeneinheit::Stunde),
        1440 => (Decimal::from(1u32), Mengeneinheit::Tag),
        m => (Decimal::from(m), Mengeneinheit::Minute),
    };
    Menge {
        wert: Some(wert),
        einheit: Some(einheit),
        ..Default::default()
    }
}
