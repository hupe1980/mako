use mako_messkonzept::{Change, Device, FeedIn, LokationKind, Modul14a, Netzanschluss};
use rust_decimal::dec;
use time::macros::date;

fn pv() -> Device {
    Device::Pv {
        kwp: dec!(9),
        inbetriebnahme: date!(2024 - 03 - 01),
        feed_in: FeedIn::Ueberschusseinspeisung,
        yield_kwh: None,
        self_consumption_kwh: None,
    }
}

fn heat_pump(modul: Modul14a) -> Device {
    Device::HeatPump { modul, kwh: None }
}

fn with(devices: Vec<Device>) -> Netzanschluss {
    Netzanschluss {
        devices,
        ..Default::default()
    }
}

fn kinds(n: &Netzanschluss) -> Vec<LokationKind> {
    n.lokationsbuendel()[0]
        .lokationen
        .iter()
        .map(|l| l.kind)
        .collect()
}

#[test]
fn a_household_alone_is_one_consuming_marktlokation() {
    let b = Netzanschluss::default().lokationsbuendel();
    assert_eq!(b.len(), 1);
    assert_eq!(
        b[0].counts().into_iter().collect::<Vec<_>>(),
        [(LokationKind::MaloConsumption, 1)]
    );
}

#[test]
fn modul_2_adds_its_own_marktlokation_and_messlokation_and_modul_1_does_not() {
    use LokationKind as K;
    let zwei = with(vec![heat_pump(Modul14a::Modul2)]);
    assert_eq!(
        kinds(&zwei),
        [
            K::MaloConsumption,
            K::MaloConsumption,
            K::Melo,
            K::TrConsumption
        ]
    );
    assert!(
        zwei.lokationsbuendel()[0].lokationen[1]
            .source
            .starts_with("BK8-22/010-A Tenor 2 a)")
    );
    assert_eq!(
        kinds(&with(vec![heat_pump(Modul14a::Modul1)])),
        [K::MaloConsumption, K::TrConsumption]
    );
}

#[test]
fn pv_adds_a_generating_marktlokation_and_a_further_household_its_own_bundle() {
    let n = with(vec![pv(), Device::Household { kwh: None }]);
    let b = n.lokationsbuendel();
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].counts().get(&LokationKind::MaloGeneration), Some(&1));
    assert_eq!(b[1].lokationen[0].device, Some(1));
    assert!(b[1].lokationen[0].source.contains("Kap. 2.5"));
}

#[test]
fn a_change_names_what_it_adds_and_removes() {
    let before = with(vec![heat_pump(Modul14a::Modul1)]);
    let after = with(vec![heat_pump(Modul14a::Modul2)]);
    let c = Change::between(&before, &after);
    assert_eq!(c.added[0].get(&LokationKind::MaloConsumption), Some(&1));
    assert_eq!(c.added[0].get(&LokationKind::Melo), Some(&1));
    assert!(c.removed[0].is_empty());
    assert_eq!(
        Change::between(&after, &before).removed[0].get(&LokationKind::MaloConsumption),
        Some(&1)
    );
}

#[test]
fn a_netzanschluss_reads_from_json() {
    let n: Netzanschluss = serde_json::from_str(
        r#"{"consumption_kwh":"4000","devices":[{"kind":"heat_pump","modul":"modul2","kwh":"3500"},{"kind":"storage"}]}"#,
    )
    .unwrap();
    assert_eq!(n.devices.len(), 2);
    assert_eq!(n.devices[0].modul(), Some(Modul14a::Modul2));
}
