//! SQL built at runtime.
//!
//! sqlx refuses a query string that is not `'static` unless it is wrapped in
//! `AssertSqlSafe`, because a string assembled from input is how SQL injection
//! happens. mako builds SQL at runtime only from its own fragments — a table
//! name from a fixed list, an `IN` list of `$n` placeholders, a `GRANT` naming a
//! configured role — never from a request value, which is always bound.
//! [`dynamic`] is the one place that assertion is made, so every such site is a
//! call to it, and `cargo xtask check-dynamic-sql` holds each file to the number
//! of calls its listed reason covers.

/// Mark `sql`, built from mako's own fragments with every value bound, as safe
/// to execute.
#[must_use]
pub fn dynamic(sql: impl Into<String>) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(sql.into())
}
