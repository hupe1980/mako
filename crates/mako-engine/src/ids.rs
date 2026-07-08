//! Typed identifier newtypes for all engine-layer concepts.
//!
//! All identifiers are UUID v4 wrappers to guarantee global uniqueness without
//! coordination. They are distinct types so the compiler rejects mixing them up
//! at the call site.

use std::fmt;

use uuid::Uuid;

macro_rules! define_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        pub struct $name(Uuid);

        impl $name {
            /// Generate a fresh random identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub fn from_uuid(u: Uuid) -> Self {
                Self(u)
            }

            /// Return the underlying UUID.
            #[must_use]
            pub fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }
    };
}

define_id!(
    EventId,
    "Globally unique identifier for a single persisted event."
);
define_id!(
    CorrelationId,
    "Groups all events and commands that originate from the same root operation."
);

impl CorrelationId {
    /// Derive a deterministic `CorrelationId` from an EDIFACT interchange
    /// reference string using UUID v5 (SHA-1 hash in a fixed namespace).
    ///
    /// EDIFACT interchanges carry a reference number in `UNB+…+…+<ref>`.
    /// Messages retransmitted with the same reference must produce the same
    /// `CorrelationId` so duplicate EDIFACT messages are correlated correctly
    /// in traces and never generate spurious new process roots.
    ///
    /// The namespace UUID is fixed and private to this crate:
    /// `a3c7e1f0-5b2d-4e80-9f6a-1b3c5d7e9a0b` (registered once).
    ///
    /// # Example
    ///
    /// ```rust
    /// use mako_engine::ids::CorrelationId;
    ///
    /// let id = CorrelationId::from_interchange_ref("A000123");
    /// // Same reference → same CorrelationId (idempotent dispatch).
    /// assert_eq!(id, CorrelationId::from_interchange_ref("A000123"));
    /// // Different reference → different CorrelationId.
    /// assert_ne!(id, CorrelationId::from_interchange_ref("A000124"));
    /// ```
    #[must_use]
    pub fn from_interchange_ref(interchange_ref: &str) -> Self {
        const INTERCHANGE_NS: Uuid = Uuid::from_u128(0xa3c7_e1f0_5b2d_4e80_9f6a_1b3c_5d7e_9a0b);
        Self(Uuid::new_v5(&INTERCHANGE_NS, interchange_ref.as_bytes()))
    }
}

define_id!(
    CausationId,
    "Points to the event or command that directly caused this event."
);
define_id!(
    ConversationId,
    "Links events that belong to the same business conversation \
     (e.g. a UTILMD exchange and its APERAK acknowledgement)."
);
define_id!(
    ProcessId,
    "Stable identifier for a single MaKo process instance."
);
define_id!(
    TenantId,
    "Scopes all streams and events to a single market participant or deployment tenant."
);

impl TenantId {
    /// Derive a deterministic `TenantId` from a GLN or other opaque operator
    /// identifier string using UUID v5 (SHA-1 hash in a fixed namespace).
    ///
    /// This allows the production binary (`makod`) to accept a GLN from the
    /// Derive a deterministic `TenantId` from a market-participant identifier
    /// (GLN, BDEW code, EIC, or any opaque operator string) using UUID v5
    /// (SHA-1 hash in a fixed namespace).
    ///
    /// This allows the production binary (`makod`) to accept a BDEW code or
    /// GLN from the CLI and produce a stable `TenantId` that is consistent
    /// across process restarts, without requiring that the identifier already
    /// be a UUID.
    ///
    /// The accepted identifier formats are:
    /// - **BDEW code** (13-digit, agency `"293"`) — most common in German MaKo
    /// - **GLN** (13-digit GS1, agency `"9"`) — global GS1 scheme, rare in MaKo
    /// - **EIC** (16-char ENTSO-E, agency `"305"`) — used by TSOs / Regelzonen
    /// - Any other opaque string used as `--tenant-id`
    ///
    /// The namespace UUID is fixed and private to this crate:
    /// `7e4a6b1c-2d3e-5f60-8a9b-0c1d2e3f4a5b` (arbitrary, registered once).
    ///
    /// # Example
    ///
    /// ```rust
    /// use mako_engine::ids::TenantId;
    ///
    /// // BDEW-issued market participant code (agency 293)
    /// let id = TenantId::from_party_id("9900123456789");
    /// assert_eq!(id, TenantId::from_party_id("9900123456789"));
    /// assert_ne!(id, TenantId::from_party_id("9900357000004"));
    ///
    /// // EIC code (ENTSO-E, agency 305) — e.g. for a TSO
    /// let tso = TenantId::from_party_id("10XDE-EON-NETZ--I");
    /// assert_ne!(id, tso);
    /// ```
    #[must_use]
    pub fn from_party_id(party_id: &str) -> Self {
        // Fixed v5 namespace for MaKo tenant party identifiers.
        const TENANT_NS: Uuid = Uuid::from_u128(0x7e4a_6b1c_2d3e_5f60_8a9b_0c1d_2e3f_4a5b);
        Self(Uuid::new_v5(&TENANT_NS, party_id.as_bytes()))
    }
}

define_id!(
    OutboxMessageId,
    "Unique identifier for a single outbox message entry."
);

define_id!(
    DeadlineId,
    "Unique identifier for a registered process deadline."
);

// ── Causation conversions ─────────────────────────────────────────────────────

// `CausationId` tracks what *caused* an event. The cause is always an
// `EventId` (a prior event) or a `CorrelationId` (a root command correlation).
// These `From` impls enable ergonomic construction without a round-trip through
// `as_uuid()`:
//
//   ctx.with_causation(prior_event_id.into())
//   ctx.with_causation(correlation_id.into())

impl From<EventId> for CausationId {
    /// Treat a prior event as the direct cause of the next event.
    fn from(id: EventId) -> Self {
        Self(id.0)
    }
}

impl From<CorrelationId> for CausationId {
    /// Treat a correlation root as the direct cause (useful for first events).
    fn from(id: CorrelationId) -> Self {
        Self(id.0)
    }
}

// ── StreamId ──────────────────────────────────────────────────────────────────

/// An append-only event stream identifier.
///
/// Streams are named with a category prefix so routing and partitioning are
/// explicit (e.g. `process/{tenant_id}/{process_id}`, `partner/{partner_id}`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct StreamId(Box<str>);

impl StreamId {
    /// Construct a stream identifier from any string-like value.
    ///
    /// # Panics
    ///
    /// Panics if `id` is empty or contains a NUL byte (`\0`).
    /// Use this constructor only for **compile-time literals** where the value
    /// is statically known to be valid. For runtime/externally-supplied strings
    /// use [`StreamId::try_new`] or the typed constructors
    /// ([`StreamId::for_process`], [`StreamId::for_partner`]).
    #[must_use]
    pub fn new(id: impl Into<Box<str>>) -> Self {
        let id: Box<str> = id.into();
        assert!(!id.is_empty(), "StreamId must not be empty");
        assert!(
            !id.contains('\0'),
            "StreamId must not contain NUL bytes, got: {id:?}"
        );
        Self(id)
    }

    /// Fallible constructor — returns `Err` instead of panicking.
    ///
    /// Prefer this over [`StreamId::new`] whenever the input string originates
    /// from user input, network data, or storage. The typed constructors
    /// ([`StreamId::for_process`], [`StreamId::for_partner`]) call this
    /// internally.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::EngineError::InvalidStreamId`] if `id` is empty or contains
    /// a NUL byte.
    pub fn try_new(id: impl Into<Box<str>>) -> Result<Self, crate::error::EngineError> {
        let id: Box<str> = id.into();
        if id.is_empty() {
            return Err(crate::error::EngineError::InvalidStreamId {
                input: id,
                reason: "stream ID must not be empty",
            });
        }
        if id.contains('\0') {
            // Truncate the displayed input to avoid log injection via embedded
            // NUL bytes or very long attacker-controlled strings.
            let truncated: Box<str> = id.chars().take(200).collect::<String>().into();
            return Err(crate::error::EngineError::InvalidStreamId {
                input: truncated,
                reason: "stream ID must not contain NUL bytes",
            });
        }
        Ok(Self(id))
    }

    /// Canonical stream for a process instance: `process/{tenant_id}/{process_id}`.
    ///
    /// The tenant discriminator prevents cross-tenant event leakage when
    /// `list_streams` is called with a tenant-scoped prefix
    /// (`process/{tenant_id}/`).
    #[must_use]
    pub fn for_process(tenant_id: TenantId, process_id: &ProcessId) -> Self {
        Self::new(format!("process/{tenant_id}/{process_id}"))
    }

    /// Canonical stream for a market partner: `partner/{partner_id}`.
    ///
    /// # Errors
    ///
    /// Returns an error if `partner_id` contains `/` or a NUL byte, which
    /// would escape the `partner/` prefix used for range scans.
    pub fn for_partner(partner_id: &str) -> Result<Self, crate::error::EngineError> {
        if partner_id.contains('\0') || partner_id.contains('/') {
            return Err(crate::error::EngineError::InvalidStreamId {
                input: partner_id.chars().take(200).collect::<String>().into(),
                reason: "partner_id must not contain '/' or NUL bytes",
            });
        }
        Ok(Self::new(format!("partner/{partner_id}")))
    }

    /// The raw stream identifier string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for StreamId {
    type Error = crate::error::EngineError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::try_new(s)
    }
}

impl TryFrom<String> for StreamId {
    type Error = crate::error::EngineError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_new(s)
    }
}

impl TryFrom<Box<str>> for StreamId {
    type Error = crate::error::EngineError;
    fn try_from(s: Box<str>) -> Result<Self, Self::Error> {
        Self::try_new(s)
    }
}

// ── ProcessIdentity ───────────────────────────────────────────────────────────

/// A serializable value type that bundles all four process identifiers.
///
/// Use `ProcessIdentity` to persist process routing information without
/// keeping a live [`Process`] handle. When a new inbound EDIFACT message
/// arrives and must be routed to a running process, look up the identity in
/// your routing table and call [`Process::from_identity`] to attach.
///
/// ## Example
///
/// ```rust,ignore
/// // Persist after process creation:
/// let identity = process.identity();
/// routing_table.insert(utilmd_conversation_id, identity.clone());
///
/// // Restore on a subsequent message:
/// let identity = routing_table.get(&aperak_conversation_id)?;
/// let process = Process::<MyWorkflow, _>::from_identity(store, identity);
/// process.execute(HandleAperak { .. }).await?;
/// ```
///
/// [`Process`]: crate::process::Process
/// [`Process::from_identity`]: crate::process::Process::from_identity
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessIdentity {
    /// The event stream identifier for this process.
    stream_id: StreamId,
    /// The stable process identifier.
    pub process_id: ProcessId,
    /// The tenant that owns this process.
    pub tenant_id: TenantId,
    /// The workflow version under which this process was created.
    pub workflow_id: crate::version::WorkflowId,
}

impl ProcessIdentity {
    /// Construct a `ProcessIdentity`, deriving `stream_id` automatically from
    /// `tenant_id` and `process_id`.
    ///
    /// `stream_id` is always `StreamId::for_process(tenant_id, &process_id)` —
    /// callers must not supply it independently to avoid accidental mismatches.
    #[must_use]
    pub fn new(
        process_id: ProcessId,
        tenant_id: TenantId,
        workflow_id: crate::version::WorkflowId,
    ) -> Self {
        Self {
            stream_id: StreamId::for_process(tenant_id, &process_id),
            process_id,
            tenant_id,
            workflow_id,
        }
    }

    /// The event stream identifier for this process.
    #[must_use]
    pub fn stream_id(&self) -> &StreamId {
        &self.stream_id
    }
}

// ── Pid ───────────────────────────────────────────────────────────────────────

/// A BDEW Prüfidentifikator — the 5-digit numeric code that identifies a MaKo
/// process family and is used to route inbound EDIFACT messages.
///
/// Valid range: `1..=99999`. Zero is reserved and never a valid PID.
/// PIDs are 5 digits in BDEW documents (e.g. `55001`, `44022`, `17115`).
///
/// # Construction
///
/// - [`Pid::new`] — unchecked compile-time const (panics on out-of-range);
///   use for known-valid literals only.
/// - [`Pid::from_u32`] — checked runtime parse; returns `None` on invalid range.
/// - [`Pid::parse_str`] — parse from a decimal string (leading zeros allowed).
///
/// # Display
///
/// `Pid` formats as a zero-padded 5-digit string (`55001`, not `55_001`).
///
/// # Example
///
/// ```rust
/// use mako_engine::ids::Pid;
///
/// // Compile-time known literal:
/// const LIEFERANTENWECHSEL: Pid = Pid::new(55001);
///
/// // Runtime-checked parse:
/// let pid = Pid::from_u32(44022).expect("valid PID");
/// assert_eq!(pid.as_u32(), 44022);
/// assert_eq!(pid.to_string(), "44022");
///
/// // Out-of-range:
/// assert!(Pid::from_u32(0).is_none());
/// assert!(Pid::from_u32(100_000).is_none());
/// ```
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Pid(u32);

impl Pid {
    /// Create a `Pid` from a known-valid compile-time literal.
    ///
    /// This is a `const fn` so it can be used in `const` context.
    ///
    /// # Panics
    ///
    /// Panics at compile time (or runtime in debug builds) if `value == 0`
    /// or `value > 99_999`.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        assert!(
            value > 0 && value <= 99_999,
            "Pid must be in range 1..=99999"
        );
        Self(value)
    }

    /// Parse a `Pid` from a runtime `u32`, returning `None` on invalid range.
    #[must_use]
    pub fn from_u32(value: u32) -> Option<Self> {
        if value > 0 && value <= 99_999 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Parse a `Pid` from a decimal string, returning `None` if the string is
    /// not a valid decimal integer in `1..=99999`.
    ///
    /// Leading zeros are allowed (e.g. `"05001"` → `Pid(5001)`).
    #[must_use]
    pub fn parse_str(s: &str) -> Option<Self> {
        s.trim().parse::<u32>().ok().and_then(Self::from_u32)
    }

    /// Return the raw `u32` value.
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:05}", self.0)
    }
}

impl From<Pid> for u32 {
    fn from(p: Pid) -> u32 {
        p.0
    }
}
