//! Category dispatch via `BillingEngine` — quantities and invoice assembly.

#[allow(unused_imports)]
use super::*;

// ── Category dispatch via BillingEngine ──────────────────────────────────────

/// Build the `Quantities` for a billing request by resolving meter data.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn build_quantities(
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    tarifbd: &Arc<TarifbdClient>,
) -> Result<Quantities, (StatusCode, String)> {
    let mut q = Quantities {
        eeg_gutschrift_eur: req.eeg_gutschrift_eur,
        ..Default::default()
    };

    match tariff.category_str() {
        "STROM" | "WAERMEPUMPE" | "WALLBOX" => {
            let is_dynamic = match tariff {
                Product::Strom(p) => p.dynamic_epex,
                Product::Waermepumpe(p) | Product::Wallbox(p) => p.base.dynamic_epex,
                _ => false,
            };
            if is_dynamic {
                q.dynamic_intervals =
                    fetch_dynamic_intervals(malo_id, period_from, period_to, edmd).await;
                q.dynamic_epex_prices = fetch_epex_prices(period_from, period_to, tarifbd).await;
            } else {
                q.electricity =
                    Some(resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?);
            }
        }
        "GAS" => {
            let mut meter = req.gas_meter.clone().unwrap_or_default();
            enrich_gas_meter(&mut meter, malo_id, period_from, period_to, edmd, marktd).await;
            q.gas = Some(meter);
        }
        "WAERME" => {
            q.heat = Some(req.waerme_meter.clone().unwrap_or_default());
        }
        "WASSER" => {
            q.wasser = Some(req.wasser_meter.clone().unwrap_or_default());
        }
        "SOLAR" => {
            q.solar = Some(req.solar_meter.clone().unwrap_or_default());
        }
        "EEG" | "EINSPEISUNG" => {
            q.eeg = Some(req.eeg_meter.clone().unwrap_or_default());
        }
        "HEMS" => {
            q.hems = Some(req.hems_meter.clone().unwrap_or_default());
        }
        "EMOBILITY" => {
            q.emobility = Some(req.emobility_meter.clone().unwrap_or_default());
        }
        "ENERGIEDIENSTLEISTUNG" => {
            q.service = Some(req.service_meter.clone().unwrap_or_default());
        }
        _ => {
            // Unknown category: try electricity as fallback
            q.electricity =
                Some(resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?);
        }
    }
    Ok(q)
}

/// Dispatch a billing request using the new `BillingEngine` architecture.
///
/// Replaces the old `dispatch_calculator` function.
/// Returns an `Invoice` instead of a `BillingResult`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_invoice(
    cfg: &BillingdConfig,
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rates: &RegulatoryRates,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    tarifbd: &Arc<TarifbdClient>,
    vertragd: &Arc<VertragdClient>,
) -> Result<Invoice, (StatusCode, String)> {
    let grid = req.grid.clone().unwrap_or_default();
    let quantities = build_quantities(
        tariff,
        req,
        malo_id,
        period_from,
        period_to,
        edmd,
        marktd,
        tarifbd,
    )
    .await?;

    // Generate a unique billing run ID for audit trail and duplicate detection.
    // Stored on the Invoice and propagated to the billing_records table.
    let run_id = Uuid::new_v4().to_string();

    // §40 Abs. 1 EnWG — the contract facts the invoice must state live in
    // vertragd, not in the tariff. Soft dependency: an unreachable vertragd
    // or an uncontracted MaLo degrades to an invoice without them, logged.
    let vertrag = match vertragd.get_vertrag_by_malo(malo_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(%malo_id, error = %e, "billingd: vertragd lookup failed — invoice will lack §40 contract facts");
            None
        }
    };

    // §40 Abs. 2 Nr. 6 EnWG — meter identity. The device registry (marktd)
    // is the authority: MaLo → Lokationszuordnung → MeLo → Zähler. Soft
    // dependency, logged when missing.
    let zaehler_id = resolve_zaehlernummer(marktd, malo_id).await;
    if zaehler_id.is_none() {
        tracing::warn!(%malo_id, "billingd: no Zählernummer resolvable via marktd — invoice will lack §40 Abs. 2 Nr. 6 meter identity");
    }

    // §40 Abs. 2 Nr. 7/8 EnWG — consumption comparison. Prior-year kWh from
    // edmd (same window one year earlier); the comparable-customer-group
    // value comes from operator config (Stromspiegel/BDEW reference data),
    // pro-rated to the billing period's length.
    let verbrauchshistorie =
        resolve_verbrauchshistorie(cfg, edmd, malo_id, period_from, period_to).await;
    let vertragsinformationen = vertrag.as_ref().map(|v| {
        energy_billing::Vertragsinformationen {
            vertragsdauer: Some(match v.vertrag.vertragsende {
                Some(ende) => format!("{} bis {ende}", v.vertrag.vertragsbeginn),
                None => format!("unbefristet seit {}", v.vertrag.vertragsbeginn),
            }),
            kuendigungsfrist: Some(match v.vertrag.kuendigungsfrist_monate {
                1 => "1 Monat".to_owned(),
                n => format!("{n} Monate"),
            }),
            naechstmoeglicher_kuendigungstermin: v.naechstmoeglicher_kuendigungstermin,
            // The next settlement follows the cadence of this one: a period of
            // the same length, starting the day after this one ends.
            naechster_abrechnungstermin: period_to.checked_add(time::Duration::days(
                (period_to - period_from).whole_days() + 1,
            )),
        }
    });

    let ctx = BillingContext {
        malo_id: malo_id.to_owned(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: rechnungsnummer.to_owned(),
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from < to"),
        // §40c EnWG: a Schlussrechnung (end of supply) settles the account;
        // the engine renders rechnungsart = SCHLUSSRECHNUNG and deducts the
        // paid Abschläge below from the Zahlbetrag.
        invoice_type: if req.schlussrechnung {
            InvoiceType::Final
        } else {
            InvoiceType::Initial
        },
        abschlage: req.abschlaege.clone(),
        // §13b Abs. 2 Nr. 5 lit. b UStG — supply to a Stromwiederverkäufer:
        // invoice net, recipient owes the VAT (EN 16931 `AE` tax breakdown).
        reverse_charge: req.reverse_charge,
        regulatory_rates: rates.clone(),
        contract_id: vertrag.as_ref().map(|v| {
            v.vertrag
                .vertrags_nr
                .clone()
                .unwrap_or_else(|| v.vertrag.id.clone())
        }),
        // §41 EnWG pro-rata: clip the billable days to the contract term.
        vertragsbeginn: vertrag.as_ref().map(|v| v.vertrag.vertragsbeginn),
        vertragsende: vertrag.as_ref().and_then(|v| v.vertrag.vertragsende),
        vertragsinformationen,
        // §40 Abs. 2 Nr. 6 EnWG — Zählernummer from the marktd device registry.
        zaehler_id,
        // §40 Abs. 2 Nr. 7/8 EnWG — Vorjahresverbrauch + Vergleichsgruppe.
        verbrauchshistorie,
        // §40 Abs. 2 Nr. 1/9/10/11/12 EnWG — supplier contact from config;
        // the statutory Schlichtungsstelle/BNetzA/Beratung hints come from
        // the engine defaults.
        verbraucherinformationen: Some(energy_billing::Verbraucherinformationen {
            lieferant_name: Some(
                cfg.seller_name
                    .clone()
                    .unwrap_or_else(|| cfg.tenant.clone()),
            ),
            lieferant_anschrift: cfg.seller_address.clone(),
            lieferant_kontakt: cfg.seller_contact.clone(),
            ..Default::default()
        }),
        // Propagate minimum invoice from product definition (tarifbd) to billing context.
        minimum_invoice_eur_brutto: tariff.minimum_invoice_eur_brutto(),
        // §42 EnWG — the product's Stromkennzeichnung, structured, so the
        // invoice can state the fuel mix and the mandatory CO₂ figure.
        energiequellen: tariff.energiequellen().cloned(),
        // §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification on invoice.
        nb_mp_id: req.nb_mp_id.clone(),
        // Audit trail: unique run ID links DB record to calculation output.
        billing_run_id: Some(run_id),
        ..Default::default()
    };

    let engine = tariff.build_engine(&grid, rates);

    let mut invoice = engine.bill(ctx, &quantities).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            engine_error_body(malo_id, &e),
        )
    })?;

    // §40c Abs. 1 EnWG — an Abrechnung must reach the customer within six weeks
    // of the end of the billed period, a Schlussrechnung within six weeks of the
    // end of the Lieferverhältnis, and **three weeks** where §40b Abs. 1 monthly
    // billing applies. The engine is clock-free by design, so the deadline is
    // checked here, where a clock legitimately exists: issue time is what the
    // law measures.
    //
    // The three-week rule keys on the agreed **cadence**, not on how long this
    // period happens to be. Inferring it from the day count made every short
    // period monthly — a ten-day move-out Schlussrechnung, entitled to six
    // weeks, was warned about after three.
    let deadline_weeks = sect40c_deadline_weeks(req.schlussrechnung, req.monatliche_abrechnung);
    let deadline = period_to + time::Duration::weeks(deadline_weeks);
    let today = time::OffsetDateTime::now_utc().date();
    if today > deadline {
        tracing::warn!(
            %malo_id,
            %period_to,
            %deadline,
            "billingd: invoice issued after the §40c EnWG deadline"
        );
        invoice.warnings.push(energy_billing::BillingWarning {
            code: "SECT40C_DEADLINE_EXCEEDED",
            severity: energy_billing::WarningSeverity::Warning,
            message: format!(
                "issued {today}, after the §40c EnWG deadline of {deadline} \
                 ({deadline_weeks} weeks past the period end {period_to})"
            ),
        });
    }
    Ok(invoice)
}

/// Resolve the Zählernummer serving a MaLo via the marktd device registry:
/// MaLo → Lokationszuordnung (B5 graph) → MeLo → Zähler.
///
/// Returns the first registered Zähler of the first linked MeLo — the common
/// single-meter case. Multi-meter locations carry their per-meter identity in
/// `MeterInput::zaehlernummer` from the caller instead.
pub(crate) async fn resolve_zaehlernummer(
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    malo_id: &str,
) -> Option<String> {
    let edges = marktd.get_lokationen(malo_id, "malo", None).await.ok()?;
    let melo_id = edges
        .iter()
        .find(|e| e.nach_typ == rubo4e::current::Lokationstyp::Melo)
        .map(|e| e.nach_id.clone())
        .or_else(|| {
            edges
                .iter()
                .find(|e| e.von_typ == rubo4e::current::Lokationstyp::Melo)
                .map(|e| e.von_id.clone())
        })?;
    marktd
        .list_zaehler_ids(&melo_id)
        .await
        .ok()?
        .into_iter()
        .next()
}

/// The same calendar window one year earlier, Feb 29 clamped to Feb 28.
pub(crate) fn year_earlier(d: time::Date) -> time::Date {
    d.replace_year(d.year() - 1)
        .unwrap_or_else(|_| d - time::Duration::days(365))
}

/// §40 Abs. 2 Nr. 7/8 EnWG — assemble the consumption comparison.
///
/// Prior-year consumption comes from edmd (soft dependency); the
/// comparable-customer-group annual value comes from operator config and is
/// pro-rated to the billing period. Returns `None` when neither source
/// yields a figure, so the engine omits the comparison positions instead of
/// rendering empty ones.
pub(crate) async fn resolve_verbrauchshistorie(
    cfg: &BillingdConfig,
    edmd: &Arc<EdmdClient>,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
) -> Option<energy_billing::Verbrauchshistorie> {
    let vorjahr_kwh = match edmd
        .get_billing_period(malo_id, year_earlier(period_from), year_earlier(period_to))
        .await
    {
        Ok(Some(m)) if m.arbeitsmenge_kwh > Decimal::ZERO => Some(m.arbeitsmenge_kwh),
        Ok(_) => None,
        Err(e) => {
            tracing::debug!(%malo_id, error = %e, "billingd: no prior-year consumption from edmd");
            None
        }
    };

    let bundesdurchschnitt_kwh = cfg.vergleichsgruppe_kwh_pro_jahr.map(|annual| {
        let days = Decimal::from((period_to - period_from).whole_days() + 1);
        let year_days = Decimal::from(time::util::days_in_year(period_from.year()));
        energy_billing::round_money(annual * days / year_days, 0)
    });

    if vorjahr_kwh.is_none() && bundesdurchschnitt_kwh.is_none() {
        return None;
    }
    Some(energy_billing::Verbrauchshistorie {
        vorjahr_kwh,
        bundesdurchschnitt_kwh,
        kundengruppe: cfg.vergleichsgruppe_label.clone(),
    })
}

/// §40c Abs. 1 EnWG — how many weeks after the period end the invoice is due.
///
/// Six weeks for an Abrechnung, six for a Schlussrechnung (measured from the end
/// of the Lieferverhältnis), and three where §40b Abs. 1 monthly billing applies.
/// The short deadline follows the agreed **cadence**, not the length of the
/// period being billed.
const fn sect40c_deadline_weeks(schlussrechnung: bool, monatliche_abrechnung: bool) -> i64 {
    if !schlussrechnung && monatliche_abrechnung {
        3
    } else {
        6
    }
}

/// Backward-compat shim: dispatch and return Invoice.
///
/// Called by existing HTTP handlers.
/// New callers should use `dispatch_invoice` directly.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn dispatch_calculator(
    cfg: &BillingdConfig,
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rates: &RegulatoryRates,
    edmd: &Arc<EdmdClient>,
    marktd: &Arc<mako_markt::marktd_client::MarktdClient>,
    tarifbd: &Arc<TarifbdClient>,
    vertragd: &Arc<VertragdClient>,
) -> Result<Invoice, (StatusCode, String)> {
    dispatch_invoice(
        cfg,
        tariff,
        req,
        malo_id,
        rechnungsnummer,
        period_from,
        period_to,
        rates,
        edmd,
        marktd,
        tarifbd,
        vertragd,
    )
    .await
}

#[cfg(test)]
mod sect40c_tests {
    use super::sect40c_deadline_weeks;

    /// §40c Abs. 1: six weeks is the rule.
    #[test]
    fn an_ordinary_abrechnung_has_six_weeks() {
        assert_eq!(sect40c_deadline_weeks(false, false), 6);
    }

    /// Three weeks only where §40b Abs. 1 monthly billing applies.
    #[test]
    fn monthly_billing_has_three_weeks() {
        assert_eq!(sect40c_deadline_weeks(false, true), 3);
    }

    /// A Schlussrechnung has six weeks from the end of the Lieferverhältnis,
    /// however short the final period is — the rule this replaced inferred
    /// "monthly" from the day count and warned about a ten-day move-out bill
    /// three weeks early.
    #[test]
    fn a_schlussrechnung_keeps_six_weeks_even_on_a_monthly_contract() {
        assert_eq!(sect40c_deadline_weeks(true, false), 6);
        assert_eq!(sect40c_deadline_weeks(true, true), 6);
    }
}
