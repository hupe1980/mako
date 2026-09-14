//! Guards `vertragd`'s SQL `CHECK` lists against the Rust enums that write them.
//!
//! A `CHECK` list is opaque text to the compiler, so nothing otherwise ties it
//! to the enum on the other side of the column. Drift is silent in both
//! directions and the two directions fail differently:
//!
//! - A variant the `CHECK` does not list is a **write** that Postgres refuses.
//!   Loud, but at the customer rather than in CI.
//! - A value the `CHECK` allows and the enum does not know is a **read** that
//!   decodes to whatever the fallback says. `Vertragsart::from_db` answers
//!   `Sondervertrag` for anything it does not recognise — deliberately, so a
//!   typo cannot grant Grundversorgungs-Fristen — which means a drifted column
//!   value silently reclassifies a contract's statutory regime instead of
//!   failing. `Kuendigungsgrund::from_db` has the same shape.
//!
//! So the enum is the authority and the list is proved against it, both ways:
//! every variant appears, and every listed value decodes back to the variant
//! that wrote it.

use std::path::PathBuf;

use vertragd::domain::{Kuendigungsgrund, Vertragsart};
use vertragd::outbound::TaskKind;

fn migration_sql() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_schema.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The quoted values of the `CHECK (<column> ... IN (…))` list.
///
/// Both spellings the migration uses are accepted — the nullable
/// `CHECK (col IS NULL OR col IN (…))` and the bare `CHECK (col IN (…))`. A
/// parser knowing only one would report "no CHECK list" for every column
/// written the other way, which is a guard that passes by not looking.
///
/// Whitespace is collapsed first: the migration aligns columns with runs of
/// spaces and wraps long lists over several lines, so an anchor written with
/// single spaces would miss by one character. SQL line comments go with it —
/// this schema annotates most list entries with `-- …`.
fn check_values(sql: &str, column: &str) -> Vec<String> {
    let uncommented: String = sql
        .lines()
        .map(|l| l.split_once("--").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    let sql: String = uncommented.split_whitespace().collect::<Vec<_>>().join(" ");

    let anchors = [
        format!("CHECK ({column} IS NULL OR {column} IN ("),
        format!("CHECK ({column} IN ("),
    ];
    let (anchor, at) = anchors
        .iter()
        .find_map(|a| sql.find(a.as_str()).map(|i| (a, i)))
        .unwrap_or_else(|| panic!("no CHECK list found for column `{column}`"));
    let start = at + anchor.len();
    let end = start
        + sql[start..]
            .find("))")
            .unwrap_or_else(|| panic!("unterminated CHECK list for `{column}`"));

    let values: Vec<String> = sql[start..end]
        .split(',')
        .filter_map(|tok| {
            let t = tok.trim();
            t.strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
                .map(ToOwned::to_owned)
        })
        .collect();
    assert!(
        !values.is_empty(),
        "the CHECK list for `{column}` parsed to nothing — the anchor matched but the \
         values did not, which is a guard that passes by not looking"
    );
    values
}

/// Hold one enum against one column, in both directions.
fn assert_pinned<T: Copy + PartialEq + std::fmt::Debug>(
    column: &str,
    all: &[T],
    as_db: fn(T) -> &'static str,
    from_db: fn(&str) -> Option<T>,
) {
    let listed = check_values(&migration_sql(), column);

    for v in all {
        assert!(
            listed.iter().any(|l| l == as_db(*v)),
            "`{column}` CHECK does not list {:?} (`{}`) — writing it would be refused by \
             Postgres. Listed: {listed:?}",
            v,
            as_db(*v)
        );
    }

    for value in &listed {
        let decoded = from_db(value).unwrap_or_else(|| {
            panic!(
                "`{column}` CHECK allows `{value}`, which no variant of this enum writes — a \
                 row holding it decodes to the fallback rather than failing"
            )
        });
        assert_eq!(
            as_db(decoded),
            value.as_str(),
            "`{column}` value `{value}` decodes to {decoded:?}, which writes back as \
             `{}` — the round trip does not close",
            as_db(decoded)
        );
    }

    assert_eq!(
        listed.len(),
        all.len(),
        "`{column}` CHECK lists {} value(s) against {} enum variant(s): {listed:?}",
        listed.len(),
        all.len()
    );
}

/// `Vertragsart` decides every statutory deadline, and its fallback hides drift.
#[test]
fn vertragsart_matches_its_check_list() {
    assert_pinned(
        "vertragsart",
        Vertragsart::ALL,
        Vertragsart::as_db,
        // `from_db` answers `Sondervertrag` for anything, so it cannot report an
        // unknown. Round-tripping through `as_db` is what detects one.
        |s| {
            let v = Vertragsart::from_db(s);
            (v.as_db() == s).then_some(v)
        },
    );
}

/// The Kündigungsgrund, not the contract, decides the notice period.
#[test]
fn kuendigungsgrund_matches_its_check_list() {
    assert_pinned(
        "kuendigung_grund",
        Kuendigungsgrund::ALL,
        Kuendigungsgrund::as_db,
        |s| {
            let v = Kuendigungsgrund::from_db(s);
            (v.as_db() == s).then_some(v)
        },
    );
}

/// `kind` drives the outbound task dispatcher; an unlisted variant cannot be
/// enqueued at all.
#[test]
fn task_kind_matches_its_check_list() {
    assert_pinned("kind", TaskKind::ALL, TaskKind::as_db, |s| {
        TaskKind::ALL.iter().copied().find(|k| k.as_db() == s)
    });
}

/// The parser reads a real list, so a silent "no values" cannot pass.
///
/// Both spellings are exercised: `kind` is the bare form and `kuendigung_grund`
/// the nullable one.
#[test]
fn the_parser_reads_both_check_spellings() {
    let sql = migration_sql();
    assert_eq!(check_values(&sql, "kind").len(), 5);
    assert_eq!(check_values(&sql, "kuendigung_grund").len(), 4);
    assert_eq!(check_values(&sql, "vertragsart").len(), 3);
}
