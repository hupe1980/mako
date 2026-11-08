//! The EEG and KWKG payments a Netzbetreiber has actually made, for the EnFG
//! Belastungsausgleich.
//!
//! The ÜNB reimburses „geleistete Zahlungen" (§ 13 Abs. 1 Satz 1 EnFG) and the
//! § 50 Nr. 1 Buchst. a EnFG Mitteilung reports „die tatsächlich geleisteten
//! Zahlungen", so what `einsd` computes as owed is not what this serves. Two
//! things count as paid:
//!
//! - **a payout the bank has settled** — a pain.001 order whose pain.002 status
//!   is `ACSC`, the only ISO 20022 status that says the funds moved. `ACCP`,
//!   `ACSP` and `ACWC` accept the order without settling it, `PART` settles part
//!   of it and `PDNG` nothing yet, so none of them is a payment;
//! - **a Forderung extinguished by Aufrechnung** — „Als geleistete Zahlungen im
//!   Sinn des Satzes 1 gelten auch Forderungen eines Anlagenbetreibers auf
//!   Zahlung, die durch Aufrechnung erloschen sind" (§ 13 Abs. 1 Satz 2 EnFG).
//!   An Aufrechnung is recorded here (`eeg_aufrechnungen`) with the
//!   Gegenforderung it was declared against, because the Gegenforderung decides
//!   whether the Netzbetreiber also owes the ÜNB the amount it collected that way.
//!
//! Source: gesetze-im-internet.de `enfg/__13.html`, `__50.html`, read 2026-11-08.

use serde::Serialize;
use time::Date;
use uuid::Uuid;

/// The pain.002 status that confirms a payout: `ACSC`, settlement completed.
pub const GELEISTET: &str = "ACSC";

/// Whether a payout order with `status` counts as a payment made.
#[must_use]
pub fn ist_geleistet(status: Option<&str>) -> bool {
    status == Some(GELEISTET)
}

/// The Netzbetreiber's claim a plant operator's EEG claim was set off against.
///
/// Each variant is one of the set-offs the EEG 2023 admits against a § 19
/// Abs. 1 claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Gegenforderung {
    /// A § 52 EEG 2023 Pflichtzahlung — § 52 Abs. 6 Satz 2 lets it be set off
    /// against the § 19 Abs. 1 claim „abweichend von § 27 Absatz 1".
    #[serde(rename = "EEG_52_PFLICHTZAHLUNG")]
    Eeg52Pflichtzahlung,
    /// An EnFG Umlage the operator owes as Umlagenschuldner — § 27 Abs. 1
    /// Satz 2 EEG 2023.
    EnfgUmlage,
    /// Any other claim of the Netzbetreiber, admissible only „soweit die
    /// Forderung unbestritten oder rechtskräftig festgestellt ist" (§ 27 Abs. 1
    /// Satz 1 EEG 2023).
    Unbestritten,
}

impl Gegenforderung {
    /// The storage token.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Eeg52Pflichtzahlung => "EEG_52_PFLICHTZAHLUNG",
            Self::EnfgUmlage => "ENFG_UMLAGE",
            Self::Unbestritten => "UNBESTRITTEN",
        }
    }

    /// Parse a storage token.
    #[must_use]
    pub fn from_token(token: &str) -> Option<Self> {
        [
            Self::Eeg52Pflichtzahlung,
            Self::EnfgUmlage,
            Self::Unbestritten,
        ]
        .into_iter()
        .find(|g| g.token() == token)
    }
}

/// How a payment was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Quelle {
    /// A settled pain.001 credit transfer.
    Auszahlung,
    /// A claim extinguished by Aufrechnung (§ 13 Abs. 1 Satz 2 EnFG).
    Aufrechnung,
}

/// One `eeg_payout_orders` row, as read.
#[derive(Debug, Clone)]
pub struct PayoutRow {
    pub payout_id: Uuid,
    pub tr_id: String,
    pub malo_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub amount_ct: i64,
    pub pain002_status: Option<String>,
    pub end_to_end_ref: String,
}

/// One `eeg_aufrechnungen` row, as read.
#[derive(Debug, Clone)]
pub struct AufrechnungRow {
    pub aufrechnung_id: Uuid,
    pub tr_id: String,
    pub malo_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub amount_ct: i64,
    pub gegenforderung: Gegenforderung,
    pub erklaert_am: Date,
}

/// A payment made for one plant and billing month.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GeleisteteZahlung {
    pub quelle: Quelle,
    /// `payout_id` or `aufrechnung_id`.
    pub id: Uuid,
    /// The plant: every payout order and Aufrechnung names one
    /// (`eeg_payout_orders.tr_id` is `NOT NULL`), so every payment served is
    /// attributable.
    pub tr_id: String,
    pub malo_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    /// The amount paid, in euro cent.
    pub amount_ct: i64,
    /// For an Aufrechnung: what the claim was set off against.
    pub gegenforderung: Option<Gegenforderung>,
    /// For an Aufrechnung: the day it was declared, `YYYY-MM-DD`.
    pub erklaert_am: Option<String>,
    /// For a payout: its ISO 20022 EndToEndId.
    pub end_to_end_ref: Option<String>,
}

/// The payments made out of `payouts` and `aufrechnungen` — the settled payouts
/// and every Aufrechnung, nothing else.
#[must_use]
pub fn geleistete_zahlungen(
    payouts: Vec<PayoutRow>,
    aufrechnungen: Vec<AufrechnungRow>,
) -> Vec<GeleisteteZahlung> {
    let mut out: Vec<GeleisteteZahlung> = payouts
        .into_iter()
        .filter(|p| ist_geleistet(p.pain002_status.as_deref()))
        .map(|p| GeleisteteZahlung {
            quelle: Quelle::Auszahlung,
            id: p.payout_id,
            tr_id: p.tr_id,
            malo_id: p.malo_id,
            billing_year: p.billing_year,
            billing_month: p.billing_month,
            amount_ct: p.amount_ct,
            gegenforderung: None,
            erklaert_am: None,
            end_to_end_ref: Some(p.end_to_end_ref),
        })
        .collect();
    out.extend(aufrechnungen.into_iter().map(|a| GeleisteteZahlung {
        quelle: Quelle::Aufrechnung,
        id: a.aufrechnung_id,
        tr_id: a.tr_id,
        malo_id: a.malo_id,
        billing_year: a.billing_year,
        billing_month: a.billing_month,
        amount_ct: a.amount_ct,
        gegenforderung: Some(a.gegenforderung),
        erklaert_am: Some(a.erklaert_am.to_string()),
        end_to_end_ref: None,
    }));
    out.sort_by(|a, b| (&a.tr_id, a.id).cmp(&(&b.tr_id, b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payout(status: Option<&str>) -> PayoutRow {
        PayoutRow {
            payout_id: Uuid::new_v4(),
            tr_id: "TR-1".to_owned(),
            malo_id: "51238696012".to_owned(),
            billing_year: 2026,
            billing_month: 3,
            amount_ct: 12_345,
            pain002_status: status.map(str::to_owned),
            end_to_end_ref: "EEG-5123869601-2026-03-AUTO".to_owned(),
        }
    }

    /// Only `ACSC` says the funds moved; every other status, and none, is not
    /// a payment made (§ 50 Nr. 1 Buchst. a EnFG: „tatsächlich geleisteten").
    #[test]
    fn a_payout_without_a_settling_status_is_not_paid() {
        assert!(ist_geleistet(Some("ACSC")));
        for s in [
            None,
            Some("ACTC"),
            Some("ACCP"),
            Some("ACSP"),
            Some("ACWC"),
            Some("PART"),
            Some("PDNG"),
            Some("RJCT"),
            Some("CANC"),
        ] {
            assert!(!ist_geleistet(s), "{s:?}");
        }
        let out = geleistete_zahlungen(vec![payout(Some("PDNG")), payout(Some("ACSC"))], vec![]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quelle, Quelle::Auszahlung);
    }

    /// § 13 Abs. 1 Satz 2 EnFG: a claim extinguished by Aufrechnung is paid.
    #[test]
    fn an_aufrechnung_is_returned_as_paid() {
        let a = AufrechnungRow {
            aufrechnung_id: Uuid::new_v4(),
            tr_id: "TR-2".to_owned(),
            malo_id: "51238696012".to_owned(),
            billing_year: 2026,
            billing_month: 3,
            amount_ct: 9_500,
            gegenforderung: Gegenforderung::Eeg52Pflichtzahlung,
            erklaert_am: time::macros::date!(2026 - 04 - 10),
        };
        let out = geleistete_zahlungen(vec![], vec![a.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].quelle, Quelle::Aufrechnung);
        assert_eq!(out[0].id, a.aufrechnung_id);
        assert_eq!(out[0].amount_ct, 9_500);
        assert_eq!(
            out[0].gegenforderung,
            Some(Gegenforderung::Eeg52Pflichtzahlung)
        );
    }

    #[test]
    fn gegenforderung_tokens_round_trip() {
        for g in [
            Gegenforderung::Eeg52Pflichtzahlung,
            Gegenforderung::EnfgUmlage,
            Gegenforderung::Unbestritten,
        ] {
            assert_eq!(Gegenforderung::from_token(g.token()), Some(g));
            // The wire spelling is the storage token — einsd parses the same word.
            assert_eq!(
                serde_json::to_value(g).expect("serialises"),
                serde_json::Value::String(g.token().to_owned())
            );
        }
    }
}

// ── Persistence ───────────────────────────────────────────────────────────────

/// A recorded Aufrechnung, as the operator declares it.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AufrechnungRequest {
    /// The plant whose claim was set off.
    pub tr_id: String,
    pub malo_id: String,
    /// The billing month of the extinguished claim.
    pub billing_year: i16,
    pub billing_month: i16,
    /// The amount extinguished, in euro cent; positive.
    pub amount_ct: i64,
    pub gegenforderung: Gegenforderung,
    /// The day the Aufrechnung was declared, `YYYY-MM-DD`.
    pub erklaert_am: String,
    pub vermerk: Option<String>,
}

/// Why an Aufrechnung is refused before it is stored.
#[derive(Debug, thiserror::Error)]
pub enum AufrechnungFehler {
    #[error("{0}")]
    Ungueltig(String),
    #[error("an Aufrechnung for this plant, month, Gegenforderung and day is already recorded")]
    Doppelt,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Store one Aufrechnung and return its id.
///
/// # Errors
///
/// [`AufrechnungFehler::Ungueltig`] for a non-positive amount, a month outside
/// 1–12, a malformed or future declaration date or an empty `tr_id`;
/// [`AufrechnungFehler::Doppelt`] for a repeat of a recorded declaration.
pub async fn record_aufrechnung(
    pool: &sqlx::PgPool,
    tenant: &str,
    req: &AufrechnungRequest,
    erfasst_von: Option<&str>,
) -> Result<Uuid, AufrechnungFehler> {
    if req.tr_id.trim().is_empty() {
        return Err(AufrechnungFehler::Ungueltig("tr_id is empty".to_owned()));
    }
    if req.amount_ct <= 0 {
        return Err(AufrechnungFehler::Ungueltig(
            "amount_ct must be positive — the amount of the extinguished claim".to_owned(),
        ));
    }
    if !(1..=12).contains(&req.billing_month) {
        return Err(AufrechnungFehler::Ungueltig(
            "billing_month must be 1–12".to_owned(),
        ));
    }
    let erklaert_am = mako_service::dates::parse_iso_date("erklaert_am", &req.erklaert_am)
        .map_err(|e| AufrechnungFehler::Ungueltig(e.to_string()))?;
    if erklaert_am > mako_fristen::heute() {
        return Err(AufrechnungFehler::Ungueltig(format!(
            "erklaert_am {erklaert_am} lies in the future — an Aufrechnung is recorded once declared"
        )));
    }
    let id: Option<Uuid> = sqlx::query_scalar(
        "INSERT INTO eeg_aufrechnungen
             (tenant, tr_id, malo_id, billing_year, billing_month, amount_ct,
              gegenforderung, erklaert_am, vermerk, erfasst_von)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
         ON CONFLICT (tenant, tr_id, billing_year, billing_month, gegenforderung, erklaert_am)
         DO NOTHING
         RETURNING aufrechnung_id",
    )
    .bind(tenant)
    .bind(&req.tr_id)
    .bind(&req.malo_id)
    .bind(req.billing_year)
    .bind(req.billing_month)
    .bind(req.amount_ct)
    .bind(req.gegenforderung.token())
    .bind(erklaert_am)
    .bind(req.vermerk.as_deref())
    .bind(erfasst_von)
    .fetch_optional(pool)
    .await?;
    id.ok_or(AufrechnungFehler::Doppelt)
}

/// The payments made for `tenant`'s plants in one billing month: settled
/// payouts and recorded Aufrechnungen.
///
/// # Errors
///
/// A database failure, or a stored Gegenforderung outside the vocabulary.
pub async fn zahlungen_im_monat(
    pool: &sqlx::PgPool,
    tenant: &str,
    year: i16,
    month: i16,
) -> anyhow::Result<Vec<GeleisteteZahlung>> {
    use sqlx::Row as _;
    let payouts = sqlx::query(
        "SELECT payout_id, tr_id, malo_id, billing_year, billing_month, amount_ct,
                pain002_status, end_to_end_ref
           FROM eeg_payout_orders
          WHERE tenant = $1 AND billing_year = $2 AND billing_month = $3",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| PayoutRow {
        payout_id: r.get("payout_id"),
        tr_id: r.get("tr_id"),
        malo_id: r.get("malo_id"),
        billing_year: r.get("billing_year"),
        billing_month: r.get("billing_month"),
        amount_ct: r.get("amount_ct"),
        pain002_status: r.get("pain002_status"),
        end_to_end_ref: r.get("end_to_end_ref"),
    })
    .collect();
    let mut aufrechnungen = Vec::new();
    for r in sqlx::query(
        "SELECT aufrechnung_id, tr_id, malo_id, billing_year, billing_month, amount_ct,
                gegenforderung, erklaert_am
           FROM eeg_aufrechnungen
          WHERE tenant = $1 AND billing_year = $2 AND billing_month = $3",
    )
    .bind(tenant)
    .bind(year)
    .bind(month)
    .fetch_all(pool)
    .await?
    {
        let token: String = r.get("gegenforderung");
        let gegenforderung = Gegenforderung::from_token(&token)
            .ok_or_else(|| anyhow::anyhow!("eeg_aufrechnungen.gegenforderung {token:?} unknown"))?;
        aufrechnungen.push(AufrechnungRow {
            aufrechnung_id: r.get("aufrechnung_id"),
            tr_id: r.get("tr_id"),
            malo_id: r.get("malo_id"),
            billing_year: r.get("billing_year"),
            billing_month: r.get("billing_month"),
            amount_ct: r.get("amount_ct"),
            gegenforderung,
            erklaert_am: r.get("erklaert_am"),
        });
    }
    Ok(geleistete_zahlungen(payouts, aufrechnungen))
}
