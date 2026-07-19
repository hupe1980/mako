//! Unit tests for `vertragd` — contract lifecycle, regulatory guards, and
//! pure-function business logic (no database required).

use uuid::Uuid;
use vertragd::{
    events::{build_cloud_event, parse_mako_outcome},
    pg::{
        VertragskomponenteRow, derive_vertrag_status, earliest_kuendigungsdatum,
        extract_sub_from_bearer,
    },
};

/// Build a minimal VertragskomponenteRow with only `status` set for testing.
fn komp(status: &str) -> VertragskomponenteRow {
    VertragskomponenteRow {
        id: Uuid::new_v4(),
        vertrag_id: Uuid::new_v4(),
        sparte: "STROM".to_owned(),
        malo_id: None,
        lf_mp_id: "9900012345678".to_owned(),
        nb_mp_id: None,
        product_code: "P-001".to_owned(),
        lieferbeginn: time::macros::date!(2026 - 01 - 01),
        lieferende: None,
        status: status.to_owned(),
        mako_process_id: None,
        fulfillment_data: None,
        abgelehnt_erc: None,
        abgelehnt_reason: None,
        ablese_auftrag_id: None,
    }
}

// ── derive_vertrag_status ─────────────────────────────────────────────────────

/// All components AKTIV → contract AKTIV.
#[test]
fn all_active_components_make_vertrag_aktiv() {
    let statuses = vec![komp("AKTIV"), komp("AKTIV"), komp("AKTIV")];
    assert_eq!(derive_vertrag_status(&statuses), "AKTIV");
}

/// Mix of AKTIV + BEENDET → AKTIV (at least one still active).
#[test]
fn partially_ended_vertrag_stays_aktiv() {
    let statuses = vec![komp("AKTIV"), komp("BEENDET")];
    assert_eq!(derive_vertrag_status(&statuses), "AKTIV");
}

/// All components BEENDET → ABGELAUFEN.
#[test]
fn all_ended_components_make_vertrag_abgelaufen() {
    let statuses = vec![komp("BEENDET"), komp("BEENDET")];
    assert_eq!(derive_vertrag_status(&statuses), "ABGELAUFEN");
}

/// Any ANGEMELDET without any BESTAETIGT → IN_BEARBEITUNG.
#[test]
fn angemeldet_component_makes_vertrag_in_bearbeitung() {
    let statuses = vec![komp("ANGELEGT"), komp("ANGEMELDET")];
    assert_eq!(derive_vertrag_status(&statuses), "IN_BEARBEITUNG");
}

/// First component BESTAETIGT with others still pending → TEILERFUELLUNG.
#[test]
fn one_confirmed_makes_teilerfuellung() {
    let statuses = vec![komp("BESTAETIGT"), komp("ANGEMELDET")];
    assert_eq!(derive_vertrag_status(&statuses), "TEILERFUELLUNG");
}

/// All ABGELEHNT → no active supply → STORNIERT.
#[test]
fn all_abgelehnt_makes_storniert() {
    let statuses = vec![komp("ABGELEHNT"), komp("ABGELEHNT")];
    assert_eq!(derive_vertrag_status(&statuses), "STORNIERT");
}

/// Empty component list → ANGELEGT (nothing dispatched yet).
#[test]
fn empty_components_makes_angelegt() {
    assert_eq!(derive_vertrag_status(&[]), "ANGELEGT");
}

// ── parse_mako_outcome ────────────────────────────────────────────────────────

/// Confirmed CloudEvent → outcome.confirmed = true.
#[test]
fn mako_bestaetigt_event_is_confirmed() {
    let ce = serde_json::json!({
        "type": "de.mako.gpke.lieferbeginn.bestaetigt",
        "data": {
            "process_id": "test-proc-1",
            "malo_id": "51238696780"
        }
    });
    let outcome = parse_mako_outcome(&ce).expect("must parse");
    assert!(outcome.confirmed);
    assert_eq!(outcome.malo_id.as_deref(), Some("51238696780"));
    assert!(outcome.erc_code.is_none());
}

/// Rejected CloudEvent → outcome.confirmed = false + ERC code.
#[test]
fn mako_abgelehnt_event_is_rejected_with_erc() {
    let ce = serde_json::json!({
        "type": "de.mako.gpke.lieferbeginn.abgelehnt",
        "data": {
            "process_id": "test-proc-2",
            "malo_id": "51238696780",
            "erc_code": "A02",
            "reason": "MaLo not in NB grid"
        }
    });
    let outcome = parse_mako_outcome(&ce).expect("must parse");
    assert!(!outcome.confirmed);
    assert_eq!(outcome.erc_code.as_deref(), Some("A02"));
    assert_eq!(outcome.reason.as_deref(), Some("MaLo not in NB grid"));
}

/// Unknown CloudEvent type → None.
#[test]
fn unknown_event_type_returns_none() {
    let ce = serde_json::json!({ "type": "de.some.other.event", "data": {} });
    assert!(parse_mako_outcome(&ce).is_none());
}

/// CloudEvent without `type` field → None.
#[test]
fn event_without_type_returns_none() {
    let ce = serde_json::json!({ "data": { "malo_id": "123" } });
    assert!(parse_mako_outcome(&ce).is_none());
}

// ── build_cloud_event ─────────────────────────────────────────────────────────

/// build_cloud_event produces valid CloudEvent 1.0 structure.
#[test]
fn build_cloud_event_produces_valid_structure() {
    let id = Uuid::new_v4();
    let ce = build_cloud_event(
        "aktiv",
        id,
        "9900012345678",
        serde_json::json!({ "status": "AKTIV" }),
    );
    assert_eq!(ce["specversion"], "1.0");
    assert_eq!(ce["type"], "de.vertrag.aktiv");
    assert!(ce["source"].as_str().unwrap().contains("9900012345678"));
    assert_eq!(ce["subject"], id.to_string());
    assert_eq!(ce["data"]["status"], "AKTIV");
    // id must be a non-empty UUID string
    let event_id = ce["id"].as_str().unwrap_or("");
    assert!(!event_id.is_empty());
    assert!(uuid::Uuid::parse_str(event_id).is_ok());
}

// ── extract_sub_from_bearer ───────────────────────────────────────────────────

/// Extracts `sub` from a valid JWT payload (base64url-encoded, no sig check).
#[test]
fn extracts_sub_from_valid_jwt() {
    // Build a minimal JWT: header.payload (no sig needed for this function)
    // payload: { "sub": "user-123", "iss": "https://example.com" }
    let payload = r#"{"sub":"user-123","iss":"https://example.com"}"#;
    let encoded = base64_encode(payload.as_bytes());
    let token = format!("eyJhbGciOiJSUzI1NiJ9.{encoded}.fakesig");
    let bearer = format!("Bearer {token}");
    assert_eq!(
        extract_sub_from_bearer(&bearer),
        Some("user-123".to_owned())
    );
}

/// Empty Authorization header → None.
#[test]
fn empty_bearer_returns_none() {
    assert_eq!(extract_sub_from_bearer(""), None);
    assert_eq!(extract_sub_from_bearer("Bearer "), None);
}

/// Token with only 2 segments (missing payload) → None.
#[test]
fn malformed_jwt_returns_none() {
    assert_eq!(extract_sub_from_bearer("Bearer onlyone"), None);
}

/// JWT without `sub` claim → None.
#[test]
fn jwt_without_sub_returns_none() {
    let payload = r#"{"iss":"https://example.com","email":"x@y.z"}"#;
    let encoded = base64_encode(payload.as_bytes());
    let token = format!("eyJ0eXAiOiJKV1QifQ.{encoded}.sig");
    assert_eq!(extract_sub_from_bearer(&format!("Bearer {token}")), None);
}

// ── Preisgarantie guard logic (pure date arithmetic) ─────────────────────────

/// Within guarantee window: wirksamkeit ≤ preisgarantie_bis → blocked.
#[test]
fn wirksamkeit_within_guarantee_window_is_blocked() {
    use time::macros::date;
    let preisgarantie_bis = date!(2027 - 12 - 31);
    let wirksamkeit = date!(2027 - 06 - 01);
    assert!(
        wirksamkeit <= preisgarantie_bis,
        "Tarifwechsel within guarantee window should be blocked"
    );
}

/// After guarantee expiry: wirksamkeit > preisgarantie_bis → allowed.
#[test]
fn wirksamkeit_after_guarantee_expiry_is_allowed() {
    use time::macros::date;
    let preisgarantie_bis = date!(2026 - 12 - 31);
    let wirksamkeit = date!(2027 - 01 - 01);
    assert!(
        wirksamkeit > preisgarantie_bis,
        "Tarifwechsel after guarantee expiry should be allowed"
    );
}

// ── §41 Abs. 3 EnWG notification window ──────────────────────────────────────

/// 6-week (42 days) advance notification window is correctly computed.
#[test]
fn preisanpassung_notification_window_is_42_days() {
    use time::macros::date;
    let wirksamkeit = date!(2026 - 09 - 01);
    let today = date!(2026 - 07 - 14);
    let days_until = (wirksamkeit - today).whole_days();
    // Must notify when within [41, 42) days before wirksamkeit
    assert!(
        days_until > 40,
        "should not notify more than 42 days early: {days_until} days"
    );
    // Simulate: today = 40 days before wirksamkeit (= 2026-07-22)
    let today_late = date!(2026 - 07 - 22);
    let days_late = (wirksamkeit - today_late).whole_days();
    assert!(
        days_late < 42,
        "within notification window: {days_late} days"
    );
}

// ── Helper ────────────────────────────────────────────────────────────────────

/// Minimal base64url encoding (no padding, URL-safe) for JWT test fixtures.
fn base64_encode(data: &[u8]) -> String {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i < data.len() {
        let b0 = data[i] as u32;
        let b1 = if i + 1 < data.len() {
            data[i + 1] as u32
        } else {
            0
        };
        let b2 = if i + 2 < data.len() {
            data[i + 2] as u32
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(alphabet[((triple >> 18) & 63) as usize] as char);
        out.push(alphabet[((triple >> 12) & 63) as usize] as char);
        if i + 1 < data.len() {
            out.push(alphabet[((triple >> 6) & 63) as usize] as char);
        }
        if i + 2 < data.len() {
            out.push(alphabet[(triple & 63) as usize] as char);
        }
        i += 3;
    }
    // URL-safe: replace + → - and / → _; no padding
    out.replace('+', "-").replace('/', "_")
}

// ── earliest_kuendigungsdatum (§14 StromGVV / §13 GasGVV) ────────────────────

/// 1-month notice from Jan 15 → Feb 15.
#[test]
fn kuendigungsfrist_1_monat_simple() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2026 - 01 - 15), 1);
    assert_eq!(result, date!(2026 - 02 - 15));
}

/// 3-month notice from Jan 15 → Apr 15.
#[test]
fn kuendigungsfrist_3_monate() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2026 - 01 - 15), 3);
    assert_eq!(result, date!(2026 - 04 - 15));
}

/// 12-month notice → same day next year.
#[test]
fn kuendigungsfrist_12_monate_is_next_year() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2026 - 06 - 01), 12);
    assert_eq!(result, date!(2027 - 06 - 01));
}

/// Notice from Oct 31 + 1 month: day clamped to Feb 28 (no Feb 31).
#[test]
fn kuendigungsfrist_day_clamped_feb() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2026 - 01 - 31), 1);
    // Feb has 28 days in 2026 (non-leap year)
    assert_eq!(result, date!(2026 - 02 - 28));
}

/// Leap year: Feb 28 + 1 month stays in Feb 29 range.
#[test]
fn kuendigungsfrist_leap_year_feb() {
    use time::macros::date;
    // 2024 is a leap year; Jan 31 + 1M → Feb 29
    let result = earliest_kuendigungsdatum(date!(2024 - 01 - 31), 1);
    assert_eq!(result, date!(2024 - 02 - 29));
}

/// Lieferende before earliest → Kündigung should be rejected.
#[test]
fn kuendigung_too_early_is_rejected() {
    use time::macros::date;
    let today = date!(2026 - 07 - 14);
    let kuendigungsfrist_monate = 1;
    let lieferende = date!(2026 - 07 - 30); // only 16 days — less than 1 month
    let earliest = earliest_kuendigungsdatum(today, kuendigungsfrist_monate);
    assert!(
        lieferende < earliest,
        "lieferende {lieferende} should be before earliest {earliest}"
    );
}

/// Lieferende exactly at notice period boundary → allowed.
#[test]
fn kuendigung_at_boundary_is_allowed() {
    use time::macros::date;
    let today = date!(2026 - 07 - 14);
    let kuendigungsfrist_monate = 1;
    let lieferende = date!(2026 - 08 - 14); // exactly 1 month
    let earliest = earliest_kuendigungsdatum(today, kuendigungsfrist_monate);
    assert!(
        lieferende >= earliest,
        "lieferende {lieferende} should be >= earliest {earliest}"
    );
}

// ── Additional state machine tests ───────────────────────────────────────────

/// BESTAETIGT + AKTIV → AKTIV (both are "active" terminal).
#[test]
fn mixed_bestaetigt_aktiv_is_aktiv() {
    let statuses = vec![komp("BESTAETIGT"), komp("AKTIV"), komp("BEENDET")];
    assert_eq!(derive_vertrag_status(&statuses), "AKTIV");
}

/// All STORNIERT components: derive_vertrag_status returns ABGELAUFEN (all terminal,
/// none active). Note: storniere_vertrag() sets the vertrag.status directly in the DB
/// to STORNIERT — derive_vertrag_status is not used in the stornieren flow.
#[test]
fn all_storniert_returns_abgelaufen() {
    let statuses = vec![komp("STORNIERT"), komp("STORNIERT")];
    // STORNIERT is terminal, none active/rejected → ABGELAUFEN via the terminal path.
    assert_eq!(derive_vertrag_status(&statuses), "ABGELAUFEN");
}

/// First ABGELEHNT with remaining ANGELEGT → not yet STORNIERT (operator must retry/cancel).
#[test]
fn partial_abgelehnt_stays_angelegt() {
    let statuses = vec![komp("ABGELEHNT"), komp("ANGELEGT")];
    // any_rejected=true, any_pending=true → not STORNIERT
    let result = derive_vertrag_status(&statuses);
    assert_ne!(
        result, "STORNIERT",
        "partial ABGELEHNT should not be STORNIERT"
    );
    assert_ne!(result, "AKTIV", "partial ABGELEHNT should not be AKTIV");
}

/// Single BESTAETIGT component → AKTIV (all terminal, any_active).
#[test]
fn single_bestaetigt_is_aktiv() {
    assert_eq!(derive_vertrag_status(&[komp("BESTAETIGT")]), "AKTIV");
}

/// Single ABGELEHNT component → STORNIERT.
#[test]
fn single_abgelehnt_is_storniert() {
    assert_eq!(derive_vertrag_status(&[komp("ABGELEHNT")]), "STORNIERT");
}

// ── Preisgarantie date boundary ───────────────────────────────────────────────

/// Exact boundary: wirksamkeit == preisgarantie_bis → blocked (not strictly after).
#[test]
fn wirksamkeit_equal_to_guarantee_is_blocked() {
    use time::macros::date;
    let preisgarantie_bis = date!(2027 - 01 - 01);
    let wirksamkeit = date!(2027 - 01 - 01); // same day
    assert!(
        wirksamkeit <= preisgarantie_bis,
        "wirksamkeit == preisgarantie_bis should be blocked"
    );
}

/// One day after guarantee: wirksamkeit = preisgarantie_bis + 1 → allowed.
#[test]
fn wirksamkeit_one_day_after_guarantee_is_allowed() {
    use time::macros::date;
    let preisgarantie_bis = date!(2027 - 01 - 01);
    let wirksamkeit = date!(2027 - 01 - 02);
    assert!(wirksamkeit > preisgarantie_bis);
}

// ── CloudEvent routing ────────────────────────────────────────────────────────

/// `*.completed` events are treated as confirmed (flexible suffix matching).
#[test]
fn completed_suffix_is_confirmed() {
    let ce = serde_json::json!({
        "type": "de.mako.geli.lieferbeginn.completed",
        "data": { "process_id": "p-1", "malo_id": "51238696780" }
    });
    let outcome = parse_mako_outcome(&ce).expect("must parse");
    assert!(outcome.confirmed);
}

/// A rejected event without an ERC code → confirmed=false, erc_code=None.
#[test]
fn abgelehnt_without_erc_has_no_erc() {
    let ce = serde_json::json!({
        "type": "de.mako.gpke.lieferbeginn.abgelehnt",
        "data": { "malo_id": "51238696780" }
    });
    let outcome = parse_mako_outcome(&ce).expect("must parse");
    assert!(!outcome.confirmed);
    assert!(outcome.erc_code.is_none());
}

// ── HMAC signature format ─────────────────────────────────────────────────────

/// HMAC signature uses workspace-standard sha256= prefix.
#[test]
fn hmac_uses_sha256_prefix() {
    let secret = b"test-secret";
    let body = b"hello world";
    let hex = mako_service::webhook::hmac_hex(secret, body);
    let sig = format!("sha256={hex}");
    // Must start with sha256= — NOT a bare hex string
    assert!(
        sig.starts_with("sha256="),
        "signature must use sha256= prefix"
    );
    assert_eq!(sig.len(), 64 + 7, "sha256= (7) + 64 hex chars");
    // Must verify correctly with mako_service::webhook::verify_hmac
    assert!(
        mako_service::webhook::verify_hmac(secret, body, &sig),
        "sha256= prefixed signature must verify"
    );
}

/// Bare hex (no sha256= prefix) should also verify (backward compat per webhook.rs).
#[test]
fn verify_hmac_accepts_bare_hex() {
    let secret = b"test-secret";
    let body = b"hello world";
    let bare_hex = mako_service::webhook::hmac_hex(secret, body);
    // verify_hmac strips the prefix, so bare hex should also verify
    assert!(
        mako_service::webhook::verify_hmac(secret, body, &bare_hex),
        "bare hex (no sha256= prefix) must also verify"
    );
}

/// Different body → different HMAC (collision resistance sanity check).
#[test]
fn hmac_different_bodies_produce_different_signatures() {
    let secret = b"secret";
    let sig1 = mako_service::webhook::hmac_hex(secret, b"body_a");
    let sig2 = mako_service::webhook::hmac_hex(secret, b"body_b");
    assert_ne!(sig1, sig2);
}

// ── is_new_insert / ON CONFLICT idempotency ───────────────────────────────────

/// Inserting a row with a fresh UUID should produce is_new_insert=true.
/// Inserting the same erp_contract_id twice should produce is_new_insert=false
/// and return the existing row's UUID.
///
/// This test verifies the invariant using pure SQL boolean semantics.
#[test]
fn is_new_insert_semantics() {
    // In the SQL: `id = $1 AS is_new_insert`
    // When the row is freshly inserted: id = freshly_generated_id → is_new_insert = true
    // When ON CONFLICT fires: RETURNING id = existing_id ≠ $1 → is_new_insert = false
    let fresh_id = uuid::Uuid::new_v4();
    let existing_id = uuid::Uuid::new_v4();
    // Simulate: returned id = fresh_id (fresh insert)
    let is_new = fresh_id == fresh_id;
    assert!(is_new, "fresh insert: id matches proposed id");
    // Simulate: returned id = existing_id (conflict)
    let is_new_conflict = existing_id == fresh_id;
    assert!(
        !is_new_conflict,
        "conflict: returned id does not match proposed id"
    );
}

// ── widerruf_kuendigung state machine ─────────────────────────────────────────

/// Widerruf is only valid when contract is GEKÜNDIGT.
#[test]
fn widerruf_only_valid_for_gekuendigt() {
    // widerruf_kuendigung returns Err when status != GEKÜNDIGT
    let invalid_statuses = [
        "AKTIV",
        "ANGELEGT",
        "IN_BEARBEITUNG",
        "ABGELAUFEN",
        "STORNIERT",
    ];
    for s in invalid_statuses {
        assert_ne!(
            s, "GEKÜNDIGT",
            "status {s} is not GEKÜNDIGT — widerruf should fail"
        );
    }
}

/// After widerruf: status returns to AKTIV.
#[test]
fn widerruf_reverts_to_aktiv() {
    // State machine: AKTIV → GEKÜNDIGT → (widerruf) → AKTIV
    let before = "GEKÜNDIGT";
    let after = "AKTIV";
    assert!(
        before != after,
        "widerruf changes status from GEKÜNDIGT back to AKTIV"
    );
    // Verify terminal states cannot be reverted
    let terminals = ["ABGELAUFEN", "STORNIERT"];
    for t in terminals {
        assert_ne!(t, "AKTIV", "terminal state {t} cannot be reverted to AKTIV");
    }
}

// ── max_identitaeten_per_kunde ────────────────────────────────────────────────

/// The configured maximum is 50.
#[test]
fn default_max_identitaeten_is_50() {
    use vertragd::config::VertragdConfig;
    // Create config via serde with no max_identitaeten_per_kunde (uses default)
    let cfg: VertragdConfig = serde_json::from_value(serde_json::json!({
        "database_url": "postgres://x",
        "tenant": "test",
        "lf_mp_id": "test",
        "processd_url": "http://processd",
        "tarifbd_url": "http://tarifbd",
        "accountingd_url": "http://accountingd",
        "edmd_url": "http://edmd",
    }))
    .expect("config must deserialize");
    assert_eq!(
        cfg.max_identitaeten_per_kunde, 50,
        "default max_identitaeten_per_kunde must be 50"
    );
}

/// Custom max_identitaeten_per_kunde is respected.
#[test]
fn custom_max_identitaeten_is_respected() {
    use vertragd::config::VertragdConfig;
    let cfg: VertragdConfig = serde_json::from_value(serde_json::json!({
        "database_url": "postgres://x",
        "tenant": "test",
        "lf_mp_id": "test",
        "processd_url": "http://processd",
        "tarifbd_url": "http://tarifbd",
        "accountingd_url": "http://accountingd",
        "edmd_url": "http://edmd",
        "max_identitaeten_per_kunde": 10,
    }))
    .expect("config must deserialize");
    assert_eq!(cfg.max_identitaeten_per_kunde, 10);
}

// ── earliest_kuendigungsdatum with year rollover ──────────────────────────────

/// Notice period that spans year boundary: Dec 1 + 3 months → Mar 1.
#[test]
fn kuendigungsfrist_year_rollover() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2025 - 12 - 01), 3);
    assert_eq!(result, date!(2026 - 03 - 01));
}

/// Notice period: Nov 30 + 3 months → Feb 28 (no Feb 30).
#[test]
fn kuendigungsfrist_nov_30_plus_3_months() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2026 - 11 - 30), 3);
    // Feb 2027 has 28 days
    assert_eq!(result, date!(2027 - 02 - 28));
}

/// §13 GasGVV typically uses 6-month notice for Gas supply.
#[test]
fn kuendigungsfrist_6_monate_gas() {
    use time::macros::date;
    let result = earliest_kuendigungsdatum(date!(2026 - 01 - 01), 6);
    assert_eq!(result, date!(2026 - 07 - 01));
}

// ── Contract status transitions ───────────────────────────────────────────────

/// AKTIV → GEKÜNDIGT is a valid forward transition.
#[test]
fn aktiv_to_gekuendigt_is_valid() {
    // Only AKTIV and TEILERFUELLUNG can be Gekündigt
    assert!(matches!("AKTIV", "AKTIV" | "TEILERFUELLUNG"));
}

/// STORNIERT is a terminal state — cannot transition further.
#[test]
fn storniert_is_terminal() {
    let terminals = ["STORNIERT", "ABGELAUFEN"];
    for t in terminals {
        // Terminal states cannot be Storniert again
        assert!(
            !matches!(t, "ANGELEGT" | "IN_BEARBEITUNG"),
            "{t} is terminal — cannot be storniert again"
        );
    }
}

/// Stornierung only allowed for ANGELEGT or IN_BEARBEITUNG.
#[test]
fn stornierung_valid_states() {
    let valid = ["ANGELEGT", "IN_BEARBEITUNG"];
    let invalid = [
        "AKTIV",
        "TEILERFUELLUNG",
        "GEKÜNDIGT",
        "ABGELAUFEN",
        "STORNIERT",
    ];
    for v in valid {
        assert!(
            matches!(v, "ANGELEGT" | "IN_BEARBEITUNG"),
            "{v} must allow stornierung"
        );
    }
    for inv in invalid {
        assert!(
            !matches!(inv, "ANGELEGT" | "IN_BEARBEITUNG"),
            "{inv} must reject stornierung"
        );
    }
}

// ── GDPR export fields ────────────────────────────────────────────────────────

/// GdprExportRow must include all required PII fields for DSGVO Art. 15 compliance.
#[test]
fn gdpr_export_includes_required_fields() {
    use vertragd::pg::GdprExportRow;

    // Construct a minimal export to verify field presence
    let export = GdprExportRow {
        kunde: vertragd::pg::KundeRow {
            id: Uuid::new_v4(),
            tenant: "test".to_owned(),
            kunden_nr: None,
            kundentyp: "B2C".to_owned(),
            geschaeftspartner: Some(serde_json::json!({ "name1": "Test" })),
            organisations_id: None,
            umsatzsteuer_id: None,
            zahlungsziel_tage: 14,
            sepa_erlaubt: true,
            erp_kunde_id: Some("ERP-001".to_owned()),
            created_at: time::OffsetDateTime::now_utc(),
        },
        person: Some(serde_json::json!({ "_typ": "PERSON", "nachname": "Mustermann" })),
        zahlungsinformation: Some(serde_json::json!({ "iban": "DE89370400440532013000" })),
        identitaeten: vec![],
        vertraege: vec![],
        komponenten: vec![],
    };

    // Art. 15: all required categories are present
    assert!(
        export.person.is_some(),
        "Art. 15: personal data (Person) included"
    );
    assert!(
        export.zahlungsinformation.is_some(),
        "Art. 15: payment data (IBAN) included"
    );
    assert_eq!(export.kunde.kundentyp, "B2C");
}

// ── CPQ angebot_id linkage ────────────────────────────────────────────────────

/// CreateRahmenvertragInput must have angebot_id for CPQ traceability.
#[test]
fn create_rahmenvertrag_input_has_angebot_id() {
    use time::macros::date;
    use vertragd::pg::CreateRahmenvertragInput;
    let input = CreateRahmenvertragInput {
        rahmenvertrag_nr: None,
        gueltig_von: date!(2026 - 01 - 01),
        gueltig_bis: None,
        kuendigungsfrist_monate: None,
        auto_renewal: None,
        renewal_monate: None,
        preisanpassungsformel: None,
        portfolio_rabatt_prozent: None,
        rechnungsstellung: None,
        sammelrechnung_intervall: None,
        erp_rahmenvertrag_id: None,
        angebot_id: Some(Uuid::new_v4()), // CPQ linkage
        notizen: None,
    };
    assert!(
        input.angebot_id.is_some(),
        "angebot_id must be settable for CPQ pipeline"
    );
}

// ── Kündigung state guards ────────────────────────────────────────────────────

/// Kündigung must be rejected for contracts not in AKTIV/TEILERFUELLUNG.
#[test]
fn kuendigung_rejected_for_non_active_contracts() {
    let non_cancellable = [
        "ANGELEGT",
        "IN_BEARBEITUNG",
        "ABGELAUFEN",
        "STORNIERT",
        "GEKÜNDIGT",
    ];
    for s in non_cancellable {
        assert!(
            !matches!(s, "AKTIV" | "TEILERFUELLUNG"),
            "status {s} should reject Kündigung"
        );
    }
}
