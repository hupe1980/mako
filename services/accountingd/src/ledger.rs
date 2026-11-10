//! accountingd's accounting/storage base — a [`doubleentry`] double-entry ledger.
//!
//! This module is the seam between accountingd's domain (energy-market
//! Massenkontokorrent, SKR 03/04, EEG payouts, dunning) and the domain-neutral
//! [`doubleentry`] kernel. accountingd owns the **chart of accounts** and the
//! **`entry_type` → postings** mapping; `doubleentry` owns everything a ledger
//! must guarantee: balances, immutability, an append-only RFC 6962 Merkle log
//! with inclusion and consistency proofs, period seals (GoBD Unveränderbarkeit /
//! § 146 AO), open-item clearing, and idempotent writes.
//!
//! ## The model
//!
//! Every money movement is one balanced entry with exactly two legs:
//!
//! - the customer/counterparty **Kontokorrent** — one leaf account per
//!   Marktlokation (`Kontokorrent:<lf_mp>:<malo>`, [`AccountKind::Asset`]). Its
//!   signed net *is* `accounts.balance_ct`: a debit balance means the
//!   customer owes, a credit balance means they are owed (EEG operators, refunds).
//! - a **general-ledger** contra account, one per `entry_type`
//!   ([`Chart`]) — Erlöse, Bank, Mahnerlöse, EEG-Aufwand.
//!
//! The Kontokorrent posting carries two dimensions — `korrelation` (the
//! originating CloudEvent) and `akteur` (who booked it); the entry type is the
//! entry's kind, so a per-type sum is one `balance` query `of_kind`.
//!
//! ## Writes go through booking drafts
//!
//! [`Ledger::post`] is called only by [`crate::booking`], which stores the
//! whole draft in the business transaction first; `cargo xtask
//! check-ledger-drafts` holds every other module to that.

use time::Date;

use doubleentry::currency::EUR;
use doubleentry::error::QueryError;
use doubleentry::store::{Postgres, SqlOptions, SqlStore, Store};
use doubleentry::{
    AccountKind, Amount, BalanceQuery, Document, Entry, EntryId, Error, Layer, LedgerId, Posting,
    Side,
};

/// EUR at 2-dp minor units (cents) — accountingd's only currency.
pub type Eur = Amount<2>;

/// The precision every accountingd account uses.
pub const P: u8 = 2;

/// The PostgreSQL schema the ledger's tables live in, beside accountingd's own
/// tables in `public`.
///
/// Not doubleentry's default `doubleentry`, the schema a 0.7 ledger used: a
/// leftover one is never read, and doubleentry refuses any schema holding
/// foreign tables by name before its DDL runs.
pub const LEDGER_SCHEMA: &str = "ledger";

/// The first day the ledger accepts a booking on.
///
/// Every booking date must fall in a defined period, and periods are
/// contiguous, so the calendar starts here and runs in calendar months through
/// next year. A booking dated earlier is refused (`300`) and parks.
pub const BUCHUNGSBEGINN: Date = time::macros::date!(2015 - 01 - 01);

/// The production ledger — a doubleentry PostgreSQL store for one deployment.
pub type PgLedger = Ledger<SqlStore<Postgres, P>>;

// ── Errors ─────────────────────────────────────────────────────────────────────

/// What the ledger answers when it does not do what it was asked.
///
/// The variants are the distinction a caller has to act on. A refusal and an
/// unknown subject are answers about the request; `Unavailable` is a failure
/// worth retrying with the same draft; an integrity or storage failure is this
/// service's fault and carries no detail a caller can correct.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The ledger refused the request for what it asked. `code` is
    /// doubleentry's stable rejection code (`301` a sealed period, `200` a
    /// reused key with other content); `None` for a refusal accountingd makes
    /// itself before the ledger is asked.
    #[error("{reason}")]
    Refused {
        /// doubleentry's rejection code, when the ledger refused.
        code: Option<u16>,
        /// The refusal, in words.
        reason: String,
    },
    /// What the request names is not in the books — an unknown entry, a
    /// period that was never sealed.
    #[error("{0}")]
    NotFound(String),
    /// The store failed transiently, or may have applied the write: the same
    /// draft is retried.
    #[error("ledger unavailable: {context}")]
    Unavailable {
        /// The operation the failure interrupted.
        context: String,
        /// The store's own error, kept for the log.
        #[source]
        source: Box<Error>,
    },
    /// The stored books do not verify.
    #[error("ledger integrity: {0}")]
    Integrity(String),
    /// The store underneath failed for good — configuration, a permanent
    /// backend error.
    #[error("ledger store failed: {context}")]
    Storage {
        /// The operation the failure interrupted.
        context: String,
        /// The store's own error, kept for the log.
        #[source]
        source: Box<Error>,
    },
}

/// The ledger's result type.
pub type LedgerResult<T> = Result<T, LedgerError>;

impl LedgerError {
    /// A refusal accountingd makes before the ledger is asked.
    fn refused(reason: impl Into<String>) -> Self {
        Self::Refused {
            code: None,
            reason: reason.into(),
        }
    }

    /// Classify a doubleentry error by what it says about the request.
    fn from_ledger(context: impl Into<String>) -> impl FnOnce(Error) -> Self {
        let context = context.into();
        move |e| match &e {
            Error::Rejected(r) | Error::BatchRejected { rejection: r, .. } => Self::Refused {
                code: Some(r.code()),
                reason: format!("{context}: {r}"),
            },
            Error::Query(
                QueryError::UnknownAccount { .. }
                | QueryError::UnknownEntry { .. }
                | QueryError::UnknownPeriod { .. }
                | QueryError::NotSealed { .. },
            ) => Self::NotFound(format!("{context}: {e}")),
            Error::Query(_) | Error::Value(_) | Error::Amount(_) => Self::Refused {
                code: None,
                reason: format!("{context}: {e}"),
            },
            Error::Integrity(_) => Self::Integrity(format!("{context}: {e}")),
            _ if e.is_transient() || e.may_have_applied() => Self::Unavailable {
                context,
                source: Box::new(e),
            },
            _ => Self::Storage {
                context,
                source: Box::new(e),
            },
        }
    }

    /// Whether the same draft may succeed when retried.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Unavailable { .. })
    }

    /// doubleentry's rejection code, when the ledger refused.
    #[must_use]
    pub fn code(&self) -> Option<u16> {
        match self {
            Self::Refused { code, .. } => *code,
            _ => None,
        }
    }

    /// The answer a handler gives for this failure.
    ///
    /// A refusal is `422` with code `ledger.<n>` (`409` for `200`, a reused
    /// key with other content); an unknown subject `404`; a retryable failure
    /// `503`; an integrity or storage failure a `500` whose `error_id` the log
    /// line carries, with the store's text kept out of the body.
    #[must_use]
    pub fn to_api(&self) -> mako_service::ApiError {
        use mako_service::ApiError;
        match self {
            Self::Refused {
                code: Some(200),
                reason,
            } => ApiError::conflict(reason.clone()).with_code("ledger.200"),
            Self::Refused {
                code: Some(code),
                reason,
            } => ApiError::unprocessable(reason.clone()).with_code(format!("ledger.{code}")),
            Self::Refused { code: None, reason } => ApiError::unprocessable(reason.clone()),
            Self::NotFound(msg) => ApiError::not_found(msg.clone()),
            Self::Unavailable { .. } => {
                ApiError::unavailable("the ledger is unavailable; retry the request")
            }
            Self::Integrity(msg) => ApiError::internal(anyhow::anyhow!("ledger integrity: {msg}")),
            Self::Storage { context, .. } => {
                ApiError::internal(anyhow::anyhow!("ledger store failed: {context}"))
            }
        }
    }
}

impl From<LedgerError> for mako_service::ApiError {
    fn from(e: LedgerError) -> Self {
        e.to_api()
    }
}

// ── IBAN lookup hash (app-layer, keyed BLAKE3 — replaces the pgcrypto digest) ──

/// Derives the 32-byte IBAN-hash key from a deployment secret.
///
/// A keyed hash — not plain SHA-256 — because the IBAN keyspace is small enough
/// to enumerate offline from an unkeyed digest. `derive_key` domain-separates the
/// secret so the same secret used elsewhere yields a different key here.
#[must_use]
pub fn iban_hash_key(secret: &str) -> [u8; 32] {
    blake3::derive_key("accountingd IBAN lookup hash v1", secret.as_bytes())
}

/// The lookup hash of an IBAN: keyed BLAKE3 over the normalised form (uppercase,
/// no spaces), hex-encoded. Stable for a given key, so it indexes CAMT.054
/// matching even when the stored IBAN is ciphertext.
///
/// `key` is `None` only in dev deployments with no secret configured, where it
/// falls back to an unkeyed hash (enumerable — a startup warning is emitted).
#[must_use]
pub fn iban_hash(key: Option<&[u8; 32]>, iban: &str) -> String {
    let normalised = iban.replace(' ', "").to_uppercase();
    let digest = match key {
        Some(key) => blake3::keyed_hash(key, normalised.as_bytes()),
        None => blake3::hash(normalised.as_bytes()),
    };
    digest.to_hex().to_string()
}

// ── Chart of accounts ──────────────────────────────────────────────────────────

/// The fixed general-ledger contra accounts, opened at boot.
///
/// These are the non-customer side of every entry. The customer side is a
/// per-Marktlokation Kontokorrent opened on its first booking.
///
/// # No `BalanceLimit` on any of them
///
/// doubleentry can pin an account to one side of the ledger and reject in the
/// write any entry that would flip it. No account here takes one, and the
/// reason differs per account rather than being an oversight:
///
/// - **Kontokorrent** is bidirectional by design — a credit balance is how an
///   EEG operator or an overpaying customer is represented.
/// - **Bank** looks like it qualifies (a chargeback always follows a
///   collection), but only in steady state. The ledger starts empty at
///   cut-over, so a `BANKRUECKLAST` for a collection made in the legacy system
///   would be the first movement on the account and a limit would make a
///   legitimate inbound bank event permanently unbookable.
/// - **Erloese / Mahnerloese / EEG-Aufwand** are each one side of a partial
///   view; a STORNO of a pre-migration invoice breaches them for the same
///   reason.
/// - **Erstattungen** is only ever credited — nothing discharges it here — so a
///   limit would be true and could never fire.
/// - **Anzahlungen** is credited by every Abschlagsforderung and debited by the
///   Verrechnung on the settling invoice, so it swings in both directions
///   whenever a Jahresrechnung lands out of order with the advances it settles.
pub struct Chart;

impl Chart {
    /// SKR 1200 — Bankguthaben. Debited by incoming payments, credited by chargebacks.
    pub const BANK: &'static str = "Aktiva:Bank";
    /// SKR 4000 — Energieerlöse. The revenue contra for invoices and settlements.
    pub const ERLOESE: &'static str = "Ertrag:Energieerloese";
    /// SKR 4003 — Mahngebühren. Fees charged for the dunning process itself.
    pub const MAHNERLOESE: &'static str = "Ertrag:Mahnerloese";
    /// SKR 2650 — Zinserträge. **Not** the same account as Mahngebühren:
    /// § 275 HGB puts *Zinsen und ähnliche Erträge* in their own P&L line.
    pub const ZINSERTRAG: &'static str = "Ertrag:Zinsertraege";
    /// The § 288 Abs. 5 BGB Pauschale, net of its Satz-3 Anrechnung. Its own
    /// account: § 275 Abs. 2 HGB shows Umsatzerlöse (Nr. 1) and sonstige
    /// Zinsen und ähnliche Erträge (Nr. 11) as separate lines, and the
    /// Pauschale is neither — not consideration for a supply and not interest.
    pub const VERZUGSPAUSCHALEN: &'static str = "Ertrag:Verzugspauschalen";
    /// EEG feed-in expense (Aufwand) — the contra for EEG Gutschrift / Marktprämie.
    pub const EEG_AUFWAND: &'static str = "Aufwand:EEG-Einspeiseverguetung";
    /// Refund payable — the contra when a Jahresabschluss books a customer refund.
    pub const ERSTATTUNG: &'static str = "Passiva:Verbindlichkeiten:Erstattungen";
    /// SKR 03 1718 / SKR 04 3272 — **Erhaltene Anzahlungen**, the contra for the
    /// Abschlag pair (§ 40 Abs. 1 EnWG, § 14 Abs. 5 UStG): neither revenue nor
    /// cash, and never `BANK`.
    pub const ANZAHLUNGEN: &'static str = "Passiva:Verbindlichkeiten:ErhalteneAnzahlungen";
    /// The grouping node every Kontokorrent sits under.
    pub const KONTOKORRENT: &'static str = "Kontokorrent";

    /// Every GL account with its kind, opened at boot.
    pub const GL: [(&'static str, AccountKind); 8] = [
        (Self::BANK, AccountKind::Asset),
        (Self::ERLOESE, AccountKind::Income),
        (Self::MAHNERLOESE, AccountKind::Income),
        (Self::ZINSERTRAG, AccountKind::Income),
        (Self::VERZUGSPAUSCHALEN, AccountKind::Income),
        (Self::EEG_AUFWAND, AccountKind::Expense),
        (Self::ERSTATTUNG, AccountKind::Liability),
        (Self::ANZAHLUNGEN, AccountKind::Liability),
    ];

    /// The GL contra account for an `entry_type`, or `None` for one outside
    /// [`ENTRY_TYPES`].
    ///
    /// The customer Kontokorrent is always the other leg; its direction follows
    /// the sign of `amount_ct` (positive → debit customer, negative → credit),
    /// which is what makes the Kontokorrent net equal `balance_ct`.
    #[must_use]
    pub fn contra(entry_type: &str) -> Option<&'static str> {
        Some(match entry_type {
            // SEPA_STORNO is a pain.007 reversal: the creditor hands a settled
            // collection back, so the money leaves the bank account again and
            // the receivable re-opens.
            "ZAHLUNG" | "BANKRUECKLAST" | "SEPA_STORNO" => Self::BANK,
            // An EEG payout the bank settled, and an operator's repayment of an
            // overpaid one: money leaving, and returning to, the bank account.
            "EEG_AUSZAHLUNG" | "EEG_RUECKZAHLUNG" => Self::BANK,
            "ABSCHLAG" | "ABSCHLAG_VERRECHNUNG" => Self::ANZAHLUNGEN,
            "MAHNGEBUEHR" => Self::MAHNERLOESE,
            "VERZUGSZINSEN" => Self::ZINSERTRAG,
            "VERZUGSPAUSCHALE" => Self::VERZUGSPAUSCHALEN,
            "EEG_GUTSCHRIFT" | "EEG_MARKTPRAEMIE" => Self::EEG_AUFWAND,
            "JAHRESABSCHLUSS" => Self::ERSTATTUNG,
            "RECHNUNG" | "STORNO" | "GUTSCHRIFT" | "KORREKTUR" => Self::ERLOESE,
            _ => return None,
        })
    }

    /// The Kontokorrent path of a Marktlokation.
    #[must_use]
    pub fn kontokorrent(lf_mp_id: &str, malo_id: &str) -> String {
        format!("{}:{lf_mp_id}:{malo_id}", Self::KONTOKORRENT)
    }
}

/// Every Buchungsart accountingd books.
///
/// [`Chart::contra`] maps each to a GL account and refuses anything outside
/// the list — one list, so a kind cannot be bookable by an automated path and
/// rejected by the operator interface.
pub const ENTRY_TYPES: &[&str] = &[
    "RECHNUNG",
    "STORNO",
    "KORREKTUR",
    "GUTSCHRIFT",
    "ABSCHLAG",
    "ABSCHLAG_VERRECHNUNG",
    "ZAHLUNG",
    "BANKRUECKLAST",
    "SEPA_STORNO",
    "MAHNGEBUEHR",
    "VERZUGSZINSEN",
    "VERZUGSPAUSCHALE",
    "EEG_GUTSCHRIFT",
    "EEG_MARKTPRAEMIE",
    "EEG_AUSZAHLUNG",
    "EEG_RUECKZAHLUNG",
    "JAHRESABSCHLUSS",
];

/// The dimension axes on the Kontokorrent posting.
pub mod axis {
    /// The originating CloudEvent id.
    pub const KORRELATION: &str = "korrelation";
    /// Who booked it (an operator's subject, a worker's name).
    pub const AKTEUR: &str = "akteur";
}

// ── Requests and answers ───────────────────────────────────────────────────────

/// A request to post one balanced entry — the whole booking draft.
///
/// Serialisable because [`crate::booking`] stores it verbatim beside the
/// business change and rebuilds the identical entry from it: a draft
/// recomputed from mutable state would turn a retry into a key conflict.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostEntry {
    /// Marktlokation — identifies the customer Kontokorrent.
    pub malo_id: String,
    /// The LF's own market-partner id — namespaces the Kontokorrent path.
    pub lf_mp_id: String,
    /// The accountingd entry type (`RECHNUNG`, `ZAHLUNG`, `EEG_GUTSCHRIFT`, …).
    pub entry_type: String,
    /// Signed minor units: positive increases the receivable, negative reduces it.
    pub amount_ct: i64,
    /// The idempotency key, ≤ 128 bytes — a CloudEvent id, a bank transaction
    /// id, or a deterministic string (`mahngebuehr:{case}:{stufe}`).
    pub idempotency: String,
    /// Booking date (Buchungsdatum).
    pub booking_date: Date,
    /// Value date (Wertstellung); defaults to `booking_date`.
    pub value_date: Date,
    /// Free-text line description, kept as the entry's erasable memo.
    pub description: Option<String>,
    /// External correlation (a CloudEvent id).
    pub correlation: Option<String>,
    /// Source-document identifier (invoice/record id), ≤ 256 bytes.
    pub document: Option<String>,
    /// The acting principal (operator sub for manual bookings).
    pub actor: Option<String>,
}

impl PostEntry {
    /// A minimal post: booking date doubles as value date, no metadata.
    #[must_use]
    pub fn new(
        malo_id: impl Into<String>,
        lf_mp_id: impl Into<String>,
        entry_type: impl Into<String>,
        amount_ct: i64,
        idempotency: impl Into<String>,
        booking_date: Date,
    ) -> Self {
        Self {
            malo_id: malo_id.into(),
            lf_mp_id: lf_mp_id.into(),
            entry_type: entry_type.into(),
            amount_ct,
            idempotency: idempotency.into(),
            booking_date,
            value_date: booking_date,
            description: None,
            correlation: None,
            document: None,
            actor: None,
        }
    }

    /// Sets the value date (for backdated corrections).
    #[must_use]
    pub fn with_value_date(mut self, value_date: Date) -> Self {
        self.value_date = value_date;
        self
    }

    /// Sets the line description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the CloudEvent correlation id.
    #[must_use]
    pub fn with_correlation(mut self, correlation: impl Into<String>) -> Self {
        self.correlation = Some(correlation.into());
        self
    }

    /// Sets the source-document identifier (invoice/record id).
    #[must_use]
    pub fn with_document(mut self, document: impl Into<String>) -> Self {
        self.document = Some(document.into());
        self
    }

    /// Sets the acting principal.
    #[must_use]
    pub fn with_actor(mut self, actor: impl Into<String>) -> Self {
        self.actor = Some(actor.into());
        self
    }

    /// Refuses a draft the ledger could never take — an unknown entry type, a
    /// zero amount, a field doubleentry would refuse — before anything is
    /// written.
    ///
    /// # Errors
    ///
    /// The refusal, as [`LedgerError::Refused`] without a code.
    pub fn validate(&self) -> LedgerResult<()> {
        self.entry().map(|_| ())
    }

    /// The doubleentry entry this draft books, or the refusal accountingd makes
    /// before asking the ledger.
    fn entry(&self) -> LedgerResult<Entry<P>> {
        let Some(contra) = Chart::contra(&self.entry_type) else {
            return Err(LedgerError::refused(format!(
                "unknown entry type {:?}; bookable are {}",
                self.entry_type,
                ENTRY_TYPES.join(", ")
            )));
        };
        if self.amount_ct == 0 {
            return Err(LedgerError::refused(format!(
                "refusing to post a zero-amount entry ({})",
                self.entry_type
            )));
        }
        let amount = Eur::from_minor(i128::from(self.amount_ct.unsigned_abs()))
            .map_err(|e| LedgerError::refused(format!("amount: {e}")))?;
        let customer_side = if self.amount_ct > 0 {
            Side::Debit
        } else {
            Side::Credit
        };
        let kontokorrent = Chart::kontokorrent(&self.lf_mp_id, &self.malo_id);
        let mut customer = match customer_side {
            Side::Debit => Posting::debit(kontokorrent.as_str(), amount, EUR),
            Side::Credit => Posting::credit(kontokorrent.as_str(), amount, EUR),
        };
        if let Some(correlation) = &self.correlation {
            customer = customer.dimension(axis::KORRELATION, correlation.as_str());
        }
        if let Some(actor) = &self.actor {
            customer = customer.dimension(axis::AKTEUR, actor.as_str());
        }
        let gl = match customer_side {
            Side::Debit => Posting::credit(contra, amount, EUR),
            Side::Credit => Posting::debit(contra, amount, EUR),
        };
        let mut entry = Entry::new(self.idempotency.as_str(), self.booking_date)
            .value_date(self.value_date)
            .kind(self.entry_type.as_str())
            .posting(customer)
            .posting(gl);
        if let Some(document) = &self.document {
            entry = entry.document(
                Document::new("beleg", document.as_str())
                    .map_err(|e| LedgerError::refused(format!("document reference: {e}")))?,
            );
        }
        if let Some(description) = &self.description {
            entry = entry.memo(description.as_str());
        }
        if let Some(first) = entry.build_errors().first() {
            return Err(LedgerError::refused(format!("booking draft: {first}")));
        }
        Ok(entry)
    }
}

/// An entry id as the `Uuid` accountingd's tables and routes carry.
#[must_use]
pub fn entry_uuid(id: EntryId) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*id.as_bytes())
}

/// The entry id behind a `Uuid` from accountingd's tables or routes.
#[must_use]
pub fn entry_id(uuid: uuid::Uuid) -> EntryId {
    EntryId::from_bytes(*uuid.as_bytes())
}

/// One open receivable (unpaid invoice/fee) after recorded clearings — the
/// authoritative Offene-Posten line (see [`Ledger::open_receivables`]).
#[derive(Debug, Clone, serde::Serialize)]
pub struct OpenReceivable {
    /// The doubleentry entry id of the open debit.
    pub entry_id: uuid::Uuid,
    /// Buchungsart (doubleentry `kind`), e.g. `RECHNUNG`, `MAHNGEBUEHR`.
    pub entry_type: Option<String>,
    /// The original billed amount in minor units.
    pub amount_ct: i64,
    /// The still-open portion in minor units (≤ `amount_ct`).
    pub outstanding_ct: i64,
    /// Booking date of the original debit.
    pub booking_date: Date,
    /// The source document the entry cites — an invoice's Rechnungsnummer, an
    /// advance's demand reference. The item's customer-facing name: a Mahnung
    /// listing an entry UUID demands something the recipient cannot match to
    /// anything they hold.
    pub document: Option<String>,
}

/// One line of the Summen- und Saldenliste (trial balance / SuSa).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrialBalanceLine {
    /// The account path (per-MaLo Kontokorrent leaves are aggregated into one line).
    pub account: String,
    /// Gross debit turnover (Soll) in minor units.
    pub debits_ct: i64,
    /// Gross credit turnover (Haben) in minor units.
    pub credits_ct: i64,
    /// Balance (Saldo) = debits − credits, in minor units.
    pub net_ct: i64,
}

/// The result of a post.
#[derive(Debug, Clone, Copy)]
pub struct Posted {
    /// The entry identifier (derived from the key, stable across replays).
    pub id: EntryId,
    /// `false` when an identical entry had already been recorded — a safe retry.
    pub is_new: bool,
}

/// One sealed period (Festschreibung).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SealedPeriod {
    /// The period id, `YYYY-MM`.
    pub period: String,
    /// The seal record's position in the log.
    pub position: u64,
    /// The period's first day.
    pub start: Date,
    /// The period's last day.
    pub end: Date,
}

impl From<&doubleentry::SealReceipt<P>> for SealedPeriod {
    fn from(r: &doubleentry::SealReceipt<P>) -> Self {
        Self {
            period: r.period.as_str().to_owned(),
            position: r.position,
            start: r.start(),
            end: r.end(),
        }
    }
}

/// Minor units as `i64`, the width accountingd's tables carry.
fn ct(amount: Eur) -> i64 {
    i64::try_from(amount.to_minor()).unwrap_or(if amount.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

// ── The ledger ─────────────────────────────────────────────────────────────────

/// A [`doubleentry`] ledger scoped to one accountingd deployment (one `tenant`).
///
/// Generic over the store so unit tests run against `MemoryStore` and
/// production against PostgreSQL, both held to doubleentry's conformance suite.
pub struct Ledger<S: Store<P>> {
    books: doubleentry::Ledger<S, P>,
    /// A clone of the store (cheap, pool-sharing), for the grants an owner
    /// hands the runtime role.
    store: S,
}

impl<S: Store<P>> Ledger<S> {
    /// Opens the ledger over `store` — creating it when the store holds none —
    /// and makes sure EUR, the calendar and the GL chart exist.
    ///
    /// Every step is idempotent: an identical repeat writes nothing.
    ///
    /// # Errors
    ///
    /// A store or configuration failure (another ledger id in the store).
    pub async fn open(store: S, tenant: &str, today: Date) -> LedgerResult<Self> {
        let id = LedgerId::new(tenant)
            .map_err(|e| LedgerError::refused(format!("ledger id {tenant}: {e}")))?;
        let books = doubleentry::Ledger::new(store.clone(), id)
            .await
            .map_err(LedgerError::from_ledger("open ledger"))?
            .as_actor("accountingd");
        let ledger = Self { books, store };
        ledger
            .books
            .declare_currency(EUR, P, BUCHUNGSBEGINN)
            .await
            .map_err(LedgerError::from_ledger("declare EUR"))?;
        ledger.define_through(today.year() + 1).await?;
        for (path, kind) in Chart::GL {
            ledger
                .books
                .open(path, kind, BUCHUNGSBEGINN)
                .await
                .map_err(LedgerError::from_ledger(format!("open {path}")))?;
        }
        Ok(ledger)
    }

    /// Defines every calendar month from [`BUCHUNGSBEGINN`] through December of
    /// `year`; months already defined are skipped.
    async fn define_through(&self, year: i32) -> LedgerResult<()> {
        for y in BUCHUNGSBEGINN.year()..=year {
            self.books
                .define_months(y, 1..=12)
                .await
                .map_err(LedgerError::from_ledger(format!("define {y}")))?;
        }
        Ok(())
    }

    /// The Kontokorrent path of a Marktlokation, refused when either id would
    /// leave an empty or invalid path segment.
    fn konto(lf_mp_id: &str, malo_id: &str) -> LedgerResult<String> {
        let path = Chart::kontokorrent(lf_mp_id, malo_id);
        path.parse::<doubleentry::AccountPath>()
            .map_err(|e| LedgerError::refused(format!("Kontokorrent {path:?}: {e}")))?;
        Ok(path)
    }

    /// The id the entry under `key` has — derived from the ledger id and the
    /// key, so it is known before the entry is booked and a business row
    /// committed with the draft can name it.
    ///
    /// # Errors
    ///
    /// A key doubleentry refuses (empty, over 128 bytes, a control character).
    pub fn entry_id_for(&self, key: &str) -> LedgerResult<uuid::Uuid> {
        self.books
            .entry_id(key)
            .map(entry_uuid)
            .map_err(|e| LedgerError::refused(format!("idempotency key {key:?}: {e}")))
    }

    /// The doubleentry ledger, for reads this façade does not wrap.
    #[must_use]
    pub fn books(&self) -> &doubleentry::Ledger<S, P> {
        &self.books
    }

    /// Opens the Kontokorrent of a Marktlokation unless it exists.
    ///
    /// `open` repeated with another date is refused (`420`), so the lookup comes
    /// first; a new account opens on the later of [`BUCHUNGSBEGINN`] and the day
    /// after the last seal, because an account may not open inside a sealed
    /// period (`309`).
    async fn ensure_kontokorrent(&self, path: &str) -> LedgerResult<()> {
        if self
            .books
            .account(path)
            .await
            .map_err(LedgerError::from_ledger(format!("look up {path}")))?
            .is_some()
        {
            return Ok(());
        }
        let open_from = match self.sealed_through().await? {
            Some(end) => end.next_day().unwrap_or(end).max(BUCHUNGSBEGINN),
            None => BUCHUNGSBEGINN,
        };
        self.books
            .open(path, AccountKind::Asset, open_from)
            .await
            .map_err(LedgerError::from_ledger(format!("open {path}")))?;
        Ok(())
    }

    /// Posts one balanced entry.
    ///
    /// Idempotent by `req.idempotency`: an identical replay returns the original
    /// entry; a reused key with different content is refused (`200`). Call it
    /// only through [`crate::booking`].
    ///
    /// # Errors
    ///
    /// A refusal (unknown entry type, zero amount, a sealed or undefined
    /// period, a key conflict) or a store failure.
    pub async fn post(&self, req: &PostEntry) -> LedgerResult<Posted> {
        let entry = req.entry()?;
        if req.booking_date.year() > Date::MAX.year() - 1 {
            return Err(LedgerError::refused("booking date out of range"));
        }
        // A booking into next year's calendar extends it; the calendar is
        // contiguous, so every year between is defined too.
        self.define_through(req.booking_date.year()).await?;
        self.ensure_kontokorrent(&Self::konto(&req.lf_mp_id, &req.malo_id)?)
            .await?;
        let recorded = self
            .books
            .record(entry)
            .await
            .map_err(LedgerError::from_ledger(format!(
                "book {}",
                req.idempotency
            )))?;
        Ok(Posted {
            id: recorded.id,
            is_new: !recorded.replayed,
        })
    }

    /// The customer's balance in minor units: positive = owes, negative = is owed.
    ///
    /// Zero for a Marktlokation that has never been booked.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn balance_ct(&self, lf_mp_id: &str, malo_id: &str) -> LedgerResult<i64> {
        self.balance(lf_mp_id, malo_id, &BalanceQuery::all()).await
    }

    /// The customer's signed balance over `query`; zero for an account never booked.
    async fn balance(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        query: &BalanceQuery,
    ) -> LedgerResult<i64> {
        match self
            .books
            .balance(Self::konto(lf_mp_id, malo_id)?.as_str(), EUR, query)
            .await
        {
            Ok(balance) => Ok(ct(balance.net())),
            Err(Error::Query(QueryError::UnknownAccount { .. })) => Ok(0),
            Err(e) => Err(LedgerError::from_ledger("read balance")(e)),
        }
    }

    /// The customer's open items — postings with an outstanding residual, in
    /// log order; empty for an account never booked.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn open_items(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
    ) -> LedgerResult<Vec<doubleentry::report::OpenItem<P>>> {
        match self
            .books
            .open_items(
                Self::konto(lf_mp_id, malo_id)?.as_str(),
                EUR,
                &BalanceQuery::all(),
            )
            .await
        {
            Ok(items) => Ok(items),
            Err(Error::Query(QueryError::UnknownAccount { .. })) => Ok(Vec::new()),
            Err(e) => Err(LedgerError::from_ledger("open items")(e)),
        }
    }

    /// Records a **FIFO Zahlungszuordnung**: matches the customer's open credit
    /// residuals (payments, Abschläge, Gutschriften) against their oldest open
    /// debit residuals (invoices, fees), oldest first by booking date and log
    /// position, one clearing per matched pair.
    ///
    /// Returns the clearing positions — empty when nothing is left to match, so
    /// a re-run is a safe no-op. Each pair's key carries the log size the run
    /// started from, so a run after a reset clears anew rather than replaying
    /// the reset clearing. This is the § 252 HGB per-receivable matching a
    /// running balance alone cannot express. The pairs are cleared in one
    /// transaction.
    ///
    /// # Errors
    ///
    /// A refusal or a store failure.
    pub async fn apply_fifo_clearing(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        on: Date,
    ) -> LedgerResult<Vec<u64>> {
        let open = self.open_items(lf_mp_id, malo_id).await?;
        let order = |i: &&doubleentry::report::OpenItem<P>| (i.booking_date, i.position);
        let mut debits: Vec<_> = open
            .iter()
            .filter(|i| i.side == Side::Debit && i.residual.is_positive())
            .collect();
        let mut credits: Vec<_> = open
            .iter()
            .filter(|i| i.side == Side::Credit && i.residual.is_positive())
            .collect();
        debits.sort_by_key(order);
        credits.sort_by_key(order);
        let head = self
            .books
            .head()
            .await
            .map_err(LedgerError::from_ledger("head"))?
            .size;

        let mut pairs = Vec::new();
        let mut credits = credits.into_iter().map(|c| (c, c.residual.to_minor()));
        let mut current = credits.next();
        'debit: for debit in debits {
            let mut need = debit.residual.to_minor();
            while need > 0 {
                let Some((credit, left)) = current.as_mut() else {
                    break 'debit;
                };
                if *left == 0 {
                    current = credits.next();
                    continue;
                }
                let take = need.min(*left);
                let key = format!("fifo:{}:{}:{head}", debit.position, credit.position);
                let amount = Eur::from_minor(take)
                    .map_err(|e| LedgerError::refused(format!("clearing amount: {e}")))?;
                pairs.push(doubleentry::Clearing::new(
                    key,
                    debit.posting_ref(),
                    credit.posting_ref(),
                    amount,
                    on,
                ));
                need -= take;
                *left -= take;
            }
        }
        if pairs.is_empty() {
            return Ok(Vec::new());
        }
        // One transaction for the whole Zahlungszuordnung: a payment matched
        // against three invoices is cleared against all three or none.
        let cleared = self
            .books
            .clear_batch(pairs)
            .await
            .map_err(LedgerError::from_ledger("clear the Zahlungszuordnung"))?;
        Ok(cleared.iter().map(|c| c.position).collect())
    }

    /// Releases one clearing (a mis-assigned Zahlungszuordnung) — its amount
    /// returns to both postings' residuals. The clearing record stays: an
    /// assignment made and withdrawn is itself part of the trail.
    ///
    /// # Errors
    ///
    /// A refusal (unknown or already reset clearing) or a store failure.
    pub async fn reset_clearing(&self, position: u64, on: Date) -> LedgerResult<()> {
        self.books
            .reset_clearing(&format!("reset:{position}"), position, on)
            .await
            .map_err(LedgerError::from_ledger(format!(
                "reset clearing {position}"
            )))?;
        Ok(())
    }

    /// The authoritative **Offene-Posten** list: open debit items (invoices, fees)
    /// with a positive residual after recorded clearings, each with its booking
    /// date, Buchungsart and Beleg, oldest first.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn open_receivables(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
    ) -> LedgerResult<Vec<OpenReceivable>> {
        let mut out = Vec::new();
        for item in self.open_items(lf_mp_id, malo_id).await? {
            if item.side != Side::Debit || !item.residual.is_positive() {
                continue;
            }
            let (entry_type, document) = self.describe(item.entry).await?;
            out.push(OpenReceivable {
                entry_id: entry_uuid(item.entry),
                entry_type,
                amount_ct: ct(item.original),
                outstanding_ct: ct(item.residual),
                booking_date: item.booking_date,
                document,
            });
        }
        out.sort_by_key(|o| o.booking_date);
        Ok(out)
    }

    /// The entry type and Beleg one entry carries; `(None, None)` for an entry
    /// the ledger does not hold.
    async fn describe(&self, id: EntryId) -> LedgerResult<(Option<String>, Option<String>)> {
        let record = self
            .books
            .entry(id)
            .await
            .map_err(LedgerError::from_ledger("read entry"))?;
        Ok(record.map_or((None, None), |r| {
            let parts = r.parts();
            (
                parts.kind.as_ref().map(|k| k.as_str().to_owned()),
                parts.documents.first().map(|d| d.reference().to_owned()),
            )
        }))
    }

    /// The source document one entry cites, if any.
    ///
    /// # Errors
    ///
    /// A store failure. An unknown entry is `None`.
    pub async fn document_of(&self, entry: uuid::Uuid) -> LedgerResult<Option<String>> {
        Ok(self.describe(entry_id(entry)).await?.1)
    }

    /// The **Summen- und Saldenliste** (trial balance, § 238 HGB): gross debit and
    /// credit turnover and the balance per GL account, with the per-Marktlokation
    /// Kontokorrent leaves as one Debitoren line.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn trial_balance(&self) -> LedgerResult<Vec<TrialBalanceLine>> {
        let tb = self
            .books
            .trial_balance(&BalanceQuery::all())
            .await
            .map_err(LedgerError::from_ledger("trial balance"))?;
        let mut agg: std::collections::BTreeMap<String, (i64, i64)> =
            std::collections::BTreeMap::new();
        for row in tb.rows() {
            let path = row.path.to_string();
            let group = if path.starts_with(Chart::KONTOKORRENT) {
                "Aktiva:Forderungen:Debitoren (Summe)".to_owned()
            } else {
                path
            };
            let entry = agg.entry(group).or_default();
            entry.0 = entry.0.saturating_add(ct(row.balance.debit()));
            entry.1 = entry.1.saturating_add(ct(row.balance.credit()));
        }
        Ok(agg
            .into_iter()
            .map(|(account, (debits, credits))| TrialBalanceLine {
                account,
                debits_ct: debits,
                credits_ct: credits,
                net_ct: debits.saturating_sub(credits),
            })
            .collect())
    }

    /// The customer's statement over `query` — opening balance, movements with
    /// running balance, closing balance; an empty statement for an account never
    /// booked is `None`.
    ///
    /// A date-scoped query opens at the balance carried into the window, so a
    /// March Kontokorrentauszug opens at February's closing figure.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn statement(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        query: &BalanceQuery,
    ) -> LedgerResult<Option<doubleentry::report::Statement<P>>> {
        match self
            .books
            .statement(Self::konto(lf_mp_id, malo_id)?.as_str(), EUR, query)
            .await
        {
            Ok(statement) => Ok(Some(statement)),
            Err(Error::Query(QueryError::UnknownAccount { .. })) => Ok(None),
            Err(e) => Err(LedgerError::from_ledger("statement")(e)),
        }
    }

    /// True when the customer has any **debit** (charge) booked on or before
    /// `cutoff` — the "debt aged past the dunning grace period" signal.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn has_debit_on_or_before(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        cutoff: Date,
    ) -> LedgerResult<bool> {
        match self
            .books
            .balance(
                Self::konto(lf_mp_id, malo_id)?.as_str(),
                EUR,
                &BalanceQuery::through(cutoff),
            )
            .await
        {
            Ok(balance) => Ok(balance.debit().is_positive()),
            Err(Error::Query(QueryError::UnknownAccount { .. })) => Ok(false),
            Err(e) => Err(LedgerError::from_ledger("read balance")(e)),
        }
    }

    /// Signed Kontokorrent movement per `entry_type` for a customer within a
    /// calendar `year` — the input to the Jahresabschluss.
    ///
    /// The value is the signed net on the Kontokorrent (debit positive, credit
    /// negative), so `RECHNUNG` sums positive and `ABSCHLAG` negative. One
    /// `balance` per entry type, filtered by the entry's kind and the year's
    /// booking dates; types with no movement are absent.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn year_kind_sums(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        year: i32,
    ) -> LedgerResult<std::collections::HashMap<String, i64>> {
        let (Ok(from), Ok(to)) = (
            Date::from_calendar_date(year, time::Month::January, 1),
            Date::from_calendar_date(year, time::Month::December, 31),
        ) else {
            return Err(LedgerError::refused(format!("year {year}")));
        };
        let mut sums = std::collections::HashMap::new();
        for kind in ENTRY_TYPES {
            let query = BalanceQuery::between(from, to).of_kind(kind);
            let net = self.balance(lf_mp_id, malo_id, &query).await?;
            if net != 0 {
                sums.insert((*kind).to_owned(), net);
            }
        }
        Ok(sums)
    }

    /// Every sealed period, oldest first — the Festschreibung history.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn seals(&self) -> LedgerResult<Vec<SealedPeriod>> {
        let receipts = self
            .books
            .seals()
            .await
            .map_err(LedgerError::from_ledger("read seals"))?;
        Ok(receipts.iter().map(SealedPeriod::from).collect())
    }

    /// The date the books are closed through — the last day of the latest
    /// sealed period, or `None` when nothing is sealed yet.
    ///
    /// # Errors
    ///
    /// A store failure.
    pub async fn sealed_through(&self) -> LedgerResult<Option<Date>> {
        self.books
            .watermark()
            .await
            .map_err(LedgerError::from_ledger("read watermark"))
    }

    /// Seals every open period through `period` (**Festschreibung** — GoBD /
    /// § 146 AO / § 239 HGB), earliest first. After this a booking dated inside
    /// a sealed period is refused (`301`); corrections book into a later open
    /// period.
    ///
    /// # Errors
    ///
    /// A refusal (unknown period, `311`) or a store failure.
    pub async fn seal_through(&self, period: &str) -> LedgerResult<Vec<SealedPeriod>> {
        let receipts = self
            .books
            .seal_through(period)
            .await
            .map_err(LedgerError::from_ledger(format!("seal through {period}")))?;
        Ok(receipts.iter().map(SealedPeriod::from).collect())
    }

    /// Replays the whole log and compares every derived table with it: the
    /// books verify, or this says where they do not.
    ///
    /// # Errors
    ///
    /// [`LedgerError::Integrity`] when the books do not verify.
    pub async fn audit(&self) -> LedgerResult<doubleentry::AuditReport> {
        self.books
            .audit()
            .await
            .map_err(LedgerError::from_ledger("audit"))
    }

    /// A tamper-evidence proof that `id` is in the log: the entry's leaf
    /// hash, the inclusion proof and the head it verifies against.
    ///
    /// # Errors
    ///
    /// [`LedgerError::NotFound`] for an unknown entry; a store failure.
    pub async fn prove_entry(
        &self,
        id: EntryId,
    ) -> LedgerResult<(
        doubleentry::log::Hash,
        doubleentry::log::InclusionProof,
        doubleentry::TreeHead,
    )> {
        let record = self
            .books
            .entry(id)
            .await
            .map_err(LedgerError::from_ledger("read entry"))?
            .ok_or_else(|| LedgerError::NotFound("entry not found".to_owned()))?;
        let proof = self
            .books
            .prove_entry(id)
            .await
            .map_err(LedgerError::from_ledger("inclusion proof"))?;
        let head = self
            .books
            .head_at(proof.size)
            .await
            .map_err(LedgerError::from_ledger("head"))?;
        let leaf = doubleentry::record::Record::Entry(record).leaf_hash();
        Ok((leaf, proof, head))
    }

    /// Proves the balance one customer's Kontokorrent closed a sealed period
    /// with, against the head alone — the Betriebsprüfung question „what did
    /// this customer owe at the balance-sheet date", answered with evidence
    /// rather than a figure read out of a table.
    ///
    /// # Errors
    ///
    /// [`LedgerError::NotFound`] when the period is not sealed or the account
    /// did not exist at the seal; a store failure.
    pub async fn prove_period_balance(
        &self,
        period: &str,
        lf_mp_id: &str,
        malo_id: &str,
    ) -> LedgerResult<(
        doubleentry::log::SealedBalanceProof<P>,
        doubleentry::TreeHead,
    )> {
        let proof = self
            .books
            .prove_balance(Self::konto(lf_mp_id, malo_id)?, EUR, Layer::Settled, period)
            .await
            .map_err(LedgerError::from_ledger(format!("prove balance {period}")))?;
        let head = self
            .books
            .head()
            .await
            .map_err(LedgerError::from_ledger("head"))?;
        Ok((proof, head))
    }

    /// Proves the log has only ever been **appended to** since it had `old_size`
    /// records: the proof, the head then and the head now.
    ///
    /// # Errors
    ///
    /// A refusal for `old_size` 0 or beyond the log; a store failure.
    pub async fn prove_append_only(
        &self,
        old_size: u64,
    ) -> LedgerResult<(
        doubleentry::log::ConsistencyProof,
        doubleentry::TreeHead,
        doubleentry::TreeHead,
    )> {
        let now = self
            .books
            .head()
            .await
            .map_err(LedgerError::from_ledger("head"))?;
        let then = self
            .books
            .head_at(old_size)
            .await
            .map_err(LedgerError::from_ledger(format!("head at {old_size}")))?;
        let proof = self
            .books
            .prove_consistency(old_size, now.size)
            .await
            .map_err(LedgerError::from_ledger("consistency proof"))?;
        Ok((proof, then, now))
    }
}

impl Ledger<SqlStore<Postgres, P>> {
    /// Opens the deployment's ledger in [`LEDGER_SCHEMA`] over `database_url`
    /// (an already resolved URL; TLS is the URL's `sslmode`), creating it when
    /// absent. One ledger per deployment, named by `tenant`.
    ///
    /// # Errors
    ///
    /// A connection, configuration or store failure.
    pub async fn connect(database_url: &str, tenant: &str, today: Date) -> LedgerResult<Self> {
        let store = SqlStore::<Postgres, P>::postgres_with(
            database_url,
            SqlOptions {
                schema: LEDGER_SCHEMA.to_owned(),
                max_connections: 5,
                ..SqlOptions::default()
            },
        )
        .await
        .map_err(|e| LedgerError::from_ledger("connect ledger store")(Error::from(e)))?;
        Self::open(store, tenant, today).await
    }

    /// Grants `role` exactly what doubleentry's store generates from its
    /// schema for a runtime role — read and append everywhere, update only the
    /// maintained projections and the columns the write path changes, never
    /// `DELETE`, `TRUNCATE` or DDL — in one transaction. Run as the owner.
    ///
    /// # Errors
    ///
    /// An invalid role name, an unknown role, or a connection without the
    /// owner's rights; then nothing was granted.
    pub async fn grant_runtime(&self, role: &str) -> LedgerResult<()> {
        self.store
            .grant_runtime(role)
            .await
            .map_err(|e| LedgerError::from_ledger("grant the runtime role")(Error::from(e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use doubleentry::store::MemoryStore;
    use time::macros::date;

    async fn test_ledger() -> Ledger<MemoryStore<P>> {
        Ledger::open(
            MemoryStore::<P>::new(),
            "test-tenant",
            date!(2026 - 03 - 01),
        )
        .await
        .expect("open ledger")
    }

    fn post(kind: &str, amount: i64, key: &str, on: Date) -> PostEntry {
        PostEntry::new("51238696012", "9900000000001", kind, amount, key, on)
    }

    const LF: &str = "9900000000001";
    const MALO: &str = "51238696012";

    #[tokio::test]
    async fn invoice_then_payment_nets_to_zero() {
        let ledger = test_ledger().await;
        ledger
            .post(&post("RECHNUNG", 11900, "inv-1", date!(2026 - 03 - 15)))
            .await
            .expect("invoice");
        assert_eq!(ledger.balance_ct(LF, MALO).await.unwrap(), 11900);
        ledger
            .post(&post("ZAHLUNG", -11900, "pay-1", date!(2026 - 03 - 20)))
            .await
            .expect("payment");
        assert_eq!(ledger.balance_ct(LF, MALO).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn replay_is_idempotent_and_returns_original() {
        let ledger = test_ledger().await;
        let entry = post("RECHNUNG", 5000, "inv-dup", date!(2026 - 03 - 15))
            .with_description("Jahresrechnung 2025")
            .with_correlation("ce-1")
            .with_document("RE-1");
        let first = ledger.post(&entry).await.expect("first");
        assert!(first.is_new);
        let second = ledger.post(&entry).await.expect("replay");
        assert!(!second.is_new, "identical replay must be a no-op");
        assert_eq!(first.id, second.id, "replay returns the original entry id");
        assert_eq!(ledger.balance_ct(LF, MALO).await.unwrap(), 5000);
    }

    #[tokio::test]
    async fn conflicting_key_is_refused_with_200() {
        let ledger = test_ledger().await;
        ledger
            .post(&post("RECHNUNG", 5000, "k", date!(2026 - 03 - 15)))
            .await
            .expect("first");
        let conflict = ledger
            .post(&post("RECHNUNG", 9999, "k", date!(2026 - 03 - 15)))
            .await
            .expect_err("reused key with different content must be refused");
        assert_eq!(conflict.code(), Some(200));
        assert_eq!(conflict.to_api().status(), axum::http::StatusCode::CONFLICT);
        assert_eq!(ledger.balance_ct(LF, MALO).await.unwrap(), 5000);
    }

    #[tokio::test]
    async fn eeg_gutschrift_credits_the_operator() {
        let ledger = test_ledger().await;
        ledger
            .post(&post(
                "EEG_GUTSCHRIFT",
                -25000,
                "eeg-1",
                date!(2026 - 03 - 31),
            ))
            .await
            .expect("eeg");
        assert_eq!(ledger.balance_ct(LF, MALO).await.unwrap(), -25000);
    }

    #[tokio::test]
    async fn zero_amount_and_unknown_type_are_refused_before_the_ledger() {
        let ledger = test_ledger().await;
        let zero = ledger
            .post(&post("RECHNUNG", 0, "z", date!(2026 - 03 - 15)))
            .await
            .expect_err("zero");
        assert_eq!(zero.code(), None);
        let unknown = ledger
            .post(&post("SONSTIGES", 100, "u", date!(2026 - 03 - 15)))
            .await
            .expect_err("unknown type");
        assert!(
            unknown.to_string().contains("unknown entry type"),
            "{unknown}"
        );
        assert_eq!(
            ledger.books().head().await.unwrap().size,
            test_ledger().await.books().head().await.unwrap().size,
            "a refused draft writes nothing"
        );
    }

    #[tokio::test]
    async fn all_entry_types_balance_and_sum_by_type() {
        let ledger = test_ledger().await;
        for (i, kind) in ENTRY_TYPES.iter().enumerate() {
            let amount = if i % 2 == 0 { 1000 } else { -400 };
            ledger
                .post(&post(
                    kind,
                    amount,
                    &format!("k-{i}"),
                    date!(2026 - 03 - 15),
                ))
                .await
                .unwrap_or_else(|e| panic!("{kind} should post: {e}"));
        }
        let tb = ledger.trial_balance().await.unwrap();
        let (d, c): (i64, i64) = tb
            .iter()
            .fold((0, 0), |a, l| (a.0 + l.debits_ct, a.1 + l.credits_ct));
        assert_eq!(d, c, "Σ Soll = Σ Haben");
        let sums = ledger.year_kind_sums(LF, MALO, 2026).await.unwrap();
        assert_eq!(sums.get("RECHNUNG"), Some(&1000));
        assert_eq!(sums.get("STORNO"), Some(&-400));
        assert!(
            ledger
                .year_kind_sums(LF, MALO, 2025)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_never_booked_malo_reads_as_empty() {
        let ledger = test_ledger().await;
        assert_eq!(ledger.balance_ct(LF, "51238696012").await.unwrap(), 0);
        assert!(ledger.open_receivables(LF, MALO).await.unwrap().is_empty());
        assert!(
            ledger
                .statement(LF, MALO, &BalanceQuery::all())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !ledger
                .has_debit_on_or_before(LF, MALO, date!(2026 - 03 - 31))
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_booking_into_a_sealed_month_is_refused_with_301() {
        let ledger = test_ledger().await;
        ledger
            .post(&post("RECHNUNG", 1000, "jan", date!(2026 - 01 - 10)))
            .await
            .unwrap();
        let sealed = ledger.seal_through("2026-01").await.unwrap();
        assert_eq!(sealed.last().map(|s| s.period.as_str()), Some("2026-01"));
        assert_eq!(
            ledger.sealed_through().await.unwrap(),
            Some(date!(2026 - 01 - 31))
        );
        let late = ledger
            .post(&post("RECHNUNG", 1000, "late", date!(2026 - 01 - 20)))
            .await
            .expect_err("sealed");
        assert_eq!(late.code(), Some(301));
        // A MaLo first booked after the seal opens the day after it.
        ledger
            .post(&PostEntry::new(
                "51238696020",
                LF,
                "RECHNUNG",
                500,
                "new",
                date!(2026 - 02 - 02),
            ))
            .await
            .expect("an account opened after the seal books into an open month");
        assert!(ledger.audit().await.is_ok());
    }

    #[tokio::test]
    async fn fifo_clearing_matches_oldest_first_and_resets() {
        let ledger = test_ledger().await;
        ledger
            .post(&post("RECHNUNG", 10000, "inv1", date!(2026 - 01 - 01)))
            .await
            .unwrap();
        ledger
            .post(&post("RECHNUNG", 20000, "inv2", date!(2026 - 02 - 01)))
            .await
            .unwrap();
        ledger
            .post(&post("ZAHLUNG", -15000, "pay", date!(2026 - 02 - 15)))
            .await
            .unwrap();
        assert_eq!(ledger.open_receivables(LF, MALO).await.unwrap().len(), 2);

        let cleared = ledger
            .apply_fifo_clearing(LF, MALO, date!(2026 - 02 - 15))
            .await
            .unwrap();
        assert_eq!(cleared.len(), 2, "the payment covers inv1 and part of inv2");
        let open = ledger.open_receivables(LF, MALO).await.unwrap();
        assert_eq!(open.len(), 1, "oldest invoice fully cleared");
        assert_eq!(open[0].outstanding_ct, 15000, "second invoice partly open");
        assert!(
            ledger
                .apply_fifo_clearing(LF, MALO, date!(2026 - 02 - 16))
                .await
                .unwrap()
                .is_empty(),
            "a re-run matches nothing more"
        );
        for position in cleared {
            ledger
                .reset_clearing(position, date!(2026 - 02 - 17))
                .await
                .unwrap();
        }
        assert_eq!(ledger.open_receivables(LF, MALO).await.unwrap().len(), 2);
        assert_eq!(
            ledger
                .apply_fifo_clearing(LF, MALO, date!(2026 - 02 - 18))
                .await
                .unwrap()
                .len(),
            2,
            "after a reset the same pairs clear anew"
        );
    }

    #[tokio::test]
    async fn proofs_verify_against_the_head() {
        let ledger = test_ledger().await;
        let posted = ledger
            .post(&post("RECHNUNG", 1000, "p", date!(2026 - 01 - 10)))
            .await
            .unwrap();
        let (leaf, proof, head) = ledger.prove_entry(posted.id).await.unwrap();
        proof.verify(&leaf, &head).expect("inclusion");
        ledger.seal_through("2026-01").await.unwrap();
        let (balance, head) = ledger
            .prove_period_balance("2026-01", LF, MALO)
            .await
            .unwrap();
        let proven = balance.verify(&head).expect("sealed balance");
        assert_eq!(ct(proven.balance.net()), 1000);
        let (consistency, then, now) = ledger.prove_append_only(1).await.unwrap();
        consistency.verify(&then, &now).expect("append-only");
    }
}
