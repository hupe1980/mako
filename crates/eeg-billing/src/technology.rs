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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ErzeugungsArt {
    /// Generic solar PV (backward compat; prefer `SolarAufdach`/`SolarFreiflaeche`).
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
