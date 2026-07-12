//! Layered TOML + environment-variable configuration loader.
//!
//! # File discovery
//!
//! `load_config("myservice")` looks for the config file in this order:
//! 1. `MYSERVICE_CONFIG` environment variable (absolute or relative path)
//! 2. `./myservice.toml` in the current working directory
//!
//! If the TOML file is absent the loader continues with env vars only — useful
//! for container environments where every value is injected via env.
//!
//! # Environment-variable overrides
//!
//! Every TOML key can be overridden via a service-prefixed environment variable.
//! Use **double-underscore** (`__`) as the TOML-section separator:
//!
//! ```text
//! # Overrides top-level `database_url` (flat config)
//! ACCOUNTINGD_DATABASE_URL=postgres://prod-host/accountingd
//!
//! # Overrides [database] url  (nested section)
//! INVOICD_DATABASE__URL=postgres://prod-host/invoicd
//!
//! # Overrides [makod] api_key  (nested section)
//! INVOICD_MAKOD__API_KEY=secret
//!
//! # Overrides [storage.postgres] url  (three levels)
//! MARKTD_STORAGE__POSTGRES__URL=postgres://prod-host/marktd
//! ```
//!
//! # Secret-file loading (`_FILE` suffix)
//!
//! Any env var whose name ends in `_FILE` is treated as a path to a file
//! whose **contents** become the config value — the `_FILE` suffix is stripped
//! from the key name.  This is the standard pattern for Kubernetes Secrets and
//! Docker Swarm secrets:
//!
//! ```text
//! # Equivalent to INVOICD_MAKOD__API_KEY=<file contents>
//! INVOICD_MAKOD__API_KEY_FILE=/run/secrets/makod-api-key
//!
//! # Equivalent to ACCOUNTINGD_DATABASE_URL=<file contents>
//! ACCOUNTINGD_DATABASE_URL_FILE=/run/secrets/db-url
//! ```
//!
//! The TOML file is loaded first (base layer); env vars take precedence.
//! This follows the [12-factor](https://12factor.net/config) methodology.
//!
//! # Example
//!
//! ```rust,no_run
//! use mako_service::load_config;
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Config {
//!     listen: String,
//! }
//!
//! let cfg: Config = load_config("myservice").unwrap();
//! ```

use std::path::PathBuf;

use serde::de::DeserializeOwned;

/// Configuration loading error — wraps [`figment::Error`].
pub type ConfigError = figment::Error;

/// Load a typed configuration `C` from a TOML file with an env-var layer on top.
///
/// See [module docs](self) for file-discovery rules and override conventions.
///
/// # Errors
///
/// Returns [`ConfigError`] when the TOML contains invalid syntax, a required
/// field is missing from both the file and env, or the values cannot be
/// deserialized into `C`.
#[allow(clippy::result_large_err)] // figment::Error is inherently large; loaded once at startup
pub fn load_config<C: DeserializeOwned>(name: &str) -> Result<C, ConfigError> {
    use figment::{
        Figment,
        providers::{Env, Format, Toml},
    };
    use figment_file_provider_adapter::FileAdapter;
    let path = config_path(name);
    let prefix = format!("{}_", name.to_uppercase().replace('-', "_"));
    Figment::new()
        .merge(FileAdapter::wrap(Toml::file(path)))
        .merge(FileAdapter::wrap(Env::prefixed(&prefix).split("__")))
        .extract()
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn config_path(name: &str) -> PathBuf {
    let env_key = format!("{}_CONFIG", name.to_uppercase().replace('-', "_"));
    if let Ok(p) = std::env::var(&env_key) {
        return PathBuf::from(p);
    }
    PathBuf::from(format!("{name}.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_default() {
        let p = config_path("myservice");
        assert_eq!(p, PathBuf::from("myservice.toml"));
    }

    #[test]
    fn config_path_hyphen_service() {
        // "nis-syncd" -> NIS_SYNCD_CONFIG (hyphens become underscores in env key).
        let p = config_path("nis-syncd");
        assert_eq!(p, PathBuf::from("nis-syncd.toml"));
    }
}
