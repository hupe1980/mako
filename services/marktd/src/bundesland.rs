//! The Bundesland of a Marktlokation, from the Postleitzahl of its address.
//!
//! An Ersatzwert compares a gap with days of its day class, and a public
//! holiday counts as a Sunday — in the Land whose holiday it is. `edmd` holds
//! no address, so it asks here once and keeps the answer.
//!
//! # The source
//!
//! The GeoNames postal code file for Germany
//! (<https://download.geonames.org/export/zip/DE.zip>), which its
//! `readme.txt` licenses under Creative Commons Attribution 4.0. Each place carries
//! its PLZ and the ISO 3166-2:DE code of its Land (`admin code1`).
//! `data/plz_bundesland.tsv` is that file folded to one line per PLZ with
//! every Land a place of it lies in.
//!
//! A PLZ is a delivery area, not a territory, and a few cross a Land border:
//! such a PLZ names more than one Land and decides none, so the answer is a
//! refusal by name — never the first Land listed. The `readme.txt` disclaims
//! any warranty of accuracy or completeness, which is why a Land it names is
//! served with its source and an operator can still name the Land per request.

use std::collections::HashMap;
use std::sync::LazyLock;

use metering::time::holiday::Bundesland;

/// The attribution the licence asks for, served with every answer.
pub const QUELLE: &str = "GeoNames postal codes DE (https://www.geonames.org), CC BY 4.0";

/// What the PLZ table says about one Postleitzahl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlzBefund {
    /// Every place of the PLZ lies in one Land.
    Eindeutig(Bundesland),
    /// The PLZ's places lie in more than one Land.
    Mehrdeutig(Vec<Bundesland>),
    /// The table holds no such PLZ.
    Unbekannt,
}

static TABELLE: LazyLock<HashMap<&'static str, Vec<Bundesland>>> = LazyLock::new(|| {
    include_str!("../data/plz_bundesland.tsv")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .filter_map(|l| {
            let (plz, laender) = l.split_once('\t')?;
            let laender: Vec<Bundesland> =
                laender.split(',').filter_map(|c| c.parse().ok()).collect();
            (!laender.is_empty()).then_some((plz, laender))
        })
        .collect()
});

/// The Land of a Postleitzahl.
#[must_use]
pub fn der_plz(plz: &str) -> PlzBefund {
    match TABELLE.get(plz.trim()).map(Vec::as_slice) {
        Some([eins]) => PlzBefund::Eindeutig(*eins),
        Some(mehrere) => PlzBefund::Mehrdeutig(mehrere.to_vec()),
        None => PlzBefund::Unbekannt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plz_inside_one_land_names_it() {
        assert_eq!(der_plz("80331"), PlzBefund::Eindeutig(Bundesland::By));
        assert_eq!(der_plz("01067"), PlzBefund::Eindeutig(Bundesland::Sn));
        assert_eq!(der_plz("50667"), PlzBefund::Eindeutig(Bundesland::Nw));
    }

    /// A PLZ across a Land border decides no Land: neither is taken.
    #[test]
    fn a_plz_across_a_border_decides_none() {
        let PlzBefund::Mehrdeutig(laender) = der_plz("14715") else {
            panic!("14715 lies in Brandenburg and Sachsen-Anhalt");
        };
        assert!(laender.contains(&Bundesland::Bb) && laender.contains(&Bundesland::St));
    }

    #[test]
    fn an_unknown_plz_is_unknown() {
        assert_eq!(der_plz("00000"), PlzBefund::Unbekannt);
        assert_eq!(der_plz("ABCDE"), PlzBefund::Unbekannt);
    }
}
