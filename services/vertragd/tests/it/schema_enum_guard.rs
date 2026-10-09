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

use mako_service::schema_check::check_values;
use vertragd::domain::{Kuendigungsgrund, Vertragsart};
use vertragd::outbound::TaskKind;

fn migration_sql() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0001_schema.sql");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
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

/// The three lists are the ones this file believes it is reading.
///
/// `mako_service::schema_check` refuses an empty parse, so this is not about
/// that — it is about the anchors resolving to *these* columns.
#[test]
fn the_guard_reads_the_lists_it_names() {
    let sql = migration_sql();
    assert_eq!(check_values(&sql, "kind").len(), 5);
    assert_eq!(check_values(&sql, "kuendigung_grund").len(), 4);
    assert_eq!(check_values(&sql, "vertragsart").len(), 3);
}
