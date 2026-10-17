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

// ── Shared config structs ─────────────────────────────────────────────────────

/// Standard `PostgreSQL` database configuration block.
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
    /// `PostgreSQL` connection URL.
    ///
    /// Use `"env:DATABASE_URL"` to defer to the environment at runtime — this
    /// avoids storing secrets in TOML files checked into version control.
    pub url: String,

    /// Maximum number of connections in the pool. Default: 10.
    #[serde(default = "DatabaseConfig::default_pool_size")]
    pub pool_size: u32,

    /// Connections kept open even when idle, to absorb a burst without paying
    /// connect latency on the first request. Default: 0 (lazy).
    #[serde(default)]
    pub min_connections: u32,

    /// How long to wait for a free connection before failing a request, rather
    /// than queueing unboundedly. Default: 30 s.
    #[serde(default = "DatabaseConfig::default_acquire_timeout_secs")]
    pub acquire_timeout_secs: u64,

    /// Close a connection after it has been idle this long, releasing it back to
    /// `PostgreSQL`. Default: 600 s (10 min).
    #[serde(default = "DatabaseConfig::default_idle_timeout_secs")]
    pub idle_timeout_secs: u64,

    /// Recycle a connection after this long regardless of use, so a pool never
    /// pins a connection across a `PostgreSQL` failover or a proxy's own timeout.
    /// Default: 1800 s (30 min).
    #[serde(default = "DatabaseConfig::default_max_lifetime_secs")]
    pub max_lifetime_secs: u64,
}

impl DatabaseConfig {
    fn default_pool_size() -> u32 {
        10
    }
    fn default_acquire_timeout_secs() -> u64 {
        30
    }
    fn default_idle_timeout_secs() -> u64 {
        600
    }
    fn default_max_lifetime_secs() -> u64 {
        1_800
    }

    /// `PgPoolOptions` with every tuning knob applied — the single place pool
    /// sizing and lifetimes are decided for the whole platform.
    #[must_use]
    pub fn pool_options(&self) -> sqlx::postgres::PgPoolOptions {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(self.pool_size)
            .min_connections(self.min_connections)
            .acquire_timeout(std::time::Duration::from_secs(self.acquire_timeout_secs))
            .idle_timeout(std::time::Duration::from_secs(self.idle_timeout_secs))
            .max_lifetime(std::time::Duration::from_secs(self.max_lifetime_secs))
    }

    /// Build the connection pool for a service, tagging every connection with
    /// `application_name` so it is attributable in `pg_stat_activity`.
    ///
    /// `url` is the already-resolved connection string (resolve any `env:` ref
    /// with [`resolve_env_secret`] first, so the secret never lands in a struct
    /// that could be logged). This is the one path a daemon should use — it
    /// guarantees the configured `pool_size` and lifetimes actually take effect,
    /// which a bare `PgPool::connect` silently ignores.
    ///
    /// # Errors
    /// Returns a [`sqlx::Error`] if the URL is malformed or the initial
    /// connection cannot be established within `acquire_timeout_secs`.
    pub async fn connect(
        &self,
        url: &str,
        application_name: &str,
    ) -> Result<sqlx::PgPool, sqlx::Error> {
        let options: sqlx::postgres::PgConnectOptions = url.parse()?;
        self.pool_options()
            .connect_with(options.application_name(application_name))
            .await
    }
}

/// The listen-address block every daemon's config takes:
///
/// ```toml
/// [http]
/// addr = "0.0.0.0:8580"
/// ```
///
/// One shape for every service, so `dev/ports.toml` and the dev configs can be
/// checked against each other, and an operator reading one service's config
/// knows where every other one listens. A daemon embeds it as
/// `#[serde(default = "…")] http: HttpConfig` with its default port from
/// [`HttpConfig::on_port`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// Socket address to bind.
    pub addr: String,
}

impl HttpConfig {
    /// Every interface on `port` — a daemon's default listen address.
    #[must_use]
    pub fn on_port(port: u16) -> Self {
        Self {
            addr: format!("0.0.0.0:{port}"),
        }
    }
}

// ── Loader ────────────────────────────────────────────────────────────────────

/// Where the loader looked for the TOML file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigFile {
    /// The path `<PREFIX>_CONFIG` names.
    FromEnv {
        /// The variable that named it, e.g. `MARKTD_CONFIG`.
        var: String,
        /// The path it holds.
        path: PathBuf,
    },
    /// The default `./<name>.toml`, read when `<PREFIX>_CONFIG` is unset.
    Default(PathBuf),
    /// A path the caller passed to [`load_config_file`].
    Explicit(PathBuf),
}

impl ConfigFile {
    /// The path read.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::FromEnv { path, .. } | Self::Default(path) | Self::Explicit(path) => path,
        }
    }
}

/// One thing wrong with a config, named by its dotted key path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigProblem {
    /// A required key is in neither the file nor the environment.
    Missing(String),
    /// A key the config type does not declare (reported for types that deny
    /// unknown fields).
    Unknown(String),
    /// A key whose value does not deserialize.
    Invalid {
        /// Dotted key path.
        path: String,
        /// What is wrong with it.
        message: String,
    },
    /// The file `<PREFIX>_CONFIG` (or the caller) named does not exist.
    FileNotFound(PathBuf),
    /// A layer could not be read at all: TOML syntax, an unset `env:` reference,
    /// an unreadable `_FILE`.
    Unreadable(String),
    /// A `Required` operator setting (a [`crate::policy::PolicyChoice`] with no
    /// default) is in neither the file nor the environment.
    PolicyRequired {
        /// Dotted key path, e.g. `policy.uev_boersenreferenz`.
        path: String,
        /// Every admissible option with its primary source.
        options: Vec<(String, String)>,
    },
    /// The checker stopped at a key it could not fill in to look further; more
    /// problems may sit behind the ones listed.
    Incomplete,
}

impl std::fmt::Display for ConfigProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(p) => write!(f, "missing field `{p}`"),
            Self::Unknown(p) => write!(f, "unknown field `{p}`"),
            Self::Invalid { path, message } => write!(f, "invalid value at `{path}`: {message}"),
            Self::FileNotFound(p) => write!(f, "config file {} does not exist", p.display()),
            Self::Unreadable(m) => write!(f, "{m}"),
            Self::PolicyRequired { path, options } => {
                write!(
                    f,
                    "missing policy setting `{path}`: it has no default, set one of "
                )?;
                for (i, (name, quelle)) in options.iter().enumerate() {
                    if i > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "`{name}` ({quelle})")?;
                }
                Ok(())
            }
            Self::Incomplete => f.write_str(
                "further problems may be hidden behind the ones above; fix them and re-run",
            ),
        }
    }
}

/// A config that did not load — every problem found, with the file the loader
/// read and the environment prefix it merged.
///
/// One error names all of it, because a service whose config is discovered one
/// missing field per restart costs a newcomer as many restarts as there are
/// fields.
#[derive(Debug, Clone)]
pub struct ConfigError(Box<ConfigErrorInner>);

#[derive(Debug, Clone)]
struct ConfigErrorInner {
    service: String,
    file: ConfigFile,
    file_exists: bool,
    env_prefix: String,
    problems: Vec<ConfigProblem>,
}

impl ConfigError {
    fn new(
        service: &str,
        file: &ConfigFile,
        env_prefix: &str,
        problems: Vec<ConfigProblem>,
    ) -> Self {
        Self(Box::new(ConfigErrorInner {
            service: service.to_owned(),
            file: file.clone(),
            file_exists: file.path().is_file(),
            env_prefix: env_prefix.to_owned(),
            problems,
        }))
    }

    /// Every problem found, in the order found.
    #[must_use]
    pub fn problems(&self) -> &[ConfigProblem] {
        &self.0.problems
    }

    /// Where the loader looked for the file.
    #[must_use]
    pub fn file(&self) -> &ConfigFile {
        &self.0.file
    }

    /// The environment-variable prefix merged over the file, e.g. `MARKTD_`.
    #[must_use]
    pub fn env_prefix(&self) -> &str {
        &self.0.env_prefix
    }
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let this = &self.0;
        let n = this.problems.len();
        writeln!(
            f,
            "{} config is invalid ({n} problem{})",
            this.service,
            if n == 1 { "" } else { "s" }
        )?;
        let path = this.file.path().display();
        let state = if this.file_exists {
            "read"
        } else {
            "not found"
        };
        match &this.file {
            ConfigFile::FromEnv { var, .. } => {
                writeln!(f, "  file:       {path} ({state}; named by {var})")?;
            }
            ConfigFile::Default(_) => {
                writeln!(
                    f,
                    "  file:       {path} ({state}; the default — set {}CONFIG to read another)",
                    this.env_prefix
                )?;
            }
            ConfigFile::Explicit(_) => writeln!(f, "  file:       {path} ({state})")?,
        }
        writeln!(
            f,
            "  env prefix: {p} (nested keys join with `__`, e.g. {p}DATABASE__URL; \
             a `_FILE` suffix reads the value from a file)",
            p = this.env_prefix
        )?;
        for p in &this.problems {
            writeln!(f, "  - {p}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// The environment-variable prefix for service `name`: upper-cased, `-` → `_`,
/// trailing `_` — `netzbilanzd` reads `NETZBILANZD_*`, a `my-daemon` would read
/// `MY_DAEMON_*`.
#[must_use]
pub fn env_prefix(name: &str) -> String {
    format!("{}_", name.to_uppercase().replace('-', "_"))
}

/// Load a service's configuration from TOML + environment variables.
///
/// See [module docs](self) for file-discovery rules and override conventions.
///
/// # Errors
///
/// Returns a [`ConfigError`] naming the file read (or where it looked), the
/// env prefix, and **every** missing, unknown or invalid key it finds.
pub fn load_config<C: DeserializeOwned>(name: &str) -> Result<C, ConfigError> {
    load(name, config_file(name))
}

/// [`load_config`] with the file given by the caller instead of discovered.
///
/// The environment layer still applies. A path that does not exist is a
/// problem, unlike the default `./<name>.toml`, whose absence means "env only".
///
/// # Errors
///
/// As [`load_config`].
pub fn load_config_file<C: DeserializeOwned>(
    name: &str,
    path: &std::path::Path,
) -> Result<C, ConfigError> {
    load(name, ConfigFile::Explicit(path.to_path_buf()))
}

fn load<C: DeserializeOwned>(name: &str, file: ConfigFile) -> Result<C, ConfigError> {
    let prefix = env_prefix(name);
    let fail = |problems| ConfigError::new(name, &file, &prefix, problems);
    if !file.path().is_file() && !matches!(file, ConfigFile::Default(_)) {
        return Err(fail(vec![ConfigProblem::FileNotFound(
            file.path().to_path_buf(),
        )]));
    }
    let figment = layered(file.path(), &prefix, true);
    // Clear what an earlier, unrelated deserialization left behind.
    let _ = crate::policy::take_missing_required();
    match figment.extract::<C>() {
        Ok(cfg) => Ok(cfg),
        Err(first) => Err(fail(diagnose::<C>(&figment, &first))),
    }
}

/// The file layer under the env layer, with `_FILE` reads and — when
/// `resolve` — `env:` indirections resolved.
fn layered(path: &std::path::Path, prefix: &str, resolve: bool) -> figment::Figment {
    use figment::{
        Figment,
        providers::{Env, Format, Toml},
    };
    use figment_file_provider_adapter::FileAdapter;
    let toml = FileAdapter::wrap(Toml::file(path));
    // `<PREFIX>_CONFIG` names the config-file path (read by `config_file`), not
    // a config key — ignore it so a struct with `#[serde(deny_unknown_fields)]`
    // does not fail startup on a stray `config` field.
    let env = FileAdapter::wrap(Env::prefixed(prefix).split("__").ignore(&[
        "CONFIG",
        "LOG_FORMAT",
        "LOG_LEVEL",
    ]));
    if resolve {
        Figment::new()
            .merge(EnvRefAdapter(toml))
            .merge(EnvRefAdapter(env))
    } else {
        Figment::new().merge(toml).merge(env)
    }
}

/// Cap on repair-and-retry rounds; a config type has far fewer keys.
const MAX_DIAGNOSE_ROUNDS: usize = 512;

/// Every problem in the merged data, not only the first.
///
/// `serde` stops at the first error, so this repairs and retries: a missing key
/// gets a placeholder (a table first, so a missing section's own required keys
/// are named too), an unknown key is removed, a wrong value is replaced — and
/// the type is deserialized again until it loads or nothing more can be filled.
fn diagnose<C: DeserializeOwned>(
    figment: &figment::Figment,
    first: &figment::Error,
) -> Vec<ConfigProblem> {
    use figment::error::Kind;
    use figment::value::Dict;

    let Ok(original) = figment.extract::<Dict>() else {
        // A layer failed before deserialization: syntax, `env:`, `_FILE`.
        return first
            .clone()
            .into_iter()
            .map(|e| ConfigProblem::Unreadable(e.to_string()))
            .collect();
    };
    let mut data = original.clone();
    let mut problems = Vec::new();
    let mut required = crate::policy::take_missing_required();
    for _ in 0..MAX_DIAGNOSE_ROUNDS {
        let Err(err) = try_extract::<C>(&data) else {
            return problems;
        };
        match &err.kind {
            Kind::MissingField(field) => {
                let mut path = err.path.clone();
                path.push(field.to_string());
                required.extend(crate::policy::take_missing_required());
                // A `Required` policy setting: name its options, and continue
                // with its first option so the rest of the config is checked.
                if let Some(r) = required.iter().find(|r| r.key == field.as_ref()) {
                    problems.push(ConfigProblem::PolicyRequired {
                        path: path.join("."),
                        options: r.options.clone(),
                    });
                    if !insert(
                        &mut data,
                        &path,
                        figment::value::Value::from(r.first.to_owned()),
                    ) {
                        problems.push(ConfigProblem::Incomplete);
                        return problems;
                    }
                    continue;
                }
                problems.push(ConfigProblem::Missing(path.join(".")));
                if !fill::<C>(&mut data, &path) {
                    problems.push(ConfigProblem::Incomplete);
                    return problems;
                }
            }
            Kind::UnknownField(field, _) => {
                // figment's path for an unknown key already ends in the key.
                let mut path = err.path.clone();
                if path.last() != Some(field) {
                    path.push(field.clone());
                }
                problems.push(ConfigProblem::Unknown(path.join(".")));
                if remove(&mut data, &path).is_none() {
                    problems.push(ConfigProblem::Incomplete);
                    return problems;
                }
            }
            kind => {
                // Only a value the operator supplied is theirs to fix; an error
                // inside a placeholder this loop inserted is an artefact of it.
                if err.path.is_empty() || lookup(&original, &err.path).is_none() {
                    if problems.is_empty() {
                        problems.push(ConfigProblem::Unreadable(err.to_string()));
                    } else {
                        problems.push(ConfigProblem::Incomplete);
                    }
                    return problems;
                }
                problems.push(ConfigProblem::Invalid {
                    path: err.path.join("."),
                    message: kind.to_string(),
                });
                if !fill::<C>(&mut data, &err.path) {
                    problems.push(ConfigProblem::Incomplete);
                    return problems;
                }
            }
        }
    }
    problems.push(ConfigProblem::Incomplete);
    problems
}

#[expect(
    clippy::result_large_err,
    reason = "figment's own error, built only while diagnosing a config that already failed to load"
)]
fn try_extract<C: DeserializeOwned>(data: &figment::value::Dict) -> Result<C, figment::Error> {
    figment::Figment::from(figment::providers::Serialized::defaults(data)).extract::<C>()
}

/// Set a placeholder at `path` that `C` accepts there, trying a table, a
/// string, a number, a boolean and an array in turn.
fn fill<C: DeserializeOwned>(data: &mut figment::value::Dict, path: &[String]) -> bool {
    use figment::value::{Dict, Value};
    let candidates = [
        Value::from(Dict::new()),
        Value::from(String::new()),
        Value::from(0u32),
        Value::from(false),
        Value::from(Vec::<Value>::new()),
    ];
    for candidate in candidates {
        let mut trial = data.clone();
        if !insert(&mut trial, path, candidate) {
            return false;
        }
        let rejected = match try_extract::<C>(&trial) {
            Ok(_) => false,
            Err(e) => {
                e.path == path
                    && !matches!(
                        e.kind,
                        figment::error::Kind::MissingField(_)
                            | figment::error::Kind::UnknownField(..)
                    )
            }
        };
        if !rejected {
            *data = trial;
            return true;
        }
    }
    false
}

fn lookup<'a>(
    data: &'a figment::value::Dict,
    path: &[String],
) -> Option<&'a figment::value::Value> {
    let (last, parents) = path.split_last()?;
    let mut dict = data;
    for key in parents {
        dict = dict.get(key)?.as_dict()?;
    }
    dict.get(last)
}

fn insert(data: &mut figment::value::Dict, path: &[String], value: figment::value::Value) -> bool {
    use figment::value::{Dict, Value};
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut dict = data;
    for key in parents {
        let entry = dict
            .entry(key.clone())
            .or_insert_with(|| Value::from(Dict::new()));
        let Value::Dict(_, inner) = entry else {
            return false;
        };
        dict = inner;
    }
    dict.insert(last.clone(), value);
    true
}

fn remove(data: &mut figment::value::Dict, path: &[String]) -> Option<figment::value::Value> {
    use figment::value::Value;
    let (last, parents) = path.split_last()?;
    let mut dict = data;
    for key in parents {
        let Value::Dict(_, inner) = dict.get_mut(key)? else {
            return None;
        };
        dict = inner;
    }
    dict.remove(last)
}

/// The merged file and environment layers for service `name`, as TOML, with
/// secrets redacted — what `--print-config` prints.
///
/// `env:` references are shown as written (a variable name is not a secret);
/// a literal under a secret-looking key (`*key`, `*secret*`, `*password*`,
/// `*token*`, `*credential*`) prints as `***`, and so does the password of a
/// URL. Keys the config type defaults are not shown, because they are in
/// neither layer.
///
/// `path` overrides discovery as [`load_config_file`] does.
///
/// # Errors
///
/// Returns the [`ConfigError`] of a layer that cannot be read.
pub fn render_config(name: &str, path: Option<&std::path::Path>) -> Result<String, ConfigError> {
    let file = path.map_or_else(
        || config_file(name),
        |p| ConfigFile::Explicit(p.to_path_buf()),
    );
    let prefix = env_prefix(name);
    let unreadable =
        |msg: String| ConfigError::new(name, &file, &prefix, vec![ConfigProblem::Unreadable(msg)]);
    let mut dict = layered(file.path(), &prefix, false)
        .extract::<figment::value::Dict>()
        .map_err(|e| unreadable(e.to_string()))?;
    redact_dict(&mut dict);
    toml::to_string_pretty(&dict).map_err(|e| unreadable(e.to_string()))
}

const REDACTED: &str = "***";

fn is_secret_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.ends_with("key")
        || ["secret", "password", "passwd", "token", "credential"]
            .iter()
            .any(|s| k.contains(s))
}

fn redact_dict(dict: &mut figment::value::Dict) {
    use figment::value::Value;
    for (key, value) in dict.iter_mut() {
        match value {
            Value::Dict(_, inner) => redact_dict(inner),
            Value::String(_, s) if s.starts_with("env:") => {}
            Value::String(_, s) if is_secret_key(key) => REDACTED.clone_into(s),
            Value::String(_, s) => *s = redact_url_password(s),
            Value::Array(_, items) if is_secret_key(key) => {
                for item in items {
                    *item = Value::from(REDACTED);
                }
            }
            _ => {}
        }
    }
}

/// `scheme://user:password@host` → `scheme://user:***@host`; anything else
/// unchanged.
fn redact_url_password(s: &str) -> String {
    let Some((scheme, rest)) = s.split_once("://") else {
        return s.to_owned();
    };
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return s.to_owned();
    };
    let Some((user, _password)) = userinfo.split_once(':') else {
        return s.to_owned();
    };
    format!("{scheme}://{user}:{REDACTED}@{host}{tail}")
}

/// Resolves every `env:VAR` indirection in a provider's data, at load time.
///
/// Without this, resolution is each service's own job — and a service that
/// forgets it ships the placeholder *as the value*. For an API key that is
/// merely loud (a 401). For an **HMAC secret it is silent and worse than having
/// no secret at all**: `verify_request` runs the full comparison against the
/// literal string `env:SVC_INBOUND_SECRET`, which is a constant published in
/// this repository, so a forged webhook *verifies*. Nine services shipped that
/// way while their own READMEs taught the `env:` syntax.
///
/// Resolving here makes the indirection a property of the loader rather than a
/// convention each service has to remember, and an unset variable fails startup
/// naming it. A value that is already literal passes through untouched, so a
/// service that also calls [`resolve_env`] itself is unaffected.
struct EnvRefAdapter<P>(P);

impl<P: figment::Provider> figment::Provider for EnvRefAdapter<P> {
    fn metadata(&self) -> figment::Metadata {
        self.0.metadata()
    }

    fn data(
        &self,
    ) -> Result<figment::value::Map<figment::Profile, figment::value::Dict>, figment::Error> {
        let mut data = self.0.data()?;
        for dict in data.values_mut() {
            resolve_dict(dict).map_err(|e| figment::Error::from(e.to_string()))?;
        }
        Ok(data)
    }

    fn profile(&self) -> Option<figment::Profile> {
        self.0.profile()
    }
}

fn resolve_dict(dict: &mut figment::value::Dict) -> Result<(), EnvRefError> {
    for value in dict.values_mut() {
        resolve_value(value)?;
    }
    Ok(())
}

fn resolve_value(value: &mut figment::value::Value) -> Result<(), EnvRefError> {
    use figment::value::Value;
    match value {
        Value::String(_, s) => {
            if s.starts_with("env:") {
                *s = resolve_env(s)?;
            }
        }
        Value::Dict(_, d) => resolve_dict(d)?,
        Value::Array(_, items) => {
            for item in items {
                resolve_value(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn config_file(name: &str) -> ConfigFile {
    let var = format!("{}CONFIG", env_prefix(name));
    match std::env::var(&var) {
        Ok(p) => ConfigFile::FromEnv {
            var,
            path: PathBuf::from(p),
        },
        Err(_) => ConfigFile::Default(PathBuf::from(format!("{name}.toml"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_default() {
        let p = config_file("myservice");
        assert_eq!(p.path(), std::path::Path::new("myservice.toml"));
    }

    #[test]
    fn config_path_hyphen_service() {
        // "my-daemon" -> MY_DAEMON_CONFIG (hyphens become underscores in env key).
        let p = config_file("my-daemon");
        assert_eq!(p.path(), std::path::Path::new("my-daemon.toml"));
        assert_eq!(env_prefix("my-daemon"), "MY_DAEMON_");
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

    /// The loader resolves `env:` anywhere in the tree, at any depth.
    ///
    /// This is what stops a service that forgets to call [`resolve_env`] from
    /// shipping the placeholder as the value. For an HMAC secret that failure is
    /// silent and worse than having none: the literal `env:SVC_SECRET` is a
    /// constant published in this repository, so a forged webhook verifies.
    #[test]
    fn the_loader_resolves_env_refs_at_every_depth() {
        use figment::value::{Dict, Value};

        // `PATH` rather than a variable the test sets: this crate denies
        // `unsafe_code`, and `std::env::set_var` is unsafe as of Rust 2024.
        let expected = std::env::var("PATH").expect("PATH is set in any test environment");

        let mut inner = Dict::new();
        inner.insert("secret".into(), Value::from("env:PATH"));
        let mut dict = Dict::new();
        dict.insert("literal".into(), Value::from("stays"));
        dict.insert("nested".into(), Value::from(inner));
        dict.insert("list".into(), Value::from(vec![Value::from("env:PATH")]));

        resolve_dict(&mut dict).expect("every referenced variable is set");

        assert_eq!(dict["literal"].as_str(), Some("stays"));
        assert_eq!(
            dict["nested"].as_dict().unwrap()["secret"].as_str(),
            Some(expected.as_str()),
            "a nested `env:` must resolve — config sections are nested by definition"
        );
        assert_eq!(
            dict["list"].as_array().unwrap()[0].as_str(),
            Some(expected.as_str()),
            "an `env:` inside an array must resolve too"
        );
    }

    /// An unset variable fails startup naming it, rather than silently yielding
    /// the placeholder.
    #[test]
    fn an_unset_env_ref_refuses_at_load_time() {
        use figment::value::{Dict, Value};

        let mut dict = Dict::new();
        dict.insert(
            "secret".into(),
            Value::from("env:MAKO_TEST_ENVREF_DEFINITELY_UNSET"),
        );
        let err = resolve_dict(&mut dict).expect_err("an unset variable must refuse");
        assert!(
            err.to_string()
                .contains("MAKO_TEST_ENVREF_DEFINITELY_UNSET"),
            "the error must name the variable, got: {err}"
        );
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

#[cfg(test)]
mod loader_diagnostics_tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[expect(dead_code, reason = "fields exist to be deserialised, never read")]
    struct Fixture {
        database: DatabaseConfig,
        http_addr: String,
        markt: Markt,
        #[serde(default)]
        optional: Option<u32>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[expect(dead_code, reason = "fields exist to be deserialised, never read")]
    struct Markt {
        tenant: String,
        retries: u32,
        enabled: bool,
    }

    fn write(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mako-service-config-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fixture.toml");
        std::fs::write(&path, body).unwrap();
        path
    }

    /// An empty file names every missing field in one error, with the file it
    /// read and the env prefix it would have read.
    #[test]
    fn an_empty_file_names_every_missing_field_at_once() {
        let path = write("empty", "");
        let err = load_config_file::<Fixture>("fixturesvc", &path)
            .expect_err("an empty file cannot satisfy the fixture");
        let msg = err.to_string();
        for field in [
            "database",
            "database.url",
            "http_addr",
            "markt",
            "markt.tenant",
            "markt.retries",
            "markt.enabled",
        ] {
            assert!(
                err.problems()
                    .iter()
                    .any(|p| matches!(p, ConfigProblem::Missing(f) if f == field)),
                "missing field `{field}` must be named, got:\n{msg}"
            );
        }
        assert!(
            msg.contains(&path.display().to_string()),
            "names the file:\n{msg}"
        );
        assert!(msg.contains("FIXTURESVC_"), "names the env prefix:\n{msg}");
        assert!(
            !err.problems()
                .iter()
                .any(|p| matches!(p, ConfigProblem::Missing(f) if f == "optional")),
            "an Option field is not missing:\n{msg}"
        );
    }

    /// Unknown keys are reported beside the missing ones, not instead of them.
    #[test]
    fn unknown_and_missing_fields_are_reported_together() {
        let path = write(
            "unknown",
            "http_addr = \"0.0.0.0:1\"\ntypo = 1\n[markt]\ntenant = \"t\"\nretires = 3\n",
        );
        let err = load_config_file::<Fixture>("fixturesvc", &path).expect_err("invalid");
        let msg = err.to_string();
        let has = |want: &ConfigProblem| err.problems().contains(want);
        assert!(has(&ConfigProblem::Unknown("typo".into())), "{msg}");
        assert!(
            has(&ConfigProblem::Unknown("markt.retires".into())),
            "{msg}"
        );
        assert!(
            has(&ConfigProblem::Missing("markt.retries".into())),
            "{msg}"
        );
        assert!(
            has(&ConfigProblem::Missing("markt.enabled".into())),
            "{msg}"
        );
        assert!(has(&ConfigProblem::Missing("database".into())), "{msg}");
    }

    /// A value of the wrong type is named with its path, and the rest of the
    /// file is still checked.
    #[test]
    fn a_wrong_type_is_named_and_does_not_hide_the_rest() {
        let path = write(
            "badtype",
            "http_addr = \"x\"\n[markt]\ntenant = \"t\"\nretries = \"three\"\nenabled = true\n",
        );
        let err = load_config_file::<Fixture>("fixturesvc", &path).expect_err("invalid");
        let msg = err.to_string();
        assert!(
            err.problems().iter().any(
                |p| matches!(p, ConfigProblem::Invalid { path, .. } if path == "markt.retries")
            ),
            "{msg}"
        );
        assert!(
            err.problems()
                .contains(&ConfigProblem::Missing("database".into())),
            "{msg}"
        );
    }

    /// A file the `<PREFIX>_CONFIG`-style path names but that does not exist is
    /// a problem; the default path being absent is not.
    #[test]
    fn a_named_file_that_does_not_exist_is_refused() {
        let missing = std::env::temp_dir().join("mako-service-config-definitely-absent.toml");
        let err = load_config_file::<Fixture>("fixturesvc", &missing).expect_err("absent");
        assert!(
            err.problems()
                .iter()
                .any(|p| matches!(p, ConfigProblem::FileNotFound(_))),
            "{err}"
        );
    }

    #[test]
    fn a_valid_file_loads() {
        let path = write(
            "valid",
            "http_addr = \"0.0.0.0:1\"\n[database]\nurl = \"postgres://x\"\n\
             [markt]\ntenant = \"t\"\nretries = 3\nenabled = true\n",
        );
        let cfg = load_config_file::<Fixture>("fixturesvc", &path).expect("valid");
        assert_eq!(cfg.markt.retries, 3);
    }

    #[test]
    fn print_config_redacts_secrets_and_url_passwords() {
        let path = write(
            "print",
            "api_key = \"hunter2\"\nplaceholder = \"env:SOME_SECRET\"\n\
             [database]\nurl = \"postgres://dev:topsecret@localhost/db\"\n\
             [webhook]\nhmac_secret = \"s3cr3t\"\nendpoint = \"http://example.invalid\"\n",
        );
        let out = render_config("fixturesvc", Some(&path)).expect("renders");
        assert!(!out.contains("hunter2"), "{out}");
        assert!(!out.contains("topsecret"), "{out}");
        assert!(!out.contains("s3cr3t"), "{out}");
        assert!(
            out.contains("env:SOME_SECRET"),
            "an env: reference is not a secret:\n{out}"
        );
        assert!(out.contains("http://example.invalid"), "{out}");
        assert!(out.contains("postgres://dev:***@localhost/db"), "{out}");
    }
}
