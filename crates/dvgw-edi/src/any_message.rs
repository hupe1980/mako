use crate::message::DvgwMessage;

#[cfg(feature = "alocat")]
use crate::messages::alocat::AlocatMessage;
#[cfg(feature = "delord")]
use crate::messages::delord::DeliveryOrderMessage;
#[cfg(feature = "delres")]
use crate::messages::delres::DeliveryResponseMessage;
#[cfg(feature = "imbnot")]
use crate::messages::imbnot::ImbalanceMessage;
#[cfg(feature = "nomint")]
use crate::messages::nomint::NomintMessage;
#[cfg(feature = "nomres")]
use crate::messages::nomres::NomresMessage;
#[cfg(feature = "schedl")]
use crate::messages::schedl::SchedlMessage;
#[cfg(feature = "tranot")]
use crate::messages::tranot::TransportNotificationMessage;

/// A parsed DVGW message, dispatched to its concrete type.
///
/// Match on the variants to access type-specific functionality, or use the
/// [`DvgwMessage`] trait methods for common operations (sender/receiver EIC,
/// version, message reference).
///
/// Each variant is only present when the corresponding Cargo feature is enabled.
/// Compile with `--all-features` to enable exhaustive matching.
///
/// The [`Unknown`][AnyDvgwMessage::Unknown] variant captures messages whose type
/// is recognised as a DVGW format but whose Cargo feature is not compiled in,
/// or future message types not yet implemented.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AnyDvgwMessage {
    /// ALOCAT — Allokationsnachricht (gas quantity allocation).
    #[cfg(feature = "alocat")]
    Alocat(Box<AlocatMessage>),

    /// NOMINT — Nominierungsintegration (nomination integration).
    #[cfg(feature = "nomint")]
    Nomint(Box<NomintMessage>),

    /// NOMRES — Nominierungsantwort (nomination response).
    #[cfg(feature = "nomres")]
    Nomres(Box<NomresMessage>),

    /// SCHEDL — Schedulingnachricht (transport schedule).
    #[cfg(feature = "schedl")]
    Schedl(Box<SchedlMessage>),

    /// IMBNOT — Imbalance Notification.
    #[cfg(feature = "imbnot")]
    Imbnot(Box<ImbalanceMessage>),

    /// TRANOT — Transport Notification.
    #[cfg(feature = "tranot")]
    Tranot(Box<TransportNotificationMessage>),

    /// DELORD — Delivery Order.
    #[cfg(feature = "delord")]
    Delord(Box<DeliveryOrderMessage>),

    /// DELRES — Delivery Response.
    #[cfg(feature = "delres")]
    Delres(Box<DeliveryResponseMessage>),

    /// An unrecognised or feature-disabled DVGW message type.
    ///
    /// Contains the raw EDIFACT message type code and the parsed segments.
    Unknown {
        /// Sanitized UNH message type code.
        raw_type: String,
        /// Raw parsed segments.
        segments: Vec<edifact_rs::OwnedSegment>,
    },
}

impl AnyDvgwMessage {
    /// Returns a [`DvgwMessage`] trait reference for common operations.
    ///
    /// Returns `None` for [`AnyDvgwMessage::Unknown`] since it does not
    /// implement the full trait.
    #[must_use]
    pub fn as_trait(&self) -> Option<&dyn DvgwMessage> {
        match self {
            #[cfg(feature = "alocat")]
            Self::Alocat(m) => Some(m.as_ref()),
            #[cfg(feature = "nomint")]
            Self::Nomint(m) => Some(m.as_ref()),
            #[cfg(feature = "nomres")]
            Self::Nomres(m) => Some(m.as_ref()),
            #[cfg(feature = "schedl")]
            Self::Schedl(m) => Some(m.as_ref()),
            #[cfg(feature = "imbnot")]
            Self::Imbnot(m) => Some(m.as_ref()),
            #[cfg(feature = "tranot")]
            Self::Tranot(m) => Some(m.as_ref()),
            #[cfg(feature = "delord")]
            Self::Delord(m) => Some(m.as_ref()),
            #[cfg(feature = "delres")]
            Self::Delres(m) => Some(m.as_ref()),
            Self::Unknown { .. } => None,
        }
    }

    /// Returns `true` if this is an [`AnyDvgwMessage::Unknown`] message.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    /// Returns the [`DvgwMessageType`](crate::DvgwMessageType) of the message,
    /// or `None` for the [`AnyDvgwMessage::Unknown`] variant.
    #[must_use]
    pub fn message_type(&self) -> Option<crate::DvgwMessageType> {
        self.as_trait()
            .map(super::message::DvgwMessage::message_type)
    }

    /// Returns the synthetic Prüfidentifikator (PID) for this message and the
    /// given direction qualifier, or `None` if the message type is unknown or
    /// no PID is defined for the combination.
    ///
    /// This is the primary routing key for DVGW messages in the
    /// `mako-engine` PID router. Pass the role qualifier from the sending
    /// market participant's NAD qualifier (e.g. `"Z15"`, `"Z01"`) or `None`
    /// to obtain the primary PID for the message type.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use dvgw_edi::{AnyDvgwMessage, DvgwPlatform};
    ///
    /// # let input: &[u8] = b"";
    /// let msg = DvgwPlatform::default().parse(input)?;
    /// if let Some(pid) = msg.detect_pid(None) {
    ///     println!("route via synthetic PID {pid}");
    /// }
    /// # Ok::<(), dvgw_edi::Error>(())
    /// ```
    #[must_use]
    pub fn detect_pid(&self, role_qualifier: Option<&str>) -> Option<u32> {
        self.message_type()?.synthetic_pid(role_qualifier)
    }
}
