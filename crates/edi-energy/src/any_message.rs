use edifact_rs::OwnedSegment;

use crate::{
    CustomRulePack, EdiEnergyMessage, EdiEnergyReport, Error, MessageType, Pruefidentifikator,
    Release,
};

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
/// Match on the variants to access type-specific functionality, or use the
/// [`EdiEnergyMessage`] trait methods (via `msg.validate()`, `msg.serialize()`, etc.)
/// for common operations across all message types.
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

    // ── Type-specific downcast helpers ────────────────────────────────────────

    /// Returns a reference to the inner [`UtilmdMessage`], or `None`.
    #[cfg(feature = "utilmd")]
    #[must_use]
    pub fn as_utilmd(&self) -> Option<&crate::messages::utilmd::UtilmdMessage> {
        if let AnyMessage::Utilmd(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`MsconsMessage`], or `None`.
    #[cfg(feature = "mscons")]
    #[must_use]
    pub fn as_mscons(&self) -> Option<&crate::messages::mscons::MsconsMessage> {
        if let AnyMessage::Mscons(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`AperakMessage`], or `None`.
    #[cfg(feature = "aperak")]
    #[must_use]
    pub fn as_aperak(&self) -> Option<&crate::messages::aperak::AperakMessage> {
        if let AnyMessage::Aperak(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`ContrlMessage`], or `None`.
    #[cfg(feature = "contrl")]
    #[must_use]
    pub fn as_contrl(&self) -> Option<&crate::messages::contrl::ContrlMessage> {
        if let AnyMessage::Contrl(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`InvoicMessage`], or `None`.
    #[cfg(feature = "invoic")]
    #[must_use]
    pub fn as_invoic(&self) -> Option<&crate::messages::invoic::InvoicMessage> {
        if let AnyMessage::Invoic(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`RemadvMessage`], or `None`.
    #[cfg(feature = "remadv")]
    #[must_use]
    pub fn as_remadv(&self) -> Option<&crate::messages::remadv::RemadvMessage> {
        if let AnyMessage::Remadv(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`OrdersMessage`], or `None`.
    #[cfg(feature = "orders")]
    #[must_use]
    pub fn as_orders(&self) -> Option<&crate::messages::orders::OrdersMessage> {
        if let AnyMessage::Orders(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`IftstaMessage`], or `None`.
    #[cfg(feature = "iftsta")]
    #[must_use]
    pub fn as_iftsta(&self) -> Option<&crate::messages::iftsta::IftstaMessage> {
        if let AnyMessage::Iftsta(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`InsrptMessage`], or `None`.
    #[cfg(feature = "insrpt")]
    #[must_use]
    pub fn as_insrpt(&self) -> Option<&crate::messages::insrpt::InsrptMessage> {
        if let AnyMessage::Insrpt(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`ReqoteMessage`], or `None`.
    #[cfg(feature = "reqote")]
    #[must_use]
    pub fn as_reqote(&self) -> Option<&crate::messages::reqote::ReqoteMessage> {
        if let AnyMessage::Reqote(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`PartinMessage`], or `None`.
    #[cfg(feature = "partin")]
    #[must_use]
    pub fn as_partin(&self) -> Option<&crate::messages::partin::PartinMessage> {
        if let AnyMessage::Partin(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`OrdchgMessage`], or `None`.
    #[cfg(feature = "ordchg")]
    #[must_use]
    pub fn as_ordchg(&self) -> Option<&crate::messages::ordchg::OrdchgMessage> {
        if let AnyMessage::Ordchg(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`OrdrespMessage`], or `None`.
    #[cfg(feature = "ordrsp")]
    #[must_use]
    pub fn as_ordrsp(&self) -> Option<&crate::messages::ordrsp::OrdrespMessage> {
        if let AnyMessage::Ordrsp(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`QuotesMessage`], or `None`.
    #[cfg(feature = "quotes")]
    #[must_use]
    pub fn as_quotes(&self) -> Option<&crate::messages::quotes::QuotesMessage> {
        if let AnyMessage::Quotes(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`ComdisMessage`], or `None`.
    #[cfg(feature = "comdis")]
    #[must_use]
    pub fn as_comdis(&self) -> Option<&crate::messages::comdis::ComdisMessage> {
        if let AnyMessage::Comdis(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`PricatMessage`], or `None`.
    #[cfg(feature = "pricat")]
    #[must_use]
    pub fn as_pricat(&self) -> Option<&crate::messages::pricat::PricatMessage> {
        if let AnyMessage::Pricat(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns a reference to the inner [`UtiltsMessage`], or `None`.
    #[cfg(feature = "utilts")]
    #[must_use]
    pub fn as_utilts(&self) -> Option<&crate::messages::utilts::UtiltsMessage> {
        if let AnyMessage::Utilts(m) = self {
            Some(m)
        } else {
            None
        }
    }

    /// Returns the inner [`MessageCore`][crate::messages::core::MessageCore], or `None`
    /// for the [`Unknown`][AnyMessage::Unknown] variant.
    ///
    /// Used internally by [`Platform`][crate::Platform] for registry-aware validation.
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
#[allow(unreachable_patterns)]
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
            } => {
                use edifact_rs::{ValidationIssue, ValidationReport, ValidationSeverity};
                let mut inner = ValidationReport::default();
                inner.add_error(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!(
                            "message type {message_type_code:?} is not validated in this build"
                        ),
                    )
                    .with_rule_id("UNKNOWN-MSG-TYPE"),
                );
                Ok(EdiEnergyReport::new(inner))
            }
            _ => delegate_any!(self, m => m.validate()),
        }
    }

    #[allow(unused_variables)]
    fn validate_against(&self, release: &Release) -> Result<EdiEnergyReport, Error> {
        match self {
            AnyMessage::Unknown {
                message_type_code, ..
            } => {
                use edifact_rs::{ValidationIssue, ValidationReport, ValidationSeverity};
                let mut inner = ValidationReport::default();
                inner.add_error(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!(
                            "message type {message_type_code:?} is not validated in this build"
                        ),
                    )
                    .with_rule_id("UNKNOWN-MSG-TYPE"),
                );
                Ok(EdiEnergyReport::new(inner))
            }
            _ => delegate_any!(self, m => m.validate_against(release)),
        }
    }

    fn serialize(&self) -> Result<Vec<u8>, Error> {
        match self {
            AnyMessage::Unknown { segments, .. } => {
                edifact_rs::segments_to_bytes_owned(segments).map_err(Error::Parse)
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

    fn validate_with_pack(&self, extra: CustomRulePack) -> Result<EdiEnergyReport, Error> {
        match self {
            AnyMessage::Unknown { .. } => {
                let _ = extra;
                Err(Error::MissingRelease)
            }
            _ => delegate_any!(self, m => m.validate_with_pack(extra)),
        }
    }

    #[allow(unused_variables)]
    fn validate_on_date(&self, reference_date: time::Date) -> Result<EdiEnergyReport, Error> {
        match self {
            AnyMessage::Unknown {
                message_type_code, ..
            } => {
                use edifact_rs::{ValidationIssue, ValidationReport, ValidationSeverity};
                let mut inner = ValidationReport::default();
                inner.add_error(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        format!(
                            "message type {message_type_code:?} is not validated in this build"
                        ),
                    )
                    .with_rule_id("UNKNOWN-MSG-TYPE"),
                );
                Ok(EdiEnergyReport::new(inner))
            }
            _ => delegate_any!(self, m => m.validate_on_date(reference_date)),
        }
    }
}
