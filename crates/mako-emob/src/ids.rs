//! The identifiers Modell 2 needs that the market does not issue.
//!
//! Everything the BDEW *does* issue is reused rather than redefined:
//! [`rubo4e::identifiers::MaloId`] for the physical Marktlokation,
//! [`mako_mabis::BilanzierungsgebietId`] and [`mako_mabis::BilanzkreisId`] for
//! the virtual BG and the Bilanzkreise it books into,
//! [`mako_mabis::MabisZaehlpunktId`] for the MaBiS-Zählpunkt.
//!
//! Three things have no issuing authority, and this module is careful about
//! why.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A **virtual Marktlokation** inside the LPB's Bilanzierungsgebiet.
///
/// # Why this is not a `MaloId`
///
/// The BDEW issues MaLo-IDs as eleven digits with a check digit, from ranges
/// delegated to Netzbetreiber. AWH Kap. 1.6.1 permits the LPB to use its
/// Stromnetzbetreibernummer for **Zählpunktbildung und die BG-Beantragung** —
/// and for nothing else. Nothing in Anlage 6 or the AWH grants an LPB a MaLo-ID
/// range for the per-vehicle, per-token objects it needs internally.
///
/// So these IDs are deliberately **not** in the 11-digit space: minting a
/// plausible-looking MaLo-ID would collide with a real one the moment a
/// Netzbetreiber issued it, and the collision would surface as energy booked
/// to a stranger's Bilanzkreis. A `VirtualMaloId` is opaque, namespaced by the
/// operator, and [`VirtualMaloId::new`] refuses anything that could be mistaken
/// for a MaLo-ID.
///
/// If the BDEW later opens a range, this type gains a variant — it does not
/// become `MaloId`, because the two remain different objects.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct VirtualMaloId(String);

/// Why a [`VirtualMaloId`] was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidVirtualMaloId {
    /// Empty, or longer than [`VirtualMaloId::MAX_LEN`].
    #[error("a virtual MaLo id must be 1..={max} characters, got {len}")]
    Length {
        /// The length supplied.
        len: usize,
        /// The maximum allowed.
        max: usize,
    },
    /// Eleven digits — indistinguishable from a BDEW MaLo-ID.
    #[error(
        "'{0}' is eleven digits and would collide with the BDEW MaLo-ID space; \
         namespace virtual Marktlokationen instead"
    )]
    LooksLikeMaloId(String),
    /// A character outside `[A-Za-z0-9._:-]`.
    #[error("'{0}' contains a character outside [A-Za-z0-9._:-]")]
    Character(String),
}

impl VirtualMaloId {
    /// The longest a virtual MaLo id may be.
    pub const MAX_LEN: usize = 64;

    /// Validate and wrap.
    ///
    /// # Errors
    ///
    /// [`InvalidVirtualMaloId::LooksLikeMaloId`] when `s` is exactly eleven
    /// ASCII digits — see the type docs for why that is refused rather than
    /// accepted.
    pub fn new(s: impl Into<String>) -> Result<Self, InvalidVirtualMaloId> {
        let s = s.into();
        if s.is_empty() || s.len() > Self::MAX_LEN {
            return Err(InvalidVirtualMaloId::Length {
                len: s.len(),
                max: Self::MAX_LEN,
            });
        }
        if s.len() == 11 && s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidVirtualMaloId::LooksLikeMaloId(s));
        }
        if !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
        {
            return Err(InvalidVirtualMaloId::Character(s));
        }
        Ok(Self(s))
    }

    /// The wrapped value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VirtualMaloId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identifier of one Ladevorgang, as the CPO backend knows it.
///
/// Free-form on purpose: OCPI calls it a `CDR.id`, OCPP a `transactionId`, and
/// a device log may have neither. What matters here is only that it is stable
/// enough to deduplicate a late-arriving CDR against a value already allocated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Wrap a session identifier.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// The wrapped value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A reference to the contract token a session authenticated with — **never the
/// token itself**.
///
/// An RFID UID or an eMAID identifies a natural person's charging contract
/// across every operator they visit. It is personal data under Art. 4 Nr. 1
/// GDPR, and the allocation does not need it: the allocation needs to know
/// *which virtual MaLo* a session belongs to, which is a lookup the token
/// registry performs once, upstream.
///
/// So this type carries an opaque, keyed hash produced by that registry.
/// Nothing here can reverse it, and a leaked allocation ledger discloses no
/// contract identities. Unknown tokens are not an error — they route to the
/// Residual-MaLo.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TokenRef(String);

impl TokenRef {
    /// Wrap a keyed hash produced by the token registry.
    ///
    /// The caller is responsible for the keying; this type only guarantees
    /// that a raw token never reaches the allocation ledger by *accident*.
    #[must_use]
    pub fn from_keyed_hash(hash: impl Into<String>) -> Self {
        Self(hash.into())
    }

    /// The wrapped hash.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TokenRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A check-digit-valid MaLo-ID: the one that would actually collide.
    #[test]
    fn an_eleven_digit_id_is_refused() {
        let err = VirtualMaloId::new("51238297068").unwrap_err();
        assert!(matches!(err, InvalidVirtualMaloId::LooksLikeMaloId(_)));
    }

    /// Eleven digits are refused on their shape alone — the check digit is
    /// never consulted, because a Netzbetreiber issuing the valid neighbour of
    /// a minted id collides just as hard.
    #[test]
    fn eleven_digits_are_refused_even_with_a_wrong_check_digit() {
        assert!(VirtualMaloId::new("51238297069").is_err());
    }

    /// Eleven *characters* are fine — it is the all-digit shape that collides.
    #[test]
    fn eleven_characters_that_are_not_all_digits_are_fine() {
        assert!(VirtualMaloId::new("veh-1234567").is_ok());
    }

    #[test]
    fn other_digit_lengths_are_fine() {
        assert!(VirtualMaloId::new("512382970").is_ok());
        assert!(VirtualMaloId::new("512382970699").is_ok());
    }

    #[test]
    fn empty_and_overlong_are_refused() {
        assert!(VirtualMaloId::new("").is_err());
        assert!(VirtualMaloId::new("x".repeat(VirtualMaloId::MAX_LEN + 1)).is_err());
    }

    #[test]
    fn separators_are_allowed_but_spaces_are_not() {
        assert!(VirtualMaloId::new("cpo:fleet.42_a-b").is_ok());
        assert!(VirtualMaloId::new("cpo fleet").is_err());
    }
}
