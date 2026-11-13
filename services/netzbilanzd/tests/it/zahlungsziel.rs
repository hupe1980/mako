//! `zahlungsziel_werktage` is the Netzbetreiber's own payment term: the boot
//! refuses without it and below the published floor, and an invoice whose
//! request states no `due_date` falls due on it.

use mako_service::Daemon as _;
use netzbilanzd::config::NetzbilanzConfig;

/// A configuration that passes every other check, with `zahlungsziel` merged in.
fn config(zahlungsziel: serde_json::Value) -> NetzbilanzConfig {
    let mut cfg = serde_json::json!({
        "database": { "url": "postgres://localhost/netzbilanzd" },
        "tenant": "9900357000004",
        "marktd_url": "http://marktd:8180", "marktd_api_key": "k",
        "makod_url": "http://makod:8080", "makod_api_key": "k",
        "inbound_secret": "s3cret",
        "allow_insecure_no_auth": true,
    });
    if let (Some(base), Some(extra)) = (cfg.as_object_mut(), zahlungsziel.as_object()) {
        for (k, v) in extra {
            base.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(cfg).expect("the configuration parses")
}

fn blockiert(cfg: &NetzbilanzConfig) -> Vec<mako_service::Befund> {
    netzbilanzd::Netzbilanzd::validate(cfg)
        .into_iter()
        .filter(mako_service::Befund::ist_blockiert)
        .collect()
}

/// Unset, the boot is refused and the finding names the setting: there is no
/// term to fall back on, and a substituted one bills every counterparty on a
/// date nobody chose.
#[test]
fn an_unset_zahlungsziel_refuses_the_boot_by_name() {
    let befunde = blockiert(&config(serde_json::json!({})));
    assert!(
        befunde.iter().any(|b| b.pfad == "zahlungsziel_werktage"),
        "{befunde:#?}"
    );
}

/// GPKE Teil 2 Kap. 3.3.2 Nr. 1: „Das Zahlungsziel darf 10 WT nach Empfang der
/// Rechnung nicht unterschreiten" — nine is refused, ten is the floor itself.
#[test]
fn a_zahlungsziel_below_the_floor_refuses_the_boot() {
    let befunde = blockiert(&config(serde_json::json!({ "zahlungsziel_werktage": 9 })));
    let befund = befunde
        .iter()
        .find(|b| b.pfad == "zahlungsziel_werktage")
        .unwrap_or_else(|| panic!("nine Werktage must be refused: {befunde:#?}"));
    assert!(
        befund
            .quelle
            .as_deref()
            .is_some_and(|q| q.contains("GPKE Teil 2")),
        "{befund:#?}"
    );

    let befunde = blockiert(&config(serde_json::json!({ "zahlungsziel_werktage": 10 })));
    assert!(befunde.is_empty(), "{befunde:#?}");
}

/// The configured term is what an invoice without a stated `due_date` falls
/// due on: 14 Werktage after Wednesday 01.07.2026 is Tuesday 21.07.2026.
#[test]
fn the_configured_term_is_the_due_date() {
    let cfg = config(serde_json::json!({ "zahlungsziel_werktage": 14 }));
    let due = netzbilanzd::zahlungsziel::faellig(
        cfg.zahlungsziel_werktage,
        time::macros::date!(2026 - 07 - 01),
        None,
        31_002,
        grid_billing::Sparte::Strom,
        false,
        false,
    )
    .expect("a configured term");
    assert_eq!(due, time::macros::date!(2026 - 07 - 21));
}
