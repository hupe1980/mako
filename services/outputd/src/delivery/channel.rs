//! The delivery channels, and what each can honestly claim.
//!
//! Every channel is an adapter over something an operator already runs. What
//! outputd owns is *when* to send, *what* counts as sent, and the evidence.

use serde::Deserialize;

use super::store::Recipient;

/// How a document reaches its recipient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The customer's portal inbox, served by `portald` out of this store.
    Portal,
    /// A configured mail relay.
    Email,
    /// A spool a print service pulls.
    Post,
    /// The operator's own system, which then owns delivery.
    Erp,
}

impl Channel {
    /// The stored spelling — the `document_deliveries.channel` CHECK values.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Portal => "PORTAL",
            Self::Email => "EMAIL",
            Self::Post => "POST",
            Self::Erp => "ERP",
        }
    }

    /// Parse a stored or requested channel. `None` for anything else — a
    /// channel this build does not know is never silently treated as one it
    /// does.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_uppercase().as_str() {
            "PORTAL" => Some(Self::Portal),
            "EMAIL" => Some(Self::Email),
            "POST" => Some(Self::Post),
            "ERP" => Some(Self::Erp),
            _ => None,
        }
    }

    /// Every channel, for the error message that lists them.
    pub const ALL: &'static [Self] = &[Self::Portal, Self::Email, Self::Post, Self::Erp];

    /// What this channel would send to, given the recipient on file.
    ///
    /// `None` means there is nothing to send to, which [`super::store::issue`]
    /// records as `SUPPRESSED` with a reason. `Portal` and `Erp` need no
    /// target: one is served from this store, the other from a configured URL.
    #[must_use]
    pub fn target_for(self, recipient: &Recipient) -> Option<String> {
        match self {
            Self::Portal | Self::Erp => None,
            Self::Email => recipient.email.clone(),
            Self::Post => recipient
                .address
                .as_ref()
                .map(std::string::ToString::to_string),
        }
    }

    /// Whether a successful send means the document **arrived** (`DELIVERED`)
    /// or only that it was handed off (`SENT`).
    ///
    /// Only `Portal` claims arrival by itself: publishing puts the document in
    /// the recipient's sphere, which is what § 126b BGB asks of a durable
    /// medium. A relay accepting an e-mail is not the recipient's server
    /// accepting it; that arrives through
    /// `POST /api/v1/deliveries/{id}/status`.
    #[must_use]
    pub const fn arrival_is_observable_at_send(self) -> bool {
        matches!(self, Self::Portal)
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Channel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "unknown delivery channel {s:?}; expected one of PORTAL, EMAIL, POST, ERP"
            ))
        })
    }
}

impl serde::Serialize for Channel {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// What a relay answers. Every field optional: a `200` with an empty body has
/// still accepted the document, and demanding a schema of an operator's own
/// adapter would make the common case the failing one.
#[derive(Debug, Default, Deserialize)]
pub struct RelayReceipt {
    /// The relay's identifier for what it accepted — an SMTP message id, a
    /// print batch. Recorded as the evidence.
    #[serde(default)]
    pub message_id: Option<String>,
    /// Set by a relay that can observe arrival (a mail gateway with delivery
    /// notifications). Absent → the send counts as handed off, not arrived.
    #[serde(default)]
    pub delivered: Option<bool>,
}
