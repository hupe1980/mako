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

/// Map a `QualityFlag` onto the BO4E `Messwertstatus` that says the same thing.
///
/// BO4E's vocabulary is narrower than `metering`'s, so the mapping is lossy —
/// but never in a direction that asserts something false:
///
/// | Flag | Status | Why that one |
/// |---|---|---|
/// | `Measured`, `Corrected` | `Abgelesen` | BO4E carries no "corrected" status; in the market a correction rides on the MSCONS *version*, and the value itself was read off the meter. `Vorlaeufigerwert` would say "subject to revision", and a correction *is* the revision. |
/// | `Estimated` | `Prognosewert` | A forecast. |
/// | `Preliminary` | `Vorlaeufigerwert` | A measurement subject to revision, which is not a forecast. |
/// | `Substituted` | `Ersatzwert` | — |
/// | `Calculated` | `Energiemengesummiert` | Derived from other readings (Residuallast = Bezug − Einspeisung), not provisional. |
/// | `Faulty` | `NichtVerwendbar` | The one status that says "do not bill this". `Unknown` is BO4E's forward-compatibility catch-all and says nothing about the reading. |
/// | `Unknown` | `Unknown` | The quality is not known — the one case where the catch-all is honest. |
pub(crate) fn quality_to_messwertstatus(q: QualityFlag) -> Messwertstatus {
    match q {
        QualityFlag::Measured | QualityFlag::Corrected => Messwertstatus::Abgelesen,
        QualityFlag::Estimated => Messwertstatus::Prognosewert,
        QualityFlag::Substituted => Messwertstatus::Ersatzwert,
        QualityFlag::Calculated => Messwertstatus::Energiemengesummiert,
        QualityFlag::Preliminary => Messwertstatus::Vorlaeufigerwert,
        QualityFlag::Faulty => Messwertstatus::NichtVerwendbar,
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
///
/// The `wert` counts the `einheit`: a quarter-hour is **one** `ViertelStunde`, as
/// an hour is one `Stunde` and a day is one `Tag`. Fifteen `ViertelStunde` would
/// be three and three-quarter hours.
pub(crate) fn minutes_to_menge(minutes: u32) -> Menge {
    let (wert, einheit) = match minutes {
        15 => (Decimal::ONE, Mengeneinheit::ViertelStunde),
        60 => (Decimal::ONE, Mengeneinheit::Stunde),
        1440 => (Decimal::ONE, Mengeneinheit::Tag),
        m => (Decimal::from(m), Mengeneinheit::Minute),
    };
    Menge {
        wert: Some(wert),
        einheit: Some(einheit),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two mappings a consumer acts on: "do not bill this" and "this may
    /// still be revised".
    #[test]
    fn the_unbillable_and_the_provisional_statuses_are_the_bo4e_ones() {
        assert_eq!(
            quality_to_messwertstatus(QualityFlag::Faulty),
            Messwertstatus::NichtVerwendbar,
            "`Unknown` is the forward-compat catch-all, not a statement about the reading"
        );
        assert_eq!(
            quality_to_messwertstatus(QualityFlag::Preliminary),
            Messwertstatus::Vorlaeufigerwert,
            "a vorläufiger Wert is a measurement, not a Prognose"
        );
        assert_eq!(
            quality_to_messwertstatus(QualityFlag::Corrected),
            Messwertstatus::Abgelesen,
            "a correction is the revision, so it cannot be `subject to revision`"
        );
        // `Unknown` is the one case where the catch-all is honest.
        assert_eq!(
            quality_to_messwertstatus(QualityFlag::Unknown),
            Messwertstatus::Unknown
        );
    }

    /// A quarter-hour is one `ViertelStunde`, not fifteen of them.
    #[test]
    fn an_interval_length_counts_its_own_unit() {
        let m = minutes_to_menge(15);
        assert_eq!(m.wert, Some(Decimal::ONE));
        assert_eq!(m.einheit, Some(Mengeneinheit::ViertelStunde));

        let hour = minutes_to_menge(60);
        assert_eq!(hour.wert, Some(Decimal::ONE));
        assert_eq!(hour.einheit, Some(Mengeneinheit::Stunde));

        // Anything the enum has no unit for falls back to counting minutes.
        let odd = minutes_to_menge(7);
        assert_eq!(odd.wert, Some(Decimal::from(7u32)));
        assert_eq!(odd.einheit, Some(Mengeneinheit::Minute));
    }
}
