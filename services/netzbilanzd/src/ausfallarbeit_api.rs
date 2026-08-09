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

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use mako_redispatch::ausfallarbeit as engine;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// Upper bound on intervals per request (~ one year of quarter-hours).
const MAX_INTERVALLE: usize = 35_136;

// ── Request / response types ────────────────────────────────────────────────

/// One quarter-hour of a Wind-Spitzabrechnung (Kap. 3.2.2.1 / 3.2.2.2) — with
/// `kf` = `KF_Bin` this is also the Wind-Bin-Verfahren (Kap. 3.2.3.2).
#[derive(Debug, Deserialize)]
pub struct WindSpitzIntervall {
    /// Kap.-3.1 inputs for `P_lim,i`.
    pub limitierung: engine::Leistungslimitierung,
    /// Korrekturfaktor `KF` (or `KF_Bin = KF_LBin × KF_V`).
    pub kf: Decimal,
    /// Theoretischer Leistungsmittelwert in kW.
    pub p_theo: Decimal,
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    #[serde(default)]
    pub p_bean: Option<Decimal>,
    /// Nennleistung of the TR in kW (plausibility cap).
    pub p_nenn: Decimal,
}

/// One quarter-hour of the Wind Pauschal-Abrechnung (Kap. 3.2.2.3).
#[derive(Debug, Deserialize)]
pub struct WindPauschalIntervall {
    pub limitierung: engine::Leistungslimitierung,
    /// Last fully measured unrestricted quarter-hour before the Maßnahme
    /// (or the Referenzprofilverfahren value), in kW.
    pub p_0: Decimal,
    /// Installierte Leistung of the TR in kW.
    pub p_inst: Decimal,
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    #[serde(default)]
    pub p_bean: Option<Decimal>,
}

/// One quarter-hour of a Solar-Spitzabrechnung (Kap. 3.2.4.1 / 3.2.4.2).
#[derive(Debug, Deserialize)]
pub struct SolarSpitzIntervall {
    pub limitierung: engine::Leistungslimitierung,
    /// Durchschnittliche Ist-Einspeisung im Vergleichszeitraum in kW.
    pub p_vz_ist: Decimal,
    /// Durchschnittliche Einstrahlleistung im Vergleichszeitraum in kW/m².
    pub g_vz: Decimal,
    /// Einstrahlleistung of the Viertelstunde in kW/m².
    pub g_i: Decimal,
    /// Wechselrichterleistung je TR in kW.
    pub p_wr: Decimal,
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    #[serde(default)]
    pub p_bean: Option<Decimal>,
    /// Nennleistung of the TR in kW (plausibility cap).
    pub p_nenn: Decimal,
}

/// One quarter-hour of the Solar Pauschal-Abrechnung (Kap. 3.2.4.3). The
/// Anlagenfaktor comes from the fixed season/time-of-day table (UTC+1).
#[derive(Debug, Deserialize)]
pub struct SolarPauschalIntervall {
    pub limitierung: engine::Leistungslimitierung,
    /// Start of the Viertelstunde (date), UTC+1.
    pub datum: time::Date,
    /// Start of the Viertelstunde (time of day), UTC+1.
    pub zeit: time::Time,
    /// Summe der Nennleistung der Module in kW.
    pub p_inst_module: Decimal,
    /// Wechselrichterleistung je TR in kW.
    pub p_wr: Decimal,
    #[serde(default)]
    pub p_mba: Option<Decimal>,
    #[serde(default)]
    pub p_bean: Option<Decimal>,
}

/// One quarter-hour of a nicht-fluktuierende Abrechnung (Kap. 3.3).
#[derive(Debug, Deserialize)]
pub struct NichtfluktuierendIntervall {
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
#[serde(tag = "verfahren", rename_all = "snake_case")]
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
        richtung: engine::RedispatchRichtung,
        intervalle: Vec<NichtfluktuierendIntervall>,
    },
    /// Kap. 3.3.2 (Prognosemodell default).
    NichtfluktuierendPauschal {
        richtung: engine::RedispatchRichtung,
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
pub async fn post_ausfallarbeit_compute(Json(req): Json<AusfallarbeitComputeRequest>) -> Response {
    let n = interval_count(&req);
    if n == 0 {
        return (StatusCode::BAD_REQUEST, "intervalle is empty").into_response();
    }
    if n > MAX_INTERVALLE {
        return (
            StatusCode::BAD_REQUEST,
            format!("too many intervalle: {n} (max {MAX_INTERVALLE})"),
        )
            .into_response();
    }
    match compute(&req) {
        Ok(w_a_kwh) => {
            let summe_kwh = w_a_kwh.iter().copied().sum();
            Json(AusfallarbeitComputeResponse { w_a_kwh, summe_kwh }).into_response()
        }
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e).into_response(),
    }
}

/// `POST /api/v1/redispatch/ausfallarbeit/ueberbauung`
pub async fn post_ausfallarbeit_ueberbauung(Json(req): Json<UeberbauungRequest>) -> Response {
    if req.trs.is_empty() {
        return (StatusCode::BAD_REQUEST, "trs is empty").into_response();
    }
    match engine::ueberbauung_kuerzung(&req.trs, req.p_anschl_kw, req.einspeisung_kwh) {
        Ok(w_a_gekuerzt_kwh) => Json(UeberbauungResponse { w_a_gekuerzt_kwh }).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

// ── Kap. 3.2.3.2 — Wind-Bin-Verfahren (KF_Bin) ──────────────────────────────

/// Per-bin Leistungswerte for one 0,5-m/s wind-speed bin.
///
/// `leistungswerte_kw` must already be filtered to störungsfreier Betrieb,
/// unrestricted feed-in and ≥ 10 % Nennleistung, per Kap. 3.2.3.2. Fewer than
/// [`engine::WIND_BIN_MINDEST_WERTEPAARE`] pairs makes the bin invalid and the
/// Ersatzwert chain below applies.
#[derive(Debug, Deserialize)]
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
pub async fn post_ausfallarbeit_kf_bin(Json(req): Json<KfBinRequest>) -> Response {
    let kf_v = match engine::verlustfaktor(req.e_einsp_kwh, req.summe_e_wea_kwh) {
        Ok(v) => v,
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    };

    let (kf_lbin, kf_lbin_quelle) = match engine::kf_lbin(&req.leistungswerte_kw, req.p_zert_lk) {
        Ok(v) => (v, engine::KfLbinQuelle::Monat),
        Err(engine::AusfallarbeitError::BinUnterbesetzt(_)) => engine::kf_lbin_ersatzwert(
            req.kf_lbin_vormonat,
            req.kf_lbin_folgemonat,
            req.kf_lbin_zwoelf_monats_mittel,
        ),
        Err(e) => return (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    };

    Json(KfBinResponse {
        bin_index: engine::wind_bin_index(req.windgeschwindigkeit_ms),
        kf_lbin,
        kf_lbin_quelle,
        kf_v,
        kf_bin: engine::kf_bin(kf_lbin, kf_v),
    })
    .into_response()
}

// ── § 24 Abs. 3 S. 2 EEG 2023 — MaLo → TR split ─────────────────────────────

/// One marktlokationsscharfer Wert and the installed capacities to split it by.
#[derive(Debug, Deserialize)]
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
pub async fn post_ausfallarbeit_malo_split(Json(req): Json<MaloSplitRequest>) -> Response {
    if req.p_inst_kw.is_empty() {
        return (StatusCode::BAD_REQUEST, "p_inst_kw is empty").into_response();
    }
    match engine::malo_wert_auf_tr(req.malo_wert, &req.p_inst_kw) {
        Ok(anteile) => Json(MaloSplitResponse { anteile }).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromPrimitive;

    fn dec(v: f64) -> Decimal {
        Decimal::from_f64(v).expect("finite")
    }

    /// Read a handler `Response` back as JSON.
    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    /// Decimals serialise as strings; compare them numerically.
    fn dec_field(v: &serde_json::Value, key: &str) -> Decimal {
        v[key]
            .as_str()
            .expect("decimal string")
            .parse()
            .expect("decimal")
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
        let body = body_json(post_ausfallarbeit_kf_bin(Json(req)).await).await;
        // Bin index = round(8.2 / 0.5) = 16; KF_LBin = 1000/2000 = 0.5;
        // KF_V = 0.9 → KF_Bin = 0.45.
        assert_eq!(body["bin_index"], serde_json::json!(16));
        assert_eq!(body["kf_lbin_quelle"], serde_json::json!("monat"));
        assert_eq!(dec_field(&body, "kf_bin"), dec(0.45));
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
        let body = body_json(post_ausfallarbeit_kf_bin(Json(req)).await).await;
        assert_eq!(body["kf_lbin_quelle"], serde_json::json!("folgemonat"));
        assert_eq!(dec_field(&body, "kf_bin"), dec(0.54));
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
        let resp = post_ausfallarbeit_kf_bin(Json(req)).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    /// The split is pro rata by installed capacity and conserves the total.
    #[tokio::test]
    async fn malo_split_distributes_pro_rata_by_installed_capacity() {
        let req: MaloSplitRequest = serde_json::from_value(serde_json::json!({
            "malo_wert": "1000",
            "p_inst_kw": ["3000", "1000"]
        }))
        .expect("valid request");
        let body = body_json(post_ausfallarbeit_malo_split(Json(req)).await).await;
        let anteile: Vec<Decimal> = body["anteile"]
            .as_array()
            .expect("array")
            .iter()
            .map(|v| {
                v.as_str()
                    .expect("decimal string")
                    .parse()
                    .expect("decimal")
            })
            .collect();
        assert_eq!(anteile, vec![dec(750.0), dec(250.0)]);
        assert_eq!(anteile.iter().copied().sum::<Decimal>(), dec(1000.0));
    }

    /// Zero installed capacity is a data error, not a division.
    #[tokio::test]
    async fn malo_split_refuses_zero_installed_capacity() {
        let req: MaloSplitRequest = serde_json::from_value(serde_json::json!({
            "malo_wert": "1000",
            "p_inst_kw": ["0", "0"]
        }))
        .expect("valid request");
        let resp = post_ausfallarbeit_malo_split(Json(req)).await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}
