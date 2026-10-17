//! Operator policy — a reading of the law or a business choice that differs
//! legitimately between operators, as one typed, sourced setting.
//!
//! A setting exists only where a primary source leaves the answer to the
//! operator: whether to pay an EEG Teil-3 payment no state-aid approval covers,
//! which law prices nEHS certificates in a year with no § 10 BEHG price. Where a
//! source settles the question, a setting would be a way to configure a breach,
//! so there is none.
//!
//! The shape:
//!
//! - [`PolicyChoice`] — a closed enum with its config key, its docs anchor, the
//!   primary source of each option, and either a [`PolicyDefault::Tightening`]
//!   default (the direction that cannot cause a breach or an overpayment, with
//!   the reason) or [`PolicyDefault::Required`]. Declared with
//!   [`policy_choice!`](crate::policy_choice), never by hand.
//! - [`Policy<T>`] — the resolved value and whether the operator set it
//!   ([`Origin::Explicit`]) or the tightening default applied
//!   ([`Origin::Default`]). A missing `Required` setting does not deserialize:
//!   the config loader reports it as [`ConfigProblem::PolicyRequired`]
//!   naming every option and its source, beside every other problem.
//! - [`PolicyReport`] — what a daemon runs under, policy and posture, which the
//!   runner logs at startup, exports as `mako_policy_info`, records in
//!   `service_policy_log` and a daemon serves at `GET /api/v1/policy`.
//! - [`PolicyTriple`] — `{key, value, quelle}`, carried by every result whose
//!   computation branches on a setting, so the result names the reading that
//!   produced it.
//!
//! A setting changes by editing the config and restarting; no route and no
//! tool writes one. A settlement computed under one reading must stay
//! reproducible, and a value that could move at runtime would make the trace
//! of a recompute unexplainable.
//!
//! [`ConfigProblem::PolicyRequired`]: crate::config::ConfigProblem::PolicyRequired

use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

// ── The trait ─────────────────────────────────────────────────────────────────

/// The default of a [`PolicyChoice`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDefault<T: 'static> {
    /// The option that cannot cause a breach or an overpayment, and why.
    Tightening(T, &'static str),
    /// No default: the service refuses to start until the setting is set.
    Required,
}

/// A date after which a setting must be looked at again, because the law it
/// represents is not yet enacted or is due to change.
///
/// Passing it never changes the value. The runner warns at every startup after
/// it, naming [`ReviewBy::grund`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewBy {
    /// Year.
    pub jahr: i32,
    /// Month, 1–12.
    pub monat: u8,
    /// Day of month.
    pub tag: u8,
    /// What has to be checked, and what retires the setting.
    pub grund: &'static str,
}

impl ReviewBy {
    /// The review date, or `None` for a `(jahr, monat, tag)` that is no date —
    /// which [`crate::policy_choice!`]'s test module refuses at `cargo test`.
    #[must_use]
    pub fn datum(&self) -> Option<time::Date> {
        let monat = time::Month::try_from(self.monat).ok()?;
        time::Date::from_calendar_date(self.jahr, monat, self.tag).ok()
    }
}

/// An operator setting: a closed set of options, each with its primary source.
///
/// Implement it with [`crate::policy_choice!`]. The trait carries no serde
/// bound: [`Policy<T>`] reads the option by [`PolicyChoice::name`], so the name
/// in the config, in the report and in a result trace is one string.
pub trait PolicyChoice: Copy + Eq + Send + Sync + fmt::Debug + 'static {
    /// The key under `[policy]`, e.g. `beihilfe_politik`.
    const KEY: &'static str;
    /// The docs anchor that documents the setting,
    /// e.g. `@/docs/services/einsd.md#policy-beihilfe-politik`.
    const DOC: &'static str;
    /// The default, or `Required`.
    const DEFAULT: PolicyDefault<Self>;
    /// When the setting must be reviewed, if ever.
    const REVIEW_BY: Option<ReviewBy>;
    /// Every option, in declaration order.
    fn options() -> &'static [Self];
    /// The option as written in the config.
    fn name(self) -> &'static str;
    /// The primary source of this option — document, chapter, page.
    fn quelle(self) -> &'static str;
}

/// A [`PolicyChoice`] whose default is [`PolicyDefault::Tightening`].
///
/// Implemented by [`crate::policy_choice!`] for a tightening default only, so
/// `Default for Policy<T>` exists exactly where a value exists without the
/// operator: a `[policy]` section holding a `Required` setting cannot derive
/// `Default`, and therefore cannot be `#[serde(default)]` either.
pub trait TighteningDefault: PolicyChoice {
    /// The tightening default.
    const TIGHTENING: Self;
}

// ── The resolved value ───────────────────────────────────────────────────────

/// Whether the operator set a value or the tightening default applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The tightening default — the operator set nothing.
    Default,
    /// Set in the config file or the environment.
    Explicit,
}

impl Origin {
    /// `default` / `explicit`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Explicit => "explicit",
        }
    }
}

/// A resolved [`PolicyChoice`]: the value and its [`Origin`].
///
/// Deserializes from the option's name (a string, or a TOML boolean for a
/// `true`/`false` choice). An absent key resolves to the tightening default;
/// an absent `Required` key fails, and the loader names its options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy<T: PolicyChoice> {
    value: T,
    origin: Origin,
}

impl<T: PolicyChoice> Policy<T> {
    /// A value the operator set.
    #[must_use]
    pub const fn explicit(value: T) -> Self {
        Self {
            value,
            origin: Origin::Explicit,
        }
    }

    /// The value.
    #[must_use]
    pub const fn get(&self) -> T {
        self.value
    }

    /// Set or defaulted.
    #[must_use]
    pub const fn origin(&self) -> Origin {
        self.origin
    }

    /// `{key, value, quelle}` for the result this value shaped.
    #[must_use]
    pub fn triple(&self) -> PolicyTriple {
        PolicyTriple {
            key: T::KEY.to_owned(),
            value: self.value.name().to_owned(),
            quelle: self.value.quelle().to_owned(),
        }
    }

    /// The report entry for this setting.
    #[must_use]
    pub fn entry(&self) -> PolicyEntry {
        let review = T::REVIEW_BY;
        PolicyEntry {
            key: T::KEY.to_owned(),
            value: self.value.name().to_owned(),
            origin: self.origin,
            class: PolicyClass::Policy,
            quelle: Some(self.value.quelle().to_owned()),
            doc: Some(T::DOC.to_owned()),
            insecure: false,
            review_by: review.and_then(|r| r.datum()),
            review_grund: review.map(|r| r.grund.to_owned()),
        }
    }
}

impl<T: TighteningDefault> Default for Policy<T> {
    fn default() -> Self {
        Self {
            value: T::TIGHTENING,
            origin: Origin::Default,
        }
    }
}

impl<T: PolicyChoice> Serialize for Policy<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.value.name())
    }
}

/// The options of `T` with their sources, for an error message.
#[must_use]
pub fn describe_options<T: PolicyChoice>() -> Vec<(String, String)> {
    T::options()
        .iter()
        .map(|o| (o.name().to_owned(), o.quelle().to_owned()))
        .collect()
}

/// The option of `T` named `name`.
///
/// # Errors
///
/// An unknown name, with every admissible option listed.
pub fn parse_option<T: PolicyChoice>(name: &str) -> Result<T, String> {
    T::options()
        .iter()
        .copied()
        .find(|o| o.name() == name)
        .ok_or_else(|| {
            format!(
                "unknown option `{name}` for `{}`; admissible: {}",
                T::KEY,
                T::options()
                    .iter()
                    .map(|o| format!("`{}`", o.name()))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

thread_local! {
    /// The `Required` settings found missing during the current load, with
    /// their options — read by the config diagnosis to name them.
    ///
    /// Thread-local because `serde` gives a `Deserialize` impl no channel to
    /// the loader other than the error, and a missing-field error carries a
    /// field name and nothing else. The diagnosis runs on the thread that
    /// deserialized, synchronously, and takes what it reads.
    static MISSING_REQUIRED: RefCell<Vec<RequiredMissing>> = const { RefCell::new(Vec::new()) };
}

/// A `Required` setting absent from the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequiredMissing {
    pub(crate) key: &'static str,
    pub(crate) first: &'static str,
    pub(crate) options: Vec<(String, String)>,
}

pub(crate) fn take_missing_required() -> Vec<RequiredMissing> {
    MISSING_REQUIRED.with(|m| std::mem::take(&mut *m.borrow_mut()))
}

impl<'de, T: PolicyChoice> Deserialize<'de> for Policy<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct OptionVisitor<T>(PhantomData<T>);
        impl<'de, T: PolicyChoice> Visitor<'de> for OptionVisitor<T> {
            type Value = Policy<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "an option of `{}`", T::KEY)
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                match T::DEFAULT {
                    PolicyDefault::Tightening(value, _) => Ok(Policy {
                        value,
                        origin: Origin::Default,
                    }),
                    PolicyDefault::Required => {
                        let first = T::options().first().map_or("", |o| o.name());
                        MISSING_REQUIRED.with(|m| {
                            let mut m = m.borrow_mut();
                            if !m.iter().any(|r| r.key == T::KEY) {
                                m.push(RequiredMissing {
                                    key: T::KEY,
                                    first,
                                    options: describe_options::<T>(),
                                });
                            }
                        });
                        Err(E::missing_field(T::KEY))
                    }
                }
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                self.visit_none()
            }
            fn visit_some<D2: Deserializer<'de>>(self, d: D2) -> Result<Self::Value, D2::Error> {
                d.deserialize_any(NameVisitor(PhantomData))
            }
        }
        d.deserialize_option(OptionVisitor(PhantomData))
    }
}

struct NameVisitor<T>(PhantomData<T>);

impl<T: PolicyChoice> Visitor<'_> for NameVisitor<T> {
    type Value = Policy<T>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an option of `{}`", T::KEY)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        parse_option::<T>(v)
            .map(Policy::explicit)
            .map_err(E::custom)
    }
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        self.visit_str(if v { "true" } else { "false" })
    }
}

// ── The report ───────────────────────────────────────────────────────────────

/// What an entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyClass {
    /// An operator reading under `[policy]`.
    Policy,
    /// A deployment posture: an `allow_*` flag or whether a secret is present.
    /// Never legitimised as policy; reported so an insecure one is seen.
    Posture,
}

impl PolicyClass {
    /// `policy` / `posture`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::Posture => "posture",
        }
    }
}

/// One line of a [`PolicyReport`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyEntry {
    /// The key, e.g. `beihilfe_politik` or `allow_insecure_no_auth`.
    pub key: String,
    /// The value as written in the config (`present`/`absent` for a secret).
    pub value: String,
    /// Set or defaulted.
    pub origin: Origin,
    /// Policy or posture.
    pub class: PolicyClass,
    /// The primary source of the value; `None` for a posture.
    pub quelle: Option<String>,
    /// The docs anchor; `None` for a posture.
    pub doc: Option<String>,
    /// A posture in its insecure state.
    pub insecure: bool,
    /// The review date of a setting that represents law not yet enacted.
    pub review_by: Option<time::Date>,
    /// What the review checks.
    pub review_grund: Option<String>,
}

/// The policy and posture a daemon runs under.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyReport {
    /// One entry per setting, in the order the daemon reports them.
    pub entries: Vec<PolicyEntry>,
}

impl PolicyReport {
    /// An empty report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a policy setting.
    #[must_use]
    pub fn with<T: PolicyChoice>(mut self, p: &Policy<T>) -> Self {
        self.entries.push(p.entry());
        self
    }

    /// Add an `allow_*`-style flag: `true` is its insecure state, and a flag
    /// that defaults to `false` can only be `true` because someone set it.
    #[must_use]
    pub fn posture_flag(mut self, key: &str, value: bool) -> Self {
        self.entries.push(PolicyEntry {
            key: key.to_owned(),
            value: value.to_string(),
            origin: if value {
                Origin::Explicit
            } else {
                Origin::Default
            },
            class: PolicyClass::Posture,
            quelle: None,
            doc: None,
            insecure: value,
            review_by: None,
            review_grund: None,
        });
        self
    }

    /// Add whether a secret or a section is configured — never its value.
    #[must_use]
    pub fn posture_presence(
        mut self,
        key: &str,
        present: bool,
        insecure_when_absent: bool,
    ) -> Self {
        self.entries.push(PolicyEntry {
            key: key.to_owned(),
            value: if present { "present" } else { "absent" }.to_owned(),
            origin: if present {
                Origin::Explicit
            } else {
                Origin::Default
            },
            class: PolicyClass::Posture,
            quelle: None,
            doc: None,
            insecure: insecure_when_absent && !present,
            review_by: None,
            review_grund: None,
        });
        self
    }

    /// No entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry for `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&PolicyEntry> {
        self.entries.iter().find(|e| e.key == key)
    }

    /// The entries whose review date is on or before `today`.
    #[must_use]
    pub fn review_due(&self, today: time::Date) -> Vec<&PolicyEntry> {
        self.entries
            .iter()
            .filter(|e| e.review_by.is_some_and(|d| d <= today))
            .collect()
    }

    /// The startup lines: one per entry, `<service>.<key>=<value> origin=…`,
    /// with the source for a policy setting.
    #[must_use]
    pub fn lines(&self, service: &str) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| {
                let mut line = format!(
                    "{service}.{}={} origin={} class={}",
                    e.key,
                    e.value,
                    e.origin.as_str(),
                    e.class.as_str()
                );
                if let Some(q) = &e.quelle {
                    use std::fmt::Write as _;
                    let _ = write!(line, " quelle={q:?}");
                }
                if e.insecure {
                    line.push_str(" insecure");
                }
                line
            })
            .collect()
    }

    /// Log the report: `info` per policy entry and per secure posture, `warn`
    /// per insecure posture and per setting past its review date.
    ///
    /// `now` decides the review date. A review date passes at Berlin midnight,
    /// which is 22:00 or 23:00 UTC of the day before; comparing against UTC
    /// midnight warns up to two hours late, which a review reminder can afford
    /// and a business date could not.
    pub fn log(&self, service: &str, now: time::OffsetDateTime) {
        for (e, line) in self.entries.iter().zip(self.lines(service)) {
            if e.insecure {
                tracing::warn!(target: "mako_policy", "{line}");
            } else {
                tracing::info!(target: "mako_policy", "{line}");
            }
            if let Some(d) = e.review_by
                && now >= d.midnight().assume_utc()
            {
                tracing::warn!(
                    target: "mako_policy",
                    "{service}.{} is past its review date {d}: {} — the value stays {} \
                     until the operator changes it",
                    e.key,
                    e.review_grund.as_deref().unwrap_or(""),
                    e.value
                );
            }
        }
    }
}

/// `{key, value, quelle}` — the reading a result was computed under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyTriple {
    /// The setting's key.
    pub key: String,
    /// The option in force.
    pub value: String,
    /// That option's primary source.
    pub quelle: String,
}

impl fmt::Display for PolicyTriple {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[policy] {}={} ({})", self.key, self.value, self.quelle)
    }
}

// ── service_policy_log ───────────────────────────────────────────────────────

/// DDL for the runner-owned `service_policy_log`: one row per value a setting
/// first ran under. Tenant-free — policy is per deployment.
pub const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS service_policy_log (
    id            BIGSERIAL   PRIMARY KEY,
    service       TEXT        NOT NULL,
    key           TEXT        NOT NULL,
    value         TEXT        NOT NULL,
    origin        TEXT        NOT NULL CHECK (origin IN ('default', 'explicit')),
    class         TEXT        NOT NULL CHECK (class IN ('policy', 'posture')),
    first_boot_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS service_policy_log_latest
    ON service_policy_log (service, key, first_boot_at DESC, id DESC);
COMMENT ON TABLE service_policy_log IS
    'When each value of an operator setting first ran: one row per change, written at boot.';
";

/// Create `service_policy_log` if it does not exist. The runner calls it for
/// every daemon with a database and a non-empty [`PolicyReport`].
///
/// # Errors
///
/// The DDL failed.
pub async fn ensure_schema(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// A setting whose value differs from the last one recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyChange {
    /// The key.
    pub key: String,
    /// The last recorded value; `None` on the first boot that records the key.
    pub old: Option<String>,
    /// The value this boot runs under.
    pub new: String,
}

/// Append every entry whose value differs from the last row recorded for it,
/// in one transaction, and return what changed.
///
/// An unchanged boot appends nothing. Concurrent boots of two replicas
/// serialise on an advisory lock, so one appends and the other sees its row.
///
/// # Errors
///
/// A database failure; nothing is appended then.
pub async fn record(
    pool: &sqlx::PgPool,
    service: &str,
    report: &PolicyReport,
) -> Result<Vec<PolicyChange>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('service_policy_log:' || $1))")
        .bind(service)
        .execute(&mut *tx)
        .await?;
    let last: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (key) key, value FROM service_policy_log
          WHERE service = $1
          ORDER BY key, first_boot_at DESC, id DESC",
    )
    .bind(service)
    .fetch_all(&mut *tx)
    .await?;
    let mut changes = Vec::new();
    for e in &report.entries {
        let old = last
            .iter()
            .find(|(k, _)| *k == e.key)
            .map(|(_, v)| v.clone());
        if old.as_deref() == Some(e.value.as_str()) {
            continue;
        }
        sqlx::query(
            "INSERT INTO service_policy_log (service, key, value, origin, class)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(service)
        .bind(&e.key)
        .bind(&e.value)
        .bind(e.origin.as_str())
        .bind(e.class.as_str())
        .execute(&mut *tx)
        .await?;
        changes.push(PolicyChange {
            key: e.key.clone(),
            old,
            new: e.value.clone(),
        });
    }
    tx.commit().await?;
    Ok(changes)
}

/// Log what [`record`] found: a changed value at `warn`, a first recording at
/// `info`.
pub fn log_changes(service: &str, changes: &[PolicyChange]) {
    for c in changes {
        if let Some(old) = &c.old {
            tracing::warn!(
                target: "mako_policy",
                "{service}: policy changed {} {old} → {}",
                c.key,
                c.new
            );
        } else {
            tracing::info!(
                target: "mako_policy",
                "{service}: policy {}={} recorded for the first time",
                c.key,
                c.new
            );
        }
    }
}

// ── Metric ───────────────────────────────────────────────────────────────────

/// Export the report as `mako_policy_info{service,key,value,origin,class} 1`
/// on the default Prometheus registry `GET /metrics` serves.
///
/// Replaces whatever an earlier call exported, so the series always describe
/// the report of the process that is running.
#[cfg(feature = "metrics")]
pub fn export_metric(service: &str, report: &PolicyReport) {
    use std::sync::OnceLock;
    static INFO: OnceLock<prometheus::IntGaugeVec> = OnceLock::new();
    let gauge = INFO.get_or_init(|| {
        prometheus::register_int_gauge_vec!(
            "mako_policy_info",
            "The operator policy and posture this process runs under (value 1 per entry)",
            &["service", "key", "value", "origin", "class"]
        )
        .expect("mako_policy_info registration failed")
    });
    gauge.reset();
    for e in &report.entries {
        gauge
            .with_label_values(&[
                service,
                &e.key,
                &e.value,
                e.origin.as_str(),
                e.class.as_str(),
            ])
            .set(1);
    }
}

/// Without the `metrics` feature there is no registry to export to.
#[cfg(not(feature = "metrics"))]
pub fn export_metric(_service: &str, _report: &PolicyReport) {}

// ── REST ─────────────────────────────────────────────────────────────────────

/// `GET /api/v1/policy` — the report, read-only, behind the daemon's own
/// authentication and the Cedar action `read-policy`.
///
/// Merge it into the daemon's router **before** the layers that supply
/// `CedarEnforcer`, `OidcVerifier` and `ExpectedTenant`, so the request meets
/// them like every other route. No route writes a setting.
#[cfg(all(feature = "cedar", feature = "oidc"))]
pub fn router(report: PolicyReport) -> axum::Router {
    use std::sync::Arc;

    use axum::Extension;
    use axum::routing::get;

    async fn get_policy(
        claims: crate::oidc::Claims,
        Extension(enforcer): Extension<Arc<crate::cedar::CedarEnforcer>>,
        tenant: Option<Extension<crate::oidc::ExpectedTenant>>,
        Extension(report): Extension<Arc<PolicyReport>>,
    ) -> crate::ApiResult<axum::Json<PolicyReport>> {
        let principal = claims.principal();
        let resource = tenant.map_or_else(|| principal.tenant.clone(), |Extension(t)| t.0);
        enforcer
            .check(&principal, "read-policy", &resource)
            .map_err(|e| crate::ApiError::Denied(e.to_string()))?;
        Ok(axum::Json((*report).clone()))
    }

    axum::Router::new()
        .route("/api/v1/policy", get(get_policy))
        .layer(Extension(Arc::new(report)))
}

// ── The macro ────────────────────────────────────────────────────────────────

/// Declare an operator setting: the enum, its options with their sources, its
/// key, docs anchor and default, and the [`PolicyChoice`] impl.
///
/// ```
/// mako_service::policy_choice! {
///     /// What happens to a payment no approval covers.
///     pub enum Politik {
///         key: "politik",
///         doc: "@/docs/services/example.md#policy-politik",
///         default: tightening(Verweigern, "paying cannot be taken back"),
///         options: {
///             /// Refuse.
///             Verweigern = "verweigern" => "BT-Drs. 21/7867 S. 248, Zu § 104",
///             /// Pay under reservation.
///             UnterVorbehalt = "unter_vorbehalt" => "BT-Drs. 21/7867 S. 248, Zu § 104",
///         }
///     }
/// }
/// use mako_service::policy::PolicyChoice as _;
/// assert_eq!(Politik::UnterVorbehalt.name(), "unter_vorbehalt");
/// ```
///
/// `default: required` declares a setting with no default; `review_by: (2027,
/// 1, 1, "…")` before `options` adds a review date. One option per line,
/// `Variant = "name" => "source"`: `cargo xtask policy-inventory` and
/// `check-policy-settings` read these lines from the source.
#[macro_export]
macro_rules! policy_choice {
    (
        $(#[$m:meta])*
        $vis:vis enum $name:ident {
            key: $key:literal,
            doc: $doc:literal,
            default: tightening($dv:ident, $reason:literal),
            $(review_by: ($ry:literal, $rm:literal, $rd:literal, $rg:literal),)?
            options: {
                $( $(#[$vm:meta])* $variant:ident = $vname:literal => $quelle:literal, )+
            } $(,)?
        }
    ) => {
        $crate::policy_choice!(@enum $(#[$m])* $vis $name { $( $(#[$vm])* $variant, )+ });
        $crate::policy_choice!(@impl $name, $key, $doc,
            $crate::policy::PolicyDefault::Tightening($name::$dv, $reason),
            [$( ($ry, $rm, $rd, $rg) )?],
            { $( $variant = $vname => $quelle, )+ });
        impl $crate::policy::TighteningDefault for $name {
            const TIGHTENING: Self = $name::$dv;
        }
    };
    (
        $(#[$m:meta])*
        $vis:vis enum $name:ident {
            key: $key:literal,
            doc: $doc:literal,
            default: required,
            $(review_by: ($ry:literal, $rm:literal, $rd:literal, $rg:literal),)?
            options: {
                $( $(#[$vm:meta])* $variant:ident = $vname:literal => $quelle:literal, )+
            } $(,)?
        }
    ) => {
        $crate::policy_choice!(@enum $(#[$m])* $vis $name { $( $(#[$vm])* $variant, )+ });
        $crate::policy_choice!(@impl $name, $key, $doc,
            $crate::policy::PolicyDefault::Required,
            [$( ($ry, $rm, $rd, $rg) )?],
            { $( $variant = $vname => $quelle, )+ });
    };
    (@enum $(#[$m:meta])* $vis:vis $name:ident { $( $(#[$vm:meta])* $variant:ident, )+ }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        $vis enum $name {
            $( $(#[$vm])* $variant, )+
        }
    };
    (@impl $name:ident, $key:literal, $doc:literal, $default:expr,
        [$( ($ry:literal, $rm:literal, $rd:literal, $rg:literal) )?],
        { $( $variant:ident = $vname:literal => $quelle:literal, )+ }) => {
        impl $crate::policy::PolicyChoice for $name {
            const KEY: &'static str = $key;
            const DOC: &'static str = $doc;
            const DEFAULT: $crate::policy::PolicyDefault<Self> = $default;
            const REVIEW_BY: ::core::option::Option<$crate::policy::ReviewBy> =
                $crate::policy_choice!(@review $( ($ry, $rm, $rd, $rg) )?);
            fn options() -> &'static [Self] {
                &[ $( $name::$variant, )+ ]
            }
            fn name(self) -> &'static str {
                match self { $( $name::$variant => $vname, )+ }
            }
            fn quelle(self) -> &'static str {
                match self { $( $name::$variant => $quelle, )+ }
            }
        }
    };
    (@review) => { ::core::option::Option::None };
    (@review ($ry:literal, $rm:literal, $rd:literal, $rg:literal)) => {
        ::core::option::Option::Some($crate::policy::ReviewBy {
            jahr: $ry, monat: $rm, tag: $rd, grund: $rg,
        })
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::policy_choice! {
        /// A fixture with a default.
        enum Lesart {
            key: "lesart",
            doc: "@/docs/reference/operator-policy.md#lesart",
            default: tightening(Additiv, "never cuts into the answer window"),
            review_by: (2027, 1, 1, "the fixture's law is enacted"),
            options: {
                Additiv = "additiv" => "Quelle A, S. 1",
                Wortlaut = "wortlaut" => "Quelle B, S. 2",
            }
        }
    }

    crate::policy_choice! {
        /// A fixture with no default.
        enum Referenz {
            key: "referenz",
            doc: "@/docs/reference/operator-policy.md#referenz",
            default: required,
            options: {
                Base = "base" => "Quelle C, S. 3",
                Peak = "peak" => "Quelle D, S. 4",
            }
        }
    }

    crate::policy_choice! {
        /// A yes/no fixture.
        enum JaNein {
            key: "ja_nein",
            doc: "@/docs/reference/operator-policy.md#ja-nein",
            default: tightening(Nein, "keeps today's behaviour"),
            options: {
                Nein = "false" => "Quelle E",
                Ja = "true" => "Quelle F",
            }
        }
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Section {
        lesart: Policy<Lesart>,
        #[serde(default)]
        ja_nein: Policy<JaNein>,
    }

    #[test]
    fn an_absent_key_resolves_to_the_tightening_default() {
        let s: Section = serde_json::from_str("{}").unwrap();
        assert_eq!(s.lesart.get(), Lesart::Additiv);
        assert_eq!(s.lesart.origin(), Origin::Default);
        assert_eq!(s.ja_nein.get(), JaNein::Nein);
    }

    #[test]
    fn a_set_key_is_explicit_and_a_boolean_reads_by_name() {
        let s: Section = serde_json::from_str(r#"{"lesart":"wortlaut","ja_nein":true}"#).unwrap();
        assert_eq!(s.lesart.get(), Lesart::Wortlaut);
        assert_eq!(s.lesart.origin(), Origin::Explicit);
        assert_eq!(s.ja_nein.get(), JaNein::Ja);
    }

    #[test]
    fn an_unknown_option_lists_the_admissible_ones() {
        let e = serde_json::from_str::<Section>(r#"{"lesart":"frei"}"#)
            .unwrap_err()
            .to_string();
        assert!(e.contains("`additiv`") && e.contains("`wortlaut`"), "{e}");
    }

    #[test]
    fn the_triple_and_the_entry_name_key_value_and_source() {
        let p = Policy::explicit(Lesart::Wortlaut);
        assert_eq!(
            p.triple(),
            PolicyTriple {
                key: "lesart".into(),
                value: "wortlaut".into(),
                quelle: "Quelle B, S. 2".into()
            }
        );
        let e = p.entry();
        assert_eq!(e.class, PolicyClass::Policy);
        assert_eq!(
            e.review_by,
            time::Date::from_calendar_date(2027, time::Month::January, 1).ok()
        );
        let report = PolicyReport::new()
            .with(&p)
            .posture_flag("allow_insecure_no_auth", true);
        let lines = report.lines("svc");
        assert_eq!(
            lines[0],
            "svc.lesart=wortlaut origin=explicit class=policy quelle=\"Quelle B, S. 2\""
        );
        assert!(lines[1].ends_with("insecure"), "{}", lines[1]);
        let day = time::Date::from_calendar_date(2027, time::Month::January, 1).unwrap();
        assert_eq!(report.review_due(day).len(), 1);
        assert!(report.review_due(day.previous_day().unwrap()).is_empty());
    }

    #[test]
    fn every_declared_review_date_is_a_date() {
        assert!(Lesart::REVIEW_BY.and_then(|r| r.datum()).is_some());
    }

    /// A `Required` setting missing is recorded with its options for the
    /// loader, and the deserialization fails as a missing field.
    #[test]
    fn a_missing_required_setting_names_its_options() {
        #[derive(Debug, serde::Deserialize)]
        struct S {
            #[expect(dead_code, reason = "only the failure is under test")]
            referenz: Policy<Referenz>,
        }
        let _ = take_missing_required();
        let e = serde_json::from_str::<S>("{}").unwrap_err().to_string();
        assert!(e.contains("missing field `referenz`"), "{e}");
        let missing = take_missing_required();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].key, "referenz");
        assert_eq!(missing[0].first, "base");
        assert_eq!(
            missing[0].options,
            vec![
                ("base".to_owned(), "Quelle C, S. 3".to_owned()),
                ("peak".to_owned(), "Quelle D, S. 4".to_owned())
            ]
        );
    }

    /// T001: a `Required` setting left out is reported with its options and
    /// their sources, beside an unrelated unknown key, in one error.
    #[test]
    fn the_loader_names_a_missing_required_setting_beside_every_other_problem() {
        #[derive(Debug, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PolicySection {
            #[expect(dead_code, reason = "only the failure is under test")]
            referenz: Policy<Referenz>,
            #[expect(dead_code, reason = "only the failure is under test")]
            lesart: Policy<Lesart>,
        }
        #[derive(Debug, serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Cfg {
            #[expect(dead_code, reason = "only the failure is under test")]
            policy: PolicySection,
            #[expect(dead_code, reason = "only the failure is under test")]
            tenant: String,
        }
        let dir = std::env::temp_dir().join(format!("policy-required-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("svc.toml");
        std::fs::write(
            &file,
            "tenant = \"t\"\nzzz_typo = 1\n\n[policy]\nlesart = \"wortlaut\"\n",
        )
        .unwrap();
        let err = crate::load_config_file::<Cfg>("policy-required-fixture", &file).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        let problems = err.problems();
        assert!(
            problems.contains(&crate::ConfigProblem::PolicyRequired {
                path: "policy.referenz".into(),
                options: vec![
                    ("base".into(), "Quelle C, S. 3".into()),
                    ("peak".into(), "Quelle D, S. 4".into()),
                ],
            }),
            "{err}"
        );
        assert!(
            problems.contains(&crate::ConfigProblem::Unknown("zzz_typo".into())),
            "{err}"
        );
        let text = err.to_string();
        assert!(text.contains("`base` (Quelle C, S. 3)"), "{text}");
    }

    /// T004: `GET /api/v1/policy` is behind the daemon's authentication and
    /// the Cedar action `read-policy`.
    #[cfg(all(feature = "cedar", feature = "oidc"))]
    #[tokio::test]
    async fn the_policy_route_refuses_an_unauthenticated_caller_and_serves_the_report() {
        use std::sync::Arc;

        use axum::Extension;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt as _;

        let verifier = crate::oidc::OidcVerifier::from_jwks_for_testing(
            "https://idp.example",
            "svc",
            jsonwebtoken::jwk::JwkSet { keys: vec![] },
        )
        .with_service_keys(vec![crate::oidc::ServiceKey::new(
            secrecy::SecretString::from("k-1".to_owned()),
            "caller",
            "t1",
            vec![],
            vec![],
        )]);
        let cedar = crate::cedar::CedarEnforcer::from_policy_str(
            r#"permit(principal, action == Action::"read-policy", resource)
               when { context.principal_tenant == context.resource_tenant };"#,
        )
        .unwrap();
        let report = PolicyReport::new().with(&Policy::explicit(Lesart::Wortlaut));
        let app = router(report)
            .layer(Extension(crate::oidc::ExpectedTenant("t1".to_owned())))
            .layer(Extension(Arc::new(cedar)))
            .layer(Extension(verifier));
        let call = |auth: Option<&str>| {
            let mut req = Request::get("/api/v1/policy");
            if let Some(a) = auth {
                req = req.header("authorization", format!("Bearer {a}"));
            }
            app.clone().oneshot(req.body(Body::empty()).unwrap())
        };
        let unauthenticated = call(None).await.unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
        let ok = call(Some("k-1")).await.unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body = axum::body::to_bytes(ok.into_body(), 1 << 16).await.unwrap();
        let served: PolicyReport = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            served.get("lesart").map(|e| e.value.as_str()),
            Some("wortlaut")
        );
    }
}
