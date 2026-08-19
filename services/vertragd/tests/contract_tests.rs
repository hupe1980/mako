//! Behaviour that spans modules: how a MaKo outcome is read off the wire, what
//! a `de.vertrag.*` event looks like on it, and how the statutory rules of
//! [`vertragd::domain`] resolve for whole, realistic contracts.
//!
//! The rules themselves are unit-tested beside their definitions; what is
//! checked here is that the pieces agree — that a Grundversorgungsvertrag for a
//! household and a fixed-term B2B supply contract each get the treatment the
//! statute gives *them*, and not each other's.

use time::macros::date;
use vertragd::{
    domain::{self, Kuendigungsgrund, Verlaengerung, Vertragsart},
    events::{build_cloud_event, parse_mako_outcome},
};

// ── MaKo outcome parsing ──────────────────────────────────────────────────────

fn ce(ce_type: &str, data: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "specversion": "1.0",
        "id": "01J0000000000000000000",
        "type": ce_type,
        "source": "/mako/processd/9900357000004",
        "data": data,
    })
}

#[test]
fn a_confirmation_carries_the_process_and_the_confirmed_malo() {
    let outcome = parse_mako_outcome(&ce(
        "de.mako.process.bestaetigt",
        serde_json::json!({ "process_id": "P-1", "malo_id": "51238696012" }),
    ))
    .expect("a .bestaetigt event is an outcome");
    assert!(outcome.confirmed);
    assert_eq!(outcome.process_id.as_deref(), Some("P-1"));
    assert_eq!(outcome.malo_id.as_deref(), Some("51238696012"));
    assert!(
        outcome.erc_code.is_none(),
        "a confirmation carries no rejection code"
    );
}

#[test]
fn every_confirmation_spelling_is_recognised() {
    for t in [
        "de.mako.process.bestaetigt",
        "de.mako.lieferbeginn.confirmed",
        "de.mako.process.completed",
    ] {
        let outcome = parse_mako_outcome(&ce(t, serde_json::json!({ "process_id": "P" })))
            .unwrap_or_else(|| panic!("{t} must parse"));
        assert!(outcome.confirmed, "{t} is a confirmation");
    }
}

#[test]
fn a_rejection_keeps_the_erc_code_the_customer_is_told() {
    let outcome = parse_mako_outcome(&ce(
        "de.mako.process.abgelehnt",
        serde_json::json!({
            "process_id": "P-2",
            "erc_code": "A02",
            "reason": "MaLo nicht im Netzgebiet",
        }),
    ))
    .expect("a .abgelehnt event is an outcome");
    assert!(!outcome.confirmed);
    assert_eq!(outcome.erc_code.as_deref(), Some("A02"));
    assert_eq!(outcome.reason.as_deref(), Some("MaLo nicht im Netzgebiet"));
}

#[test]
fn an_unrelated_event_is_not_an_outcome() {
    assert!(parse_mako_outcome(&ce("de.markt.malo.updated", serde_json::json!({}))).is_none());
    assert!(parse_mako_outcome(&serde_json::json!({ "data": {} })).is_none());
}

// ── Emitted events ────────────────────────────────────────────────────────────

#[test]
fn an_emitted_event_correlates_on_the_contract_it_belongs_to() {
    let vertrag_id = uuid::Uuid::new_v4();
    let event = build_cloud_event(
        mako_events::vertrag::KUENDIGUNG,
        vertrag_id,
        "9900357000004",
        serde_json::json!({ "lieferende": "2026-12-31" }),
    );
    let json = serde_json::to_value(&event).expect("serialise");
    assert_eq!(json["type"], mako_events::vertrag::KUENDIGUNG);
    assert_eq!(json["subject"], vertrag_id.to_string());
    assert_eq!(json["tenantid"], "9900357000004");
    assert_eq!(
        json["correlationid"], json["subject"],
        "consumers correlate a contract's whole lifecycle without parsing data"
    );
}

// ── Whole-contract scenarios ─────────────────────────────────────────────────

/// A household in the Grundversorgung terminates on two weeks, whatever the
/// contract record says about a notice period — § 20 Abs. 1 StromGVV is not
/// dispositive.
#[test]
fn the_grundversorgung_ignores_a_longer_agreed_notice_period() {
    let f = domain::kuendigungsfrist(
        date!(2026 - 03 - 02),
        Vertragsart::Grundversorgung,
        true,
        Kuendigungsgrund::Ordentlich,
        6,
        None,
    );
    assert_eq!(f.fruehestens, date!(2026 - 03 - 16));
    assert_eq!(f.rechtsgrundlage, "§ 20 Abs. 1 StromGVV / GasGVV");
}

/// The same customer under a Sondervertrag is capped at one month by
/// § 309 Nr. 9 lit. c BGB — a longer clause is void, not enforceable.
#[test]
fn a_consumer_sondervertrag_is_capped_at_one_month() {
    let f = domain::kuendigungsfrist(
        date!(2026 - 03 - 02),
        Vertragsart::Sondervertrag,
        true,
        Kuendigungsgrund::Ordentlich,
        6,
        None,
    );
    assert_eq!(f.fruehestens, date!(2026 - 04 - 02));
    assert!(f.rechtsgrundlage.contains("§ 309 Nr. 9 lit. c BGB"));
}

/// A price change gives that customer a way out on the day it lands, free of
/// charge, whichever regime they are in.
#[test]
fn a_price_change_ends_the_contract_on_the_day_it_takes_effect() {
    for (art, norm) in [
        (Vertragsart::Sondervertrag, "§ 41 Abs. 5 Satz 4 EnWG"),
        (Vertragsart::Grundversorgung, "§ 5 Abs. 3 StromGVV / GasGVV"),
    ] {
        let f = domain::kuendigungsfrist(
            date!(2026 - 03 - 02),
            art,
            true,
            Kuendigungsgrund::Preisanpassung,
            6,
            Some(date!(2026 - 04 - 01)),
        );
        assert_eq!(f.fruehestens, date!(2026 - 04 - 01));
        assert_eq!(f.rechtsgrundlage, norm);
    }
}

/// The three price-change notice periods are genuinely different, and the
/// Grundversorgung additionally may only change at a month boundary.
#[test]
fn the_notice_a_price_change_needs_depends_on_the_regime_and_the_customer() {
    let gv = domain::preisanpassungsregime(Vertragsart::Grundversorgung, true);
    let haushalt = domain::preisanpassungsregime(Vertragsart::Sondervertrag, true);
    let gewerbe = domain::preisanpassungsregime(Vertragsart::Sondervertrag, false);
    assert!(gv.vorlauf_tage > haushalt.vorlauf_tage);
    assert!(haushalt.vorlauf_tage > gewerbe.vorlauf_tage);
    assert!(gv.nur_zum_monatsersten);
    assert!(!haushalt.nur_zum_monatsersten);
}

/// A 24-month consumer contract that rolls over into another 24 months is two
/// § 309 Nr. 9 BGB problems, and both are named.
#[test]
fn an_unlawful_consumer_term_reports_every_clause_it_breaks() {
    let verstoesse = domain::pruefe_laufzeit(
        true,
        Vertragsart::Sondervertrag,
        date!(2026 - 01 - 01),
        Some(date!(2029 - 01 - 01)),
        3,
        true,
        24,
    );
    let normen: Vec<&str> = verstoesse.iter().map(|v| v.rechtsgrundlage).collect();
    assert!(normen.contains(&"§ 309 Nr. 9 lit. a BGB"), "{normen:?}");
    assert!(normen.contains(&"§ 309 Nr. 9 lit. b BGB"), "{normen:?}");
    assert!(normen.contains(&"§ 309 Nr. 9 lit. c BGB"), "{normen:?}");
}

/// The same terms are lawful for a business customer — § 309 does not reach
/// them (§ 310 Abs. 1 BGB).
#[test]
fn the_same_terms_are_lawful_for_a_business_customer() {
    assert!(
        domain::pruefe_laufzeit(
            false,
            Vertragsart::Sondervertrag,
            date!(2026 - 01 - 01),
            Some(date!(2029 - 01 - 01)),
            3,
            true,
            24,
        )
        .is_empty()
    );
}

/// The extension a consumer contract actually receives is open-ended, so the
/// customer can leave on a month's notice — extending it by a further fixed
/// term is what the clause forbids.
#[test]
fn a_consumer_contract_rolls_into_an_open_ended_one_and_a_business_one_does_not() {
    assert_eq!(
        domain::verlaengerung(true, date!(2026 - 12 - 31), 12, date!(2027 - 01 - 01)),
        Verlaengerung::Unbefristet
    );
    assert_eq!(
        domain::verlaengerung(false, date!(2026 - 12 - 31), 12, date!(2027 - 01 - 01)),
        Verlaengerung::Befristet(date!(2027 - 12 - 31))
    );
}
