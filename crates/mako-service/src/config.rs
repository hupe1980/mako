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

// ── Shared config structs ─────────────────────────────────────────────────────

/// Standard PostgreSQL database configuration block.
///
/// All mako services that use a database embed this under `[database]`:
///
/// ```toml
/// [database]
/// url      = "env:DATABASE_URL"      # or literal postgres://...
/// pool_size = 10                      # optional, default 10
/// ```
///
/// Services re-export it as `pub use mako_service::config::DatabaseConfig;`
/// rather than defining an identical struct.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DatabaseConfig {
    /// PostgreSQL connection URL.
    ///
    /// Use `"env:DATABASE_URL"` to defer to the environment at runtime — this
    /// avoids storing secrets in TOML files checked into version control.
    pub url: String,

    /// Maximum number of connections in the pool.  Default: 10.
    #[serde(default = "DatabaseConfig::default_pool_size")]
    pub pool_size: u32,
}

impl DatabaseConfig {
    fn default_pool_size() -> u32 {
        10
    }
}

/// Standard HTTP listen-address configuration block.
///
/// Services that take `[http]` embed this struct rather than defining their own:
///
/// ```toml
/// [http]
/// addr = "0.0.0.0:8580"
/// ```
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HttpConfig {
    /// Socket address to bind.  Defaults to `"0.0.0.0:<default-port>"`.
    pub addr: String,
}

// ── Loader ────────────────────────────────────────────────────────────────────
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

// ── `env:` indirection ────────────────────────────────────────────────────────

/// An `env:VARNAME` reference whose variable is not set.
///
/// Carries only the variable name — never the value — so the error is safe to
/// log even when the reference points at a secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("environment variable {var:?} is referenced in the config but not set")]
pub struct EnvRefError {
    /// Name of the missing variable.
    pub var: String,
}

/// Resolve an `env:VARNAME` indirection to the variable's value.
///
/// A value without the prefix is returned unchanged, so a config may mix
/// literals and indirections freely.
///
/// A service that never calls this silently ships the placeholder as the value:
/// `api_key = "env:SVC_API_KEY"` becomes the literal bearer token
/// `env:SVC_API_KEY`, which authenticates against nothing and fails as a 401
/// rather than as a configuration error.
///
/// # Errors
///
/// Returns an error naming the variable when it is referenced but unset —
/// failing at startup rather than on the first request that needs it.
pub fn resolve_env(value: &str) -> Result<String, EnvRefError> {
    match value.strip_prefix("env:") {
        Some(var) => std::env::var(var).map_err(|_| EnvRefError {
            var: var.to_owned(),
        }),
        None => Ok(value.to_owned()),
    }
}

/// [`resolve_env`], wrapping the result in a [`secrecy::SecretString`].
///
/// # Errors
///
/// As [`resolve_env`].
pub fn resolve_env_secret(value: &str) -> Result<secrecy::SecretString, EnvRefError> {
    resolve_env(value).map(secrecy::SecretString::from)
}

#[cfg(test)]
mod env_indirection_tests {
    use super::*;

    #[test]
    fn a_literal_passes_through_unchanged() {
        assert_eq!(resolve_env("plain-value").unwrap(), "plain-value");
    }

    #[test]
    fn an_unset_variable_names_itself_in_the_error() {
        let err = resolve_env("env:MAKO_TEST_DEFINITELY_UNSET").unwrap_err();
        assert!(
            err.to_string().contains("MAKO_TEST_DEFINITELY_UNSET"),
            "the error must name the missing variable, got: {err}"
        );
    }

    #[test]
    fn a_value_that_merely_contains_env_is_not_an_indirection() {
        // Only a `env:` *prefix* indirects; a secret containing the substring
        // must survive verbatim.
        assert_eq!(resolve_env("prod-env:key").unwrap(), "prod-env:key");
    }
}
