//! [`OrdrespBuilder`] — fluent type-safe builder for ORDRSP messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

#[derive(Debug, Clone)]
struct OrdrespBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_id: Option<String>,
    document_date: Option<String>,
    pruefidentifikator: Option<u32>,
    /// BGM DE 1001. Defaults to `7`; the ESA answers use `Z57`.
    document_code: Option<String>,
    /// SG1 references as `(1153 qualifier, 1154 value)`. Additive: an ESA
    /// answer carries the correlation reference *and* `RFF+Z13`.
    references: Vec<(String, String)>,
    /// `IMD+7081` Abonnement code — Muss on 19011/19012.
    abonnement: Option<String>,
    // Additive ESA-Antwort (PID 19011-19014) content — ORDRSP carries no LOC.
    /// SG2 `AJT` as `(4465 Prüfschritt-Code, 1082 EBD)`.
    adjustment: Option<String>,
    /// SG2 `AJT` DE 1082 — the Entscheidungsbaum the Prüfschritt comes from.
    adjustment_ebd: Option<String>,
    adjustment_reason: Option<String>,
    item_description: bool,
    line_item: bool,
}

/// Fluent builder for `ORDRSP` (Purchase Order Response) messages.
///
/// Wire type string: `ORDRSP:D:10A:UN:{release}`.
///
/// # Type-state
///
/// [`build`](OrdrespBuilder::build) is only available once both
/// [`sender`](OrdrespBuilder::sender) and [`receiver`](OrdrespBuilder::receiver)
/// have been called.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::OrdrespBuilder;
///
/// let msg = OrdrespBuilder::new(Release::new("1.4b"))
///     .sender("9900357000004")
///     .receiver("4012345000023")
///     .document_id("ORDRSP20251001001")
///     .build()?;
///
/// assert_eq!(msg.assoc_code(), "1.4b");
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct OrdrespBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: OrdrespBuilderInner,
}

impl OrdrespBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: OrdrespBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_id: None,
                document_date: None,
                pruefidentifikator: None,
                document_code: None,
                references: Vec::new(),
                abonnement: None,
                adjustment: None,
                adjustment_ebd: None,
                adjustment_reason: None,
                item_description: false,
                line_item: false,
            },
        }
    }
}

impl<S, R> OrdrespBuilder<S, R> {
    fn transition<S2, R2>(self) -> OrdrespBuilder<S2, R2> {
        OrdrespBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> OrdrespBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> OrdrespBuilder<S, Set> {
        self.inner.receiver_id = Some(id.into());
        self.transition()
    }

    /// Override the agency code for the sender's party identifier.
    ///
    /// Default: [`AgencyCode::Bdew`] (`"293"`). Use [`AgencyCode::Etso`] (`"305"`)
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

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the message reference number. Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the document date for DTM+137 (`YYYYMMDD`).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Set the Prüfidentifikator (BGM DE 1004) — the routing key of the answer
    /// (e.g. 19011 Bestätigung, 19012 Ablehnung for ESA Wertebestellung).
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Add an SG1 reference `RFF+<qual>:<value>`. Additive.
    pub fn reference(mut self, qualifier: impl Into<String>, value: impl Into<String>) -> Self {
        self.inner.references.push((qualifier.into(), value.into()));
        self
    }

    /// Set the BGM DE 1001 document code. Defaults to `7`.
    ///
    /// The ESA Wertebestellung answers use `Z57` („Übermittlung von Werten an
    /// ESA“, ORDRSP AHB 1.1b §4.15).
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = Some(code.into());
        self
    }

    /// Set the `IMD+7081` Abonnement code (`Z01`/`Z02`/`Z03`).
    ///
    /// **Muss** on ORDRSP 19011/19012; it echoes the ORDERS being answered and
    /// selects the EBD the `AJT` cites.
    pub fn abonnement(mut self, code: impl Into<String>) -> Self {
        self.inner.abonnement = Some(code.into());
        self
    }

    /// Set the SG2 adjustment — emits `AJT+<4465 Prüfschritt>+<1082 EBD>`.
    ///
    /// **Muss** on every ESA answer PID: DE 4465 carries the Prüfschritt code
    /// and DE 1082 the Entscheidungsbaum it belongs to (`E_0254`, `E_0256`,
    /// `E_0257`). Without the EBD the receiver cannot resolve what the code
    /// means — the same numeric code lives in several trees.
    pub fn adjustment(mut self, code: impl Into<String>, ebd: impl Into<String>) -> Self {
        self.inner.adjustment = Some(code.into());
        self.inner.adjustment_ebd = Some(ebd.into());
        self
    }

    /// Set the SG2 adjustment reason — emits `FTX+<code>` after the AJT.
    ///
    /// The MIG caps this FTX at two elements, so the reason is carried as the
    /// coded 4451 qualifier (`∈ {AAP, ABO, Z27, Z28, Z33}`), not free text.
    pub fn adjustment_reason(mut self, ftx_qualifier: impl Into<String>) -> Self {
        self.inner.adjustment_reason = Some(ftx_qualifier.into());
        self
    }

    /// Emit a minimal item description `IMD+A` (the AHB requires the segment).
    pub fn item_description(mut self) -> Self {
        self.inner.item_description = true;
        self
    }

    /// Emit an SG27 line item `LIN+1` (the AHB requires one for PID 19011).
    pub fn line_item(mut self) -> Self {
        self.inner.line_item = true;
        self
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let dtm_val = self
            .inner
            .document_date
            .as_deref()
            .map_or_else(super::now_ccyymmddhhmm, str::to_owned);

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        // BGM DE 1004 carries the Prüfidentifikator (profile pid_source =
        // BgmDe1004); fall back to `document_id` for non-MaKo callers.
        // BGM DE 1004 is the **Dokumentennummer** (ORDRSP AHB 1.1b); the
        // Prüfidentifikator has its own `SG1 RFF+Z13`.
        let bgm_1004 = self
            .inner
            .document_id
            .as_deref()
            .unwrap_or(&self.inner.message_ref);
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["ORDRSP", "D", "10A", "UN", self.inner.release.as_str()]
        );
        // ORDRSP BGM: DE 1001 = 7 (the only value the MIG permits), DE 1004 =
        // the Prüfidentifikator, and no third element.
        emit_seg!(
            w,
            "BGM",
            self.inner.document_code.as_deref().unwrap_or("7"),
            bgm_1004
        );
        // `DTM+137` Dokumentendatum. Every EDI@Energy AHB gives DE 2379 as
        // `303` (`CCYYMMDDHHMMZZZ`) with condition `[931]` fixing the zone to
        // `+00`; `[494]` requires the stamp to be the creation moment or
        // earlier. There is no Anwendungsfall in any AHB that takes `102`.
        emit_comp!(w, "DTM", ["137", &super::ccyymmddhhmm_utc(&dtm_val), "303"]);
        // Abonnement — `IMD++<7081>` (DE 7081 sits in C272, element 2).
        if let Some(code) = &self.inner.abonnement {
            emit_comp!(w, "IMD", [""], [code]);
        } else if self.inner.item_description {
            // Free-form indicator, for the non-ESA answer PIDs.
            emit_comp!(w, "IMD", ["A"]);
        }
        // ── SG1: references ──────────────────────────────────────────────────
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        for (q, v) in &self.inner.references {
            emit_comp!(w, "RFF", [q, v]);
        }
        // ── SG2: adjustment (Prüfschritt + EBD) + coded reason ───────────────
        if let Some(code) = &self.inner.adjustment {
            if let Some(ebd) = &self.inner.adjustment_ebd {
                emit_comp!(w, "AJT", [code], [ebd]);
            } else {
                emit_comp!(w, "AJT", [code]);
            }
        }
        if let Some(code) = &self.inner.adjustment_reason {
            emit_comp!(w, "FTX", [code]);
        }
        // ── SG3: parties ─────────────────────────────────────────────────────
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
        // ── SG27: line item ──────────────────────────────────────────────────
        if self.inner.line_item {
            emit_seg!(w, "LIN", "1");
        }
        emit_seg!(w, "UNS", "D");
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

impl OrdrespBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::ordrsp::OrdrespMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::ordrsp::OrdrespMessage, Error> {
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::ordrsp::OrdrespMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            None,
        ))
    }
}
