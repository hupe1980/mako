use mako_messkonzept::{
    Bound, Catalogue, Device, FeedIn, LokationKind, Messkonzept, Modul14a, Netzanschluss,
    structures,
};
use rust_decimal::dec;
use time::macros::date;

fn bound(kind: LokationKind, min: u32, max: Option<u32>) -> Bound {
    Bound { kind, min, max }
}

fn catalogue() -> Catalogue {
    use LokationKind as K;
    Catalogue {
        messkonzepte: vec![
            Messkonzept {
                number: "MK 1".into(),
                title: "Bezug".into(),
                objects: vec![
                    bound(K::MaloConsumption, 1, Some(1)),
                    bound(K::TrConsumption, 0, None),
                ],
                lokationsbuendelcode: None,
                source: "Beispielkatalog S. 1".into(),
            },
            Messkonzept {
                number: "MK 2".into(),
                title: "Bezug und § 14a Modul 2".into(),
                objects: vec![
                    bound(K::MaloConsumption, 2, Some(2)),
                    bound(K::TrConsumption, 1, None),
                    bound(K::Melo, 2, Some(2)),
                ],
                lokationsbuendelcode: None,
                source: "Beispielkatalog S. 2".into(),
            },
        ],
    }
}

#[test]
fn a_plain_household_fits_the_codelistes_standard_structure() {
    let s = structures(&Netzanschluss::default().lokationsbuendel()[0]);
    assert!(s.iter().any(|s| s.code == "9992000000026"), "{s:?}");
}

#[test]
fn a_pv_household_fits_no_consumption_only_structure() {
    let n = Netzanschluss {
        devices: vec![Device::Pv {
            kwp: dec!(9),
            inbetriebnahme: date!(2026 - 03 - 01),
            feed_in: FeedIn::Ueberschusseinspeisung,
            yield_kwh: None,
            self_consumption_kwh: None,
        }],
        ..Default::default()
    };
    let s = structures(&n.lokationsbuendel()[0]);
    assert!(!s.is_empty());
    assert!(s.iter().all(|s| s.code != "9992000000026"));
}

#[test]
fn the_catalogue_names_the_messkonzept_that_holds_the_bundle() {
    let n = Netzanschluss {
        devices: vec![Device::HeatPump {
            modul: Modul14a::Modul2,
            kwh: None,
        }],
        ..Default::default()
    };
    let c = catalogue();
    let m = c.matching(&n.lokationsbuendel());
    assert_eq!(
        m[0].messkonzepte
            .iter()
            .map(|k| k.number.as_str())
            .collect::<Vec<_>>(),
        ["MK 2"]
    );
}

#[test]
fn an_empty_catalogue_names_no_messkonzept() {
    let c = Catalogue::default();
    let m = c.matching(&Netzanschluss::default().lokationsbuendel());
    assert!(m[0].messkonzepte.is_empty());
}
