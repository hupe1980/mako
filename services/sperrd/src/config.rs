//! Configuration for `sperrd`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SperrdConfig {
    pub database_url: String,
    pub port: Option<u16>,
    /// `makod` base URL — used to dispatch IFTSTA 21039 on execution confirmation.
    pub makod_url: String,
    pub makod_api_key: String,
}
