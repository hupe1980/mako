//! accountingd's accounting/storage base — a [`doubleentry`] double-entry ledger.
//!
//! This module is the seam between accountingd's domain (energy-market
//! Massenkontokorrent, SKR 03/04, EEG payouts, dunning) and the domain-neutral
//! [`doubleentry`] kernel. accountingd owns the **chart of accounts** and the
//! **`entry_type` → postings** mapping; `doubleentry` owns everything a ledger
//! must guarantee and accountingd used to hand-roll: balances, immutability, an
//! append-only Merkle log with inclusion/consistency proofs, period seals
//! (GoBD Unveränderbarkeit / § 146 AO), open-item clearing, and store-level
//! idempotency.
//!
//! ## The model
//!
//! Every money movement is one balanced entry with exactly two legs:
//!
//! - the customer/counterparty **Kontokorrent** — one leaf account per
//!   Marktlokation (`Kontokorrent:<lf_mp>:<malo>`, [`AccountKind::Asset`]). Its
//!   signed net *is* the old `accounts.balance_ct`: a debit balance means the
//!   customer owes, a credit balance means they are owed (EEG operators, refunds).
//! - a **general-ledger** contra account, one per `entry_type`
//!   ([`Chart`]) — Erlöse, Bank, Mahnerlöse, EEG-Aufwand.
//!
//! Because the entry is balanced by construction and the Kontokorrent leg
//! reproduces the signed `amount_ct` exactly, per-customer balances survive the
//! move with no cached column to drift, and the GL accounts roll up to the SKR
//! trial balance for free — replacing both the `ledger_entries` running log and
//! the `journal_lines` SKR shadow with one authoritative, provable structure.

use std::sync::Mutex;

use time::Date;

use doubleentry::account::{Account, AccountId, AccountKind, AccountPath, AccountRecord};
use doubleentry::clearing::{ClearedItem, Clearing, ClearingId, PostingRef};
use doubleentry::entry::{Description, DocumentRef, LedgerPolicy, Provenance, SealContext};
use doubleentry::period::{LedgerId, Period, PeriodCalendar, PeriodId, PeriodState};
use doubleentry::posting::Direction;
use doubleentry::seal::{SealChain, SealedBalanceOutcome};
use doubleentry::storage::postgres::PostgresStore;
use doubleentry::storage::{Cursor, EntryBatch, LedgerStore, PostingCursor, StatementPage};
use doubleentry::{
    AccountRegistry, Amount, BalanceKey, ConsistencyProof, Currency, Entry, EntryId, Hash,
    IdempotencyKey, InclusionProof, Label, Layer, OpenItem, Seal, TreeHead,
};

/// EUR at 2-dp minor units (cents) — accountingd's only currency.
pub type Eur = Amount<2>;

/// The precision every accountingd account uses.
pub const P: u8 = 2;

/// The PostgreSQL schema the doubleentry ledger's tables live in, sharing
/// accountingd's database with the customer/SEPA satellites in `public`.
pub const LEDGER_SCHEMA: &str = "doubleentry";

/// The production ledger type — a doubleentry [`PostgresStore`] for one deployment.
pub type PgLedger = Ledger<PostgresStore<P>>;

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

/// The fixed general-ledger contra accounts, resolved once at load.
///
/// These are the non-customer side of every entry. The customer side is a
/// per-Marktlokation Kontokorrent registered lazily on first booking.
///
/// # No `BalanceLimit` on any of them
///
/// doubleentry can pin an account to one side of the ledger and reject in the
/// append transaction any entry that would flip it. No account here takes one,
/// and the reason differs per account rather than being an oversight:
///
/// - **Kontokorrent** is bidirectional by design — a credit balance is how an
///   EEG operator or an overpaying customer is represented. A limit here would
///   contradict the ledger's own model.
/// - **Bank** looks like it qualifies (a chargeback always follows a
///   collection), but only in steady state. The ledger starts empty at
///   cut-over, so a `BANKRUECKLAST` for a collection made in the legacy system
///   would be the first movement on the account and a limit would make a
///   legitimate inbound bank event permanently unbookable, with no override.
/// - **Erloese / Mahnerloese / EEG-Aufwand** are each one side of a partial
///   view; a STORNO of a pre-migration invoice breaches them for the same
///   reason.
/// - **Erstattungen** is only ever credited — nothing discharges it here — so a
///   `NoDebitBalance` limit would be true and could never fire.
///
/// The invariant that is actually worth enforcing (every entry balances) is
/// structural and already unconditional.
#[derive(Debug, Clone, Copy)]
pub struct Chart {
    /// SKR 1200 — Bankguthaben. Debited by incoming payments, credited by chargebacks.
    pub bank: AccountId,
    /// SKR 4000 — Energieerlöse. The revenue contra for invoices and settlements.
    pub erloese: AccountId,
    /// SKR 4003 — Mahngebühren / Verzugszinsen.
    pub mahnerloese: AccountId,
    /// EEG feed-in expense (Aufwand) — the contra for EEG Gutschrift / Marktprämie.
    pub eeg_aufwand: AccountId,
    /// Refund payable — the contra when a Jahresabschluss books a customer refund
    /// (Erstattung): the customer credit is zeroed and this liability recognises
    /// the amount owed until the pain.001 payout discharges it.
    pub erstattung: AccountId,
}

impl Chart {
    /// The GL contra account for an `entry_type`.
    ///
    /// The customer Kontokorrent is always the other leg; its direction follows
    /// the sign of `amount_ct` (positive → debit customer, negative → credit),
    /// which is what makes the Kontokorrent net equal the old `balance_ct`.
    #[must_use]
    fn contra(&self, entry_type: &str) -> AccountId {
        match entry_type {
            // SEPA_STORNO is a pain.007 reversal: the creditor hands a settled
            // collection back, so the money leaves the bank account again and
            // the receivable re-opens — the same two accounts as the collection
            // it undoes, in the opposite direction.
            "ZAHLUNG" | "ABSCHLAG" | "BANKRUECKLAST" | "SEPA_STORNO" => self.bank,
            "MAHNGEBUEHR" => self.mahnerloese,
            "EEG_GUTSCHRIFT" | "EEG_MARKTPRAEMIE" => self.eeg_aufwand,
            // Erstattung: zero the customer credit against a refund payable.
            "JAHRESABSCHLUSS" => self.erstattung,
            // RECHNUNG, STORNO, GUTSCHRIFT, KORREKTUR, …
            _ => self.erloese,
        }
    }
}

/// A request to post one balanced entry.
#[derive(Debug, Clone)]
pub struct PostEntry {
    /// Marktlokation — identifies the customer Kontokorrent.
    pub malo_id: String,
    /// The LF's own market-partner id — namespaces the Kontokorrent path.
    pub lf_mp_id: String,
    /// The accountingd entry type (`RECHNUNG`, `ZAHLUNG`, `EEG_GUTSCHRIFT`, …).
    /// Stored verbatim as the doubleentry entry `kind` label.
    pub entry_type: String,
    /// Signed minor units: positive increases the receivable, negative reduces it.
    pub amount_ct: i64,
    /// The idempotency key. **Every** write carries one — a CloudEvent id, a bank
    /// transaction id, or a deterministic string (`mahngebuehr:{malo}:{stufe}:{date}`).
    /// The store makes the write a no-op on identical replay and a conflict on a
    /// reused key with different content.
    pub idempotency: String,
    /// Booking date (Buchungsdatum).
    pub booking_date: Date,
    /// Value date (Wertstellung); defaults to `booking_date`.
    pub value_date: Date,
    /// Free-text line description.
    pub description: Option<String>,
    /// External correlation (a CloudEvent id) folded into provenance.
    pub correlation: Option<String>,
    /// Source-document identifier (invoice/record id) — attached as a
    /// [`DocumentRef`], the tamper-evident link from a booking to its document.
    pub document: Option<String>,
    /// The acting principal (operator sub for manual bookings).
    pub actor: Option<String>,
    /// When set, this entry reverses `(id, original booking date)` — a STORNO.
    pub reverses: Option<(EntryId, Date)>,
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
            reverses: None,
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
}

/// One entry of a given kind, with the customer it belongs to (see
/// [`Ledger::entries_of_kind_in_month`]).
#[derive(Debug, Clone)]
pub struct KindEntry {
    /// The LF market-partner id of the owning Kontokorrent.
    pub lf_mp_id: String,
    /// The Marktlokation of the owning Kontokorrent.
    pub malo_id: String,
    /// The doubleentry entry id.
    pub entry_id: uuid::Uuid,
    /// The Kontokorrent leg's amount in minor units.
    pub amount_ct: i64,
    /// The originating CloudEvent id, from provenance, if any.
    pub correlation: Option<String>,
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
    /// The entry identifier (stable across idempotent replays).
    pub id: EntryId,
    /// `false` when an identical entry had already been recorded — a safe retry.
    pub is_new: bool,
}

/// A [`doubleentry`] ledger scoped to one accountingd deployment (one `tenant`).
///
/// Generic over the backend so unit tests run against `MemoryStore` and
/// production runs against `PostgresStore`, both verified by doubleentry's own
/// conformance suite.
pub struct Ledger<S: LedgerStore<P>> {
    store: S,
    /// The account tree. Guarded by a std mutex held only across the synchronous
    /// seal — never across an `await` — so posts to existing accounts run
    /// concurrently at the database level.
    registry: Mutex<AccountRegistry>,
    chart: Chart,
    /// Period calendar (Festschreibung). Guarded like the registry — locked only
    /// across the synchronous seal validation, never across an `await`. Sealed
    /// periods here make `post` reject a backdated booking into a closed period.
    calendar: Mutex<PeriodCalendar>,
    policy: LedgerPolicy,
}

impl<S: LedgerStore<P>> Ledger<S> {
    /// Opens the ledger over `store`, restoring the account tree from storage and
    /// ensuring the fixed GL chart exists.
    ///
    /// The registry is loaded from stored records (never rebuilt by re-registering
    /// paths, which would reissue handles and repoint history), then the GL
    /// accounts are registered if absent and persisted.
    ///
    /// # Errors
    ///
    /// Propagates storage and registry errors.
    pub async fn load(store: S) -> anyhow::Result<Self> {
        let records = store
            .accounts()
            .await
            .map_err(|e| anyhow::anyhow!("load accounts: {e}"))?;
        let mut registry = AccountRegistry::from_records(records)
            .map_err(|e| anyhow::anyhow!("rebuild registry: {e}"))?;

        // Opening date for the GL chart: the epoch of the ledger. Booking dates
        // are always after this, so the open-window check never rejects a GL leg.
        let epoch = Date::from_calendar_date(2000, time::Month::January, 1)
            .map_err(|e| anyhow::anyhow!("epoch: {e}"))?;

        let mut to_persist: Vec<AccountRecord> = Vec::new();
        let mut ensure = |registry: &mut AccountRegistry,
                          path: &str,
                          kind: AccountKind|
         -> anyhow::Result<AccountId> {
            let parsed = AccountPath::parse(path)
                .map_err(|e| anyhow::anyhow!("account path {path}: {e}"))?;
            if let Some(id) = registry.id_of(&parsed) {
                return Ok(id);
            }
            let account = Account::new(parsed, epoch).with_kind(kind);
            let id = registry
                .register(account.clone())
                .map_err(|e| anyhow::anyhow!("register {path}: {e}"))?;
            to_persist.push(AccountRecord { id, account });
            Ok(id)
        };

        let chart = Chart {
            bank: ensure(&mut registry, "Aktiva:Bank", AccountKind::Asset)?,
            erloese: ensure(&mut registry, "Ertrag:Energieerloese", AccountKind::Income)?,
            mahnerloese: ensure(&mut registry, "Ertrag:Mahnerloese", AccountKind::Income)?,
            eeg_aufwand: ensure(
                &mut registry,
                "Aufwand:EEG-Einspeiseverguetung",
                AccountKind::Expense,
            )?,
            erstattung: ensure(
                &mut registry,
                "Passiva:Verbindlichkeiten:Erstattungen",
                AccountKind::Liability,
            )?,
        };

        for record in &to_persist {
            store
                .register_account(record)
                .await
                .map_err(|e| anyhow::anyhow!("persist GL account: {e}"))?;
        }

        Ok(Self {
            store,
            registry: Mutex::new(registry),
            chart,
            calendar: Mutex::new(PeriodCalendar::new()),
            policy: LedgerPolicy::default(),
        })
    }

    /// The underlying store, for reads the ledger does not wrap.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The Kontokorrent path for a Marktlokation.
    fn kontokorrent_path(lf_mp_id: &str, malo_id: &str) -> anyhow::Result<AccountPath> {
        AccountPath::parse(&format!("Kontokorrent:{lf_mp_id}:{malo_id}"))
            .map_err(|e| anyhow::anyhow!("kontokorrent path for {malo_id}: {e}"))
    }

    /// Resolves an existing Kontokorrent, or `None` if the customer has never been booked.
    fn resolve(&self, lf_mp_id: &str, malo_id: &str) -> anyhow::Result<Option<AccountId>> {
        let path = Self::kontokorrent_path(lf_mp_id, malo_id)?;
        let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        Ok(registry.id_of(&path))
    }

    /// Resolves a Kontokorrent, registering (and persisting) it on first use.
    ///
    /// The account row is persisted before this returns, so any entry that later
    /// references the handle cannot precede its account in the store.
    async fn resolve_or_register(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        opened_on: Date,
    ) -> anyhow::Result<AccountId> {
        let path = Self::kontokorrent_path(lf_mp_id, malo_id)?;

        // A new binding to persist, produced under the lock. `None` = the account
        // already existed (no persist needed). The lock is never held across the
        // await below.
        let new_record: Option<AccountRecord> = {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(id) = registry.id_of(&path) {
                return Ok(id);
            }
            let account = Account::new(path.clone(), opened_on).with_kind(AccountKind::Asset);
            let id = registry
                .register(account.clone())
                .map_err(|e| anyhow::anyhow!("register kontokorrent {malo_id}: {e}"))?;
            Some(AccountRecord { id, account })
        };

        match new_record {
            Some(record) => {
                let id = record.id;
                store_register_account(&self.store, &record).await?;
                Ok(id)
            }
            // Unreachable in practice (we returned above), kept for exhaustiveness.
            None => self
                .resolve(lf_mp_id, malo_id)?
                .ok_or_else(|| anyhow::anyhow!("kontokorrent vanished after registration")),
        }
    }

    /// Posts one balanced entry and records it in the log.
    ///
    /// Idempotent by `req.idempotency`: an identical replay is a no-op returning
    /// the original entry id; a reused key with different content is refused.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero amount, an invalid field, a failed validation,
    /// or a storage failure (including an idempotency conflict).
    pub async fn post(&self, req: PostEntry) -> anyhow::Result<Posted> {
        if req.amount_ct == 0 {
            anyhow::bail!("refusing to post a zero-amount entry ({})", req.entry_type);
        }
        let abs = i64::try_from(req.amount_ct.unsigned_abs())
            .map_err(|_| anyhow::anyhow!("amount out of range"))?;
        let amount = Eur::from_minor(abs);

        let customer = self
            .resolve_or_register(&req.lf_mp_id, &req.malo_id, req.booking_date)
            .await?;
        let contra = self.chart.contra(&req.entry_type);
        // Positive → customer owes (debit customer); negative → customer credited.
        let (debit, credit) = if req.amount_ct > 0 {
            (customer, contra)
        } else {
            (contra, customer)
        };

        let key = IdempotencyKey::new(req.idempotency.into_bytes())
            .map_err(|e| anyhow::anyhow!("idempotency key: {e}"))?;

        let mut provenance = Provenance::none()
            .with_source("accountingd")
            .map_err(|e| anyhow::anyhow!("provenance source: {e}"))?;
        if let Some(correlation) = &req.correlation {
            provenance = provenance
                .with_correlation(correlation)
                .map_err(|e| anyhow::anyhow!("provenance correlation: {e}"))?;
        }
        if let Some(actor) = &req.actor {
            provenance = provenance
                .with_actor(actor)
                .map_err(|e| anyhow::anyhow!("provenance actor: {e}"))?;
        }

        let mut draft = Entry::new(EntryId::generate(), key, req.booking_date)
            .with_value_date(req.value_date)
            .debit(debit, amount, Currency::EUR)
            .credit(credit, amount, Currency::EUR)
            .with_kind(
                Label::new(req.entry_type.as_str())
                    .map_err(|e| anyhow::anyhow!("entry kind: {e}"))?,
            )
            .with_provenance(provenance);
        if let Some(description) = &req.description {
            draft = draft.with_description(
                Description::new(description.as_str())
                    .map_err(|e| anyhow::anyhow!("description: {e}"))?,
            );
        }
        if let Some(document) = &req.document {
            draft = draft.with_document(
                DocumentRef::unverified(document.as_str())
                    .map_err(|e| anyhow::anyhow!("document ref: {e}"))?,
            );
        }
        if let Some((original_id, original_date)) = req.reverses {
            draft = draft.reversing(original_id, original_date);
        }

        let balanced = {
            let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            let calendar = self.calendar.lock().unwrap_or_else(|e| e.into_inner());
            let ctx = SealContext {
                accounts: &registry,
                calendar: &calendar,
                policy: &self.policy,
            };
            draft
                .seal(&ctx)
                .map_err(|errors| anyhow::anyhow!("ledger validation rejected entry: {errors}"))?
        };

        let recorded = self
            .store
            .append(&EntryBatch::single(balanced))
            .await
            .map_err(|e| anyhow::anyhow!("append entry: {e}"))?;
        let first = recorded
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("append returned no record"))?;
        Ok(Posted {
            id: first.id,
            is_new: first.is_new,
        })
    }

    /// The customer's balance in minor units: positive = owes, negative = is owed.
    ///
    /// Zero for a Marktlokation that has never been booked.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn balance_ct(&self, lf_mp_id: &str, malo_id: &str) -> anyhow::Result<i64> {
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(0);
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };
        let balance = self
            .store
            .balance(key, None)
            .await
            .map_err(|e| anyhow::anyhow!("read balance: {e}"))?;
        Ok(balance
            .signed_net()
            .map_err(|e| anyhow::anyhow!("net balance: {e}"))?
            .to_minor())
    }

    /// The customer's open items (postings with an outstanding residual).
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn open_items(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
    ) -> anyhow::Result<Vec<OpenItem<P>>> {
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(Vec::new());
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };
        self.all_open_items(key).await
    }

    /// Every open item on the account — the deliberately unbounded read.
    ///
    /// Both callers need completeness, not just the oldest few. Allocating a
    /// payment across invoices and totalling what an account has outstanding are
    /// each answered *wrongly* by a partial list: a payment larger than the first
    /// page's residuals under-allocates, and a total comes out short.
    ///
    /// What a partial read would **not** break is the FIFO order itself — pages
    /// come oldest first, so the first page is the oldest items and matching over
    /// it is correct as far as it goes. The § 252 HGB risk here is incompleteness,
    /// not mis-ordering.
    ///
    /// A per-Marktlokation Kontokorrent holds a handful of open invoices, so this
    /// is one page in practice; it is written to be right when it is not.
    async fn all_open_items(&self, key: BalanceKey) -> anyhow::Result<Vec<OpenItem<P>>> {
        self.store
            .all_open_items(key)
            .await
            .map_err(|e| anyhow::anyhow!("open items: {e}"))
    }

    /// Booking date + log position for every posting on the account, from the
    /// statement — the ordering key for FIFO clearing and the OP list.
    async fn posting_meta(
        &self,
        key: BalanceKey,
    ) -> anyhow::Result<std::collections::HashMap<PostingRef, (Date, u64, Option<String>)>> {
        let mut meta = std::collections::HashMap::new();
        // A posting cursor, not an entry cursor: one entry can put several lines
        // on the same account, so a page boundary falling inside an entry would
        // skip its remaining postings — invisibly, because the running balance
        // stays consistent across the gap.
        let mut cursor = PostingCursor::start();
        loop {
            let page = self
                .store
                .statement(key, cursor)
                .await
                .map_err(|e| anyhow::anyhow!("statement: {e}"))?;
            for line in &page.lines {
                meta.insert(
                    line.posting,
                    (
                        line.booking_date,
                        line.index.get(),
                        line.kind.as_ref().map(|k| k.as_str().to_owned()),
                    ),
                );
            }
            match page.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(meta)
    }

    /// Records a **FIFO Zahlungszuordnung**: matches the customer's open credit
    /// residuals (payments, Abschläge, Gutschriften) against their oldest open
    /// debit residuals (invoices, fees), oldest first, and stores the clearing so
    /// the open-item list reflects what has actually been paid — not a gross list.
    ///
    /// Returns the new clearing id, or `None` when nothing is left to match (so a
    /// re-run is a safe no-op). This is the § 252 HGB per-receivable matching that
    /// a running balance alone cannot express.
    ///
    /// # Errors
    ///
    /// Propagates storage/clearing errors.
    pub async fn apply_fifo_clearing(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        on: Date,
    ) -> anyhow::Result<Option<ClearingId>> {
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(None);
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };

        let meta = self.posting_meta(key).await?;
        let sort_key = |p: &PostingRef| meta.get(p).map_or((Date::MIN, u64::MAX), |m| (m.0, m.1));

        let open = self.all_open_items(key).await?;
        let mut debits: Vec<(PostingRef, i64)> = open
            .iter()
            .filter(|i| i.direction == Direction::Debit && i.residual.to_minor() > 0)
            .map(|i| (i.posting, i.residual.to_minor()))
            .collect();
        let credits: Vec<(PostingRef, i64)> = {
            let mut c: Vec<(PostingRef, i64)> = open
                .iter()
                .filter(|i| i.direction == Direction::Credit && i.residual.to_minor() > 0)
                .map(|i| (i.posting, i.residual.to_minor()))
                .collect();
            c.sort_by_key(|(p, _)| sort_key(p));
            c
        };
        debits.sort_by_key(|(p, _)| sort_key(p));

        // Consume the credit pool against the oldest debits, oldest credit first.
        let mut applied: std::collections::BTreeMap<PostingRef, i64> =
            std::collections::BTreeMap::new();
        let mut ci = 0usize;
        let mut credit_left = credits.first().map_or(0, |c| c.1);
        'debit: for (dref, mut need) in debits {
            while need > 0 {
                while credit_left == 0 {
                    ci += 1;
                    match credits.get(ci) {
                        Some(c) => credit_left = c.1,
                        None => break 'debit,
                    }
                }
                let take = need.min(credit_left);
                *applied.entry(dref).or_default() += take;
                *applied.entry(credits[ci].0).or_default() += take;
                need -= take;
                credit_left -= take;
            }
        }

        let items: Vec<ClearedItem<P>> = applied
            .into_iter()
            .filter(|(_, amt)| *amt > 0)
            .map(|(posting, amt)| ClearedItem {
                posting,
                applied: Eur::from_minor(amt),
            })
            .collect();
        if items.len() < 2 {
            return Ok(None);
        }
        let id = ClearingId::generate();
        self.store
            .clear(Clearing {
                id,
                account,
                currency: Currency::EUR,
                // accountingd books only settled movements — it has no
                // reservation layer. Naming it explicitly keeps a future
                // reservation (a pending SEPA collection, say) from being
                // netted against a booked payment.
                layer: Layer::Settled,
                cleared_on: on,
                items,
            })
            .await
            .map_err(|e| anyhow::anyhow!("record clearing: {e}"))?;
        Ok(Some(id))
    }

    /// The authoritative **Offene-Posten** list: open debit items (invoices, fees)
    /// with a positive residual after recorded clearings, each with its booking
    /// date and Buchungsart, oldest first.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn open_receivables(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
    ) -> anyhow::Result<Vec<OpenReceivable>> {
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(Vec::new());
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };
        let meta = self.posting_meta(key).await?;
        let open = self.all_open_items(key).await?;
        let mut out: Vec<OpenReceivable> = open
            .iter()
            .filter(|i| i.direction == Direction::Debit && i.residual.to_minor() > 0)
            .map(|i| {
                let (date, kind) = meta
                    .get(&i.posting)
                    .map(|m| (m.0, m.2.clone()))
                    .unwrap_or((Date::MIN, None));
                OpenReceivable {
                    entry_id: *i.posting.entry.as_uuid(),
                    entry_type: kind,
                    amount_ct: i.original.to_minor(),
                    outstanding_ct: i.residual.to_minor(),
                    booking_date: date,
                }
            })
            .collect();
        out.sort_by_key(|o| o.booking_date);
        Ok(out)
    }

    /// The **Summen- und Saldenliste** (trial balance, § 238 HGB): gross debit and
    /// credit turnover and the balance per GL account. The per-Marktlokation
    /// Kontokorrent leaves are aggregated into one Debitoren line, so this stays a
    /// GL-level report rather than one row per customer.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn trial_balance(&self) -> anyhow::Result<Vec<TrialBalanceLine>> {
        let tb = self
            .store
            .trial_balance(None)
            .await
            .map_err(|e| anyhow::anyhow!("trial balance: {e}"))?;
        let mut agg: std::collections::BTreeMap<String, (i64, i64)> =
            std::collections::BTreeMap::new();
        {
            let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            for (key, balance) in tb.iter() {
                let path = registry
                    .get(key.account)
                    .map_or_else(String::new, |a| a.path.to_string());
                let group = if path.starts_with("Kontokorrent:") {
                    "Aktiva:Forderungen:Debitoren (Summe)".to_owned()
                } else {
                    path
                };
                let entry = agg.entry(group).or_default();
                entry.0 = entry.0.saturating_add(balance.debits.to_minor());
                entry.1 = entry.1.saturating_add(balance.credits.to_minor());
            }
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

    /// Releases a clearing (a mis-assigned Zahlungszuordnung) — the applied
    /// amounts return to their postings' residuals. The original record stays: an
    /// assignment made and withdrawn is itself part of the trail.
    ///
    /// # Errors
    ///
    /// Propagates storage/clearing errors (unknown or already-reset clearing).
    pub async fn reset_clearing(&self, id: ClearingId, on: Date) -> anyhow::Result<()> {
        self.store
            .reset_clearing(id, on)
            .await
            .map_err(|e| anyhow::anyhow!("reset clearing: {e}"))
    }

    /// A page of the customer's statement (movements + running balance).
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn statement(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        cursor: PostingCursor,
    ) -> anyhow::Result<StatementPage<P>> {
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(StatementPage {
                lines: Vec::new(),
                next: None,
            });
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };
        self.store
            .statement(key, cursor)
            .await
            .map_err(|e| anyhow::anyhow!("statement: {e}"))
    }

    /// True when the customer has any **debit** (charge) booked on or before
    /// `cutoff` — the "debt aged past the dunning grace period" signal. Scans the
    /// statement oldest-first with early exit.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn has_debit_on_or_before(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        cutoff: Date,
    ) -> anyhow::Result<bool> {
        use doubleentry::Direction;
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(false);
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };
        let mut cursor = PostingCursor::start();
        loop {
            let page = self
                .store
                .statement(key, cursor)
                .await
                .map_err(|e| anyhow::anyhow!("statement: {e}"))?;
            for line in &page.lines {
                if line.direction == Direction::Debit && line.booking_date <= cutoff {
                    return Ok(true);
                }
            }
            match page.next {
                Some(next) => cursor = next,
                None => return Ok(false),
            }
        }
    }

    /// Signed Kontokorrent movement per `entry_type` (doubleentry `kind`) for a
    /// customer within a calendar `year` — the input to the Jahresabschluss.
    ///
    /// The value is the signed net on the Kontokorrent (debit positive, credit
    /// negative), so `RECHNUNG` sums positive and `ABSCHLAG` negative — matching
    /// the old `SUM(amount_ct)` semantics. Bounded: one customer's yearly entries.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn year_kind_sums(
        &self,
        lf_mp_id: &str,
        malo_id: &str,
        year: i32,
    ) -> anyhow::Result<std::collections::HashMap<String, i64>> {
        use doubleentry::Direction;
        let mut sums: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            return Ok(sums);
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };
        let mut cursor = PostingCursor::start();
        loop {
            let page = self
                .store
                .statement(key, cursor)
                .await
                .map_err(|e| anyhow::anyhow!("statement: {e}"))?;
            for line in &page.lines {
                if line.booking_date.year() != year {
                    continue;
                }
                let signed = match line.direction {
                    Direction::Debit => line.amount.to_minor(),
                    Direction::Credit => -line.amount.to_minor(),
                };
                let kind = self
                    .store
                    .get(line.posting.entry)
                    .await
                    .map_err(|e| anyhow::anyhow!("get entry: {e}"))?
                    .and_then(|stored| stored.entry.kind().map(|k| k.as_str().to_owned()))
                    .unwrap_or_default();
                *sums.entry(kind).or_default() += signed;
            }
            match page.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(sums)
    }

    /// Every entry of a given `kind` booked in `(year, month)`, with the
    /// customer it belongs to — the input to the batch EEG-payout run.
    ///
    /// Pages the whole log (a manual, infrequent batch operation). Each result
    /// names the Kontokorrent leg's customer and that leg's amount.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn entries_of_kind_in_month(
        &self,
        kind: &str,
        year: i32,
        month: u8,
    ) -> anyhow::Result<Vec<KindEntry>> {
        // Map Kontokorrent account handles back to (lf_mp, malo).
        let records = self
            .store
            .accounts()
            .await
            .map_err(|e| anyhow::anyhow!("load accounts: {e}"))?;
        let mut konto: std::collections::HashMap<AccountId, (String, String)> =
            std::collections::HashMap::new();
        for rec in &records {
            let path = rec.account.path.to_string();
            if let Some(rest) = path.strip_prefix("Kontokorrent:")
                && let Some((lf, malo)) = rest.split_once(':')
            {
                konto.insert(rec.id, (lf.to_owned(), malo.to_owned()));
            }
        }

        let mut out = Vec::new();
        let mut cursor = Cursor::start();
        loop {
            let page = self
                .store
                .page(cursor)
                .await
                .map_err(|e| anyhow::anyhow!("page log: {e}"))?;
            for rec in &page.records {
                let entry = &rec.entry;
                if entry.kind().map(|k| k.as_str()) != Some(kind) {
                    continue;
                }
                let bd = entry.booking_date();
                if bd.year() != year || bd.month() as u8 != month {
                    continue;
                }
                for posting in entry.postings() {
                    if let Some((lf, malo)) = konto.get(&posting.account) {
                        out.push(KindEntry {
                            lf_mp_id: lf.clone(),
                            malo_id: malo.clone(),
                            entry_id: *entry.id().as_uuid(),
                            amount_ct: posting.amount.to_minor(),
                            correlation: entry
                                .provenance()
                                .correlation
                                .as_ref()
                                .map(|l| l.as_str().to_owned()),
                        });
                        break;
                    }
                }
            }
            match page.next {
                Some(next) => cursor = next,
                None => break,
            }
        }
        Ok(out)
    }

    /// Every period seal recorded, oldest first — the Festschreibung history.
    ///
    /// # Errors
    ///
    /// Propagates storage errors.
    pub async fn seals(&self) -> anyhow::Result<Vec<Seal>> {
        self.store
            .seals()
            .await
            .map_err(|e| anyhow::anyhow!("read seals: {e}"))
    }

    /// The date the books are closed through — the latest end date among sealed
    /// periods, or `None` when nothing is sealed yet.
    ///
    /// This, and not the list of sealed periods, is what decides whether a
    /// booking is accepted: every date at or before it is Sealed whether or not
    /// a period covers it. A gap below the watermark is not an opening to book
    /// through — it is a range already committed to — so an operator asking
    /// "can I still book into February" needs this number rather than the
    /// calendar.
    #[must_use]
    pub fn sealed_through(&self) -> Option<Date> {
        self.calendar
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sealed_through()
    }

    /// Verifies the seal chain: every seal is self-consistent and commits to the
    /// prior one, so removing or reordering a sealed period breaks the chain.
    /// Returns the number of seals verified.
    ///
    /// # Errors
    ///
    /// Returns an error when a seal is inconsistent or the chain is broken.
    pub async fn verify_seals(&self) -> anyhow::Result<usize> {
        // The chain is bound to this ledger's identity: a seal names the books
        // it attests to inside its own hash, so a seal lifted from another
        // deployment cannot be pushed onto ours.
        let mut chain = SealChain::new(self.store.ledger().clone());
        for seal in self.seals().await? {
            chain
                .push(seal)
                .map_err(|e| anyhow::anyhow!("seal chain broken: {e}"))?;
        }
        Ok(chain.len())
    }

    /// A tamper-evidence proof that `id` is committed to by the current Merkle
    /// head — the content hash, the `O(log n)` inclusion proof, and the head it
    /// verifies against. An auditor can check it without this crate.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is unknown or not yet sequenced.
    pub async fn prove_entry(
        &self,
        id: EntryId,
    ) -> anyhow::Result<(Hash, InclusionProof, TreeHead)> {
        let stored = self
            .store
            .get(id)
            .await
            .map_err(|e| anyhow::anyhow!("get entry: {e}"))?
            .ok_or_else(|| anyhow::anyhow!("entry not found"))?;
        let index = stored
            .require_index()
            .map_err(|e| anyhow::anyhow!("entry not sequenced: {e}"))?;
        let head = self
            .store
            .head()
            .await
            .map_err(|e| anyhow::anyhow!("head: {e}"))?;
        let proof = self
            .store
            .prove_inclusion(index)
            .await
            .map_err(|e| anyhow::anyhow!("inclusion proof: {e}"))?;
        Ok((stored.content_hash, proof, head))
    }

    /// Proves that a **sealed period closed with a stated balance** for one
    /// customer's Kontokorrent.
    ///
    /// [`Self::prove_entry`] answers "is this booking in the books". A
    /// Betriebsprüfung asks the other question — "what did this customer owe at
    /// the balance-sheet date" — and a number read out of a table is not
    /// evidence for it. This answers it with two proofs that chain:
    ///
    /// 1. the [`BalanceProof`] shows the balance sat in the trial balance the
    ///    seal committed to, for some account *handle*;
    /// 2. the [`AccountBindingProof`] shows that handle was bound to this
    ///    customer's Kontokorrent at the same moment.
    ///
    /// Neither half is sufficient alone: without the binding the handles float,
    /// and re-registering the same accounts in a different order would leave
    /// every balance proof verifying while referring to someone else's account.
    ///
    /// The closing balance is rebuilt **by booking date**, the way the seal
    /// built it — not over a prefix of the log. Those differ in the ordinary
    /// case: sealing January in February means the log already holds February
    /// entries when the seal is taken, so a prefix-folded trial balance would
    /// reproduce a different root and nothing would be provable at all.
    ///
    /// Three answers are possible and only one carries a proof. "No activity in
    /// the period" and "not on the books yet" are both honest replies about
    /// intact books, not failures, so they come back as
    /// [`SealedBalanceOutcome`] variants rather than errors — and they are
    /// different sentences to whoever asked.
    ///
    /// # Errors
    ///
    /// Returns an error when the period is not sealed, the customer has no
    /// Kontokorrent at all, or the rebuilt closing balance does not reproduce
    /// the seal — which means the books were restated beneath a Festschreibung,
    /// and there is then nothing honest to prove.
    pub async fn prove_period_balance(
        &self,
        period_id: &str,
        lf_mp_id: &str,
        malo_id: &str,
    ) -> anyhow::Result<SealedBalanceOutcome<P>> {
        let id = PeriodId::new(period_id).map_err(|e| anyhow::anyhow!("period id: {e}"))?;
        let Some(account) = self.resolve(lf_mp_id, malo_id)? else {
            // Never booked at all, so not an account the registry can speak
            // about either — the same answer the seal would give.
            return Ok(SealedBalanceOutcome::NotYetRegistered);
        };
        let key = BalanceKey {
            account,
            currency: Currency::EUR,
            layer: Layer::Settled,
        };

        // The whole recipe — find the seal, rebuild the closing balance as the
        // seal built it, check the rebuild *against* the seal, prove the row,
        // prove the binding at the registry size the seal recorded — lives in
        // doubleentry so that this service and the ledger cannot drift on it.
        // The check in the middle is the one that matters and the one nothing
        // forces when it is assembled by hand.
        self.store
            .prove_sealed_balance(&id, key)
            .await
            .map_err(|e| anyhow::anyhow!("prove sealed balance: {e}"))
    }

    /// Proves the log has only ever been **appended to** since it had `old_size`
    /// entries — that nothing recorded before then was altered or removed.
    ///
    /// An auditor who archived a head at an earlier visit checks the returned
    /// proof against the head they hold and the head returned here. Without it,
    /// a fresh inclusion proof says only that the ledger is internally
    /// consistent *now*, which a rebuilt ledger would also satisfy.
    ///
    /// # Errors
    ///
    /// Returns an error when `old_size` exceeds the current log.
    pub async fn prove_append_only(
        &self,
        old_size: u64,
    ) -> anyhow::Result<(ConsistencyProof, TreeHead, TreeHead)> {
        let now = self
            .store
            .head()
            .await
            .map_err(|e| anyhow::anyhow!("head: {e}"))?;
        let then = self
            .store
            .head_at(old_size)
            .await
            .map_err(|e| anyhow::anyhow!("head at {old_size}: {e}"))?;
        let proof = self
            .store
            .prove_consistency(old_size)
            .await
            .map_err(|e| anyhow::anyhow!("consistency proof: {e}"))?;
        Ok((proof, then, now))
    }
}

impl Ledger<PostgresStore<P>> {
    /// Connects the deployment's ledger: opens `database_url` with `search_path`
    /// set to the [`LEDGER_SCHEMA`], applies doubleentry's schema, and restores
    /// the account registry. One ledger per deployment, named by `tenant`.
    ///
    /// # Errors
    ///
    /// Propagates connection, migration, and registry errors.
    pub async fn connect(database_url: &str, tenant: &str) -> anyhow::Result<Self> {
        let ledger_id =
            LedgerId::new(tenant).map_err(|e| anyhow::anyhow!("ledger id {tenant}: {e}"))?;
        let store = PostgresStore::<P>::connect_with(database_url, ledger_id, LEDGER_SCHEMA)
            .await
            .map_err(|e| anyhow::anyhow!("connect ledger store: {e}"))?;
        store
            .migrate()
            .await
            .map_err(|e| anyhow::anyhow!("migrate ledger schema: {e}"))?;
        let ledger = Self::load(store).await?;
        // Reconstruct the period calendar from storage so that sealed periods keep
        // rejecting backdated postings across restarts (Festschreibung).
        ledger.refresh_calendar().await?;
        Ok(ledger)
    }

    /// Seals a period (**Festschreibung** — GoBD / § 146 AO / § 239 HGB): defines
    /// and closes it, then commits to which entries it contains and what they add
    /// up to, as chained Merkle roots. After this, `post` rejects a backdated
    /// booking into the period — corrections must book into a later open period.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid period id/range or a storage failure.
    pub async fn seal_period(
        &self,
        period_id: &str,
        start: Date,
        end: Date,
    ) -> anyhow::Result<Seal> {
        let id = PeriodId::new(period_id).map_err(|e| anyhow::anyhow!("period id: {e}"))?;
        let period = Period::new(id.clone(), start, end)
            .map_err(|e| anyhow::anyhow!("period range: {e}"))?;

        // The store owns period state. Defining is idempotent for an unchanged
        // range, so a re-run of a close that failed verification does not need
        // to know whether the period had been defined before.
        self.store
            .define_period(&period)
            .await
            .map_err(|e| anyhow::anyhow!("define period: {e}"))?;
        self.store
            .transition_period(&id, PeriodState::Closing)
            .await
            .map_err(|e| anyhow::anyhow!("close period: {e}"))?;

        // Seals in the same storage transaction that advances the period to
        // Sealed, so a seal is never recorded for a period the books still
        // consider open.
        let seal = self
            .store
            .seal_period(&id)
            .await
            .map_err(|e| anyhow::anyhow!("seal period: {e}"))?;

        self.refresh_calendar().await?;
        Ok(seal)
    }

    /// Re-reads the period calendar from storage into the in-process mirror that
    /// [`Self::post`] validates against.
    ///
    /// The mirror is what makes a backdated booking into a sealed period fail
    /// without a database round-trip per entry; storage stays the authority for
    /// what is sealed.
    ///
    /// # Errors
    ///
    /// Propagates storage errors and an inconsistent stored calendar.
    async fn refresh_calendar(&self) -> anyhow::Result<()> {
        let periods = self
            .store
            .periods()
            .await
            .map_err(|e| anyhow::anyhow!("read periods: {e}"))?;
        let calendar = PeriodCalendar::from_periods(periods)
            .map_err(|e| anyhow::anyhow!("rebuild period calendar: {e}"))?;
        *self.calendar.lock().unwrap_or_else(|e| e.into_inner()) = calendar;
        Ok(())
    }
}

/// Persist an account through any [`LedgerStore`], mapping the store error.
async fn store_register_account<S: LedgerStore<P>>(
    store: &S,
    record: &AccountRecord,
) -> anyhow::Result<()> {
    store
        .register_account(record)
        .await
        .map_err(|e| anyhow::anyhow!("persist account: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use doubleentry::period::LedgerId;
    use doubleentry::storage::MemoryStore;
    use time::macros::date;

    async fn test_ledger() -> Ledger<MemoryStore<P>> {
        let store = MemoryStore::<P>::new(LedgerId::new("test-tenant").expect("ledger id"));
        Ledger::load(store).await.expect("load ledger")
    }

    #[tokio::test]
    async fn invoice_then_payment_nets_to_zero() {
        let ledger = test_ledger().await;
        let (malo, lf) = ("51238696781", "9900000000001");

        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "RECHNUNG",
                11900,
                "inv-1",
                date!(2026 - 03 - 15),
            ))
            .await
            .expect("invoice");
        assert_eq!(ledger.balance_ct(lf, malo).await.unwrap(), 11900);

        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "ZAHLUNG",
                -11900,
                "pay-1",
                date!(2026 - 03 - 20),
            ))
            .await
            .expect("payment");
        assert_eq!(ledger.balance_ct(lf, malo).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn replay_is_idempotent_and_returns_original() {
        let ledger = test_ledger().await;
        let (malo, lf) = ("51238696781", "9900000000001");
        let entry = PostEntry::new(malo, lf, "RECHNUNG", 5000, "inv-dup", date!(2026 - 03 - 15));

        let first = ledger.post(entry.clone()).await.expect("first");
        assert!(first.is_new);
        let second = ledger.post(entry).await.expect("replay");
        assert!(!second.is_new, "identical replay must be a no-op");
        assert_eq!(first.id, second.id, "replay returns the original entry id");
        // Booked once, not twice.
        assert_eq!(ledger.balance_ct(lf, malo).await.unwrap(), 5000);
    }

    #[tokio::test]
    async fn conflicting_key_is_refused() {
        let ledger = test_ledger().await;
        let (malo, lf) = ("51238696781", "9900000000001");
        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "RECHNUNG",
                5000,
                "k",
                date!(2026 - 03 - 15),
            ))
            .await
            .expect("first");
        // Same idempotency key, different amount → conflict, never a second entry.
        let conflict = ledger
            .post(PostEntry::new(
                malo,
                lf,
                "RECHNUNG",
                9999,
                "k",
                date!(2026 - 03 - 15),
            ))
            .await;
        assert!(
            conflict.is_err(),
            "reused key with different content must be refused"
        );
        assert_eq!(ledger.balance_ct(lf, malo).await.unwrap(), 5000);
    }

    #[tokio::test]
    async fn eeg_gutschrift_credits_the_operator() {
        let ledger = test_ledger().await;
        let (malo, lf) = ("51111111111", "9900000000001");
        // We owe the plant operator: their Kontokorrent goes credit (negative).
        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "EEG_GUTSCHRIFT",
                -25000,
                "eeg-1",
                date!(2026 - 03 - 31),
            ))
            .await
            .expect("eeg");
        assert_eq!(ledger.balance_ct(lf, malo).await.unwrap(), -25000);
    }

    #[tokio::test]
    async fn zero_amount_is_refused() {
        let ledger = test_ledger().await;
        let err = ledger
            .post(PostEntry::new(
                "5",
                "9",
                "RECHNUNG",
                0,
                "z",
                date!(2026 - 03 - 15),
            ))
            .await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn all_entry_types_balance() {
        // Every mapped entry_type produces a balanced entry the ledger accepts.
        let ledger = test_ledger().await;
        let (malo, lf) = ("51238696781", "9900000000001");
        let cases = [
            ("RECHNUNG", 10000),
            ("ABSCHLAG", -3000),
            ("ZAHLUNG", -2000),
            ("MAHNGEBUEHR", 500),
            ("BANKRUECKLAST", 2000),
            ("GUTSCHRIFT", -1000),
            ("STORNO", -10000),
            ("KORREKTUR", 700),
            ("JAHRESABSCHLUSS", 1200),
        ];
        for (i, (kind, amount)) in cases.iter().enumerate() {
            ledger
                .post(PostEntry::new(
                    malo,
                    lf,
                    *kind,
                    *amount,
                    format!("k-{i}"),
                    date!(2026 - 03 - 15),
                ))
                .await
                .unwrap_or_else(|e| panic!("{kind} should post: {e}"));
        }
    }

    #[tokio::test]
    async fn fifo_clearing_matches_oldest_first_and_resets() {
        let ledger = test_ledger().await;
        let (malo, lf) = ("51238696781", "9900000000001");
        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "RECHNUNG",
                10000,
                "inv1",
                date!(2026 - 01 - 01),
            ))
            .await
            .unwrap();
        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "RECHNUNG",
                20000,
                "inv2",
                date!(2026 - 02 - 01),
            ))
            .await
            .unwrap();
        ledger
            .post(PostEntry::new(
                malo,
                lf,
                "ZAHLUNG",
                -15000,
                "pay",
                date!(2026 - 02 - 15),
            ))
            .await
            .unwrap();

        // Before clearing: both invoices are gross-open.
        assert_eq!(ledger.open_receivables(lf, malo).await.unwrap().len(), 2);

        let cid = ledger
            .apply_fifo_clearing(lf, malo, date!(2026 - 02 - 15))
            .await
            .unwrap()
            .expect("a match is recorded");

        // Oldest invoice fully cleared; the second keeps its 5000 residual.
        let open = ledger.open_receivables(lf, malo).await.unwrap();
        assert_eq!(open.len(), 1, "oldest invoice fully cleared");
        assert_eq!(open[0].outstanding_ct, 15000, "second invoice partly open");

        // A re-run matches nothing more.
        assert!(
            ledger
                .apply_fifo_clearing(lf, malo, date!(2026 - 02 - 16))
                .await
                .unwrap()
                .is_none()
        );

        // Resetting the clearing re-opens both invoices (gross again).
        ledger
            .reset_clearing(cid, date!(2026 - 02 - 17))
            .await
            .unwrap();
        assert_eq!(ledger.open_receivables(lf, malo).await.unwrap().len(), 2);
    }
}
