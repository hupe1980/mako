use mako_messkonzept::{
    Decision, Device, FeedIn, Modul14a, Netzanschluss, Preisblatt, Topic, volleinspeisung_from,
};
use rust_decimal::{Decimal, dec};
use time::macros::date;

const TODAY: time::Date = date!(2026 - 11 - 21);

fn topic(n: &Netzanschluss, t: Topic) -> Decision {
    n.decisions(TODAY)
        .into_iter()
        .find(|d| d.topic == t)
        .unwrap()
}

fn eur(d: &Decision, key: &str) -> Option<Decimal> {
    d.choices
        .iter()
        .find(|c| c.key == key)
        .unwrap()
        .eur_per_year
}

fn fact(d: &Decision, key: &str) -> Option<String> {
    d.facts
        .iter()
        .find(|(f, _)| *f == key)
        .map(|(_, v)| v.clone())
}

fn pv(kwp: Decimal) -> Device {
    Device::Pv {
        kwp,
        inbetriebnahme: date!(2024 - 03 - 01),
        feed_in: FeedIn::Ueberschusseinspeisung,
        yield_kwh: Some(dec!(8000)),
        self_consumption_kwh: Some(dec!(2500)),
    }
}

#[test]
fn modul_2_saves_sixty_percent_of_the_arbeitspreis_on_the_device() {
    let n = Netzanschluss {
        devices: vec![Device::HeatPump {
            modul: Modul14a::Modul1,
            kwh: Some(dec!(4000)),
        }],
        preisblatt: Preisblatt {
            arbeitspreis_ct: Some(dec!(8.5)),
            pauschale_eur: Some(dec!(150)),
        },
        ..Default::default()
    };
    let d = topic(&n, Topic::Modul14a);
    assert_eq!(eur(&d, "modul1"), Some(dec!(150)));
    assert_eq!(eur(&d, "modul2"), Some(dec!(204.00)));
    assert_eq!(fact(&d, "modul2_from_kwh").as_deref(), Some("2941"));
    assert!(d.missing.is_empty());
}

#[test]
fn without_the_preisblatt_the_module_decision_stays_open() {
    let n = Netzanschluss {
        devices: vec![Device::Wallbox {
            modul: Modul14a::Modul1,
            kwh: Some(dec!(2000)),
        }],
        ..Default::default()
    };
    let d = topic(&n, Topic::Modul14a);
    assert_eq!(eur(&d, "modul2"), None);
    assert_eq!(
        d.missing,
        ["preisblatt.arbeitspreis_ct", "preisblatt.pauschale_eur"]
    );
}

#[test]
fn the_feed_in_rates_are_the_settlements_net_of_section_53() {
    let n = Netzanschluss {
        devices: vec![pv(dec!(9))],
        energy_price_ct: Some(dec!(32)),
        ..Default::default()
    };
    let d = topic(&n, Topic::FeedIn);
    assert_eq!(
        fact(&d, "ueberschusseinspeisung_ct").as_deref(),
        Some("8.11")
    );
    assert_eq!(fact(&d, "volleinspeisung_ct").as_deref(), Some("12.87"));
    // 5 500 kWh × 8,11 ct + 2 500 kWh × 32 ct; 8 000 kWh × 12,87 ct.
    assert_eq!(eur(&d, "ueberschusseinspeisung"), Some(dec!(1246.05)));
    assert_eq!(eur(&d, "volleinspeisung"), Some(dec!(1029.60)));
}

#[test]
fn volleinspeisung_applies_from_the_year_after_a_notice_before_1_december() {
    let ibn = date!(2024 - 03 - 01);
    assert_eq!(volleinspeisung_from(date!(2024 - 02 - 01), ibn), ibn);
    assert_eq!(
        volleinspeisung_from(date!(2026 - 11 - 30), ibn),
        date!(2027 - 01 - 01)
    );
    assert_eq!(
        volleinspeisung_from(date!(2026 - 12 - 01), ibn),
        date!(2028 - 01 - 01)
    );
}

#[test]
fn a_heat_pump_under_14a_makes_the_smart_meter_mandatory() {
    let n = Netzanschluss {
        consumption_kwh: Some(dec!(3000)),
        devices: vec![Device::HeatPump {
            modul: Modul14a::Modul1,
            kwh: Some(dec!(4000)),
        }],
        ..Default::default()
    };
    let d = topic(&n, Topic::Messsystem);
    assert_eq!(fact(&d, "duty").as_deref(), Some("yes"));
    // 7 000 kWh with a § 14a Vereinbarung: § 30 Abs. 1 Nr. 4, 50 EUR.
    assert_eq!(eur(&d, "pflichteinbau"), Some(dec!(-50)));
}

#[test]
fn a_plant_above_7_kw_alone_makes_the_duty_conditional() {
    let n = Netzanschluss {
        consumption_kwh: Some(dec!(3000)),
        devices: vec![pv(dec!(9))],
        ..Default::default()
    };
    let d = topic(&n, Topic::Messsystem);
    assert_eq!(
        fact(&d, "duty").as_deref(),
        Some("conditional"),
        "§ 29 Abs. 1 Nr. 2 b „soweit … erforderlich“"
    );
    assert!(!d.choices[0].chosen);
}

#[test]
fn a_small_household_has_no_duty_and_the_optional_cap() {
    let n = Netzanschluss {
        consumption_kwh: Some(dec!(3000)),
        ..Default::default()
    };
    let d = topic(&n, Topic::Messsystem);
    assert_eq!(fact(&d, "duty").as_deref(), Some("no"));
    assert_eq!(eur(&d, "optionaler_einbau"), Some(dec!(-30)));
}

#[test]
fn gebaeudeversorgung_and_mieterstrom_need_the_buildings_pv() {
    let keys = |n: &Netzanschluss| -> Vec<&'static str> {
        topic(n, Topic::Household)
            .choices
            .iter()
            .map(|c| c.key)
            .collect()
    };
    let flat = Device::Household { kwh: None };
    assert_eq!(
        keys(&Netzanschluss {
            devices: vec![flat.clone()],
            ..Default::default()
        }),
        ["own_marktlokation"]
    );
    assert_eq!(
        keys(&Netzanschluss {
            devices: vec![pv(dec!(9)), flat],
            ..Default::default()
        }),
        [
            "own_marktlokation",
            "gemeinschaftliche_gebaeudeversorgung",
            "mieterstrom"
        ]
    );
}
