//! # Example: the BO4E gate — accepting a document you did not write
//!
//! BO4E is a **transfer** standard, and its schema is deliberately permissive:
//! of the 35 Geschäftsobjekte at `v202607.1.0`, exactly two declare a
//! `required` array and none declares `oneOf`/`anyOf`/`not`. `Marktlokation`
//! accepts `{}`. So "it deserialised" is not validation, and every real rule
//! lives in prose.
//!
//! [`mako_markt::bo4e`] is mako's answer: **one** gate, four stages, in front of
//! every endpoint that takes a BO4E payload.
//!
//! | Stage | Refuses | `code` |
//! |---|---|---|
//! | 1. Discriminator | a `Zaehler` posted to the `Geraet` endpoint | `bo4e.discriminator` |
//! | 2. Schema | a value the type cannot hold | `bo4e.schema` |
//! | 3. Strict enums | `"sparte": "STROMM"`, at any depth, by JSON-path | `bo4e.unknown_enum` |
//! | 4. BO4E rules | a document the standard's prose forbids | `bo4e.rule` |
//!
//! This example runs all four, shows the two variants that deliberately do
//! *less* ([`decode_received`], [`ensure_conformant`]), and demonstrates
//! [`Bo4e<T>`] — the type that makes forgetting the gate impossible.
//!
//! ## Run
//!
//! ```text
//! cargo run -p mako-markt --example 01_bo4e_gate
//! ```

use mako_markt::bo4e::{self, Bo4e};
use rubo4e::current::{Geschaeftspartner, Marktlokation, Rechnung};

fn main() {
    let mut findings = 0_usize;
    findings += stage_1_discriminator();
    findings += stage_2_schema();
    findings += stage_3_strict_enums();
    findings += stage_4_rules();
    findings += a_received_document_is_read_and_disputed();
    findings += the_outbound_gate_is_stricter();
    findings += the_type_that_cannot_skip_the_gate();

    println!("\n─────────────────────────────────────────────────────────────");
    assert_eq!(
        findings, 0,
        "every assertion in this example must hold; {findings} did not"
    );
    println!("✓ all four stages, both variants, and Bo4e<T> behaved as documented");
}

// ── Stage 1: the discriminator ───────────────────────────────────────────────

/// `_typ` is **injected when absent** and refused when present and wrong.
///
/// Injecting it is not laxity: the endpoint already fixes which BO it takes, so
/// asking the caller to repeat it adds a way to be wrong and no information. A
/// `_typ` that names *another* BO is a different matter — nothing downstream
/// would catch it, because the strict-enum walk visits a value's fields and
/// never reaches `typ`, so a wrong one decodes to `Unknown` and serialises back
/// out as the literal string `"UNKNOWN"`.
fn stage_1_discriminator() -> usize {
    section("1. Discriminator");
    let mut bad = 0;

    let malo: Marktlokation = bo4e::decode(serde_json::json!({
        "marktlokationsId": "51238696781"
    }))
    .expect("a payload with no _typ is accepted — the gate injects it");
    println!("  no `_typ` in the request  → accepted, injected on the way out");
    bad += check(
        "the stored form carries the discriminant",
        bo4e::to_canonical_json(&malo).expect("serialisable")["_typ"] == "MARKTLOKATION",
    );

    let err = bo4e::decode::<Marktlokation>(serde_json::json!({ "_typ": "MESSLOKATION" }))
        .expect_err("a Messlokation is not a Marktlokation");
    println!("  `_typ: MESSLOKATION`      → {err}");
    bad += check(
        "code is bo4e.discriminator",
        err.code() == "bo4e.discriminator",
    );
    bad += check(
        "the body names both types",
        err.to_json()["expected_typ"] == "MARKTLOKATION"
            && err.to_json()["found_typ"] == "MESSLOKATION",
    );
    bad
}

// ── Stage 2: the schema ──────────────────────────────────────────────────────

/// A value the type cannot hold. Note *which* values those are: BO4E makes
/// nearly every field optional, so this stage catches shapes, not omissions —
/// a missing `sparte` is not a schema error, and an endpoint that needs one
/// says so itself, after the gate.
fn stage_2_schema() -> usize {
    section("2. Schema");
    let mut bad = 0;
    let err = bo4e::decode::<Marktlokation>(serde_json::json!({
        "marktlokationsId": 51_238_696_781_i64
    }))
    .expect_err("a MaLo-ID is a string, not a number");
    println!("  numeric `marktlokationsId` → {err}");
    bad += check("code is bo4e.schema", err.code() == "bo4e.schema");

    // And the identifier itself is checked: 11 digits with a BDEW check digit.
    let err = bo4e::decode::<Marktlokation>(serde_json::json!({
        "marktlokationsId": "51238696782"
    }))
    .expect_err("the check digit is wrong");
    println!("  wrong BDEW check digit    → {err}");
    bad += check("code is bo4e.schema", err.code() == "bo4e.schema");
    bad
}

// ── Stage 3: strict enums ────────────────────────────────────────────────────

/// The stage that most needs to be unmissable.
///
/// BO4E's forward compatibility cuts both ways: every enum carries an `Unknown`
/// catch-all, so an unrecognised value **decodes** rather than failing — and
/// `Unknown` serialises back as the literal string `"UNKNOWN"`. At an endpoint
/// that stores the canonical round-trip rather than the request body, a typo is
/// therefore not merely accepted, it *overwrites* what the caller sent.
///
/// It is not a courtesy, either. `go-bo4e`'s generated `UnmarshalJSON` returns
/// `invalid Sparte %q` and has no catch-all variant at all; BO4E-python's
/// pydantic `StrEnum` raises. Both reject the **whole document**.
fn stage_3_strict_enums() -> usize {
    section("3. Strict enums");
    let mut bad = 0;
    let err = bo4e::decode::<Marktlokation>(serde_json::json!({
        "marktlokationsId": "51238696781",
        "sparte": "STROMM"
    }))
    .expect_err("STROMM is not a Sparte");
    println!("  `sparte: \"STROMM\"`        → {err}");
    bad += check(
        "code is bo4e.unknown_enum",
        err.code() == "bo4e.unknown_enum",
    );
    bad += check("the path is named", err.to_json()["paths"][0] == "sparte");

    // At any depth, by JSON-path — this one is two levels down.
    let err = bo4e::decode::<Geschaeftspartner>(serde_json::json!({
        "kontaktwege": [{ "kontaktart": "BRIEFTAUBE", "kontaktwert": "—" }]
    }))
    .expect_err("BRIEFTAUBE is not a Kontaktart");
    println!("  nested, two levels down   → {}", err.to_json()["paths"]);
    bad += check(
        "the path locates it",
        err.to_json()["paths"][0]
            .as_str()
            .is_some_and(|p| p.contains("kontaktart")),
    );
    bad
}

// ── Stage 4: the rules BO4E states and enforces nowhere ──────────────────────

/// `rubo4e`'s derived validators over the whole tree, plus the two residual
/// rules mako carries because the standard states them in prose and the schema
/// cannot.
fn stage_4_rules() -> usize {
    section("4. BO4E rules");
    let mut bad = 0;
    // `gesamtnetto`: „Die Summe der Nettobeträge der Rechnungsteile."
    let err = bo4e::decode::<Rechnung>(serde_json::json!({
        "rechnungsnummer": "RE-2026-0001",
        "gesamtnetto": { "wert": "100.00", "waehrung": "EUR" },
        "rechnungspositionen": [{
            "positionsnummer": 1,
            "gesamtpreis": { "wert": "42.00", "waehrung": "EUR" }
        }]
    }))
    .expect_err("the positions do not sum to the stated net");
    println!("  positions ≠ gesamtnetto   → {err}");
    bad += check("code is bo4e.rule", err.code() == "bo4e.rule");
    bad += check(
        "each broken rule names its path",
        err.to_json()["failures"][0]["path"].is_string(),
    );
    bad
}

// ── The two variants ─────────────────────────────────────────────────────────

/// A market document a counterparty sent: stages 1–3 refuse, stage 4 **does
/// not**.
///
/// An invoice whose `gesamtbrutto` is not net plus tax is *disputable*, not
/// unreadable, and the market's answer to it is a REMADV naming the defect.
/// Refusing to parse would replace that answer with silence and a dead letter
/// for an operator to find — mako would fall silent exactly where the process
/// requires it to speak.
fn a_received_document_is_read_and_disputed() -> usize {
    section("decode_received — readable, and wrong");
    let mut bad = 0;
    let (rechnung, violations) = bo4e::decode_received::<Rechnung>(serde_json::json!({
        "rechnungsnummer": "RE-2026-0001",
        "gesamtnetto": { "wert": "100.00", "waehrung": "EUR" },
        "rechnungspositionen": [{
            "positionsnummer": 1,
            "gesamtpreis": { "wert": "42.00", "waehrung": "EUR" }
        }]
    }))
    .expect("stages 1-3 pass, so the document is readable");
    println!(
        "  the same invoice          → read as {:?}, with {} violation(s) to answer",
        rechnung.rechnungsnummer.as_deref().unwrap_or("?"),
        violations.len()
    );
    bad += check("the value comes back", rechnung.rechnungsnummer.is_some());
    bad += check("the violation comes with it", !violations.is_empty());
    bad
}

/// The outbound gate: stages 3 and 4 on a value mako **built** — the first two
/// are the compiler's job there — plus one check that belongs on this side
/// only.
///
/// `ensure_no_extension_data` refuses a field no BO4E schema declares. Inbound
/// that would throw away the forward compatibility `_additional` exists for: a
/// sender one release ahead is to be read, not rejected. On a document mako
/// authored it can only be a mistake — and it is the mistake nothing else can
/// see, because a decode round-trip returns `Ok` for a misspelled key and reads
/// the field back as `None`.
fn the_outbound_gate_is_stricter() -> usize {
    section("ensure_conformant — mako never sends what it would refuse");
    let mut bad = 0;
    let gp = Geschaeftspartner {
        organisationsname: Some("Demo Energie GmbH".to_owned()),
        ..Default::default()
    };
    bad += check(
        "a typed value mako built passes",
        bo4e::ensure_conformant(&gp).is_ok(),
    );
    println!("  a typed Geschaeftspartner → conformant");

    // The same object with a field BO4E does not define, which is what a
    // hand-assembled `json!` produces and what a rename leaves behind.
    let with_stray: Geschaeftspartner = serde_json::from_value(serde_json::json!({
        "organisationsname": "Demo Energie GmbH",
        "kundennummer": "K-4711"
    }))
    .expect("serde absorbs the unknown key into `_additional`");
    println!(
        "  after a decode round-trip → `kundennummer` reads back as {:?} (absorbed, not refused)",
        with_stray.zusatz_attribute
    );
    let err = bo4e::ensure_conformant(&with_stray).expect_err("outbound refuses an unknown field");
    println!("  outbound                  → {err}");
    bad += check(
        "code is bo4e.unknown_field",
        err.code() == "bo4e.unknown_field",
    );
    bad
}

// ── The type that cannot skip the gate ───────────────────────────────────────

/// `Bo4e<T>` is `decode` moved into the type.
///
/// A request struct declaring `serde_json::Value` and a doc comment saying
/// "full BO4E payload" is a document nothing checked — six of those were live
/// in this tree at once, and nothing distinguished them from the twenty that
/// got it right except a line in a handler. There is no constructor for
/// `Bo4e<T>` that skips the gate: `Deserialize` is the only way in from
/// untrusted JSON, and it *is* the gate.
fn the_type_that_cannot_skip_the_gate() -> usize {
    section("Bo4e<T> — the gate in the type");
    let mut bad = 0;

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct UpsertRequest {
        sparte: String,
        /// Full BO4E `Marktlokation`.
        data: Bo4e<Marktlokation>,
    }

    let req: UpsertRequest = serde_json::from_value(serde_json::json!({
        "sparte": "STROM",
        "data": { "marktlokationsId": "51238696781", "netzebene": "NSP" }
    }))
    .expect("a valid request");
    println!(
        "  request deserialised      → sparte={}, MaLo={:?}",
        req.sparte,
        req.data.marktlokations_id.as_deref().unwrap_or("?")
    );
    bad += check(
        "`canonical_json` is what to store, and it carries the discriminant",
        req.data.canonical_json().expect("serialisable")["_typ"] == "MARKTLOKATION",
    );

    // The same request with an out-of-schema enum never becomes a value.
    let err = serde_json::from_value::<UpsertRequest>(serde_json::json!({
        "sparte": "STROM",
        "data": { "marktlokationsId": "51238696781", "netzebene": "NIEDERSPANNUNG" }
    }))
    .expect_err("NIEDERSPANNUNG is not a Netzebene");
    let (sentence, detail) = mako_markt::bo4e::recover_rejection(&err.to_string())
        .expect("the rejection survives serde, so a 422 keeps its keys");
    println!("  bad enum in `data`        → {sentence}");
    println!("                              {detail}");
    bad += check("the stage survives", detail["code"] == "bo4e.unknown_enum");

    // …and `deny_unknown_fields` catches the other half: without it, a field
    // the API does not have is accepted and silently dropped.
    let err = serde_json::from_value::<UpsertRequest>(serde_json::json!({
        "sparte": "STROM", "data": {}, "bo4e_version": "202607.1.0"
    }))
    .expect_err("this request has no `bo4e_version` field");
    println!("  a field the API lacks     → {err}");
    bad += check(
        "named, not dropped",
        err.to_string().contains("bo4e_version"),
    );
    bad
}

// ── Reporting ────────────────────────────────────────────────────────────────

fn section(title: &str) {
    println!("\n── {title} ──");
}

fn check(what: &str, ok: bool) -> usize {
    if ok {
        0
    } else {
        println!("  ✗ FAILED: {what}");
        1
    }
}
