//! `RedispatchRouter` — maps [`RedispatchDocumentKind`]s to workflow names.
//!
//! Redispatch 2.0 uses **CIM/XML documents** for the primary data exchange,
//! not EDIFACT `RFF+Z13` Prüfidentifikatoren. Routing is therefore based on
//! [`RedispatchDocumentKind`] — a domain-owned enum that mirrors the XML root
//! element taxonomy but carries no dependency on the `redispatch-xml` parse crate.
//!
//! # Layer boundary
//!
//! Parsing (`redispatch_xml::parse`) stays at the `makod` transport boundary.
//! The inbound dispatcher converts the parse result to a [`RedispatchDocumentKind`]
//! before calling [`RedispatchRouter::route`]:
//!
//! ```rust,ignore
//! // In makod's AS4 ingest path — transport boundary only:
//! let doc = redispatch_xml::parse(bytes)?;
//! let kind = RedispatchDocumentKind::from(doc.document_type()); // From impl in makod
//! let workflow_name = router.route(kind)?;
//! // resume the workflow process and dispatch the command …
//! ```
//!
//! [`RedispatchModule`]: crate::RedispatchModule

use std::fmt;

use thiserror::Error;

// ── RedispatchDocumentKind ────────────────────────────────────────────────────

/// Domain-owned classification of a Redispatch 2.0 XML document.
///
/// Mirrors the nine XML root-element types defined by the BDEW Redispatch 2.0
/// schema family, but is **independent of `redispatch-xml`**. The conversion
/// from a parsed `redispatch_xml::documents::DocumentType` to this type is done
/// at the `makod` transport boundary, keeping `mako-redispatch` free of any
/// format-layer dependency.
///
/// # Non-exhaustive
///
/// New document types may be added as the BDEW schema evolves. Match with a
/// `_` arm or use [`RedispatchRouter::is_registered`] for membership checks.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RedispatchDocumentKind {
    /// `ActivationDocument` (ACO/ACR/AAR).
    Activation,
    /// `PlannedResourceScheduleDocument`.
    PlannedResourceSchedule,
    /// `AcknowledgementDocument`.
    ///
    /// Routed by correlation key (`ReceivingDocumentIdentification`), not by
    /// document type. This variant exists for completeness and for
    /// [`RedispatchRouter::is_registered`] guards; it must **not** be registered
    /// in the type-based router.
    Acknowledgement,
    /// `Stammdaten`.
    Stammdaten,
    /// `StatusRequest_MarketDocument`.
    StatusRequest,
    /// `Unavailability_MarketDocument`.
    Unavailability,
    /// `Kaskade`.
    Kaskade,
    /// `NetworkConstraintDocument`.
    NetworkConstraint,
    /// `Kostenblatt`.
    Kostenblatt,
}

impl fmt::Display for RedispatchDocumentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Activation => write!(f, "ActivationDocument"),
            Self::PlannedResourceSchedule => write!(f, "PlannedResourceScheduleDocument"),
            Self::Acknowledgement => write!(f, "AcknowledgementDocument"),
            Self::Stammdaten => write!(f, "Stammdaten"),
            Self::StatusRequest => write!(f, "StatusRequest_MarketDocument"),
            Self::Unavailability => write!(f, "Unavailability_MarketDocument"),
            Self::Kaskade => write!(f, "Kaskade"),
            Self::NetworkConstraint => write!(f, "NetworkConstraintDocument"),
            Self::Kostenblatt => write!(f, "Kostenblatt"),
        }
    }
}

// ── Routing error ─────────────────────────────────────────────────────────────

/// Error returned when no workflow is registered for a given document kind.
#[derive(Debug, Error)]
#[error("no Redispatch workflow registered for document kind {doc_kind}")]
pub struct RoutingError {
    /// The document kind that could not be routed.
    pub doc_kind: RedispatchDocumentKind,
}

// ── RedispatchRouter ──────────────────────────────────────────────────────────

/// Routes Redispatch 2.0 [`RedispatchDocumentKind`]s to workflow names.
///
/// Constructed by [`crate::RedispatchModule::build_router`] during `makod` startup.
/// After construction the mapping is **sealed** — no runtime mutation.
///
/// # Registration order
///
/// Each [`RedispatchDocumentKind`] maps to exactly one workflow name. Duplicate
/// registrations overwrite the previous entry (last-write-wins), analogous
/// to `PidRouter`. Use `cargo xtask validate-pruefids` to detect conflicts.
#[derive(Debug, Default, Clone)]
pub struct RedispatchRouter {
    /// Mapping from `RedispatchDocumentKind` discriminant to workflow name.
    ///
    /// Uses a fixed-size array indexed by `RedispatchDocumentKind as usize`.
    entries: [Option<&'static str>; Self::TABLE_SIZE],
}

impl RedispatchRouter {
    /// Number of distinct [`RedispatchDocumentKind`] variants (keep in sync with the enum).
    const TABLE_SIZE: usize = 16;

    /// Create an empty router (no routes registered).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a mapping from `doc_kind` to `workflow_name`.
    ///
    /// If `doc_kind` was already registered the new entry **overwrites** the
    /// previous one. This mirrors the `PidRouter` contract.
    pub fn register(&mut self, doc_kind: RedispatchDocumentKind, workflow_name: &'static str) {
        let idx = doc_kind as usize;
        debug_assert!(
            idx < Self::TABLE_SIZE,
            "RedispatchDocumentKind discriminant {idx} exceeds RedispatchRouter table size; \
             increase TABLE_SIZE"
        );
        if idx < Self::TABLE_SIZE {
            self.entries[idx] = Some(workflow_name);
        }
    }

    /// Look up the workflow name for `doc_kind`.
    ///
    /// Returns `Ok(name)` when a mapping was registered, or a [`RoutingError`]
    /// when the document kind is unknown.
    ///
    /// # Errors
    ///
    /// Returns [`RoutingError`] when no workflow was registered for `doc_kind`.
    pub fn route(&self, doc_kind: RedispatchDocumentKind) -> Result<&'static str, RoutingError> {
        let idx = doc_kind as usize;
        if idx < Self::TABLE_SIZE {
            self.entries[idx].ok_or(RoutingError { doc_kind })
        } else {
            Err(RoutingError { doc_kind })
        }
    }

    /// Return `true` if `doc_kind` has a registered workflow.
    #[must_use]
    pub fn is_registered(&self, doc_kind: RedispatchDocumentKind) -> bool {
        self.route(doc_kind).is_ok()
    }

    /// Iterate over all registered `(RedispatchDocumentKind, workflow_name)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (RedispatchDocumentKind, &'static str)> + '_ {
        ALL_DOC_KINDS
            .iter()
            .filter_map(|&dk| self.entries[dk as usize].map(|name| (dk, name)))
    }
}

/// Canonical ordered list of all [`RedispatchDocumentKind`] variants.
///
/// Used by [`RedispatchRouter::iter`] to iterate registrations in a stable order.
const ALL_DOC_KINDS: &[RedispatchDocumentKind] = &[
    RedispatchDocumentKind::Activation,
    RedispatchDocumentKind::PlannedResourceSchedule,
    RedispatchDocumentKind::Acknowledgement,
    RedispatchDocumentKind::Stammdaten,
    RedispatchDocumentKind::StatusRequest,
    RedispatchDocumentKind::Unavailability,
    RedispatchDocumentKind::Kaskade,
    RedispatchDocumentKind::NetworkConstraint,
    RedispatchDocumentKind::Kostenblatt,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_route_roundtrip() {
        let mut router = RedispatchRouter::new();
        router.register(RedispatchDocumentKind::Activation, "redispatch-aktivierung");
        router.register(RedispatchDocumentKind::Stammdaten, "redispatch-stammdaten");

        assert_eq!(
            router.route(RedispatchDocumentKind::Activation).unwrap(),
            "redispatch-aktivierung"
        );
        assert_eq!(
            router.route(RedispatchDocumentKind::Stammdaten).unwrap(),
            "redispatch-stammdaten"
        );
    }

    #[test]
    fn unregistered_doc_kind_returns_error() {
        let router = RedispatchRouter::new();
        assert!(router.route(RedispatchDocumentKind::Kostenblatt).is_err());
    }

    #[test]
    fn duplicate_registration_overwrites() {
        let mut router = RedispatchRouter::new();
        router.register(RedispatchDocumentKind::Activation, "first");
        router.register(RedispatchDocumentKind::Activation, "second");
        assert_eq!(
            router.route(RedispatchDocumentKind::Activation).unwrap(),
            "second"
        );
    }

    #[test]
    fn is_registered_reflects_state() {
        let mut router = RedispatchRouter::new();
        assert!(!router.is_registered(RedispatchDocumentKind::Activation));
        router.register(RedispatchDocumentKind::Activation, "redispatch-aktivierung");
        assert!(router.is_registered(RedispatchDocumentKind::Activation));
    }

    #[test]
    fn iter_returns_only_registered() {
        let mut router = RedispatchRouter::new();
        router.register(RedispatchDocumentKind::Activation, "redispatch-aktivierung");
        router.register(RedispatchDocumentKind::Stammdaten, "redispatch-stammdaten");

        let pairs: Vec<_> = router.iter().collect();
        assert_eq!(pairs.len(), 2);
        assert!(
            pairs
                .iter()
                .any(|(dk, _)| *dk == RedispatchDocumentKind::Activation)
        );
        assert!(
            pairs
                .iter()
                .any(|(dk, _)| *dk == RedispatchDocumentKind::Stammdaten)
        );
    }
}
