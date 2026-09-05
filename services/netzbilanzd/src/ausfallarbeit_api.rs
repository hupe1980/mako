//! BilAReM Kap. 3 Ausfallarbeit compute API (BK6-23-241, Beschluss
//! 07.05.2026).
//!
//! Stateless, schema-validated compute endpoints over the pure engine in
//! `mako_redispatch::ausfallarbeit`. The caller supplies the quarter-hour
//! series (measured `P_ist`, theoretical `P_theo` from the Leistungskennlinie,
//! irradiation, Ex-ante-Planungsdaten, …) — sourcing those series from SCADA /
//! edmd / DWD stays with the operator until the EDI@Energy BilAReM wire
//! formats are published (relative go-live ≤ 6 months after publication).
//!
//! Routes:
//! - `POST /api/v1/redispatch/ausfallarbeit/compute` — per-TR W_A series
//!   (one entry per Viertelstunde) plus the sum, for every Kap.-3 variant.
//! - `POST /api/v1/redispatch/ausfallarbeit/ueberbauung` — the Kap.-3.4 cap
//!   for one Viertelstunde of a Netzlokation across its TR.
//! - `POST /api/v1/redispatch/ausfallarbeit/kf-bin` — the Kap.-3.2.3.2
//!   Wind-Bin-Verfahren factor `KF_Bin` for offshore Windenergieanlagen, to be
//!   fed back in as `kf` on a `wind_spitz` request.
//! - `POST /api/v1/redispatch/ausfallarbeit/malo-split` — splits one
//!   marktlokationsscharfer Wert onto the TR behind the MaLo
//!   (§ 24 Abs. 3 S. 2 EEG 2023).
//! - `POST /api/v1/redispatch/ausfallarbeit/vergleichstag` — selects the Solar
//!   Vergleichstag (Kap. 3.2.4.1) and returns `P_VZ,ist` and `G_VZ`.
//! - `POST /api/v1/redispatch/ausfallarbeit/vergleichszeitraum` — selects the
//!   Kap.-3.2.2.1 Vergleichszeitraum out of a TR's quarter-hour series and
//!   returns the `KF` to feed back as `kf` on a `wind_spitz` request.

use std::sync::Arc;

use axum::{Extension, Json};
use mako_redispatch::ausfallarbeit as engine;
use mako_service::{ApiError, ApiResult, oidc::Claims};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::config::NetzbilanzConfig;
use crate::handlers::{Authz, authorize};

/// Shorthand for the handler's shared state — the tenant every authorization
/// decision is made against.
type Cfg = Extension<Arc<NetzbilanzConfig>>;

/// Upper bound on intervals per request (~ one year of quarter-hours).
const MAX_INTERVALLE: usize = 35_136;

// ── Request / response types ────────────────────────────────────────────────

/// One quarter-hour of a Wind-Spitzabrechnung (Kap. 3.2.2.1 / 3.2.2.2) — with
/// `kf` = `KF_Bin` this is also the Wind-Bin-Verfahren (Kap. 3.2.3.2).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindSpitzIntervall {
    /// Kap.-3.1 inputs for `P_lim,i`.
    pub limitierung: engine::Leistungslimitierung,
    /// Korrekturfaktor `KF` (or `KF_Bin = KF_LBin × KF_V`).
    pub kf: Decimal,
    /// Theoretischer Leistungsmittelwert in kW.
    pub p_theo: Decimal,
    /// Marktbedingte Anpassung in kW, where one applies.
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    /// Beanspruchbare Leistung in kW — caps `P_theo` when supplied.
    #[serde(default)]
    pub p_bean: Option<Decimal>,
    /// Nennleistung of the TR in kW (plausibility cap).
    pub p_nenn: Decimal,
}

/// One quarter-hour of the Wind Pauschal-Abrechnung (Kap. 3.2.2.3).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindPauschalIntervall {
    /// Kap.-3.1 inputs for `P_lim,i`.
    pub limitierung: engine::Leistungslimitierung,
    /// Last fully measured unrestricted quarter-hour before the Maßnahme
    /// (or the Referenzprofilverfahren value), in kW.
    pub p_0: Decimal,
    /// Installierte Leistung of the TR in kW.
    pub p_inst: Decimal,
    /// Marktbedingte Anpassung in kW, where one applies.
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    /// Beanspruchbare Leistung in kW.
    #[serde(default)]
    pub p_bean: Option<Decimal>,
}

/// One quarter-hour of a Solar-Spitzabrechnung (Kap. 3.2.4.1 / 3.2.4.2).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolarSpitzIntervall {
    /// Kap.-3.1 inputs for `P_lim,i`.
    pub limitierung: engine::Leistungslimitierung,
    /// Durchschnittliche Ist-Einspeisung im Vergleichszeitraum in kW.
    pub p_vz_ist: Decimal,
    /// Durchschnittliche Einstrahlleistung im Vergleichszeitraum in kW/m².
    pub g_vz: Decimal,
    /// Einstrahlleistung of the Viertelstunde in kW/m².
    pub g_i: Decimal,
    /// Wechselrichterleistung je TR in kW.
    pub p_wr: Decimal,
    /// Marktbedingte Anpassung in kW, where one applies.
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    /// Beanspruchbare Leistung in kW.
    #[serde(default)]
    pub p_bean: Option<Decimal>,
    /// Nennleistung of the TR in kW (plausibility cap).
    pub p_nenn: Decimal,
}

/// One quarter-hour of the Solar Pauschal-Abrechnung (Kap. 3.2.4.3). The
/// Anlagenfaktor comes from the fixed season/time-of-day table (UTC+1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolarPauschalIntervall {
    /// Kap.-3.1 inputs for `P_lim,i`.
    pub limitierung: engine::Leistungslimitierung,
    /// Start of the Viertelstunde (date), UTC+1.
    pub datum: time::Date,
    /// Start of the Viertelstunde (time of day), UTC+1.
    pub zeit: time::Time,
    /// Summe der Nennleistung der Module in kW.
    pub p_inst_module: Decimal,
    /// Wechselrichterleistung je TR in kW.
    pub p_wr: Decimal,
    /// Marktbedingte Anpassung in kW, where one applies.
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    /// Beanspruchbare Leistung in kW.
    #[serde(default)]
    pub p_bean: Option<Decimal>,
}

/// One quarter-hour of a nicht-fluktuierende Abrechnung (Kap. 3.3).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NichtfluktuierendIntervall {
    /// Kap.-3.1 inputs for `P_lim,i`.
    pub limitierung: engine::Leistungslimitierung,
    /// Spitz: geplante Leistung per Ex-ante-Planungsdaten in kW.
    /// Pauschal: last fully measured quarter-hour `P_0` in kW.
    pub p_wert: Decimal,
    /// Pauschal only: beanspruchbare Leistung in kW.
    #[serde(default)]
    pub p_bean: Option<Decimal>,
}

/// Compute request — tagged by BilAReM Kap.-3 Abrechnungsvariante.
///
/// Fluktuierende variants (Kap. 3.2) are defined for negativen Redispatch
/// only; the nicht-fluktuierenden variants (Kap. 3.3) carry the direction.
#[derive(Debug, Deserialize)]
#[serde(tag = "verfahren", rename_all = "snake_case", deny_unknown_fields)]
pub enum AusfallarbeitComputeRequest {
    /// Kap. 3.2.2.1/3.2.2.2 (and 3.2.3.2 with `kf` = `KF_Bin`).
    WindSpitz { intervalle: Vec<WindSpitzIntervall> },
    /// Kap. 3.2.2.3 (grandfathered TR only, until 31.12.2028).
    WindPauschal {
        intervalle: Vec<WindPauschalIntervall>,
    },
    /// Kap. 3.2.4.1/3.2.4.2.
    SolarSpitz {
        intervalle: Vec<SolarSpitzIntervall>,
    },
    /// Kap. 3.2.4.3 (grandfathered TR only, until 31.12.2028).
    SolarPauschal {
        intervalle: Vec<SolarPauschalIntervall>,
    },
    /// Kap. 3.3.1 (Planwertmodell; Prognosemodell on request).
    NichtfluktuierendSpitz {
        /// Positiver or negativer Redispatch.
        richtung: engine::RedispatchRichtung,
        /// The quarter-hour series.
        intervalle: Vec<NichtfluktuierendIntervall>,
    },
    /// Kap. 3.3.2 (Prognosemodell default).
    NichtfluktuierendPauschal {
        /// Positiver or negativer Redispatch.
        richtung: engine::RedispatchRichtung,
        /// The quarter-hour series.
        intervalle: Vec<NichtfluktuierendIntervall>,
    },
}

/// Per-Viertelstunde Ausfallarbeit plus totals.
#[derive(Debug, Serialize)]
pub struct AusfallarbeitComputeResponse {
    /// `W_A,i` per Viertelstunde in kWh (BilAReM sign convention: ≥ 0 for
    /// negativen, ≤ 0 for positiven Redispatch).
    pub w_a_kwh: Vec<Decimal>,
    /// Sum over all intervals in kWh.
    pub summe_kwh: Decimal,
}

/// Kap.-3.4 Überbauung request — one Viertelstunde of one Netzlokation.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UeberbauungRequest {
    /// All TR behind the Netzlokation with their Kap.-3.2/3.3 Ausfallarbeit.
    pub trs: Vec<engine::UeberbauungTr>,
    /// Vertragliche (or smaller tatsächliche) Anschlussleistung in kW.
    pub p_anschl_kw: Decimal,
    /// Einspeisung über die Netzlokation in the Viertelstunde in kWh.
    pub einspeisung_kwh: Decimal,
}

/// Gekürzte Ausfallarbeit per TR (same order as the request).
#[derive(Debug, Serialize)]
pub struct UeberbauungResponse {
    /// The capped `W_A` per TR, in the order the request listed them.
    pub w_a_gekuerzt_kwh: Vec<Decimal>,
}

// ── Pure computation (unit-testable without axum) ───────────────────────────

fn compute(req: &AusfallarbeitComputeRequest) -> Result<Vec<Decimal>, String> {
    use engine::RedispatchRichtung::Negativ;
    let series: Vec<Decimal> = match req {
        AusfallarbeitComputeRequest::WindSpitz { intervalle } => intervalle
            .iter()
            .map(|iv| {
                engine::wind_spitz(&engine::WindSpitzInput {
                    kf: iv.kf,
                    p_theo: iv.p_theo,
                    p_mba: iv.p_mba,
                    p_bean: iv.p_bean,
                    p_lim: iv.limitierung.wert(Negativ),
                    p_nenn: iv.p_nenn,
                })
            })
            .collect(),
        AusfallarbeitComputeRequest::WindPauschal { intervalle } => intervalle
            .iter()
            .map(|iv| {
                engine::wind_pauschal(
                    iv.p_0,
                    iv.p_inst,
                    iv.p_mba,
                    iv.p_bean,
                    iv.limitierung.wert(Negativ),
                )
            })
            .collect(),
        AusfallarbeitComputeRequest::SolarSpitz { intervalle } => intervalle
            .iter()
            .map(|iv| {
                engine::solar_spitz(&engine::SolarSpitzInput {
                    p_vz_ist: iv.p_vz_ist,
                    g_vz: iv.g_vz,
                    g_i: iv.g_i,
                    p_wr: iv.p_wr,
                    p_mba: iv.p_mba,
                    p_bean: iv.p_bean,
                    p_lim: iv.limitierung.wert(Negativ),
                    p_nenn: iv.p_nenn,
                })
                .map_err(|e| e.to_string())
            })
            .collect::<Result<_, _>>()?,
        AusfallarbeitComputeRequest::SolarPauschal { intervalle } => intervalle
            .iter()
            .map(|iv| {
                engine::solar_pauschal(
                    engine::anlagenfaktor(iv.datum, iv.zeit),
                    iv.p_inst_module,
                    iv.p_wr,
                    iv.p_mba,
                    iv.p_bean,
                    iv.limitierung.wert(Negativ),
                )
            })
            .collect(),
        AusfallarbeitComputeRequest::NichtfluktuierendSpitz {
            richtung,
            intervalle,
        } => intervalle
            .iter()
            .map(|iv| {
                engine::nichtfluktuierend_spitz(
                    *richtung,
                    iv.p_wert,
                    iv.limitierung.wert(*richtung),
                )
            })
            .collect(),
        AusfallarbeitComputeRequest::NichtfluktuierendPauschal {
            richtung,
            intervalle,
        } => intervalle
            .iter()
            .map(|iv| {
                engine::nichtfluktuierend_pauschal(
                    *richtung,
                    iv.p_wert,
                    iv.p_bean,
                    iv.limitierung.wert(*richtung),
                )
            })
            .collect(),
    };
    Ok(series)
}

fn interval_count(req: &AusfallarbeitComputeRequest) -> usize {
    match req {
        AusfallarbeitComputeRequest::WindSpitz { intervalle } => intervalle.len(),
        AusfallarbeitComputeRequest::WindPauschal { intervalle } => intervalle.len(),
        AusfallarbeitComputeRequest::SolarSpitz { intervalle } => intervalle.len(),
        AusfallarbeitComputeRequest::SolarPauschal { intervalle } => intervalle.len(),
        AusfallarbeitComputeRequest::NichtfluktuierendSpitz { intervalle, .. }
        | AusfallarbeitComputeRequest::NichtfluktuierendPauschal { intervalle, .. } => {
            intervalle.len()
        }
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// `POST /api/v1/redispatch/ausfallarbeit/compute`
///
/// # Errors
///
/// - `400` for an empty or oversized interval series.
/// - `422` when a variant's inputs do not describe a computable Ausfallarbeit.
pub async fn post_ausfallarbeit_compute(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(cfg): Cfg,
    Json(req): Json<AusfallarbeitComputeRequest>,
) -> ApiResult<Json<AusfallarbeitComputeResponse>> {
    authorize(&cedar, &claims, "compute-ausfallarbeit", &cfg.tenant)?;
    let n = interval_count(&req);
    if n == 0 {
        return Err(ApiError::bad_request("intervalle is empty"));
    }
    if n > MAX_INTERVALLE {
        return Err(ApiError::bad_request(format!(
            "too many intervalle: {n} (max {MAX_INTERVALLE}, about one year of quarter-hours)"
        )));
    }
    let w_a_kwh = compute(&req).map_err(ApiError::Unprocessable)?;
    let summe_kwh = w_a_kwh.iter().copied().sum();
    Ok(Json(AusfallarbeitComputeResponse { w_a_kwh, summe_kwh }))
}

/// `POST /api/v1/redispatch/ausfallarbeit/ueberbauung`
///
/// # Errors
///
/// - `400` when no TechnischeRessource is named.
/// - `422` when the Kap.-3.4 cap cannot be applied to the given series.
pub async fn post_ausfallarbeit_ueberbauung(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(cfg): Cfg,
    Json(req): Json<UeberbauungRequest>,
) -> ApiResult<Json<UeberbauungResponse>> {
    authorize(&cedar, &claims, "compute-ausfallarbeit", &cfg.tenant)?;
    if req.trs.is_empty() {
        return Err(ApiError::bad_request("trs is empty"));
    }
    let w_a_gekuerzt_kwh =
        engine::ueberbauung_kuerzung(&req.trs, req.p_anschl_kw, req.einspeisung_kwh)
            .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    Ok(Json(UeberbauungResponse { w_a_gekuerzt_kwh }))
}

// ── Kap. 3.2.3.2 — Wind-Bin-Verfahren (KF_Bin) ──────────────────────────────

/// Per-bin Leistungswerte for one 0,5-m/s wind-speed bin.
///
/// `leistungswerte_kw` must already be filtered to störungsfreier Betrieb,
/// unrestricted feed-in and ≥ 10 % Nennleistung, per Kap. 3.2.3.2. Fewer than
/// [`engine::WIND_BIN_MINDEST_WERTEPAARE`] pairs makes the bin invalid and the
/// Ersatzwert chain below applies.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KfBinRequest {
    /// Wind speed in m/s identifying the bin. Echoed back as `bin_index`.
    pub windgeschwindigkeit_ms: Decimal,
    /// Measured Leistungswerte of the bin in kW.
    #[serde(default)]
    pub leistungswerte_kw: Vec<Decimal>,
    /// Zertifizierte Leistungskennlinie value `P_zertLK` for the bin, in kW.
    pub p_zert_lk: Decimal,
    /// `KF_LBin` of the same bin in the Vormonat, if one was determined.
    #[serde(default)]
    pub kf_lbin_vormonat: Option<Decimal>,
    /// `KF_LBin` of the same bin in the Folgemonat, if one was determined.
    #[serde(default)]
    pub kf_lbin_folgemonat: Option<Decimal>,
    /// Mittelwert of the twelve months before the relevant month, if available.
    #[serde(default)]
    pub kf_lbin_zwoelf_monats_mittel: Option<Decimal>,
    /// Einspeisung at the Messlokation over twelve months, in kWh.
    pub e_einsp_kwh: Decimal,
    /// Sum of the WEA-side Erzeugung over the same twelve months, in kWh.
    pub summe_e_wea_kwh: Decimal,
}

#[derive(Debug, Serialize)]
pub struct KfBinResponse {
    /// Index of the 0,5-m/s bin the wind speed falls into.
    pub bin_index: i64,
    /// Leistungsfaktor of the bin.
    pub kf_lbin: Decimal,
    /// Where `kf_lbin` came from — `monat` when the bin was sufficiently
    /// occupied, otherwise the Ersatzwert step that supplied it.
    pub kf_lbin_quelle: engine::KfLbinQuelle,
    /// Verlustfaktor `KF_V` over twelve months.
    pub kf_v: Decimal,
    /// `KF_Bin = KF_LBin × KF_V` — pass this as `kf` on a `wind_spitz` request.
    pub kf_bin: Decimal,
}

/// `POST /api/v1/redispatch/ausfallarbeit/kf-bin`
///
/// An underoccupied bin is not an error: Kap. 3.2.3.2 prescribes a binding
/// Ersatzwert order (Vormonat → Folgemonat → 12-Monats-Mittel → 1), and the
/// response names which step applied so the operator can evidence it.
///
/// # Errors
///
/// `422` when `KF_V` falls outside `]0;1[`, or when the bin is unusable for a
/// reason the Ersatzwert order does not cover.
pub async fn post_ausfallarbeit_kf_bin(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(cfg): Cfg,
    Json(req): Json<KfBinRequest>,
) -> ApiResult<Json<KfBinResponse>> {
    authorize(&cedar, &claims, "compute-ausfallarbeit", &cfg.tenant)?;
    let unprocessable = |e: engine::AusfallarbeitError| ApiError::unprocessable(e.to_string());
    let kf_v =
        engine::verlustfaktor(req.e_einsp_kwh, req.summe_e_wea_kwh).map_err(unprocessable)?;

    let (kf_lbin, kf_lbin_quelle) = match engine::kf_lbin(&req.leistungswerte_kw, req.p_zert_lk) {
        Ok(v) => (v, engine::KfLbinQuelle::Monat),
        Err(engine::AusfallarbeitError::BinUnterbesetzt(_)) => engine::kf_lbin_ersatzwert(
            req.kf_lbin_vormonat,
            req.kf_lbin_folgemonat,
            req.kf_lbin_zwoelf_monats_mittel,
        ),
        Err(e) => return Err(unprocessable(e)),
    };

    Ok(Json(KfBinResponse {
        bin_index: engine::wind_bin_index(req.windgeschwindigkeit_ms),
        kf_lbin,
        kf_lbin_quelle,
        kf_v,
        kf_bin: engine::kf_bin(kf_lbin, kf_v),
    }))
}

// ── § 24 Abs. 3 S. 2 EEG 2023 — MaLo → TR split ─────────────────────────────

/// One marktlokationsscharfer Wert and the installed capacities to split it by.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaloSplitRequest {
    /// The MaLo-level value to distribute (kWh or kW — the split is linear, so
    /// the unit is whatever the caller supplied).
    pub malo_wert: Decimal,
    /// Installierte Leistung `P_inst,k` per TR in kW, in the caller's order.
    pub p_inst_kw: Vec<Decimal>,
}

#[derive(Debug, Serialize)]
pub struct MaloSplitResponse {
    /// The per-TR share, in the same order as `p_inst_kw`.
    pub anteile: Vec<Decimal>,
}

/// `POST /api/v1/redispatch/ausfallarbeit/malo-split`
///
/// A measurement taken at the Marktlokation has to reach the Technische
/// Ressourcen behind it before any Kap.-3 variant can be applied per TR. The
/// split is pro rata by installed capacity; `Σ P_inst ≤ 0` is refused rather
/// than divided by.
///
/// # Errors
///
/// - `400` when no TechnischeRessource capacity is given.
/// - `422` when the installed capacities sum to zero or less.
pub async fn post_ausfallarbeit_malo_split(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(cfg): Cfg,
    Json(req): Json<MaloSplitRequest>,
) -> ApiResult<Json<MaloSplitResponse>> {
    authorize(&cedar, &claims, "compute-ausfallarbeit", &cfg.tenant)?;
    if req.p_inst_kw.is_empty() {
        return Err(ApiError::bad_request("p_inst_kw is empty"));
    }
    let anteile = engine::malo_wert_auf_tr(req.malo_wert, &req.p_inst_kw)
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    Ok(Json(MaloSplitResponse { anteile }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromPrimitive;

    fn dec(v: f64) -> Decimal {
        Decimal::from_f64(v).expect("finite")
    }

    /// The tenant the test principal and the test configuration share — Cedar
    /// permits nothing across that boundary.
    const TENANT: &str = "9900357000004";

    /// A dev principal for the tenant, as the disabled verifier mints one.
    fn claims() -> Claims {
        Claims(mako_service::oidc::OidcVerifier::disabled(TENANT).disabled_claims())
    }

    /// The service's own policy, so a test exercises the authorization the
    /// router applies rather than a permissive stand-in.
    fn enforcer() -> Authz {
        Extension(Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/netzbilanzd.cedar"
            ))
            .expect("netzbilanzd.cedar parses"),
        ))
    }

    /// The minimum configuration the daemon deserialises, for its tenant alone.
    fn config() -> Cfg {
        Extension(Arc::new(
            serde_json::from_value(serde_json::json!({
                "database": { "url": "postgres://localhost/netzbilanzd" },
                "tenant": TENANT,
                "marktd_url": "http://localhost:9180",
                "marktd_api_key": "test",
                "makod_url": "http://localhost:8080",
                "makod_api_key": "test",
            }))
            .expect("the test configuration parses"),
        ))
    }

    #[test]
    fn wind_spitz_request_computes_series_and_sum() {
        let req: AusfallarbeitComputeRequest = serde_json::from_value(serde_json::json!({
            "verfahren": "wind_spitz",
            "intervalle": [
                {
                    "limitierung": { "fall": "aufforderung", "p_ist": "380", "vorgabe": "400" },
                    "kf": "0.9", "p_theo": "2000", "p_nenn": "3000"
                },
                {
                    "limitierung": { "fall": "duldung", "p_ist": "400" },
                    "kf": "0.9", "p_theo": "2000", "p_bean": "1000", "p_nenn": "3000"
                }
            ]
        }))
        .expect("valid request");
        let series = compute(&req).expect("computes");
        // Interval 1: P_lim = max(380, 400) = 400 → (1800 − 400)/4 = 350.
        // Interval 2: P_lim = 400, min(1800, P_bean 1000) → (1000 − 400)/4 = 150.
        assert_eq!(series, vec![dec(350.0), dec(150.0)]);
    }

    #[test]
    fn solar_spitz_divisor_error_maps_to_message() {
        let req: AusfallarbeitComputeRequest = serde_json::from_value(serde_json::json!({
            "verfahren": "solar_spitz",
            "intervalle": [{
                "limitierung": { "fall": "referenzprofil", "vorgabe": "100" },
                "p_vz_ist": "800", "g_vz": "0", "g_i": "0.5", "p_wr": "1500", "p_nenn": "1400"
            }]
        }))
        .expect("valid request");
        assert!(compute(&req).is_err());
    }

    #[test]
    fn nichtfluktuierend_positiv_yields_mehrarbeit() {
        let req: AusfallarbeitComputeRequest = serde_json::from_value(serde_json::json!({
            "verfahren": "nichtfluktuierend_spitz",
            "richtung": "positiv",
            "intervalle": [{
                "limitierung": { "fall": "aufforderung", "p_ist": "950", "vorgabe": "1000" },
                "p_wert": "400"
            }]
        }))
        .expect("valid request");
        // P_lim = min(950, 1000) = 950 → (400 − 950)/4 = −137.5 (Mehrarbeit).
        assert_eq!(compute(&req).expect("computes"), vec![dec(-137.5)]);
    }

    /// The route reads RFC 3339, returns RFC 3339, and picks the run
    /// Kap. 3.2.2.1 admits rather than the first one offered.
    #[tokio::test]
    async fn vergleichszeitraum_picks_the_nearest_admissible_run() {
        let quarter = |offset: i64, p_ist: &str, gemessen: bool| {
            serde_json::json!({
                "beginn": format!("2026-06-15T{:02}:{:02}:00Z", offset / 60, offset % 60),
                "p_ist_kw": p_ist,
                "p_theo_kw": "1000",
                "vollstaendig_gemessen": gemessen,
                "unbeschraenkt": true,
            })
        };
        // 00:00–01:00 is spoilt by one unmeasured quarter-hour; 04:00–05:00 is
        // clean and therefore the only candidate.
        let mut kandidaten: Vec<_> = (0..4).map(|i| quarter(i * 15, "950", i != 2)).collect();
        kandidaten.extend((16..20).map(|i| quarter(i * 15, "800", true)));

        let req: VergleichszeitraumRequest = serde_json::from_value(serde_json::json!({
            "massnahme_beginn": "2026-06-15T03:00:00Z",
            "massnahme_ende": "2026-06-15T03:00:00Z",
            "p_nenn_kw": "1000",
            "kandidaten": kandidaten,
        }))
        .expect("valid request");

        let Json(body) =
            post_ausfallarbeit_vergleichszeitraum(claims(), enforcer(), config(), Json(req))
                .await
                .expect("an admissible run exists");
        assert_eq!(body.kf, dec(0.8));
        assert_eq!(body.lage, engine::VergleichszeitraumLage::Danach);
        assert_eq!(body.viertelstunden.len(), 4);
        assert_eq!(body.viertelstunden[0], "2026-06-15T04:00:00Z");
    }

    /// Solar's Vergleichszeitraum is a calendar day: the nearest one without a
    /// Maßnahme against the SR, ties to the day before, and only the
    /// quarter-hours over 10 % of the Nennleistung enter the two means.
    #[tokio::test]
    async fn vergleichstag_picks_the_nearest_day_without_a_massnahme() {
        let req: VergleichstagRequest = serde_json::from_value(serde_json::json!({
            "massnahme_tag": "2026-06-14",
            "tage_mit_massnahme": ["2026-06-13"],
            "p_nenn_kw": "1000",
            "kandidaten": [
                // 12 June — admissible, and the tie-break winner at 2 days.
                {"beginn": "2026-06-12T10:00:00Z", "p_ist_kw": "900",
                 "einstrahlung_kw_m2": "0.9", "nichtbeanspruchbar_oder_mba": false},
                {"beginn": "2026-06-12T06:00:00Z", "p_ist_kw": "50",
                 "einstrahlung_kw_m2": "0.1", "nichtbeanspruchbar_oder_mba": false},
                // 13 June — nearer, but a Maßnahme ran against the SR.
                {"beginn": "2026-06-13T10:00:00Z", "p_ist_kw": "800",
                 "einstrahlung_kw_m2": "0.8", "nichtbeanspruchbar_oder_mba": false},
                // 16 June — also 2 days away, loses the tie.
                {"beginn": "2026-06-16T10:00:00Z", "p_ist_kw": "700",
                 "einstrahlung_kw_m2": "0.7", "nichtbeanspruchbar_oder_mba": false},
            ],
        }))
        .expect("valid request");

        let Json(body) =
            post_ausfallarbeit_vergleichstag(claims(), enforcer(), config(), Json(req))
                .await
                .expect("an admissible day exists");
        assert_eq!(body.tag, "2026-06-12");
        assert_eq!(body.lage, engine::VergleichszeitraumLage::Davor);
        assert_eq!(body.viertelstunden, 1, "the 06:00 value is under 10 %");
        assert_eq!(body.p_vz_ist_kw, dec(900.0));
        assert_eq!(body.g_vz_kw_m2, dec(0.9));
    }

    /// No admissible day is a `422`, not a fabricated `P_VZ,ist / G_VZ`.
    #[tokio::test]
    async fn vergleichstag_refuses_when_no_day_qualifies() {
        let req: VergleichstagRequest = serde_json::from_value(serde_json::json!({
            "massnahme_tag": "2026-06-30",
            "p_nenn_kw": "1000",
            "kandidaten": [{
                "beginn": "2026-07-01T10:00:00Z",
                "p_ist_kw": "900",
                "einstrahlung_kw_m2": "0.9",
                "nichtbeanspruchbar_oder_mba": false,
            }],
        }))
        .expect("valid request");
        assert!(
            post_ausfallarbeit_vergleichstag(claims(), enforcer(), config(), Json(req))
                .await
                .is_err()
        );
    }

    /// No admissible run is a `422`, not a fabricated Korrekturfaktor.
    #[tokio::test]
    async fn vergleichszeitraum_refuses_when_nothing_qualifies() {
        let req: VergleichszeitraumRequest = serde_json::from_value(serde_json::json!({
            "massnahme_beginn": "2026-06-15T03:00:00Z",
            "massnahme_ende": "2026-06-15T03:00:00Z",
            "p_nenn_kw": "1000",
            "kandidaten": [{
                "beginn": "2026-06-15T00:00:00Z",
                "p_ist_kw": "950",
                "p_theo_kw": "1000",
                "vollstaendig_gemessen": true,
                "unbeschraenkt": true,
            }],
        }))
        .expect("valid request");
        assert!(
            post_ausfallarbeit_vergleichszeitraum(claims(), enforcer(), config(), Json(req))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn kf_bin_uses_the_measured_bin_when_it_is_occupied() {
        let req: KfBinRequest = serde_json::from_value(serde_json::json!({
            "windgeschwindigkeit_ms": "8.2",
            "leistungswerte_kw": ["900", "1000", "1100"],
            "p_zert_lk": "2000",
            "e_einsp_kwh": "900000",
            "summe_e_wea_kwh": "1000000"
        }))
        .expect("valid request");
        let Json(body) = post_ausfallarbeit_kf_bin(claims(), enforcer(), config(), Json(req))
            .await
            .expect("computes");
        // Bin index = round(8.2 / 0.5) = 16; KF_LBin = 1000/2000 = 0.5;
        // KF_V = 0.9 → KF_Bin = 0.45.
        assert_eq!(body.bin_index, 16);
        assert_eq!(body.kf_lbin_quelle, engine::KfLbinQuelle::Monat);
        assert_eq!(body.kf_bin, dec(0.45));
    }

    /// An underoccupied bin must fall through the binding Ersatzwert order
    /// rather than fail — and say which step supplied the value.
    #[tokio::test]
    async fn kf_bin_falls_through_the_ersatzwert_chain() {
        let req: KfBinRequest = serde_json::from_value(serde_json::json!({
            "windgeschwindigkeit_ms": "3.0",
            "leistungswerte_kw": ["900"],
            "p_zert_lk": "2000",
            "kf_lbin_folgemonat": "0.6",
            "e_einsp_kwh": "900000",
            "summe_e_wea_kwh": "1000000"
        }))
        .expect("valid request");
        let Json(body) = post_ausfallarbeit_kf_bin(claims(), enforcer(), config(), Json(req))
            .await
            .expect("computes");
        assert_eq!(body.kf_lbin_quelle, engine::KfLbinQuelle::Folgemonat);
        assert_eq!(body.kf_bin, dec(0.54));
    }

    /// `KF_V` outside `]0;1[` is a data error, not a silent clamp.
    #[tokio::test]
    async fn kf_bin_rejects_a_verlustfaktor_outside_its_domain() {
        let req: KfBinRequest = serde_json::from_value(serde_json::json!({
            "windgeschwindigkeit_ms": "8.0",
            "leistungswerte_kw": ["900", "1000", "1100"],
            "p_zert_lk": "2000",
            "e_einsp_kwh": "1200000",
            "summe_e_wea_kwh": "1000000"
        }))
        .expect("valid request");
        let err = post_ausfallarbeit_kf_bin(claims(), enforcer(), config(), Json(req))
            .await
            .expect_err("KF_V above 1 is a data error");
        assert_eq!(err.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// The split is pro rata by installed capacity and conserves the total.
    #[tokio::test]
    async fn malo_split_distributes_pro_rata_by_installed_capacity() {
        let req: MaloSplitRequest = serde_json::from_value(serde_json::json!({
            "malo_wert": "1000",
            "p_inst_kw": ["3000", "1000"]
        }))
        .expect("valid request");
        let Json(body) = post_ausfallarbeit_malo_split(claims(), enforcer(), config(), Json(req))
            .await
            .expect("splits");
        assert_eq!(body.anteile, vec![dec(750.0), dec(250.0)]);
        assert_eq!(body.anteile.iter().copied().sum::<Decimal>(), dec(1000.0));
    }

    /// Zero installed capacity is a data error, not a division.
    #[tokio::test]
    async fn malo_split_refuses_zero_installed_capacity() {
        let req: MaloSplitRequest = serde_json::from_value(serde_json::json!({
            "malo_wert": "1000",
            "p_inst_kw": ["0", "0"]
        }))
        .expect("valid request");
        let err = post_ausfallarbeit_malo_split(claims(), enforcer(), config(), Json(req))
            .await
            .expect_err("zero installed capacity cannot be divided by");
        assert_eq!(err.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// A misspelt field is refused, not ignored.
    ///
    /// `p_bean` caps the Ausfallarbeit. Dropping a typo'd one silently removes
    /// the cap and over-reports the curtailed energy — the same failure the
    /// billing request model uses `deny_unknown_fields` to prevent.
    #[test]
    fn a_misspelt_field_is_refused_rather_than_dropped() {
        let with = |key: &str| {
            serde_json::from_value::<AusfallarbeitComputeRequest>(serde_json::json!({
                "verfahren": "wind_spitz",
                "intervalle": [{
                    "limitierung": { "fall": "duldung", "p_ist": "400" },
                    "kf": "0.9", "p_theo": "2000", "p_nenn": "3000", key: "1000"
                }]
            }))
        };
        assert!(with("p_bean").is_ok(), "the real field parses");
        assert!(
            with("p_beam").is_err(),
            "a typo must not silently remove the beanspruchbare-Leistung cap"
        );
    }
}

// ── Kap. 3.2.2.1 — Vergleichszeitraum selection ─────────────────────────────

/// The candidate series and the Maßnahme to place it against.
///
/// Sizing: one calendar month of quarter-hours is well inside the per-request
/// interval cap, and Kap. 3.2.2.1 never reaches beyond the Maßnahme's own month,
/// so a caller has no reason to send more.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VergleichszeitraumRequest {
    /// Start of the Redispatch-Maßnahme — the anchor a run **before** it is
    /// measured to.
    #[serde(with = "time::serde::rfc3339")]
    pub massnahme_beginn: time::OffsetDateTime,
    /// End of the Redispatch-Maßnahme — the anchor a run **after** it is
    /// measured to. Kap. 3.2.2.1 names both („vor oder nach der Viertelstunde,
    /// in der die Redispatch-Maßnahme beginnt bzw. endet"), and using the
    /// beginning for both sides makes a long Maßnahme prefer a Vergleichszeitraum
    /// hours before it over the quarter-hours immediately after.
    #[serde(with = "time::serde::rfc3339")]
    pub massnahme_ende: time::OffsetDateTime,
    /// Nennleistung of the TR in kW — the 10 % admissibility floor is a share
    /// of it.
    pub p_nenn_kw: Decimal,
    /// Candidate quarter-hours, ascending. Runs that are not contiguous at a
    /// quarter-hour spacing are skipped rather than rejected: a series with a
    /// measurement gap is ordinary, and another run may still qualify.
    pub kandidaten: Vec<engine::VergleichsViertelstunde>,
}

#[derive(Debug, Serialize)]
pub struct VergleichszeitraumResponse {
    /// `P_VZ,ist` in kW.
    pub p_vz_ist_kw: Decimal,
    /// `P_VZ,theo` in kW.
    pub p_vz_theo_kw: Decimal,
    /// `KF = P_VZ,ist / P_VZ,theo` — pass this as `kf` on a `wind_spitz`
    /// request.
    pub kf: Decimal,
    /// Which side of the Maßnahme the four quarter-hours came from.
    pub lage: engine::VergleichszeitraumLage,
    /// Their start instants, ascending — the evidence for the selection.
    /// RFC 3339, like every other timestamp on this service's wire.
    pub viertelstunden: Vec<String>,
}

// ── Kap. 3.2.4.1 — Solar-Vergleichstag selection ────────────────────────────

/// The candidate series and the Maßnahme to place a Solar Vergleichstag against.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VergleichstagRequest {
    /// The calendar day the Redispatch-Maßnahme falls on. Distance is counted
    /// in whole days from it.
    pub massnahme_tag: time::Date,
    /// Calendar days on which a Maßnahme was directed at the SR — Kap. 3.2.4.1
    /// admits only a day „an dem keine Redispatch-Maßnahme gegenüber der SR
    /// stattgefunden hat". `massnahme_tag` itself is excluded whether or not it
    /// appears here.
    #[serde(default)]
    pub tage_mit_massnahme: Vec<time::Date>,
    /// Nennleistung of the TR in kW — the 10 % admissibility floor is a share
    /// of it.
    pub p_nenn_kw: Decimal,
    /// Candidate quarter-hours across the candidate days.
    pub kandidaten: Vec<engine::VergleichstagViertelstunde>,
}

#[derive(Debug, Serialize)]
pub struct VergleichstagResponse {
    /// The calendar day selected.
    pub tag: String,
    /// `P_VZ,ist` in kW — pass this on a `solar_spitz` request.
    pub p_vz_ist_kw: Decimal,
    /// `G_VZ` in kW/m² — pass this on a `solar_spitz` request.
    pub g_vz_kw_m2: Decimal,
    /// Which side of the Maßnahme the day lies on.
    pub lage: engine::VergleichszeitraumLage,
    /// How many quarter-hours of that day were admitted — the evidence for the
    /// two means.
    pub viertelstunden: usize,
}

/// `POST /api/v1/redispatch/ausfallarbeit/vergleichstag`
///
/// Solar's Vergleichszeitraum is a **calendar day**, not the wind rule's four
/// quarter-hours (Kap. 3.2.4.1): the nearest day before or after the Maßnahme on
/// which no Maßnahme was directed at the SR, ties to the day before, never from
/// another month — and within it only the quarter-hours that reach 10 % of the
/// Nennleistung and carry no Nichtbeanspruchbarkeit or marktbedingte Anpassung.
/// `P_VZ,ist / G_VZ` scales every kWh of the Spitzabrechnung, so the selection
/// belongs to the engine rather than to each party's spreadsheet.
///
/// # Errors
///
/// - `400` when no candidates are given, or more than the per-request cap.
/// - `422` when no day qualifies, or when the admitted quarter-hours carry no
///   irradiation at all — `G_VZ` would be zero.
pub async fn post_ausfallarbeit_vergleichstag(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(cfg): Cfg,
    Json(req): Json<VergleichstagRequest>,
) -> ApiResult<Json<VergleichstagResponse>> {
    authorize(&cedar, &claims, "compute-ausfallarbeit", &cfg.tenant)?;
    if req.kandidaten.is_empty() {
        return Err(ApiError::bad_request("kandidaten is empty"));
    }
    if req.kandidaten.len() > MAX_INTERVALLE {
        return Err(ApiError::bad_request(format!(
            "kandidaten exceeds {MAX_INTERVALLE} intervals"
        )));
    }
    let tag = engine::solar_vergleichstag(
        &req.kandidaten,
        req.massnahme_tag,
        &req.tage_mit_massnahme,
        req.p_nenn_kw,
    )
    .map_err(|e| ApiError::unprocessable(e.to_string()))?;
    Ok(Json(VergleichstagResponse {
        tag: tag.tag.to_string(),
        p_vz_ist_kw: tag.p_vz_ist_kw,
        g_vz_kw_m2: tag.g_vz_kw_m2,
        lage: tag.lage,
        viertelstunden: tag.viertelstunden,
    }))
}

/// `POST /api/v1/redispatch/ausfallarbeit/vergleichszeitraum`
///
/// The Korrekturfaktor prices every kWh of a Wind-Spitzabrechnung, and which
/// four quarter-hours it is computed from is a rule rather than a convention —
/// contiguous, fully measured, unrestricted, each at least 10 % of the
/// Nennleistung, nearest to the Maßnahme with ties to the side before it, and
/// never from the Folgemonat. „Nearest" is measured to the **beginning** of the
/// Maßnahme on the one side and to its **end** on the other, which is why both
/// instants are required. Deciding it caller-side is how two parties settle the
/// same Maßnahme at different figures.
///
/// # Errors
///
/// - `400` when no candidates are given, or more than the per-request cap.
/// - `422` when no admissible run of four exists on either side — the operator
///   then falls back to the vereinfachte Spitzabrechnung or the Pauschale, which
///   is a decision rather than a computation.
pub async fn post_ausfallarbeit_vergleichszeitraum(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(cfg): Cfg,
    Json(req): Json<VergleichszeitraumRequest>,
) -> ApiResult<Json<VergleichszeitraumResponse>> {
    authorize(&cedar, &claims, "compute-ausfallarbeit", &cfg.tenant)?;
    if req.kandidaten.is_empty() {
        return Err(ApiError::bad_request("kandidaten is empty"));
    }
    if req.kandidaten.len() > MAX_INTERVALLE {
        return Err(ApiError::bad_request(format!(
            "kandidaten exceeds {MAX_INTERVALLE} intervals"
        )));
    }
    let unprocessable = |e: engine::AusfallarbeitError| ApiError::unprocessable(e.to_string());
    if req.massnahme_ende < req.massnahme_beginn {
        return Err(ApiError::bad_request(
            "massnahme_ende is before massnahme_beginn",
        ));
    }
    let zeitraum = engine::vergleichszeitraum(
        &req.kandidaten,
        req.massnahme_beginn,
        req.massnahme_ende,
        req.p_nenn_kw,
    )
    .map_err(unprocessable)?;
    let kf = zeitraum.korrekturfaktor().map_err(unprocessable)?;
    Ok(Json(VergleichszeitraumResponse {
        p_vz_ist_kw: zeitraum.p_vz_ist_kw,
        p_vz_theo_kw: zeitraum.p_vz_theo_kw,
        kf,
        lage: zeitraum.lage,
        viertelstunden: zeitraum
            .viertelstunden
            .iter()
            .map(|v| {
                v.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_else(|_| v.to_string())
            })
            .collect(),
    }))
}
