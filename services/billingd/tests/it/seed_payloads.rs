//! Guard: the request bodies the seeder posts to `billingd`
//! (`tools/mako-seed/world/`) deserialise into `billingd`'s request type.
//!
//! See `services/vertragd/tests/it/seed_payloads.rs` for why: a seeded body is
//! shipped API surface, and one whose fields the endpoint does not have would
//! return `2xx` with the values going nowhere.

use billingd::handlers::CalculateRequest;

/// A body from the seeder's world, `tools/mako-seed/world/<file>`.
fn world(file: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/mako-seed/world")
        .join(file);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    serde_json::from_str(&src)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

/// The seeded on-demand calculation.
///
/// `meter` and `grid` are the documented overrides: the reading and the
/// Netzentgelt are supplied in the body, so the invoice `mako-seed --verify`
/// checks is reproducible to the cent. The Stromsteuer is **not** among
/// them — § 3 StromStG fixes it at 2.05 ct/kWh and `billingd` applies it to
/// every Strom supply whatever the `grid` block says.
#[test]
fn the_seeded_calculate_payload_deserialises() {
    let req: CalculateRequest = serde_json::from_value(world("billingd-calculate.json")).expect(
        "billingd would refuse this seeded body — fix tools/mako-seed/world, or the endpoint",
    );
    assert_eq!(req.lf_mp_id, "9900357000004");
    assert!(
        req.meter.is_some(),
        "the seed supplies the reading, so the invoice does not depend on edmd's month"
    );
    assert_eq!(
        req.grid.and_then(|g| g.nne_arbeitspreis_ct_per_kwh),
        Some(rust_decimal::dec!(8.00)),
        "the grid override must land in the field the provider reads"
    );
}

/// A `grid` field `GridInput` does not have is a refusal.
///
/// Every grid field is optional and a provider adds a position only for a
/// field that is set, so a dropped key bills no Netzentgelt at all — the body
/// below is accepted by a `GridInput` that drops unknown keys, and prices to
/// zero.
#[test]
fn a_grid_field_the_type_does_not_have_is_refused() {
    let err = serde_json::from_value::<CalculateRequest>(serde_json::json!({
        "lf_mp_id": "9912345000005",
        "period_from": "2026-01-01",
        "period_to": "2026-01-31",
        "grid": { "netzentgelt_eur": "0", "messentgelt_eur": "0",
                  "umlagen_eur": "0", "konzessionsabgabe_eur": "0",
                  "stromsteuer_eur": "0" }
    }))
    .expect_err("an unknown grid field must be refused");
    assert!(err.to_string().contains("unknown field"), "{err}");
}

/// A field the endpoint does not have is a refusal, not a discarded value.
///
/// The trap this one guards is real: a caller who believes they can override
/// the electricity tax by naming it at the top level gets told they cannot,
/// instead of an invoice silently taxed anyway.
#[test]
fn a_field_the_endpoint_does_not_have_is_refused() {
    let err = serde_json::from_value::<CalculateRequest>(serde_json::json!({
        "lf_mp_id": "9912345000005",
        "period_from": "2026-01-01",
        "period_to": "2026-01-31",
        "stromsteuer_eur": "0"
    }))
    .expect_err("`stromsteuer_eur` is no field of the request");
    assert!(err.to_string().contains("unknown field"), "{err}");
}
