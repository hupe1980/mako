//! Read a SQL `CHECK (<column> IN (…))` list out of a migration, so a test can
//! hold it against the Rust that writes the column.
//!
//! A `CHECK` list is opaque text to the compiler. Nothing ties it to the enum or
//! constant on the other side of the column, and drift is silent in **both**
//! directions — which fail differently and want different assertions:
//!
//! - A value the Rust writes and the list omits is a write Postgres refuses.
//!   Loud, but at the customer rather than in CI.
//! - A value the list allows and nothing writes is a claim the schema makes and
//!   the code does not honour. That is either a missing feature or a category
//!   error, and it should have to be one on purpose. `invoicd` carried a
//!   `'Paid'` outcome for exactly this reason.
//! - A value the list allows and the Rust *decoder* does not know is the worst:
//!   it decodes to whatever the fallback says. `vertragd`'s `Vertragsart::from_db`
//!   answers `Sondervertrag` for anything unrecognised — deliberately, so a typo
//!   cannot grant Grundversorgungs-Fristen — so a drifted value silently
//!   reclassifies a contract's statutory regime instead of failing.
//!
//! # Why this is shared rather than copied
//!
//! The parser is the part that goes wrong, and it goes wrong by *passing*. Four
//! services had grown their own copy and each copy knew a different subset of
//! the spellings these migrations actually use:
//!
//! - `CHECK (c IN (…))` and `CHECK (c IS NULL OR c IN (…))` — a parser knowing
//!   one reports "no CHECK list" for every column written the other way.
//! - `CHECK (c IN (…) OR c IS NULL)` — the same thing backwards, which `processd`
//!   writes. A parser anchored on the bare form then scans past the constraint's
//!   own `)` looking for `))`.
//! - Unquoted integers (`pid IN (55600, 55601)`) — a parser that filters for
//!   `'…'` tokens returns an empty list, which reads as a clean pass.
//! - `--` comments inside the list, and the alignment whitespace every migration
//!   here uses.
//!
//! Every one of those makes a guard succeed while looking at nothing, so
//! [`check_values`] refuses an empty result rather than returning it.

use std::collections::BTreeSet;

/// The values of a `CHECK (<column> …IN (…))` list, in the order written.
///
/// Handles the three spellings and both token kinds; strips `--` comments and
/// collapses alignment whitespace first.
///
/// # Panics
///
/// Panics when the column has no `CHECK` list, when the list is unterminated, or
/// when it parses to nothing. All three mean the guard is not looking at what it
/// believes it is, and returning an empty list would read as a pass.
#[must_use]
pub fn check_values(sql: &str, column: &str) -> Vec<String> {
    let flat = flatten(sql);
    let (start, end) = locate(&flat, column);
    let values: Vec<String> = flat[start..end]
        .split(',')
        .filter_map(|tok| {
            let t = tok.trim();
            t.strip_prefix('\'')
                .and_then(|t| t.strip_suffix('\''))
                .map(ToOwned::to_owned)
                // An unquoted integer is a value too — `pid IN (55600, 55601)`.
                .or_else(|| t.chars().all(|c| c.is_ascii_digit()).then(|| t.to_owned()))
        })
        .filter(|v| !v.is_empty())
        .collect();
    assert!(
        !values.is_empty(),
        "the CHECK list for `{column}` parsed to nothing — the anchor matched but the \
         values did not, which is a guard that passes by not looking"
    );
    values
}

/// [`check_values`], scoped to one table's `CREATE TABLE` block.
///
/// Column names repeat across tables — `processd` has three `status` columns
/// with three different vocabularies — and [`check_values`] answers with the
/// first match in the file. A guard that asked the unscoped question would
/// silently check the wrong table and pass. Prefer this wherever the name is
/// not unique, which in practice is `status`, `pid` and `sparte`.
///
/// # Panics
///
/// Panics when the table is absent or its block is unterminated, for the same
/// reason [`check_values`] panics on a missed anchor.
#[must_use]
pub fn check_values_in(sql: &str, table: &str, column: &str) -> Vec<String> {
    check_values(&table_block(sql, table), column)
}

/// [`check_values`] as a set, for order-insensitive comparison.
#[must_use]
pub fn check_value_set(sql: &str, column: &str) -> BTreeSet<String> {
    check_values(sql, column).into_iter().collect()
}

/// [`check_values_in`] as a set.
#[must_use]
pub fn check_value_set_in(sql: &str, table: &str, column: &str) -> BTreeSet<String> {
    check_values_in(sql, table, column).into_iter().collect()
}

/// The `CREATE TABLE <table> ( … );` block, brackets balanced.
fn table_block(sql: &str, table: &str) -> String {
    let flat = flatten(sql);
    let anchor = format!("CREATE TABLE {table} (");
    let at = flat.find(&anchor).unwrap_or_else(|| {
        panic!(
            "no `CREATE TABLE {table} (` in this migration — the table may have been \
             renamed, and a missed anchor reads as a guard that passes"
        )
    });
    let start = at + anchor.len();
    let mut depth = 1usize;
    for (i, c) in flat[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return flat[start..start + i].to_owned();
                }
            }
            _ => {}
        }
    }
    panic!("unterminated `CREATE TABLE {table}` block")
}

/// [`assert_agrees`], scoped to one table. Prefer it for a repeated column name.
///
/// # Panics
///
/// Panics with the offending value when the two disagree.
pub fn assert_agrees_in<S: AsRef<str>>(sql: &str, table: &str, column: &str, written: &[S]) {
    assert_agrees(&table_block(sql, table), column, written);
}

/// Hold a `CHECK` list against the values some Rust writes, in both directions.
///
/// `written` is every value the service can produce for that column. Both
/// directions are asserted because both have shipped broken, and the message
/// names which one failed.
///
/// # Panics
///
/// Panics with the offending value when the two disagree.
pub fn assert_agrees<S: AsRef<str>>(sql: &str, column: &str, written: &[S]) {
    let listed = check_value_set(sql, column);
    let written: BTreeSet<&str> = written.iter().map(AsRef::as_ref).collect();

    for w in &written {
        assert!(
            listed.contains(*w),
            "`{column}`: the CHECK list does not allow {w:?}, which this service writes — \
             Postgres would refuse the statement. Listed: {listed:?}"
        );
    }
    for l in &listed {
        assert!(
            written.contains(l.as_str()),
            "`{column}`: the CHECK list allows {l:?} and nothing writes it — remove it, or \
             write it. Written: {written:?}"
        );
    }
}

/// Collapse a migration to one line the anchors can be found in.
///
/// Comments first: these migrations annotate most list entries with `-- …`, and
/// a comment can contain a comma or a quote.
fn flatten(sql: &str) -> String {
    let uncommented: String = sql
        .lines()
        .map(|l| l.split_once("--").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    uncommented.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Byte range of the value list for `column`, exclusive of the brackets.
///
/// The terminator is the `)` that closes the `IN (`, found by counting depth —
/// not the literal `))`, which is absent when the constraint continues
/// (`IN (…) OR c IS NULL`) and present early when a value contains one.
fn locate(flat: &str, column: &str) -> (usize, usize) {
    let anchors = [
        format!("CHECK ({column} IS NULL OR {column} IN ("),
        format!("CHECK ({column} IN ("),
    ];
    let (anchor, at) = anchors
        .iter()
        .find_map(|a| flat.find(a.as_str()).map(|i| (a, i)))
        .unwrap_or_else(|| {
            panic!(
                "no CHECK list found for column `{column}` — the migration may spell it \
                 differently, and a missed anchor reads as a guard that passes"
            )
        });
    let start = at + anchor.len();
    let mut depth = 1usize;
    for (i, c) in flat[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (start, start + i);
                }
            }
            _ => {}
        }
    }
    panic!("unterminated CHECK list for `{column}`")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nullable form, both ways round. `processd` writes the second.
    #[test]
    fn both_nullable_spellings_parse() {
        let a = "x TEXT CHECK (eog_art IS NULL OR eog_art IN ('A','B')),";
        let b = "x TEXT CHECK (eog_art IN ('A', 'B') OR eog_art IS NULL),";
        assert_eq!(check_values(a, "eog_art"), ["A", "B"]);
        assert_eq!(
            check_values(b, "eog_art"),
            ["A", "B"],
            "the trailing `OR … IS NULL` form must not run past the list"
        );
    }

    /// Unquoted integers are values. `neuanlage_faelle.pid` and
    /// `invoice_drafts.pid` are written this way.
    #[test]
    fn an_integer_list_parses() {
        let sql = "pid SMALLINT NOT NULL CHECK (pid IN (55600, 55601)),";
        assert_eq!(check_values(sql, "pid"), ["55600", "55601"]);
    }

    /// Comments, wrapping and alignment are all stripped before matching.
    #[test]
    fn comments_and_wrapping_do_not_hide_the_list() {
        let sql = "\
    status   TEXT   NOT NULL   CHECK (status IN (
                 'pending',    -- not yet, submitted
                 'submitted'   -- sent to the ÜNB
             )),";
        assert_eq!(check_values(sql, "status"), ["pending", "submitted"]);
    }

    /// A nested bracket inside the constraint does not end the list early.
    #[test]
    fn a_nested_bracket_does_not_terminate_the_list() {
        let sql = "c TEXT CHECK (c IN ('a','b')) , d TEXT CHECK (d IN ('x')),";
        assert_eq!(check_values(sql, "c"), ["a", "b"]);
        assert_eq!(check_values(sql, "d"), ["x"]);
    }

    /// A column whose CHECK is absent is a panic, never an empty pass.
    #[test]
    #[should_panic(expected = "no CHECK list found for column `nope`")]
    fn a_missing_check_is_refused() {
        let _ = check_values("x TEXT CHECK (other IN ('A')),", "nope");
    }

    /// An anchor that matches but yields nothing is a panic too.
    #[test]
    #[should_panic(expected = "parsed to nothing")]
    fn an_empty_list_is_refused() {
        let _ = check_values("x TEXT CHECK (x IN ()),", "x");
    }

    /// Both directions of disagreement are named.
    #[test]
    #[should_panic(expected = "does not allow \"C\"")]
    fn a_written_value_the_list_omits_is_named() {
        assert_agrees("x TEXT CHECK (x IN ('A','B')),", "x", &["A", "B", "C"]);
    }

    #[test]
    #[should_panic(expected = "allows \"B\" and nothing writes it")]
    fn a_listed_value_nothing_writes_is_named() {
        assert_agrees("x TEXT CHECK (x IN ('A','B')),", "x", &["A"]);
    }

    /// A repeated column name resolves per table, not to the first match.
    #[test]
    fn a_repeated_column_resolves_within_its_table() {
        let sql = "\
CREATE TABLE a (
    status TEXT NOT NULL CHECK (status IN ('open','shut'))
);
CREATE TABLE b (
    status TEXT NOT NULL CHECK (status IN ('pending','submitted'))
);";
        assert_eq!(check_values_in(sql, "a", "status"), ["open", "shut"]);
        assert_eq!(
            check_values_in(sql, "b", "status"),
            ["pending", "submitted"]
        );
        assert_eq!(
            check_values(sql, "status"),
            ["open", "shut"],
            "the unscoped form answers with the first match — which is why the scoped \
             one exists"
        );
    }

    #[test]
    #[should_panic(expected = "no `CREATE TABLE gone (`")]
    fn a_missing_table_is_refused() {
        let _ = check_values_in("CREATE TABLE a (x TEXT CHECK (x IN ('A')));", "gone", "x");
    }

    #[test]
    fn agreement_is_order_insensitive() {
        assert_agrees("x TEXT CHECK (x IN ('A','B')),", "x", &["B", "A"]);
    }
}
