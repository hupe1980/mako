use edifact_rs::OwnedSegment;

use crate::{EdiEnergyMessage, EdiEnergyReport, Error, MessageType, Pruefidentifikator, Release};

#[cfg(feature = "aperak")]
use crate::messages::aperak::AperakMessage;
#[cfg(feature = "comdis")]
use crate::messages::comdis::ComdisMessage;
#[cfg(feature = "contrl")]
use crate::messages::contrl::ContrlMessage;
#[cfg(feature = "iftsta")]
use crate::messages::iftsta::IftstaMessage;
#[cfg(feature = "insrpt")]
use crate::messages::insrpt::InsrptMessage;
#[cfg(feature = "invoic")]
use crate::messages::invoic::InvoicMessage;
#[cfg(feature = "mscons")]
use crate::messages::mscons::MsconsMessage;
#[cfg(feature = "ordchg")]
use crate::messages::ordchg::OrdchgMessage;
#[cfg(feature = "orders")]
use crate::messages::orders::OrdersMessage;
#[cfg(feature = "ordrsp")]
use crate::messages::ordrsp::OrdrespMessage;
#[cfg(feature = "partin")]
use crate::messages::partin::PartinMessage;
#[cfg(feature = "pricat")]
use crate::messages::pricat::PricatMessage;
#[cfg(feature = "quotes")]
use crate::messages::quotes::QuotesMessage;
#[cfg(feature = "remadv")]
use crate::messages::remadv::RemadvMessage;
#[cfg(feature = "reqote")]
use crate::messages::reqote::ReqoteMessage;
#[cfg(feature = "utilmd")]
use crate::messages::utilmd::UtilmdMessage;
#[cfg(feature = "utilts")]
use crate::messages::utilts::UtiltsMessage;

/// A parsed EDI@Energy message, dispatched to its concrete type.
///
/// Match on the variants to access type-specific functionality — the enum is
/// public, so a pattern is the downcast and there are no per-type `as_*`
/// accessors — or use the [`EdiEnergyMessage`] trait methods (via
/// `msg.validate()`, `msg.serialize()`, etc.) for common operations across all
/// message types.
///
/// Each variant is only present when the corresponding Cargo feature is enabled.
/// Compile with `--all-features` to enable exhaustive matching — this is the
/// recommended pattern for dispatch tables and process-layer routers.
///
/// The [`Unknown`][AnyMessage::Unknown] variant captures messages whose type is
/// not compiled in or not recognised, preserving the raw segments for routing
/// and forwarding use-cases (e.g. BDEW AS4 adapters).
#[derive(Debug)]
pub enum AnyMessage {
    /// UTILMD — Utilities Master Data message.
    #[cfg(feature = "utilmd")]
    Utilmd(UtilmdMessage),

    /// MSCONS — Metered Services Consumption Report.
    #[cfg(feature = "mscons")]
    Mscons(MsconsMessage),

    /// APERAK — Application Error and Acknowledgement.
    #[cfg(feature = "aperak")]
    Aperak(AperakMessage),

    /// CONTRL — Interchange Control Structure (syntax acknowledgement).
    #[cfg(feature = "contrl")]
    Contrl(ContrlMessage),

    /// INVOIC — Invoice.
    #[cfg(feature = "invoic")]
    Invoic(InvoicMessage),

    /// REMADV — Remittance Advice.
    #[cfg(feature = "remadv")]
    Remadv(RemadvMessage),

    /// ORDERS — Purchase Order.
    #[cfg(feature = "orders")]
    Orders(OrdersMessage),

    /// IFTSTA — International Multimodal Status Report.
    #[cfg(feature = "iftsta")]
    Iftsta(IftstaMessage),

    /// INSRPT — Inspection Report.
    #[cfg(feature = "insrpt")]
    Insrpt(InsrptMessage),

    /// REQOTE — Request for Quotation.
    #[cfg(feature = "reqote")]
    Reqote(ReqoteMessage),

    /// PARTIN — Party Information.
    #[cfg(feature = "partin")]
    Partin(PartinMessage),

    /// ORDCHG — Purchase Order Change.
    #[cfg(feature = "ordchg")]
    Ordchg(OrdchgMessage),

    /// ORDRSP — Purchase Order Response.
    #[cfg(feature = "ordrsp")]
    Ordrsp(OrdrespMessage),

    /// QUOTES — Quotation.
    #[cfg(feature = "quotes")]
    Quotes(QuotesMessage),

    /// COMDIS — Commercial Dispute (Handelsunstimmigkeit).
    #[cfg(feature = "comdis")]
    Comdis(ComdisMessage),

    /// PRICAT — Price/Sales Catalogue (Preisliste).
    #[cfg(feature = "pricat")]
    Pricat(PricatMessage),

    /// UTILTS — Übertragung technischer Stammdaten (Technical Master Data).
    #[cfg(feature = "utilts")]
    Utilts(UtiltsMessage),

    /// A message whose type is not recognised or not compiled into this build.
    ///
    /// Raw segments are preserved so callers can inspect, log, or forward the
    /// message without loss of data. This variant is returned instead of
    /// `Err(Error::UnknownMessageType)` or `Err(Error::FeatureNotEnabled)` so
    /// that gateway / routing applications can handle interchanges with mixed
    /// message types without failing the entire parse.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use edi_energy::{Platform, AnyMessage};
    ///
    /// let input: &[u8] = &[];
    /// let msg = Platform::with_all_profiles().parse(input)?;
    /// if let AnyMessage::Unknown { message_type_code, release, .. } = &msg {
    ///     eprintln!("Unhandled message type: {} (release {})", message_type_code, release);
    /// }
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    Unknown {
        /// The EDIFACT type code from UNH DE 0065, e.g. `"TRADEX"`.
        message_type_code: Box<str>,
        /// The association-assigned release code from UNH DE 0057.
        release: Release,
        /// The raw message reference from UNH DE 0062.
        message_ref: Box<str>,
        /// All parsed segments (UNH … UNT inclusive).
        segments: Vec<OwnedSegment>,
    },
}

impl AnyMessage {
    /// The message-level sender MP-ID — `NAD+MS` DE 3039.
    ///
    /// `None` for CONTRL, which is a syntax acknowledgement carried at the
    /// interchange level and has no NAD, and for `Unknown`.
    ///
    /// Per BDEW Allgemeine Festlegungen §2.13 this **must** equal the
    /// interchange sender in `UNB` DE 0004; [`crate::parse`] rejects a message
    /// where they disagree.
    #[must_use]
    pub fn nad_sender(&self) -> Option<&str> {
        match self {
            #[cfg(feature = "utilmd")]
            Self::Utilmd(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "mscons")]
            Self::Mscons(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "aperak")]
            Self::Aperak(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "invoic")]
            Self::Invoic(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "remadv")]
            Self::Remadv(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "orders")]
            Self::Orders(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "iftsta")]
            Self::Iftsta(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "insrpt")]
            Self::Insrpt(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "reqote")]
            Self::Reqote(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "partin")]
            Self::Partin(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "ordchg")]
            Self::Ordchg(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "ordrsp")]
            Self::Ordrsp(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "quotes")]
            Self::Quotes(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "comdis")]
            Self::Comdis(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "pricat")]
            Self::Pricat(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "utilts")]
            Self::Utilts(m) => m.sender().and_then(|n| n.party_id.as_deref()),
            _ => None,
        }
    }

    /// The message-level receiver MP-ID — `NAD+MR` DE 3039.
    ///
    /// Counterpart to [`Self::nad_sender`]; must equal `UNB` DE 0010.
    #[must_use]
    pub fn nad_receiver(&self) -> Option<&str> {
        match self {
            #[cfg(feature = "utilmd")]
            Self::Utilmd(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "mscons")]
            Self::Mscons(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "aperak")]
            Self::Aperak(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "invoic")]
            Self::Invoic(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "remadv")]
            Self::Remadv(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "orders")]
            Self::Orders(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "iftsta")]
            Self::Iftsta(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "insrpt")]
            Self::Insrpt(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "reqote")]
            Self::Reqote(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "partin")]
            Self::Partin(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "ordchg")]
            Self::Ordchg(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "ordrsp")]
            Self::Ordrsp(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "quotes")]
            Self::Quotes(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "comdis")]
            Self::Comdis(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "pricat")]
            Self::Pricat(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            #[cfg(feature = "utilts")]
            Self::Utilts(m) => m.receiver().and_then(|n| n.party_id.as_deref()),
            _ => None,
        }
    }

    /// Returns `true` if this message has a recognised, compiled-in type.
    ///
    /// Returns `false` for the [`Unknown`][AnyMessage::Unknown] variant.
    #[must_use]
    pub fn is_known(&self) -> bool {
        !matches!(self, AnyMessage::Unknown { .. })
    }

    /// Returns the message type discriminant, or `None` for the
    /// [`Unknown`][AnyMessage::Unknown] variant.
    ///
    /// Prefer this over the [`EdiEnergyMessage::try_message_type`] trait method when
    /// working with an `AnyMessage` value that may be `Unknown`.
    #[must_use]
    pub fn try_message_type(&self) -> Option<MessageType> {
        // Delegates to the EdiEnergyMessage trait impl, which handles all
        // feature-gated variants via the delegate_any! macro.
        <Self as EdiEnergyMessage>::try_message_type(self)
    }

    /// Returns the inner [`MessageCore`][crate::messages::core::MessageCore], or `None`
    /// for the [`Unknown`][AnyMessage::Unknown] variant.
    ///
    /// Used internally by [`Platform`][crate::Platform] for registry-aware validation.
    #[cfg(any_message)]
    #[must_use]
    pub(crate) fn message_core(&self) -> Option<&crate::messages::core::MessageCore> {
        match self {
            #[cfg(feature = "utilmd")]
            AnyMessage::Utilmd(m) => Some(&m.core),
            #[cfg(feature = "mscons")]
            AnyMessage::Mscons(m) => Some(&m.core),
            #[cfg(feature = "aperak")]
            AnyMessage::Aperak(m) => Some(&m.core),
            #[cfg(feature = "contrl")]
            AnyMessage::Contrl(m) => Some(&m.core),
            #[cfg(feature = "invoic")]
            AnyMessage::Invoic(m) => Some(&m.core),
            #[cfg(feature = "remadv")]
            AnyMessage::Remadv(m) => Some(&m.core),
            #[cfg(feature = "orders")]
            AnyMessage::Orders(m) => Some(&m.core),
            #[cfg(feature = "iftsta")]
            AnyMessage::Iftsta(m) => Some(&m.core),
            #[cfg(feature = "insrpt")]
            AnyMessage::Insrpt(m) => Some(&m.core),
            #[cfg(feature = "reqote")]
            AnyMessage::Reqote(m) => Some(&m.core),
            #[cfg(feature = "partin")]
            AnyMessage::Partin(m) => Some(&m.core),
            #[cfg(feature = "ordchg")]
            AnyMessage::Ordchg(m) => Some(&m.core),
            #[cfg(feature = "ordrsp")]
            AnyMessage::Ordrsp(m) => Some(&m.core),
            #[cfg(feature = "quotes")]
            AnyMessage::Quotes(m) => Some(&m.core),
            #[cfg(feature = "comdis")]
            AnyMessage::Comdis(m) => Some(&m.core),
            #[cfg(feature = "pricat")]
            AnyMessage::Pricat(m) => Some(&m.core),
            #[cfg(feature = "utilts")]
            AnyMessage::Utilts(m) => Some(&m.core),
            AnyMessage::Unknown { .. } => None,
        }
    }

    /// Mutable access to the shared core, for settings applied after dispatch.
    #[cfg(any_message)]
    pub(crate) fn message_core_mut(&mut self) -> Option<&mut crate::messages::core::MessageCore> {
        match self {
            #[cfg(feature = "utilmd")]
            AnyMessage::Utilmd(m) => Some(&mut m.core),
            #[cfg(feature = "mscons")]
            AnyMessage::Mscons(m) => Some(&mut m.core),
            #[cfg(feature = "aperak")]
            AnyMessage::Aperak(m) => Some(&mut m.core),
            #[cfg(feature = "contrl")]
            AnyMessage::Contrl(m) => Some(&mut m.core),
            #[cfg(feature = "invoic")]
            AnyMessage::Invoic(m) => Some(&mut m.core),
            #[cfg(feature = "remadv")]
            AnyMessage::Remadv(m) => Some(&mut m.core),
            #[cfg(feature = "orders")]
            AnyMessage::Orders(m) => Some(&mut m.core),
            #[cfg(feature = "iftsta")]
            AnyMessage::Iftsta(m) => Some(&mut m.core),
            #[cfg(feature = "insrpt")]
            AnyMessage::Insrpt(m) => Some(&mut m.core),
            #[cfg(feature = "reqote")]
            AnyMessage::Reqote(m) => Some(&mut m.core),
            #[cfg(feature = "partin")]
            AnyMessage::Partin(m) => Some(&mut m.core),
            #[cfg(feature = "ordchg")]
            AnyMessage::Ordchg(m) => Some(&mut m.core),
            #[cfg(feature = "ordrsp")]
            AnyMessage::Ordrsp(m) => Some(&mut m.core),
            #[cfg(feature = "quotes")]
            AnyMessage::Quotes(m) => Some(&mut m.core),
            #[cfg(feature = "comdis")]
            AnyMessage::Comdis(m) => Some(&mut m.core),
            #[cfg(feature = "pricat")]
            AnyMessage::Pricat(m) => Some(&mut m.core),
            #[cfg(feature = "utilts")]
            AnyMessage::Utilts(m) => Some(&mut m.core),
            AnyMessage::Unknown { .. } => None,
        }
    }

    /// Record the date `validate()` resolves profiles against.
    ///
    /// Applied after dispatch rather than threaded through seventeen
    /// `from_parts` signatures. This is what makes
    /// [`ParseConfig::reference_date`](crate::ParseConfig::reference_date)
    /// reach `validate()`.
    pub(crate) fn set_reference_date(&mut self, date: time::Date) {
        #[cfg(any_message)]
        if let Some(core) = self.message_core_mut() {
            core.reference_date = Some(date);
        }
        #[cfg(not(any_message))]
        let _ = date;
    }
}

// ── EdiEnergyMessage delegation ──────────────────────────────────────────────

macro_rules! delegate_any {
    ($self:expr, $inner:ident => $body:expr) => {
        match $self {
            #[cfg(feature = "utilmd")]
            AnyMessage::Utilmd($inner) => $body,
            #[cfg(feature = "mscons")]
            AnyMessage::Mscons($inner) => $body,
            #[cfg(feature = "aperak")]
            AnyMessage::Aperak($inner) => $body,
            #[cfg(feature = "contrl")]
            AnyMessage::Contrl($inner) => $body,
            #[cfg(feature = "invoic")]
            AnyMessage::Invoic($inner) => $body,
            #[cfg(feature = "remadv")]
            AnyMessage::Remadv($inner) => $body,
            #[cfg(feature = "orders")]
            AnyMessage::Orders($inner) => $body,
            #[cfg(feature = "iftsta")]
            AnyMessage::Iftsta($inner) => $body,
            #[cfg(feature = "insrpt")]
            AnyMessage::Insrpt($inner) => $body,
            #[cfg(feature = "reqote")]
            AnyMessage::Reqote($inner) => $body,
            #[cfg(feature = "partin")]
            AnyMessage::Partin($inner) => $body,
            #[cfg(feature = "ordchg")]
            AnyMessage::Ordchg($inner) => $body,
            #[cfg(feature = "ordrsp")]
            AnyMessage::Ordrsp($inner) => $body,
            #[cfg(feature = "quotes")]
            AnyMessage::Quotes($inner) => $body,
            #[cfg(feature = "comdis")]
            AnyMessage::Comdis($inner) => $body,
            #[cfg(feature = "pricat")]
            AnyMessage::Pricat($inner) => $body,
            #[cfg(feature = "utilts")]
            AnyMessage::Utilts($inner) => $body,
            AnyMessage::Unknown {
                message_type_code,
                release,
                message_ref: _,
                segments,
            } => {
                let _ = (message_type_code, release, segments);
                unreachable!("Unknown variant must be handled before delegation")
            }
        }
    };
}

// With `--no-default-features` all typed variants are removed and only
// `Unknown` remains, making the `_ =>` delegation arms in each match
// structurally unreachable.  The logic is correct; suppress the lint.

/// The report for a message type this build cannot validate.
///
/// An **error**, not a warning: no rule ran, so nothing has been shown to
/// conform — reporting it valid makes a vacuous pass indistinguishable from a
/// real one.
fn unknown_type_report(message_type_code: &str) -> EdiEnergyReport {
    use edifact_rs::{ValidationIssue, ValidationReport, ValidationSeverity};
    let mut inner = ValidationReport::default();
    inner.add_error(
        ValidationIssue::new(
            ValidationSeverity::Error,
            format!("message type {message_type_code:?} is not validated in this build"),
        )
        .with_rule_id("UNKNOWN-MSG-TYPE"),
    );
    EdiEnergyReport::new(inner)
}

#[allow(
    unreachable_patterns,
    reason = "the arm is reachable in some feature combinations"
)]
impl EdiEnergyMessage for AnyMessage {
    fn try_message_type(&self) -> Option<MessageType> {
        match self {
            AnyMessage::Unknown { .. } => None,
            _ => delegate_any!(self, m => m.try_message_type()),
        }
    }

    fn detect_release(&self) -> Result<&Release, Error> {
        match self {
            AnyMessage::Unknown { release, .. } => Ok(release),
            _ => delegate_any!(self, m => m.detect_release()),
        }
    }

    fn message_ref(&self) -> &str {
        match self {
            AnyMessage::Unknown { message_ref, .. } => message_ref,
            _ => delegate_any!(self, m => m.message_ref()),
        }
    }

    fn detect_pruefidentifikator(&self) -> Result<Pruefidentifikator, Error> {
        match self {
            AnyMessage::Unknown { .. } => Err(Error::MissingPruefidentifikator),
            _ => delegate_any!(self, m => m.detect_pruefidentifikator()),
        }
    }

    fn validate(&self) -> Result<EdiEnergyReport, Error> {
        match self {
            AnyMessage::Unknown {
                message_type_code, ..
            } => Ok(unknown_type_report(message_type_code)),
            _ => delegate_any!(self, m => m.validate()),
        }
    }

    #[allow(
        unused_variables,
        reason = "read only in some feature or target combinations"
    )]
    fn validate_against(&self, release: &Release) -> Result<EdiEnergyReport, Error> {
        match self {
            AnyMessage::Unknown {
                message_type_code, ..
            } => Ok(unknown_type_report(message_type_code)),
            _ => delegate_any!(self, m => m.validate_against(release)),
        }
    }

    fn serialize(&self) -> Result<Vec<u8>, Error> {
        match self {
            AnyMessage::Unknown { segments, .. } => {
                edifact_rs::segments_to_bytes(segments).map_err(Error::Parse)
            }
            _ => delegate_any!(self, m => m.serialize()),
        }
    }

    fn segments(&self) -> &[OwnedSegment] {
        match self {
            AnyMessage::Unknown { segments, .. } => segments,
            _ => delegate_any!(self, m => m.segments()),
        }
    }

    #[allow(
        unused_variables,
        reason = "read only in some feature or target combinations"
    )]
    fn validate_on_date(&self, reference_date: time::Date) -> Result<EdiEnergyReport, Error> {
        match self {
            AnyMessage::Unknown {
                message_type_code, ..
            } => Ok(unknown_type_report(message_type_code)),
            _ => delegate_any!(self, m => m.validate_on_date(reference_date)),
        }
    }
}
