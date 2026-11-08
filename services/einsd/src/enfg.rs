//! The EnFG ledger: what the Netzbetreiber paid, per ÜNB, month and head.
//!
//! The ÜNB reimburses „die … geleisteten Zahlungen" (§ 13 Abs. 1 Satz 1 EnFG)
//! and the § 50 Nr. 1 EnFG Mitteilung reports „die tatsächlich geleisteten
//! Zahlungen". The ledger is therefore built from what `accountingd` reports as
//! paid — settled payouts and Aufrechnungen, which § 13 Abs. 1 Satz 2 counts as
//! paid — and never from `settlement_receipts.settlement_eur`, which is what
//! was owed. The receipt is read only to say which head a payment belongs to
//! and to reconcile its amount.
//!
//! ## The ÜNB
//!
//! § 50 EnFG addresses „ihrem vorgelagerten Übertragungsnetzbetreiber" and does
//! not say whether that is decided per plant or per Verteilernetzbetreiber. The
//! two readings agree when every plant's Messlokation lies in one Regelzone, so
//! until the reading is settled a month is attributed only in that case: a plant
//! whose MeLo carries no Regelzone, or plants in two Regelzonen, refuse the
//! month, naming the plants. The Regelzone is stored on each ledger row at build
//! time, so a later master-data change does not move a booked row.
//!
//! ## Reconciliation (cent vs. five decimals)
//!
//! `accountingd` pays in euro cent; a settlement is computed to 10⁻⁵ EUR. The
//! cent amount paid for a plant and month must equal one of the month's
//! receipts, rounded kaufmännisch (DIN 1333) to the cent — at most half a cent
//! apart. Anything else is a payment the receipts do not explain and refuses
//! the month.
//!
//! Source: gesetze-im-internet.de `enfg/__13.html`, `__14.html`, `__50.html`,
//! read 2026-11-08; EEG 2023 (Clearingstelle Arbeitsausgabe, Stand 23.12.2025).

use std::collections::{BTreeMap, BTreeSet};

use billing::Amount;
use eeg_billing::enfg::{EnfgPosten, EnfgZuordnung, SettlementModel, zuordnung};
use eeg_billing::round_money;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Amounts in the ledger: 10⁻⁵ EUR, the settlement's own resolution.
pub type Betrag = Amount<5>;

// ── What accountingd reports as paid ─────────────────────────────────────────

/// How a payment was made — `accountingd`'s `quelle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Quelle {
    /// A payout the bank settled.
    Auszahlung,
    /// A claim extinguished by Aufrechnung (§ 13 Abs. 1 Satz 2 EnFG).
    Aufrechnung,
}

impl Quelle {
    /// The storage token.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Auszahlung => "AUSZAHLUNG",
            Self::Aufrechnung => "AUFRECHNUNG",
        }
    }
}

/// What an Aufrechnung was declared against — `accountingd`'s `gegenforderung`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Gegenforderung {
    /// A § 52 EEG 2023 Pflichtzahlung (§ 52 Abs. 6 Satz 2).
    #[serde(rename = "EEG_52_PFLICHTZAHLUNG")]
    Eeg52Pflichtzahlung,
    /// An EnFG Umlage (§ 27 Abs. 1 Satz 2 EEG 2023).
    EnfgUmlage,
    /// Another unbestrittene claim (§ 27 Abs. 1 Satz 1 EEG 2023).
    Unbestritten,
}

/// One payment made, as `GET /api/v1/eeg/enfg-zahlungen` serves it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Zahlung {
    pub quelle: Quelle,
    /// `payout_id` or `aufrechnung_id`.
    pub id: String,
    pub tr_id: Option<String>,
    /// Euro cent paid.
    pub amount_ct: i64,
    #[serde(default)]
    pub gegenforderung: Option<Gegenforderung>,
}

/// `accountingd`'s answer for one month.
#[derive(Debug, Clone, Deserialize)]
pub struct Zahlungen {
    pub zahlungen: Vec<Zahlung>,
}

// ── What einsd knows about each plant ─────────────────────────────────────────

/// One settlement receipt a payment may be for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beleg {
    pub receipt_id: uuid::Uuid,
    pub settlement_model: String,
    pub settlement_eur: Decimal,
    /// The receipt's § 50 EEG positions (Flexibilitätsprämie, -zuschlag),
    /// summed; `None` where the receipt stores no positions.
    pub sect50_eur: Option<Decimal>,
}

/// A plant's facts for one month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnlageMonat {
    pub tr_id: String,
    /// The Regelzone (EIC) of the plant's Messlokation, from `marktd`.
    pub regelzone: Option<String>,
    /// The month's `calculated` receipts, originals and corrections.
    pub belege: Vec<Beleg>,
}

// ── The ledger ────────────────────────────────────────────────────────────────

/// One ledger row: an amount paid for one plant under one head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Posten {
    pub tr_id: String,
    pub posten: EnfgPosten,
    pub quelle: Quelle,
    pub quelle_id: String,
    pub receipt_id: uuid::Uuid,
    pub betrag: Betrag,
}

/// A month's ledger for the one ÜNB its plants share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Monatsposten {
    /// The Regelzone EIC every plant's MeLo carries; `None` for a month with
    /// no payment.
    pub regelzone: Option<String>,
    pub posten: Vec<Posten>,
}

/// Why a month's ledger is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnfgFehler {
    #[error(
        "plants without a Regelzone on their Messlokation: {tr_ids:?} — the ÜNB a payment is \
         reimbursed by is not attributed to a default"
    )]
    OhneRegelzone { tr_ids: Vec<String> },
    #[error(
        "the month's plants lie in more than one Regelzone ({zonen:?}); whether „vorgelagerter \
         Übertragungsnetzbetreiber\" (§ 50 EnFG) is decided per plant or per \
         Verteilernetzbetreiber is not settled, so the month is not attributed"
    )]
    MehrereRegelzonen {
        zonen: BTreeMap<String, Vec<String>>,
    },
    #[error("payment {zahlung_id} names no plant, so it cannot be attributed")]
    OhneAnlage { zahlung_id: String },
    #[error("payment {zahlung_id} is for plant {tr_id}, which this tenant does not hold")]
    UnbekannteAnlage { tr_id: String, zahlung_id: String },
    #[error(
        "plant {tr_id}: {paid_ct} ct paid, but no calculated receipt of the month rounds to it \
         (receipts: {belege:?}) — a payment the receipts do not explain is not reported"
    )]
    Abweichung {
        tr_id: String,
        paid_ct: i64,
        belege: Vec<(uuid::Uuid, Decimal)>,
    },
    #[error("plant {tr_id}: settlement model {model} — {grund}")]
    KeinPosten {
        tr_id: String,
        model: String,
        grund: String,
    },
    #[error("plant {tr_id}: {grund}")]
    Unteilbar { tr_id: String, grund: String },
    #[error("no ledger is booked for {year}-{month:02}: build it before the Mitteilung")]
    KeinLedger { year: i16, month: i16 },
}

/// Euro cent as a ledger amount.
fn cent(ct: i64) -> Option<Betrag> {
    ct.checked_mul(1_000).map(Betrag::from_raw)
}

/// The receipt the plant's paid total reconciles to: the one whose
/// settlement, rounded kaufmännisch to the cent, is the cent amount paid.
///
/// # Errors
///
/// [`EnfgFehler::Abweichung`] when no receipt does.
pub fn abgleich<'a>(
    tr_id: &str,
    paid_ct: i64,
    belege: &'a [Beleg],
) -> Result<&'a Beleg, EnfgFehler> {
    let paid = Decimal::from(paid_ct) / Decimal::ONE_HUNDRED;
    belege
        .iter()
        .rev()
        .find(|b| round_money(b.settlement_eur, 2) == paid)
        .ok_or_else(|| EnfgFehler::Abweichung {
            tr_id: tr_id.to_owned(),
            paid_ct,
            belege: belege
                .iter()
                .map(|b| (b.receipt_id, b.settlement_eur))
                .collect(),
        })
}

/// The month's ledger rows from the payments made and the plants' facts.
///
/// # Errors
///
/// Every [`EnfgFehler`]: an unattributable payment, a missing or second
/// Regelzone, a paid amount the receipts do not explain, a model with no head.
pub fn monatsposten(
    zahlungen: &[Zahlung],
    anlagen: &BTreeMap<String, AnlageMonat>,
) -> Result<Monatsposten, EnfgFehler> {
    // Every payment names a plant this tenant holds.
    let mut je_anlage: BTreeMap<&str, Vec<&Zahlung>> = BTreeMap::new();
    for z in zahlungen {
        let Some(tr_id) = z.tr_id.as_deref() else {
            return Err(EnfgFehler::OhneAnlage {
                zahlung_id: z.id.clone(),
            });
        };
        if !anlagen.contains_key(tr_id) {
            return Err(EnfgFehler::UnbekannteAnlage {
                tr_id: tr_id.to_owned(),
                zahlung_id: z.id.clone(),
            });
        }
        je_anlage.entry(tr_id).or_default().push(z);
    }

    // One Regelzone for every paid plant, or the month is refused.
    let mut ohne = Vec::new();
    let mut zonen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tr_id in je_anlage.keys() {
        match &anlagen[*tr_id].regelzone {
            Some(z) => zonen
                .entry(z.clone())
                .or_default()
                .push((*tr_id).to_owned()),
            None => ohne.push((*tr_id).to_owned()),
        }
    }
    if !ohne.is_empty() {
        return Err(EnfgFehler::OhneRegelzone { tr_ids: ohne });
    }
    if zonen.len() > 1 {
        return Err(EnfgFehler::MehrereRegelzonen { zonen });
    }
    let regelzone = zonen.into_keys().next();

    let mut posten = Vec::new();
    for (tr_id, zs) in je_anlage {
        let paid_ct: i64 = zs.iter().map(|z| z.amount_ct).sum();
        let beleg = abgleich(tr_id, paid_ct, &anlagen[tr_id].belege)?;
        let kein_posten = |grund: String| EnfgFehler::KeinPosten {
            tr_id: tr_id.to_owned(),
            model: beleg.settlement_model.clone(),
            grund,
        };
        let model = SettlementModel::from_token(&beleg.settlement_model)
            .ok_or_else(|| kein_posten("not a settlement model".to_owned()))?;
        let zu = zuordnung(model).map_err(|e| kein_posten(e.grund.to_owned()))?;
        for z in zs {
            let betrag = cent(z.amount_ct).ok_or_else(|| EnfgFehler::Unteilbar {
                tr_id: tr_id.to_owned(),
                grund: format!("{} ct is out of range", z.amount_ct),
            })?;
            let row = |posten: EnfgPosten, betrag: Betrag| Posten {
                tr_id: tr_id.to_owned(),
                posten,
                quelle: z.quelle,
                quelle_id: z.id.clone(),
                receipt_id: beleg.receipt_id,
                betrag,
            };
            match zu {
                EnfgZuordnung::Keine { grund } => {
                    return Err(kein_posten(format!(
                        "a payment was made, but the model carries none: {grund}"
                    )));
                }
                EnfgZuordnung::Ein(p) => posten.push(row(p, betrag)),
                EnfgZuordnung::Geteilt {
                    grundanspruch,
                    flexibilitaet,
                } => {
                    let unteilbar = |grund: &str| EnfgFehler::Unteilbar {
                        tr_id: tr_id.to_owned(),
                        grund: grund.to_owned(),
                    };
                    let sect50 = beleg.sect50_eur.ok_or_else(|| {
                        unteilbar("the receipt stores no positions, so its § 50 part is unknown")
                    })?;
                    let rest = beleg.settlement_eur - sect50;
                    if sect50 < Decimal::ZERO || rest < Decimal::ZERO {
                        return Err(unteilbar(
                            "the receipt's § 50 part lies outside its settlement",
                        ));
                    }
                    let teile = betrag
                        .split(&[rest, sect50])
                        .map_err(|e| unteilbar(&e.to_string()))?;
                    posten.push(row(grundanspruch, teile[0]));
                    posten.push(row(flexibilitaet, teile[1]));
                }
            }
            // § 52 Abs. 6 Satz 2 EEG 2023: the Pflichtzahlung collected by the
            // set-off is the Netzbetreiber's § 52 income, owed to the ÜNB under
            // § 14 Satz 1 Nr. 3 EnFG.
            if z.quelle == Quelle::Aufrechnung
                && z.gegenforderung == Some(Gegenforderung::Eeg52Pflichtzahlung)
            {
                posten.push(row(EnfgPosten::Sect14Eeg52, betrag));
            }
        }
    }
    Ok(Monatsposten { regelzone, posten })
}

/// The plants a month's payments name, deduplicated.
#[must_use]
pub fn bezahlte_anlagen(zahlungen: &[Zahlung]) -> BTreeSet<String> {
    zahlungen.iter().filter_map(|z| z.tr_id.clone()).collect()
}

// ── Master data from marktd ──────────────────────────────────────────────────

/// What `marktd` holds for a Messlokation that the EnFG reports need.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MeloFakten {
    /// Regelzone EIC — names the ÜNB.
    pub regelzone: Option<String>,
    /// BO4E Netzebene of the Messung — the Spannungsebene of § 72 Abs. 2 Nr. 1
    /// EEG 2023.
    #[serde(rename = "netzebene_messung")]
    pub netzebene: Option<String>,
}

/// Where the MeLo facts come from — `marktd` in service, a map in tests.
pub trait Stammdaten {
    /// The MeLo's facts; `None` when `marktd` holds no such MeLo.
    fn melo(
        &self,
        melo_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<MeloFakten>>> + Send;
}

impl Stammdaten for BTreeMap<String, MeloFakten> {
    async fn melo(&self, melo_id: &str) -> anyhow::Result<Option<MeloFakten>> {
        Ok(self.get(melo_id).cloned())
    }
}

/// `marktd`'s `GET /api/v1/melos/{id}`.
pub struct Marktd(pub mako_service::http::Upstream);

impl Stammdaten for Marktd {
    async fn melo(&self, melo_id: &str) -> anyhow::Result<Option<MeloFakten>> {
        let req = self.0.get(&format!("/api/v1/melos/{melo_id}"));
        Ok(self.0.json::<MeloFakten>(req).await?)
    }
}

// ── Persistence ───────────────────────────────────────────────────────────────

/// A refusal on the merits, or a failure underneath.
#[derive(Debug, thiserror::Error)]
pub enum BuildFehler {
    #[error(transparent)]
    Fachlich(#[from] EnfgFehler),
    #[error(transparent)]
    Technisch(#[from] anyhow::Error),
}

impl From<sqlx::Error> for BuildFehler {
    fn from(e: sqlx::Error) -> Self {
        Self::Technisch(e.into())
    }
}

/// One stored ledger row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LedgerZeile {
    pub uenb_regelzone: String,
    pub tr_id: String,
    pub posten: EnfgPosten,
    pub rechtsgrundlage: &'static str,
    pub quelle: String,
    pub quelle_id: String,
    pub receipt_id: uuid::Uuid,
    pub betrag_eur: Decimal,
}

/// The § 50 EEG positions of a receipt's `positions_json`, summed.
fn sect50_summe(positions: Option<&serde_json::Value>) -> Option<Decimal> {
    let arr = positions?.as_array()?;
    let mut sum = Decimal::ZERO;
    for p in arr {
        let basis = p.get("legal_basis")?.as_str()?;
        if basis.trim_start_matches('§').trim_start().starts_with("50") {
            sum += p.get("eur")?.as_str()?.parse::<Decimal>().ok()?;
        }
    }
    Some(sum)
}

/// The facts of the plants `tr_ids` for one month: their `calculated`
/// receipts and the Regelzone of their MeLo. A plant this tenant does not
/// hold is absent from the map.
///
/// # Errors
///
/// A database or `marktd` failure.
pub async fn anlagen_im_monat<S: Stammdaten>(
    pool: &sqlx::PgPool,
    tenant: &str,
    year: i16,
    month: i16,
    tr_ids: &BTreeSet<String>,
    stammdaten: &S,
) -> anyhow::Result<BTreeMap<String, AnlageMonat>> {
    use sqlx::Row as _;
    let ids: Vec<String> = tr_ids.iter().cloned().collect();
    let mut out = BTreeMap::new();
    for r in
        sqlx::query("SELECT tr_id, melo_id FROM eeg_anlagen WHERE tenant = $1 AND tr_id = ANY($2)")
            .bind(tenant)
            .bind(&ids)
            .fetch_all(pool)
            .await?
    {
        let tr_id: String = r.get("tr_id");
        let melo_id: Option<String> = r.get("melo_id");
        let regelzone = match melo_id {
            Some(m) => stammdaten.melo(&m).await?.and_then(|f| f.regelzone),
            None => None,
        };
        out.insert(
            tr_id.clone(),
            AnlageMonat {
                tr_id,
                regelzone,
                belege: Vec::new(),
            },
        );
    }
    for r in sqlx::query(
        "SELECT id, tr_id, settlement_model, settlement_eur, positions_json
           FROM settlement_receipts
          WHERE tenant = $1 AND billing_year = $2 AND billing_month = $3
            AND tr_id = ANY($4) AND status = 'calculated' AND settlement_eur IS NOT NULL
          ORDER BY settled_at",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .bind(&ids)
    .fetch_all(pool)
    .await?
    {
        let tr_id: String = r.get("tr_id");
        let positions: Option<serde_json::Value> = r.get("positions_json");
        if let Some(a) = out.get_mut(&tr_id) {
            a.belege.push(Beleg {
                receipt_id: r.get("id"),
                settlement_model: r.get("settlement_model"),
                settlement_eur: r.get("settlement_eur"),
                sect50_eur: sect50_summe(positions.as_ref()),
            });
        }
    }
    Ok(out)
}

/// Build and book one month's ledger from the payments `accountingd` reports.
///
/// Rows are written once — keyed on their payment and head — and never moved:
/// a rebuild adds the payments made since and leaves booked rows, and their
/// Regelzone, as they are. A month whose booked rows name another Regelzone
/// than the new ones is refused like any second Regelzone.
///
/// # Errors
///
/// [`BuildFehler::Fachlich`] for every [`EnfgFehler`];
/// [`BuildFehler::Technisch`] for a database or `marktd` failure.
pub async fn build_month<S: Stammdaten>(
    pool: &sqlx::PgPool,
    tenant: &str,
    year: i16,
    month: i16,
    zahlungen: &[Zahlung],
    stammdaten: &S,
) -> Result<Vec<LedgerZeile>, BuildFehler> {
    let tr_ids = bezahlte_anlagen(zahlungen);
    let anlagen = anlagen_im_monat(pool, tenant, year, month, &tr_ids, stammdaten).await?;
    let monat = monatsposten(zahlungen, &anlagen)?;
    let mut tx = pool.begin().await?;
    if let Some(zone) = &monat.regelzone {
        let gebucht: Vec<(String, String)> = sqlx::query_as(
            "SELECT DISTINCT uenb_regelzone, tr_id FROM enfg_ledger
              WHERE tenant = $1 AND billing_year = $2 AND billing_month = $3
                AND uenb_regelzone <> $4",
        )
        .bind(tenant)
        .bind(year)
        .bind(month)
        .bind(zone)
        .fetch_all(&mut *tx)
        .await?;
        if !gebucht.is_empty() {
            let mut zonen: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (z, t) in gebucht {
                zonen.entry(z).or_default().push(t);
            }
            zonen.insert(
                zone.clone(),
                monat
                    .posten
                    .iter()
                    .map(|p| p.tr_id.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
            );
            return Err(EnfgFehler::MehrereRegelzonen { zonen }.into());
        }
        for p in &monat.posten {
            sqlx::query(
                "INSERT INTO enfg_ledger
                     (tenant, uenb_regelzone, billing_year, billing_month, posten, tr_id,
                      quelle, quelle_id, receipt_id, betrag_eur)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                 ON CONFLICT (tenant, quelle, quelle_id, posten) DO NOTHING",
            )
            .bind(tenant)
            .bind(zone)
            .bind(year)
            .bind(month)
            .bind(p.posten.token())
            .bind(&p.tr_id)
            .bind(p.quelle.token())
            .bind(&p.quelle_id)
            .bind(p.receipt_id)
            .bind(p.betrag.to_decimal())
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(ledger_monat(pool, tenant, year, month).await?)
}

/// The booked ledger rows of one month.
///
/// # Errors
///
/// A database failure, or a stored head outside the vocabulary.
pub async fn ledger_monat(
    pool: &sqlx::PgPool,
    tenant: &str,
    year: i16,
    month: i16,
) -> anyhow::Result<Vec<LedgerZeile>> {
    use sqlx::Row as _;
    let mut out = Vec::new();
    for r in sqlx::query(
        "SELECT uenb_regelzone, tr_id, posten, quelle, quelle_id, receipt_id, betrag_eur
           FROM enfg_ledger
          WHERE tenant = $1 AND billing_year = $2 AND billing_month = $3
          ORDER BY tr_id, posten, quelle, quelle_id",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .fetch_all(pool)
    .await?
    {
        let token: String = r.get("posten");
        let posten = EnfgPosten::from_token(&token)
            .ok_or_else(|| anyhow::anyhow!("enfg_ledger.posten {token:?} unknown"))?;
        out.push(LedgerZeile {
            uenb_regelzone: r.get("uenb_regelzone"),
            tr_id: r.get("tr_id"),
            posten,
            rechtsgrundlage: posten.rechtsgrundlage(),
            quelle: r.get("quelle"),
            quelle_id: r.get("quelle_id"),
            receipt_id: r.get("receipt_id"),
            betrag_eur: r.get("betrag_eur"),
        });
    }
    Ok(out)
}

// ── § 50 Nr. 1 EnFG Mitteilung ───────────────────────────────────────────────

/// Whether mako holds a Mitteilung item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Bestand {
    /// The figure is complete.
    Gehalten,
    /// Part of the item is held; `fehlt` names the rest.
    Teilweise,
    /// mako holds nothing for the item; it carries no figure, never a zero.
    NichtGehalten,
}

/// One item of the Mitteilung.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MitteilungsPosten {
    /// The provision that asks for it.
    pub posten: &'static str,
    /// What it is, in the statute's words.
    pub inhalt: &'static str,
    pub bestand: Bestand,
    /// The figure; `None` whenever `bestand` is `nicht_gehalten`.
    pub betrag_eur: Option<Decimal>,
    /// What mako does not hold.
    pub fehlt: Option<&'static str>,
}

/// A plant's facts for the § 72 EEG 2023 part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anlage72 {
    pub tr_id: String,
    pub settlement_model: String,
    pub erzeugungsart: String,
    pub leistung_kw: Decimal,
    /// The last Veräußerungsform switch, when it falls in the month.
    pub wechsel_am: Option<time::Date>,
    /// The MeLo's Netzebene, from `marktd`.
    pub spannungsebene: Option<String>,
}

/// § 72 Abs. 1 Nr. 2 EEG 2023: the facts owed on a switch into the Ausfallvergütung.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ausfallverguetung72 {
    pub energietraeger: String,
    pub installierte_leistung_kw: Decimal,
    pub seit: String,
}

/// One plant's § 72 line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Anlage72Zeile {
    pub tr_id: String,
    /// The § 21b Abs. 1 EEG 2023 Veräußerungsform; `None` where the model
    /// does not say which.
    pub veraeusserungsform: Option<&'static str>,
    /// The § 21c Abs. 1 switch the month carries, `YYYY-MM-DD`.
    pub wechsel_am: Option<String>,
    /// § 72 Abs. 2 Nr. 1: the Spannungsebene; `None` when marktd holds none.
    pub spannungsebene: Option<String>,
    pub ausfallverguetung: Option<Ausfallverguetung72>,
}

/// The § 50 Nr. 1 EnFG Mitteilung for one ÜNB and month.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mitteilung {
    pub uenb_regelzone: String,
    pub billing_year: i16,
    pub billing_month: i16,
    /// § 50 Nr. 1 Buchst. a–g EnFG.
    pub posten: Vec<MitteilungsPosten>,
    /// § 72 Abs. 1, 2 EEG 2023, „im Rahmen der Mitteilung nach § 50 Nummer 1".
    pub sect72_anlagen: Vec<Anlage72Zeile>,
    /// Every item, and every plant fact, mako does not hold.
    pub nicht_gehalten: Vec<String>,
}

/// The § 21b Abs. 1 Satz 1 EEG 2023 Veräußerungsform a model settles in.
///
/// `Ok(None)` for a KWKG plant, which has none; `Err` names why the model
/// does not say which.
///
/// # Errors
///
/// The models whose Veräußerungsform the token does not determine.
pub fn veraeusserungsform(model: SettlementModel) -> Result<Option<&'static str>, &'static str> {
    use SettlementModel as M;
    match model {
        M::Direktvermarktung | M::Ausschreibung => Ok(Some(
            "§ 21b Abs. 1 Satz 1 Nr. 1 EEG 2023 — Marktprämie nach § 20",
        )),
        M::Verguetung | M::Ausfallverguetung => Ok(Some(
            "§ 21b Abs. 1 Satz 1 Nr. 2 EEG 2023 — Einspeisevergütung nach § 21 Abs. 1 Satz 1",
        )),
        M::Mieterstrom => Ok(Some(
            "§ 21b Abs. 1 Satz 1 Nr. 3 EEG 2023 — Mieterstromzuschlag nach § 21 Abs. 3",
        )),
        M::SonstigeDirektvermarktung => Ok(Some(
            "§ 21b Abs. 1 Satz 1 Nr. 4 EEG 2023 — sonstige Direktvermarktung nach § 21a",
        )),
        // A KWK-Anlage is no EEG Anlage and has no § 21b Veräußerungsform.
        M::KwkgZuschlag => Ok(None),
        // § 50b Satz 1: the Prämie accompanies „einer Veräußerung des Stroms in
        // den Veräußerungsformen einer Direktvermarktung" — Nr. 1 or Nr. 4.
        M::Flexibilitaet => Err("the Flexibilitätsprämie runs beside a Direktvermarktung \
                                 (§ 50b Satz 1 EEG 2023), and the model does not say whether \
                                 it is Nr. 1 or Nr. 4"),
        M::FlexibilitaetZuschlag => Err("the Flexibilitätszuschlag (§ 50a EEG 2023) is a \
                                         capacity payment; the model does not say how the \
                                         Strom is sold"),
        M::Ggv => Err(
            "the gemeinschaftliche Gebäudeversorgung (§ 42b EnWG) is no § 21b Abs. 1 \
                       Veräußerungsform",
        ),
        M::PostEegSpot => Err("the model is not shown to be the § 21 Abs. 1 Satz 1 Nr. 4 \
                               Einspeisevergütung (see eeg_billing::enfg)"),
        M::Eigenverbrauch => Err("a plant with no feed-in is assigned no Veräußerungsform"),
    }
}

/// The figure of one head, summed over the ledger rows `nimm` selects.
fn summe(ledger: &[LedgerZeile], nimm: impl Fn(&LedgerZeile) -> bool) -> Decimal {
    ledger
        .iter()
        .filter(|z| nimm(z))
        .map(|z| z.betrag_eur)
        .sum()
}

/// Build the Mitteilung from a month's booked ledger rows and its plants.
#[must_use]
pub fn mitteilung(
    regelzone: &str,
    year: i16,
    month: i16,
    ledger: &[LedgerZeile],
    anlagen: &[Anlage72],
) -> Mitteilung {
    use Bestand::{Gehalten, NichtGehalten, Teilweise};
    let gehalten = |posten, inhalt, betrag| MitteilungsPosten {
        posten,
        inhalt,
        bestand: Gehalten,
        betrag_eur: Some(betrag),
        fehlt: None,
    };
    let fehlt = |posten, inhalt, fehlt| MitteilungsPosten {
        posten,
        inhalt,
        bestand: NichtGehalten,
        betrag_eur: None,
        fehlt: Some(fehlt),
    };
    let kopf = |p: EnfgPosten| summe(ledger, |z| z.posten == p);
    let posten = vec![
        gehalten(
            EnfgPosten::Eeg19Abs1.mitteilungsposten(),
            "die tatsächlich geleisteten Zahlungen für Strom aus erneuerbaren Energien und \
             Grubengas nach § 19 Absatz 1 EEG",
            kopf(EnfgPosten::Eeg19Abs1),
        ),
        gehalten(
            EnfgPosten::Eeg50.mitteilungsposten(),
            "die tatsächlich geleisteten Zahlungen für die Bereitstellung von installierter \
             Leistung nach § 50 EEG",
            kopf(EnfgPosten::Eeg50),
        ),
        gehalten(
            EnfgPosten::KwkgStrom.mitteilungsposten(),
            "die tatsächlich geleisteten Zahlungen für KWK-Strom",
            kopf(EnfgPosten::KwkgStrom),
        ),
        fehlt(
            "§ 50 Nr. 1 Buchst. a Doppelbuchst. dd EnFG",
            "die Boni nach den §§ 7a bis 7c KWKG",
            "einsd settles no KWKG Bonus",
        ),
        fehlt(
            "§ 50 Nr. 1 Buchst. b EnFG",
            "die Höhe der vermiedenen Netzentgelte nach § 13 Absatz 2",
            "grid-billing computes the vermiedene Netzentgelt per plant; no amount per ÜNB \
             is stored",
        ),
        fehlt(
            "§ 50 Nr. 1 Buchst. c EnFG",
            "die Erlöse oder vermiedenen Aufwendungen aus der Verwertung des kaufmännisch \
             abgenommenen KWK-Stroms",
            "mako records no Verwertung of kaufmännisch abgenommenem KWK-Strom",
        ),
        fehlt(
            "§ 50 Nr. 1 Buchst. d EnFG",
            "die umlagenpflichtigen Netzentnahmen und der jeweils anzuwendende Umlagensatz",
            "the Umlagen are billed per Netznutzer by grid-billing; no Netzentnahme per ÜNB \
             is stored",
        ),
        MitteilungsPosten {
            posten: EnfgPosten::Sect14Eeg52.mitteilungsposten(),
            inhalt: "die Höhe der Einnahmen aus Zahlungen nach den §§ 52 und 55b EEG",
            bestand: Teilweise,
            betrag_eur: Some(kopf(EnfgPosten::Sect14Eeg52)),
            fehlt: Some(
                "§ 52 Pflichtzahlungen the operator paid rather than had set off, and every \
                 § 55b payment — mako holds only the § 52 claims collected by Aufrechnung",
            ),
        },
        gehalten(
            "§ 50 Nr. 1 Buchst. f EnFG",
            "die Höhe der durch Aufrechnung erloschenen Forderungen (EEG/KWKG claims recorded \
             in accountingd)",
            summe(ledger, |z| {
                z.quelle == Quelle::Aufrechnung.token() && z.posten.ist_erstattung()
            }),
        ),
        fehlt(
            "§ 50 Nr. 1 Buchst. g EnFG",
            "die sonstigen für den Ausgleich nach Teil 4 erforderlichen Angaben",
            "not specified until the ÜNB's Formularvorlage (§ 54 Satz 2 EnFG) is read",
        ),
    ];

    let mut nicht_gehalten: Vec<String> = posten
        .iter()
        .filter(|p| p.bestand != Gehalten)
        .map(|p| format!("{}: {}", p.posten, p.fehlt.unwrap_or_default()))
        .collect();

    let mut sect72_anlagen = Vec::new();
    for a in anlagen {
        let form = match SettlementModel::from_token(&a.settlement_model) {
            Some(m) => veraeusserungsform(m),
            None => Err("not a settlement model"),
        };
        let veraeusserungsform = match form {
            Ok(None) => continue,
            Ok(Some(f)) => Some(f),
            Err(why) => {
                nicht_gehalten.push(format!(
                    "§ 72 Abs. 1 Nr. 1 EEG 2023 — Veräußerungsform of {}: {why}",
                    a.tr_id
                ));
                None
            }
        };
        if a.spannungsebene.is_none() {
            nicht_gehalten.push(format!(
                "§ 72 Abs. 2 Nr. 1 EEG 2023 — Spannungsebene of {}: marktd holds no Netzebene \
                 for its Messlokation",
                a.tr_id
            ));
        }
        let ausfallverguetung = (a.settlement_model == SettlementModel::Ausfallverguetung.token())
            .then_some(a.wechsel_am)
            .flatten()
            .map(|seit| Ausfallverguetung72 {
                energietraeger: a.erzeugungsart.clone(),
                installierte_leistung_kw: a.leistung_kw,
                seit: seit.to_string(),
            });
        sect72_anlagen.push(Anlage72Zeile {
            tr_id: a.tr_id.clone(),
            veraeusserungsform,
            wechsel_am: a.wechsel_am.map(|d| d.to_string()),
            spannungsebene: a.spannungsebene.clone(),
            ausfallverguetung,
        });
    }
    for item in [
        "§ 72 Abs. 1 Nr. 3 EEG 2023: die sonstigen für die Weitergabe und die Vermarktung des \
         Stroms erforderlichen Angaben — not specified until the ÜNB's Formularvorlage is read",
        "§ 72 Abs. 2 Nr. 2 EEG 2023: inwieweit Energiemengen von einem nachgelagerten Netz \
         abgenommen wurden — mako holds no inter-network quantities",
        "§ 72 Abs. 2 Nr. 3 EEG 2023: inwieweit diese Energiemengen abgegeben oder selbst \
         verbraucht wurden — mako holds no inter-network quantities",
    ] {
        nicht_gehalten.push(item.to_owned());
    }

    Mitteilung {
        uenb_regelzone: regelzone.to_owned(),
        billing_year: year,
        billing_month: month,
        posten,
        sect72_anlagen,
        nicht_gehalten,
    }
}

/// The § 50 Nr. 1 EnFG Mitteilung for a booked month.
///
/// Its plants are the ledger's plus every plant whose last Veräußerungsform
/// switch falls in the month — a switch into the sonstige Direktvermarktung
/// carries no payment, and § 72 Abs. 1 Nr. 1 EEG 2023 still reports it.
///
/// # Errors
///
/// [`EnfgFehler::KeinLedger`] for a month with no booked row; a database or
/// `marktd` failure.
pub async fn mitteilung_monat<S: Stammdaten>(
    pool: &sqlx::PgPool,
    tenant: &str,
    year: i16,
    month: i16,
    stammdaten: &S,
) -> Result<Mitteilung, BuildFehler> {
    use sqlx::Row as _;
    let ledger = ledger_monat(pool, tenant, year, month).await?;
    let Some(regelzone) = ledger.first().map(|z| z.uenb_regelzone.clone()) else {
        return Err(EnfgFehler::KeinLedger { year, month }.into());
    };
    let monat = time::Month::try_from(u8::try_from(month).unwrap_or(0))
        .map_err(|e| anyhow::anyhow!("billing month {month}: {e}"))?;
    let von = time::Date::from_calendar_date(i32::from(year), monat, 1)
        .map_err(|e| anyhow::anyhow!("billing month {year}-{month}: {e}"))?;
    let bis = von
        .replace_day(monat.length(i32::from(year)))
        .map_err(|e| anyhow::anyhow!("billing month {year}-{month}: {e}"))?;
    let ids: Vec<String> = ledger
        .iter()
        .map(|z| z.tr_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut anlagen = Vec::new();
    for r in sqlx::query(
        "SELECT tr_id, melo_id, settlement_model, erzeugungsart, leistung_kwp,
                last_veraeusserungsform_switch
           FROM eeg_anlagen
          WHERE tenant = $1
            AND (tr_id = ANY($2) OR last_veraeusserungsform_switch BETWEEN $3 AND $4)
          ORDER BY tr_id",
    )
    .bind(tenant)
    .bind(&ids)
    .bind(von)
    .bind(bis)
    .fetch_all(pool)
    .await?
    {
        let melo_id: Option<String> = r.get("melo_id");
        let spannungsebene = match melo_id {
            Some(m) => stammdaten.melo(&m).await?.and_then(|f| f.netzebene),
            None => None,
        };
        let switch: Option<time::Date> = r.get("last_veraeusserungsform_switch");
        anlagen.push(Anlage72 {
            tr_id: r.get("tr_id"),
            settlement_model: r.get("settlement_model"),
            erzeugungsart: r.get("erzeugungsart"),
            leistung_kw: r.get("leistung_kwp"),
            wechsel_am: switch.filter(|d| (von..=bis).contains(d)),
            spannungsebene,
        });
    }
    Ok(mitteilung(&regelzone, year, month, &ledger, &anlagen))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    fn beleg(model: &str, eur: Decimal) -> Beleg {
        Beleg {
            receipt_id: uuid::Uuid::new_v4(),
            settlement_model: model.to_owned(),
            settlement_eur: eur,
            sect50_eur: Some(Decimal::ZERO),
        }
    }

    fn anlage(tr: &str, zone: Option<&str>, belege: Vec<Beleg>) -> (String, AnlageMonat) {
        (
            tr.to_owned(),
            AnlageMonat {
                tr_id: tr.to_owned(),
                regelzone: zone.map(str::to_owned),
                belege,
            },
        )
    }

    fn auszahlung(tr: &str, ct: i64) -> Zahlung {
        Zahlung {
            quelle: Quelle::Auszahlung,
            id: format!("P-{tr}-{ct}"),
            tr_id: Some(tr.to_owned()),
            amount_ct: ct,
            gegenforderung: None,
        }
    }

    const ZONE: &str = "10YDE-EON------1";

    /// The ledger holds the paid plant only; the settled-but-unpaid one has no
    /// payment and so no row, whatever its receipt says.
    #[test]
    fn only_a_paid_receipt_reaches_the_ledger() {
        let anlagen: BTreeMap<_, _> = [
            anlage(
                "TR-PAID",
                Some(ZONE),
                vec![beleg("VERGUETUNG", dec!(12.34444))],
            ),
            anlage(
                "TR-UNPAID",
                Some(ZONE),
                vec![beleg("VERGUETUNG", dec!(99.00000))],
            ),
        ]
        .into();
        let out = monatsposten(&[auszahlung("TR-PAID", 1234)], &anlagen).expect("ledger");
        assert_eq!(out.regelzone.as_deref(), Some(ZONE));
        assert_eq!(out.posten.len(), 1);
        assert_eq!(out.posten[0].tr_id, "TR-PAID");
        assert_eq!(out.posten[0].posten, EnfgPosten::Eeg19Abs1);
        // The paid cents, not the receipt's 12,34444.
        assert_eq!(out.posten[0].betrag, Betrag::from_raw(1_234_000));
    }

    /// § 14 Satz 1 Nr. 3 EnFG: the § 52 claim collected by Aufrechnung is owed to
    /// the ÜNB, and the extinguished EEG claim counts as paid (§ 13 Abs. 1 Satz 2).
    #[test]
    fn a_sect52_set_off_lands_as_a_sect14_item() {
        let anlagen: BTreeMap<_, _> = [anlage(
            "TR-1",
            Some(ZONE),
            vec![beleg("VERGUETUNG", dec!(100.00))],
        )]
        .into();
        let set_off = Zahlung {
            quelle: Quelle::Aufrechnung,
            id: "A-1".to_owned(),
            tr_id: Some("TR-1".to_owned()),
            amount_ct: 3_000,
            gegenforderung: Some(Gegenforderung::Eeg52Pflichtzahlung),
        };
        let out = monatsposten(&[auszahlung("TR-1", 7_000), set_off], &anlagen).expect("ledger");
        let by: BTreeMap<(EnfgPosten, Quelle), Betrag> = out
            .posten
            .iter()
            .map(|p| ((p.posten, p.quelle), p.betrag))
            .collect();
        assert_eq!(
            by.get(&(EnfgPosten::Eeg19Abs1, Quelle::Auszahlung)),
            Some(&Betrag::from_raw(7_000_000))
        );
        assert_eq!(
            by.get(&(EnfgPosten::Eeg19Abs1, Quelle::Aufrechnung)),
            Some(&Betrag::from_raw(3_000_000))
        );
        assert_eq!(
            by.get(&(EnfgPosten::Sect14Eeg52, Quelle::Aufrechnung)),
            Some(&Betrag::from_raw(3_000_000))
        );
        assert_eq!(out.posten.len(), 3);
    }

    #[test]
    fn a_plant_without_a_regelzone_refuses_with_its_tr_id() {
        let anlagen: BTreeMap<_, _> = [
            anlage("TR-A", Some(ZONE), vec![beleg("VERGUETUNG", dec!(1))]),
            anlage("TR-B", None, vec![beleg("VERGUETUNG", dec!(1))]),
        ]
        .into();
        let err = monatsposten(
            &[auszahlung("TR-A", 100), auszahlung("TR-B", 100)],
            &anlagen,
        )
        .expect_err("refused");
        assert_eq!(
            err,
            EnfgFehler::OhneRegelzone {
                tr_ids: vec!["TR-B".to_owned()]
            }
        );
        assert!(err.to_string().contains("TR-B"));
    }

    #[test]
    fn plants_in_two_regelzonen_refuse_the_month() {
        let anlagen: BTreeMap<_, _> = [
            anlage("TR-A", Some(ZONE), vec![beleg("VERGUETUNG", dec!(1))]),
            anlage(
                "TR-B",
                Some("10YDE-RWENET---I"),
                vec![beleg("VERGUETUNG", dec!(1))],
            ),
        ]
        .into();
        let err = monatsposten(
            &[auszahlung("TR-A", 100), auszahlung("TR-B", 100)],
            &anlagen,
        )
        .expect_err("refused");
        assert!(matches!(err, EnfgFehler::MehrereRegelzonen { ref zonen } if zonen.len() == 2));
    }

    /// The cents paid may differ from the five-decimal settlement by its
    /// own rounding — half a cent — and not by more.
    #[test]
    fn a_payout_off_by_more_than_its_rounding_refuses() {
        let b = [beleg("VERGUETUNG", dec!(12.345))];
        // 12,345 rounds kaufmännisch to 12,35.
        assert!(abgleich("TR-1", 1235, &b).is_ok());
        assert!(matches!(
            abgleich("TR-1", 1234, &b),
            Err(EnfgFehler::Abweichung { paid_ct: 1234, .. })
        ));
        assert!(abgleich("TR-1", 1236, &b).is_err());
        // A corrected month: the payment reconciles to whichever receipt it was for.
        let two = [
            beleg("VERGUETUNG", dec!(10)),
            beleg("VERGUETUNG", dec!(12.344)),
        ];
        assert_eq!(
            abgleich("TR-1", 1234, &two).expect("second").settlement_eur,
            dec!(12.344)
        );
        let anlagen: BTreeMap<_, _> = [anlage(
            "TR-1",
            Some(ZONE),
            vec![beleg("VERGUETUNG", dec!(12.345))],
        )]
        .into();
        assert!(matches!(
            monatsposten(&[auszahlung("TR-1", 1300)], &anlagen),
            Err(EnfgFehler::Abweichung { .. })
        ));
    }

    /// A Flexibilitätsprämie settlement pays a § 19 and a § 50 part, split by its
    /// receipt's § 50 position; the parts sum to what was paid.
    #[test]
    fn a_flexibilitaet_payment_splits_by_its_receipt() {
        let mut b = beleg("FLEXIBILITAET", dec!(100));
        b.sect50_eur = Some(dec!(25));
        let anlagen: BTreeMap<_, _> = [anlage("TR-F", Some(ZONE), vec![b])].into();
        let out = monatsposten(&[auszahlung("TR-F", 10_000)], &anlagen).expect("ledger");
        let by: BTreeMap<EnfgPosten, Betrag> =
            out.posten.iter().map(|p| (p.posten, p.betrag)).collect();
        assert_eq!(by[&EnfgPosten::Eeg50], Betrag::from_raw(2_500_000));
        assert_eq!(by[&EnfgPosten::Eeg19Abs1], Betrag::from_raw(7_500_000));
    }

    /// A paid plant whose model has no cited head refuses rather than defaulting.
    #[test]
    fn a_paid_post_eeg_plant_refuses() {
        let anlagen: BTreeMap<_, _> = [anlage(
            "TR-P",
            Some(ZONE),
            vec![beleg("POST_EEG_SPOT", dec!(1))],
        )]
        .into();
        assert!(matches!(
            monatsposten(&[auszahlung("TR-P", 100)], &anlagen),
            Err(EnfgFehler::KeinPosten { .. })
        ));
    }

    #[test]
    fn a_payment_without_a_plant_refuses() {
        let mut z = auszahlung("TR-1", 100);
        z.tr_id = None;
        assert!(matches!(
            monatsposten(&[z], &BTreeMap::new()),
            Err(EnfgFehler::OhneAnlage { .. })
        ));
    }

    fn zeile(tr: &str, posten: EnfgPosten, quelle: Quelle, eur: Decimal) -> LedgerZeile {
        LedgerZeile {
            uenb_regelzone: ZONE.to_owned(),
            tr_id: tr.to_owned(),
            posten,
            rechtsgrundlage: posten.rechtsgrundlage(),
            quelle: quelle.token().to_owned(),
            quelle_id: format!("{tr}-{}", quelle.token()),
            receipt_id: uuid::Uuid::nil(),
            betrag_eur: eur,
        }
    }

    fn anlage72(tr: &str, model: &str, ebene: Option<&str>) -> Anlage72 {
        Anlage72 {
            tr_id: tr.to_owned(),
            settlement_model: model.to_owned(),
            erzeugungsart: "SOLAR_AUFDACH".to_owned(),
            leistung_kw: dec!(150),
            wechsel_am: None,
            spannungsebene: ebene.map(str::to_owned),
        }
    }

    /// Every § 50 Nr. 1 Buchst. a–g item is present; an item mako does
    /// not hold carries no figure — never a zero — and is named.
    #[test]
    fn an_item_mako_does_not_hold_is_named_never_zero() {
        let ledger = [
            zeile("TR-1", EnfgPosten::Eeg19Abs1, Quelle::Auszahlung, dec!(70)),
            zeile("TR-1", EnfgPosten::Eeg19Abs1, Quelle::Aufrechnung, dec!(30)),
            zeile(
                "TR-1",
                EnfgPosten::Sect14Eeg52,
                Quelle::Aufrechnung,
                dec!(30),
            ),
        ];
        let m = mitteilung(
            ZONE,
            2026,
            3,
            &ledger,
            &[anlage72("TR-1", "VERGUETUNG", Some("NSP"))],
        );
        assert_eq!(m.posten.len(), 10, "aa, bb, cc, dd, b, c, d, e, f, g");
        let by: BTreeMap<&str, &MitteilungsPosten> =
            m.posten.iter().map(|p| (p.posten, p)).collect();
        assert_eq!(
            by["§ 50 Nr. 1 Buchst. a Doppelbuchst. aa EnFG"].betrag_eur,
            Some(dec!(100))
        );
        assert_eq!(by["§ 50 Nr. 1 Buchst. f EnFG"].betrag_eur, Some(dec!(30)));
        let e = by["§ 50 Nr. 1 Buchst. e EnFG"];
        assert_eq!(
            (e.bestand, e.betrag_eur),
            (Bestand::Teilweise, Some(dec!(30)))
        );
        assert!(e.fehlt.is_some());
        for p in &m.posten {
            match p.bestand {
                Bestand::NichtGehalten => {
                    assert_eq!(p.betrag_eur, None, "{} carries a figure", p.posten);
                    assert!(p.fehlt.is_some());
                    assert!(
                        m.nicht_gehalten.iter().any(|n| n.contains(p.posten)),
                        "{} is not named",
                        p.posten
                    );
                }
                Bestand::Teilweise => {
                    assert!(m.nicht_gehalten.iter().any(|n| n.contains(p.posten)))
                }
                Bestand::Gehalten => assert!(p.betrag_eur.is_some()),
            }
        }
        for unheld in [
            "§ 50 Nr. 1 Buchst. b EnFG",
            "§ 50 Nr. 1 Buchst. d EnFG",
            "§ 50 Nr. 1 Buchst. a Doppelbuchst. dd EnFG",
        ] {
            assert_eq!(by[unheld].bestand, Bestand::NichtGehalten, "{unheld}");
        }
    }

    /// § 72 Abs. 1 Nr. 1 and Abs. 2 Nr. 1 EEG 2023: each plant's
    /// Veräußerungsform and Spannungsebene; a missing one is named by plant.
    #[test]
    fn the_sect72_part_carries_the_veraeusserungsform_and_names_a_missing_spannungsebene() {
        let mut av = anlage72("TR-AV", "AUSFALLVERGUETUNG", None);
        av.wechsel_am = Some(time::macros::date!(2026 - 03 - 01));
        let m = mitteilung(
            ZONE,
            2026,
            3,
            &[],
            &[anlage72("TR-1", "DIREKTVERMARKTUNG", Some("MSP")), av],
        );
        let by: BTreeMap<&str, &Anlage72Zeile> = m
            .sect72_anlagen
            .iter()
            .map(|a| (a.tr_id.as_str(), a))
            .collect();
        assert!(
            by["TR-1"]
                .veraeusserungsform
                .expect("held")
                .contains("Nr. 1")
        );
        assert_eq!(by["TR-1"].spannungsebene.as_deref(), Some("MSP"));
        let av = by["TR-AV"];
        assert!(av.veraeusserungsform.expect("held").contains("Nr. 2"));
        assert_eq!(av.spannungsebene, None);
        let details = av.ausfallverguetung.as_ref().expect("§ 72 Abs. 1 Nr. 2");
        assert_eq!(details.installierte_leistung_kw, dec!(150));
        assert!(
            m.nicht_gehalten
                .iter()
                .any(|n| n.contains("TR-AV") && n.contains("Spannungsebene"))
        );
        assert!(
            m.nicht_gehalten
                .iter()
                .any(|n| n.contains("§ 72 Abs. 2 Nr. 2"))
        );
    }

    #[test]
    fn a_model_without_a_veraeusserungsform_is_named() {
        assert_eq!(veraeusserungsform(SettlementModel::KwkgZuschlag), Ok(None));
        assert!(veraeusserungsform(SettlementModel::Ggv).is_err());
        let m = mitteilung(ZONE, 2026, 3, &[], &[anlage72("TR-G", "GGV", Some("NSP"))]);
        assert_eq!(m.sect72_anlagen[0].veraeusserungsform, None);
        assert!(m.nicht_gehalten.iter().any(|n| n.contains("TR-G")));
    }
}
