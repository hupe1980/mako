//! Guards `processd`'s SQL `CHECK` lists against the Rust that writes them.
//!
//! Nine columns across four tables. Two things make this service the awkward
//! one, and both are reasons a guard here could pass while looking at nothing:
//!
//! - **Three tables carry a `status` column**, with three unrelated
//!   vocabularies. An unscoped lookup answers with the first match in the file,
//!   so every assertion would silently be about `approval_queue`.
//! - **`eog_art` writes the nullable form backwards** —
//!   `CHECK (eog_art IN (…) OR eog_art IS NULL)`. A parser anchored on the bare
//!   `CHECK (c IN (` then scans past the constraint's own `)` hunting for `))`.
//!
//! Both are handled in `mako_service::schema_check`, which is why the parser
//! lives there rather than here.
//!
//! # Where the values come from
//!
//! Only two columns are written from an enum through a bind (`approval_queue.status`,
//! `anmeldung_decisions.decision`). The rest are inline SQL literals or a value
//! parsed at ingest. For those the guard holds the `CHECK` list against the
//! vocabulary the crate declares — and, where the writes are inline literals,
//! against the literals themselves, so the declaration cannot become
//! decorative.

use mako_service::schema_check::{assert_agrees_in, check_values_in};

// The `neuanlage` modules are gated on the NB role — `E_0608` is an NB tree, so
// without it there is no Prüflauf to remember. The migration is one file whatever
// the feature set, so the `CHECK` assertions below stay ungated; only the two
// tests that name the Rust are.
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
use processd::pg::neuanlage::NeuanlageStatus;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");

/// Rust source with `//`-comments removed.
///
/// The two source scans below look for SQL fragments in code, and this file's
/// own prose contains both of the things they look for — a doc comment showing
/// `status = '…'`, and the comment recording why the `to_uppercase()` catch-all
/// was removed. Scanning raw text finds those and reports a defect that is a
/// sentence.
fn code_of(src: &str) -> String {
    src.lines()
        .map(|l| l.split_once("//").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
fn neuanlage_code() -> String {
    code_of(include_str!("../src/pg/neuanlage.rs"))
}

fn eog_code() -> String {
    code_of(include_str!("../src/eog_module.rs"))
}

/// `QueueStatus` reaches the column through `Display`.
#[test]
fn the_queue_status_list_matches_its_enum() {
    assert_agrees_in(
        SCHEMA,
        "approval_queue",
        "status",
        &["Pending", "Approved", "Rejected", "Expired"],
    );
}

/// `AnmeldungDecision` reaches the column through `as_str`.
#[test]
fn the_anmeldung_decision_list_matches_its_enum() {
    assert_agrees_in(
        SCHEMA,
        "anmeldung_decisions",
        "decision",
        &["Accept", "Reject", "Escalate"],
    );
}

/// `mako_markt::domain::Sparte` has exactly these two.
#[test]
fn the_eog_sparte_list_matches_the_domain_enum() {
    assert_agrees_in(SCHEMA, "eog_activations", "sparte", &["STROM", "GAS"]);
}

/// The EoG lifecycle, written as inline SQL literals in `eog_module`.
#[test]
fn the_eog_status_list_matches_the_transitions() {
    assert_agrees_in(
        SCHEMA,
        "eog_activations",
        "status",
        &[
            "detected",
            "angemeldet",
            "active",
            "expiring",
            "expired",
            "closed",
        ],
    );
}

/// The two statutory fallback regimes, and the backwards nullable spelling.
///
/// `Versorgungsart` has a third value — `ZE3` Ersatzbelieferung, which is also
/// how the §38a Übergangsversorgung arrives — that this column deliberately does
/// not admit: it is a contract regime outside the statutory fallback, `marktd`
/// does not emit it, and `eog_module` drops it with a warning rather than
/// letting the column refuse the whole activation.
#[test]
fn the_eog_art_list_is_the_two_statutory_regimes() {
    let listed = check_values_in(SCHEMA, "eog_activations", "eog_art");
    assert_eq!(
        listed,
        ["ERSATZVERSORGUNG", "GRUNDVERSORGUNG"],
        "the `IN (…) OR col IS NULL` spelling must not run past the list"
    );
    assert!(
        !eog_code().contains("to_uppercase()"),
        "`eog_art` is CHECK-constrained, so a catch-all that upper-cases whatever arrived \
         writes a value the column refuses — and a refused INSERT loses the § 38 Abs. 4 \
         activation entirely"
    );
}

/// An **integer** CHECK list, held against the constant that gates the ingest.
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
#[test]
fn the_neuanlage_pid_list_matches_the_accept_gate() {
    let listed = check_values_in(SCHEMA, "neuanlage_faelle", "pid");
    let gated: Vec<String> = processd::neuanlage_module::NEUANLAGE_PIDS
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        listed, gated,
        "the `pid` CHECK and `NEUANLAGE_PIDS` disagree — the parse gate would open a case \
         the column then refuses"
    );
}

/// The two Marktlokationsarten the case store distinguishes.
///
/// `mako_pruefung`'s enum has three; `Ruhend` is stored as `VERBRAUCHEND` on
/// purpose (`E_0622` Prüfschritt 10 splits only two ways), which is why this is
/// pinned against the column's own pair rather than against the enum.
#[test]
fn the_marktlokationsart_list_is_the_stored_pair() {
    assert_agrees_in(
        SCHEMA,
        "neuanlage_faelle",
        "marktlokationsart",
        &["VERBRAUCHEND", "ERZEUGEND"],
    );
}

/// The four published `CCI+Z22` codes, held against the type that parses them.
///
/// This column is why the ingest parses rather than passing the wire string
/// through: a fifth code reached the INSERT verbatim, Postgres refused the
/// statement, and the LF's Anmeldung was never answered at all.
#[test]
fn the_veraeusserungsform_list_matches_the_wire_codes() {
    use mako_pruefung::nb::types::Veraeusserungsform;
    let written: Vec<&str> = [
        Veraeusserungsform::Einspeiseverguetung,
        Veraeusserungsform::Marktpraemie,
        Veraeusserungsform::SonstigeDirektvermarktung,
        Veraeusserungsform::KwkgVerguetung,
    ]
    .into_iter()
    .map(Veraeusserungsform::wire_code)
    .collect();
    assert_agrees_in(SCHEMA, "neuanlage_faelle", "veraeusserungsform", &written);

    for value in check_values_in(SCHEMA, "neuanlage_faelle", "veraeusserungsform") {
        assert!(
            Veraeusserungsform::from_wire_code(&value).is_some(),
            "the column allows {value:?}, which `from_wire_code` refuses — the ingest \
             would drop it and the case would open without a Veräußerungsform"
        );
    }
}

/// The Neuanlage case lifecycle, held against the declared vocabulary **and**
/// the literals that actually write it.
///
/// `NeuanlageStatus` is not on the write path — the statements set `status`
/// with an inline SQL literal — so pinning the `CHECK` against the enum alone
/// would prove nothing about what is written. Both halves are asserted, which
/// is what stops the enum being decorative.
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
#[test]
fn the_neuanlage_status_list_matches_the_enum_and_the_literals() {
    let declared: Vec<&str> = NeuanlageStatus::ALL.iter().map(|s| s.as_str()).collect();
    assert_agrees_in(SCHEMA, "neuanlage_faelle", "status", &declared);

    let code = neuanlage_code();
    for state in &declared {
        assert!(
            code.contains(&format!("status = '{state}'")),
            "no statement writes or reads `status = '{state}'` — the enum declares a \
             state the queries do not use"
        );
    }
    // And no literal outside the declared set.
    for hit in code.match_indices("status = '") {
        let rest = &code[hit.0 + "status = '".len()..];
        let value = &rest[..rest.find('\'').expect("closing quote")];
        assert!(
            declared.contains(&value),
            "a statement names `status = '{value}'`, which `NeuanlageStatus` does not \
             declare and the column would refuse"
        );
    }
}

/// Three tables carry `status`, and each assertion above is about its own.
#[test]
fn the_three_status_columns_are_read_separately() {
    let queue = check_values_in(SCHEMA, "approval_queue", "status");
    let eog = check_values_in(SCHEMA, "eog_activations", "status");
    let neuanlage = check_values_in(SCHEMA, "neuanlage_faelle", "status");
    assert_eq!((queue.len(), eog.len(), neuanlage.len()), (4, 6, 3));
    assert_ne!(queue, eog);
    assert_ne!(eog, neuanlage);
}
