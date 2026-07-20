//! [`PricatBuilder`] — fluent type-safe builder for PRICAT messages.

use std::marker::PhantomData;

use edifact_rs::Writer;

use crate::AgencyCode;
use crate::{Error, Release};

use super::{Set, Unset, bytes_to_segments};

// ── PRICAT-specific DTM helpers (format 303) ──────────────────────────────────
//
// These return only the DE 2380 datetime value; qualifier and format code are
// separate components at the emit site.

fn midnight_303(date: &str) -> String {
    format!("{date}0000+0000")
}

fn now_303() -> String {
    let now = time::OffsetDateTime::now_utc();
    let (y, mo, d, h, m) = (
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
    );
    format!("{y:04}{mo:02}{d:02}{h:02}{m:02}+0000")
}

// ── PRICAT price-body structs (public, re-exported from mod.rs) ──────────────

/// A single price entry (SG40) within a PRICAT line item.
///
/// Emits `PRI`, optionally `RNG`, and optionally up to two `DTM` validity
/// segments inside SG40.
#[derive(Debug, Clone)]
pub struct PricatPriceEntry {
    /// Price qualifier (DE 5125 in C509), e.g. `"CAL"`.
    pub qualifier: String,
    /// Price amount as a string (DE 5118 in C509), e.g. `"5.00"`.
    pub amount: String,
    /// Currency code (DE 6345 in C509), e.g. `"EUR"`.
    pub currency: String,
    /// Optional lower range boundary for tiered pricing.
    pub range_low: Option<String>,
    /// Optional upper range boundary for tiered pricing.
    pub range_high: Option<String>,
    /// Optional start of the price validity period (format-303, e.g. `"202504010000+0000"`).
    pub valid_from: Option<String>,
    /// Optional end of the price validity period (format-303).
    pub valid_to: Option<String>,
}

impl PricatPriceEntry {
    /// Create a minimal price entry with qualifier, amount, and currency.
    pub fn new(
        qualifier: impl Into<String>,
        amount: impl Into<String>,
        currency: impl Into<String>,
    ) -> Self {
        Self {
            qualifier: qualifier.into(),
            amount: amount.into(),
            currency: currency.into(),
            range_low: None,
            range_high: None,
            valid_from: None,
            valid_to: None,
        }
    }

    /// Add a consumption or quantity range (`RNG+10+{low}:{high}`).
    pub fn range(mut self, low: impl Into<String>, high: impl Into<String>) -> Self {
        self.range_low = Some(low.into());
        self.range_high = Some(high.into());
        self
    }

    /// Set the validity start date (`DTM+163`, format-303 datetime string).
    pub fn valid_from(mut self, dt: impl Into<String>) -> Self {
        self.valid_from = Some(dt.into());
        self
    }

    /// Set the validity end date (`DTM+164`, format-303 datetime string).
    pub fn valid_to(mut self, dt: impl Into<String>) -> Self {
        self.valid_to = Some(dt.into());
        self
    }
}

/// A single line item (SG36) within a PRICAT price group.
///
/// Emits `LIN`, optionally `PIA` and `IMD`, followed by one or more SG40 entries.
#[derive(Debug, Clone)]
pub struct PricatLineItem {
    /// Optional 1-based line position (LIN DE 1082). Auto-assigned when `None`.
    pub position: Option<u32>,
    /// Optional price-schedule identifier (`PIA+{qualifier}+{id}::ZZZ`).
    pub schedule_id: Option<String>,
    /// PIA qualifier (DE 4347). Defaults to `"1"` when `schedule_id` is set.
    pub schedule_id_qualifier: Option<String>,
    /// Optional free-form description (`IMD+F+++:::{text}`).
    pub description: Option<String>,
    /// Price entries (SG40).
    pub prices: Vec<PricatPriceEntry>,
}

impl PricatLineItem {
    /// Create a line item with a single price entry.
    #[must_use]
    pub fn new(price: PricatPriceEntry) -> Self {
        Self {
            position: None,
            schedule_id: None,
            schedule_id_qualifier: None,
            description: None,
            prices: vec![price],
        }
    }

    /// Set the line position (DE 1082). Defaults to auto-assigned 1-based index.
    #[must_use]
    pub fn position(mut self, pos: u32) -> Self {
        self.position = Some(pos);
        self
    }

    /// Set the price-schedule identifier (`PIA` qualifier `"1"`, DE 7140).
    pub fn schedule_id(mut self, id: impl Into<String>) -> Self {
        self.schedule_id = Some(id.into());
        self
    }

    /// Override the PIA qualifier.  Defaults to `"1"`.
    pub fn schedule_id_qualifier(mut self, q: impl Into<String>) -> Self {
        self.schedule_id_qualifier = Some(q.into());
        self
    }

    /// Set a free-form description (IMD free text, DE 7008).
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Append an additional price entry (SG40).
    #[must_use]
    pub fn add_price(mut self, price: PricatPriceEntry) -> Self {
        self.prices.push(price);
        self
    }
}

/// A product group (SG17) containing line items (SG36).
///
/// Emits `PGI` followed by one or more SG36 entries.
#[derive(Debug, Clone)]
pub struct PricatPriceGroup {
    /// Product group type code (PGI DE 5379), e.g. `"Z01"`.
    pub group_type: String,
    /// Line items (SG36).
    pub items: Vec<PricatLineItem>,
}

impl PricatPriceGroup {
    /// Create a price group with a single line item.
    pub fn new(group_type: impl Into<String>, item: PricatLineItem) -> Self {
        Self {
            group_type: group_type.into(),
            items: vec![item],
        }
    }

    /// Append a line item (SG36) to this group.
    #[must_use]
    pub fn add_item(mut self, item: PricatLineItem) -> Self {
        self.items.push(item);
        self
    }
}

// ── PricatBuilder ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PricatBuilderInner {
    release: Release,
    sender_id: Option<String>,
    receiver_id: Option<String>,
    sender_agency: AgencyCode,
    receiver_agency: AgencyCode,
    message_ref: String,
    document_code: String,
    document_id: Option<String>,
    document_date: Option<String>,
    pruefidentifikator: Option<u32>,
    price_groups: Vec<PricatPriceGroup>,
}

/// Fluent builder for `PRICAT` (Price/Sales Catalogue) messages.
///
/// # Type-state
///
/// [`build`](PricatBuilder::build) is only available once both
/// [`sender`](PricatBuilder::sender) and [`receiver`](PricatBuilder::receiver)
/// have been called.
///
/// **DTM format**: PRICAT uses format 303 (`CCYYMMDDHHMMZZZ`), unlike other
/// EDI@Energy builders which use format 102.
///
/// # Example
///
/// ```rust,no_run
/// use edi_energy::Release;
/// use edi_energy::builders::{PricatBuilder, PricatPriceEntry, PricatLineItem, PricatPriceGroup};
///
/// let price = PricatPriceEntry::new("CAL", "5.50", "EUR")
///     .valid_from("202504010000+0000")
///     .valid_to("202505010000+0000");
/// let line = PricatLineItem::new(price).schedule_id("AEP001");
/// let group = PricatPriceGroup::new("Z01", line);
///
/// let msg = PricatBuilder::new(Release::new("2.0e"))
///     .sender("4012345000023")
///     .receiver("9900357000004")
///     .pruefidentifikator(27001)
///     .document_id("PRICAT-001")
///     .add_price_group(group)
///     .build()?;
///
/// assert_eq!(msg.sender().unwrap().party_id.as_deref(), Some("4012345000023"));
/// # Ok::<(), edi_energy::Error>(())
/// ```
#[derive(Debug, Clone)]
#[must_use = "Builder must be consumed via .build() or .serialize()"]
pub struct PricatBuilder<S = Unset, R = Unset> {
    _ph: PhantomData<fn() -> (S, R)>,
    inner: PricatBuilderInner,
}

impl PricatBuilder<Unset, Unset> {
    /// Create a builder targeting the given EDI@Energy release.
    pub fn new(release: Release) -> Self {
        Self {
            _ph: PhantomData,
            inner: PricatBuilderInner {
                release,
                sender_id: None,
                receiver_id: None,
                sender_agency: AgencyCode::Bdew,
                receiver_agency: AgencyCode::Bdew,
                message_ref: "1".to_owned(),
                document_code: "Z04".to_owned(),
                document_id: None,
                document_date: None,
                pruefidentifikator: None,
                price_groups: Vec::new(),
            },
        }
    }
}

impl<S, R> PricatBuilder<S, R> {
    fn transition<S2, R2>(self) -> PricatBuilder<S2, R2> {
        PricatBuilder {
            _ph: PhantomData,
            inner: self.inner,
        }
    }

    /// Set the message sender's market-participant identifier.
    pub fn sender(mut self, id: impl Into<String>) -> PricatBuilder<Set, R> {
        self.inner.sender_id = Some(id.into());
        self.transition()
    }

    /// Set the message recipient's market-participant identifier.
    pub fn receiver(mut self, id: impl Into<String>) -> PricatBuilder<S, Set> {
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

    /// Set the BGM document identifier.
    pub fn document_id(mut self, id: impl Into<String>) -> Self {
        self.inner.document_id = Some(id.into());
        self
    }

    /// Override the BGM document type code.  Defaults to `"Z04"`.
    pub fn document_code(mut self, code: impl Into<String>) -> Self {
        self.inner.document_code = code.into();
        self
    }

    /// Override the message reference number.  Defaults to `"1"`.
    pub fn message_ref(mut self, reference: impl Into<String>) -> Self {
        self.inner.message_ref = reference.into();
        self
    }

    /// Set the document date (`YYYYMMDD`) for DTM+137 (emitted in format 303, midnight UTC).
    pub fn document_date(mut self, date: impl Into<String>) -> Self {
        self.inner.document_date = Some(date.into());
        self
    }

    /// Set a full format-303 datetime (`CCYYMMDDHHMMZZZ`) for DTM+137.
    pub fn document_datetime(mut self, datetime: impl Into<String>) -> Self {
        self.inner.document_date = Some(format!("__303:{}", datetime.into()));
        self
    }

    /// Set the Prüfidentifikator (emitted as `RFF+Z13:{pid}`).
    pub fn pruefidentifikator(mut self, pid: u32) -> Self {
        self.inner.pruefidentifikator = Some(pid);
        self
    }

    /// Add a price group (SG17 with nested SG36/SG40) to the message body.
    pub fn add_price_group(mut self, group: PricatPriceGroup) -> Self {
        self.inner.price_groups.push(group);
        self
    }

    fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let dtm_val = match self.inner.document_date.as_deref() {
            None => now_303(),
            Some(s) if s.starts_with("__303:") => s["__303:".len()..].to_owned(),
            Some(date) => midnight_303(date),
        };

        let mut buf = Vec::new();
        let mut w = Writer::new(&mut buf);

        let doc_id = self.inner.document_id.as_deref().unwrap_or("");
        emit_comp!(
            w,
            "UNH",
            [&self.inner.message_ref],
            ["PRICAT", "D", "20B", "UN", self.inner.release.as_str()]
        );
        emit_seg!(w, "BGM", &self.inner.document_code, doc_id);
        if let Some(pid) = self.inner.pruefidentifikator {
            emit_comp!(w, "RFF", ["Z13", &pid.to_string()]);
        }
        emit_comp!(w, "DTM", ["137", &dtm_val, "303"]);
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
        for (group_idx, group) in self.inner.price_groups.iter().enumerate() {
            emit_seg!(w, "PGI", &group.group_type);
            for (item_idx, item) in group.items.iter().enumerate() {
                #[expect(clippy::cast_possible_truncation)]
                let pos = item
                    .position
                    .unwrap_or((group_idx * 1000 + item_idx + 1) as u32);
                emit_seg!(w, "LIN", &pos.to_string());
                if let Some(id) = &item.schedule_id {
                    let q = item.schedule_id_qualifier.as_deref().unwrap_or("1");
                    emit_comp!(w, "PIA", [q], [id, "", "ZZZ"]);
                }
                if let Some(desc) = &item.description {
                    emit_comp!(w, "IMD", ["F"], [""], ["", "", "", desc]);
                }
                for price in &item.prices {
                    emit_comp!(w, "PRI", [&price.qualifier, &price.amount, &price.currency]);
                    if let (Some(low), Some(high)) = (&price.range_low, &price.range_high) {
                        emit_comp!(w, "RNG", ["10"], [low, high]);
                    }
                    if let Some(from) = &price.valid_from {
                        emit_comp!(w, "DTM", ["163", from, "303"]);
                    }
                    if let Some(to) = &price.valid_to {
                        emit_comp!(w, "DTM", ["164", to, "303"]);
                    }
                }
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

impl PricatBuilder<Set, Set> {
    /// Build and return a fully-parsed [`crate::messages::pricat::PricatMessage`].
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if EDIFACT serialization or parsing fails.
    pub fn build(self) -> Result<crate::messages::pricat::PricatMessage, Error> {
        let pid = self.inner.pruefidentifikator;
        let message_ref = self.inner.message_ref.clone();
        let assoc_code = self.inner.release.as_str().to_owned();
        let segments = bytes_to_segments(&self.to_bytes()?)?;
        Ok(crate::messages::pricat::PricatMessage::from_parts(
            segments,
            message_ref.as_str(),
            assoc_code.as_str(),
            pid,
        ))
    }
}
