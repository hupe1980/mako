//! Concrete `BillingProvider` implementations for all product types.
//!
//! Each provider corresponds to one product category. Build providers from
//! a `TariffInput` (the product definition from `tarifbd`) and register them
//! with `BillingEngine`.

use billing::BillingError;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

use crate::context::BillingContext;
use crate::position::{
    BillingPosition, PositionCategory, arbeitspreis_position, grundpreis_position, levy_position,
    validated_eur,
};
use crate::provider::BillingProvider;
use crate::quantities::{GridInput, Quantities};
use crate::tariff::TariffInput;

// ── ElectricityProvider ───────────────────────────────────────────────────────

/// STROM / WAERMEPUMPE / WALLBOX billing provider.
///
/// Produces commodity positions (Grundpreis, Arbeitspreis HT/NT, §14a credits).
/// Does NOT include MwSt — add `MwStProvider` to the engine.
/// Stromsteuer is included as a levy position.
pub struct ElectricityProvider {
    tariff: TariffInput,
    grid: GridInput,
}

impl ElectricityProvider {
    /// Build from a `TariffInput` product definition.
    #[must_use]
    pub fn new(tariff: TariffInput, grid: GridInput) -> Self {
        Self { tariff, grid }
    }

    pub fn from_tariff(tariff: &TariffInput, grid: &GridInput) -> Self {
        Self {
            tariff: tariff.clone(),
            grid: grid.clone(),
        }
    }
}

impl BillingProvider for ElectricityProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.electricity.as_ref().cloned().unwrap_or_default();
        let kwh = meter.arbeitsmenge_kwh;
        let days = ctx.days();
        let tariff = &self.tariff;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;
        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── Grundpreis ─────────────────────────────────────────────────────────
        if let Some(gp_ct_day) = tariff.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis",
                    gp_ct_day / dec!(100),
                    days,
                    "§41 EnWG",
                    &["strom"],
                )
                .with_tag("strom"),
            );
        }

        // ── Arbeitspreis ───────────────────────────────────────────────────────
        if kwh > Decimal::ZERO {
            if let (Some(ht), Some(nt)) = (meter.arbeitsmenge_ht_kwh, meter.arbeitsmenge_nt_kwh) {
                // Zweitarif (HT/NT)
                if let Some(ap_ht) = tariff.arbeitspreis_ht_ct_per_kwh
                    && ht > Decimal::ZERO
                {
                    positions.push(
                        arbeitspreis_position(
                            "Arbeitspreis Hochtarif (HT)",
                            ht,
                            ap_ht,
                            "kWh",
                            "§41 EnWG",
                            &["strom", "ht"],
                        )
                        .with_tag("strom"),
                    );
                }
                if let Some(ap_nt) = tariff.arbeitspreis_nt_ct_per_kwh
                    && nt > Decimal::ZERO
                {
                    positions.push(
                        arbeitspreis_position(
                            "Arbeitspreis Niedertarif (NT)",
                            nt,
                            ap_nt,
                            "kWh",
                            "§41 EnWG",
                            &["strom", "nt"],
                        )
                        .with_tag("strom"),
                    );
                }
            } else if let Some(ap_ct) = tariff.arbeitspreis_ct_per_kwh {
                positions.push(
                    arbeitspreis_position(
                        "Arbeitspreis Strom",
                        kwh,
                        ap_ct,
                        "kWh",
                        "§41 EnWG",
                        &["strom"],
                    )
                    .with_tag("strom"),
                );
            }
        }

        // ── EEG-Gutschrift pass-through ────────────────────────────────────────
        if let Some(eeg_ct) = quantities.eeg_gutschrift_eur
            && eeg_ct != Decimal::ZERO
        {
            positions.push(
                BillingPosition::credit(
                    "EEG-Gutschrift (Photovoltaik)",
                    Decimal::ONE,
                    "EUR",
                    eeg_ct.abs(),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§38 EEG 2023")
                .with_tag("eeg_gutschrift")
                .with_tag("solar"),
            );
        }

        // ── Grid charges (NNE / KA) ────────────────────────────────────────────
        let kwh_for_grid = kwh;
        if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
            let daily = nne_gp / dec!(365);
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Grundpreis",
                    Decimal::from(days),
                    "Tage",
                    daily,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_grundpreis")
                .with_tag("nne"),
            );
        }
        if let Some(nne_ap_ct) = grid.nne_arbeitspreis_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Arbeitspreis",
                    kwh_for_grid,
                    "kWh",
                    nne_ap_ct / dec!(100),
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_arbeitspreis")
                .with_tag("nne"),
            );
        }
        if let (Some(nne_lp), Some(kw)) = (
            grid.nne_leistungspreis_eur_per_kw_year,
            meter.spitzenleistung_kw,
        ) {
            let months_frac = Decimal::from(days) / dec!(30.4375);
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Leistungspreis",
                    kw,
                    "kW",
                    nne_lp / dec!(12) * months_frac,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_leistungspreis")
                .with_tag("nne"),
            );
        }
        if let Some(ka_ct) = grid.ka_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Konzessionsabgabe",
                    kwh_for_grid,
                    "kWh",
                    ka_ct / dec!(100),
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("KAV")
                .with_tag("konzessionsabgabe")
                .with_tag("nne"),
            );
        }

        // ── Stromsteuer ────────────────────────────────────────────────────────
        let st_rate = rates.effective_stromsteuer(tariff);
        if st_rate > Decimal::ZERO && kwh > Decimal::ZERO && tariff.category != "SOLAR" {
            positions.push(
                levy_position(
                    "Stromsteuer",
                    kwh,
                    "kWh",
                    st_rate,
                    "§3 StromStG",
                    "stromsteuer",
                )
                .with_tag("strom"),
            );
        }

        // ── §14a EnWG — Steuerbare Verbrauchseinrichtungen ─────────────────────
        // WAERMEPUMPE and WALLBOX: Modul 1 (capacity reduction) + Modul 3 (EV compensation)
        if matches!(tariff.category.as_str(), "WAERMEPUMPE" | "WALLBOX") {
            if let Some(sect14a_m1_ct) = tariff.sect14a_modul1_nne_reduktion_ct_per_kwh
                && sect14a_m1_ct > Decimal::ZERO
                && kwh > Decimal::ZERO
            {
                positions.push(
                    BillingPosition::credit(
                        "§14a EnWG Modul 1 — NNE Reduktion",
                        kwh,
                        "kWh",
                        sect14a_m1_ct / dec!(100),
                        PositionCategory::Credit,
                    )
                    .with_legal_basis("§14a EnWG")
                    .with_tag("§14a")
                    .with_tag("sect14a_modul1"),
                );
            }
            // Annual capacity-based NNE reduction (steuerungsrabatt_modul1_eur_per_kw_year)
            if let (Some(m1_year), Some(kw)) = (
                tariff.steuerungsrabatt_modul1_eur_per_kw_year,
                meter.spitzenleistung_kw,
            ) && m1_year > Decimal::ZERO
                && kw > Decimal::ZERO
            {
                let months_frac = Decimal::from(days) / dec!(30.4375);
                let monthly_per_kw = m1_year / dec!(12);
                positions.push(
                    BillingPosition::credit(
                        "§14a EnWG Modul 1 — Steuerungsrabatt NNE",
                        kw,
                        "kW",
                        monthly_per_kw * months_frac,
                        PositionCategory::Credit,
                    )
                    .with_legal_basis("§14a EnWG")
                    .with_tag("§14a")
                    .with_tag("sect14a_modul1"),
                );
            }
            // Annual capacity-based Modul 3 Entschädigung (steuerungsrabatt_modul3_eur_per_kw_year)
            // Compensation for each kW-hour of remote steuerung
            if let (Some(m3_year), Some(kw), Some(steuerung_h)) = (
                tariff.steuerungsrabatt_modul3_eur_per_kw_year,
                meter.spitzenleistung_kw,
                meter.steuerung_stunden,
            ) && m3_year > Decimal::ZERO
                && kw > Decimal::ZERO
                && steuerung_h > Decimal::ZERO
            {
                // compensation = rate × kW × (steuerung_h / 8760h per year)
                let rate = m3_year * kw * (steuerung_h / dec!(8760));
                positions.push(
                    BillingPosition::credit(
                        "§14a EnWG Modul 3 — Steuerungsentschädigung",
                        kw,
                        "kW",
                        m3_year * (steuerung_h / dec!(8760)),
                        PositionCategory::Credit,
                    )
                    .with_legal_basis("§14a EnWG")
                    .with_tag("§14a")
                    .with_tag("sect14a_modul3"),
                );
                let _ = rate;
            }
            if let (Some(modul3_ct), Some(steuerung_h)) = (
                tariff.sect14a_modul3_entschaedigung_ct_per_kwh,
                meter.steuerung_stunden,
            ) {
                let kw = meter.spitzenleistung_kw.unwrap_or(dec!(0));
                if modul3_ct > Decimal::ZERO && steuerung_h > Decimal::ZERO && kw > Decimal::ZERO {
                    let steuerung_kwh = kw * steuerung_h;
                    positions.push(
                        BillingPosition::credit(
                            "§14a EnWG Modul 3 — Steuerungsentschädigung",
                            steuerung_kwh,
                            "kWh",
                            modul3_ct / dec!(100),
                            PositionCategory::Credit,
                        )
                        .with_legal_basis("§14a EnWG")
                        .with_tag("§14a")
                        .with_tag("sect14a_modul3"),
                    );
                }
            }
        }

        Ok(positions)
    }
}

// ── GasProvider ───────────────────────────────────────────────────────────────

/// GAS billing provider.
///
/// Includes Brennwertkorrektur info, commodity positions, gas NNE,
/// Energiesteuer and BEHG CO₂ levy. Does NOT include MwSt.
pub struct GasProvider {
    tariff: TariffInput,
    grid: GridInput,
}

impl GasProvider {
    pub fn from_tariff(tariff: &TariffInput, grid: &GridInput) -> Self {
        Self {
            tariff: tariff.clone(),
            grid: grid.clone(),
        }
    }
}

impl BillingProvider for GasProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.gas.as_ref().cloned().unwrap_or_default();
        let days = ctx.days();
        let tariff = &self.tariff;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;

        // Compute kWh_Hs
        let kwh_hs = if let Some(kwh) = meter.kwh_hs {
            kwh
        } else {
            let hs = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
            let z = meter.zustandszahl.unwrap_or(dec!(1.0));
            (meter.messung_qm3 * hs * z).round_dp(3)
        };

        let mut positions: Vec<BillingPosition> = Vec::new();

        // ── Brennwertkorrektur (info position) ────────────────────────────────
        if meter.kwh_hs.is_none() && meter.brennwert_kwh_per_qm3.is_some() {
            let hs = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
            let z = meter.zustandszahl.unwrap_or(dec!(1.0));
            positions.push(BillingPosition {
                description: format!(
                    "Brennwertkorrektur: {:.4} kWh/m³ × {:.4} = {:.3} kWh_Hs",
                    hs, z, kwh_hs
                ),
                legal_basis: Some("§24 GasGVV / DVGW G 685".to_owned()),
                quantity: meter.messung_qm3,
                unit: "m³".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["brennwertkorrektur".to_owned(), "info".to_owned()],
            });
        }

        // ── Gas quality annotation (always added when set) ────────────────────
        // Carried as a tagged info position; to_rechnung_json() injects it as ZusatzAttribut.
        // Per DVGW G 260: the measured Brennwert already reflects the H2 blend —
        // this is a regulatory audit annotation, not a billing correction.
        if let Some(ref gq) = meter.gasqualitaet {
            positions.push(BillingPosition {
                description: format!("Gasqualität: {gq} (§ DVGW G 260)"),
                // Use legal_basis to carry the gasqualitaet value for to_rechnung_json()
                legal_basis: Some(gq.clone()),
                quantity: Decimal::ZERO,
                unit: "".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["gasqualitaet".to_owned(), "info".to_owned()],
            });
        }

        // ── Grundpreis ─────────────────────────────────────────────────────────
        if let Some(gp_ct_day) = tariff.gas_grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis Gas",
                    gp_ct_day / dec!(100),
                    days,
                    "§41 EnWG",
                    &["gas"],
                )
                .with_tag("gas"),
            );
        }

        // ── Arbeitspreis ───────────────────────────────────────────────────────
        if kwh_hs > Decimal::ZERO {
            if let Some(ap_ct) = tariff.gas_arbeitspreis_ct_per_kwh_hs {
                positions.push(
                    arbeitspreis_position(
                        "Arbeitspreis Gas",
                        kwh_hs,
                        ap_ct,
                        "kWh_Hs",
                        "§41 EnWG",
                        &["gas"],
                    )
                    .with_tag("gas"),
                );
            }

            // ── Gas NNE ────────────────────────────────────────────────────────
            if let Some(nne_gp) = grid.gas_nne_grundpreis_eur_per_year {
                let daily = nne_gp / dec!(365);
                positions.push(
                    BillingPosition::debit(
                        "Gasnetznutzungsentgelt Grundpreis",
                        Decimal::from(days),
                        "Tage",
                        daily,
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("GasNEV")
                    .with_tag("gas_nne_grundpreis")
                    .with_tag("nne"),
                );
            }
            if let Some(nne_ap_ct) = grid.gas_nne_arbeitspreis_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Gasnetznutzungsentgelt Arbeitspreis",
                        kwh_hs,
                        "kWh_Hs",
                        nne_ap_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("GasNEV")
                    .with_tag("gas_nne_arbeitspreis")
                    .with_tag("nne"),
                );
            }
            if let Some(ka_ct) = grid.gas_ka_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Konzessionsabgabe Gas",
                        kwh_hs,
                        "kWh_Hs",
                        ka_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("KAV")
                    .with_tag("gas_konzessionsabgabe")
                    .with_tag("nne"),
                );
            }
            if let Some(bilu_ct) = grid.gas_bilanzierungsumlage_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Bilanzierungsumlage Gas",
                        kwh_hs,
                        "kWh_Hs",
                        bilu_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("GasNZV")
                    .with_tag("gas_bilanzierungsumlage")
                    .with_tag("nne"),
                );
            }

            // ── Energiesteuer ──────────────────────────────────────────────────
            let est_rate = rates.effective_energiesteuer_gas(tariff);
            if est_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "Energiesteuer Erdgas",
                        kwh_hs,
                        "kWh_Hs",
                        est_rate,
                        "§2 Nr. 3 EnergieStG",
                        "energiesteuer_gas",
                    )
                    .with_tag("gas"),
                );
            }

            // ── BEHG CO₂ ───────────────────────────────────────────────────────
            let behg_rate = rates.effective_behg_gas(tariff);
            if behg_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "CO₂-Abgabe BEHG",
                        kwh_hs,
                        "kWh_Hs",
                        behg_rate,
                        "BEHG",
                        "behg",
                    )
                    .with_tag("gas"),
                );
            }
        }

        Ok(positions)
    }
}

// ── HeatProvider ──────────────────────────────────────────────────────────────

/// WAERME (Fernwärme) billing provider.
pub struct HeatProvider {
    tariff: TariffInput,
}

impl HeatProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for HeatProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.heat.as_ref().cloned().unwrap_or_default();
        let days = ctx.days();
        let tariff = &self.tariff;
        let mut positions: Vec<BillingPosition> = Vec::new();
        let months = meter.months.unwrap_or(dec!(1));

        if let Some(gp) = tariff.waerme_grundpreis_eur_per_month {
            positions.push(
                BillingPosition::debit(
                    "Grundpreis Fernwärme",
                    months,
                    "Monate",
                    gp,
                    PositionCategory::Commodity,
                )
                .with_tag("commodity")
                .with_tag("waerme"),
            );
        }
        if let (Some(lp), Some(kw)) = (
            tariff.waerme_leistungspreis_eur_per_kw_year.or_else(|| {
                tariff
                    .waerme_leistungspreis_eur_per_kw_month
                    .map(|m| m * dec!(12))
            }),
            meter.spitzenleistung_kw,
        ) {
            // Use meter.months directly when provided (more accurate than day-based proration)
            let billing_months = meter
                .months
                .unwrap_or_else(|| Decimal::from(days) / dec!(30.4375));
            positions.push(
                BillingPosition::debit(
                    "Leistungspreis Fernwärme",
                    kw,
                    "kW",
                    lp / dec!(12) * billing_months,
                    PositionCategory::Commodity,
                )
                .with_tag("commodity")
                .with_tag("waerme"),
            );
        }
        if let Some(ap_ct) = tariff.waerme_arbeitspreis_ct_per_kwh
            && meter.kwh_waerme > Decimal::ZERO
        {
            positions.push(
                arbeitspreis_position(
                    "Arbeitspreis Fernwärme",
                    meter.kwh_waerme,
                    ap_ct,
                    "kWh_th",
                    "§41 EnWG",
                    &["waerme"],
                )
                .with_tag("waerme"),
            );
        }
        Ok(positions)
    }
}

// ── SolarProvider ─────────────────────────────────────────────────────────────

/// SOLAR (Eigenverbrauch / Mieterstrom §38a / §42a GGV) billing provider.
pub struct SolarProvider {
    tariff: TariffInput,
}

impl SolarProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for SolarProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.solar.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let kwh = meter.eigenverbrauch_kwh;
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(ap_ct) = tariff.solar_arbeitspreis_ct_per_kwh {
            positions.push(
                arbeitspreis_position(
                    "Arbeitspreis Solarstrom (Eigenverbrauch)",
                    kwh,
                    ap_ct,
                    "kWh",
                    "§42a EEG 2023",
                    &["solar"],
                )
                .with_tag("solar"),
            );
        }
        if let Some(ms_ct) = tariff.mieterstrom_aufschlag_ct_per_kwh {
            positions.push(
                arbeitspreis_position(
                    "Mieterstrom-Aufschlag (§38a EEG 2023)",
                    kwh,
                    ms_ct,
                    "kWh",
                    "§38a EEG 2023",
                    &["solar", "mieterstrom"],
                )
                .with_tag("mieterstrom_aufschlag"),
            );
        }
        if let Some(rabatt_ct) = tariff.gemeinschaft_rabatt_ct_per_kwh {
            positions.push(
                BillingPosition::credit(
                    "Rabatt Gemeinschaftliche Gebäudeversorgung (§42a EEG)",
                    kwh,
                    "kWh",
                    rabatt_ct / dec!(100),
                    PositionCategory::Discount,
                )
                .with_legal_basis("§42a EEG 2023")
                .with_tag("gemeinschaft_rabatt")
                .with_tag("solar"),
            );
        }
        Ok(positions)
    }
}

// ── EegProvider ───────────────────────────────────────────────────────────────

/// EEG feed-in settlement billing provider.
///
/// **Preferred path**: when `quantities.eeg_full` is set, delegates to
/// `eeg_billing::calculate_settlement()` for version-aware §51/§52/§44b rules.
///
/// **Fallback path**: when only `quantities.eeg` is set, uses the simplified
/// EEG credit note formula (Vergütung, Marktprämie, Managementprämie, KWKG).
/// This is suitable for LF-side Gutschrift documents where plant-specific
/// regulatory details (§52 sanctions, §44b biogas quota) are not relevant.
///
/// ## Recommended usage
///
/// - **NB-side settlement** (plant registry, MaStR compliance): use `einsd` + `eeg-billing`
/// - **LF-side credit notes** (monthly Gutschrift to generator): use `EegProvider`
///   with `eeg_full` when plant parameters are available, `eeg` otherwise
pub struct EegProvider {
    tariff: TariffInput,
}

impl EegProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for EegProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        // ── Preferred path: delegate to eeg-billing for full regulatory accuracy ──
        if let Some(eeg_full) = &quantities.eeg_full {
            return bill_eeg_full(eeg_full, ctx);
        }

        // ── Fallback: simplified EEG credit note ──────────────────────────────
        let meter = quantities.eeg.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let kwh = meter.einspeisung_kwh;

        let billable_kwh = meter
            .kwh_during_negative_epex
            .map(|neg| (kwh - neg).max(Decimal::ZERO))
            .unwrap_or(kwh);

        let suspended_kwh = kwh - billable_kwh;
        let mut positions: Vec<BillingPosition> = Vec::new();

        if suspended_kwh > Decimal::ZERO {
            positions.push(BillingPosition {
                description: "Keine Vergütung (§51 EEG Negativpreisregel)".to_owned(),
                legal_basis: Some("§51 EEG 2023".to_owned()),
                quantity: suspended_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: Decimal::ZERO,
                net_eur: Decimal::ZERO,
                category: PositionCategory::Info,
                tags: vec!["eeg_negativpreis_suspension".to_owned(), "info".to_owned()],
            });
        }
        if let Some(vg_ct) = tariff.eeg_verguetungssatz_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "EEG Einspeisevergütung",
                    billable_kwh,
                    "kWh",
                    vg_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§21 EEG 2023")
                .with_tag("eeg_verguetung")
                .with_tag("eeg"),
            );
        }
        if let Some(mp_ct) = tariff.eeg_marktpraemie_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "EEG Marktprämie",
                    kwh,
                    "kWh",
                    mp_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 EEG 2023")
                .with_tag("eeg_marktpraemie")
                .with_tag("eeg"),
            );
        }
        if let Some(mgp_ct) = tariff.eeg_managementpraemie_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Managementprämie Direktvermarktung",
                    kwh,
                    "kWh",
                    mgp_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 Abs. 3 EEG 2023")
                .with_tag("eeg_managementpraemie")
                .with_tag("eeg"),
            );
        }
        if let Some(kwkg_ct) = tariff.kwkg_zuschlag_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "KWKG Zuschlag",
                    kwh,
                    "kWh",
                    kwkg_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§7 KWKG 2023")
                .with_tag("kwkg_zuschlag")
                .with_tag("kwkg"),
            );
        }
        Ok(positions)
    }
}

/// Bridge from eeg-billing SettleOutput → Vec<BillingPosition>.
///
/// EEG settlements are positive values — the generator receives this amount.
fn bill_eeg_full(
    settle_input: &eeg_billing::SettleInput,
    _ctx: &BillingContext,
) -> Result<Vec<BillingPosition>, BillingError> {
    let output = eeg_billing::calculate_settlement(settle_input);
    let positions = output
        .positions
        .into_iter()
        .map(|p| BillingPosition {
            description: p.description,
            legal_basis: Some(p.legal_basis),
            quantity: p.kwh,
            unit: "kWh".to_owned(),
            unit_price_eur: p.rate_ct_kwh / dec!(100),
            // Positive: generator receives payment (credit note perspective)
            net_eur: validated_eur(p.eur),
            category: PositionCategory::Credit,
            tags: vec!["eeg".to_owned(), "eeg_full".to_owned()],
        })
        .collect();
    Ok(positions)
}

// ── EinspeisungProvider ───────────────────────────────────────────────────────

/// Non-EEG Direktvermarktung feed-in settlement (EINSPEISUNG).
pub struct EinspeisungProvider {
    tariff: TariffInput,
}

impl EinspeisungProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for EinspeisungProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let meter = quantities.einspeisung.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let kwh = meter.einspeisung_kwh;
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(mv_ct) = tariff.marktwert_ct_per_kwh {
            positions.push(
                BillingPosition::debit(
                    "Marktwert Strom (EPEX Spot Monatsmarktwert)",
                    kwh,
                    "kWh",
                    mv_ct / dec!(100),
                    PositionCategory::Credit,
                )
                .with_legal_basis("§20 EEG 2023")
                .with_tag("marktwert")
                .with_tag("einspeisung"),
            );
        }
        if let Some(vm_ct) = tariff.vermarktungsgebuehr_ct_per_kwh {
            // Vermarktungsgebühr is a cost for the generator (reduces net payment)
            positions.push(
                BillingPosition::debit(
                    "Vermarktungsgebühr Direktvermarktung",
                    kwh,
                    "kWh",
                    -(vm_ct / dec!(100)), // negative: cost deducted from settlement
                    PositionCategory::Fee,
                )
                .with_tag("vermarktungsgebuehr")
                .with_tag("einspeisung"),
            );
        }
        Ok(positions)
    }
}

// ── HemsProvider ──────────────────────────────────────────────────────────────

/// HEMS subscription + event billing provider.
pub struct HemsProvider {
    tariff: TariffInput,
}

impl HemsProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for HemsProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let usage = quantities.hems.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        // Support both field names: hems_subscription_eur_per_month and hems_platform_fee_eur_per_month
        let sub_eur = tariff
            .hems_subscription_eur_per_month
            .or(tariff.hems_platform_fee_eur_per_month);

        if let Some(sub_eur) = sub_eur {
            positions.push(
                BillingPosition::debit(
                    "HEMS Grundgebühr",
                    months,
                    "Monate",
                    sub_eur,
                    PositionCategory::Fee,
                )
                .with_tag("hems_subscription")
                .with_tag("hems"),
            );
        }
        if let (Some(events), Some(event_eur)) = (
            usage.optimization_events,
            tariff.hems_optimization_event_eur,
        ) && events > 0
        {
            positions.push(
                BillingPosition::debit(
                    "HEMS Optimierungsereignisse",
                    Decimal::from(events),
                    "Ereignisse",
                    event_eur,
                    PositionCategory::Fee,
                )
                .with_tag("hems_events")
                .with_tag("hems"),
            );
        }
        if let (Some(reads), Some(read_eur)) = (usage.readout_events, tariff.hems_readout_event_eur)
            && reads > 0
        {
            positions.push(
                BillingPosition::debit(
                    "HEMS Smart Meter Ablesungen",
                    Decimal::from(reads),
                    "Ablesungen",
                    read_eur,
                    PositionCategory::Fee,
                )
                .with_tag("hems_readouts")
                .with_tag("hems"),
            );
        }
        Ok(positions)
    }
}

// ── EmobilityProvider ─────────────────────────────────────────────────────────

/// E-Mobility CPO/EMSP billing provider.
pub struct EmobilityProvider {
    tariff: TariffInput,
}

impl EmobilityProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for EmobilityProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let usage = quantities.emobility.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        // Support field aliases
        let svc_eur = tariff
            .emobility_service_fee_eur
            .or(tariff.emobility_service_fee_eur_per_month);
        let kwh_price = tariff
            .emobility_kwh_price_ct
            .or(tariff.emobility_arbeitspreis_ct_per_kwh);

        if let Some(svc_eur) = svc_eur {
            positions.push(
                BillingPosition::debit(
                    "E-Mobility Servicegebühr",
                    months,
                    "Monate",
                    svc_eur,
                    PositionCategory::Fee,
                )
                .with_tag("emobility_service")
                .with_tag("emobility"),
            );
        }
        if let (Some(kwh), Some(kwh_price_ct)) = (usage.kwh_charged, kwh_price)
            && kwh > Decimal::ZERO
        {
            positions.push(
                arbeitspreis_position(
                    "E-Mobility Ladeenergie",
                    kwh,
                    kwh_price_ct,
                    "kWh",
                    "§41a EnWG",
                    &["emobility"],
                )
                .with_tag("emobility"),
            );
        }
        if let (Some(sessions), Some(session_eur)) =
            (usage.sessions, tariff.emobility_session_fee_eur)
            && sessions > 0
        {
            positions.push(
                BillingPosition::debit(
                    "E-Mobility Ladesessionsgebühr",
                    Decimal::from(sessions),
                    "Sessionen",
                    session_eur,
                    PositionCategory::Fee,
                )
                .with_tag("emobility_sessions")
                .with_tag("emobility"),
            );
        }
        if let (Some(roaming), Some(roaming_eur)) =
            (usage.roaming_sessions, tariff.emobility_roaming_fee_eur)
            && roaming > 0
        {
            positions.push(
                BillingPosition::debit(
                    "E-Mobility Roaming-Gebühr",
                    Decimal::from(roaming),
                    "Sessionen",
                    roaming_eur,
                    PositionCategory::Fee,
                )
                .with_tag("emobility_roaming")
                .with_tag("emobility"),
            );
        }
        Ok(positions)
    }
}

// ── ServiceProvider ───────────────────────────────────────────────────────────

/// Energiedienstleistung (MSB, EMS, maintenance) billing provider.
pub struct ServiceProvider {
    tariff: TariffInput,
}

impl ServiceProvider {
    pub fn from_tariff(tariff: &TariffInput) -> Self {
        Self {
            tariff: tariff.clone(),
        }
    }
}

impl BillingProvider for ServiceProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let usage = quantities.service.as_ref().cloned().unwrap_or_default();
        let tariff = &self.tariff;
        let months = usage.months.unwrap_or(dec!(1));
        let mut positions: Vec<BillingPosition> = Vec::new();

        if let Some(fee_eur) = tariff.service_fee_eur {
            positions.push(
                BillingPosition::debit(
                    "Energiedienstleistung Grundgebühr",
                    months,
                    "Monate",
                    fee_eur,
                    PositionCategory::Fee,
                )
                .with_tag("service"),
            );
        }
        let event_price = usage.event_price_eur.or(tariff.service_event_price_eur);
        if let (Some(events), Some(event_eur)) = (usage.event_count, event_price)
            && events > 0
        {
            positions.push(
                BillingPosition::debit(
                    "Energiedienstleistung Ereignisgebühr",
                    Decimal::from(events),
                    "Ereignisse",
                    event_eur,
                    PositionCategory::Fee,
                )
                .with_tag("service_events")
                .with_tag("service"),
            );
        }
        Ok(positions)
    }
}

// ── DynamicElectricityProvider ────────────────────────────────────────────────

/// §41a EnWG dynamic electricity tariff — per-interval EPEX Spot pricing.
///
/// Accepts any `SpotPriceSource` implementation, not just EPEX.
/// Also includes NNE and Stromsteuer positions.
pub struct DynamicElectricityProvider {
    tariff: TariffInput,
    grid: GridInput,
    spot_price_source: Box<dyn crate::provider::SpotPriceSource>,
}

impl DynamicElectricityProvider {
    pub fn new(
        tariff: TariffInput,
        grid: GridInput,
        spot_source: impl crate::provider::SpotPriceSource + 'static,
    ) -> Self {
        Self {
            tariff,
            grid,
            spot_price_source: Box::new(spot_source),
        }
    }

    pub fn with_epex_map(
        tariff: TariffInput,
        grid: GridInput,
        epex_prices: std::collections::HashMap<(i32, u8, u8, u8), Decimal>,
    ) -> Self {
        Self::new(
            tariff,
            grid,
            crate::provider::EpexSpotSource {
                prices: epex_prices,
            },
        )
    }
}

impl BillingProvider for DynamicElectricityProvider {
    fn bill(
        &self,
        ctx: &BillingContext,
        quantities: &Quantities,
        _prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        let tariff = &self.tariff;
        let grid = &self.grid;
        let rates = &ctx.regulatory_rates;
        let days = ctx.days();
        let floor_ct = tariff.dynamic_epex_floor_ct_kwh;
        let source_name = self.spot_price_source.source_name().to_owned();
        let mut positions: Vec<BillingPosition> = Vec::new();

        // Grundpreis
        if let Some(gp_ct_day) = tariff.grundpreis_ct_per_day {
            positions.push(
                grundpreis_position(
                    "Grundpreis Strom (§41a)",
                    gp_ct_day / dec!(100),
                    days,
                    "§41a EnWG",
                    &["strom"],
                )
                .with_tag("strom"),
            );
        }

        // Per-interval EPEX pricing.
        // Primary source: `self.spot_price_source` (e.g. live API, Tibber, NordPool).
        // Fallback: `quantities.dynamic_epex_prices` (pre-fetched map set by billingd
        // from `marktd GET /api/v1/epex-preise`). This is the typical production path
        // when `build_engine()` creates the provider before prices are known.
        let mut total_kwh = Decimal::ZERO;
        let mut total_energy_eur = Decimal::ZERO;
        let mut missing_price_intervals: u32 = 0;

        for interval in &quantities.dynamic_intervals {
            // Try primary SpotPriceSource first, then quantities fallback.
            let price_ct = self
                .spot_price_source
                .price_ct_kwh(interval.timestamp_utc)
                .or_else(|| {
                    if quantities.dynamic_epex_prices.is_empty() {
                        return None;
                    }
                    // Same UTC → Berlin conversion as EpexSpotSource.
                    use time_tz::{OffsetDateTimeExt, timezones};
                    let berlin = timezones::db::europe::BERLIN;
                    let local = interval.timestamp_utc.to_timezone(berlin);
                    let key = (local.year(), local.month() as u8, local.day(), local.hour());
                    quantities.dynamic_epex_prices.get(&key).copied()
                });

            let Some(price_ct) = price_ct else {
                missing_price_intervals += 1;
                continue;
            };
            let effective_ct = if let Some(floor) = floor_ct {
                price_ct.max(floor)
            } else {
                price_ct
            };
            let eur = validated_eur(interval.kwh * effective_ct / dec!(100));
            total_kwh += interval.kwh;
            total_energy_eur += eur;
        }

        if missing_price_intervals > 0 {
            tracing::warn!(
                missing_intervals = missing_price_intervals,
                total_intervals = quantities.dynamic_intervals.len(),
                "DynamicElectricityProvider: {missing_price_intervals} interval(s) skipped \
                 (no EPEX price data). Set quantities.dynamic_epex_prices from marktd."
            );
        }

        if total_kwh > Decimal::ZERO {
            let avg_ct = if total_kwh.is_zero() {
                Decimal::ZERO
            } else {
                (total_energy_eur * dec!(100) / total_kwh).round_dp(4)
            };
            positions.push(BillingPosition {
                description: format!("Arbeitspreis {source_name} (∅ {avg_ct:.4} ct/kWh)",),
                legal_basis: Some("§41a EnWG".to_owned()),
                quantity: total_kwh,
                unit: "kWh".to_owned(),
                unit_price_eur: avg_ct / dec!(100),
                net_eur: validated_eur(total_energy_eur),
                category: PositionCategory::Commodity,
                tags: vec![
                    "commodity".to_owned(),
                    "arbeitspreis".to_owned(),
                    "strom".to_owned(),
                    "§41a".to_owned(),
                ],
            });

            // NNE + KA
            if let Some(nne_ap_ct) = grid.nne_arbeitspreis_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Netznutzungsentgelt Arbeitspreis",
                        total_kwh,
                        "kWh",
                        nne_ap_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("StromNEV")
                    .with_tag("nne_arbeitspreis")
                    .with_tag("nne"),
                );
            }
            if let Some(ka_ct) = grid.ka_ct_per_kwh {
                positions.push(
                    BillingPosition::debit(
                        "Konzessionsabgabe",
                        total_kwh,
                        "kWh",
                        ka_ct / dec!(100),
                        PositionCategory::GridCharge,
                    )
                    .with_legal_basis("KAV")
                    .with_tag("konzessionsabgabe")
                    .with_tag("nne"),
                );
            }

            // Stromsteuer
            let st_rate = rates.effective_stromsteuer(tariff);
            if st_rate > Decimal::ZERO {
                positions.push(
                    levy_position(
                        "Stromsteuer",
                        total_kwh,
                        "kWh",
                        st_rate,
                        "§3 StromStG",
                        "stromsteuer",
                    )
                    .with_tag("strom"),
                );
            }
        }

        // NNE Grundpreis
        if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
            let daily = nne_gp / dec!(365);
            positions.push(
                BillingPosition::debit(
                    "Netznutzungsentgelt Grundpreis",
                    Decimal::from(days),
                    "Tage",
                    daily,
                    PositionCategory::GridCharge,
                )
                .with_legal_basis("StromNEV")
                .with_tag("nne_grundpreis")
                .with_tag("nne"),
            );
        }

        Ok(positions)
    }
}

// ── MwStProvider ──────────────────────────────────────────────────────────────

/// MwSt (Mehrwertsteuer / Umsatzsteuer) provider.
///
/// **Must be registered last** — computes tax on the sum of ALL prior positions.
///
/// ## §12 Abs. 3 UStG (Solarpaket I)
///
/// Solar PV ≤ 30 kWp: zero MwSt from 01.01.2023. Configure via `mwst_rate_override = 0.0`
/// in the product definition (tarifbd).
pub struct MwStProvider {
    rate: Decimal,
}

impl MwStProvider {
    /// Construct with the configured MwSt rate (e.g. `dec!(0.19)`).
    #[must_use]
    pub fn new(rate: Decimal) -> Self {
        Self { rate }
    }
}

impl BillingProvider for MwStProvider {
    fn bill(
        &self,
        _ctx: &BillingContext,
        _quantities: &Quantities,
        prior: &[BillingPosition],
    ) -> Result<Vec<BillingPosition>, BillingError> {
        if self.rate.is_zero() {
            return Ok(vec![]);
        }
        let net_base: Decimal = prior.iter().map(|p| p.net_eur).sum();
        if net_base.is_zero() {
            return Ok(vec![]);
        }
        let mwst_eur = validated_eur(net_base.abs() * self.rate);
        // Sign follows the net base (credit invoices → negative MwSt)
        let mwst_eur = if net_base < Decimal::ZERO {
            -mwst_eur
        } else {
            mwst_eur
        };
        Ok(vec![BillingPosition {
            description: format!("Mehrwertsteuer {:.0} %", self.rate * dec!(100)),
            legal_basis: Some("§12 UStG".to_owned()),
            quantity: Decimal::ONE,
            unit: "%".to_owned(),
            unit_price_eur: mwst_eur,
            net_eur: mwst_eur,
            category: PositionCategory::Tax,
            tags: vec!["mwst".to_owned(), "tax".to_owned()],
        }])
    }

    fn is_tax_pass(&self) -> bool {
        true
    }
}
