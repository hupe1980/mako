//! [`UtilmdBuilder`] — fluent type-safe builder for UTILMD messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, ObjectType, Pruefidentifikator, Release};

use super::{Set, Unset, bytes_to_segments, today_ccyymmdd};

// ── Inner fields structs ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct UtilmdTransactionSpec {
    ide_qualifier: String,
    ide_id: String,
    status_code: Option<String>,
    process_dates: Vec<(String, String)>,
    location: Option<(String, String)>,
    references: Vec<(String, String)>,
    free_texts: Vec<(String, String)>,
    agr: Option<(String, String)>,
    customer_nad: Option<(String, String)>,
}

#[derive(Debug, Clone)]
struct UtilmdBuilderInner {
    release: Release,
    pruefidentifikator: Option<Pruefidentifikator>,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    document_date: Option<String>,
    rff_entries: Vec<(String, String)>,
    transactions: Vec<UtilmdTransactionSpec>,
}

// ── UtilmdBuilder ─────────────────────────────────────────────────────────────

/// Fluent builder for `UTILMD` (Utilities Master Data) messages.
///
/// # Type-state
///
/// [`build`](UtilmdBuilder::build) is only available once both
/// [`sender`](UtilmdBuilder::sender) and [`receiver`](UtilmdBuilder::receiver)
/// have been called. The compiler enforces this at the call site.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::{Release, Pruefidentifikator};
/// use edi_energy::builders::UtilmdBuilder;
///
/// let msg = UtilmdBuilder::new(Release::new("5.5.3a"))
///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
///     .sender("9900987654321")
///     .receiver("9900123456789")
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("9900987654321"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct UtilmdBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: UtilmdBuilderInner,
}

impl UtilmdBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: UtilmdBuilderInner {
                release,
                pruefidentifikator: None,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "E01".to_owned(),
                document_date: None,
                rff_entries: Vec::new(),
                transactions: Vec::new(),
            },
        }
    }
}

impl<S, R> UtilmdBuilder<S, R> {
    fn transition<S2, R2>(self) -> UtilmdBuilder<S2, R2> {
        UtilmdBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier (DE 3039).
    pub fn sender(mut self, id: impl Into<String>) -> UtilmdBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier (DE 3039).
    pub fn receiver(mut self, id: impl Into<String>) -> UtilmdBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`). Use [`AgencyCode::Entso`] (`"305"`)
    /// for TSO/ÜNB parties that carry a 16-char EIC code.
    pub fn sender_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.sender_agency = agency;
        self
    }

    /// Override the agency code for the receiver's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`).
    pub fn receiver_agency(mut self, agency: crate::AgencyCode) -> Self {
        self.inner.receiver_agency = agency;
        self
    }

    /// Set the Pruefidentifikator (process-variant code, e.g. `55001`).
    pub fn pruefidentifikator(mut self, pid: Pruefidentifikator) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Override the message reference number (UNH / DE 0062).  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Override the BGM document name code (DE 1001).  Defaults to `"E01"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Add a reference segment (RFF, SG1) to the message header.
    ///
    /// `qualifier` is the DE 1153 reference qualifier (e.g. `"ACE"`, `"Z13"`).
    /// `reference` is the reference identifier (DE 1154).
    ///
    /// UTILMD MIG 5.5.3a requires at least one `RFF` in SG1 (max 99).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use edi_energy::{Release, Pruefidentifikator};
    /// # use edi_energy::builders::UtilmdBuilder;
    /// let msg = UtilmdBuilder::new(Release::new("5.5.3a"))
    ///     .pruefidentifikator(Pruefidentifikator::new(55001).unwrap())
    ///     .sender("9900987654321")
    ///     .receiver("9900123456789")
    ///     .rff("ACE", "20230701")
    ///     .build()?;
    /// # Ok::<(), edi_energy::Error>(())
    /// ```
    pub fn rff(mut self, qualifier: impl Into<String>, reference: impl Into<String>) -> Self {
        self.inner
            .rff_entries
            .push((qualifier.into(), reference.into()));
        self
    }

    /// Start configuring a transaction (SG4 / IDE block) in the message.
    ///
    /// `object_type` is the BDEW supply-point object type (DE 7495 qualifier).
    /// Use the [`ObjectType`](crate::ObjectType) enum for type-safe, self-documenting
    /// code — e.g. `ObjectType::Marktlokation` (wire code `"Z18"`) or
    /// `ObjectType::Messlokation` (wire code `"Z19"`).
    /// `ide_id` is the object identifier (DE 7402).
    ///
    /// Returns a [`UtilmdTransactionBuilder`] sub-builder. Call
    /// [`done`](UtilmdTransactionBuilder::done) to finalize and return.
    pub fn transaction(
        self,
        object_type: ObjectType,
        ide_id: impl Into<String>,
    ) -> UtilmdTransactionBuilder<S, R> {
        self.transaction_with_qualifier(object_type.qualifier_code(), ide_id)
    }

    /// Start configuring a transaction with an explicit DE 7495 qualifier.
    ///
    /// The AHB fixes the IDE qualifier **per Prüfidentifikator** (e.g. `Z19`
    /// for GPKE/GeLi Lieferbeginn 55001/44001, `24` for the `WiM`
    /// Messlokations-PIDs), which does not always coincide with the
    /// [`ObjectType`] wire codes. Use this when the caller resolves the
    /// qualifier from the AHB rather than from an object type.
    pub fn transaction_with_qualifier(
        self,
        ide_qualifier: impl Into<String>,
        ide_id: impl Into<String>,
    ) -> UtilmdTransactionBuilder<S, R> {
        UtilmdTransactionBuilder {
            parent: self,
            spec: UtilmdTransactionSpec {
                ide_qualifier: ide_qualifier.into(),
                ide_id: ide_id.into(),
                ..Default::default()
            },
        }
    }
}

impl<S, R> UtilmdBuilder<S, R> {
    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let pid_str = self
            .inner
            .pruefidentifikator
            .map(|p| format!("{:05}", p.as_u32()))
            .unwrap_or_default();
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(today_ccyymmdd, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["UTILMD", "D", "11A", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", &self.inner.document_code, &pid_str, "9");
        emit_comp!(w, "DTM", ["137", &dtm_val, "102"]);
        for (qualifier, reference) in &self.inner.rff_entries {
            emit_comp!(w, "RFF", [qualifier, reference]);
        }
        if let Some(id) = &self.inner.sender_id {
            emit_comp!(
                w,
                "NAD",
                ["MS"],
                [id, "", self.inner.sender_agency.as_str()]
            );
        }
        if let Some(id) = &self.inner.receiver_id {
            emit_comp!(
                w,
                "NAD",
                ["MR"],
                [id, "", self.inner.receiver_agency.as_str()]
            );
        }
        for tx in &self.inner.transactions {
            emit_seg!(w, "IDE", &tx.ide_qualifier, &tx.ide_id);
            if let Some(status) = &tx.status_code {
                emit_seg!(w, "STS", status);
            }
            for (qualifier, date_val) in &tx.process_dates {
                emit_comp!(w, "DTM", [qualifier, date_val, "102"]);
            }
            if let Some((loc_q, loc_id)) = &tx.location {
                emit_comp!(w, "LOC", [loc_q], [loc_id, "", "293"]);
            }
            for (rff_q, rff_ref) in &tx.references {
                emit_comp!(w, "RFF", [rff_q, rff_ref]);
            }
            for (ftx_q, ftx_text) in &tx.free_texts {
                emit_comp!(w, "FTX", [ftx_q], [""], [""], [ftx_text]);
            }
            if let Some((svc_req, resp_type)) = &tx.agr {
                emit_comp!(w, "AGR", [svc_req, resp_type]);
            }
            if let Some((nad_q, nad_id)) = &tx.customer_nad {
                emit_comp!(w, "NAD", [nad_q], [nad_id, "", "293"]);
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

impl UtilmdBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::utilmd::UtilmdMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::utilmd::UtilmdMessage, Error> {
        let pid = self
            .inner
            .pruefidentifikator
            .map(super::super::pruefidentifikator::Pruefidentifikator::as_u32);
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::utilmd::UtilmdMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}

// ── UtilmdTransactionBuilder ──────────────────────────────────────────────────

/// Sub-builder for a transaction (SG4 / IDE block) in a UTILMD message.
///
/// Obtained via [`UtilmdBuilder::transaction`]. Call
/// [`done`](UtilmdTransactionBuilder::done) to finalize and return to the
/// parent builder.
#[derive(Debug)]
#[must_use = "Sub-builder must be finalized with .done()"]
pub struct UtilmdTransactionBuilder<S = Unset, R = Unset> {
    parent: UtilmdBuilder<S, R>,
    spec: UtilmdTransactionSpec,
}

impl<S, R> UtilmdTransactionBuilder<S, R> {
    /// Set the STS status code (DE 9015), e.g. `"E07"` for Sperrung.
    pub fn status(mut self, code: impl Into<String>) -> Self {
        self.spec.status_code = Some(code.into());
        self
    }

    /// Add a process-date DTM segment inside SG4.
    ///
    /// `qualifier` is DE 2005 (e.g. `"163"` for delivery start, `"164"` for end).
    /// `date` is `YYYYMMDD`.
    pub fn process_date(mut self, qualifier: impl Into<String>, date: impl Into<String>) -> Self {
        self.spec
            .process_dates
            .push((qualifier.into(), date.into()));
        self
    }

    /// Set the SG5/LOC location segment.
    pub fn location(mut self, qualifier: impl Into<String>, id: impl Into<String>) -> Self {
        self.spec.location = Some((qualifier.into(), id.into()));
        self
    }

    /// Add a SG6/RFF reference segment.
    pub fn reference(mut self, qualifier: impl Into<String>, ref_id: impl Into<String>) -> Self {
        self.spec.references.push((qualifier.into(), ref_id.into()));
        self
    }

    /// Set the SG12/NAD customer segment.
    pub fn customer(mut self, party_qualifier: impl Into<String>, id: impl Into<String>) -> Self {
        self.spec.customer_nad = Some((party_qualifier.into(), id.into()));
        self
    }

    /// Add a free-text (FTX) segment inside SG4.
    pub fn free_text(mut self, text_function: impl Into<String>, text: impl Into<String>) -> Self {
        self.spec
            .free_texts
            .push((text_function.into(), text.into()));
        self
    }

    /// Set the AGR (Agreement Identification) segment inside SG4.
    pub fn agr(
        mut self,
        service_requirement: impl Into<String>,
        response_type: impl Into<String>,
    ) -> Self {
        self.spec.agr = Some((service_requirement.into(), response_type.into()));
        self
    }

    /// Finalize this transaction and return to the parent [`UtilmdBuilder`].
    pub fn done(mut self) -> UtilmdBuilder<S, R> {
        self.parent.inner.transactions.push(self.spec);
        self.parent
    }
}
