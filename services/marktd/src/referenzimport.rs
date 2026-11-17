//! Read a publisher's file into Referenzwerte rows — pure, no I/O.
//!
//! Each reader turns one native format into [`ImportZeile`]s in the series'
//! catalogued unit, with the valid period as the half-open instant range the
//! store keeps. A file the reader cannot read whole is refused whole: a
//! partial import is a series with a hole nobody sees.
//!
//! | `format` | Publisher | Series |
//! |---|---|---|
//! | `generisch` | anyone — `zeitraum;dim;wert` | every series |
//! | `smard` | Bundesnetzagentur SMARD `chart_data` JSON (`series: [[ms, EUR/MWh]]`) | `day_ahead_de_lu` |
//! | `energy_charts` | energy-charts `/price` JSON (`unix_seconds`, `price`, `unit`) | `day_ahead_de_lu` |
//!
//! The netztransparenz Marktwert CSV and the Destatis GENESIS table have no
//! reader yet: no sample of either is mirrored in the repository, and a reader
//! written against a remembered layout is a guess at a column order.

use std::collections::BTreeSet;
use std::str::FromStr as _;

use mako_markt::referenzreihen::{Granularitaet, Reihe, Zeitraum};
use rust_decimal::Decimal;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// One value read from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportZeile {
    /// The dimension value, `""` for none.
    pub dim: String,
    /// Start of the valid period (inclusive).
    pub gilt_von: OffsetDateTime,
    /// End of the valid period (exclusive).
    pub gilt_bis: OffsetDateTime,
    /// The value, in the series' catalogued unit.
    pub wert: Decimal,
}

/// A native format the import reads.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// `zeitraum;dim;wert`, one header row.
    Generisch,
    /// SMARD `chart_data` JSON.
    Smard,
    /// energy-charts `/price` JSON.
    EnergyCharts,
}

impl Format {
    /// The `format` column value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generisch => "generisch",
            Self::Smard => "smard",
            Self::EnergyCharts => "energy_charts",
        }
    }
}

/// Why a file was not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Unlesbar(pub String);

fn fehler(msg: impl Into<String>) -> Unlesbar {
    Unlesbar(msg.into())
}

/// Read `body` as `format` for `reihe`.
///
/// # Errors
/// A format that does not carry this series, a malformed row, a dimension the
/// catalogue does not admit, two rows for one period, or an empty file.
pub fn lies(reihe: Reihe, format: Format, body: &[u8]) -> Result<Vec<ImportZeile>, Unlesbar> {
    let zeilen = match format {
        Format::Generisch => generisch(reihe, body)?,
        Format::Smard | Format::EnergyCharts if reihe != Reihe::DayAheadDeLu => {
            return Err(fehler(format!(
                "das Format {} trägt nur day_ahead_de_lu, nicht {reihe}",
                format.as_str()
            )));
        }
        Format::Smard => smard(body)?,
        Format::EnergyCharts => energy_charts(body)?,
    };
    if zeilen.is_empty() {
        return Err(fehler("die Datei enthält keinen Wert"));
    }
    let mut seen = BTreeSet::new();
    for z in &zeilen {
        if !reihe.admits_dim(&z.dim) {
            return Err(fehler(format!(
                "die Dimension {:?} gibt es für {reihe} nicht (zulässig: {:?})",
                z.dim,
                reihe.info().dimensionen
            )));
        }
        if !seen.insert((z.dim.clone(), z.gilt_von)) {
            return Err(fehler(format!(
                "die Datei nennt den Zeitraum ab {} ({:?}) zweimal",
                z.gilt_von, z.dim
            )));
        }
    }
    Ok(zeilen)
}

/// A decimal in either German (`17,5`) or point (`17.5`) notation; a value
/// carrying both marks is refused rather than guessed at.
fn dezimal(raw: &str) -> Result<Decimal, Unlesbar> {
    let t = raw.trim();
    if t.contains(',') && t.contains('.') {
        return Err(fehler(format!(
            "{t:?} trägt Komma und Punkt — Tausendertrennzeichen werden nicht geraten"
        )));
    }
    Decimal::from_str(&t.replace(',', ".")).map_err(|_| fehler(format!("{t:?} ist keine Zahl")))
}

/// The generic file: a header `zeitraum;dim;wert` and one value per row.
///
/// `zeitraum` is `YYYY` for an annual series, `YYYY-MM` for a monthly one and
/// the RFC 3339 start of the quarter hour for `day_ahead_de_lu`.
fn generisch(reihe: Reihe, body: &[u8]) -> Result<Vec<ImportZeile>, Unlesbar> {
    let text = std::str::from_utf8(body).map_err(|_| fehler("die Datei ist kein UTF-8"))?;
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .ok_or_else(|| fehler("die Datei ist leer"))?
        .split(';')
        .map(|h| h.trim().to_lowercase())
        .collect();
    if header != ["zeitraum", "dim", "wert"] {
        return Err(fehler(format!(
            "die Kopfzeile muss zeitraum;dim;wert lauten, nicht {header:?}"
        )));
    }
    let granularitaet = reihe.info().granularitaet;
    let mut out = Vec::new();
    for (n, line) in lines.enumerate() {
        let f: Vec<&str> = line.split(';').map(str::trim).collect();
        let [zeitraum, dim, wert] = f[..] else {
            return Err(fehler(format!(
                "Zeile {}: drei Felder erwartet, {} gelesen",
                n + 2,
                f.len()
            )));
        };
        let (gilt_von, gilt_bis) = periode(granularitaet, zeitraum)
            .map_err(|e| fehler(format!("Zeile {}: {}", n + 2, e.0)))?;
        out.push(ImportZeile {
            dim: dim.to_owned(),
            gilt_von,
            gilt_bis,
            wert: dezimal(wert).map_err(|e| fehler(format!("Zeile {}: {}", n + 2, e.0)))?,
        });
    }
    Ok(out)
}

/// The valid range of a period as the generic file states it.
fn periode(
    granularitaet: Granularitaet,
    raw: &str,
) -> Result<(OffsetDateTime, OffsetDateTime), Unlesbar> {
    let zahl = |s: &str| {
        s.parse::<i32>()
            .map_err(|_| fehler(format!("{raw:?} ist kein Zeitraum")))
    };
    let zeitraum = match granularitaet {
        Granularitaet::Kalenderjahr => Zeitraum::Jahr { jahr: zahl(raw)? },
        Granularitaet::Kalendermonat => {
            let (j, m) = raw
                .split_once('-')
                .ok_or_else(|| fehler(format!("{raw:?} ist kein Monat (YYYY-MM)")))?;
            Zeitraum::Monat {
                jahr: zahl(j)?,
                monat: u8::try_from(zahl(m)?).map_err(|_| fehler(format!("{raw:?}")))?,
            }
        }
        Granularitaet::Viertelstunde => {
            let von = OffsetDateTime::parse(raw, &Rfc3339)
                .map_err(|_| fehler(format!("{raw:?} ist kein RFC-3339-Zeitpunkt")))?;
            return viertelstunde(von);
        }
    };
    zeitraum.instants().map_err(|e| fehler(e.to_string()))
}

/// A quarter hour starting at `von`, which must sit on a quarter-hour boundary.
fn viertelstunde(von: OffsetDateTime) -> Result<(OffsetDateTime, OffsetDateTime), Unlesbar> {
    if !von.minute().is_multiple_of(15) || von.second() != 0 || von.nanosecond() != 0 {
        return Err(fehler(format!("{von} beginnt keine Viertelstunde")));
    }
    Ok((von, von + time::Duration::minutes(15)))
}

/// EUR/MWh → ct/kWh: 1 EUR/MWh is 0,1 ct/kWh, exactly.
fn eur_mwh_in_ct_kwh(v: Decimal) -> Decimal {
    v / Decimal::TEN
}

/// A JSON number as a `Decimal`, read from its own text so no float rounds it.
fn json_dezimal(v: &serde_json::Value) -> Result<Option<Decimal>, Unlesbar> {
    match v {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => Decimal::from_str(&n.to_string())
            .or_else(|_| Decimal::from_scientific(&n.to_string()))
            .map(Some)
            .map_err(|_| fehler(format!("{n} ist keine Dezimalzahl"))),
        other => Err(fehler(format!("{other} ist keine Zahl"))),
    }
}

/// SMARD `chart_data`: `{"series": [[epoch_ms, EUR/MWh | null], …]}` per
/// quarter hour. A `null` is a quarter hour SMARD has no price for, and is
/// skipped rather than stored as zero.
fn smard(body: &[u8]) -> Result<Vec<ImportZeile>, Unlesbar> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| fehler(format!("kein JSON: {e}")))?;
    let series = v["series"]
        .as_array()
        .ok_or_else(|| fehler("SMARD-Datei ohne `series`"))?;
    let mut out = Vec::new();
    for pair in series {
        let (Some(ms), Some(preis)) =
            (pair.get(0).and_then(serde_json::Value::as_i64), pair.get(1))
        else {
            return Err(fehler(format!("SMARD-Eintrag {pair} ist kein [ms, Wert]")));
        };
        let Some(eur) = json_dezimal(preis)? else {
            continue;
        };
        let von = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
            .map_err(|_| fehler(format!("{ms} ist kein Zeitpunkt")))?;
        let (gilt_von, gilt_bis) = viertelstunde(von)?;
        out.push(ImportZeile {
            dim: String::new(),
            gilt_von,
            gilt_bis,
            wert: eur_mwh_in_ct_kwh(eur),
        });
    }
    Ok(out)
}

/// energy-charts `/price`: parallel `unix_seconds` and `price` arrays in the
/// `unit` it names, which must be `EUR / MWh`.
fn energy_charts(body: &[u8]) -> Result<Vec<ImportZeile>, Unlesbar> {
    let v: serde_json::Value =
        serde_json::from_slice(body).map_err(|e| fehler(format!("kein JSON: {e}")))?;
    let unit = v["unit"].as_str().unwrap_or_default();
    if unit.replace(' ', "") != "EUR/MWh" {
        return Err(fehler(format!(
            "energy-charts-Einheit {unit:?} — gelesen wird nur EUR / MWh"
        )));
    }
    let (Some(ts), Some(preise)) = (v["unix_seconds"].as_array(), v["price"].as_array()) else {
        return Err(fehler(
            "energy-charts-Datei ohne `unix_seconds` und `price`",
        ));
    };
    if ts.len() != preise.len() {
        return Err(fehler("`unix_seconds` und `price` sind ungleich lang"));
    }
    let mut out = Vec::new();
    for (t, p) in ts.iter().zip(preise) {
        let secs = t
            .as_i64()
            .ok_or_else(|| fehler(format!("{t} ist kein Zeitpunkt")))?;
        let Some(eur) = json_dezimal(p)? else {
            continue;
        };
        let von = OffsetDateTime::from_unix_timestamp(secs)
            .map_err(|_| fehler(format!("{secs} ist kein Zeitpunkt")))?;
        let (gilt_von, gilt_bis) = viertelstunde(von)?;
        out.push(ImportZeile {
            dim: String::new(),
            gilt_von,
            gilt_bis,
            wert: eur_mwh_in_ct_kwh(eur),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::datetime;

    /// Synthetic values only: the repository ships no EPEX figure.
    #[test]
    fn smard_reads_quarter_hours_in_ct_per_kwh_and_skips_gaps() {
        let body = br#"{"meta_data":{"version":1},"series":[[1793574000000,100.5],[1793574900000,null],[1793575800000,-12.34]]}"#;
        let z = lies(Reihe::DayAheadDeLu, Format::Smard, body).expect("reads");
        assert_eq!(z.len(), 2, "the null quarter hour is no value");
        assert_eq!(z[0].wert, dec!(10.05));
        assert_eq!(z[0].gilt_bis - z[0].gilt_von, time::Duration::minutes(15));
        assert_eq!(z[1].wert, dec!(-1.234));
    }

    #[test]
    fn energy_charts_reads_its_parallel_arrays_and_checks_the_unit() {
        let body =
            br#"{"unix_seconds":[1790805600,1790806500],"price":[100.0,50.25],"unit":"EUR / MWh"}"#;
        let z = lies(Reihe::DayAheadDeLu, Format::EnergyCharts, body).expect("reads");
        assert_eq!(z[1].wert, dec!(5.025));
        assert_eq!(z[0].gilt_von, datetime!(2026-09-30 22:00 UTC));
        let wrong = br#"{"unix_seconds":[1790805600],"price":[1],"unit":"ct / kWh"}"#;
        assert!(lies(Reihe::DayAheadDeLu, Format::EnergyCharts, wrong).is_err());
    }

    #[test]
    fn the_generic_file_reads_years_between_berlin_midnights() {
        let body = "zeitraum;dim;wert\n2025;;17,5\n2024;;16.25\n".as_bytes();
        let z = lies(Reihe::KavGrenzpreisStrom, Format::Generisch, body).expect("reads");
        assert_eq!(z[0].wert, dec!(17.5));
        assert_eq!(z[0].gilt_von, datetime!(2024-12-31 23:00 UTC));
        assert_eq!(z[0].gilt_bis, datetime!(2025-12-31 23:00 UTC));
        let monat = "zeitraum;dim;wert\n2026-07;solar;4,1\n".as_bytes();
        let z = lies(Reihe::EegMonatsmarktwert, Format::Generisch, monat).expect("reads");
        assert_eq!(z[0].gilt_von, datetime!(2026-06-30 22:00 UTC));
    }

    #[test]
    fn a_file_the_catalogue_contradicts_is_refused_whole() {
        let refused = |reihe, f, b: &str| lies(reihe, f, b.as_bytes()).is_err();
        // A dimension the series does not have.
        assert!(refused(
            Reihe::EegMonatsmarktwert,
            Format::Generisch,
            "zeitraum;dim;wert\n2026-07;biogas;4\n"
        ));
        // A period twice.
        assert!(refused(
            Reihe::KavGrenzpreisStrom,
            Format::Generisch,
            "zeitraum;dim;wert\n2025;;1\n2025;;2\n"
        ));
        // Both decimal marks.
        assert!(refused(
            Reihe::KavGrenzpreisStrom,
            Format::Generisch,
            "zeitraum;dim;wert\n2025;;1.234,5\n"
        ));
        // A day-ahead format for an annual series.
        assert!(refused(
            Reihe::KavGrenzpreisStrom,
            Format::Smard,
            r#"{"series":[[1793574000000,1]]}"#
        ));
        // No value at all.
        assert!(refused(
            Reihe::KavGrenzpreisStrom,
            Format::Generisch,
            "zeitraum;dim;wert\n"
        ));
        // Off a quarter-hour boundary.
        assert!(refused(
            Reihe::DayAheadDeLu,
            Format::Generisch,
            "zeitraum;dim;wert\n2026-10-01T00:07:00+02:00;;1\n"
        ));
    }
}
