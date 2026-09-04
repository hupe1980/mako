//! Which profile validates a message: the registry maps `(MessageType,
//! Release)` to a [`Profile`] and resolves the Formatversion in force on a
//! given date.
//!
//! [`ReleaseRegistry::global()`] holds the profiles embedded at build time;
//! [`ReleaseRegistry::new`] builds one from any set of profiles.
use std::sync::OnceLock;

pub use crate::profile::Profile;
use crate::{Error, MessageType, Pruefidentifikator, Release};

// ── PidSource ─────────────────────────────────────────────────────────────────

/// Where a message type carries its Prüfidentifikator.
///
/// Read from the profile at parse time, so the extraction strategy follows
/// the AHB rather than a hardcoded message-type list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PidSource {
    /// `BGM` DE 1004 — the default for most EDI@Energy message types.
    #[default]
    BgmDe1004,
    /// The first `RFF+Z13` — UTILMD (per Vorgang), MSCONS, ORDERS, IFTSTA,
    /// COMDIS, PRICAT and the other types whose AHB names it there.
    RffZ13,
}

// ── TransitionState ───────────────────────────────────────────────────────────

/// Default receive tolerance, in calendar days, for a format that has expired.
///
/// **Zero, because EDIFACT has no transition window.** Allgemeine Festlegungen
/// 6.1 §2.5 gives the EDIFACT formats a single *Anwendungszeitpunkt* — the 1st
/// of April or October — and no overlap around it: before that instant the old
/// format applies, from it the new one does.
///
/// The 15-*Werktage* Übergangszeitraum in §8.5 is the **XML** rule and does not
/// transfer: it begins *at* the Anwendungszeitpunkt, counts Werktage rather than
/// calendar days, and selects the version by the *Erfüllungsdatum* stated in the
/// message rather than by when it was sent.
///
/// [`ReleaseRegistry::with_receive_tolerance_days`] raises this for operators who
/// choose to accept a late-arriving message in the superseded format — a local
/// inbound policy, not a licence to send late, and it extends only the trailing
/// edge.
pub const DEFAULT_RECEIVE_TOLERANCE_DAYS: i64 = 0;

/// The normative dispatch state for a `(MessageType, date, track)` triple.
///
/// Produced by [`ReleaseRegistry::transition_state`] and
/// [`ProcessContext::transition_state`].  Callers should use this to decide
/// which profile(s) to accept when validating incoming messages.
///
/// # Usage
///
/// ```rust,ignore
/// match ctx.transition_state(MessageType::Mscons, None) {
///     TransitionState::Stable { profile } => {
///         // Exactly one active profile — validate against it.
///     }
///     TransitionState::Transition { outgoing, incoming } => {
///         // Grace period: accept messages conforming to either version.
///         // Both profiles must be tried; reject only when both fail.
///     }
///     TransitionState::None => {
///         // No profile active yet — pre-launch period.
///     }
/// }
/// ```
pub enum TransitionState<'r> {
    /// Exactly one profile is active on this date.
    Stable {
        /// The currently active profile.
        profile: &'r Profile,
    },
    /// The successor has taken effect and the predecessor is still inside the
    /// configured receive tolerance.
    ///
    /// Send `incoming`: it is the format in force. Accept either on receipt,
    /// which is the whole point of a non-zero tolerance.
    ///
    /// With the default tolerance of zero this state never occurs, because
    /// EDIFACT changes at a single Anwendungszeitpunkt — see
    /// [`DEFAULT_RECEIVE_TOLERANCE_DAYS`].
    Transition {
        /// The superseded profile, still inside `valid_until + tolerance`.
        outgoing: &'r Profile,
        /// The profile in force from its `valid_from`.
        incoming: &'r Profile,
    },
    /// No profile with `valid_from ≤ date` exists for this `(type, track)`.
    None,
}

impl std::fmt::Debug for TransitionState<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransitionState::Stable { profile } => f
                .debug_struct("Stable")
                .field("release", &profile.release().as_str())
                .field("message_type", &profile.message_type())
                .finish(),
            TransitionState::Transition { outgoing, incoming } => f
                .debug_struct("Transition")
                .field("outgoing", &outgoing.release().as_str())
                .field("incoming", &incoming.release().as_str())
                .finish(),
            TransitionState::None => write!(f, "None"),
        }
    }
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Process-global registry mapping `(MessageType, Release)` → `&'static Profile`.
#[derive(Clone)]
pub struct ReleaseRegistry {
    profiles: Vec<&'static Profile>,
    /// Two-level index: `MessageType` → `release_str` → sorted list of profile indices.
    ///
    /// Multiple profiles may share the same wire release code (same `release` string)
    /// when BDEW issues a format revision without changing the EDIFACT version code
    /// (e.g. COMDIS `"1.0g"` appears in both `fv20251001` and `fv20261001`).
    /// The list is sorted ascending by `valid_from` so the date-aware selector can
    /// pick the correct entry efficiently.
    index: std::collections::HashMap<MessageType, std::collections::HashMap<Box<str>, Vec<usize>>>,
    /// Extra calendar days past `valid_until` during which a superseded format
    /// is still accepted on receipt.
    ///
    /// Defaults to [`DEFAULT_RECEIVE_TOLERANCE_DAYS`] (0 — the BDEW rule).
    receive_tolerance_days: i64,
}

static REGISTRY: OnceLock<std::sync::Arc<ReleaseRegistry>> = OnceLock::new();

impl ReleaseRegistry {
    /// Build a new, empty registry and populate it with `profiles`.
    ///
    /// Use this in tests and benchmarks that need an isolated registry.
    /// Application code should use [`ReleaseRegistry::global()`] instead.
    #[must_use]
    pub fn new(profiles: Vec<&'static Profile>) -> Self {
        let mut index: std::collections::HashMap<
            MessageType,
            std::collections::HashMap<Box<str>, Vec<usize>>,
        > = std::collections::HashMap::new();
        for (i, p) in profiles.iter().enumerate() {
            index
                .entry(p.message_type())
                .or_default()
                .entry(p.release().as_str().into())
                .or_default()
                .push(i);
        }
        // Sort each candidate list ascending by valid_from so date-range selection
        // (pick the latest valid_from that is ≤ query date) can use a simple scan.
        for type_map in index.values_mut() {
            for candidates in type_map.values_mut() {
                candidates.sort_by(|&a, &b| {
                    let va = profiles[a].valid_from();
                    let vb = profiles[b].valid_from();
                    va.cmp(&vb)
                });
            }
        }
        Self {
            profiles,
            index,
            receive_tolerance_days: DEFAULT_RECEIVE_TOLERANCE_DAYS,
        }
    }

    /// Accept a superseded format for `days` past its `valid_until`.
    ///
    /// The returned registry is independent of the global singleton. Use this
    /// for a tenant whose contract tolerates late-arriving messages in the old
    /// format — a receiving policy, not a BDEW rule; see
    /// [`DEFAULT_RECEIVE_TOLERANCE_DAYS`] for why the default is zero.
    ///
    /// # Example
    /// ```rust,no_run
    /// use edi_energy::registry::ReleaseRegistry;
    ///
    /// let lenient = ReleaseRegistry::global().clone().with_receive_tolerance_days(3);
    /// assert_eq!(lenient.receive_tolerance_days(), 3);
    /// ```
    #[must_use]
    pub fn with_receive_tolerance_days(mut self, days: i64) -> Self {
        self.receive_tolerance_days = days;
        self
    }

    /// The configured receive tolerance in calendar days.
    #[must_use]
    pub fn receive_tolerance_days(&self) -> i64 {
        self.receive_tolerance_days
    }

    /// Return a reference to the global registry.
    ///
    /// Initialises the registry on first call from the profiles embedded at
    /// build time. Subsequent calls are lock-free reads.
    #[must_use]
    pub fn global() -> &'static Self {
        Self::global_arc()
    }

    /// Return an `Arc` clone of the global registry.
    ///
    /// Use this when you need an owned `Arc<ReleaseRegistry>` — for instance when
    /// constructing a [`ProcessContext`] or spawning tasks that need registry access
    /// without borrowing from a `'static` reference.
    #[must_use]
    pub fn global_arc() -> &'static std::sync::Arc<ReleaseRegistry> {
        REGISTRY.get_or_init(|| {
            let mut profiles: Vec<&'static Profile> = Vec::new();
            crate::register_profiles(&mut profiles);
            std::sync::Arc::new(Self::new(profiles))
        })
    }

    /// Look up the profile for a `(message_type, release)` pair, **without
    /// disambiguating by date**: the last registered candidate wins.
    ///
    /// A wire release code can identify more than one profile — BDEW reuses the
    /// code across consecutive Formatversionen (INVOIC `"2.8e"`, UTILTS
    /// `"1.1e"`). Only a date separates those, and this crate does not read a
    /// clock: which day a message is judged on is the caller's fact, not the
    /// parser's. Use [`profile_on`][Self::profile_on] wherever the answer can
    /// depend on it.
    ///
    /// This form answers the questions a date cannot change — where the
    /// Prüfidentifikator sits in the message, which segments the layout has —
    /// because consecutive Formatversionen sharing one wire code share those.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ProfileNotFound`] when no profile has been registered for
    /// the given combination.
    pub fn profile(
        &self,
        message_type: MessageType,
        release: &Release,
    ) -> Result<&'static Profile, Error> {
        self.profile_on(message_type, release, time::Date::MAX)
    }

    /// Like [`profile`][Self::profile] but disambiguates same-wire-code profiles
    /// against `date` rather than today.
    ///
    /// Returns the profile with the greatest `valid_from ≤ date`.
    ///
    /// When all candidate profiles lack a `valid_from` (legacy profiles), the
    /// last registered one is returned as before.  When all candidates have a
    /// `valid_from` that is strictly in the future relative to `date`,
    /// [`Error::ProfileNotYetActive`] is returned — the previous behaviour of
    /// silently returning the most-future-dated profile is gone.
    ///
    /// # Wire-code collisions
    ///
    /// BDEW occasionally reuses the same EDIFACT release string across two
    /// consecutive format versions.  The date parameter is therefore **required**
    /// for correct dispatch — `profile()` falls back to today when called without
    /// an explicit date.
    ///
    /// Known collisions (as of 2026):
    ///
    /// | Message type | Wire code | Profiles |
    /// |--------------|-----------|---------|
    /// | INVOIC       | `"2.8e"`  | `fv20251001` (valid from 2025-10-01) and `fv20260401` (valid from 2026-04-01) |
    /// | UTILTS       | `"1.1e"`  | `fv20241001` (valid from 2024-10-01) and `fv20260401` (valid from 2026-04-01) |
    ///
    /// UTILMD Strom and Gas do **not** have collisions: the FV2024-10-01 profiles
    /// carry wire codes `"S1.1a"` / `"G1.0a"`, while `fv20251001` uses `"S2.1"` /
    /// `"G1.1"` — distinct codes, no disambiguation needed.
    ///
    /// When a collision is resolved by date, a `DEBUG`-level trace event is
    /// emitted (requires the `tracing` feature):
    ///
    /// ```text
    /// edi_energy::registry: wire_code_collision_resolved
    ///   message_type=INVOIC release="2.8e" date=2025-11-01
    ///   resolved_to="fv20251001" candidates=2
    /// ```
    ///
    /// # Errors
    ///
    /// - [`Error::ProfileNotFound`] — no profile has been registered for the
    ///   given message type + release combination.
    /// - [`Error::ProfileNotYetActive`] — profiles are registered but none has
    ///   `valid_from ≤ date`; the nearest future profile is reported as context.
    ///
    /// # Panics
    ///
    /// Panics if the internal candidate index is inconsistent (should be
    /// unreachable with a correctly built registry).
    pub fn profile_on(
        &self,
        message_type: MessageType,
        release: &Release,
        date: time::Date,
    ) -> Result<&'static Profile, Error> {
        let candidates = self
            .index
            .get(&message_type)
            .and_then(|m| m.get(release.as_str()))
            .ok_or_else(|| Error::ProfileNotFound {
                message_type,
                release: release.clone(),
            })?;

        // Pick the candidate with the greatest valid_from ≤ date.
        if let Some(&idx) = candidates
            .iter()
            .rfind(|&&i| self.profiles[i].valid_from().is_none_or(|vf| vf <= date))
        {
            // When multiple profiles share the same wire code (collision), emit a
            // debug trace so operators can verify the correct profile was selected.
            #[cfg(feature = "tracing")]
            if candidates.len() > 1 {
                tracing::debug!(
                    message_type = ?message_type,
                    release = release.as_str(),
                    date = %date,
                    resolved_to = self.profiles[idx].release().as_str(),
                    candidates = candidates.len(),
                    "wire_code_collision_resolved",
                );
            }
            return Ok(self.profiles[idx]);
        }

        // No candidate satisfied valid_from ≤ date.
        // If every candidate lacks a valid_from, they are legacy/undated profiles;
        // return the last one (pre-existing behaviour for legacy profiles).
        let all_undated = candidates
            .iter()
            .all(|&i| self.profiles[i].valid_from().is_none());
        if all_undated {
            let idx = *candidates.last().expect("candidates is non-empty");
            return Ok(self.profiles[idx]);
        }

        // At least one candidate has a valid_from, but all are in the future.
        // Return ProfileNotYetActive with the earliest future profile as context.
        let (_earliest_idx, earliest_vf) = candidates
            .iter()
            .filter_map(|&i| self.profiles[i].valid_from().map(|vf| (i, vf)))
            .min_by_key(|(_, vf)| *vf)
            .expect("at least one dated candidate exists");
        Err(Error::ProfileNotYetActive {
            message_type,
            release: release.clone(),
            valid_from: earliest_vf,
            date,
        })
    }

    /// Return all registered profiles.
    #[must_use]
    pub fn all_profiles(&self) -> &[&'static Profile] {
        &self.profiles
    }

    /// Return all profiles for a given message type.
    pub fn profiles_for(
        &self,
        message_type: MessageType,
    ) -> impl Iterator<Item = &'static Profile> + '_ {
        self.profiles
            .iter()
            .filter(move |p| p.message_type() == message_type)
            .copied()
    }

    /// Returns `true` if at least one registered profile for `message_type`
    /// carries real AHB rules for `pid`.
    ///
    /// Returns `false` when no profile exists for `message_type`, or when every
    /// matching profile falls back to the stand-in pack — the case for unknown
    /// or not-yet-imported Prüfidentifikatoren.
    ///
    /// Use this to detect **vacuous validation**: `msg.validate()` returns
    /// `Ok(report)` with `report.is_valid() == true` for an unregistered PID,
    /// so the report alone cannot distinguish a genuinely-validated message
    /// from one that passed because no rules applied to it.
    ///
    /// # Detection idiom
    ///
    /// Whether the profile has a column (Prüfschablone) for the code. A code
    /// without one validates with the `AHB-UNKNOWN-PID` warning and no AHB
    /// rules — `is_valid()` says nothing about it.
    ///
    /// # Example
    /// ```rust,no_run
    /// use edi_energy::{MessageType, Pruefidentifikator, registry::ReleaseRegistry};
    ///
    /// let reg = ReleaseRegistry::global();
    /// assert!(reg.pid_has_ahb_rules(MessageType::Utilmd, Pruefidentifikator::new(55001).unwrap()));
    /// assert!(!reg.pid_has_ahb_rules(MessageType::Utilmd, Pruefidentifikator::new(56001).unwrap()));
    /// ```
    #[must_use]
    pub fn pid_has_ahb_rules(&self, message_type: MessageType, pid: Pruefidentifikator) -> bool {
        self.profiles_for(message_type)
            .any(|p| p.has_anwendungsfall(pid))
    }

    /// Return all known releases for a message type in ascending semantic order.
    ///
    /// Within-track releases (e.g. `"2.5a"` → `"2.10a"`) are ordered correctly
    /// by their numeric components.  Cross-track releases fall back to byte order.
    ///
    /// Returns an empty vec when no profiles are registered for `message_type`.
    #[must_use]
    pub fn releases(&self, message_type: MessageType) -> Vec<&Release> {
        let mut releases: Vec<&Release> = self
            .profiles
            .iter()
            .filter(|p| p.message_type() == message_type)
            .map(|p| p.release())
            .collect();
        releases.sort();
        releases.dedup_by_key(|r| r.as_str());
        releases
    }

    /// Return all distinct BDEW format-version strings present in the registry,
    /// sorted chronologically.
    ///
    /// A format version is derived from each profile's [`Profile::valid_from`]
    /// date: a profile with `valid_from = 2025-10-01` contributes
    /// `"FV2025-10-01"`.  Profiles without a `valid_from` date are skipped.
    ///
    /// The returned strings are suitable for passing directly to
    /// `FormatVersion::parse` in `mako-engine`.
    ///
    /// # Example
    /// ```rust,no_run
    /// use edi_energy::registry::ReleaseRegistry;
    ///
    /// let fvs = ReleaseRegistry::global().format_versions();
    /// assert!(fvs.iter().any(|v| v == "FV2025-10-01"));
    /// ```
    #[must_use]
    pub fn format_versions(&self) -> Vec<String> {
        let mut dates: Vec<time::Date> = self
            .profiles
            .iter()
            .filter_map(|p| p.valid_from())
            .collect();
        dates.sort();
        dates.dedup();
        dates
            .into_iter()
            .map(|d| format!("FV{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day()))
            .collect()
    }

    /// Return the semantically greatest (latest) release for `message_type`
    /// within the same release track as `reference`.
    ///
    /// Only profiles on `reference`'s own track are considered, so the result
    /// never depends on the cross-track order, which carries no BDEW meaning.
    ///
    /// Returns `None` when no profiles on the same track are registered.
    #[must_use]
    pub fn latest_for_track(
        &self,
        message_type: MessageType,
        reference: &Release,
    ) -> Option<&Release> {
        let kind = reference.kind();
        let mut releases: Vec<&Release> = self
            .profiles
            .iter()
            .filter(|p| p.message_type() == message_type && p.release().kind().same_track(&kind))
            .map(|p| p.release())
            .collect();
        releases.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        releases.into_iter().next_back()
    }

    /// Find the profile active on `date` for messages on a specific release track.
    ///
    /// Use this when a message type has multiple parallel format tracks —
    /// for example UTILMD Strom (`ReleaseTrack::Strom`) and UTILMD Gas
    /// (`ReleaseTrack::Gas`).
    ///
    /// Dispatch uses the explicit [`ReleaseTrack`](crate::ReleaseTrack) enum
    /// rather than string-prefix matching, so a future BDEW track whose codes
    /// share a prefix with an existing one cannot be silently folded into it.
    ///
    /// Returns the profile with the greatest `valid_from ≤ date` among those
    /// on the given track.  Returns `None` when no matching profile is found.
    #[must_use]
    pub fn profile_for_date_and_track(
        &self,
        message_type: MessageType,
        date: time::Date,
        track: crate::release::ReleaseTrack,
    ) -> Option<&'static Profile> {
        // Deterministic tie-breaking: latest valid_from wins; release string as secondary key.
        self.profiles
            .iter()
            .filter(|p| p.message_type() == message_type)
            .filter(|p| p.release().track() == track)
            .filter_map(|p| p.valid_from().map(|vf| (vf, *p)))
            .filter(|(vf, _)| *vf <= date)
            .max_by(|(va, pa), (vb, pb)| {
                va.cmp(vb)
                    .then(pa.release().as_str().cmp(pb.release().as_str()))
            })
            .map(|(_, p)| p)
    }

    /// Determine whether a message with the given wire `release` code is
    /// acceptable on `date`.
    ///
    /// Returns `true` when a profile for `(message_type, release)` exists and
    /// `date` falls in `[valid_from, valid_until + receive_tolerance_days]`.
    ///
    /// The leading edge is hard — Allgemeine Festlegungen 6.1 §2.5 puts the
    /// EDIFACT change at a single Anwendungszeitpunkt — so a format is never
    /// acceptable before it takes effect, whatever the tolerance is set to.
    #[must_use]
    pub fn is_acceptable_on(
        &self,
        message_type: MessageType,
        release: &Release,
        date: time::Date,
    ) -> bool {
        let Ok(profile) = self.profile_on(message_type, release, date) else {
            return false;
        };
        let Some(valid_from) = profile.valid_from() else {
            // No date information — assume always valid (legacy profile).
            return true;
        };
        if date < valid_from {
            return false;
        }
        if let Some(valid_until) = profile.valid_until() {
            let tolerance_end = valid_until + time::Duration::days(self.receive_tolerance_days);
            if date > tolerance_end {
                return false;
            }
        }
        true
    }

    /// Return the [`TransitionState`] for `(message_type, date)`.
    ///
    /// When `track` is `Some`, only profiles on that release track are
    /// considered (e.g. [`ReleaseTrack::Strom`](crate::ReleaseTrack) for UTILMD
    /// Strom). This takes the typed track rather than a string prefix because
    /// prefix matching is the failure mode [`ReleaseTrack`](crate::ReleaseTrack)
    /// was introduced to remove — `"S"` also prefixes any future BDEW code that
    /// happens to start with it.
    ///
    /// # Algorithm
    ///
    /// 1. Find the profile with the greatest `valid_from ≤ date` — the one in
    ///    force.
    /// 2. Find its predecessor: the profile whose `valid_until + tolerance` has
    ///    not yet passed on `date`.
    /// 3. Return `Transition { outgoing, incoming }` when such a predecessor
    ///    exists, `Stable { profile }` otherwise.
    ///
    /// With the default zero tolerance step 2 never matches, so this reports
    /// `Stable` on every date a profile is in force — which is what Allgemeine
    /// Festlegungen §2.5 describes.
    #[must_use]
    pub fn transition_state(
        &self,
        message_type: MessageType,
        date: time::Date,
        track: Option<crate::ReleaseTrack>,
    ) -> TransitionState<'_> {
        // Candidate profiles restricted to the given track (if any).
        let candidates: Vec<&Profile> = self
            .profiles
            .iter()
            .filter(|p| p.message_type() == message_type)
            .filter(|p| track.is_none_or(|t| p.release().track() == t))
            .copied()
            .collect();

        // Step 1: current = profile with greatest valid_from ≤ date.
        // Tie-break by release string (descending) for determinism.
        let current = candidates
            .iter()
            .filter_map(|p| p.valid_from().map(|vf| (vf, *p)))
            .filter(|(vf, _)| *vf <= date)
            .max_by(|(va, pa), (vb, pb)| {
                va.cmp(vb)
                    .then(pa.release().as_str().cmp(pb.release().as_str()))
            })
            .map(|(_, p)| p);

        let Some(current) = current else {
            return TransitionState::None;
        };

        // Step 2: is a superseded profile still inside the receive tolerance?
        // The search runs backwards from the profile in force, because the
        // overlap the tolerance creates is always trailing: a format is never
        // acceptable before its own Anwendungszeitpunkt.
        let tolerance = time::Duration::days(self.receive_tolerance_days);
        let current_valid_from = current.valid_from().unwrap_or(date);
        let outgoing = candidates
            .iter()
            .filter(|p| p.valid_from().is_some_and(|vf| vf < current_valid_from))
            .filter(|p| {
                p.valid_until()
                    .is_some_and(|vu| vu < date && date <= vu + tolerance)
            })
            .max_by_key(|p| p.valid_from())
            .copied();

        match outgoing {
            Some(outgoing) => TransitionState::Transition {
                outgoing,
                incoming: current,
            },
            None => TransitionState::Stable { profile: current },
        }
    }
}

// ── ProcessContext ────────────────────────────────────────────────────────────

/// A calendar-date-aware dispatch context for building and validating
/// EDI@Energy messages.
///
/// `ProcessContext` selects the normatively-active profile for each message
/// type based on a date, so callers never need to hard-code release strings.
/// When BDEW publishes a new format version and the profile is registered, a
/// context anchored to a later date picks it up on the effective date — **no
/// code change at the call site is required**.
///
/// The date is always the caller's: a Formatversion changes at German midnight,
/// so the caller states which day it means — a service reads that from
/// `mako_fristen::heute()` — rather than having a parser read a clock.
///
/// Both global and isolated registries are supported:
/// - [`ProcessContext::for_date()`] uses the process-global registry (suitable
///   for single-process applications).
/// - [`Platform::process_context()`][crate::Platform::process_context] creates a
///   context backed by the platform's own isolated registry (required for
///   multi-tenant servers and test isolation).
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::ProcessContext;
///
/// // The caller states the day; a service reads it from `mako_fristen::heute()`.
/// let ctx = ProcessContext::for_date(time::macros::date!(2026 - 04 - 01));
/// let release = ctx.active_release(edi_energy::MessageType::Mscons)
///     .expect("MSCONS profile must be registered");
/// assert!(!release.as_str().is_empty());
/// ```
#[derive(Clone)]
pub struct ProcessContext {
    date: time::Date,
    registry: std::sync::Arc<ReleaseRegistry>,
}

impl ProcessContext {
    /// Create a context for messages valid on a specific `date`, backed by
    /// the process-global registry.
    ///
    /// Use [`Platform::process_context()`][crate::Platform::process_context] instead
    /// when working with an isolated registry.
    #[must_use]
    pub fn for_date(date: time::Date) -> Self {
        Self {
            date,
            registry: std::sync::Arc::clone(ReleaseRegistry::global_arc()),
        }
    }

    /// Create a context for `date` backed by an explicit registry.
    ///
    /// This is the low-level constructor used by
    /// [`Platform::process_context()`][crate::Platform::process_context].
    /// Most application code should use [`ProcessContext::for_date`].
    #[must_use]
    pub fn for_date_with_registry(
        date: time::Date,
        registry: std::sync::Arc<ReleaseRegistry>,
    ) -> Self {
        Self { date, registry }
    }

    /// The date this context is anchored to.
    #[must_use]
    pub fn date(&self) -> time::Date {
        self.date
    }

    /// Return the release that is normatively active for `message_type` on
    /// this context's date.
    ///
    /// Returns `None` when no profile with a `valid_from` ≤ `date` is
    /// registered for the type.  For message types with multiple parallel tracks
    /// (e.g. UTILMD Strom / Gas), use [`active_release_for_track`][Self::active_release_for_track]
    /// to get a deterministic result.
    #[must_use]
    pub fn active_release(&self, message_type: MessageType) -> Option<&Release> {
        self.registry
            .profiles_for(message_type)
            .filter_map(|p| p.valid_from().map(|vf| (vf, p)))
            .filter(|(vf, _)| *vf <= self.date)
            .max_by(|(va, pa), (vb, pb)| {
                va.cmp(vb)
                    .then(pa.release().as_str().cmp(pb.release().as_str()))
            })
            .map(|(_, p)| p.release())
    }

    /// Return the profile that is normatively active for `message_type` on
    /// this context's date.
    ///
    /// Returns `None` when no profile is active on this date.  For message types
    /// with multiple parallel tracks, use [`active_profile_for_track`][Self::active_profile_for_track].
    #[must_use]
    pub fn active_profile(&self, message_type: MessageType) -> Option<&'static Profile> {
        self.registry
            .profiles_for(message_type)
            .filter_map(|p| p.valid_from().map(|vf| (vf, p)))
            .filter(|(vf, _)| *vf <= self.date)
            .max_by(|(va, pa), (vb, pb)| {
                va.cmp(vb)
                    .then(pa.release().as_str().cmp(pb.release().as_str()))
            })
            .map(|(_, p)| p)
    }

    /// Return the release active for `message_type` on this date, restricted
    /// to profiles on the given `track`.
    ///
    /// Use this when a message type has parallel format tracks:
    /// - UTILMD Strom: `track = ReleaseTrack::Strom`
    /// - UTILMD Gas:   `track = ReleaseTrack::Gas`
    ///
    /// Returns `None` when no profile on the given track is active on this date.
    #[must_use]
    pub fn active_release_for_track(
        &self,
        message_type: MessageType,
        track: crate::release::ReleaseTrack,
    ) -> Option<&Release> {
        self.registry
            .profile_for_date_and_track(message_type, self.date, track)
            .map(Profile::release)
    }

    /// Return the profile active for `message_type` on this date, restricted
    /// to the given release track.  See [`active_release_for_track`][Self::active_release_for_track].
    #[must_use]
    pub fn active_profile_for_track(
        &self,
        message_type: MessageType,
        track: crate::release::ReleaseTrack,
    ) -> Option<&'static Profile> {
        self.registry
            .profile_for_date_and_track(message_type, self.date, track)
    }

    /// Return the [`TransitionState`] for `message_type` on this context's date.
    ///
    /// When `track` is `Some`, only profiles on that release track are
    /// considered. Pass `None` for message types with a single release track.
    ///
    /// See [`TransitionState`] for the three possible outcomes and their
    /// handling implications.
    #[must_use]
    pub fn transition_state(
        &self,
        message_type: MessageType,
        track: Option<crate::ReleaseTrack>,
    ) -> TransitionState<'_> {
        self.registry
            .transition_state(message_type, self.date, track)
    }

    /// Check whether a message carrying the given wire `release` code is
    /// normatively acceptable on this context's date.
    ///
    /// Delegates to [`ReleaseRegistry::is_acceptable_on`].
    #[must_use]
    pub fn is_acceptable(&self, message_type: MessageType, release: &Release) -> bool {
        self.registry
            .is_acceptable_on(message_type, release, self.date)
    }
}
