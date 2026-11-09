//! EEG payouts per receipt chain.
//!
//! `einsd` announces every settlement receipt it issues as
//! `de.eeg.verguetung.berechnet` or `de.eeg.marktpraemie.berechnet`. A
//! settlement period — plant (`tr_id`), billing year and month — is one
//! **receipt chain**: the initial receipt and every § 147 AO correction after
//! it each state the *whole* amount owed for the period. Two rules follow, and
//! both are kept here:
//!
//! - **The ledger books what a receipt changes.** The initial receipt credits
//!   its amount, a correction the difference to the chain's current receipt,
//!   and a receipt overtaken by a later one (a retried delivery) nothing —
//!   [`gutschrift_delta`]. Booking a correction's whole amount credits the
//!   period twice.
//! - **A payout pays what is outstanding.** The chain's current receipt less
//!   what was already paid or ordered for it — every payout order the bank has
//!   not rejected and that was not cancelled, and every Aufrechnung, which
//!   § 13 Abs. 1 Satz 2 EnFG counts as paid, less every Rückzahlung. A lower
//!   correction pays nothing and leaves the overpayment as the plant
//!   operator's debt — [`ausgleich`] — until the operator pays it back
//!   ([`erfasse_rueckzahlung`]). The Vergütung and the Marktprämie are paid by
//!   the same rule ([`zahle`]).
//!
//! The Kontokorrent follows the money: a receipt credits the operator, a
//! payout the bank confirms with `ACSC` debits it ([`bestaetige_auszahlung`]),
//! and a Rückzahlung credits it again — so the account shows what is left
//! owed either way, never the claim beside its own payment.
//!
//! The chain's current receipt is the one einsd wrote last — the
//! `ketten_folge` einsd assigns in the write that records the receipt — not the
//! one that arrived last, and not the one whose CloudEvent `time` is latest,
//! which two einsd replicas' clocks can put in either order. So a redelivery
//! overtaken by the next receipt cannot reopen the period at its old amount.
//! Every write to a chain runs under a per-chain advisory lock, which is what
//! makes a replayed receipt pay nothing: under the lock it finds nothing
//! outstanding.

use sqlx::{PgConnection, Row as _};
use time::Date;
use uuid::Uuid;

use crate::config::AccountingdConfig;

/// Which einsd event announced a receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    /// `de.eeg.verguetung.berechnet`.
    Verguetung,
    /// `de.eeg.marktpraemie.berechnet` — the § 20 EEG 2023 Marktprämie, which
    /// the Netzbetreiber owes the plant operator (§ 19 Abs. 1 Nr. 1).
    Marktpraemie,
}

impl Art {
    /// The storage token (`eeg_belege.art`).
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Verguetung => "VERGUETUNG",
            Self::Marktpraemie => "MARKTPRAEMIE",
        }
    }

    /// Parse a storage token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        [Self::Verguetung, Self::Marktpraemie]
            .into_iter()
            .find(|a| a.token() == token)
    }

    /// The Buchungsart its ledger entry is booked as.
    #[must_use]
    pub const fn entry_type(self) -> &'static str {
        match self {
            Self::Verguetung => "EEG_GUTSCHRIFT",
            Self::Marktpraemie => "EEG_MARKTPRAEMIE",
        }
    }

    /// The entry's line description.
    #[must_use]
    pub const fn beschreibung(self) -> &'static str {
        match self {
            Self::Verguetung => "EEG Einspeisevergütung §21 EEG",
            Self::Marktpraemie => "EEG Direktvermarktung Marktprämie §20 EEG",
        }
    }
}

/// What a receipt chain earns, given what was paid for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ausgleich {
    /// What is still to be paid out, in ct; `0` when nothing is.
    pub auszahlung_ct: i64,
    /// What the plant operator owes back, in ct; `0` when nothing is.
    pub rueckforderung_ct: i64,
}

/// The difference rule: the current receipt's amount `anspruch_ct` against what
/// was already paid or ordered for the chain, `geleistet_ct`.
///
/// Never more than the difference is paid, and an amount paid beyond the
/// current receipt is owed back, never offset by paying less on another chain.
#[must_use]
pub fn ausgleich(anspruch_ct: i64, geleistet_ct: i64) -> Ausgleich {
    let offen = anspruch_ct.saturating_sub(geleistet_ct);
    if offen > 0 {
        Ausgleich {
            auszahlung_ct: offen,
            rueckforderung_ct: 0,
        }
    } else {
        Ausgleich {
            auszahlung_ct: 0,
            rueckforderung_ct: offen.saturating_neg(),
        }
    }
}

/// A chain's current receipt, as [`gutschrift_delta`] compares against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aktuell {
    pub anspruch_ct: i64,
    pub ketten_folge: i64,
}

/// What a newly received receipt changes the chain's claim by — the amount its
/// ledger entry books, positive for a credit to the plant operator.
///
/// The chain's first receipt is its whole amount; one einsd wrote after the
/// current receipt (a higher `ketten_folge`) is the difference to it; one
/// written before — overtaken in delivery — changes nothing.
#[must_use]
pub fn gutschrift_delta(anspruch_ct: i64, ketten_folge: i64, aktuell: Option<Aktuell>) -> i64 {
    match aktuell {
        None => anspruch_ct,
        Some(a) if ketten_folge > a.ketten_folge => anspruch_ct.saturating_sub(a.anspruch_ct),
        Some(_) => 0,
    }
}

/// One receipt, as the CloudEvent announced it.
#[derive(Debug, Clone)]
pub struct Beleg {
    pub source_ce_id: String,
    pub art: Art,
    pub receipt_id: Uuid,
    pub korrektur_von: Option<Uuid>,
    pub tr_id: String,
    pub malo_id: String,
    pub account_id: Uuid,
    pub billing_year: i16,
    pub billing_month: i16,
    /// The receipt's settlement, in ct (kaufmännisch rounded).
    pub anspruch_ct: i64,
    /// Its place in the chain, as einsd wrote it.
    pub ketten_folge: i64,
    pub bank_iban: Option<String>,
    pub zahlungsempfaenger: Option<String>,
}

/// What recording a receipt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Erfasst {
    /// `false` for a replay of a receipt already recorded.
    pub neu: bool,
    /// What its ledger entry books (positive = credit). A draft is staged under
    /// the CloudEvent id exactly when this is non-zero.
    pub gutschrift_delta_ct: i64,
}

/// Why a receipt was not recorded.
#[derive(Debug, thiserror::Error)]
pub enum ErfassFehler {
    /// Another receipt already holds the chain position this one claims, so it
    /// is not a receipt einsd wrote.
    #[error(
        "receipt chain {tr_id} {year:04}-{month:02}: position {ketten_folge} is already held by \
         CloudEvent {belegt_von}"
    )]
    FolgeVergeben {
        tr_id: String,
        year: i16,
        month: i16,
        ketten_folge: i64,
        belegt_von: String,
    },
    #[error(transparent)]
    Technisch(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ErfassFehler {
    fn from(e: sqlx::Error) -> Self {
        Self::Technisch(e.into())
    }
}

/// Serialise every write to one chain for the rest of the transaction.
async fn sperre_kette(
    conn: &mut PgConnection,
    tenant: &str,
    tr_id: &str,
    year: i16,
    month: i16,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("eeg-kette:{tenant}:{tr_id}:{year}:{month}"))
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Record `beleg` and stage its ledger entry, in the caller's transaction.
///
/// A replay returns what the first delivery recorded and stages nothing new.
/// The caller books the staged draft (`booking::book` under the CloudEvent id)
/// after the commit when [`Erfasst::gutschrift_delta_ct`] is non-zero.
///
/// # Errors
///
/// [`ErfassFehler::FolgeVergeben`] for a second receipt at a taken chain
/// position; a database failure, or a draft the ledger would refuse.
pub async fn erfasse(
    conn: &mut PgConnection,
    ledger: &crate::ledger::PgLedger,
    tenant: &str,
    lf_mp_id: &str,
    beleg: &Beleg,
    heute: Date,
) -> Result<Erfasst, ErfassFehler> {
    sperre_kette(
        conn,
        tenant,
        &beleg.tr_id,
        beleg.billing_year,
        beleg.billing_month,
    )
    .await?;
    let bekannt: Option<i64> = sqlx::query_scalar(
        "SELECT gutschrift_delta_ct FROM eeg_belege WHERE tenant = $1 AND source_ce_id = $2",
    )
    .bind(tenant)
    .bind(&beleg.source_ce_id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(delta) = bekannt {
        return Ok(Erfasst {
            neu: false,
            gutschrift_delta_ct: delta,
        });
    }
    let belegt_von: Option<String> = sqlx::query_scalar(
        "SELECT source_ce_id FROM eeg_belege
          WHERE tenant = $1 AND tr_id = $2 AND billing_year = $3 AND billing_month = $4
            AND ketten_folge = $5",
    )
    .bind(tenant)
    .bind(&beleg.tr_id)
    .bind(beleg.billing_year)
    .bind(beleg.billing_month)
    .bind(beleg.ketten_folge)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(belegt_von) = belegt_von {
        return Err(ErfassFehler::FolgeVergeben {
            tr_id: beleg.tr_id.clone(),
            year: beleg.billing_year,
            month: beleg.billing_month,
            ketten_folge: beleg.ketten_folge,
            belegt_von,
        });
    }
    let aktuell = sqlx::query(
        "SELECT anspruch_ct, ketten_folge FROM eeg_belege
          WHERE tenant = $1 AND tr_id = $2 AND billing_year = $3 AND billing_month = $4
          ORDER BY ketten_folge DESC
          LIMIT 1",
    )
    .bind(tenant)
    .bind(&beleg.tr_id)
    .bind(beleg.billing_year)
    .bind(beleg.billing_month)
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| Aktuell {
        anspruch_ct: r.get("anspruch_ct"),
        ketten_folge: r.get("ketten_folge"),
    });
    let delta = gutschrift_delta(beleg.anspruch_ct, beleg.ketten_folge, aktuell);
    sqlx::query(
        "INSERT INTO eeg_belege
             (tenant, source_ce_id, art, receipt_id, korrektur_von, tr_id, malo_id, account_id,
              billing_year, billing_month, anspruch_ct, gutschrift_delta_ct, ketten_folge,
              bank_iban, zahlungsempfaenger)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(tenant)
    .bind(&beleg.source_ce_id)
    .bind(beleg.art.token())
    .bind(beleg.receipt_id)
    .bind(beleg.korrektur_von)
    .bind(&beleg.tr_id)
    .bind(&beleg.malo_id)
    .bind(beleg.account_id)
    .bind(beleg.billing_year)
    .bind(beleg.billing_month)
    .bind(beleg.anspruch_ct)
    .bind(delta)
    .bind(beleg.ketten_folge)
    .bind(beleg.bank_iban.as_deref())
    .bind(beleg.zahlungsempfaenger.as_deref())
    .execute(&mut *conn)
    .await?;
    if delta != 0 {
        let draft = crate::pg::draft(
            &beleg.malo_id,
            lf_mp_id,
            beleg.art.entry_type(),
            // The Kontokorrent sign: a credit to the operator is negative.
            delta.saturating_neg(),
            &beleg.source_ce_id,
            Some(&beleg.source_ce_id),
            Some(&beleg.tr_id),
            heute,
            heute,
            Some(beleg.art.beschreibung()),
            None,
        );
        crate::booking::stage(conn, ledger, tenant, &draft)
            .await
            .map_err(anyhow::Error::from)?;
    }
    Ok(Erfasst {
        neu: true,
        gutschrift_delta_ct: delta,
    })
}

/// One row of `eeg_ketten`: a chain's current receipt and what was paid for it.
#[derive(Debug, Clone)]
pub struct Kette {
    pub tr_id: String,
    pub malo_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub account_id: Uuid,
    pub art: Art,
    pub receipt_id: Uuid,
    pub source_ce_id: String,
    pub anspruch_ct: i64,
    pub geleistet_ct: i64,
    pub bank_iban: Option<String>,
    pub zahlungsempfaenger: Option<String>,
}

impl Kette {
    /// [`ausgleich`] of this chain.
    #[must_use]
    pub fn ausgleich(&self) -> Ausgleich {
        ausgleich(self.anspruch_ct, self.geleistet_ct)
    }
}

fn kette(r: &sqlx::postgres::PgRow) -> Result<Kette, sqlx::Error> {
    let token: String = r.try_get("art")?;
    let art = Art::from_token(&token).ok_or_else(|| sqlx::Error::ColumnDecode {
        index: "art".to_owned(),
        source: format!("eeg_belege.art {token:?} unknown").into(),
    })?;
    Ok(Kette {
        tr_id: r.try_get("tr_id")?,
        malo_id: r.try_get("malo_id")?,
        billing_year: r.try_get("billing_year")?,
        billing_month: r.try_get("billing_month")?,
        account_id: r.try_get("account_id")?,
        art,
        receipt_id: r.try_get("receipt_id")?,
        source_ce_id: r.try_get("source_ce_id")?,
        anspruch_ct: r.try_get("anspruch_ct")?,
        geleistet_ct: r.try_get("geleistet_ct")?,
        bank_iban: r.try_get("bank_iban")?,
        zahlungsempfaenger: r.try_get("zahlungsempfaenger")?,
    })
}

/// The receipt chains of one billing month — Vergütung and Marktprämie —
/// optionally of one MaLo: what the payout run walks.
///
/// # Errors
///
/// A database failure.
pub async fn ketten_im_monat(
    pool: &sqlx::PgPool,
    tenant: &str,
    year: i16,
    month: i16,
    malo_id: Option<&str>,
) -> Result<Vec<Kette>, sqlx::Error> {
    // Each `eeg_ketten` statement is a plain literal, so `cargo xtask check-sql`
    // prepares it against the schema.
    sqlx::query(
        "SELECT tr_id, malo_id, billing_year, billing_month, account_id, art, receipt_id,
                source_ce_id, anspruch_ct, geleistet_ct, bank_iban, zahlungsempfaenger
           FROM eeg_ketten
          WHERE tenant = $1 AND billing_year = $2 AND billing_month = $3
            AND ($4::text IS NULL OR malo_id = $4)
          ORDER BY tr_id",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .bind(malo_id)
    .fetch_all(pool)
    .await?
    .iter()
    .map(kette)
    .collect()
}

/// Every chain whose plant operator owes money back: paid or ordered beyond the
/// current receipt.
///
/// # Errors
///
/// A database failure.
pub async fn rueckforderungen(
    pool: &sqlx::PgPool,
    tenant: &str,
) -> Result<Vec<Kette>, sqlx::Error> {
    sqlx::query(
        "SELECT tr_id, malo_id, billing_year, billing_month, account_id, art, receipt_id,
                source_ce_id, anspruch_ct, geleistet_ct, bank_iban, zahlungsempfaenger
           FROM eeg_ketten
          WHERE tenant = $1 AND geleistet_ct > anspruch_ct
          ORDER BY billing_year, billing_month, tr_id",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?
    .iter()
    .map(kette)
    .collect()
}

/// A payout order written, for the caller to submit after the commit.
#[derive(Debug, Clone)]
pub struct Auftrag {
    pub payout_id: Uuid,
    pub end_to_end_ref: String,
    pub amount_ct: i64,
    pub pain001_xml: String,
}

/// What paying a chain did.
#[derive(Debug, Clone)]
pub enum Zahlung {
    /// An order for the outstanding difference was written.
    Auftrag(Auftrag),
    /// Nothing is outstanding; `rueckforderung_ct` is what the operator owes back.
    NichtsOffen { rueckforderung_ct: i64 },
    /// Something is outstanding, but no creditor IBAN is known — neither in the
    /// receipt nor in the account's zahlungsinformation. The chain stays
    /// outstanding for the next run.
    OhneKonto { offen_ct: i64 },
    /// No receipt is recorded for the chain.
    KeinBeleg,
}

/// Why a payout that is owed could not be ordered.
#[derive(Debug, thiserror::Error)]
pub enum AuszahlungFehler {
    /// `[eeg]` / `pain001_schema` cannot produce a payment — refused at startup
    /// by `Daemon::validate`, so reaching it here means the check was bypassed.
    #[error("EEG payout configuration: {0}")]
    Konfiguration(String),
    /// The pain.001 for the order could not be built (an invalid creditor IBAN,
    /// say). Nothing is written; the chain stays outstanding.
    #[error("pain.001 for plant {tr_id}: {grund}")]
    Pain001 { tr_id: String, grund: String },
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Pay what chain (`tr_id`, `year`, `month`) has outstanding, in the caller's
/// transaction. The caller commits, then submits [`Auftrag::pain001_xml`].
///
/// # Errors
///
/// [`AuszahlungFehler`] when an outstanding payout could not be ordered.
pub async fn zahle(
    conn: &mut PgConnection,
    cfg: &AccountingdConfig,
    tr_id: &str,
    year: i16,
    month: i16,
    instant: bool,
    heute: Date,
) -> Result<Zahlung, AuszahlungFehler> {
    let tenant = cfg.tenant.as_str();
    sperre_kette(conn, tenant, tr_id, year, month).await?;
    let Some(row) = sqlx::query(
        "SELECT tr_id, malo_id, billing_year, billing_month, account_id, art, receipt_id,
                source_ce_id, anspruch_ct, geleistet_ct, bank_iban, zahlungsempfaenger
           FROM eeg_ketten
          WHERE tenant = $1 AND tr_id = $2 AND billing_year = $3 AND billing_month = $4",
    )
    .bind(tenant)
    .bind(tr_id)
    .bind(year)
    .bind(month)
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(Zahlung::KeinBeleg);
    };
    let k = kette(&row)?;
    let a = k.ausgleich();
    if a.auszahlung_ct == 0 {
        return Ok(Zahlung::NichtsOffen {
            rueckforderung_ct: a.rueckforderung_ct,
        });
    }

    let account = sqlx::query_as::<_, crate::pg::AccountRow>(
        "SELECT * FROM accounts WHERE account_id = $1 AND tenant = $2",
    )
    .bind(k.account_id)
    .bind(tenant)
    .fetch_optional(&mut *conn)
    .await?;
    let zahlungsinformation: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT zahlungsinformation FROM accounts WHERE account_id = $1 AND tenant = $2",
    )
    .bind(k.account_id)
    .bind(tenant)
    .fetch_optional(&mut *conn)
    .await?
    .flatten();
    let zi = zahlungsinformation.as_ref();
    // Creditor: the IBAN einsd forwarded with the receipt, else the account's
    // stored zahlungsinformation.
    let creditor_iban = k.bank_iban.clone().or_else(|| {
        zi.and_then(|z| z.get("bankverbindung"))
            .and_then(|b| b.get("iban"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    });
    let Some(creditor_iban) = creditor_iban else {
        return Ok(Zahlung::OhneKonto {
            offen_ct: a.auszahlung_ct,
        });
    };
    let creditor_name = k
        .zahlungsempfaenger
        .clone()
        .or_else(|| {
            zi.and_then(|z| z.get("kontoinhaber"))
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "EEG Einspeiser".to_owned());
    // `Cdtr/PstlAdr` — the plant operator's own address, from the account's
    // master data (BO4E's Zahlungsinformation carries none).
    let creditor_address = account.map(|acc| acc.postal_address()).unwrap_or_default();

    let debtor_iban =
        cfg.eeg.debtor_iban.as_deref().ok_or_else(|| {
            AuszahlungFehler::Konfiguration("[eeg].debtor_iban is not set".into())
        })?;
    let schema = crate::sepa::resolve_pain001_schema(cfg.pain001_schema.as_deref())
        .map_err(|e| AuszahlungFehler::Konfiguration(e.to_string()))?;

    let payout_id = Uuid::new_v4();
    // ISO 20022 EndToEndId, at most 35 characters: 31 here.
    let end_to_end_ref = format!(
        "EEG-{}-{year:04}-{month:02}-{}",
        &k.malo_id[..k.malo_id.len().min(10)],
        &payout_id.simple().to_string()[..8]
    );
    let payment_type = if instant { "SCT_INST" } else { "SCT_CORE" };
    // §25 Abs. 1 EEG 2023 — "unverzüglich nach Ende des Monats": the payout
    // leaves today, as the German calendar counts it.
    let pain001_xml = crate::sepa::build_pain_001(
        &crate::sepa::DebtorIdentity {
            iban: debtor_iban,
            name: cfg.creditor_name.as_deref().unwrap_or(tenant),
            address: Some(&cfg.creditor_address),
        },
        &[crate::sepa::CreditTransferItem {
            iban: &creditor_iban,
            name: &creditor_name,
            amount_ct: a.auszahlung_ct,
            end_to_end_ref: &end_to_end_ref,
            address: Some(&creditor_address),
        }],
        heute,
        instant,
        schema,
    )
    .map_err(|e| AuszahlungFehler::Pain001 {
        tr_id: tr_id.to_owned(),
        grund: e.to_string(),
    })?;

    // The creditor address is snapshotted beside the IBAN and name: what was
    // sent has to stay readable after the account's master data moves on.
    sqlx::query(
        "INSERT INTO eeg_payout_orders
             (payout_id, malo_id, account_id, tr_id, billing_year, billing_month, amount_ct,
              creditor_iban, creditor_name, payment_type, end_to_end_ref,
              pain001_xml, source_ce_id, tenant,
              creditor_town, creditor_country, creditor_street,
              creditor_building_number, creditor_post_code,
              creditor_country_subdivision)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20)",
    )
    .bind(payout_id)
    .bind(&k.malo_id)
    .bind(k.account_id)
    .bind(&k.tr_id)
    .bind(year)
    .bind(month)
    .bind(a.auszahlung_ct)
    .bind(&creditor_iban)
    .bind(&creditor_name)
    .bind(payment_type)
    .bind(&end_to_end_ref)
    .bind(&pain001_xml)
    .bind(&k.source_ce_id)
    .bind(tenant)
    .bind(&creditor_address.town)
    .bind(&creditor_address.country)
    .bind(&creditor_address.street)
    .bind(&creditor_address.building_number)
    .bind(&creditor_address.post_code)
    .bind(&creditor_address.country_subdivision)
    .execute(&mut *conn)
    .await?;
    Ok(Zahlung::Auftrag(Auftrag {
        payout_id,
        end_to_end_ref,
        amount_ct: a.auszahlung_ct,
        pain001_xml,
    }))
}

/// Book a payout the bank confirmed settled (`ACSC`) on the plant's
/// Kontokorrent, in the caller's transaction: the order's amount debits the
/// operator's credit, so the account shows what is still owed rather than the
/// claim beside its own payment. Returns the draft's key, for the caller to
/// book after the commit.
///
/// The caller stages this once, on the order's transition to `ACSC`; the key
/// is the payout, so a second staging of the same draft is a no-op.
///
/// # Errors
///
/// A database failure, an unknown order, or a draft the ledger would refuse.
pub async fn bestaetige_auszahlung(
    conn: &mut PgConnection,
    ledger: &crate::ledger::PgLedger,
    tenant: &str,
    payout_id: Uuid,
    heute: Date,
) -> anyhow::Result<String> {
    let row = sqlx::query(
        "SELECT malo_id, tr_id, amount_ct, end_to_end_ref FROM eeg_payout_orders
          WHERE payout_id = $1 AND tenant = $2",
    )
    .bind(payout_id)
    .bind(tenant)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("no EEG payout order {payout_id}"))?;
    let malo_id: String = row.try_get("malo_id")?;
    let tr_id: String = row.try_get("tr_id")?;
    let amount_ct: i64 = row.try_get("amount_ct")?;
    let e2e: String = row.try_get("end_to_end_ref")?;
    let key = format!("eeg-auszahlung:{payout_id}");
    let draft = crate::pg::draft(
        &malo_id,
        tenant,
        "EEG_AUSZAHLUNG",
        amount_ct,
        &key,
        Some(&e2e),
        Some(&tr_id),
        heute,
        heute,
        Some("EEG Auszahlung (pain.002 ACSC)"),
        None,
    );
    crate::booking::stage(conn, ledger, tenant, &draft).await?;
    Ok(key)
}

/// A plant operator's repayment of an overpaid receipt chain, as the operator
/// of accountingd records it.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RueckzahlungRequest {
    /// The plant and billing month of the overpaid chain.
    pub tr_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    /// The amount received, in euro cent; positive, at most what is owed back.
    pub amount_ct: i64,
    /// The day the money arrived, `YYYY-MM-DD`.
    pub eingegangen_am: String,
    pub vermerk: Option<String>,
}

/// Why a repayment is refused before it is stored.
#[derive(Debug, thiserror::Error)]
pub enum RueckzahlungFehler {
    #[error("{0}")]
    Ungueltig(String),
    /// The chain owes nothing back — none is recorded, or it is paid up.
    #[error("plant {tr_id} {year:04}-{month:02} owes nothing back")]
    NichtsZurueck {
        tr_id: String,
        year: i16,
        month: i16,
    },
    #[error("a Rückzahlung for this plant, month and day is already recorded")]
    Doppelt,
    #[error(transparent)]
    Technisch(#[from] anyhow::Error),
}

impl From<sqlx::Error> for RueckzahlungFehler {
    fn from(e: sqlx::Error) -> Self {
        Self::Technisch(e.into())
    }
}

/// A recorded repayment and the key of its staged booking.
#[derive(Debug, Clone)]
pub struct Rueckzahlung {
    pub rueckzahlung_id: Uuid,
    /// The booking draft's key, for the caller to book after the commit.
    pub buchung: String,
    /// What the chain still owes back after it, in ct.
    pub verbleibend_ct: i64,
}

/// Record a repayment against chain (`tr_id`, year, month) and stage its
/// `EEG_RUECKZAHLUNG` booking, in the caller's transaction, under the chain's
/// lock — so it is never more than the chain owes back at that moment.
///
/// # Errors
///
/// [`RueckzahlungFehler::Ungueltig`] for a malformed request or an amount above
/// what is owed back; [`RueckzahlungFehler::NichtsZurueck`] for a chain that
/// owes nothing; [`RueckzahlungFehler::Doppelt`] for a second repayment of the
/// chain on the same day.
pub async fn erfasse_rueckzahlung(
    conn: &mut PgConnection,
    ledger: &crate::ledger::PgLedger,
    tenant: &str,
    req: &RueckzahlungRequest,
    erfasst_von: Option<&str>,
    heute: Date,
) -> Result<Rueckzahlung, RueckzahlungFehler> {
    if req.tr_id.trim().is_empty() {
        return Err(RueckzahlungFehler::Ungueltig("tr_id is empty".to_owned()));
    }
    if req.amount_ct <= 0 {
        return Err(RueckzahlungFehler::Ungueltig(
            "amount_ct must be positive — the amount received".to_owned(),
        ));
    }
    if !(1..=12).contains(&req.billing_month) {
        return Err(RueckzahlungFehler::Ungueltig(
            "billing_month must be 1–12".to_owned(),
        ));
    }
    let eingegangen_am = mako_service::dates::parse_iso_date("eingegangen_am", &req.eingegangen_am)
        .map_err(|e| RueckzahlungFehler::Ungueltig(e.to_string()))?;
    if eingegangen_am > heute {
        return Err(RueckzahlungFehler::Ungueltig(format!(
            "eingegangen_am {eingegangen_am} lies in the future — a Rückzahlung is recorded once received"
        )));
    }
    let (year, month) = (req.billing_year, req.billing_month);
    sperre_kette(conn, tenant, &req.tr_id, year, month).await?;
    let nichts = || RueckzahlungFehler::NichtsZurueck {
        tr_id: req.tr_id.clone(),
        year,
        month,
    };
    let row = sqlx::query(
        "SELECT tr_id, malo_id, billing_year, billing_month, account_id, art, receipt_id,
                source_ce_id, anspruch_ct, geleistet_ct, bank_iban, zahlungsempfaenger
           FROM eeg_ketten
          WHERE tenant = $1 AND tr_id = $2 AND billing_year = $3 AND billing_month = $4",
    )
    .bind(tenant)
    .bind(&req.tr_id)
    .bind(year)
    .bind(month)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(nichts)?;
    let k = kette(&row)?;
    let offen = k.ausgleich().rueckforderung_ct;
    if offen == 0 {
        return Err(nichts());
    }
    if req.amount_ct > offen {
        return Err(RueckzahlungFehler::Ungueltig(format!(
            "amount_ct {} exceeds the {offen} ct plant {} owes back for {year:04}-{month:02}",
            req.amount_ct, req.tr_id
        )));
    }
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO eeg_rueckzahlungen
             (tenant, tr_id, malo_id, billing_year, billing_month, amount_ct,
              eingegangen_am, vermerk, erfasst_von)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (tenant, tr_id, billing_year, billing_month, eingegangen_am) DO NOTHING
         RETURNING rueckzahlung_id",
    )
    .bind(tenant)
    .bind(&req.tr_id)
    .bind(&k.malo_id)
    .bind(year)
    .bind(month)
    .bind(req.amount_ct)
    .bind(eingegangen_am)
    .bind(req.vermerk.as_deref())
    .bind(erfasst_von)
    .fetch_optional(&mut *conn)
    .await?;
    let id = id.ok_or(RueckzahlungFehler::Doppelt)?;
    let key = format!("eeg-rueckzahlung:{id}");
    let draft = crate::pg::draft(
        &k.malo_id,
        tenant,
        "EEG_RUECKZAHLUNG",
        // The Kontokorrent sign: money received from the operator is a credit.
        req.amount_ct.saturating_neg(),
        &key,
        None,
        Some(&req.tr_id),
        heute,
        eingegangen_am,
        Some("EEG Rückzahlung einer Überzahlung"),
        erfasst_von,
    );
    crate::booking::stage(conn, ledger, tenant, &draft)
        .await
        .map_err(anyhow::Error::from)?;
    Ok(Rueckzahlung {
        rueckzahlung_id: id,
        buchung: key,
        verbleibend_ct: offen - req.amount_ct,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_higher_correction_pays_the_difference() {
        assert_eq!(
            ausgleich(11_000, 10_000),
            Ausgleich {
                auszahlung_ct: 1_000,
                rueckforderung_ct: 0
            }
        );
    }

    #[test]
    fn a_lower_correction_pays_nothing_and_is_owed_back() {
        assert_eq!(
            ausgleich(9_000, 10_000),
            Ausgleich {
                auszahlung_ct: 0,
                rueckforderung_ct: 1_000
            }
        );
    }

    #[test]
    fn a_paid_chain_pays_nothing() {
        assert_eq!(
            ausgleich(10_000, 10_000),
            Ausgleich {
                auszahlung_ct: 0,
                rueckforderung_ct: 0
            }
        );
    }

    /// A receipt netting to a claim on the operator is never paid out, and
    /// what was paid is owed back on top of it.
    #[test]
    fn a_negative_receipt_is_owed_back_in_full() {
        assert_eq!(
            ausgleich(-500, 1_000),
            Ausgleich {
                auszahlung_ct: 0,
                rueckforderung_ct: 1_500
            }
        );
    }

    fn aktuell(ct: i64, folge: i64) -> Aktuell {
        Aktuell {
            anspruch_ct: ct,
            ketten_folge: folge,
        }
    }

    #[test]
    fn the_ledger_books_what_a_receipt_changes() {
        assert_eq!(gutschrift_delta(10_000, 1, None), 10_000);
        let a = aktuell(10_000, 1);
        assert_eq!(gutschrift_delta(11_000, 2, Some(a)), 1_000);
        assert_eq!(gutschrift_delta(9_000, 2, Some(a)), -1_000);
        // Overtaken: written before the current receipt, it changes nothing.
        let b = aktuell(11_000, 2);
        assert_eq!(gutschrift_delta(10_000, 1, Some(b)), 0);
    }

    /// A zero receipt after a paid one is the whole amount owed back.
    #[test]
    fn a_zero_receipt_owes_back_what_was_paid() {
        assert_eq!(gutschrift_delta(0, 2, Some(aktuell(10_000, 1))), -10_000);
        assert_eq!(
            ausgleich(0, 10_000),
            Ausgleich {
                auszahlung_ct: 0,
                rueckforderung_ct: 10_000
            }
        );
    }
}
