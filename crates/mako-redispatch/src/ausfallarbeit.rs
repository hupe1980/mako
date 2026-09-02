//! Ausfallarbeit engine — `BilAReM` Kap. 3 (Anlage zur Festlegung BK6-23-241,
//! Beschluss vom 07.05.2026).
//!
//! Implements the binding final text (not the second-consultation draft, which
//! differs in several places: the Duldungsfall rule `P_lim = P_ist` was
//! restored, the plausibility cap is per-TR Nennleistung, the Solar formulas
//! carry `P_WR`, Wind-Pauschal carries `P_inst`, the nicht-fluktuierende
//! Pauschal-Abrechnung dropped `P_mbA`, and the Überbauung cap subtracts the
//! Einspeisung über die Netzlokation).
//!
//! All Leistungswerte are Viertelstundenmittelwerte in kW; every `W_A` result is
//! the Ausfallarbeit of one Viertelstunde in kWh (`kW × ¼ h`). Sign convention
//! (Kap. 3): negative Redispatch → Ausfallarbeit ≥ 0; positive Redispatch →
//! Ausfallarbeit ≤ 0 (Mehrarbeit).
//!
//! The engine is pure: callers supply the measured/derived quarter-hour values
//! (`P_ist`, `P_theo`, wind speeds, irradiation, Ex-ante-Planungsdaten) — the
//! sourcing of those series (SCADA, edmd Lastgang, DWD, Referenzanlage) is a
//! service concern. Elections/admissibility of the Abrechnungsvarianten live
//! in [`crate::bilarem`].

use rust_decimal::Decimal;
use rust_decimal::prelude::Zero;
use time::{Date, Month, Time};

// ── Frist constants (BilAReM Kap. 3.2.1) ─────────────────────────────────────

/// Wetterdaten/Referenzmessdaten for Spitz-/vereinfachte Spitzabrechnung are
/// due by the end of this Werktag of the following month; afterwards the ANB
/// builds Ersatzwerte.
pub const WETTERDATEN_LIEFERFRIST_WERKTAGE: u8 = 4;

/// A TR leaving the (metering-driven) Pauschal-Abrechnung switches with this
/// notice, effective at the end of the next 31.12., into the vereinfachte
/// Spitzabrechnung (unless Spitzabrechnung was elected by 30.11.).
pub const PAUSCHAL_WECHSEL_FRIST_MONATE: u8 = 3;

/// Quarter-hours count towards the Vergleichszeitraum only if the
/// Leistungsmittelwert is at least this share of the TR Nennleistung
/// (Kap. 3.2.2.1 / 3.2.4.1: "mindestens 10 %").
pub const VERGLEICHSZEITRAUM_MINDESTANTEIL: Decimal = Decimal::from_parts(1, 0, 0, false, 1); // 0.1

/// Length of the Wind-KF Vergleichszeitraum: the nearest four fully measured,
/// contiguous quarter-hours before or after the Maßnahme (ties → before;
/// Folgemonat quarter-hours are never used).
pub const VERGLEICHSZEITRAUM_VIERTELSTUNDEN: usize = 4;

/// Minimum Wertepaare per Wind-Bin for a valid monthly Leistungsfaktor
/// (Kap. 3.2.3.2: `m ≥ 3`; a bin also needs ≥ 30 minutes of valid data).
pub const WIND_BIN_MINDEST_WERTEPAARE: usize = 3;

/// Wind-Bin width in m/s (DIN EN 61400-12-1 method).
pub const WIND_BIN_BREITE_MS: Decimal = Decimal::from_parts(5, 0, 0, false, 1); // 0.5

const QUARTER_HOUR: Decimal = Decimal::from_parts(25, 0, 0, false, 2); // 0.25

// ── Errors ───────────────────────────────────────────────────────────────────

/// Errors from the Ausfallarbeit computation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AusfallarbeitError {
    /// A divisor (`P_VZ,theo`, `G_VZ`, `ΣE_WEA`, `ΣP_inst`) was zero or negative.
    #[error("unzulässiger Divisor: {0}")]
    UnzulaessigerDivisor(&'static str),
    /// The Verlustfaktor `KF_V` left its domain ]0;1[ (Kap. 3.2.3.2).
    #[error("Verlustfaktor {0} liegt nicht in ]0;1[")]
    VerlustfaktorAusserhalb(Decimal),
    /// Too few Wertepaare for a valid Wind-Bin Leistungsfaktor (m ≥ 3).
    #[error("Wind-Bin unterbesetzt: {0} Wertepaare (< {WIND_BIN_MINDEST_WERTEPAARE})")]
    BinUnterbesetzt(usize),
    /// A required input was negative where the Festlegung admits none.
    #[error("negativer Eingabewert: {0}")]
    NegativerWert(&'static str),
}

// ── Kap. 3.1 — Wert der Leistungslimitierung ────────────────────────────────

/// Direction of the Redispatch-Maßnahme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedispatchRichtung {
    /// Erzeugung erhöhen / Bezug senken — Ausfallarbeit ≤ 0 (Mehrarbeit).
    Positiv,
    /// Erzeugung senken / Bezug erhöhen — Ausfallarbeit ≥ 0.
    Negativ,
}

/// How the Wert der Leistungslimitierung `P_lim,i` of one Viertelstunde is
/// determined (Kap. 3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "fall")]
pub enum Leistungslimitierung {
    /// Aufforderungsfall: the anweisende NB requests the EIV to adapt;
    /// `vorgabe` is `P_min` (positiver Redispatch) or `P_max` (negativer).
    Aufforderung {
        /// Tatsächlicher Leistungsmittelwert `P_ist,i` in kW.
        p_ist: Decimal,
        /// NB-Vorgabe aus der Redispatch-Abrufinformation in kW.
        vorgabe: Decimal,
    },
    /// Duldungsfall: the anweisende NB steers the SR itself → `P_lim = P_ist`.
    Duldung {
        /// Tatsächlicher Leistungsmittelwert `P_ist,i` in kW.
        p_ist: Decimal,
    },
    /// Referenzprofilverfahren (and Redispatch-Maßnahme mit beidseitiger
    /// Fixierung): `P_lim` equals the NB-Vorgabe outright.
    Referenzprofil {
        /// NB-Vorgabe (`P_min` bzw. `P_max`) in kW.
        vorgabe: Decimal,
    },
}

impl Leistungslimitierung {
    /// The Wert der Leistungslimitierung `P_lim,i` in kW.
    ///
    /// Aufforderungsfall: positiver Redispatch → `min{P_ist; P_min}`,
    /// negativer → `max{P_ist; P_max}`. Duldungsfall: `P_ist`.
    /// Referenzprofil/beidseitige Fixierung: the Vorgabe.
    #[must_use]
    pub fn wert(self, richtung: RedispatchRichtung) -> Decimal {
        match self {
            Self::Aufforderung { p_ist, vorgabe } => match richtung {
                RedispatchRichtung::Positiv => p_ist.min(vorgabe),
                RedispatchRichtung::Negativ => p_ist.max(vorgabe),
            },
            Self::Duldung { p_ist } => p_ist,
            Self::Referenzprofil { vorgabe } => vorgabe,
        }
    }
}

// ── Shared helpers ───────────────────────────────────────────────────────────

/// Splits a marktlokationsscharfer Wert onto the TR behind the `MaLo` pro rata
/// by installed capacity (Kap. 3 i. V. m. § 24 Abs. 3 S. 2 EEG 2023).
///
/// # Errors
///
/// [`AusfallarbeitError::UnzulaessigerDivisor`] if `Σ P_inst ≤ 0`.
pub fn malo_wert_auf_tr(
    malo_wert: Decimal,
    p_inst_kw: &[Decimal],
) -> Result<Vec<Decimal>, AusfallarbeitError> {
    let summe: Decimal = p_inst_kw.iter().copied().sum();
    if summe <= Decimal::zero() {
        return Err(AusfallarbeitError::UnzulaessigerDivisor("Σ P_inst"));
    }
    Ok(p_inst_kw.iter().map(|p| malo_wert * p / summe).collect())
}

/// `min` over the theoretical value and the optional `P_mbA` / `P_bean` / `P_WR`
/// bounds, then `(… − P_lim) × ¼ h`, clamped by direction.
fn w_a(theo: Decimal, bounds: &[Option<Decimal>], p_lim: Decimal, negativ: bool) -> Decimal {
    let mut m = theo;
    for b in bounds.iter().copied().flatten() {
        m = m.min(b);
    }
    let w = (m - p_lim) * QUARTER_HOUR;
    if negativ {
        w.max(Decimal::zero())
    } else {
        w.min(Decimal::zero())
    }
}

// ── Kap. 3.2.2 — Windenergieanlagen (Spitzabrechnung) ───────────────────────

/// Korrekturfaktor `KF = P_VZ,ist / P_VZ,theo` (Kap. 3.2.2.1).
///
/// `P_VZ,ist`: measured mean over the nearest four fully measured contiguous
/// quarter-hours before or after the Maßnahme (unrestricted feed-in, ≥ 10 %
/// Nennleistung); `P_VZ,theo`: the theoretical mean over the same
/// quarter-hours from the zertifizierte Leistungskennlinie.
///
/// # Errors
///
/// [`AusfallarbeitError::UnzulaessigerDivisor`] if `P_VZ,theo ≤ 0`.
pub fn korrekturfaktor(
    p_vz_ist: Decimal,
    p_vz_theo: Decimal,
) -> Result<Decimal, AusfallarbeitError> {
    if p_vz_theo <= Decimal::zero() {
        return Err(AusfallarbeitError::UnzulaessigerDivisor("P_VZ,theo"));
    }
    Ok(p_vz_ist / p_vz_theo)
}

/// One Viertelstunde of a Wind-Spitzabrechnung (Kap. 3.2.2.1) or — with
/// `kf = KF_Bin` — of the Wind-Bin-Verfahren (Kap. 3.2.3.2). The vereinfachte
/// Spitzabrechnung (Kap. 3.2.2.2) uses the same formula with wind speeds from
/// a meteorological provider or Referenzanlage.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct WindSpitzInput {
    /// Korrekturfaktor KF (or `KF_Bin`).
    pub kf: Decimal,
    /// Theoretischer Leistungsmittelwert `P_theo,i` of the Viertelstunde in kW
    /// (from wind speed × zertifizierte Leistungskennlinie).
    pub p_theo: Decimal,
    /// Marktbedingte Anpassung `P_mbA,i` in kW (None if none applies).
    pub p_mba: Option<Decimal>,
    /// Beanspruchbare Leistung `P_bean,i` in kW (installed − Nichtbeanspruch-
    /// barkeit; None if none applies).
    pub p_bean: Option<Decimal>,
    /// Wert der Leistungslimitierung `P_lim,i` in kW.
    pub p_lim: Decimal,
    /// Nennleistung of the TR in kW — plausibility cap for `KF × P_theo`.
    pub p_nenn: Decimal,
}

/// `W_A,i = max{0; (min(KF·P_theo,i; P_mbA,i; P_bean,i) − P_lim,i) × ¼ h}`
/// with `KF·P_theo,i` capped at the Nennleistung of the TR. Result in kWh.
///
/// Only defined for negativen Redispatch (Kap. 3.2 applies to fluktuierende
/// Erzeugung under negative measures only).
#[must_use]
pub fn wind_spitz(input: &WindSpitzInput) -> Decimal {
    let theo = (input.kf * input.p_theo).min(input.p_nenn);
    w_a(theo, &[input.p_mba, input.p_bean], input.p_lim, true)
}

/// `W_A,i = max{0; [min(P_0; P_inst; P_mbA,i; P_bean,i) − P_lim,i] × ¼ h}` —
/// Wind Pauschal-Abrechnung (Kap. 3.2.2.3), grandfathered TR only. `p_0` is
/// the last fully measured unrestricted quarter-hour before the Maßnahme (or
/// the Referenzprofilverfahren value if no ¼-h-Messung exists).
#[must_use]
pub fn wind_pauschal(
    p_0: Decimal,
    p_inst: Decimal,
    p_mba: Option<Decimal>,
    p_bean: Option<Decimal>,
    p_lim: Decimal,
) -> Decimal {
    w_a(p_0.min(p_inst), &[p_mba, p_bean], p_lim, true)
}

// ── Kap. 3.2.3.2 — Wind-Bin-Verfahren (Windenergieanlagen auf See) ──────────

/// Index of the 0,5-m/s-Bin a wind speed falls into (bins centred on
/// multiples of 0,5 m/s per DIN EN 61400-12-1).
#[must_use]
pub fn wind_bin_index(windgeschwindigkeit_ms: Decimal) -> i64 {
    // A speed exactly on a bin boundary goes to the outer bin — `Decimal::round`
    // would send it to the even one, which is the workspace's banker's-rounding
    // trap in a place that is not money.
    let idx = (windgeschwindigkeit_ms / WIND_BIN_BREITE_MS).round_dp_with_strategy(0, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    idx.try_into().unwrap_or(i64::MAX)
}

/// Monthly Leistungsfaktor `KF_LBin = P̄_Bin / P_zertLK` of one bin, with
/// `P̄_Bin = Σ P / m` (m ≥ 3) and `KF_LBin ≥ 0`. Wertepaare must already be
/// filtered to störungsfreier Betrieb, unrestricted feed-in and ≥ 10 %
/// Nennleistung; valid for the corresponding month of the next two Folgejahre.
///
/// # Errors
///
/// [`AusfallarbeitError::BinUnterbesetzt`] for `m < 3`;
/// [`AusfallarbeitError::UnzulaessigerDivisor`] if `P_zertLK ≤ 0`.
pub fn kf_lbin(
    leistungswerte_kw: &[Decimal],
    p_zert_lk: Decimal,
) -> Result<Decimal, AusfallarbeitError> {
    let m = leistungswerte_kw.len();
    if m < WIND_BIN_MINDEST_WERTEPAARE {
        return Err(AusfallarbeitError::BinUnterbesetzt(m));
    }
    if p_zert_lk <= Decimal::zero() {
        return Err(AusfallarbeitError::UnzulaessigerDivisor("P_zertLK"));
    }
    let mittel: Decimal = leistungswerte_kw.iter().copied().sum::<Decimal>()
        / Decimal::from(u64::try_from(m).unwrap_or(u64::MAX));
    Ok((mittel / p_zert_lk).max(Decimal::zero()))
}

/// Where a bin's Leistungsfaktor came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KfLbinQuelle {
    /// Regularly determined for the relevant month.
    Monat,
    /// Ersatzwert from the Vormonat.
    Vormonat,
    /// Ersatzwert from the Folgemonat.
    Folgemonat,
    /// Mittelwert of the twelve months before the relevant month.
    ZwoelfMonatsMittel,
    /// No sufficient Wertepaare anywhere → `KF_LBin = 1` (also used outside
    /// the Leistungskennlinie range, below cut-in / above cut-out).
    Standard,
}

/// Resolves the Ersatzwert chain for an invalid bin (Kap. 3.2.3.2, order
/// binding): Vormonat → Folgemonat → 12-Monats-Mittel → `KF_LBin = 1`.
#[must_use]
pub fn kf_lbin_ersatzwert(
    vormonat: Option<Decimal>,
    folgemonat: Option<Decimal>,
    zwoelf_monats_mittel: Option<Decimal>,
) -> (Decimal, KfLbinQuelle) {
    if let Some(v) = vormonat {
        (v, KfLbinQuelle::Vormonat)
    } else if let Some(v) = folgemonat {
        (v, KfLbinQuelle::Folgemonat)
    } else if let Some(v) = zwoelf_monats_mittel {
        (v, KfLbinQuelle::ZwoelfMonatsMittel)
    } else {
        (Decimal::ONE, KfLbinQuelle::Standard)
    }
}

/// Verlustfaktor `KF_V = E_Einsp / Σ E_WEA` over twelve months — parkinterne
/// Verluste, per Messlokation, essentially constant over the park lifetime.
///
/// # Errors
///
/// [`AusfallarbeitError::UnzulaessigerDivisor`] if `Σ E_WEA ≤ 0`;
/// [`AusfallarbeitError::VerlustfaktorAusserhalb`] if the ratio leaves the
/// binding domain `]0;1[` (which also enforces `E_Einsp ≤ Σ E_WEA`).
pub fn verlustfaktor(
    e_einsp_kwh: Decimal,
    summe_e_wea_kwh: Decimal,
) -> Result<Decimal, AusfallarbeitError> {
    if summe_e_wea_kwh <= Decimal::zero() {
        return Err(AusfallarbeitError::UnzulaessigerDivisor("Σ E_WEA"));
    }
    let kf_v = e_einsp_kwh / summe_e_wea_kwh;
    if kf_v <= Decimal::zero() || kf_v >= Decimal::ONE {
        return Err(AusfallarbeitError::VerlustfaktorAusserhalb(kf_v));
    }
    Ok(kf_v)
}

/// `KF_Bin = KF_LBin × KF_V` (Kap. 3.2.3.2). Feed into [`wind_spitz`] as `kf`.
#[must_use]
pub fn kf_bin(kf_lbin: Decimal, kf_v: Decimal) -> Decimal {
    kf_lbin * kf_v
}

// ── Kap. 3.2.4 — Solaranlagen ───────────────────────────────────────────────

/// One Viertelstunde of a Solar-Spitzabrechnung (Kap. 3.2.4.1); the
/// vereinfachte Spitzabrechnung (Kap. 3.2.4.2) uses the same formula with
/// Einstrahlwerte from a meteorological provider (Heliosat-2 qualifies).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct SolarSpitzInput {
    /// Durchschnittliche Ist-Einspeisung `P_VZ,ist` im Vergleichszeitraum in kW.
    pub p_vz_ist: Decimal,
    /// Durchschnittliche Einstrahlleistung `G_VZ` im Vergleichszeitraum in kW/m².
    pub g_vz: Decimal,
    /// Durchschnittliche Einstrahlleistung `G_i` of the Viertelstunde in kW/m².
    pub g_i: Decimal,
    /// Wechselrichterleistung `P_WR` je TR in kW (split pro rata by installed
    /// capacity when several TR share one Wechselrichter).
    pub p_wr: Decimal,
    /// Marktbedingte Anpassung `P_mbA,i` in kW (None if none applies).
    pub p_mba: Option<Decimal>,
    /// Beanspruchbare Leistung `P_bean,i` in kW (None if none applies).
    pub p_bean: Option<Decimal>,
    /// Wert der Leistungslimitierung `P_lim,i` in kW.
    pub p_lim: Decimal,
    /// Nennleistung of the TR in kW — plausibility cap for
    /// `P_VZ,ist / G_VZ × G_i`.
    pub p_nenn: Decimal,
}

/// `W_A,i = max{0; (min(P_VZ,ist/G_VZ × G_i; P_WR; P_mbA,i; P_bean,i)
/// − P_lim,i) × ¼ h}` with the irradiation-scaled term capped at the
/// Nennleistung of the TR. Result in kWh.
///
/// # Errors
///
/// [`AusfallarbeitError::UnzulaessigerDivisor`] if `G_VZ ≤ 0`.
pub fn solar_spitz(input: &SolarSpitzInput) -> Result<Decimal, AusfallarbeitError> {
    if input.g_vz <= Decimal::zero() {
        return Err(AusfallarbeitError::UnzulaessigerDivisor("G_VZ"));
    }
    let theo = (input.p_vz_ist / input.g_vz * input.g_i).min(input.p_nenn);
    Ok(w_a(
        theo,
        &[Some(input.p_wr), input.p_mba, input.p_bean],
        input.p_lim,
        true,
    ))
}

/// Anlagenfaktor AF for the Solar Pauschal-Abrechnung (Kap. 3.2.4.3).
///
/// `zeit` is the start of the Viertelstunde in **UTC+1** (the table is fixed
/// to UTC+1 — no DST switch). Sommer = 01.03.–31.10., Winter = 01.11.–28./29.02.
#[must_use]
pub fn anlagenfaktor(datum: Date, zeit: Time) -> Decimal {
    let sommer = matches!(
        datum.month(),
        Month::March
            | Month::April
            | Month::May
            | Month::June
            | Month::July
            | Month::August
            | Month::September
            | Month::October
    );
    let minuten = i32::from(zeit.hour()) * 60 + i32::from(zeit.minute());
    let af = |zehntausendstel: i64| Decimal::new(zehntausendstel, 4);
    if sommer {
        match minuten {
            m if (360..540).contains(&m) => af(2456),  // 06:00–09:00
            m if (540..900).contains(&m) => af(6189),  // 09:00–15:00
            m if (900..1140).contains(&m) => af(2456), // 15:00–19:00
            _ => Decimal::zero(),                      // 19:00–06:00
        }
    } else {
        match minuten {
            m if (540..600).contains(&m) => af(2796),  // 09:00–10:00
            m if (600..840).contains(&m) => af(5030),  // 10:00–14:00
            m if (840..1005).contains(&m) => af(2796), // 14:00–16:45
            _ => Decimal::zero(),                      // 16:45–09:00
        }
    }
}

/// `W_A,i = max{0; [min(AF × P_inst; P_WR; P_mbA,i; P_bean,i) − P_lim,i]
/// × ¼ h}` — Solar Pauschal-Abrechnung (Kap. 3.2.4.3), grandfathered TR only.
/// `p_inst` is the Summe der Nennleistung der Module in kW.
#[must_use]
pub fn solar_pauschal(
    af: Decimal,
    p_inst_module: Decimal,
    p_wr: Decimal,
    p_mba: Option<Decimal>,
    p_bean: Option<Decimal>,
    p_lim: Decimal,
) -> Decimal {
    w_a(
        af * p_inst_module,
        &[Some(p_wr), p_mba, p_bean],
        p_lim,
        true,
    )
}

// ── Kap. 3.3 — Anlagen mit nicht-fluktuierender Erzeugung ───────────────────

/// Spitzabrechnung (Kap. 3.3.1): `W_A,i` from the geplante Fahrweise
/// (Ex-ante-Planungsdaten). Positiver Redispatch → `min{0; (P_plan − P_lim)
/// × ¼ h}` (Mehrarbeit ≤ 0); negativer → `max{0; (P_plan − P_lim) × ¼ h}`.
/// TR im Planwertmodell are always settled this way.
#[must_use]
pub fn nichtfluktuierend_spitz(
    richtung: RedispatchRichtung,
    p_plan: Decimal,
    p_lim: Decimal,
) -> Decimal {
    let w = (p_plan - p_lim) * QUARTER_HOUR;
    match richtung {
        RedispatchRichtung::Positiv => w.min(Decimal::zero()),
        RedispatchRichtung::Negativ => w.max(Decimal::zero()),
    }
}

/// Pauschal-Abrechnung (Kap. 3.3.2): Fortschreibung of the last fully
/// measured quarter-hour `P_0`. Positiver Redispatch →
/// `min{0; (P_0 − min(P_lim; P_bean)) × ¼ h}`; negativer →
/// `max{0; (min(P_0; P_bean) − P_lim) × ¼ h}`. TR im Prognosemodell default
/// here (Spitz on request with correct Ex-ante-Planungsdaten).
#[must_use]
pub fn nichtfluktuierend_pauschal(
    richtung: RedispatchRichtung,
    p_0: Decimal,
    p_bean: Option<Decimal>,
    p_lim: Decimal,
) -> Decimal {
    match richtung {
        RedispatchRichtung::Positiv => {
            let grenze = p_bean.map_or(p_lim, |b| p_lim.min(b));
            ((p_0 - grenze) * QUARTER_HOUR).min(Decimal::zero())
        }
        RedispatchRichtung::Negativ => {
            let basis = p_bean.map_or(p_0, |b| p_0.min(b));
            ((basis - p_lim) * QUARTER_HOUR).max(Decimal::zero())
        }
    }
}

// ── Kap. 3.4 — Überbauung von Anschlüssen ───────────────────────────────────

/// One TR's contribution to the Überbauung check of a Netzlokation.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct UeberbauungTr {
    /// Ausfallarbeit `W_A,i,k` of the TR per Kap. 3.2/3.3 in kWh.
    pub w_a_kwh: Decimal,
    /// Installierte Leistung `P_inst,k` of the TR in kW.
    pub p_inst_kw: Decimal,
}

/// Caps the summed Ausfallarbeit of all TR behind one Netzlokation at
/// `P_anschl × ¼ h − Einspeisung über die Netzlokation` (Kap. 3.4) and
/// distributes the Kürzung pro rata by installed capacity (the "jedenfalls
/// sachgerecht" default), clamping each TR at zero and redistributing the
/// remainder among the others.
///
/// Returns the gekürzte Ausfallarbeit per TR, same order as `trs`.
///
/// # Errors
///
/// [`AusfallarbeitError::NegativerWert`] if `p_anschl_kw` is negative;
/// [`AusfallarbeitError::UnzulaessigerDivisor`] if a Kürzung is required but
/// `Σ P_inst ≤ 0`.
pub fn ueberbauung_kuerzung(
    trs: &[UeberbauungTr],
    p_anschl_kw: Decimal,
    einspeisung_netzlokation_kwh: Decimal,
) -> Result<Vec<Decimal>, AusfallarbeitError> {
    if p_anschl_kw < Decimal::zero() {
        return Err(AusfallarbeitError::NegativerWert("P_anschl"));
    }
    let cap = (p_anschl_kw * QUARTER_HOUR - einspeisung_netzlokation_kwh).max(Decimal::zero());
    let summe: Decimal = trs.iter().map(|t| t.w_a_kwh).sum();
    if summe <= cap {
        return Ok(trs.iter().map(|t| t.w_a_kwh).collect());
    }
    // Pro-rata Kürzung by installed capacity with clamp-at-zero: a TR whose
    // gekürzte Ausfallarbeit would turn negative is set to 0 and excluded from
    // the remaining distribution (iterated until stable).
    let mut werte: Vec<Decimal> = trs.iter().map(|t| t.w_a_kwh).collect();
    let mut aktiv: Vec<bool> = trs.iter().map(|t| t.w_a_kwh > Decimal::zero()).collect();
    loop {
        let ueberschuss: Decimal = werte
            .iter()
            .zip(&aktiv)
            .map(|(w, a)| if *a { *w } else { Decimal::zero() })
            .sum::<Decimal>()
            - cap;
        if ueberschuss <= Decimal::zero() {
            break;
        }
        let p_inst_summe: Decimal = trs
            .iter()
            .zip(&aktiv)
            .filter(|(_, a)| **a)
            .map(|(t, _)| t.p_inst_kw)
            .sum();
        if p_inst_summe <= Decimal::zero() {
            return Err(AusfallarbeitError::UnzulaessigerDivisor("Σ P_inst"));
        }
        let mut geclampt = false;
        for (idx, tr) in trs.iter().enumerate() {
            if !aktiv[idx] {
                continue;
            }
            let anteil = ueberschuss * tr.p_inst_kw / p_inst_summe;
            let neu = werte[idx] - anteil;
            if neu < Decimal::zero() {
                werte[idx] = Decimal::zero();
                aktiv[idx] = false;
                geclampt = true;
            } else {
                werte[idx] = neu;
            }
        }
        if !geclampt {
            break;
        }
        // A clamp shifted burden — recompute against the survivors.
    }
    for (idx, w) in werte.iter_mut().enumerate() {
        if !aktiv[idx] && trs[idx].w_a_kwh <= Decimal::zero() {
            *w = trs[idx].w_a_kwh; // non-positive entries pass through untouched
        }
    }
    Ok(werte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::FromPrimitive;
    use time::macros::{date, time};

    fn dec(v: f64) -> Decimal {
        Decimal::from_f64(v).expect("finite")
    }

    // ── Kap. 3.1 ─────────────────────────────────────────────────────────

    #[test]
    fn leistungslimitierung_aufforderungsfall() {
        // Positiver Redispatch: min{P_ist; P_min}.
        let l = Leistungslimitierung::Aufforderung {
            p_ist: dec(800.0),
            vorgabe: dec(1000.0),
        };
        assert_eq!(l.wert(RedispatchRichtung::Positiv), dec(800.0));
        // Negativer Redispatch: max{P_ist; P_max}.
        let l = Leistungslimitierung::Aufforderung {
            p_ist: dec(300.0),
            vorgabe: dec(500.0),
        };
        assert_eq!(l.wert(RedispatchRichtung::Negativ), dec(500.0));
    }

    #[test]
    fn leistungslimitierung_duldung_und_referenzprofil() {
        let d = Leistungslimitierung::Duldung { p_ist: dec(420.0) };
        assert_eq!(d.wert(RedispatchRichtung::Positiv), dec(420.0));
        assert_eq!(d.wert(RedispatchRichtung::Negativ), dec(420.0));
        let r = Leistungslimitierung::Referenzprofil {
            vorgabe: dec(500.0),
        };
        assert_eq!(r.wert(RedispatchRichtung::Negativ), dec(500.0));
    }

    // ── Wind Spitz / KF ──────────────────────────────────────────────────

    #[test]
    fn korrekturfaktor_ratio_and_divisor_guard() {
        assert_eq!(korrekturfaktor(dec(900.0), dec(1000.0)), Ok(dec(0.9)));
        assert_eq!(
            korrekturfaktor(dec(900.0), Decimal::ZERO),
            Err(AusfallarbeitError::UnzulaessigerDivisor("P_VZ,theo"))
        );
    }

    #[test]
    fn wind_spitz_basic_and_nennleistung_cap() {
        // KF·P_theo = 0.9 × 2000 = 1800, P_lim 400 → (1800−400)/4 = 350 kWh.
        let mut input = WindSpitzInput {
            kf: dec(0.9),
            p_theo: dec(2000.0),
            p_mba: None,
            p_bean: None,
            p_lim: dec(400.0),
            p_nenn: dec(3000.0),
        };
        assert_eq!(wind_spitz(&input), dec(350.0));
        // Cap: KF·P_theo > P_nenn → begrenzt auf 3000 → (3000−400)/4 = 650.
        input.kf = dec(1.8);
        assert_eq!(wind_spitz(&input), dec(650.0));
        // P_bean binds below the product.
        input.kf = dec(0.9);
        input.p_bean = Some(dec(1000.0));
        assert_eq!(wind_spitz(&input), dec(150.0));
        // P_lim above everything → keine (negative) Ausfallarbeit, floor 0.
        input.p_lim = dec(2500.0);
        assert_eq!(wind_spitz(&input), Decimal::ZERO);
    }

    #[test]
    fn wind_pauschal_fortschreibung() {
        // min(P_0=1200; P_inst=2000) − P_lim=200 → 1000/4 = 250 kWh.
        assert_eq!(
            wind_pauschal(dec(1200.0), dec(2000.0), None, None, dec(200.0)),
            dec(250.0)
        );
        // P_inst binds: min(2500; 2000) − 200 → 450.
        assert_eq!(
            wind_pauschal(dec(2500.0), dec(2000.0), None, None, dec(200.0)),
            dec(450.0)
        );
    }

    // ── Wind-Bin ─────────────────────────────────────────────────────────

    #[test]
    fn wind_bin_index_centres_on_half_ms() {
        assert_eq!(wind_bin_index(dec(0.0)), 0);
        assert_eq!(wind_bin_index(dec(7.6)), 15); // 7.6/0.5 = 15.2 → bin 15 (7.5 m/s)
        assert_eq!(wind_bin_index(dec(7.74)), 15);
        assert_eq!(wind_bin_index(dec(7.8)), 16);
    }

    #[test]
    fn kf_lbin_requires_three_wertepaare_and_clamps_at_zero() {
        assert_eq!(
            kf_lbin(&[dec(900.0), dec(950.0)], dec(1000.0)),
            Err(AusfallarbeitError::BinUnterbesetzt(2))
        );
        let kf = kf_lbin(&[dec(900.0), dec(950.0), dec(1000.0)], dec(1000.0)).unwrap();
        assert_eq!(kf, dec(0.95));
        // Negative mean (e.g. Eigenverbrauch artefacts) clamps to ≥ 0.
        let kf = kf_lbin(&[dec(-10.0), dec(-20.0), dec(-30.0)], dec(1000.0)).unwrap();
        assert_eq!(kf, Decimal::ZERO);
    }

    #[test]
    fn kf_lbin_ersatzwert_chain_order() {
        assert_eq!(
            kf_lbin_ersatzwert(Some(dec(0.9)), Some(dec(0.8)), Some(dec(0.7))),
            (dec(0.9), KfLbinQuelle::Vormonat)
        );
        assert_eq!(
            kf_lbin_ersatzwert(None, Some(dec(0.8)), Some(dec(0.7))),
            (dec(0.8), KfLbinQuelle::Folgemonat)
        );
        assert_eq!(
            kf_lbin_ersatzwert(None, None, Some(dec(0.7))),
            (dec(0.7), KfLbinQuelle::ZwoelfMonatsMittel)
        );
        assert_eq!(
            kf_lbin_ersatzwert(None, None, None),
            (Decimal::ONE, KfLbinQuelle::Standard)
        );
    }

    #[test]
    fn verlustfaktor_domain() {
        assert_eq!(verlustfaktor(dec(970.0), dec(1000.0)), Ok(dec(0.97)));
        // KF_V = 1 (E_Einsp == ΣE_WEA) is outside ]0;1[.
        assert!(matches!(
            verlustfaktor(dec(1000.0), dec(1000.0)),
            Err(AusfallarbeitError::VerlustfaktorAusserhalb(_))
        ));
        assert!(matches!(
            verlustfaktor(dec(1100.0), dec(1000.0)),
            Err(AusfallarbeitError::VerlustfaktorAusserhalb(_))
        ));
        assert!(matches!(
            verlustfaktor(dec(0.0), dec(1000.0)),
            Err(AusfallarbeitError::VerlustfaktorAusserhalb(_))
        ));
    }

    #[test]
    fn wind_bin_composes_into_spitz_formula() {
        let kf = kf_bin(dec(0.95), dec(0.97));
        let input = WindSpitzInput {
            kf,
            p_theo: dec(1000.0),
            p_mba: None,
            p_bean: None,
            p_lim: dec(121.5),
            p_nenn: dec(5000.0),
        };
        // 0.9215 × 1000 − 121.5 = 800 → 200 kWh.
        assert_eq!(wind_spitz(&input), dec(200.0));
    }

    // ── Solar ────────────────────────────────────────────────────────────

    #[test]
    fn solar_spitz_scales_by_irradiation() {
        // P_VZ,ist/G_VZ = 800/0.4 = 2000 kW per kW/m²; G_i = 0.6 → 1200 kW.
        let input = SolarSpitzInput {
            p_vz_ist: dec(800.0),
            g_vz: dec(0.4),
            g_i: dec(0.6),
            p_wr: dec(1500.0),
            p_mba: None,
            p_bean: None,
            p_lim: dec(200.0),
            p_nenn: dec(1400.0),
        };
        // theo = min(1200, 1400) = 1200; min(1200, P_WR 1500) = 1200 → 250 kWh.
        assert_eq!(solar_spitz(&input).unwrap(), dec(250.0));
        // Wechselrichter binds.
        let engpass = SolarSpitzInput {
            p_wr: dec(1000.0),
            ..input
        };
        assert_eq!(solar_spitz(&engpass).unwrap(), dec(200.0));
        // G_VZ = 0 guarded.
        let kaputt = SolarSpitzInput {
            g_vz: Decimal::ZERO,
            ..input
        };
        assert_eq!(
            solar_spitz(&kaputt),
            Err(AusfallarbeitError::UnzulaessigerDivisor("G_VZ"))
        );
    }

    #[test]
    fn anlagenfaktor_table_summer_winter() {
        // Sommer midday.
        assert_eq!(
            anlagenfaktor(date!(2027 - 06 - 15), time!(12:00)),
            dec(0.6189)
        );
        // Sommer morning shoulder, boundary inclusion 06:00.
        assert_eq!(
            anlagenfaktor(date!(2027 - 06 - 15), time!(06:00)),
            dec(0.2456)
        );
        // Sommer boundary 09:00 belongs to the midday band.
        assert_eq!(
            anlagenfaktor(date!(2027 - 06 - 15), time!(09:00)),
            dec(0.6189)
        );
        // Sommer night.
        assert_eq!(
            anlagenfaktor(date!(2027 - 06 - 15), time!(19:00)),
            Decimal::ZERO
        );
        // Winter midday and shoulders.
        assert_eq!(
            anlagenfaktor(date!(2027 - 01 - 15), time!(12:00)),
            dec(0.5030)
        );
        assert_eq!(
            anlagenfaktor(date!(2027 - 01 - 15), time!(09:15)),
            dec(0.2796)
        );
        assert_eq!(
            anlagenfaktor(date!(2027 - 01 - 15), time!(16:30)),
            dec(0.2796)
        );
        assert_eq!(
            anlagenfaktor(date!(2027 - 01 - 15), time!(16:45)),
            Decimal::ZERO
        );
        // Season boundaries: 01.03. is Sommer, 01.11. is Winter.
        assert_eq!(
            anlagenfaktor(date!(2027 - 03 - 01), time!(12:00)),
            dec(0.6189)
        );
        assert_eq!(
            anlagenfaktor(date!(2027 - 11 - 01), time!(12:00)),
            dec(0.5030)
        );
    }

    #[test]
    fn solar_pauschal_uses_af_and_wr() {
        let af = anlagenfaktor(date!(2027 - 06 - 15), time!(12:00)); // 0.6189
        // min(0.6189 × 1000 = 618.9; P_WR 600) − P_lim 100 → 500/4 = 125 kWh.
        assert_eq!(
            solar_pauschal(af, dec(1000.0), dec(600.0), None, None, dec(100.0)),
            dec(125.0)
        );
    }

    // ── Nicht-fluktuierend ───────────────────────────────────────────────

    #[test]
    fn nichtfluktuierend_spitz_sign_convention() {
        // Negativer Redispatch: Plan 1000, Limit 400 → +150 kWh.
        assert_eq!(
            nichtfluktuierend_spitz(RedispatchRichtung::Negativ, dec(1000.0), dec(400.0)),
            dec(150.0)
        );
        // Positiver Redispatch (Mehrarbeit): Plan 400, Limit 1000 → −150 kWh.
        assert_eq!(
            nichtfluktuierend_spitz(RedispatchRichtung::Positiv, dec(400.0), dec(1000.0)),
            dec(-150.0)
        );
        // Clamps: no positive W_A on positive Redispatch and vice versa.
        assert_eq!(
            nichtfluktuierend_spitz(RedispatchRichtung::Positiv, dec(1000.0), dec(400.0)),
            Decimal::ZERO
        );
        assert_eq!(
            nichtfluktuierend_spitz(RedispatchRichtung::Negativ, dec(400.0), dec(1000.0)),
            Decimal::ZERO
        );
    }

    #[test]
    fn nichtfluktuierend_pauschal_final_formulas() {
        // Negativ: min(P_0 1200; P_bean 1100) − P_lim 300 → 200 kWh.
        assert_eq!(
            nichtfluktuierend_pauschal(
                RedispatchRichtung::Negativ,
                dec(1200.0),
                Some(dec(1100.0)),
                dec(300.0)
            ),
            dec(200.0)
        );
        // Positiv: P_0 400 − min(P_lim 1000; P_bean 800) → (400−800)/4 = −100.
        assert_eq!(
            nichtfluktuierend_pauschal(
                RedispatchRichtung::Positiv,
                dec(400.0),
                Some(dec(800.0)),
                dec(1000.0)
            ),
            dec(-100.0)
        );
        // Ohne P_bean: plain Fortschreibungsdifferenz.
        assert_eq!(
            nichtfluktuierend_pauschal(RedispatchRichtung::Negativ, dec(1200.0), None, dec(300.0)),
            dec(225.0)
        );
    }

    // ── Überbauung ───────────────────────────────────────────────────────

    #[test]
    fn ueberbauung_no_cut_when_under_cap() {
        let trs = [
            UeberbauungTr {
                w_a_kwh: dec(100.0),
                p_inst_kw: dec(2000.0),
            },
            UeberbauungTr {
                w_a_kwh: dec(50.0),
                p_inst_kw: dec(1000.0),
            },
        ];
        // Cap: 1000 kW × ¼ h − 50 kWh Einspeisung = 200 kWh ≥ 150 → untouched.
        let out = ueberbauung_kuerzung(&trs, dec(1000.0), dec(50.0)).unwrap();
        assert_eq!(out, vec![dec(100.0), dec(50.0)]);
    }

    #[test]
    fn ueberbauung_pro_rata_by_installed_capacity() {
        let trs = [
            UeberbauungTr {
                w_a_kwh: dec(100.0),
                p_inst_kw: dec(2000.0),
            },
            UeberbauungTr {
                w_a_kwh: dec(50.0),
                p_inst_kw: dec(1000.0),
            },
        ];
        // Cap: 400 kW × ¼ h − 10 kWh = 90 kWh; Überschuss 60 kWh split 2:1.
        let out = ueberbauung_kuerzung(&trs, dec(400.0), dec(10.0)).unwrap();
        assert_eq!(out, vec![dec(60.0), dec(30.0)]);
        assert_eq!(out.iter().copied().sum::<Decimal>(), dec(90.0));
    }

    #[test]
    fn ueberbauung_clamps_at_zero_and_redistributes() {
        let trs = [
            UeberbauungTr {
                w_a_kwh: dec(10.0),
                p_inst_kw: dec(3000.0),
            },
            UeberbauungTr {
                w_a_kwh: dec(200.0),
                p_inst_kw: dec(1000.0),
            },
        ];
        // Cap 100 kWh, Überschuss 110. Pro-rata cut for TR1 = 82.5 > 10 →
        // TR1 clamps to 0, the rest of the Kürzung lands on TR2 → 100.
        let out = ueberbauung_kuerzung(&trs, dec(400.0), Decimal::ZERO).unwrap();
        assert_eq!(out[0], Decimal::ZERO);
        assert_eq!(out[1], dec(100.0));
        assert_eq!(out.iter().copied().sum::<Decimal>(), dec(100.0));
    }

    // ── MaLo → TR split ──────────────────────────────────────────────────

    #[test]
    fn malo_split_pro_rata() {
        let out = malo_wert_auf_tr(dec(900.0), &[dec(2000.0), dec(1000.0)]).unwrap();
        assert_eq!(out, vec![dec(600.0), dec(300.0)]);
        assert!(malo_wert_auf_tr(dec(900.0), &[]).is_err());
    }
}
