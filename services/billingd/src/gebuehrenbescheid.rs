//! A public-law Gebühr is issued as a Bescheid, beside — never inside — the
//! invoice for the taxable supply.
//!
//! A municipal operator bills Trinkwasser (a taxable supply, 7 %) and the
//! öffentlich-rechtliche Abwassergebühr in one period. The Gebühr is outside
//! the UStG, EN 16931 category `O`, and BR-O-11 … BR-O-14 make `O` exclusive:
//! a document carrying both has no valid e-invoice. So the period is issued as
//! two records under one `billingRunId`: the supply invoice, which carries its
//! `en16931_json`, and a Gebührenbescheid (`dokumentart = GEBUEHRENBESCHEID`),
//! which is no invoice and is rendered by `outputd`'s `GEBUEHRENBESCHEID` kind.
//!
//! The Bescheid states what § 157 Abs. 1 AO attaches to a Steuerbescheid
//! (gesetze-im-internet.de, read 2026-11-10): the levy „nach Art und Betrag",
//! „wer die Steuer schuldet", and a Belehrung „welcher Rechtsbehelf zulässig ist
//! und binnen welcher Frist und bei welcher Behörde er einzulegen ist". The
//! Rechtsbehelfsbelehrung is the operator's — `[gebuehrenbescheid]` — and a
//! period with a Gebühr is refused without it.

use energy_billing::{BillingPosition, Invoice, PositionCategory};
use rust_decimal::Decimal;

use crate::error::{BillingError, BillingResult};

/// `billing_records.dokumentart` of an ordinary invoice.
pub const RECHNUNG: &str = "RECHNUNG";
/// `billing_records.dokumentart` of a Gebührenbescheid.
pub const GEBUEHRENBESCHEID: &str = "GEBUEHRENBESCHEID";

/// The suffix a Bescheid's `product_code` carries, so it and the supply
/// invoice of the same period occupy two slots of `br_unique_original`.
pub const PRODUCT_SUFFIX: &str = ":GEBUEHR";

/// The documents one billed period is issued as.
#[derive(Debug)]
pub struct Belege {
    /// The taxable supply, if the period carries any.
    pub lieferung: Option<Invoice>,
    /// The public-law Gebühren, if the period carries any.
    pub gebuehr: Option<Invoice>,
}

fn gehoert_zur_gebuehr(p: &BillingPosition) -> bool {
    p.is_out_of_scope() || (p.category == PositionCategory::Info && p.has_tag("abwasser"))
}

fn bepreist(p: &BillingPosition) -> bool {
    !matches!(
        p.category,
        PositionCategory::Info | PositionCategory::Tax | PositionCategory::Abschlag
    )
}

/// The warning that a Gebühr and a supply share a document — settled by the
/// split, so neither document carries it.
const GEMISCHT: &str = "GEBUEHR_UND_ENTGELT_AUF_EINEM_BELEG";

/// Split a billed period into its supply invoice and its Gebührenbescheid.
///
/// An invoice with no out-of-scope position comes back unchanged as the supply
/// invoice. The Abschläge the period settles stay on the supply invoice.
#[must_use]
pub fn trennen(invoice: Invoice) -> Belege {
    if !invoice
        .positions
        .iter()
        .any(BillingPosition::is_out_of_scope)
    {
        return Belege {
            lieferung: Some(invoice),
            gebuehr: None,
        };
    }
    let (gebuehr, lieferung): (Vec<BillingPosition>, Vec<BillingPosition>) = invoice
        .positions
        .iter()
        .cloned()
        .partition(gehoert_zur_gebuehr);
    let warnings: Vec<_> = invoice
        .warnings
        .iter()
        .filter(|w| w.code != GEMISCHT)
        .cloned()
        .collect();
    let lieferung = lieferung.iter().any(bepreist).then(|| {
        let mut ctx = invoice.context.clone();
        ctx.billing_run_id.clone_from(&invoice.billing_run_id);
        Invoice::from_positions(ctx, lieferung, warnings.clone())
    });
    let mut ctx = invoice.context.clone();
    ctx.abschlage.clear();
    ctx.billing_run_id.clone_from(&invoice.billing_run_id);
    let gebuehr = Some(Invoice::from_positions(ctx, gebuehr, warnings));
    Belege { lieferung, gebuehr }
}

/// The two documents of one period belong to one run: when the period was
/// billed outside a run, the pair gets its own.
pub fn eine_lauf_id(belege: &mut Belege) {
    let (Some(l), Some(g)) = (belege.lieferung.as_mut(), belege.gebuehr.as_mut()) else {
        return;
    };
    let id = l
        .billing_run_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    for inv in [l, g] {
        inv.billing_run_id = Some(id.clone());
        inv.context.billing_run_id = Some(id.clone());
    }
}

/// A decimal as a German reader reads it — `84`, `2,35`.
fn deutsch(d: Decimal) -> String {
    d.normalize().to_string().replace('.', ",")
}

/// The `outputd` `GEBUEHRENBESCHEID` view of `gebuehr`.
///
/// # Errors
///
/// `422 GEBUEHRENBESCHEID_OHNE_RECHTSBEHELFSBELEHRUNG` when `[gebuehrenbescheid]`
/// is not configured — a Bescheid without it does not state what § 157 Abs. 1
/// Satz 3 AO attaches to one.
pub fn view(
    gebuehr: &Invoice,
    cfg: &crate::config::BillingdConfig,
) -> BillingResult<serde_json::Value> {
    let Some(b) = cfg.gebuehrenbescheid.as_ref() else {
        return Err(BillingError::unprocessable(
            "GEBUEHRENBESCHEID_OHNE_RECHTSBEHELFSBELEHRUNG",
            "the period carries a public-law Gebühr, which is issued as a Gebührenbescheid, and \
             [gebuehrenbescheid] states no rechtsbehelf, frist and behoerde — a Bescheid \
             without the Rechtsbehelfsbelehrung does not state what § 157 Abs. 1 Satz 3 AO \
             attaches to one",
        ));
    };
    let ctx = &gebuehr.context;
    let positionen: Vec<serde_json::Value> = gebuehr
        .positions
        .iter()
        .filter(|p| bepreist(p))
        .map(|p| {
            serde_json::json!({
                "art": p.description,
                "bemessungsgrundlage": format!("{} {}", deutsch(p.quantity), p.unit),
                "satz": format!("{} EUR/{}", deutsch(p.unit_price_eur), p.unit),
                "betrag": p.net_eur.to_string(),
            })
        })
        .collect();
    let seller = cfg.seller.as_ref();
    let party =
        |name: Option<&str>, line1: Option<&str>, post_code: Option<&str>, city: Option<&str>| {
            serde_json::json!({
                "name": name, "vat_id": null, "tax_number": null, "line1": line1,
                "post_code": post_code, "city": city, "country": "DE",
                "contact_name": null, "phone": null, "email": null,
            })
        };
    let empf = ctx.rechnungsempfaenger.as_ref();
    if empf
        .and_then(|e| e.name.as_deref())
        .is_none_or(str::is_empty)
    {
        return Err(BillingError::unprocessable(
            "GEBUEHRENBESCHEID_OHNE_SCHULDNER",
            format!(
                "the Gebührenbescheid for MaLo {} names no Schuldner — § 157 Abs. 1 Satz 2 AO \
                 requires it to state who owes the levy; vertragd holds no \
                 Rechnungsempfänger for the contract",
                ctx.malo_id
            ),
        ));
    }
    let ausgestellt = ctx.ausstellungsdatum();
    let faellig = ausgestellt + time::Duration::days(i64::from(b.zahlungsfrist_tage));
    Ok(serde_json::json!({
        "datum": ausgestellt.to_string(),
        "bescheidnummer": ctx.rechnungsnummer,
        "absender": party(
            Some(cfg.seller_name.as_deref().unwrap_or(&cfg.tenant)),
            seller.and_then(|s| s.street.as_deref()),
            seller.and_then(|s| s.post_code.as_deref()),
            seller.and_then(|s| s.city.as_deref()),
        ),
        "schuldner": party(
            empf.and_then(|e| e.name.as_deref()),
            empf.and_then(|e| e.line1.as_deref()),
            empf.and_then(|e| e.post_code.as_deref()),
            empf.and_then(|e| e.city.as_deref()),
        ),
        "malo_id": ctx.malo_id,
        "zeitraum_von": ctx.period.from().to_string(),
        "zeitraum_bis": ctx.period.to().to_string(),
        "positionen": positionen,
        "festgesetzt": gebuehr.brutto_eur.to_string(),
        "faellig_am": faellig.to_string(),
        "iban": cfg.seller_iban,
        "rechtsbehelf": {
            "rechtsbehelf": b.rechtsbehelf,
            "frist": b.frist,
            "behoerde": b.behoerde,
        },
    }))
}

/// Write `gebuehr` as a Gebührenbescheid record in the caller's transaction and,
/// unless the period is held, enqueue its `de.billing.rechnung.erstellt` — the
/// Gebühr is a receivable like the invoice beside it.
///
/// # Errors
///
/// The record cannot be written.
pub async fn speichern(
    tx: &mut sqlx::PgConnection,
    cfg: &crate::config::BillingdConfig,
    gebuehr: &Invoice,
    view: &serde_json::Value,
    product_code: &str,
    category: &str,
    held: bool,
) -> Result<uuid::Uuid, crate::pg::InsertError> {
    let ctx = &gebuehr.context;
    let rechnung_json = gebuehr.to_rechnung_json();
    let product_code = format!("{product_code}{PRODUCT_SUFFIX}");
    let id = crate::pg::insert_billing_record(
        &mut *tx,
        &crate::pg::NewBillingRecord {
            tenant: &cfg.tenant,
            malo_id: &ctx.malo_id,
            lf_mp_id: &ctx.lf_mp_id,
            product_code: &product_code,
            category,
            rechnungsnummer: &ctx.rechnungsnummer,
            period_from: ctx.period.from(),
            period_to: ctx.period.to(),
            rechnung_json: &rechnung_json,
            total_netto_eur: gebuehr.netto_eur,
            total_brutto_eur: gebuehr.brutto_eur,
            steuerzeilen: &crate::steuerzeilen::aus_rechnung(gebuehr),
        },
    )
    .await?;
    crate::pg::attach_gebuehrenbescheid(&mut *tx, id, view).await?;
    if !held {
        let ce = crate::handlers::rechnung_erstellt_ce(
            id,
            &ctx.malo_id,
            &ctx.lf_mp_id,
            &rechnung_json,
            false,
        );
        crate::handlers::issue_record(&mut *tx, cfg, id, &ce)
            .await
            .map_err(|e| crate::pg::InsertError::Other(anyhow::anyhow!("{e}")))?;
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use energy_billing::{
        BillingContext, BillingPeriod, GridInput, InvoiceType, Product, Quantities,
        Rechnungsempfaenger, RegulatoryRates, WasserMeterInput,
    };
    use rust_decimal::dec;
    use time::macros::date;

    /// Trinkwasser and the öffentlich-rechtliche Schmutzwassergebühr for one
    /// month — what a municipal operator bills.
    fn gemischt() -> Invoice {
        let tariff: Product = serde_json::from_str(
            r#"{"category":"WASSER","wasser_grundpreis_eur_per_month":"8.0",
                 "wasser_mengenpreis_eur_per_m3":"2.10","schmutzwasser_eur_per_m3":"2.60",
                 "abwasser_regime":"PUBLIC_LAW_FEE"}"#,
        )
        .expect("a WASSER product");
        let rates = RegulatoryRates::default();
        let ctx = BillingContext {
            malo_id: "51238696781".to_owned(),
            lf_mp_id: "9900000000001".to_owned(),
            rechnungsnummer: "RE-2026-000001".to_owned(),
            period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
                .expect("a period"),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: rates.clone(),
            rechnungsempfaenger: Some(Rechnungsempfaenger {
                name: Some("Erika Mustermann".to_owned()),
                line1: Some("Beispielweg 7".to_owned()),
                post_code: Some("10115".to_owned()),
                city: Some("Berlin".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        };
        tariff
            .build_engine(&GridInput::default(), &rates)
            .bill(
                ctx,
                &Quantities {
                    wasser: Some(WasserMeterInput {
                        frischwasser_m3: dec!(120),
                        months: Some(dec!(1)),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .expect("a combined period bills")
    }

    fn cfg(bescheid: bool) -> crate::config::BillingdConfig {
        let mut v = serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "productd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080",
            "seller_name": "Stadtwerke Musterstadt",
        });
        if bescheid {
            v["gebuehrenbescheid"] = serde_json::json!({
                "rechtsbehelf": "Widerspruch",
                "frist": "innerhalb eines Monats nach Bekanntgabe",
                "behoerde": "Stadtwerke Musterstadt, Musterstraße 1, 12345 Musterstadt",
                "zahlungsfrist_tage": 30,
            });
        }
        serde_json::from_value(v).expect("config parses")
    }

    /// **Invariant: a period carrying taxable supply and a public-law Gebühr is
    /// issued as two documents under one run, and the supply one has an
    /// e-invoice.** As one document it has none (BR-O-11 ff.); split, the
    /// supply invoice maps to EN 16931, the Bescheid carries only the `O`
    /// lines, and the two add up to the period.
    #[test]
    fn a_mixed_period_is_two_documents_under_one_run() {
        let cfg = cfg(true);
        let ganz = gemischt();
        assert!(
            crate::einvoice::build(&ganz, &cfg).is_err(),
            "as one document the period has no e-invoice"
        );

        let mut belege = trennen(ganz.clone());
        eine_lauf_id(&mut belege);
        let lieferung = belege.lieferung.expect("the Trinkwasser invoice");
        let gebuehr = belege.gebuehr.expect("the Gebührenbescheid");

        crate::einvoice::build(&lieferung, &cfg).expect("the supply invoice has its e-invoice");
        assert!(
            !lieferung
                .positions
                .iter()
                .any(BillingPosition::is_out_of_scope),
            "no Gebühr on the invoice"
        );
        assert!(
            gebuehr
                .positions
                .iter()
                .filter(|p| bepreist(p))
                .all(BillingPosition::is_out_of_scope),
            "nothing taxable on the Bescheid"
        );
        assert_eq!(lieferung.netto_eur + gebuehr.netto_eur, ganz.netto_eur);
        assert_eq!(lieferung.brutto_eur + gebuehr.brutto_eur, ganz.brutto_eur);
        assert!(lieferung.billing_run_id.is_some());
        assert_eq!(lieferung.billing_run_id, gebuehr.billing_run_id);
        assert!(
            !lieferung
                .warnings
                .iter()
                .chain(&gebuehr.warnings)
                .any(|w| w.code == GEMISCHT),
            "the split settles the mixed-document warning"
        );
    }

    /// An invoice without a Gebühr comes back unchanged.
    #[test]
    fn an_ordinary_invoice_is_not_split() {
        let mut ganz = gemischt();
        ganz.positions.retain(|p| !p.is_out_of_scope());
        let belege = trennen(ganz);
        assert!(belege.lieferung.is_some() && belege.gebuehr.is_none());
    }

    /// The Bescheid's view states each Gebühr by Art and Betrag, the
    /// Schuldner and the Rechtsbehelfsbelehrung — and is refused without the
    /// operator's `[gebuehrenbescheid]`.
    #[test]
    fn the_view_states_what_section_157_ao_attaches() {
        let gebuehr = trennen(gemischt()).gebuehr.expect("a Bescheid");
        let err = view(&gebuehr, &cfg(false)).expect_err("no Rechtsbehelfsbelehrung");
        assert_eq!(err.code(), "GEBUEHRENBESCHEID_OHNE_RECHTSBEHELFSBELEHRUNG");

        let v = view(&gebuehr, &cfg(true)).expect("configured");
        assert_eq!(v["schuldner"]["name"], "Erika Mustermann");
        assert_eq!(v["positionen"][0]["art"], "Schmutzwassergebühr");
        assert_eq!(v["positionen"][0]["bemessungsgrundlage"], "120 m³");
        assert_eq!(v["rechtsbehelf"]["rechtsbehelf"], "Widerspruch");
        assert_eq!(v["festgesetzt"], gebuehr.brutto_eur.to_string());
        assert_eq!(v["faellig_am"], {
            let d = gebuehr.context.ausstellungsdatum() + time::Duration::days(30);
            d.to_string()
        });

        let mut ohne = gebuehr.clone();
        ohne.context.rechnungsempfaenger = None;
        assert_eq!(
            view(&ohne, &cfg(true)).expect_err("no Schuldner").code(),
            "GEBUEHRENBESCHEID_OHNE_SCHULDNER"
        );
    }
}
