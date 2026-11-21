//! The Netzanschluss as stated, and the Lokationen it requires.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use time::Date;

/// A kind of Lokation, as matching and drawing need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LokationKind {
    /// Marktlokation, consuming.
    MaloConsumption,
    /// Marktlokation, generating.
    MaloGeneration,
    /// Messlokation.
    Melo,
    /// Technische Ressource, consuming.
    TrConsumption,
    /// Technische Ressource, generating.
    TrGeneration,
    /// Technische Ressource that consumes and generates — a battery.
    TrStorage,
}

impl LokationKind {
    /// Whether matching counts it: a Messlokation is the Messkonzept's choice.
    #[must_use]
    pub const fn compared(self) -> bool {
        !matches!(self, Self::Melo)
    }
}

/// The § 14a EnWG module of a steuerbare Verbrauchseinrichtung (BK8-22/010-A).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modul14a {
    /// Not under § 14a.
    #[default]
    Without,
    /// Pauschale Reduzierung of the Netzentgelt.
    Modul1,
    /// Arbeitspreis at 40 % on a separate Marktlokation (Tenor 2).
    Modul2,
    /// Modul 1 with the zeitvariables Netzentgelt of Modul 3, the only pair.
    Modul1And3,
}

/// How a PV system feeds in (§ 48 Abs. 2 / 2a EEG 2023).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedIn {
    /// Überschusseinspeisung — the household uses what it can.
    #[default]
    Ueberschusseinspeisung,
    /// Volleinspeisung — every kWh into the grid.
    Volleinspeisung,
}

/// A device at the Netzanschluss.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Device {
    /// A PV system on the building.
    Pv {
        /// Installed capacity.
        kwp: Decimal,
        /// Inbetriebnahme, planned or done.
        inbetriebnahme: Date,
        /// Feed-in mode.
        #[serde(default)]
        feed_in: FeedIn,
        /// Expected generation a year.
        #[serde(default)]
        yield_kwh: Option<Decimal>,
        /// Of which used in the household.
        #[serde(default)]
        self_consumption_kwh: Option<Decimal>,
    },
    /// A heat pump.
    HeatPump {
        /// § 14a module.
        #[serde(default)]
        modul: Modul14a,
        /// Consumption a year.
        #[serde(default)]
        kwh: Option<Decimal>,
    },
    /// A wallbox.
    Wallbox {
        /// § 14a module.
        #[serde(default)]
        modul: Modul14a,
        /// Consumption a year.
        #[serde(default)]
        kwh: Option<Decimal>,
    },
    /// A battery.
    Storage,
    /// A further household — an Einliegerwohnung, a flat.
    Household {
        /// Consumption a year.
        #[serde(default)]
        kwh: Option<Decimal>,
    },
}

impl Device {
    /// The § 14a module, for a device that can have one.
    #[must_use]
    pub const fn modul(&self) -> Option<Modul14a> {
        match self {
            Self::HeatPump { modul, .. } | Self::Wallbox { modul, .. } => Some(*modul),
            _ => None,
        }
    }
}

/// The Netzbetreiber's Preisblatt figures a decision needs, net.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Preisblatt {
    /// Arbeitspreis ohne Leistungsmessung in der Niederspannung, ct/kWh.
    #[serde(default)]
    pub arbeitspreis_ct: Option<Decimal>,
    /// The Modul-1 Pauschale, EUR a year.
    #[serde(default)]
    pub pauschale_eur: Option<Decimal>,
}

/// A Netzanschluss as its owner states it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Netzanschluss {
    /// The household's own consumption a year, devices excluded.
    #[serde(default)]
    pub consumption_kwh: Option<Decimal>,
    /// The devices.
    #[serde(default)]
    pub devices: Vec<Device>,
    /// The Netzbetreiber's figures.
    #[serde(default)]
    pub preisblatt: Preisblatt,
    /// The household's energy price, ct/kWh gross — values self-consumed PV.
    #[serde(default)]
    pub energy_price_ct: Option<Decimal>,
}

/// One Lokation in a bundle, and why it is there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lokation {
    /// Its kind.
    pub kind: LokationKind,
    /// The device it belongs to, by index into [`Netzanschluss::devices`];
    /// `None` for the household.
    pub device: Option<usize>,
    /// The source that requires it.
    pub source: &'static str,
}

/// One Lokationsbündel: the Lokationen it must hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lokationsbuendel {
    /// Its Lokationen.
    pub lokationen: Vec<Lokation>,
}

impl Lokationsbuendel {
    /// How many of each compared kind it holds.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<LokationKind, u32> {
        let mut n = BTreeMap::new();
        for l in self.lokationen.iter().filter(|l| l.kind.compared()) {
            *n.entry(l.kind).or_insert(0) += 1;
        }
        n
    }
}

const CODELISTE: &str =
    "Codeliste der Lokationsbündelstrukturen 1.0, konsolidierte Lesefassung 13.12.2024";
const HOUSEHOLD: &str = "Codeliste der Lokationsbündelstrukturen 1.0, konsolidierte Lesefassung 13.12.2024, Kap. 4.2 (S. 19) — 9992 00000 002 6";
const PV_MALO: &str = "GPKE Teil 2 Kap. 2.2.2 Nr. 1 (Neuanlage erzeugende Marktlokation)";
const MODUL2: &str = "BK8-22/010-A Tenor 2 a) (S. 3) — „separat gemessen und an einer separaten Marktlokation abgerechnet“";
const STEUVE: &str = "BK6-22-300 Anlage 1 Ziff. 4.7 — separater Zählpunkt „nicht erforderlich“";
const STORAGE: &str = "BDEW Leitfaden Netzanschluss V2.0 S. 31–33";
const FURTHER_HOUSEHOLD: &str = "Codeliste der Lokationsbündelstrukturen 1.0, konsolidierte Lesefassung 13.12.2024, Kap. 2.5 (S. 12)";

fn lok(kind: LokationKind, device: Option<usize>, source: &'static str) -> Lokation {
    Lokation {
        kind,
        device,
        source,
    }
}

impl Netzanschluss {
    /// The bundles the Netzanschluss requires: the household's, then one per
    /// further household — several bundles behind one Netzlokation (Codeliste
    /// Kap. 2.5).
    #[must_use]
    pub fn lokationsbuendel(&self) -> Vec<Lokationsbuendel> {
        let mut house = vec![lok(LokationKind::MaloConsumption, None, HOUSEHOLD)];
        let mut further = Vec::new();
        for (i, d) in self.devices.iter().enumerate() {
            let i = Some(i);
            match d {
                Device::Pv { .. } => {
                    house.push(lok(LokationKind::MaloGeneration, i, PV_MALO));
                    // Objektcode 9992 00000 105 8, Technische Ressource Erzeugung.
                    house.push(lok(LokationKind::TrGeneration, i, CODELISTE));
                }
                Device::HeatPump { modul, .. } | Device::Wallbox { modul, .. } => {
                    if *modul == Modul14a::Modul2 {
                        house.push(lok(LokationKind::MaloConsumption, i, MODUL2));
                        house.push(lok(LokationKind::Melo, i, MODUL2));
                        house.push(lok(LokationKind::TrConsumption, i, MODUL2));
                    } else {
                        house.push(lok(LokationKind::TrConsumption, i, STEUVE));
                    }
                }
                // Objektcode 9992 00000 107 4, Technische Ressource Speicher.
                Device::Storage => house.push(lok(LokationKind::TrStorage, i, STORAGE)),
                Device::Household { .. } => further.push(Lokationsbuendel {
                    lokationen: vec![lok(LokationKind::MaloConsumption, i, FURTHER_HOUSEHOLD)],
                }),
            }
        }
        std::iter::once(Lokationsbuendel { lokationen: house })
            .chain(further)
            .collect()
    }

    /// Whether a PV system is among the devices.
    #[must_use]
    pub fn has_pv(&self) -> bool {
        self.devices.iter().any(|d| matches!(d, Device::Pv { .. }))
    }
}

/// What a change to a Netzanschluss adds and removes, bundle by bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Change {
    /// Per bundle, the kinds added with their count.
    pub added: Vec<BTreeMap<LokationKind, u32>>,
    /// Per bundle, the kinds removed with their count.
    pub removed: Vec<BTreeMap<LokationKind, u32>>,
}

impl Change {
    /// Compare two states of one Netzanschluss, Messlokationen included.
    #[must_use]
    pub fn between(before: &Netzanschluss, after: &Netzanschluss) -> Self {
        let count = |b: Option<&Lokationsbuendel>| {
            let mut n: BTreeMap<LokationKind, u32> = BTreeMap::new();
            for l in b.map(|b| b.lokationen.as_slice()).unwrap_or_default() {
                *n.entry(l.kind).or_insert(0) += 1;
            }
            n
        };
        let diff = |x: &BTreeMap<LokationKind, u32>, y: &BTreeMap<LokationKind, u32>| {
            x.iter()
                .filter_map(|(k, v)| {
                    v.checked_sub(*y.get(k).unwrap_or(&0))
                        .filter(|d| *d > 0)
                        .map(|d| (*k, d))
                })
                .collect::<BTreeMap<_, _>>()
        };
        let (a, b) = (before.lokationsbuendel(), after.lokationsbuendel());
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for i in 0..a.len().max(b.len()) {
            let (old, new) = (count(a.get(i)), count(b.get(i)));
            added.push(diff(&new, &old));
            removed.push(diff(&old, &new));
        }
        Self { added, removed }
    }
}
