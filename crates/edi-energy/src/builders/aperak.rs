//! [`AperakBuilder`] — fluent type-safe builder for APERAK messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments};

/// `ERC` DE 9321 codes for which `SG5 FTX+Z02` „Ortsangabe des AHB-Fehlers" is
/// Muss.
///
/// APERAK AHB 1.0 makes Nr 00017 `Muss [4] ∧ ([5] ∨ [9] ∨ [10] ∨ [11] ∨ [12] ∨
/// [13])`, and those six Bedingungen are „Wenn SG4 ERC+Z29 / Z35 / Z38 / Z39 /
/// Z41 / Z40 vorhanden." — the codes that say the message violates the
/// Anwendungshandbuch, as opposed to failing a business check.
pub const CODES_REQUIRING_ORTSANGABE: [&str; 6] = ["Z29", "Z35", "Z38", "Z39", "Z40", "Z41"];

/// Whether an `ERC` DE 9321 code obliges an `SG5 FTX+Z02`.
#[must_use]
pub fn requires_ortsangabe(code: &str) -> bool {
    CODES_REQUIRING_ORTSANGABE.contains(&code)
}

#[derive(Debug, Clone)]
struct AperakBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: Option<AgencyCode>,
    receiver_agency: Option<AgencyCode>,
    message_ref: String,
    /// `BGM` DE 1001. `None` derives it from the presence of an error code:
    /// `313` Verarbeitbarkeitsfehlermeldung when one is set, `312`
    /// Anerkennungsmeldung when none is. Those are the only two codes the
    /// APERAK MIG admits.
    document_code: Option<String>,
    document_id: Option<String>,
    acw_ref: Option<String>,
    /// `SG5 RFF+AGO` — the Dokumentennummer of the message answered.
    ago_ref: Option<String>,
    /// `SG2 DTM+171` — the Dokumentendatum of the message answered, `CCYYMMDDHHMM`
    /// UTC, wire form.
    reference_date: Option<String>,
    error_code: Option<String>,
    error_text: Option<String>,
    /// `SG5 FTX+Z02` — where in the acknowledged message the AHB is violated.
    ortsangabe: Option<String>,
    document_date: Option<String>,
}

/// Fluent builder for `APERAK` (Application Error and Acknowledgement) messages.
///
/// # Type-state
///
/// [`build`](AperakBuilder::build) is only available once both
/// [`sender`](AperakBuilder::sender) and [`receiver`](AperakBuilder::receiver)
/// have been called.
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct AperakBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: AperakBuilderInner,
}

impl AperakBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: AperakBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: None,
                receiver_agency: None,
                message_ref: "1".to_owned(),
                document_code: None,
                document_id: None,
                acw_ref: None,
                ago_ref: None,
                reference_date: None,
                error_code: None,
                error_text: None,
                ortsangabe: None,
                document_date: None,
            },
        }
    }
}

impl<S, R> AperakBuilder<S, R> {
    /// Address this APERAK as the acknowledgement of a received message.
    ///
    /// Mirrors the parties (the original receiver becomes the sender), carries
    /// the acknowledged message's UNH reference into `RFF+ACW`, and adopts its
    /// transmission date. Those three fields are what correlate an
    /// acknowledgement with what it acknowledges, and deriving them from the
    /// received message is the only way they cannot drift apart.
    ///
    /// The release is the **APERAK's own**, set on [`new`](Self::new) — not the
    /// release of the message being acknowledged, which is usually a different
    /// message type on a different track.
    ///
    /// ```rust,no_run
    /// # #[cfg(all(feature = "aperak", feature = "utilmd"))]
    /// # fn main() -> Result<(), edi_energy::Error> {
    /// use edi_energy::{Platform, Release};
    /// use edi_energy::builders::AperakBuilder;
    ///
    /// # let wire: &[u8] = b"";
    /// let received = Platform::with_all_profiles().parse_interchange_full(wire)?;
    /// let ack = AperakBuilder::new(Release::new("2.1i"))
    ///     .for_receipt(&received.messages[0].receipt_context())
    ///     .error_code("Z10")
    ///     .serialize()?;
    /// # Ok(())
    /// # }
    /// # #[cfg(not(all(feature = "aperak", feature = "utilmd")))]
    /// # fn main() {}
    /// ```
    pub fn for_receipt(
        mut self,
        ctx: &crate::interchange::ReceiptContext<'_>,
    ) -> AperakBuilder<Set, Set> {
        self.inner.sender_id = Some(ctx.original_receiver.to_owned());
        self.inner.receiver_id = Some(ctx.original_sender.to_owned());
        self.inner.acw_ref = Some(ctx.message_ref.to_owned());
        if let Some(date) = ctx.transmission_date {
            self.inner.document_date = Some(format!(
                "{:04}{:02}{:02}",
                date.year(),
                date.month() as u8,
                date.day()
            ));
        }
        self.transition()
    }

    fn transition<S2, R2>(self) -> AperakBuilder<S2, R2> {
        AperakBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> AperakBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> AperakBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Leave unset and the agency is derived from the MP-ID itself —
    /// [`AgencyCode::for_mp_id`]: `99…` → BDEW `293`, `98…` → DVGW `332`, any
    /// other 13-digit code → GS1 `9`. Override only for a party whose
    /// registered code list differs from what its number implies.
    pub fn sender_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.sender_agency = Some(agency);
        self
    }

    /// Override the agency code for the receiver's party identifier.
    ///
    /// Derived from the MP-ID when unset — see
    /// [`sender_agency`](Self::sender_agency).
    pub fn receiver_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.receiver_agency = Some(agency);
        self
    }

    /// Set the Prüfidentifikator (BGM document identifier).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.document_id = Some(pid.as_u32().to_string());
        self
    }

    /// The Nachrichten-Referenznummer of the message answered — `SG2 RFF+ACE`
    /// and `SG5 RFF+ACW` both carry it.
    pub fn acw_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.acw_ref = Some(reference.into());
        self
    }

    /// The Dokumentennummer (`BGM` DE 1004) of the message answered —
    /// `RFF+AGO`, in `SG2` on an Anerkennungsmeldung and in `SG5` on a
    /// rejection. Defaults to the Nachrichten-Referenznummer.
    pub fn ago_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.ago_ref = Some(reference.into());
        self
    }

    /// The Dokumentendatum of the message answered (`CCYYMMDD` or
    /// `CCYYMMDDHHMM`, UTC) — `SG2 DTM+171`. Defaults to this message's date.
    pub fn reference_date(mut self, ccyymmddhhmm: impl Into<String>) -> Self {
        self.inner.reference_date = Some(ccyymmddhhmm.into());
        self
    }

    /// Set an application error code (ERC segment).
    pub fn error_code(mut self, code: impl Into<String>) -> Self {
        self.inner.error_code = Some(code.into());
        self
    }

    /// Set a free-text error description (`FTX+ABO`, the qualifier the MIG admits).
    pub fn error_text(mut self, text: impl Into<String>) -> Self {
        self.inner.error_text = Some(text.into());
        self
    }

    /// `SG5 FTX+Z02` „Ortsangabe des AHB-Fehlers" — where in the acknowledged
    /// message the reported error is.
    ///
    /// Muss for the six codes [`requires_ortsangabe`] names; unset, the
    /// error text stands in for it, because an APERAK carrying one of those
    /// codes without it is refused by the receiving Marktpartner.
    pub fn ortsangabe(mut self, where_: impl Into<String>) -> Self {
        self.inner.ortsangabe = Some(where_.into());
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Override the BGM document function code (DE 1001).
    ///
    /// Leave it unset. The APERAK MIG admits exactly two codes and which one
    /// applies follows from the message itself:
    ///
    /// - `"312"` — Anerkennungsmeldung (PID 29002), no `SG4` Fehlerbeschreibung
    /// - `"313"` — Verarbeitbarkeitsfehlermeldung (PID 29001), `SG4` present
    ///
    /// so the default is derived from [`error_code`](Self::error_code) rather
    /// than fixed. `"1000"` — the generic UN/EDIFACT APERAK code — is *not*
    /// admitted by any EDI@Energy Prüfschablone; passing it here produces a
    /// message the receiving Marktpartner rejects.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// The `BGM` DE 1001 this message will carry.
    fn effective_document_code(&self) -> &str {
        match self.inner.document_code.as_deref() {
            Some(code) => code,
            None if self.inner.error_code.is_some() => "313",
            None => "312",
        }
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(super::now_ccyymmddhhmm, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        // `BGM` DE 1004 is the Dokumentennummer — the message reference unless
        // one is given.
        let doc_id = self
            .inner
            .document_id
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["APERAK", "D", "07B", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", self.effective_document_code(), doc_id);
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // `SG2` — the message answered, by reference and date, before the
        // parties (APERAK MIG Nr 00004/00005).
        if let Some(r) = &self.inner.acw_ref {
            emit_comp!(w, "RFF", ["ACE", r]);
            let reference_date = self.inner.reference_date.as_deref().unwrap_or(&dtm_val);
            emit_comp!(
                w,
                "DTM",
                ["171", &super::ccyymmddhhmm_utc(reference_date), "303"]
            );
            // `RFF+AGO` has two places, and which one it takes is decided by
            // `SG4`: Nr 00006 in `SG2` for an Anerkennungsmeldung, Nr 00015
            // inside the Fehlerbeschreibung for a rejection. An
            // Anerkennungsmeldung opens no `SG4`, so an AGO emitted down there
            // would sit in a group that never starts — `MIG-STRUCTURE`, plus a
            // Muss `SG2` reported missing on top.
            if self.inner.error_code.is_none() {
                let ago = self.inner.ago_ref.as_deref().unwrap_or(r);
                emit_comp!(w, "RFF", ["AGO", ago]);
            }
        }
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", super::agency_for(self.inner.sender_agency, id)]
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", super::agency_for(self.inner.receiver_agency, id)]
            );
        }
        // `SG5` — the Fehlermeldung: code, text, then the references of the
        // message it is about (MIG Nr 00012–00015).
        if let Some(code) = &self.inner.error_code {
            emit_seg!(w, "ERC", code);
        }
        if let Some(text) = &self.inner.error_text {
            emit_comp!(w, "FTX", ["ABO"], [""], [""], [text]);
        }
        if let Some(code) = &self.inner.error_code
            && let Some(r) = &self.inner.acw_ref
        {
            emit_comp!(w, "RFF", ["ACW", r]);
            let ago = self.inner.ago_ref.as_deref().unwrap_or(r);
            emit_comp!(w, "RFF", ["AGO", ago]);
            // `SG5 FTX+Z02` (Nr 00017) is Muss for the six codes that report an
            // AHB violation rather than a business one — the receiver is told
            // *where* the message breaks the Anwendungshandbuch, not only that
            // it does. Emitted only for those, because for every other code the
            // AHB does not ask for it.
            if requires_ortsangabe(code) {
                let ortsangabe = self
                    .inner
                    .ortsangabe
                    .as_deref()
                    .or(self.inner.error_text.as_deref())
                    .unwrap_or(code);
                emit_comp!(w, "FTX", ["Z02"], [""], [""], [ortsangabe]);
            }
        }
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

impl AperakBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::aperak::AperakMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::aperak::AperakMessage, Error> {
        let pid = self
            .inner
            .document_id
            .as_deref()
            .and_then(|s| s.parse::<u32>().ok());
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::aperak::AperakMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}
