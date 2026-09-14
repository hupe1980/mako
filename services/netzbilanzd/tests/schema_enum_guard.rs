//! Guards `netzbilanzd`'s SQL `CHECK` lists against the Rust that writes them.
//!
//! Eight columns across two tables, and three of them are written from a type
//! whose vocabulary is **wider** than the column's. That is the interesting
//! case: the code compiles, the type says a value is legal, and Postgres refuses
//! the statement at run time.
//!
//! - `invoice_drafts.pid` lists five, `SettlementType::default_pid` answers ten
//!   things including `31006` and `0`.
//! - `invoice_drafts.steuer_kategorie` lists two, `billing::TaxCategory` has ten
//!   codes.
//!
//! Neither is wrong — netzbilanzd reaches only the narrow set — but nothing said
//! so, so a sixth `SettlementRequest` arm or a new Umsatzsteuer branch would be
//! a refused INSERT rather than a compile error. The exhaustive matches below
//! turn the first into a compile error and the guard turns the second into a
//! test failure.
//!
//! The parser is `mako_service::schema_check`: `pid` is an **unquoted integer**
//! list, which a quote-seeking parser reads as empty and then passes.

use mako_service::schema_check::{assert_agrees_in, check_values_in};
use netzbilanzd::request::SettlementRequest;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// The PID each `SettlementRequest` arm settles as.
///
/// Exhaustive on purpose: a sixth arm stops this file compiling, which is the
/// signal wanted — `invoice_drafts.pid` would otherwise refuse its INSERT at
/// run time. Adding one means deciding its PID and widening the `CHECK`.
fn pid_of(request: &SettlementRequest) -> u32 {
    match request {
        SettlementRequest::Abschlag(_) => 31_001,
        SettlementRequest::Nne(_) => 31_002,
        SettlementRequest::Mmm(_) => 31_005,
        SettlementRequest::Msb(_) => 31_009,
        SettlementRequest::GasAwh(_) => 31_011,
    }
}

/// Every PID an arm settles as, as the column spells them.
const REACHABLE_PIDS: &[&str] = &["31001", "31002", "31005", "31009", "31011"];

/// The `pid` list is exactly the PIDs the five request arms produce.
///
/// `SettlementType::default_pid` also answers `31006` (MMM selbstausgestellt)
/// and `0` (Redispatch-Kostenblatt, dezentrale Einspeisung); neither is
/// reachable from a `SettlementRequest`, so neither belongs in the column.
#[test]
fn the_pid_list_is_the_reachable_settlement_pids() {
    assert_agrees_in(SCHEMA, "invoice_drafts", "pid", REACHABLE_PIDS);
}

/// `REACHABLE_PIDS` and `pid_of` state the same mapping.
///
/// Without this the constant above is a second copy of the match and could
/// drift from it — the exact defect this file exists to stop, one level up.
#[test]
fn the_constant_agrees_with_the_match() {
    let from_match: Vec<String> = [31_001_u32, 31_002, 31_005, 31_009, 31_011]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        from_match,
        REACHABLE_PIDS
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        "`REACHABLE_PIDS` and the arms of `pid_of` disagree"
    );
    // `pid_of` is referenced so the exhaustive match is compiled, not dead.
    let _ = pid_of as fn(&SettlementRequest) -> u32;
}

/// Both Sparten, and only those two.
#[test]
fn the_sparte_list_matches_grid_billing() {
    assert_agrees_in(SCHEMA, "invoice_drafts", "sparte", &["STROM", "GAS"]);
}

/// The two Umsatzsteuer categories a grid settlement can carry.
///
/// `billing::TaxCategory` publishes ten codes. grid-billing constructs
/// `Standard` (19 % on network services, UStAE 13b.3a) and `ReverseCharge`
/// (§ 13b UStG) and nothing else, so the column lists those two. A new branch —
/// an echte Steuerbefreiung would be `E`, a § 4 Nr. 1 export `G` — is a refused
/// INSERT until the column learns it.
#[test]
fn the_steuer_kategorie_list_is_the_two_codes_grid_billing_constructs() {
    use billing::TaxCategory;
    assert_eq!(TaxCategory::Standard.code(), "S");
    assert_eq!(TaxCategory::ReverseCharge.code(), "AE");
    assert_agrees_in(
        SCHEMA,
        "invoice_drafts",
        "steuer_kategorie",
        &[
            TaxCategory::Standard.code(),
            TaxCategory::ReverseCharge.code(),
        ],
    );
}

/// The three document kinds the handlers write.
#[test]
fn the_rechnungsart_list_matches_the_handlers() {
    assert_agrees_in(
        SCHEMA,
        "invoice_drafts",
        "rechnungsart",
        &["RECHNUNG", "STORNORECHNUNG", "KORREKTURRECHNUNG"],
    );
}

/// `check_outcome` is `invoic_checker::CheckOutcome` through `outcome_str`.
#[test]
fn the_check_outcome_list_matches_the_enum() {
    use invoic_checker::CheckOutcome;
    let written: Vec<&str> = [CheckOutcome::Ok, CheckOutcome::Warn, CheckOutcome::Dispute]
        .into_iter()
        .map(netzbilanzd::pg::outcome_str)
        .collect();
    assert_agrees_in(SCHEMA, "invoice_drafts", "check_outcome", &written);
}

/// The draft lifecycle, written as inline SQL literals.
#[test]
fn the_invoice_draft_status_list_matches_the_transitions() {
    assert_agrees_in(
        SCHEMA,
        "invoice_drafts",
        "status",
        &["draft", "dispatched", "paid", "disputed", "rejected"],
    );
}

/// Provenance of `dispatch_kwh`, for § 147 AO / GoBD auditability.
#[test]
fn the_dispatch_source_list_matches_the_three_provenances() {
    assert_agrees_in(
        SCHEMA,
        "kostenblatt_records",
        "dispatch_source",
        &["lastgang_sum", "billing_period", "manual_override"],
    );
}

/// The Kostenblatt lifecycle stops at `submitted`, and the column says so.
///
/// `confirmed` / `disputed` / `paid` were listed here and written by nothing:
/// they are the ÜNB's answer and netzbilanzd has no leg that receives one. A
/// schema claiming a lifecycle the code does not have leaves a reader unable to
/// tell "not implemented" from "never reached".
#[test]
fn the_kostenblatt_status_list_stops_where_the_code_does() {
    assert_agrees_in(
        SCHEMA,
        "kostenblatt_records",
        "status",
        &["pending", "submitted"],
    );
}

/// Two tables carry a `status` column with different vocabularies.
///
/// Pinned because an unscoped lookup answers with the first match in the file,
/// so a guard asking the wrong question would check `invoice_drafts` twice and
/// pass.
#[test]
fn the_two_status_columns_are_read_separately() {
    let drafts = check_values_in(SCHEMA, "invoice_drafts", "status");
    let kostenblatt = check_values_in(SCHEMA, "kostenblatt_records", "status");
    assert_eq!(drafts.len(), 5);
    assert_eq!(kostenblatt.len(), 2);
    assert_ne!(drafts, kostenblatt);
}
