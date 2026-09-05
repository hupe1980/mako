//! Category dispatch via `BillingEngine` — quantities and invoice assembly.

use super::*;

// ── Category dispatch via BillingEngine ──────────────────────────────────────

/// The caller-supplied meter input for a delivery mako reads from nowhere else.
///
/// Only for a category whose provider **prices a delivered quantity**: an
/// absent block defaults to zero, so the invoice charges the standing charges,
/// bills nothing for the commodity and reads as an ordinary document.
///
/// A category billed per month and per event — HEMS, Energiedienstleistung, and
/// an e-mobility product that prices no charging energy — has no such quantity.
/// Its provider derives the month count from the billed period and treats an
/// absent event count as no events, which is what a pure-subscription product
/// is: `Quantities::empty_energy_sources` leaves those categories out for the
/// same reason. Refusing them would reject a correct Grundgebühr invoice.
///
/// # Errors
///
/// `422 NO_METER_DATA` when it is absent.
fn require_meter<T>(supplied: Option<T>, category: &str, malo_id: &str) -> BillingResult<T> {
    supplied.ok_or_else(|| {
        BillingError::unprocessable(
            "NO_METER_DATA",
            format!(
                "no quantity supplied for {category} MaLo {malo_id}: mako holds no \
                 meter source for this category, so the reading has to arrive with \
                 the request. Billing it as zero would charge the Grundpreis and \
                 leave the whole period's consumption uninvoiced."
            ),
        )
    })
}

/// Build the `Quantities` for a billing request by resolving meter data.
pub(crate) async fn build_quantities(
    deps: &BillingDeps,
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
) -> BillingResult<Quantities> {
    let (edmd, marktd, productd) = (&deps.edmd, &deps.marktd, &deps.productd);
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
            // The meter reading is resolved either way. A dynamic tariff
            // prices from the Lastgang, but the §40 Abs. 2 Nr. 6 register
            // readings, the §40a estimation flag and the `metering_mode` all
            // live here — and without the metering mode
            // `DynamicElectricityProvider::validate_warnings` cannot apply the
            // §41a Abs. 1 iMSys guard at all.
            q.electricity =
                Some(resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?);
            if is_dynamic {
                q.dynamic_intervals =
                    fetch_dynamic_intervals(malo_id, period_from, period_to, edmd).await?;
                q.dynamic_epex_prices = fetch_epex_prices(period_from, period_to, productd).await?;
                // Without intervals the dynamic provider prices nothing: the
                // invoice comes back carrying the Grundpreis and no energy at
                // all — no Arbeitspreis, no Stromsteuer, no NNE-Arbeitspreis —
                // and looks entirely ordinary. That is the §41a twin of the
                // priceless-product defect, and it must be refused, not billed.
                if q.dynamic_intervals.is_empty() {
                    return Err(BillingError::unprocessable(
                        "SECT41A_NO_LASTGANG",
                        format!(
                            "§41a EnWG: no 15-minute Lastgang for MaLo {malo_id} in \
                             {period_from}..{period_to}. A dynamic tariff is billed per \
                             market time unit and cannot be billed without one — import \
                             the MSCONS interval data or switch the customer to a \
                             fixed-price product."
                        ),
                    ));
                }
            }
        }
        "GAS" => {
            let mut meter = req.gas_meter.clone().unwrap_or_default();
            enrich_gas_meter(&mut meter, malo_id, period_from, period_to, edmd, marktd).await?;
            q.gas = Some(meter);
        }
        // Neither `edmd` nor `marktd` holds a quantity for these: the request is
        // their only source, so an absent delivery is a refusal and never a
        // zero. A Fernwärme month billed as Grundpreis alone leaves the whole
        // month's heat uninvoiced, reads as an ordinary invoice, and scores in
        // the risk gate's SAMPLE band, so nothing holds it. The metered
        // categories above refuse a missing reading; so do these.
        "WAERME" => {
            q.heat = Some(require_meter(req.waerme_meter.clone(), "WAERME", malo_id)?);
        }
        "WASSER" => {
            q.wasser = Some(require_meter(req.wasser_meter.clone(), "WASSER", malo_id)?);
        }
        "SOLAR" => {
            q.solar = Some(require_meter(req.solar_meter.clone(), "SOLAR", malo_id)?);
        }
        "EEG" => {
            q.eeg = Some(require_meter(req.eeg_meter.clone(), "EEG", malo_id)?);
        }
        // `EinspeisungProvider` reads `Quantities::einspeisung`; the same
        // `eeg_meter` body carries it, because a Direktvermarktung settlement
        // states the same fed-in kWh.
        "EINSPEISUNG" => {
            q.einspeisung = Some(require_meter(
                req.eeg_meter.clone(),
                "EINSPEISUNG",
                malo_id,
            )?);
        }
        // Billed per month and per event: the month count comes from the billed
        // period and an absent event count is no events, so a pure-subscription
        // product bills a correct Grundgebühr invoice from no block at all.
        "HEMS" => {
            q.hems = req.hems_meter.clone();
        }
        "ENERGIEDIENSTLEISTUNG" => {
            q.service = req.service_meter.clone();
        }
        // Charging energy is the one e-mobility figure only the caller holds,
        // so it is required exactly when the product prices it. A service- and
        // session-priced product carries no kWh and needs no block.
        "EMOBILITY" => {
            let prices_energy = matches!(
                tariff,
                Product::Emobility(p) if p.emobility_kwh_price_ct.is_some()
            );
            q.emobility = if prices_energy {
                Some(require_meter(
                    req.emobility_meter.clone(),
                    "EMOBILITY",
                    malo_id,
                )?)
            } else {
                req.emobility_meter.clone()
            };
        }
        // §42c EnWG: a sharing participant is an ordinary supply customer whose
        // bill carries a credit for their allocated community share. Both halves
        // are needed — the residual supply from edmd and the allocation from the
        // caller — and the credit silently vanishing was the failure mode of
        // routing SHARING through the electricity fallback below.
        "SHARING" => {
            q.electricity =
                Some(resolve_strom_meter(req, malo_id, period_from, period_to, edmd).await?);
            q.energy_share = req.energy_share.clone();
            if q.energy_share.is_none() {
                tracing::warn!(
                    malo_id,
                    "billingd SHARING: no energy_share allocation supplied — the invoice bills \
                     the residual supply without the §42c community credit"
                );
            }
        }
        other => {
            // Every `Product` variant is named above. A category that is not is
            // a productd/billingd version skew, and billing it as electricity
            // would issue a plausible-looking invoice for the wrong product.
            return Err(BillingError::unprocessable(
                "UNKNOWN_CATEGORY",
                format!("no quantity source for product category {other}"),
            ));
        }
    }
    require_coverage(deps.cfg.as_ref(), &q, malo_id, period_from, period_to)?;
    Ok(q)
}

/// Refuse a period below the operator's coverage floor.
///
/// A sum over the readings that arrived says nothing about the ones that did
/// not: a month delivered up to the 3rd yields a plausible Arbeitsmenge and
/// bills as a complete month.
///
/// § 40a Abs. 2 EnWG nonetheless makes such a period billable — the invoice
/// „darf … auf einer Verbrauchsschätzung beruhen" where the supplier cannot
/// determine the actual consumption for reasons it does not answer for, stated
/// prominently on the document with its ground and its factors. So there is no
/// statutory floor to enforce, the engine's `MENGE_UNVOLLSTAENDIG` finding
/// carries the labelling duty, and this gate binds only where an operator set
/// `min_meter_coverage_pct` themselves. Unset, nothing is refused: one late
/// MSCONS day in a 31-day month reports 96.77 %, and refusing it spends the
/// § 40c Abs. 2 EnWG six weeks waiting for a reading.
///
/// A source that states no coverage is not gated — the figure is a claim about
/// completeness, and its absence is not the claim that the period is short.
fn require_coverage(
    cfg: &BillingdConfig,
    q: &Quantities,
    malo_id: &str,
    period_from: time::Date,
    period_to: time::Date,
) -> BillingResult<()> {
    let Some(minimum) = cfg.min_meter_coverage_pct() else {
        return Ok(());
    };
    for (label, pct) in [
        ("Strom", q.electricity.as_ref().and_then(|m| m.coverage_pct)),
        ("Gas", q.gas.as_ref().and_then(|m| m.coverage_pct)),
    ] {
        let Some(pct) = pct.filter(|p| *p < minimum) else {
            continue;
        };
        return Err(BillingError::unprocessable(
            "INCOMPLETE_METER_DATA",
            format!(
                "{label} MaLo {malo_id}: only {pct} % of {period_from}..{period_to} is \
                 covered by billable readings, below the operator's floor of \
                 {minimum} %. Either the missing readings arrive, or the gap is \
                 estimated under § 40a Abs. 2 EnWG and the estimate stated on the \
                 invoice."
            ),
        ));
    }
    Ok(())
}

/// Resolve everything a period needs and run it through the engine.
///
/// Quantities (meter data per category), the §40 contract facts from vertragd,
/// the Zählernummer from marktd and the §40 Abs. 2 Nr. 7/8 comparison figures
/// come together in one [`BillingContext`]; the engine turns that plus the
/// [`Product`] into an [`Invoice`]. The only clock in the pipeline is the §40c
/// deadline check at the end — the engine itself is clock-free by design.
#[allow(clippy::too_many_arguments)]
async fn dispatch_invoice(
    deps: &BillingDeps,
    tariff: &Product,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rates: &RegulatoryRates,
    run: RunId<'_>,
) -> BillingResult<Billed> {
    let cfg = deps.cfg.as_ref();
    let (edmd, marktd, vertragd) = (&deps.edmd, &deps.marktd, &deps.vertragd);
    let grid = req.grid.clone().unwrap_or_default();
    let quantities = build_quantities(deps, tariff, req, malo_id, period_from, period_to).await?;

    // §40 Abs. 1 EnWG — the contract facts the invoice must state live in
    // vertragd, not in the tariff. Soft dependency: an unreachable vertragd
    // or an uncontracted MaLo degrades to an invoice without them, logged.
    //
    // **One lookup, one snapshot.** Every caller also needs the BG-7 buyer from
    // this same answer. Fetching it separately would be two round trips per
    // invoice, and two answers that a concurrent master-data change could make
    // disagree about which customer the document is for.
    let vertrag = match vertragd.get_vertrag_by_malo(malo_id).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(%malo_id, error = %e, "billingd: vertragd lookup failed — invoice will lack §40 contract facts and its BG-7 buyer");
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
            // The next settlement covers the period after this one, in the same
            // cadence — so a January bill states 28 February, not 3 March.
            naechster_abrechnungstermin: next_abrechnungstermin(period_from, period_to),
        }
    });

    // The day this document is issued. The engine is clock-free by design, so
    // it cannot know it — and two statutory facts hang off it: § 14 Abs. 4
    // Nr. 3 UStG's Ausstellungsdatum, and the § 40c Abs. 1 EnWG Fälligkeit two
    // weeks after the request reaches the customer. Derived from the period end
    // instead, a catch-up run and every late Schlussrechnung issued invoices
    // that were already overdue on arrival, which the dunning downstream acted
    // on.
    let issue_date = mako_fristen::heute();

    let ctx = BillingContext {
        malo_id: malo_id.to_owned(),
        lf_mp_id: req.lf_mp_id.clone(),
        rechnungsnummer: rechnungsnummer.to_owned(),
        issue_date: Some(issue_date),
        period: BillingPeriod::new(period_from, period_to)
            .expect("parse_period guarantees from <= to"),
        // §40c EnWG: a Schlussrechnung (end of supply) settles the account;
        // the engine renders rechnungsart = SCHLUSSRECHNUNG and deducts the
        // paid Abschläge below from the Zahlbetrag.
        invoice_type: if req.schlussrechnung {
            InvoiceType::Final
        } else {
            InvoiceType::Initial
        },
        abschlage: req.abschlaege.clone(),
        // §14 Abs. 5 Satz 2 UStG — how the settling invoice presents the
        // advances. Both forms are lawful and the customer pays the same;
        // `Endrechnung` (the default) states the whole supply and deducts the
        // advances with their tax, `Restrechnung` invoices only the remainder,
        // which is what the BMF recommends for e-invoices (Schreiben v.
        // 15.10.2024, Rn. 48) because EN 16931's core profiles have nowhere to
        // carry per-advance tax. Per request, falling back to the operator's
        // configured default.
        settlement_form: req.settlement_form.unwrap_or(cfg.settlement_form()),
        // §13b Abs. 2 Nr. 5 lit. b UStG — supply to a Stromwiederverkäufer:
        // invoice net, recipient owes the VAT (EN 16931 `AE` tax breakdown).
        // Derived from the customer master (`kunden.stromwiederverkaeufer`) —
        // §13b is mandatory when its conditions are met, so master data ORs
        // with the request flag: the caller can assert it for a customer not
        // yet flagged, but cannot switch it off for one that is.
        reverse_charge: req.reverse_charge
            || vertrag
                .as_ref()
                .and_then(|v| v.rechnungsempfaenger.as_ref())
                .is_some_and(|r| r.stromwiederverkaeufer),
        regulatory_rates: rates.clone(),
        contract_id: vertrag.as_ref().map(|v| {
            v.vertrag
                .vertrags_nr
                .clone()
                .unwrap_or_else(|| v.vertrag.id.clone())
        }),
        // §36 / §38 / §41b EnWG — which supply regime this contract is under.
        //
        // Not cosmetic. `BillingContext::vertragsart` defaults to
        // `Sondervertrag`, and nothing here ever set it, so the engine's
        // § 38 Abs. 4 EnWG guard — an Ersatzversorgung ends three months after
        // supply began, and a period past that is refused — could not fire from
        // the service at all: every invoice, Ersatzversorgung included, was
        // billed as a freely negotiated contract, and the `mako:vertragsart`
        // ZusatzAttribut that discloses the GVV regime said SONDERVERTRAG.
        //
        // `vertragd` stores it (`versorgungsvertraege.vertragsart`, `NOT NULL`
        // under a CHECK) and serves it on this same by-malo answer, so it costs
        // no extra round trip. A MaLo with no contract in `vertragd` keeps the
        // engine default, which claims no regime.
        vertragsart: vertrag
            .as_ref()
            .map(|v| v.vertrag.regime())
            .unwrap_or_default(),
        // §41 EnWG pro-rata: clip the billable days to the contract term.
        //
        // Also the anchor for the § 38 Abs. 4 EnWG three-month limit above: the
        // engine measures from `vertragsbeginn` and falls back to the period's
        // own first day, which can only report the limit early, never miss it.
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
            lieferant_anschrift: cfg
                .seller
                .as_ref()
                .and_then(crate::config::SellerConfig::anschrift),
            lieferant_kontakt: cfg
                .seller
                .as_ref()
                .and_then(crate::config::SellerConfig::kontakt),
            ..Default::default()
        }),
        // Propagate minimum invoice from product definition (productd) to billing context.
        minimum_invoice_eur_brutto: tariff.minimum_invoice_eur_brutto(),
        // §42 EnWG — the product's Stromkennzeichnung, structured, so the
        // invoice can state the fuel mix and the mandatory CO₂ figure.
        energiequellen: tariff.energiequellen().cloned(),
        // §41 Abs. 1 Nr. 5 EnWG — Netzbetreiber identification on invoice.
        nb_mp_id: req.nb_mp_id.clone(),
        // The run this invoice belongs to, when it belongs to one — a §40b
        // sweep, a Sammelrechnung, a GGV batch. `None` for a single on-demand
        // calculation, which is a run of one and needs no group identity.
        billing_run_id: run.0.map(ToOwned::to_owned),
        ..Default::default()
    };

    let engine = tariff.build_engine(&grid, rates);

    let mut invoice = engine.bill(ctx, &quantities)?;

    // § 40c Abs. 2 EnWG — an Abrechnung must reach the customer within six weeks
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
    let today = issue_date;
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
    Ok(Billed {
        invoice,
        buyer: vertrag.and_then(|v| v.rechnungsempfaenger),
    })
}

/// One leg of a billing period: the product in force, and the days it covers.
///
/// A period with no price change is one leg. A Tarifwechsel splits it.
pub(crate) struct TariffLeg {
    pub(crate) tariff: Product,
    pub(crate) from: time::Date,
    /// Inclusive.
    pub(crate) to: time::Date,
    /// Override this leg's electricity meter reading.
    ///
    /// The scheduled sweep leaves it `None` and each leg is read from `edmd`
    /// for its own dates. A caller that already holds the split readings — the
    /// Tarifwechsel endpoint — supplies them here instead.
    pub(crate) meter: Option<MeterInput>,
}

/// The product legs covering a period, ready for [`dispatch_invoice_multi`].
///
/// One leg per product assignment in force inside the period, split further at
/// every statutory rate boundary. A request that states its own product is one
/// leg, since the caller has named the price for the whole period.
///
/// This is the only way to price a period correctly: an invoice covers a period,
/// and both a Tarifwechsel (§ 41 Abs. 5 EnWG, a contract fact `vertragd` owns)
/// and a VAT or levy Stichtag divide it into parts with different prices.
/// Resolving one product "as of" a single day charges that day's price for every
/// day of the period.
///
/// # Errors
///
/// `422 NO_ACTIVE_PRODUCT` when the MaLo has no assignment covering the period
/// or an assigned code has no version valid on its leg's first day; `502` when
/// `vertragd` or `productd` is unreachable.
pub(crate) async fn resolve_legs(
    req: &CalculateRequest,
    deps: &BillingDeps,
    malo_id: &str,
    from: time::Date,
    to: time::Date,
) -> BillingResult<Vec<TariffLeg>> {
    let cfg = deps.cfg.as_ref();
    if let Some(tariff) = req.tariff.clone() {
        return Ok(split_on_rate_boundaries(
            cfg,
            vec![TariffLeg {
                tariff,
                from,
                to,
                meter: None,
            }],
        ));
    }
    let slices = deps
        .vertragd
        .get_product_slices(malo_id, from, to)
        .await
        .map_err(|e| BillingError::upstream("vertragd", e))?;
    if slices.is_empty() {
        return Err(BillingError::unprocessable(
            "NO_ACTIVE_PRODUCT",
            format!("MaLo {malo_id} has no product assignment in {from}..{to} in vertragd"),
        ));
    }
    // One round trip prices every leg: asking productd per leg is an N+1 on
    // every invoice, and two calls could disagree if the catalogue changed
    // between them.
    let anfragen: Vec<(String, time::Date)> = slices
        .iter()
        .map(|s| (s.product_code.clone(), s.gueltig_von.max(from)))
        .collect();
    let produkte = deps
        .productd
        .resolve_products(&req.lf_mp_id, &anfragen)
        .await
        .map_err(|e| BillingError::upstream("productd", e))?;
    let mut legs = Vec::with_capacity(slices.len());
    for (slice, produkt) in slices.iter().zip(produkte) {
        let am = slice.gueltig_von.max(from);
        let tariff = produkt.ok_or_else(|| {
            BillingError::unprocessable(
                "NO_ACTIVE_PRODUCT",
                format!(
                    "product {} assigned to MaLo {malo_id} has no version valid on {am} \
                     in productd",
                    slice.product_code
                ),
            )
        })?;
        legs.push(TariffLeg {
            tariff,
            from: am,
            to: slice.last_day(to),
            meter: None,
        });
    }
    Ok(split_on_rate_boundaries(cfg, legs))
}

/// Split legs further wherever a statutory rate boundary falls inside one.
///
/// A period that crosses a VAT or levy Stichtag has no single correct rate: the
/// gas period crossing 31.03.2024 is 7 % before and 19 % after, and billing it
/// whole overcharges one part by twelve points while reading exactly like a
/// correct invoice downstream. The engine's answer to a price change and its
/// answer to a rate change are the same — bill the parts and merge them — so a
/// Tarifwechsel leg and a tax-boundary leg are the same kind of thing.
///
/// A leg whose meter reading was **supplied by the caller** is never split:
/// its consumption cannot be apportioned across the boundary without inventing
/// a reading. Those keep their whole span, and the rate resolution refuses them
/// with the Stichtage named, which is the honest outcome.
pub(crate) fn split_on_rate_boundaries(
    cfg: &BillingdConfig,
    legs: Vec<TariffLeg>,
) -> Vec<TariffLeg> {
    let mut out = Vec::with_capacity(legs.len());
    for leg in legs {
        if leg.meter.is_some() {
            out.push(leg);
            continue;
        }
        let stichtage = cfg.steuer_stichtage(leg.tariff.category_str(), leg.from, leg.to);
        if stichtage.is_empty() {
            out.push(leg);
            continue;
        }
        // Each Stichtag is the first day of the next regime.
        let mut start = leg.from;
        for tag in stichtage {
            if tag <= start || tag > leg.to {
                continue;
            }
            out.push(TariffLeg {
                tariff: leg.tariff.clone(),
                from: start,
                to: tag - time::Duration::days(1),
                meter: None,
            });
            start = tag;
        }
        out.push(TariffLeg {
            tariff: leg.tariff,
            from: start,
            to: leg.to,
            meter: None,
        });
    }
    out
}

/// How a multi-leg period is filed: what product it was billed under, and the
/// statutory rates the risk gate scores it against.
pub(crate) struct LegSummary {
    /// The product code, or every code the period touched when it was split.
    pub(crate) product_code: String,
    /// `TARIFWECHSEL` for a split period, else the product's own category.
    pub(crate) category: String,
}

impl LegSummary {
    /// Summarise the legs of a period for the billing record.
    #[must_use]
    pub(crate) fn of(legs: &[TariffLeg]) -> Self {
        let codes: Vec<&str> = legs
            .iter()
            .map(|l| l.tariff.product_code().unwrap_or(l.tariff.category_str()))
            .collect();
        match legs {
            // A period with one product is filed under it.
            [only] => Self {
                product_code: codes.first().map_or_else(String::new, |c| (*c).to_owned()),
                category: only.tariff.category_str().to_owned(),
            },
            // A split period names every product it touched, in order, so the
            // record says which prices the document actually contains.
            _ => Self {
                product_code: codes.join("+"),
                category: "TARIFWECHSEL".to_owned(),
            },
        }
    }
}

/// Bill a period that may contain a price change, as one document.
///
/// Each leg is billed under **its own** product, its own statutory rates and
/// its own meter reading, and the legs are merged into a single invoice —
/// § 41 Abs. 1 Nr. 4 EnWG wants the old and the new price itemised with the
/// periods they applied to, which is exactly what the merged positions carry.
///
/// A single-leg period takes the same path with one leg, so there is one
/// billing pipeline rather than two that can drift apart. Reading the meter per
/// leg is the part that cannot be skipped: billing both halves from the whole
/// period's consumption prices the wrong kWh at each price.
///
/// # Errors
///
/// Propagates whatever the underlying single-leg billing reports; a period with
/// no legs at all is a caller error, and so is a split period whose quantities
/// arrive as one period total (`TARIFWECHSEL_OHNE_TEILMENGEN`).
pub(crate) async fn dispatch_invoice_multi(
    deps: &BillingDeps,
    legs: &[TariffLeg],
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    run: RunId<'_>,
) -> BillingResult<Billed> {
    let Some((_, rest)) = legs.split_first() else {
        return Err(BillingError::bad_request(
            "NO_TARIFF_LEG",
            "a billing period needs at least one product assignment",
        ));
    };
    if !rest.is_empty() {
        refuse_unapportionable_quantities(legs, req, malo_id)?;
    }

    // The legs carry `/A`, `/B`, … for the trace; only the merged document is
    // issued, so only it consumes a number from the § 14 Abs. 4 Nr. 4 UStG
    // series.
    let leg_nr = |i: usize| {
        if rest.is_empty() {
            rechnungsnummer.to_owned()
        } else {
            format!(
                "{rechnungsnummer}/{}",
                (b'A' + u8::try_from(i).unwrap_or(0)) as char
            )
        }
    };

    // How the period's days divide among its legs: the ratio a quantity stated
    // once for the whole period is apportioned by.
    let leg_days: Vec<u32> = legs
        .iter()
        .map(|l| u32::try_from((l.to - l.from).whole_days() + 1).unwrap_or(1))
        .collect();

    let last = legs.len() - 1;
    let mut billed: Option<Billed> = None;
    for (i, leg) in legs.iter().enumerate() {
        let share = if rest.is_empty() {
            energy_billing::DayApportionment::whole()
        } else {
            energy_billing::DayApportionment::new(&leg_days, i)
        };
        let next = bill_leg(deps, leg, req, malo_id, &leg_nr(i), run, i == last, &share).await?;
        billed = Some(match billed {
            None => next,
            Some(acc) => Billed {
                invoice: acc.invoice.merge(next.invoice),
                // The buyer is the same customer throughout; keep the first
                // answer that resolved one.
                buyer: acc.buyer.or(next.buyer),
            },
        });
    }
    let billed = billed.expect("legs is non-empty");
    if !rest.is_empty() {
        billed.invoice.assert_valid();
        tracing::info!(
            %malo_id, legs = legs.len(), %rechnungsnummer,
            "billingd: period billed across a Tarifwechsel"
        );
    }
    Ok(billed)
}

/// Refuse a split period whose electricity or gas quantity arrives as one
/// figure for the whole of it.
///
/// Each leg is priced at its own tariff and its own statutory rates, so it needs
/// its own quantity. For electricity and gas the caller has a way to give it
/// one, and so is asked for it rather than having a period total divided:
///
/// - **`meter`** — [`TariffLeg::meter`] carries a reading per leg, which is the
///   shape the Tarifwechsel endpoint already sends. Omitted entirely, each leg
///   is read from `edmd` for its own dates.
/// - **`gas_meter`** carrying a volume or a kWh figure — omitted, `edmd`
///   answers per leg with the Brennwert and Zustandszahl that applied to those
///   days, which no apportionment of a period total can reconstruct.
///
/// Every other category is apportioned by days instead — see
/// [`energy_billing::DayApportionment`] — because mako reads it from nowhere
/// else and there is no per-leg field to fill in.
fn refuse_unapportionable_quantities(
    legs: &[TariffLeg],
    req: &CalculateRequest,
    malo_id: &str,
) -> BillingResult<()> {
    // One leg is one price for the whole period, so the period total *is* the
    // leg's quantity and there is nothing to apportion. Refusing here would
    // reject every ordinary single-tariff invoice.
    if legs.len() < 2 {
        return Ok(());
    }
    let mut carried: Vec<&str> = Vec::new();
    if req.meter.is_some() && legs.iter().any(|l| l.meter.is_none()) {
        carried.push("meter");
    }
    // A gas block with no quantity in it states only conversion factors; the
    // reading itself still comes from edmd, per leg.
    if req
        .gas_meter
        .as_ref()
        .is_some_and(|m| m.messung_qm3 != Decimal::ZERO || m.kwh_hs.is_some())
    {
        carried.push("gas_meter");
    }
    if carried.is_empty() {
        return Ok(());
    }
    let spans: Vec<String> = legs
        .iter()
        .map(|l| format!("{}..{}", l.from, l.to))
        .collect();
    Err(BillingError::unprocessable(
        "TARIFWECHSEL_OHNE_TEILMENGEN",
        format!(
            "MaLo {malo_id}: the period splits into {} legs ({}) and the request states \
             {} for the period as a whole. Each leg is priced at its own tariff and \
             statutory rates, so it needs its own reading — supply the readings per leg, \
             omit them so edmd answers per leg, or bill the legs as separate periods.",
            legs.len(),
            spans.join(", "),
            carried.join(", "),
        ),
    ))
}

/// Bill one leg under its own statutory rates and its own share of the period.
///
/// `is_last` marks the leg that carries the facts belonging to the **period**
/// rather than to any one part of it: the advances § 40 Abs. 1 EnWG has the
/// settling invoice deduct, and the EEG credit. Each is a single figure for the
/// whole document, so it rides on exactly one leg — carried on every leg, a
/// year of twelve advances split at a levy Stichtag deducts them twice over and
/// refunds a customer who owes money.
///
/// `share` is this leg's part of the quantities the caller stated once for the
/// whole period. mako has no other source for them and [`TariffLeg`] has no
/// field to carry them per leg, so they are apportioned by days rather than
/// refused or invented — see [`energy_billing::DayApportionment`], which also
/// says which figures are sums over days and which are carried whole. On a
/// one-leg period the share is the whole and nothing is touched.
#[allow(clippy::too_many_arguments)]
async fn bill_leg(
    deps: &BillingDeps,
    leg: &TariffLeg,
    req: &CalculateRequest,
    malo_id: &str,
    rechnungsnummer: &str,
    run: RunId<'_>,
    is_last: bool,
    share: &energy_billing::DayApportionment,
) -> BillingResult<Billed> {
    // A leg inside a VAT or levy window carries that window's rate — which is
    // the other reason a period is split, and why the rates are resolved per
    // leg rather than once for the whole period.
    let mut rates = deps
        .cfg
        .try_regulatory_rates_for_period(leg.tariff.category_str(), leg.from, leg.to)
        .map_err(|e| BillingError::unprocessable("REGULATORY_RATES", e.to_string()))?;
    // The nEHS certificate price is a series, read at the leg's own start: a
    // period split at a BEHG Stichtag is split precisely because the CO₂ price
    // changed inside it.
    apply_nehs_market_price(
        &mut rates,
        leg.tariff.category_str(),
        leg.from,
        deps.cfg.as_ref(),
        &deps.productd,
    )
    .await;
    // The leg's own dates, its own reading where the caller supplied one, and
    // its share of every period total.
    let leg_req = CalculateRequest {
        period_from: leg.from.to_string(),
        period_to: leg.to.to_string(),
        meter: leg.meter.clone().or_else(|| req.meter.clone()),
        waerme_meter: req.waerme_meter.as_ref().map(|m| m.apportioned(share)),
        wasser_meter: req.wasser_meter.as_ref().map(|m| m.apportioned(share)),
        solar_meter: req.solar_meter.as_ref().map(|m| m.apportioned(share)),
        eeg_meter: req.eeg_meter.as_ref().map(|m| m.apportioned(share)),
        hems_meter: req.hems_meter.as_ref().map(|m| m.apportioned(share)),
        emobility_meter: req.emobility_meter.as_ref().map(|m| m.apportioned(share)),
        service_meter: req.service_meter.as_ref().map(|m| m.apportioned(share)),
        energy_share: req.energy_share.as_ref().map(|m| m.apportioned(share)),
        abschlaege: if is_last {
            req.abschlaege.clone()
        } else {
            Vec::new()
        },
        // A EUR credit for the whole period, not a per-leg price.
        eeg_gutschrift_eur: is_last.then_some(req.eeg_gutschrift_eur).flatten(),
        ..req.clone()
    };
    dispatch_invoice(
        deps,
        &leg.tariff,
        &leg_req,
        malo_id,
        rechnungsnummer,
        leg.from,
        leg.to,
        &rates,
        run,
    )
    .await
}

/// A priced period and the customer master the document is addressed to.
///
/// The two travel together because they come from the same `vertragd` answer:
/// the § 40 Abs. 1 contract facts on the invoice and the EN 16931 BG-7 buyer are
/// two views of one contract, and resolving them separately meant two round
/// trips and two answers that could disagree.
pub(crate) struct Billed {
    pub(crate) invoice: Invoice,
    /// `None` when vertragd is unreachable or the MaLo is uncontracted — the
    /// document is then built with a buyer synthesised from the MaLo-ID and
    /// carries the resulting BR-DE-8/9 findings, which is the documented
    /// degradation rather than a failed billing run.
    pub(crate) buyer: Option<crate::clients::Rechnungsempfaenger>,
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
        .find(|e| e.nach_typ == mako_markt::domain::Lokationstyp::Melo)
        .map(|e| e.nach_id.clone())
        .or_else(|| {
            edges
                .iter()
                .find(|e| e.von_typ == mako_markt::domain::Lokationstyp::Melo)
                .map(|e| e.von_id.clone())
        })?;
    marktd
        .list_zaehler_ids(&melo_id)
        .await
        .ok()?
        .into_iter()
        .next()
}

/// The same calendar date one year earlier, clamped to the end of the month.
///
/// 29 February has no counterpart in a common year. Subtracting 365 days landed
/// on **1 March** — one day *past* the missing date, so the comparison window
/// (§40 Abs. 2 Nr. 7 EnWG) started after it should. Clamping to 28 February
/// keeps the window on the right side of the boundary.
pub(crate) fn year_earlier(d: time::Date) -> time::Date {
    let year = d.year() - 1;
    d.replace_year(year).unwrap_or_else(|_| {
        time::Date::from_calendar_date(year, time::Month::February, 28)
            .expect("28 February exists in every year")
    })
}

/// §40 Abs. 1 EnWG — when the next settlement falls due.
///
/// A period that spans whole calendar months advances by that many **months**,
/// so the January bill of a monthly contract announces 28 February and the
/// Q1 bill announces 30 June. Anything else (a move-in fragment, a Teilzeitraum)
/// advances by its own day count, which is the only meaning it has.
///
/// Adding the day count unconditionally — what this replaced — turned every
/// monthly invoice into an announcement three days into the month after next.
pub(crate) fn next_abrechnungstermin(from: time::Date, to: time::Date) -> Option<time::Date> {
    let next_start = to.next_day()?;
    if let Some(months) = whole_months(from, to) {
        // The next period is `months` long and starts the day after this one
        // ends; its last day is the day before the month after that.
        return next_start
            .checked_add(time::Duration::days(0))
            .and_then(|s| add_months(s, months))
            .and_then(time::Date::previous_day);
    }
    let days = (to - from).whole_days() + 1;
    next_start.checked_add(time::Duration::days(days - 1))
}

/// How many whole calendar months the period covers, if it covers whole ones.
fn whole_months(from: time::Date, to: time::Date) -> Option<i32> {
    if from.day() != 1 || to.next_day()?.day() != 1 {
        return None;
    }
    let months = (to.year() - from.year()) * 12 + i32::from(u8::from(to.month()))
        - i32::from(u8::from(from.month()))
        + 1;
    (months > 0).then_some(months)
}

/// Advance a first-of-month date by `months`.
fn add_months(d: time::Date, months: i32) -> Option<time::Date> {
    let zero_based = i32::from(u8::from(d.month())) - 1 + months;
    let year = d.year() + zero_based.div_euclid(12);
    let month = time::Month::try_from(u8::try_from(zero_based.rem_euclid(12) + 1).ok()?).ok()?;
    time::Date::from_calendar_date(year, month, 1).ok()
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

/// § 40c Abs. 2 EnWG — how many weeks after the period end the invoice is owed.
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

#[cfg(test)]
mod termin_tests {
    use super::{next_abrechnungstermin, year_earlier};
    use time::macros::date;

    /// A monthly period announces the end of the following month — the previous
    /// day-count arithmetic announced 3 March for a January bill.
    #[test]
    fn a_calendar_month_advances_by_a_month() {
        assert_eq!(
            next_abrechnungstermin(date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
            Some(date!(2026 - 02 - 28))
        );
        assert_eq!(
            next_abrechnungstermin(date!(2026 - 02 - 01), date!(2026 - 02 - 28)),
            Some(date!(2026 - 03 - 31))
        );
    }

    /// A quarter advances by a quarter, and a year by a year.
    #[test]
    fn multi_month_periods_keep_their_cadence() {
        assert_eq!(
            next_abrechnungstermin(date!(2026 - 01 - 01), date!(2026 - 03 - 31)),
            Some(date!(2026 - 06 - 30))
        );
        assert_eq!(
            next_abrechnungstermin(date!(2025 - 01 - 01), date!(2025 - 12 - 31)),
            Some(date!(2026 - 12 - 31))
        );
    }

    /// A fragment has no cadence to keep, so it advances by its own length.
    #[test]
    fn a_partial_period_advances_by_its_day_count() {
        // 10 days (12–21 June) → the next 10 days end on 1 July.
        assert_eq!(
            next_abrechnungstermin(date!(2026 - 06 - 12), date!(2026 - 06 - 21)),
            Some(date!(2026 - 07 - 01))
        );
    }

    /// 29 February clamps back to 28 February, not forward to 1 March.
    #[test]
    fn the_leap_day_clamps_backwards() {
        assert_eq!(year_earlier(date!(2024 - 02 - 29)), date!(2023 - 02 - 28));
        assert_eq!(year_earlier(date!(2026 - 07 - 19)), date!(2025 - 07 - 19));
    }
}

#[cfg(test)]
mod sect40c_tests {
    use super::sect40c_deadline_weeks;

    /// § 40c Abs. 2: six weeks is the rule.
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

#[cfg(test)]
mod leg_summary_tests {
    use super::{LegSummary, TariffLeg};
    use energy_billing::Product;
    use time::macros::date;

    /// A config with no pinned VAT rate, so the statutory windows apply.
    fn cfg() -> crate::config::BillingdConfig {
        serde_json::from_value(serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "productd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080"
        }))
        .expect("config parses")
    }

    fn gas_leg(from: time::Date, to: time::Date) -> TariffLeg {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "GAS",
            "product_code": "GAS-BASIS",
            "arbeitspreis_ct_per_kwh": "9",
        }))
        .expect("a minimal GAS product");
        TariffLeg {
            tariff,
            from,
            to,
            meter: None,
        }
    }

    fn leg(code: &str) -> TariffLeg {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "product_code": code,
            "arbeitspreis_ct_per_kwh": "30",
        }))
        .expect("a minimal STROM product");
        TariffLeg {
            tariff,
            from: date!(2026 - 03 - 01),
            to: date!(2026 - 03 - 31),
            meter: None,
        }
    }

    #[test]
    fn a_single_leg_is_filed_under_its_own_product_and_category() {
        let s = LegSummary::of(&[leg("STROM-BASIS")]);
        assert_eq!(s.product_code, "STROM-BASIS");
        assert_eq!(s.category, "STROM");
    }

    #[test]
    fn a_leg_whose_reading_the_caller_supplied_is_never_split() {
        // Its consumption cannot be apportioned across the boundary without
        // inventing a reading; the rate resolution then refuses it by name.
        let mut l = gas_leg(date!(2024 - 03 - 01), date!(2024 - 04 - 30));
        l.meter = Some(energy_billing::MeterInput::default());
        let out = super::split_on_rate_boundaries(&cfg(), vec![l]);
        assert_eq!(out.len(), 1, "nothing can apportion a supplied reading");
    }

    /// Gas carried 7 % USt until 31.03.2024 and 19 % after (§ 28 Abs. 5/6
    /// UStG). A March–April period has no correct single rate; billing it whole
    /// charged the March portion — legally 7 % — at 19 %, and the result reads
    /// exactly like a correct invoice downstream.
    #[test]
    fn a_gas_period_crossing_the_vat_window_is_split_at_the_stichtag() {
        let out = super::split_on_rate_boundaries(
            &cfg(),
            vec![gas_leg(date!(2024 - 03 - 01), date!(2024 - 04 - 30))],
        );
        assert_eq!(out.len(), 2, "one leg per rate regime");
        assert_eq!(out[0].from, date!(2024 - 03 - 01));
        assert_eq!(out[0].to, date!(2024 - 03 - 31));
        assert_eq!(out[1].from, date!(2024 - 04 - 01));
        assert_eq!(out[1].to, date!(2024 - 04 - 30));
        assert_eq!(
            out[0].to.next_day().unwrap(),
            out[1].from,
            "the legs tile the period with no gap and no shared day"
        );
    }

    /// An annual Fernwärme invoice covers a year end, and the § 10 BEHG step
    /// there is the gas levy: a heat product's CO₂ cost is its own CO2KostAufG
    /// § 3 figure and is the same on both sides. Splitting there would yield two
    /// legs priced identically, so the period stays whole.
    #[test]
    fn a_year_crossing_waerme_period_stays_one_leg() {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "WAERME",
            "product_code": "WAERME-BASIS",
            "waerme_arbeitspreis_ct_per_kwh": "11",
        }))
        .expect("a minimal WAERME product");
        let out = super::split_on_rate_boundaries(
            &cfg(),
            vec![TariffLeg {
                tariff,
                from: date!(2025 - 07 - 01),
                to: date!(2026 - 06 - 30),
                meter: None,
            }],
        );
        assert_eq!(out.len(), 1, "one CO₂ rate governs the whole heat year");
    }

    #[test]
    fn a_period_inside_one_rate_regime_stays_one_leg() {
        let out = super::split_on_rate_boundaries(&cfg(), vec![leg("STROM-BASIS")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].from, date!(2026 - 03 - 01));
        assert_eq!(out[0].to, date!(2026 - 03 - 31));
    }

    #[test]
    fn a_split_period_names_every_product_it_touched() {
        // The record has to say which prices the document actually contains;
        // filing it under one of the two hid the other from every later query.
        let s = LegSummary::of(&[leg("STROM-ALT"), leg("STROM-NEU")]);
        assert_eq!(s.product_code, "STROM-ALT+STROM-NEU");
        assert_eq!(s.category, "TARIFWECHSEL");
    }
}

#[cfg(test)]
mod quantity_source_tests {
    use super::{TariffLeg, refuse_unapportionable_quantities, require_coverage, require_meter};
    use crate::handlers::CalculateRequest;
    use energy_billing::{MeterInput, Product, Quantities, WaermeMeterInput};
    use rust_decimal::dec;
    use time::macros::date;

    fn cfg() -> crate::config::BillingdConfig {
        serde_json::from_value(serde_json::json!({
            "database": { "url": "postgres://localhost/x" },
            "tenant": "9900357000004",
            "productd_url": "http://localhost:9080",
            "edmd_url": "http://localhost:8380",
            "marktd_url": "http://localhost:8080"
        }))
        .expect("config parses")
    }

    /// One leg for January, a second for February — the shape of a period split
    /// by a price change or a levy Stichtag.
    fn strom_legs(n: usize) -> Vec<TariffLeg> {
        let tariff: Product = serde_json::from_value(serde_json::json!({
            "category": "STROM",
            "product_code": "STROM-BASIS",
            "arbeitspreis_ct_per_kwh": "30",
        }))
        .expect("a minimal STROM product");
        let spans = [
            (date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
            (date!(2026 - 02 - 01), date!(2026 - 02 - 28)),
        ];
        spans
            .into_iter()
            .take(n)
            .map(|(from, to)| TariffLeg {
                tariff: tariff.clone(),
                from,
                to,
                meter: None,
            })
            .collect()
    }

    /// A category mako reads from nowhere else has to be told the quantity: an
    /// all-zero default bills the Grundpreis and calls the period settled.
    #[test]
    fn a_category_with_no_meter_source_refuses_a_missing_reading() {
        let err = require_meter::<WaermeMeterInput>(None, "WAERME", "51238696781")
            .expect_err("no source, no invoice");
        assert_eq!(err.code(), "NO_METER_DATA");
        assert!(
            require_meter(Some(WaermeMeterInput::default()), "WAERME", "51238696781").is_ok(),
            "a supplied reading is billed, zero or not"
        );
    }

    /// Each leg of a split period is priced at its own tariff, so it needs its
    /// own quantity: one period total cannot be divided into them.
    #[test]
    fn a_split_period_refuses_a_period_total_quantity() {
        let req = CalculateRequest {
            meter: Some(MeterInput::default()),
            ..Default::default()
        };
        let err = refuse_unapportionable_quantities(&strom_legs(2), &req, "51238696781")
            .expect_err("nothing can apportion a period total across two prices");
        assert_eq!(err.code(), "TARIFWECHSEL_OHNE_TEILMENGEN");
    }

    /// A period with one price has one tariff to price its total at.
    #[test]
    fn an_unsplit_period_takes_a_period_total_quantity() {
        let req = CalculateRequest {
            meter: Some(MeterInput::default()),
            ..Default::default()
        };
        assert!(refuse_unapportionable_quantities(&strom_legs(1), &req, "51238696781").is_ok());
    }

    /// A caller that supplied a reading **per leg** has apportioned it already.
    #[test]
    fn legs_carrying_their_own_readings_are_billed() {
        let req = CalculateRequest {
            meter: Some(MeterInput::default()),
            ..Default::default()
        };
        let mut legs = strom_legs(2);
        for leg in &mut legs {
            leg.meter = Some(MeterInput::default());
        }
        assert!(refuse_unapportionable_quantities(&legs, &req, "51238696781").is_ok());
    }

    /// § 40a Abs. 2 EnWG lets a period the supplier cannot fully measure be
    /// billed on a labelled estimate, so mako gates nothing until an operator
    /// sets a floor. One late MSCONS day in a 31-day month reports 96.77 %, and
    /// refusing it spends the § 40c Abs. 2 six weeks waiting for a reading.
    #[test]
    fn an_unconfigured_coverage_floor_refuses_nothing() {
        for pct in [dec!(96.77), dec!(9.68), dec!(0)] {
            let q = Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(500),
                    coverage_pct: Some(pct),
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(
                require_coverage(
                    &cfg(),
                    &q,
                    "51238696781",
                    date!(2026 - 01 - 01),
                    date!(2026 - 01 - 31),
                )
                .is_ok(),
                "coverage {pct} % is billable as a § 40a Abs. 2 estimate"
            );
        }
    }

    /// An operator who sets a floor gets one: below it the period is refused,
    /// at it and above it billed.
    #[test]
    fn a_configured_coverage_floor_binds_below_itself() {
        let mut cfg = cfg();
        cfg.min_meter_coverage_pct = Some(dec!(95));
        let with = |pct| Quantities {
            electricity: Some(MeterInput {
                arbeitsmenge_kwh: dec!(500),
                coverage_pct: Some(pct),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = require_coverage(
            &cfg,
            &with(dec!(9.68)),
            "51238696781",
            date!(2026 - 01 - 01),
            date!(2026 - 01 - 31),
        )
        .expect_err("three days of readings are under the operator's floor");
        assert_eq!(err.code(), "INCOMPLETE_METER_DATA");
        assert!(
            require_coverage(
                &cfg,
                &with(dec!(95)),
                "51238696781",
                date!(2026 - 01 - 01),
                date!(2026 - 01 - 31),
            )
            .is_ok(),
            "the floor itself is covered enough"
        );
    }

    /// A Fernwärme period is billed from the caller's figures alone — mako reads
    /// heat from nowhere else and a leg carries no heat reading of its own — so
    /// a split period apportions the period total rather than refusing it.
    /// Refusing it would leave the caller nothing to do: the heat block is
    /// required, and its presence would be the ground for the refusal.
    #[test]
    fn a_split_period_bills_a_caller_only_quantity_by_apportioning_it() {
        let req = CalculateRequest {
            waerme_meter: Some(WaermeMeterInput {
                kwh_waerme: dec!(12000),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(
            refuse_unapportionable_quantities(&strom_legs(2), &req, "51238696781").is_ok(),
            "a heat period total is apportioned, not refused"
        );
    }

    /// The legs of a split period sum back to the quantity the caller stated,
    /// each in proportion to its days — 31 of 59 and 28 of 59 here.
    #[test]
    fn apportioned_legs_sum_back_to_the_period_total() {
        let legs = strom_legs(2);
        let days: Vec<u32> = legs
            .iter()
            .map(|l| u32::try_from((l.to - l.from).whole_days() + 1).expect("a short leg"))
            .collect();
        assert_eq!(days, vec![31, 28]);
        let total = dec!(12000);
        let parts: Vec<_> = (0..legs.len())
            .map(|i| {
                WaermeMeterInput {
                    kwh_waerme: total,
                    ..Default::default()
                }
                .apportioned(&energy_billing::DayApportionment::new(&days, i))
                .kwh_waerme
            })
            .collect();
        assert_eq!(
            parts.iter().copied().sum::<rust_decimal::Decimal>(),
            total,
            "no kWh is created or lost by the split: {parts:?}"
        );
        assert!(
            parts[0] > parts[1],
            "the longer leg carries more: {parts:?}"
        );
    }

    /// A gas block that carries only conversion factors states no quantity, so
    /// edmd still answers per leg and there is nothing to refuse.
    #[test]
    fn a_gas_block_without_a_reading_does_not_block_a_split_period() {
        let req = CalculateRequest {
            gas_meter: Some(energy_billing::GasMeterInput {
                brennwert_kwh_per_qm3: Some(dec!(11.5)),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(refuse_unapportionable_quantities(&strom_legs(2), &req, "51238696781").is_ok());
        let with_reading = CalculateRequest {
            gas_meter: Some(energy_billing::GasMeterInput {
                messung_qm3: dec!(900),
                ..Default::default()
            }),
            ..Default::default()
        };
        let err = refuse_unapportionable_quantities(&strom_legs(2), &with_reading, "51238696781")
            .expect_err("edmd holds the Brennwert of each leg's own days");
        assert_eq!(err.code(), "TARIFWECHSEL_OHNE_TEILMENGEN");
    }

    /// A fully delivered period bills, and a source that states no coverage
    /// makes no claim about completeness either way.
    #[test]
    fn full_coverage_and_an_unstated_coverage_both_bill() {
        for pct in [Some(dec!(100)), None] {
            let q = Quantities {
                electricity: Some(MeterInput {
                    arbeitsmenge_kwh: dec!(500),
                    coverage_pct: pct,
                    ..Default::default()
                }),
                ..Default::default()
            };
            assert!(
                require_coverage(
                    &cfg(),
                    &q,
                    "51238696781",
                    date!(2026 - 01 - 01),
                    date!(2026 - 01 - 31),
                )
                .is_ok(),
                "coverage {pct:?} must bill"
            );
        }
    }
}

/// The § 38 Abs. 4 EnWG three-month limit, from `vertragd`'s answer to the wire.
///
/// `BillingContext::vertragsart` was never set anywhere in `billingd`, so it was
/// `Sondervertrag` on every invoice the service produced. The engine's Pass 0
/// refuses an Ersatzversorgung period past three months
/// (`ERSATZVERSORGUNG_UEBER_3_MONATE`, `WarningSeverity::Error` →
/// `EngineError::ValidationBlocked`), but that branch is gated on
/// `vertragsart == Ersatzversorgung` — so the whole guard was dead in
/// production: a substitute supply running past its statutory end billed
/// cleanly, as a freely negotiated contract, and the `mako:vertragsart`
/// ZusatzAttribut disclosed the wrong regime.
///
/// These build the context the way [`bill_one`] builds it — regime and
/// Vertragsbeginn out of the same `vertragd` answer — and run it through the
/// engine and this service's own error mapping.
#[cfg(test)]
mod ersatzversorgung_limit_tests {
    use crate::clients::VertragByMalo;
    use energy_billing::{
        BillingContext, BillingEngine, BillingPeriod, InvoiceType, Quantities, RegulatoryRates,
    };
    use rust_decimal::dec;
    use time::macros::date;

    fn vertrag(vertragsart: &str, vertragsbeginn: &str) -> VertragByMalo {
        serde_json::from_value(serde_json::json!({
            "vertrag": {
                "id": "3f2504e0-4f89-41d3-9a0c-0305e82c3301",
                "vertrags_nr": "VV-2026-00000001",
                "vertragsart": vertragsart,
                "vertragsbeginn": vertragsbeginn,
                "vertragsende": null,
                "kuendigungsfrist_monate": 1,
            },
            "naechstmoeglicher_kuendigungstermin": null,
        }))
        .expect("by-malo answer")
    }

    /// The two lines `bill_one` sets from the contract answer, in isolation.
    fn context(v: &VertragByMalo, from: time::Date, to: time::Date) -> BillingContext {
        BillingContext {
            malo_id: "51238696781".to_owned(),
            lf_mp_id: "9900111000002".to_owned(),
            rechnungsnummer: "RE-2026-0001".to_owned(),
            issue_date: Some(to),
            period: BillingPeriod::new(from, to).expect("period"),
            invoice_type: InvoiceType::Initial,
            regulatory_rates: RegulatoryRates::default(),
            vertragsart: v.vertrag.regime(),
            vertragsbeginn: Some(v.vertrag.vertragsbeginn),
            vertragsende: v.vertrag.vertragsende,
            ..Default::default()
        }
    }

    fn quantities() -> Quantities {
        Quantities {
            electricity: Some(energy_billing::MeterInput {
                arbeitsmenge_kwh: dec!(500),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Supply that began on 1 January cannot lawfully still be Ersatzversorgung
    /// in May, so the invoice for May is refused — as a 422 the caller can act
    /// on, naming the code.
    #[test]
    fn an_ersatzversorgung_past_three_months_is_refused_through_the_service() {
        let v = vertrag("ERSATZVERSORGUNG", "2026-01-01");
        let ctx = context(&v, date!(2026 - 05 - 01), date!(2026 - 05 - 31));
        let err = BillingEngine::new()
            .bill(ctx, &quantities())
            .expect_err("§ 38 Abs. 4 EnWG bars a fourth month of Ersatzversorgung");

        let mapped: crate::error::BillingError = err.into();
        assert_eq!(
            mapped.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        let body = mapped.body();
        assert_eq!(body["error"]["code"], "VALIDATION_BLOCKED");
        assert!(
            body["error"]["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .any(|w| w["code"] == "ERSATZVERSORGUNG_UEBER_3_MONATE"),
            "the refusal must name the statutory limit: {body}"
        );
    }

    /// The first three months bill normally — the guard is a limit, not a ban.
    #[test]
    fn an_ersatzversorgung_inside_three_months_bills() {
        let v = vertrag("ERSATZVERSORGUNG", "2026-01-01");
        let ctx = context(&v, date!(2026 - 02 - 01), date!(2026 - 02 - 28));
        BillingEngine::new()
            .bill(ctx, &quantities())
            .expect("February is inside the three months");
    }

    /// …and the regime has to come off the wire for any of it to happen.
    ///
    /// This is the state `billingd` shipped in: nothing set `vertragsart`, so
    /// the same period billed cleanly as a Sondervertrag. Fails if the field is
    /// dropped again.
    #[test]
    fn without_the_regime_the_statutory_limit_cannot_fire() {
        let v = vertrag("ERSATZVERSORGUNG", "2026-01-01");
        let mut ctx = context(&v, date!(2026 - 05 - 01), date!(2026 - 05 - 31));
        ctx.vertragsart = energy_billing::Vertragsart::Sondervertrag;
        assert!(
            BillingEngine::new().bill(ctx, &quantities()).is_ok(),
            "a Sondervertrag has no three-month limit — which is exactly why the \
             regime must be read from vertragd rather than defaulted"
        );
    }

    /// `bill_one` must actually set it. The engine guard and the wire field are
    /// both in place; the seam between them is one line, and it is the line
    /// that was missing.
    #[test]
    fn bill_one_sets_the_regime_from_the_contract() {
        let src = include_str!("dispatch.rs");
        let start = src
            .find("let ctx = BillingContext {")
            .expect("bill_one's context");
        let body = &src[start..start + 4000];
        assert!(
            body.contains("vertragsart:") && body.contains("regime()"),
            "bill_one must set BillingContext::vertragsart from the vertragd answer"
        );
    }
}
