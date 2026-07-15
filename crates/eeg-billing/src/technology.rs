//! [`ErzeugungsArt`] — typed EEG/KWKG plant technology category.
//!
//! Maps 1:1 to the `erzeugungsart` TEXT column in `einsd`'s `eeg_anlagen` table.
//! Used for technology-specific rule dispatch (e.g. §51 EEG 2017 wind exemption).

// ── ErzeugungsArt ─────────────────────────────────────────────────────────────

/// EEG/KWKG plant technology type.
///
/// ## §51 EEG 2017 relevance
///
/// EEG 2017 distinguishes wind turbines (<3 MW exempt) from all other types (<500 kW exempt).
/// Use [`ErzeugungsArt::is_wind`] to select the correct §51 threshold.
///
/// ## DB mapping
///
/// | `ErzeugungsArt` | DB `erzeugungsart` TEXT |
/// |---|---|
/// | `Solar` | `"SOLAR"` |
/// | `SolarAufdach` | `"SOLAR_AUFDACH"` |
/// | `SolarFreiflaeche` | `"SOLAR_FREFLAECHE"` |
/// | `SolarAgriPv` | `"SOLAR_AGRIPV"` |
/// | `SolarMieterstrom` | `"SOLAR_MIETERSTROM"` |
/// | `SolarStecker` | `"SOLAR_STECKER"` |
/// | `WindOnshore` | `"WIND_ONSHORE"` |
/// | `WindOffshore` | `"WIND_OFFSHORE"` |
/// | `Biomasse` | `"BIOMASSE"` |
/// | `BiomassHolz` | `"BIOMASSE_HOLZ"` |
/// | `Biogas` | `"BIOGAS"` |
/// | `Biomethan` | `"BIOMETHAN"` |
/// | `Klaegas` | `"KLAEGAS"` |
/// | `Grubengas` | `"GRUBENGAS"` |
/// | `Deponiegas` | `"DEPONIEGAS"` |
/// | `Wasserkraft` | `"WASSERKRAFT"` |
/// | `Geothermie` | `"GEOTHERMIE"` |
/// | `Gezeiten` | `"GEZEITEN"` |
/// | `Kwk` | `"KWKG"` |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ErzeugungsArt {
    /// Generic solar PV (backward compat; prefer `SolarAufdach`/`SolarFreiflaeche`).
    #[default]
    Solar,
    /// Rooftop PV (Gebäudeanlage) — higher §48 rates.
    SolarAufdach,
    /// Ground-mounted PV (Freiflächenanlage) — lower rates, tender-based >1 MWp.
    SolarFreiflaeche,
    /// Agri-PV (§51a bonus, dual land use).
    SolarAgriPv,
    /// Mieterstrom building solar (§38a).
    SolarMieterstrom,
    /// Balkonkraftwerk / Stecker-PV (<800 W, simplified registration).
    SolarStecker,
    /// Wind onshore (§21 EEG, tender-based >750 kW).
    WindOnshore,
    /// Wind offshore (§§70ff EEG, Offshore-Zuschlag via BNetzA).
    WindOffshore,
    /// Biomasse (§42-43 EEG 2023).
    Biomasse,
    /// Holzbiomasse (§42a EEG 2023, restricted).
    BiomassHolz,
    /// Biogas (plant-based gas).
    Biogas,
    /// Biomethan (upgraded biomethane).
    Biomethan,
    /// Klärgas (sewage gas).
    Klaegas,
    /// Grubengas (mine gas).
    Grubengas,
    /// Deponiegas (landfill gas).
    Deponiegas,
    /// Wasserkraft (run-of-river and reservoir hydro).
    Wasserkraft,
    /// Geothermie.
    Geothermie,
    /// Gezeitenenergie (tidal).
    Gezeiten,
    /// Kraft-Wärme-Kopplungsanlage (KWKG, not EEG).
    Kwk,
}

impl ErzeugungsArt {
    /// Returns `true` for wind turbines (onshore or offshore).
    ///
    /// Used for §51 Abs. 3 Nr. 1 EEG 2017: wind turbines <3 MW are exempt
    /// (higher threshold than the 500 kW for "sonstige Anlagen").
    pub fn is_wind(self) -> bool {
        matches!(self, Self::WindOnshore | Self::WindOffshore)
    }

    /// Returns `true` for solar PV variants (all rooftop, ground-mounted, agri-PV).
    pub fn is_solar(self) -> bool {
        matches!(
            self,
            Self::Solar
                | Self::SolarAufdach
                | Self::SolarFreiflaeche
                | Self::SolarAgriPv
                | Self::SolarMieterstrom
                | Self::SolarStecker
        )
    }

    /// Returns `true` for biomass/biogas/biomethan/gas variants.
    pub fn is_biomasse_or_gas(self) -> bool {
        matches!(
            self,
            Self::Biomasse
                | Self::BiomassHolz
                | Self::Biogas
                | Self::Biomethan
                | Self::Klaegas
                | Self::Grubengas
                | Self::Deponiegas
        )
    }

    /// Parse from the DB `erzeugungsart` TEXT column.
    ///
    /// Returns `Err` for unknown values — callers should fall back to `Solar`
    /// or log a warning for unexpected technology codes.
    pub fn from_db_str(s: &str) -> Result<Self, InvalidErzeugungsArt> {
        match s {
            "SOLAR" => Ok(Self::Solar),
            "SOLAR_AUFDACH" => Ok(Self::SolarAufdach),
            "SOLAR_FREFLAECHE" => Ok(Self::SolarFreiflaeche),
            "SOLAR_AGRIPV" => Ok(Self::SolarAgriPv),
            "SOLAR_MIETERSTROM" => Ok(Self::SolarMieterstrom),
            "SOLAR_STECKER" => Ok(Self::SolarStecker),
            "WIND_ONSHORE" => Ok(Self::WindOnshore),
            "WIND_OFFSHORE" => Ok(Self::WindOffshore),
            "BIOMASSE" => Ok(Self::Biomasse),
            "BIOMASSE_HOLZ" => Ok(Self::BiomassHolz),
            "BIOGAS" => Ok(Self::Biogas),
            "BIOMETHAN" => Ok(Self::Biomethan),
            "KLAEGAS" => Ok(Self::Klaegas),
            "GRUBENGAS" => Ok(Self::Grubengas),
            "DEPONIEGAS" => Ok(Self::Deponiegas),
            "WASSERKRAFT" => Ok(Self::Wasserkraft),
            "GEOTHERMIE" => Ok(Self::Geothermie),
            "GEZEITEN" => Ok(Self::Gezeiten),
            "KWKG" => Ok(Self::Kwk),
            _ => Err(InvalidErzeugungsArt(s.to_owned())),
        }
    }

    /// The canonical DB column value for this variant.
    pub fn to_db_str(self) -> &'static str {
        match self {
            Self::Solar => "SOLAR",
            Self::SolarAufdach => "SOLAR_AUFDACH",
            Self::SolarFreiflaeche => "SOLAR_FREFLAECHE",
            Self::SolarAgriPv => "SOLAR_AGRIPV",
            Self::SolarMieterstrom => "SOLAR_MIETERSTROM",
            Self::SolarStecker => "SOLAR_STECKER",
            Self::WindOnshore => "WIND_ONSHORE",
            Self::WindOffshore => "WIND_OFFSHORE",
            Self::Biomasse => "BIOMASSE",
            Self::BiomassHolz => "BIOMASSE_HOLZ",
            Self::Biogas => "BIOGAS",
            Self::Biomethan => "BIOMETHAN",
            Self::Klaegas => "KLAEGAS",
            Self::Grubengas => "GRUBENGAS",
            Self::Deponiegas => "DEPONIEGAS",
            Self::Wasserkraft => "WASSERKRAFT",
            Self::Geothermie => "GEOTHERMIE",
            Self::Gezeiten => "GEZEITEN",
            Self::Kwk => "KWKG",
        }
    }
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Returned by [`ErzeugungsArt::from_db_str`] for unknown technology strings.
#[derive(Debug, thiserror::Error)]
#[error("unknown erzeugungsart: {0:?}")]
pub struct InvalidErzeugungsArt(pub String);

// ── InbetriebnahmeTyp ─────────────────────────────────────────────────────────

/// Type of commissioning event that started (or restarted) the EEG Förderdauer.
///
/// The commissioning type determines which regulatory rules apply and whether
/// the 20-year Förderdauer clock is reset or continues from the original date.
///
/// ## Legal basis
///
/// §3 Nr. 30 EEG 2023 defines "Inbetriebnahme" as the first feed-in of
/// electricity after all necessary installations are complete.
///
/// §22 EEG 2023 (Repowering): replacing components with higher capacity resets
/// the Förderdauer clock.
///
/// §24 EEG 2023 (Zusammenlegung): merging physically separate plants does NOT
/// reset the clock — the oldest plant's Förderdauer continues.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum InbetriebnahmeTyp {
    /// §3 Nr. 30 EEG 2023: first time the plant generates electricity.
    ///
    /// Starts the 20-year Förderdauer. All EEG rules apply from commissioning date.
    #[default]
    Erstinbetriebnahme,

    /// §3 Nr. 30 EEG 2023: temporary shutdown + restart (same plant, same capacity).
    ///
    /// Does NOT reset the Förderdauer. The original commissioning date continues
    /// to govern tariff and duration. Typical: plant moved, repaired, or temporarily
    /// decommissioned and returned to operation.
    Wiederinbetriebnahme,

    /// §3 Nr. 30a EEG 2023: technical modernization without capacity increase.
    ///
    /// Replaces equipment (inverter, cables) but not generators. Förderdauer continues.
    /// May affect technical compliance status (e.g. Fernsteuerbarkeit).
    Modernisierung,

    /// §22 EEG 2023: repowering — complete replacement of generating components.
    ///
    /// **Resets the Förderdauer clock** to the repowering date. The plant receives a
    /// new 20-year subsidy period at the tariff valid at the repowering commissioning.
    /// Use `foerderendedatum_repowering(repowering_datum)` to compute the new end date.
    Repowering,

    /// §24 EEG 2023: plant created by Zusammenlegung of multiple existing plants.
    ///
    /// Does NOT reset the Förderdauer. The oldest component plant's commissioning date
    /// governs the subsidy duration for the merged entity.
    /// Individual component plants continue under their original `foerderendedatum`.
    Zusammenlegung,

    /// §24 EEG 2023: capacity extension block (Erweiterung).
    ///
    /// The extension block starts its own 20-year Förderdauer at the extension date,
    /// at the tariff valid at that date (typically lower due to degression).
    /// Model via `CapacityBlock` in `SettleInput`.
    Erweiterung,
}

impl InbetriebnahmeTyp {
    /// Returns `true` when this commissioning type resets the 20-year Förderdauer.
    ///
    /// Only `Repowering` resets the clock. All other types continue from the
    /// original commissioning date (or start a new parallel block for `Erweiterung`).
    #[must_use]
    pub fn resets_foerderdauer(self) -> bool {
        self == Self::Repowering
    }

    /// Returns `true` for the initial commissioning (first electricity generation).
    #[must_use]
    pub fn is_erstinbetriebnahme(self) -> bool {
        self == Self::Erstinbetriebnahme
    }

    /// Parse from the DB `inbetriebnahme_typ` TEXT column.
    pub fn from_db_str(s: &str) -> Result<Self, InvalidInbetriebnahmeTyp> {
        match s {
            "ERSTINBETRIEBNAHME" => Ok(Self::Erstinbetriebnahme),
            "WIEDERINBETRIEBNAHME" => Ok(Self::Wiederinbetriebnahme),
            "MODERNISIERUNG" => Ok(Self::Modernisierung),
            "REPOWERING" => Ok(Self::Repowering),
            "ZUSAMMENLEGUNG" => Ok(Self::Zusammenlegung),
            "ERWEITERUNG" => Ok(Self::Erweiterung),
            _ => Err(InvalidInbetriebnahmeTyp(s.to_owned())),
        }
    }

    /// Canonical DB column value.
    #[must_use]
    pub fn to_db_str(self) -> &'static str {
        match self {
            Self::Erstinbetriebnahme => "ERSTINBETRIEBNAHME",
            Self::Wiederinbetriebnahme => "WIEDERINBETRIEBNAHME",
            Self::Modernisierung => "MODERNISIERUNG",
            Self::Repowering => "REPOWERING",
            Self::Zusammenlegung => "ZUSAMMENLEGUNG",
            Self::Erweiterung => "ERWEITERUNG",
        }
    }
}

/// Returned by [`InbetriebnahmeTyp::from_db_str`] for unknown values.
#[derive(Debug, thiserror::Error)]
#[error("unknown inbetriebnahme_typ: {0:?}")]
pub struct InvalidInbetriebnahmeTyp(pub String);

// ── RepoweringScope ───────────────────────────────────────────────────────────

/// Scope of a repowering event — determines whether the 20-year Förderdauer resets.
///
/// Repowering is one of the most legally complex topics in the EEG. Whether the
/// Förderdauer resets depends on what exactly was replaced.
///
/// ## §22 EEG 2023 — Key rule
///
/// The Förderdauer resets only for **Vollrepowering** (complete new plant at the
/// same site). Partial component replacements do NOT reset the clock — the original
/// commissioning date continues to govern.
///
/// ## Practical guidance
///
/// When in doubt, consult BNetzA guidance or a specialized EEG attorney.
/// The distinction between `RotorBlade`, `WholeNacelle`, and `TurbineUnit` is
/// fact-specific and the BNetzA has issued conflicting guidance in edge cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum RepoweringScope {
    /// **Vollrepowering**: Complete replacement of all generating components
    /// (generator, nacelle, rotor, tower, foundation).
    ///
    /// **Resets Förderdauer** to the new commissioning date.
    /// Equivalent to a new plant at the same grid connection point.
    /// Uses `foerderendedatum_repowering(new_commissioning_date)`.
    Full,

    /// **Teilrepowering — Rotor only**: rotor blades and hub replaced,
    /// nacelle and generator unchanged.
    ///
    /// **Does NOT reset Förderdauer.** Old commissioning date continues.
    /// Rotor replacement alone does not constitute "Inbetriebnahme" under §3 Nr. 30 EEG 2023.
    RotorOnly,

    /// **Teilrepowering — Nacelle and rotor replaced**, tower and foundation unchanged.
    ///
    /// **Legal status is contested** (BNetzA has not issued definitive guidance).
    /// Conservative interpretation: Förderdauer does NOT reset (original date governs).
    /// Aggressive interpretation: may reset if generator output increases substantially.
    NacelleAndRotor,

    /// **Teilrepowering — Complete turbine unit replaced** (generator + nacelle + rotor),
    /// but tower and foundation unchanged.
    ///
    /// **Legal status is contested.** Most EEG specialists consider this a
    /// Vollrepowering (Förderdauer resets) when capacity increases significantly.
    /// BNetzA position: resets if the turbine is "technisch and wirtschaftlich neu."
    TurbineUnit,

    /// **Repowering with capacity increase** — same classification as `Full` but
    /// explicitly tracks that the new plant has higher rated power than the original.
    ///
    /// Relevant for Ausschreibungspflicht threshold check (§22 EEG 2023):
    /// the new capacity may push the plant above the 750 kW wind tender threshold.
    FullWithCapacityIncrease,
}

impl RepoweringScope {
    /// Returns `true` when this repowering scope **definitely resets** the Förderdauer.
    ///
    /// Returns `false` for contested cases — the caller must resolve the legal question
    /// before computing the new Förderdauer.
    #[must_use]
    pub fn resets_foerderdauer_definitely(self) -> bool {
        matches!(self, Self::Full | Self::FullWithCapacityIncrease)
    }

    /// Returns `true` when this scope involves replacing the nacelle or generating unit.
    ///
    /// Rotor-only replacement never replaces the generating unit.
    #[must_use]
    pub fn replaces_generating_unit(self) -> bool {
        matches!(
            self,
            Self::Full | Self::FullWithCapacityIncrease | Self::NacelleAndRotor | Self::TurbineUnit
        )
    }
}
