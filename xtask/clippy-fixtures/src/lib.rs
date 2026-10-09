//! One use of every item the workspace `clippy.toml` disallows. Each use ends in
//! a marker comment naming the configured path it must be reported under;
//! `cargo xtask check-clippy-bans` holds the two in step.

use num_traits::FromPrimitive as _;

pub async fn bare_pool(url: &str) {
    let _ = sqlx::PgPool::connect(url).await; // BAN: sqlx::Pool::connect
    let opts: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    let _ = sqlx::PgPool::connect_with(opts).await; // BAN: sqlx::Pool::connect_with
}

pub fn runtime_backdoor() -> bool {
    tokio::runtime::Handle::try_current().is_ok() // BAN: tokio::runtime::Handle::try_current
}

pub fn money_through_floats(v: f64, w: f32) {
    let _ = rust_decimal::Decimal::from_f64_retain(v); // BAN: rust_decimal::Decimal::from_f64_retain
    let _ = rust_decimal::Decimal::from_f32_retain(w); // BAN: rust_decimal::Decimal::from_f32_retain
    let _ = rust_decimal::Decimal::from_f64(v); // BAN: num_traits::cast::FromPrimitive::from_f64
    let _ = rust_decimal::Decimal::from_f32(w); // BAN: num_traits::cast::FromPrimitive::from_f32
}

pub fn chrono_types(
    _a: chrono::DateTime<chrono::FixedOffset>, // BAN: chrono::DateTime
    _b: chrono::NaiveDate, // BAN: chrono::NaiveDate
    _c: chrono::NaiveDateTime, // BAN: chrono::NaiveDateTime
    _d: chrono::Utc, // BAN: chrono::Utc
    _e: chrono::Local, // BAN: chrono::Local
) {
}
