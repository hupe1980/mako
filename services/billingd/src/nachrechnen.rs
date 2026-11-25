//! Rechnung nachrechnen — the seal, the verify token and the two views.
//!
//! At issue every invoice is **sealed**: SHA-256 over the RFC 8785 (JCS)
//! canonical form of its stored `rechnung_json`. BO4E carries every amount as a
//! decimal string, so no float enters the canonical form. A recomputation
//! states whether the stored document still hashes to its seal, beside whether
//! its lines and totals recompute ([`energy_billing::recompute`]).
//!
//! When `[nachrechnen]` is configured the invoice also gets a **verify
//! token**: HMAC-SHA256(`token_key`, record id), cut to 128 bits, base64url.
//! Only its SHA-256 is stored. The token is the one credential the public page
//! asks for, so it is derived, never stored in clear: the letter can print it
//! again on every re-send, and a read of the database yields no working link.
//! A revoked token answers like an unknown one.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use rust_decimal::Decimal;
use schemars::JsonSchema;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::BillingdConfig;
use crate::pg::BillingRecordRow;

/// SHA-256 (hex) over the RFC 8785 canonical form of a stored document.
///
/// # Errors
///
/// Only if the value cannot be canonicalised (a non-finite number, which a
/// `serde_json::Value` cannot hold).
pub fn seal(rechnung_json: &serde_json::Value) -> anyhow::Result<String> {
    let canonical = serde_jcs::to_vec(rechnung_json)?;
    Ok(hex(&Sha256::digest(canonical)))
}

/// The verify token of a record: HMAC-SHA256(key, record id), 128 bits,
/// base64url without padding (22 characters).
#[must_use]
pub fn verify_token(key: &[u8], record_id: Uuid) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(b"mako-nachrechnen-v1:");
    mac.update(record_id.as_bytes());
    let tag = mac.finalize().into_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tag[..16])
}

/// SHA-256 (hex) of a token — what is stored and looked up.
#[must_use]
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// The public link printed on the invoice, when `[nachrechnen]` is set.
#[must_use]
pub fn public_link(cfg: &BillingdConfig, record_id: Uuid) -> Option<String> {
    let n = cfg.nachrechnen.as_ref()?;
    let token = verify_token(n.token_key.as_bytes(), record_id);
    Some(format!("{}{token}", with_slash(&n.public_url)))
}

fn with_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_owned()
    } else {
        format!("{url}/")
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Seal a record and mint its token hash, inside the issuing transaction.
///
/// # Errors
///
/// A failed query, or a stored document that cannot be canonicalised.
pub async fn seal_record(
    tx: &mut sqlx::PgConnection,
    cfg: &BillingdConfig,
    record_id: Uuid,
) -> anyhow::Result<()> {
    let json: serde_json::Value =
        sqlx::query_scalar("SELECT rechnung_json FROM billing_records WHERE id = $1")
            .bind(record_id)
            .fetch_one(&mut *tx)
            .await?;
    let seal = seal(&json)?;
    let token = cfg
        .nachrechnen
        .as_ref()
        .map(|n| token_hash(&verify_token(n.token_key.as_bytes(), record_id)));
    sqlx::query(
        "UPDATE billing_records
            SET seal_sha256 = $2,
                verify_token_sha256 = COALESCE($3, verify_token_sha256)
          WHERE id = $1",
    )
    .bind(record_id)
    .bind(seal)
    .bind(token)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

// ── Views ─────────────────────────────────────────────────────────────────────

/// One recomputed line, amounts as decimal strings.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LineView {
    /// `positionsnummer`, from 1.
    pub position: i64,
    /// The line's text as printed.
    pub text: String,
    /// Quantity as stated.
    pub quantity: Option<String>,
    /// Unit (`KWH`, `Tage`, …).
    pub unit: Option<String>,
    /// Unit price in EUR, full precision.
    pub unit_price_eur: Option<String>,
    /// The line amount the invoice states.
    pub stated_eur: Option<String>,
    /// Quantity × unit price, to the cent.
    pub recomputed_eur: Option<String>,
    /// `stated − recomputed`.
    pub difference_eur: Option<String>,
    /// The line recomputes to the cent.
    pub matches: bool,
    /// The calculation as the engine wrote it.
    pub formula: Option<String>,
    /// Provisions the line bills under.
    pub rules: Vec<String>,
    /// Where the unit price comes from.
    pub price_source: Option<String>,
    /// How the quantity was obtained (§ 40 Abs. 2 Nr. 6 EnWG).
    pub quantity_source: Option<String>,
    /// The quantity is an estimate or an Ersatzwert (§ 40a EnWG).
    pub estimated: bool,
}

/// One recomputed document total.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TotalView {
    /// `NETTO`, `STEUER`, `STEUERBASIS`, `GESAMTSTEUER`, `BRUTTO`, `ZU_ZAHLEN`.
    pub kind: String,
    /// VAT rate in percent, on a `STEUER` row.
    pub rate_percent: Option<String>,
    /// The amount the invoice states.
    pub stated_eur: Option<String>,
    /// The amount recomputed.
    pub recomputed_eur: String,
    /// The two agree to the cent.
    pub matches: bool,
}

/// The verdict on one invoice.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Verdict {
    /// Every line and total recomputes and the seal holds — „Stimmt überein".
    pub matches: bool,
    /// Every line and total recomputes to the cent.
    pub arithmetic_matches: bool,
    /// The stored document still hashes to the seal taken at issue. `None`
    /// for a record never issued (no seal yet).
    pub seal_holds: Option<bool>,
    /// The positions whose stated amount does not recompute.
    pub mismatched_positions: Vec<i64>,
    /// The invoice was cancelled by a Storno.
    pub storniert: bool,
}

/// `recompute_invoice` / `GET /api/v1/billing/{id}/nachrechnung`: the full
/// operator view.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Nachrechnung {
    /// The billing record.
    pub record_id: String,
    /// § 14 Abs. 4 Nr. 4 UStG Rechnungsnummer.
    pub rechnungsnummer: String,
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// First day of the billed period.
    #[schemars(extend("format" = "date"))]
    pub period_from: String,
    /// Last day of the billed period.
    #[schemars(extend("format" = "date"))]
    pub period_to: String,
    /// Issuance time from the billing record. Channel evidence is owned by outputd.
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(with = "Option<String>", extend("format" = "date-time"))]
    pub dispatched_at: Option<time::OffsetDateTime>,
    /// The verdict, on top.
    pub verdict: Verdict,
    /// Every line, in the invoice's order.
    pub lines: Vec<LineView>,
    /// Every document total.
    pub totals: Vec<TotalView>,
    /// Sum of the Vorauszahlungen deducted.
    pub vorauszahlungen_eur: String,
    /// The public Nachrechnen link printed on the invoice (and in its QR
    /// code), when `[nachrechnen]` is configured — what the customer opens.
    #[schemars(extend("format" = "uri"))]
    pub public_link: Option<String>,
}

/// The public page's projection: no name, no address, the MaLo-ID masked.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PublicNachrechnung {
    /// § 14 Abs. 4 Nr. 4 UStG Rechnungsnummer — printed on the customer's own
    /// document, so it identifies nothing they do not hold.
    pub rechnungsnummer: String,
    /// The MaLo-ID with all but its first and last three digits masked.
    pub malo_id_masked: String,
    /// First day of the billed period.
    #[schemars(extend("format" = "date"))]
    pub period_from: String,
    /// Last day of the billed period.
    #[schemars(extend("format" = "date"))]
    pub period_to: String,
    /// Gross total the invoice states.
    pub total_brutto_eur: String,
    /// Amount due (negative: a credit), as stated.
    pub zu_zahlen_eur: Option<String>,
    /// The verdict.
    pub verdict: Verdict,
    /// Every line, in the invoice's order.
    pub lines: Vec<LineView>,
    /// Every document total.
    pub totals: Vec<TotalView>,
}

fn s(d: Decimal) -> String {
    d.normalize().to_string()
}

/// Mask a MaLo-ID to its first and last three characters.
#[must_use]
pub fn mask(id: &str) -> String {
    let n = id.chars().count();
    if n <= 6 {
        return "*".repeat(n);
    }
    id.chars()
        .enumerate()
        .map(|(i, c)| if i < 3 || i >= n - 3 { c } else { '*' })
        .collect()
}

/// Recompute a stored record.
///
/// # Errors
///
/// The stored JSON is not a BO4E `Rechnung`.
pub fn nachrechnen(row: &BillingRecordRow) -> anyhow::Result<Nachrechnung> {
    let rechnung: rubo4e::current::Rechnung = serde_json::from_value(row.rechnung_json.clone())?;
    let r = energy_billing::recompute::recompute(&rechnung);
    let seal_holds = match &row.seal_sha256 {
        Some(stored) => Some(seal(&row.rechnung_json)? == *stored),
        None => None,
    };
    let arithmetic_matches = r.matches;
    let lines = r
        .lines
        .iter()
        .map(|l| LineView {
            position: l.position,
            text: l.text.clone(),
            quantity: l.quantity.map(s),
            unit: l.unit.clone(),
            unit_price_eur: l.unit_price_eur.map(s),
            stated_eur: l.stated_eur.map(s),
            recomputed_eur: l.recomputed_eur.map(s),
            difference_eur: l.difference_eur.map(s),
            matches: l.matches,
            formula: l.formula.clone(),
            rules: l.rules.clone(),
            price_source: l.price_source.clone(),
            quantity_source: l.quantity_source.clone(),
            estimated: l.estimated,
        })
        .collect();
    let totals = r
        .totals
        .iter()
        .map(|t| TotalView {
            kind: serde_json::to_value(t.kind)
                .ok()
                .and_then(|v| v.as_str().map(ToOwned::to_owned))
                .unwrap_or_default(),
            rate_percent: t.rate_percent.map(s),
            stated_eur: t.stated_eur.map(s),
            recomputed_eur: s(t.recomputed_eur),
            matches: t.matches,
        })
        .collect();
    Ok(Nachrechnung {
        record_id: row.id.to_string(),
        dispatched_at: row.dispatched_at,
        rechnungsnummer: row.rechnungsnummer.clone(),
        malo_id: row.malo_id.clone(),
        period_from: row.period_from.to_string(),
        period_to: row.period_to.to_string(),
        verdict: Verdict {
            matches: arithmetic_matches && seal_holds != Some(false),
            arithmetic_matches,
            seal_holds,
            mismatched_positions: r.mismatched_lines(),
            storniert: row.outcome == "cancelled",
        },
        lines,
        totals,
        vorauszahlungen_eur: s(r.vorauszahlungen_eur),
        public_link: None,
    })
}

/// The masked projection of [`nachrechnen`] the public page reads.
///
/// # Errors
///
/// As [`nachrechnen`].
pub fn public_view(row: &BillingRecordRow) -> anyhow::Result<PublicNachrechnung> {
    let n = nachrechnen(row)?;
    let zu_zahlen = row
        .rechnung_json
        .pointer("/zuZahlen/wert")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    Ok(PublicNachrechnung {
        rechnungsnummer: n.rechnungsnummer,
        malo_id_masked: mask(&n.malo_id),
        period_from: n.period_from,
        period_to: n.period_to,
        total_brutto_eur: s(row.total_brutto_eur),
        zu_zahlen_eur: zu_zahlen,
        verdict: n.verdict,
        lines: n.lines,
        totals: n.totals,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_128_bits_and_tied_to_the_record() {
        let key = b"0123456789abcdef0123456789abcdef";
        let a = verify_token(key, Uuid::nil());
        assert_eq!(a.len(), 22, "16 bytes, base64url without padding");
        assert_ne!(a, verify_token(key, Uuid::max()));
        assert_ne!(
            a,
            verify_token(b"another key, also 32 bytes long!", Uuid::nil())
        );
        assert_eq!(token_hash(&a).len(), 64);
    }

    #[test]
    fn the_seal_ignores_key_order_and_sees_a_cent() {
        let a = serde_json::json!({"gesamtbrutto": {"wert": "132.48"}, "_typ": "RECHNUNG"});
        let b = serde_json::json!({"_typ": "RECHNUNG", "gesamtbrutto": {"wert": "132.48"}});
        let c = serde_json::json!({"_typ": "RECHNUNG", "gesamtbrutto": {"wert": "132.49"}});
        assert_eq!(seal(&a).unwrap(), seal(&b).unwrap());
        assert_ne!(seal(&a).unwrap(), seal(&c).unwrap());
    }

    #[test]
    fn a_malo_id_is_masked_to_its_ends() {
        assert_eq!(mask("51238696012"), "512*****012");
    }
}
