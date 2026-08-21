//! [`ContrlBuilder`] — fluent type-safe builder for CONTRL messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

#[derive(Debug, Clone)]
struct ContrlBuilderInner {
    release: Release,
    message_ref: String,
    interchange_ref: String,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    action_code: String,
}

/// Fluent builder for `CONTRL` (Syntax and Service Report) messages.
///
/// CONTRL is the UN/EDIFACT acknowledgement message — it reports whether
/// a received interchange was accepted or rejected.
///
/// # Type-state
///
/// [`build`](ContrlBuilder::build) is only available once both
/// [`sender`](ContrlBuilder::sender) and [`receiver`](ContrlBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::ContrlBuilder;
///
/// let msg = ContrlBuilder::new(Release::new("1.0a"))
///     .interchange_ref("INTER-2024-001")
///     .sender("9900111222333")
///     .receiver("9900444555666")
///     .accept()
///     .build()?;
///
/// assert_eq!(msg.uci().unwrap().action_code.as_deref(), Some("4"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct ContrlBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: ContrlBuilderInner,
}

impl ContrlBuilder<Unset, Unset> {
    /// Create a builder for the given EDI@Energy CONTRL release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: ContrlBuilderInner {
                release,
                message_ref: "1".to_owned(),
                interchange_ref: String::new(),
                sender_id: None,
                receiver_id: None,
                action_code: "4".to_owned(),
            },
        }
    }
}

impl<S, R> ContrlBuilder<S, R> {
    /// Address this CONTRL as the acknowledgement of a received interchange.
    ///
    /// Mirrors the UNB parties and carries the received Datenaustauschreferenz
    /// into UCI element 0. A CONTRL acknowledges the **interchange**, not a
    /// message inside it, which is why this takes the header rather than a
    /// [`ReceiptContext`](crate::interchange::ReceiptContext): a syntax failure
    /// can prevent any message from being identified at all.
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "contrl", feature = "utilmd"))]
    /// # fn main() -> Result<(), edi_energy::Error> {
    /// use edi_energy::{Platform, Release};
    /// use edi_energy::builders::ContrlBuilder;
    ///
    /// # let wire: &[u8] = b"";
    /// let received = Platform::with_all_profiles().parse_interchange_full(wire)?;
    /// let ack = ContrlBuilder::new(Release::new("2.0c"))
    ///     .for_interchange(&received.header)
    ///     .accept()
    ///     .serialize()?;
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "contrl", feature = "utilmd")))]
    /// # fn main() {}
    /// ```
    pub fn for_interchange(
        mut self,
        header: &crate::interchange::InterchangeHeader,
    ) -> ContrlBuilder<Set, Set> {
        self.inner.sender_id = Some(header.receiver_id.to_string());
        self.inner.receiver_id = Some(header.sender_id.to_string());
        self.inner.interchange_ref = header.control_ref.to_string();
        self.transition()
    }

    fn transition<S2, R2>(self) -> ContrlBuilder<S2, R2> {
        ContrlBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the sender identification (UCI element 1 / DE 0004).
    pub fn sender(mut self, id: impl Into<String>) -> ContrlBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the recipient identification (UCI element 2 / DE 0010).
    pub fn receiver(mut self, id: impl Into<String>) -> ContrlBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Set the interchange control reference being acknowledged (UCI element 0).
    pub fn interchange_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.interchange_ref = reference.into();
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set action code to `4` — interchange accepted.
    pub fn accept(mut self) -> Self {
        "4".clone_into(&mut self.inner.action_code);
        self
    }

    /// Set action code to `8` — interchange rejected (entire interchange).
    pub fn reject(mut self) -> Self {
        "8".clone_into(&mut self.inner.action_code);
        self
    }

    /// Set action code to `7` — interchange rejected (group-level).
    pub fn reject_group(mut self) -> Self {
        "7".clone_into(&mut self.inner.action_code);
        self
    }

    /// Set an explicit UCI action code (DE 0083).
    ///
    /// Prefer [`accept`][Self::accept] / [`reject`][Self::reject] for common cases.
    pub fn action_code(mut self, code: impl Into<String>) -> Self {
        self.inner.action_code = code.into();
        self
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let sender = self.inner.sender_id.as_deref().unwrap_or("");
        let receiver = self.inner.receiver_id.as_deref().unwrap_or("");

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["CONTRL", "D", "3", "UN", self.inner.release.as_str()]
        );
        emit_seg!(
            w,
            "UCI",
            &self.inner.interchange_ref,
            sender,
            receiver,
            &self.inner.action_code
        );
        w.finish_unt(&self.inner.message_ref)
            .map_err(Error::Parse)?;
        Ok(buf)
    }
    /// Build and serialize the message to EDIFACT bytes.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if serialization fails.
    pub fn serialize(self) -> Result<Vec<u8>, Error> {
        self.to_bytes()
    }
}

impl ContrlBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::contrl::ContrlMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::contrl::ContrlMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::contrl::ContrlMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None, // CONTRL has no BGM → no Pruefidentifikator
        ))
    }
}
