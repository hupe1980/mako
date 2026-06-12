/// Explicit application handle for multi-tenant and test-isolated EDI@Energy processing.
///
/// A [`Platform`] bundles a custom [`ReleaseRegistry`] with optional configuration
/// so multiple platform instances can coexist in the same process — each with its
/// own profile subset, grace-period override, or test fixtures.
///
/// # Motivation
///
/// The top-level functions ([`crate::parse`], [`crate::parse_interchange`], …) use
/// [`ReleaseRegistry::global()`], which is a process-singleton initialised on first
/// use.  This is convenient for simple applications but prevents:
///
/// - **Test isolation** — tests that manipulate registered profiles cannot run
///   concurrently without interfering.
/// - **Multi-tenant gateways** — an AS4 gateway serving both Strom and Gas tenants
///   with different profile subsets cannot maintain separate registries via globals.
/// - **Hot-reload** — incorporating a new release requires a process restart.
///
/// `Platform` solves these by owning an explicit `Arc<ReleaseRegistry>` that callers
/// build and configure.
///
/// # Usage
///
/// ```rust,no_run
/// use edi_energy::Platform;
///
/// // All built-in profiles:
/// let platform = Platform::with_all_profiles();
///
/// let input = b"UNB+UNOA:3+...";
/// let msg = platform.parse(input)?;
/// # Ok::<(), edi_energy::Error>(())
/// ```
///
/// # Custom profile subset
///
/// ```rust,ignore
/// use edi_energy::{Platform, registry::{Profile, ReleaseRegistry}};
///
/// let mut profiles: Vec<&'static dyn Profile> = Vec::new();
/// my_profiles::register(&mut profiles);
/// let platform = Platform::new(ReleaseRegistry::new(profiles));
/// ```
use std::sync::Arc;

use crate::{AnyMessage, Error, ParseConfig, generated, registry::ReleaseRegistry};
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
use crate::{EdiEnergyReport, Release};

/// An explicit EDI@Energy processing context.
///
/// See module-level docs for a full explanation of when to use this
/// instead of the top-level free functions.
#[derive(Clone)]
pub struct Platform {
    registry: Arc<ReleaseRegistry>,
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Platform")
            .field("profiles", &self.registry.all_profiles().len())
            .finish()
    }
}

impl Platform {
    /// Create a platform backed by a custom registry.
    ///
    /// Use [`Platform::with_all_profiles()`] for the standard set of registered
    /// profiles, or build a [`ReleaseRegistry`] from a custom profile list for
    /// test isolation or subset deployments.
    #[must_use]
    pub fn new(registry: ReleaseRegistry) -> Self {
        Self {
            registry: Arc::new(registry),
        }
    }

    /// Create a platform with all built-in profiles registered.
    ///
    /// Equivalent to calling `Platform::new(ReleaseRegistry::with_all_profiles())`
    /// but more convenient.  Unlike [`ReleaseRegistry::global()`], each call creates
    /// a fresh, independent registry — useful for test isolation.
    #[must_use]
    pub fn with_all_profiles() -> Self {
        let mut profiles: Vec<&'static dyn crate::registry::Profile> = Vec::new();
        generated::register_profiles(&mut profiles);
        Self::new(ReleaseRegistry::new(profiles))
    }

    /// Override the transition grace period for this platform's registry.
    ///
    /// The BDEW default is 7 calendar days (GPKE §10, `WiM` §12).  Use this when
    /// a specific tenant contract or test scenario requires a different window.
    ///
    /// Returns `self` for builder chaining:
    /// ```rust,no_run
    /// use edi_energy::Platform;
    /// let platform = Platform::with_all_profiles().with_transition_grace_days(14);
    /// ```
    #[must_use]
    pub fn with_transition_grace_days(self, days: i64) -> Self {
        let registry = Arc::try_unwrap(self.registry)
            .unwrap_or_else(|arc| (*arc).clone())
            .with_transition_grace_days(days);
        Self {
            registry: Arc::new(registry),
        }
    }

    /// Parse an EDIFACT/EDI@Energy byte slice using the platform's registry.
    ///
    /// Unlike the free-function [`crate::parse`], this method uses the platform's
    /// own [`ReleaseRegistry`] for PID-source lookup and profile dispatch.
    /// Two platform instances with disjoint profile subsets will each resolve
    /// Prüfidentifikatoren against their own registry.
    ///
    /// # Errors
    ///
    /// Returns `Err` when the byte slice cannot be parsed as valid EDIFACT.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, input),
            fields(
                bytes = input.len(),
                // message_type and release are recorded after parsing via Span::current().record(...)
                message_type = tracing::field::Empty,
                release = tracing::field::Empty,
                pruefidentifikator = tracing::field::Empty,
            )
        )
    )]
    pub fn parse(&self, input: &[u8]) -> Result<AnyMessage, Error> {
        let msg = crate::parse::parse_with_registry(input, ParseConfig::default(), &self.registry)?;
        // Record structured span fields after parsing (F-034 fix).
        #[cfg(feature = "tracing")]
        {
            let span = tracing::Span::current();
            if let Some(mt) = msg.try_message_type() {
                span.record("message_type", mt.as_str());
            }
            if let Ok(release) = crate::EdiEnergyMessage::detect_release(&msg) {
                span.record("release", release.as_str());
            }
            if let Ok(pid) = crate::EdiEnergyMessage::detect_pruefidentifikator(&msg) {
                span.record("pruefidentifikator", pid.as_u32());
            }
        }
        Ok(msg)
    }

    /// Parse with explicit [`ParseConfig`], using the platform's registry.
    ///
    /// # Errors
    ///
    /// Returns `Err` when parsing fails.
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, input),
            fields(
                bytes = input.len(),
                message_type = tracing::field::Empty,
                release = tracing::field::Empty,
                pruefidentifikator = tracing::field::Empty,
            )
        )
    )]
    pub fn parse_with_config(
        &self,
        input: &[u8],
        config: ParseConfig,
    ) -> Result<AnyMessage, Error> {
        let msg = crate::parse::parse_with_registry(input, config, &self.registry)?;
        #[cfg(feature = "tracing")]
        {
            let span = tracing::Span::current();
            if let Some(mt) = msg.try_message_type() {
                span.record("message_type", mt.as_str());
            }
            if let Ok(release) = crate::EdiEnergyMessage::detect_release(&msg) {
                span.record("release", release.as_str());
            }
            if let Ok(pid) = crate::EdiEnergyMessage::detect_pruefidentifikator(&msg) {
                span.record("pruefidentifikator", pid.as_u32());
            }
        }
        Ok(msg)
    }

    /// Parse all messages from an EDIFACT interchange, using the platform's registry.
    ///
    /// Returns a lazy iterator yielding one [`AnyMessage`] per UNH…UNT window.
    /// PID extraction and profile dispatch use this platform's registry, not the
    /// global singleton.
    pub fn parse_interchange(
        &self,
        reader: impl std::io::Read,
    ) -> impl Iterator<Item = Result<AnyMessage, Error>> {
        self.parse_interchange_with_config(reader, ParseConfig::default())
    }

    /// Parse all messages from an interchange with explicit [`ParseConfig`], using
    /// the platform's registry.
    pub fn parse_interchange_with_config(
        &self,
        reader: impl std::io::Read,
        config: ParseConfig,
    ) -> impl Iterator<Item = Result<AnyMessage, Error>> {
        crate::parse::parse_interchange_with_arc_registry(
            reader,
            config,
            Arc::clone(&self.registry),
        )
    }

    /// Validate `message` using this platform's registry instead of the global one.
    ///
    /// Useful for testing with a stripped-down or synthetic registry that does not
    /// contain production profiles.
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::ProfileNotFound)` when the message's release is not
    /// registered in this platform's registry.
    ///
    /// Returns `Err(Error::UnknownMessageType)` for [`AnyMessage::Unknown`].
    #[cfg(any(
        feature = "utilmd",
        feature = "mscons",
        feature = "aperak",
        feature = "contrl",
        feature = "invoic",
        feature = "remadv",
        feature = "orders",
        feature = "iftsta",
        feature = "insrpt",
        feature = "reqote",
        feature = "partin",
        feature = "ordchg",
        feature = "ordrsp",
        feature = "quotes",
        feature = "comdis",
        feature = "pricat",
        feature = "utilts",
    ))]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(skip(self, message), fields(message_type = ?message.try_message_type()))
    )]
    pub fn validate(&self, message: &AnyMessage) -> Result<EdiEnergyReport, Error> {
        let core = message
            .message_core()
            .ok_or_else(|| Error::UnknownMessageType {
                raw_code: crate::error::sanitize_code(
                    message.try_message_type().map_or("Unknown", |t| t.as_str()),
                ),
            })?;
        let release = core.detect_release()?;
        core.validate_against_with_semantic_and_registry(release, None, &self.registry)
    }

    /// Validate `message` against an explicit `release`, using this platform's registry.
    ///
    /// # Errors
    ///
    /// Returns `Err(Error::ProfileNotFound)` when `release` is not registered.
    ///
    /// Returns `Err(Error::UnknownMessageType)` for [`AnyMessage::Unknown`].
    #[cfg(any(
        feature = "utilmd",
        feature = "mscons",
        feature = "aperak",
        feature = "contrl",
        feature = "invoic",
        feature = "remadv",
        feature = "orders",
        feature = "iftsta",
        feature = "insrpt",
        feature = "reqote",
        feature = "partin",
        feature = "ordchg",
        feature = "ordrsp",
        feature = "quotes",
        feature = "comdis",
        feature = "pricat",
        feature = "utilts",
    ))]
    #[cfg_attr(
        feature = "tracing",
        tracing::instrument(
            skip(self, message),
            fields(message_type = ?message.try_message_type(), release = %release.as_str())
        )
    )]
    pub fn validate_against(
        &self,
        message: &AnyMessage,
        release: &Release,
    ) -> Result<EdiEnergyReport, Error> {
        let core = message
            .message_core()
            .ok_or_else(|| Error::UnknownMessageType {
                raw_code: crate::error::sanitize_code(
                    message.try_message_type().map_or("Unknown", |t| t.as_str()),
                ),
            })?;
        core.validate_against_with_semantic_and_registry(release, None, &self.registry)
    }

    /// A reference to the underlying [`ReleaseRegistry`].
    #[must_use]
    pub fn registry(&self) -> &ReleaseRegistry {
        &self.registry
    }

    /// Return an `Arc` clone of the underlying registry.
    ///
    /// Use this to share the registry across threads or to construct a
    /// [`crate::registry::ProcessContext`] manually.
    #[must_use]
    pub fn registry_arc(&self) -> Arc<ReleaseRegistry> {
        Arc::clone(&self.registry)
    }

    /// Create a [`crate::registry::ProcessContext`] anchored to `date`, backed
    /// by this platform's isolated registry.
    ///
    /// Use this instead of [`crate::ProcessContext::for_date`] when working
    /// with a [`Platform`] that holds a custom or test-isolated registry.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use edi_energy::Platform;
    ///
    /// let platform = Platform::with_all_profiles();
    /// let ctx = platform.process_context(time::Date::from_calendar_date(2026, time::Month::January, 1).unwrap());
    /// ```
    #[must_use]
    pub fn process_context(&self, date: time::Date) -> crate::registry::ProcessContext {
        crate::registry::ProcessContext::for_date_with_registry(date, Arc::clone(&self.registry))
    }

    /// Create a [`crate::registry::ProcessContext`] anchored to today's UTC date,
    /// backed by this platform's isolated registry.
    #[must_use]
    pub fn current_context(&self) -> crate::registry::ProcessContext {
        let today = time::OffsetDateTime::now_utc().date();
        self.process_context(today)
    }

    /// Check whether the wire release code in `envelope` is normatively
    /// acceptable on `date`, using this platform's registry.
    ///
    /// This is the platform-aware alternative to
    /// `MessageEnvelope::is_wire_code_acceptable_on_global`: it uses the
    /// platform's own [`ReleaseRegistry`] so that test registries and
    /// multi-tenant configurations are respected (F-007 fix).
    #[must_use]
    pub fn is_wire_code_acceptable_on(
        &self,
        envelope: &crate::interchange::MessageEnvelope,
        date: time::Date,
    ) -> bool {
        envelope.is_wire_code_acceptable_on(date, &self.registry)
    }

    /// Warm up all `LazyLock` rule-pack statics across every registered profile.
    ///
    /// Triggers eager initialisation of every MIG and AHB union rule pack so
    /// that the first real validation call incurs no latency spike (F-010 fix).
    /// Call this once during service startup, before the first request is accepted.
    pub fn warm_up(&self) {
        for profile in self.registry.all_profiles() {
            // Force initialisation of the LazyLock<Arc<ProfileRulePack>> statics.
            let _ = profile.mig_rule_pack();
            let _ = profile.ahb_rule_pack(None);
        }
    }
}
