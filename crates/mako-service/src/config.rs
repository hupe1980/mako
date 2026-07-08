//! TOML configuration loader with `env:VAR_NAME` resolution.
//!
//! # File discovery
//!
//! `load_config("myservice")` looks for the config file in this order:
//! 1. `MYSERVICE_CONFIG` environment variable (absolute or relative path)
//! 2. `./myservice.toml` in the current working directory
//!
//! # Env-var references
//!
//! Any string value in the TOML file that starts with `"env:"` is replaced
//! with the corresponding environment variable value at load time:
//!
//! ```toml
//! [storage.postgres]
//! url = "env:DATABASE_URL"
//!
//! [oidc]
//! client_secret = "env:OIDC_CLIENT_SECRET"
//! ```
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

/// Errors that can occur when loading a service configuration file.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The config file path could not be read.
    #[error("cannot read config file '{path}': {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// The TOML file is syntactically invalid.
    #[error("TOML parse error in '{path}': {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },

    /// An `"env:VAR_NAME"` reference in the config points to an unset variable.
    #[error("environment variable '{var}' referenced in config is not set")]
    EnvVarMissing { var: String },

    /// The resolved TOML value cannot be deserialized into the target type.
    #[error("config deserialization error in '{path}': {source}")]
    Deserialize {
        path: String,
        #[source]
        source: toml::de::Error,
    },

    /// Internal serialization error during env-var resolution round-trip.
    #[error("internal config processing error in '{path}': {message}")]
    Internal { path: String, message: String },
}

/// Load a typed configuration `C` from a TOML file.
///
/// See [module docs](self) for file discovery rules and env-var resolution.
///
/// # Errors
///
/// Returns [`ConfigError`] when the file is not found, contains invalid TOML,
/// references an unset environment variable, or cannot be deserialized into `C`.
pub fn load_config<C: DeserializeOwned>(name: &str) -> Result<C, ConfigError> {
    let path = config_path(name);
    let path_str = path.display().to_string();

    let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::Io {
        path: path_str.clone(),
        source,
    })?;

    let raw: toml::Value = content.parse().map_err(|source| ConfigError::Toml {
        path: path_str.clone(),
        source,
    })?;

    let resolved = resolve_env_vars(raw)?;

    // Serialize resolved value back to TOML string so we can use `toml::from_str`
    // which drives the standard serde path. This round-trip is negligible for
    // config loading and avoids depending on toml::Value as a Deserializer.
    let serialized = toml::to_string(&resolved).map_err(|e| ConfigError::Internal {
        path: path_str.clone(),
        message: e.to_string(),
    })?;

    toml::from_str(&serialized).map_err(|source| ConfigError::Deserialize {
        path: path_str,
        source,
    })
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn config_path(name: &str) -> PathBuf {
    // Check {NAME}_CONFIG env var first
    let env_key = format!("{}_CONFIG", name.to_uppercase().replace('-', "_"));
    if let Ok(p) = std::env::var(&env_key) {
        return PathBuf::from(p);
    }
    PathBuf::from(format!("{name}.toml"))
}

/// Recursively replace `"env:VAR_NAME"` string values using `getter`.
///
/// `getter(var_name)` returns `Some(value)` when the var is set, `None` when
/// it is unset.  In production this calls `std::env::var`; in tests a closure
/// provides deterministic values without touching process state.
fn resolve_env_vars_with(
    value: toml::Value,
    getter: impl Fn(&str) -> Option<String> + Copy,
) -> Result<toml::Value, ConfigError> {
    match value {
        toml::Value::String(ref s) if s.starts_with("env:") => {
            let var = &s[4..];
            getter(var)
                .map(toml::Value::String)
                .ok_or_else(|| ConfigError::EnvVarMissing {
                    var: var.to_owned(),
                })
        }
        toml::Value::Table(map) => {
            let mut out = toml::Table::new();
            for (k, v) in map {
                out.insert(k, resolve_env_vars_with(v, getter)?);
            }
            Ok(toml::Value::Table(out))
        }
        toml::Value::Array(arr) => {
            let resolved: Result<Vec<_>, _> = arr
                .into_iter()
                .map(|v| resolve_env_vars_with(v, getter))
                .collect();
            Ok(toml::Value::Array(resolved?))
        }
        other => Ok(other),
    }
}

fn resolve_env_vars(value: toml::Value) -> Result<toml::Value, ConfigError> {
    resolve_env_vars_with(value, |var| std::env::var(var).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_plain_string_unchanged() {
        let v = toml::Value::String("hello".to_owned());
        let out = resolve_env_vars_with(v, |_| None).unwrap();
        assert_eq!(out, toml::Value::String("hello".to_owned()));
    }

    #[test]
    fn resolve_env_ref_found() {
        let v = toml::Value::String("env:MY_VAR".to_owned());
        let out = resolve_env_vars_with(v, |var| {
            if var == "MY_VAR" {
                Some("resolved_value".to_owned())
            } else {
                None
            }
        })
        .unwrap();
        assert_eq!(out, toml::Value::String("resolved_value".to_owned()));
    }

    #[test]
    fn resolve_env_ref_missing_returns_error() {
        let v = toml::Value::String("env:MISSING_VAR".to_owned());
        let err = resolve_env_vars_with(v, |_| None).unwrap_err();
        assert!(matches!(err, ConfigError::EnvVarMissing { var } if var == "MISSING_VAR"));
    }

    #[test]
    fn resolve_nested_table() {
        let raw: toml::Value = toml::from_str(
            r#"[database]
url = "env:DB_URL"
"#,
        )
        .unwrap();
        let resolved = resolve_env_vars_with(raw, |var| {
            if var == "DB_URL" {
                Some("postgres://localhost/test".to_owned())
            } else {
                None
            }
        })
        .unwrap();
        let url = resolved["database"]["url"].as_str().unwrap();
        assert_eq!(url, "postgres://localhost/test");
    }

    #[test]
    fn non_env_string_in_table_unchanged() {
        let raw: toml::Value = toml::from_str(r#"key = "plain_value""#).unwrap();
        let resolved = resolve_env_vars_with(raw, |_| None).unwrap();
        assert_eq!(resolved["key"].as_str().unwrap(), "plain_value");
    }
}
