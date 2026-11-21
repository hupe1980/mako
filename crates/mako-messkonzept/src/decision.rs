//! The decisions that shape a Messkonzept, each with its choices, its euros
//! where a source fixes them, and the inputs it still lacks.

use grid_billing::RoundMoney;
use grid_billing::msbg::{
    Entgeltschuldner, MessstellenKategorie, PflichtEinstufung, Preisobergrenze,
    preisobergrenze_eur_per_jahr,
};
use rust_decimal::Decimal;
use serde::Serialize;
use time::{Date, Month};

use crate::netzanschluss::{Device, FeedIn, Modul14a, Netzanschluss, Preisblatt};

/// What a decision is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// § 14a EnWG Modul 1 or Modul 2 for one device.
    Modul14a,
    /// Überschuss- or Volleinspeisung for one PV system.
    FeedIn,
    /// Whether an intelligentes Messsystem is required, and its cap.
    Messsystem,
    /// How a further household is supplied.
    Household,
}

/// One choice of a decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    /// Stable key: `modul1`, `modul2`, `ueberschusseinspeisung`, …
    pub key: &'static str,
    /// Its yearly value to the owner, EUR, where a source fixes it: a saving
    /// or a revenue is positive, a charge negative.
    pub eur_per_year: Option<Decimal>,
    /// The choice the Netzanschluss states.
    pub chosen: bool,
}

/// One decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Decision {
    /// What it is about.
    pub topic: Topic,
    /// The device, by index into [`Netzanschluss::devices`].
    pub device: Option<usize>,
    /// The choices.
    pub choices: Vec<Choice>,
    /// The inputs a priced comparison still needs, by field name.
    pub missing: Vec<&'static str>,
    /// Facts the decision carries — a threshold, a date — by key.
    pub facts: Vec<(&'static str, String)>,
    /// The sources it rests on.
    pub source: &'static str,
}

const Q_14A: &str = "§ 14a EnWG; BK8-22/010-A Tenor 2 b) (S. 3) — reduzierter Arbeitspreis 40 %";
const Q_EEG: &str = "§ 48 Abs. 2, 2a, § 49, § 53 EEG 2023";
const Q_MSBG: &str = "§§ 29, 30 MsbG";
const Q_HOUSEHOLD: &str = "§ 3 Nr. 53, § 42a Abs. 1, § 42b Abs. 1 EnWG; § 21 Abs. 3 EEG 2023; \
                           Codeliste der Lokationsbündelstrukturen 1.0, konsolidierte Lesefassung \
                           13.12.2024, Kap. 2.5 (S. 12)";

/// The first day a Volleinspeisung notified on `notice` applies to a PV
/// system commissioned on `inbetriebnahme`: from commissioning when notified
/// before it, otherwise from the 1 January after the next 1 December —
/// „vor dem 1. Dezember des vorangegangenen Kalenderjahres“ (§ 48 Abs. 2a
/// EEG 2023).
#[must_use]
pub fn volleinspeisung_from(notice: Date, inbetriebnahme: Date) -> Date {
    if notice < inbetriebnahme {
        return inbetriebnahme;
    }
    let year = if notice.month() < Month::December {
        notice.year() + 1
    } else {
        notice.year() + 2
    };
    Date::from_calendar_date(year, Month::January, 1).unwrap_or(inbetriebnahme)
}

fn net_ct(aw: Decimal) -> Decimal {
    (aw - eeg_billing::rates::sect53_deduction(eeg_billing::ErzeugungsArt::SolarAufdach))
        .max(Decimal::ZERO)
}

fn eur(ct_times_kwh: Decimal) -> Decimal {
    (ct_times_kwh / Decimal::ONE_HUNDRED).round_kfm(2)
}

impl Netzanschluss {
    /// The decisions this Netzanschluss poses, on `today`.
    #[must_use]
    pub fn decisions(&self, today: Date) -> Vec<Decision> {
        let mut out = Vec::new();
        for (i, d) in self.devices.iter().enumerate() {
            match d {
                Device::HeatPump { modul, kwh } | Device::Wallbox { modul, kwh } => {
                    out.push(self.modul14a(i, *modul, *kwh));
                }
                Device::Pv { .. } => out.extend(self.feed_in(i, d, today)),
                Device::Household { .. } => out.push(self.household(i)),
                Device::Storage => {}
            }
        }
        out.push(self.messsystem(today));
        out
    }

    fn household(&self, i: usize) -> Decision {
        // A Gebäudestromanlage is „aus solarer Strahlungsenergie“ (§ 3 Nr. 53
        // EnWG) and Mieterstrom is that of § 21 Abs. 3 EEG (§ 42a Abs. 1 EnWG),
        // so both need the building's PV; without it the household has its own
        // Marktlokation and nothing to share.
        let keys: &[&'static str] = if self.has_pv() {
            &[
                "own_marktlokation",
                "gemeinschaftliche_gebaeudeversorgung",
                "mieterstrom",
            ]
        } else {
            &["own_marktlokation"]
        };
        Decision {
            topic: Topic::Household,
            device: Some(i),
            choices: keys
                .iter()
                .map(|key| Choice {
                    key,
                    eur_per_year: None,
                    chosen: false,
                })
                .collect(),
            missing: Vec::new(),
            // § 42b Abs. 1 Nr. 3 EnWG: „viertelstündlich gemessen“.
            facts: if self.has_pv() {
                vec![(
                    "quarter_hourly_metering",
                    "gemeinschaftliche_gebaeudeversorgung".into(),
                )]
            } else {
                Vec::new()
            },
            source: Q_HOUSEHOLD,
        }
    }

    fn modul14a(&self, i: usize, modul: Modul14a, kwh: Option<Decimal>) -> Decision {
        let share = grid_billing::Reduktionsfaktor::REGELFALL.get();
        let Preisblatt {
            arbeitspreis_ct,
            pauschale_eur,
        } = self.preisblatt;
        let mut missing = Vec::new();
        if arbeitspreis_ct.is_none() {
            missing.push("preisblatt.arbeitspreis_ct");
        }
        if pauschale_eur.is_none() {
            missing.push("preisblatt.pauschale_eur");
        }
        if kwh.is_none() {
            missing.push("kwh");
        }
        let saving_ct = arbeitspreis_ct.map(|ap| ap * (Decimal::ONE - share));
        let modul2 = saving_ct.zip(kwh).map(|(s, k)| eur(s * k));
        let mut facts = vec![(
            "arbeitspreis_share_percent",
            (share * Decimal::ONE_HUNDRED).normalize().to_string(),
        )];
        if let Some((s, p)) = saving_ct
            .zip(pauschale_eur)
            .filter(|(s, _)| *s > Decimal::ZERO)
        {
            facts.push((
                "modul2_from_kwh",
                (p * Decimal::ONE_HUNDRED / s).round_kfm(0).to_string(),
            ));
        }
        Decision {
            topic: Topic::Modul14a,
            device: Some(i),
            choices: vec![
                Choice {
                    key: "modul1",
                    eur_per_year: pauschale_eur,
                    chosen: matches!(modul, Modul14a::Modul1 | Modul14a::Modul1And3),
                },
                Choice {
                    key: "modul2",
                    eur_per_year: modul2,
                    chosen: modul == Modul14a::Modul2,
                },
            ],
            missing,
            facts,
            source: Q_14A,
        }
    }

    fn feed_in(&self, i: usize, pv: &Device, today: Date) -> Option<Decision> {
        let Device::Pv {
            kwp,
            inbetriebnahme,
            feed_in,
            yield_kwh,
            self_consumption_kwh,
        } = *pv
        else {
            return None;
        };
        let ueberschuss =
            eeg_billing::rates::solar_pv_ueberschuss_aw_ct(kwp, inbetriebnahme).map(net_ct);
        let voll =
            eeg_billing::rates::solar_pv_volleinspeisung_aw_ct(kwp, inbetriebnahme).map(net_ct);
        let mut missing = Vec::new();
        if yield_kwh.is_none() {
            missing.push("yield_kwh");
        }
        if self_consumption_kwh.is_none() {
            missing.push("self_consumption_kwh");
        }
        if self.energy_price_ct.is_none() {
            missing.push("energy_price_ct");
        }
        let own_kwh = self_consumption_kwh
            .unwrap_or_default()
            .min(yield_kwh.unwrap_or_default());
        let ueberschuss_eur = ueberschuss
            .zip(yield_kwh)
            .zip(self_consumption_kwh)
            .zip(self.energy_price_ct)
            .map(|(((ct, y), _), price)| eur((y - own_kwh) * ct + own_kwh * price));
        let voll_eur = voll.zip(yield_kwh).map(|(ct, y)| eur(y * ct));
        let mut facts = vec![(
            "volleinspeisung_from",
            volleinspeisung_from(today, inbetriebnahme).to_string(),
        )];
        if let Some(ct) = ueberschuss {
            facts.push(("ueberschusseinspeisung_ct", ct.to_string()));
        }
        if let Some(ct) = voll {
            facts.push(("volleinspeisung_ct", ct.to_string()));
        }
        Some(Decision {
            topic: Topic::FeedIn,
            device: Some(i),
            choices: vec![
                Choice {
                    key: "ueberschusseinspeisung",
                    eur_per_year: ueberschuss_eur,
                    chosen: feed_in == FeedIn::Ueberschusseinspeisung,
                },
                Choice {
                    key: "volleinspeisung",
                    eur_per_year: voll_eur,
                    chosen: feed_in == FeedIn::Volleinspeisung,
                },
            ],
            missing,
            facts,
            source: Q_EEG,
        })
    }

    fn messsystem(&self, today: Date) -> Decision {
        let kw = self
            .devices
            .iter()
            .filter_map(|d| match d {
                Device::Pv { kwp, .. } => Some(*kwp),
                _ => None,
            })
            .sum::<Decimal>();
        let steuerbar = self
            .devices
            .iter()
            .any(|d| d.modul().is_some_and(|m| m != Modul14a::Without));
        let devices_kwh = self
            .devices
            .iter()
            .filter_map(|d| match d {
                // A Modul-2 device is its own Marktlokation, so its own figure.
                Device::HeatPump { modul, kwh } | Device::Wallbox { modul, kwh }
                    if *modul != Modul14a::Modul2 =>
                {
                    *kwh
                }
                _ => None,
            })
            .sum::<Decimal>();
        let kwh = self.consumption_kwh.map(|v| v + devices_kwh);
        // § 29 Abs. 1 MsbG: above 6 000 kWh (Nr. 1) and with a § 14a
        // Vereinbarung (Nr. 2 a) the duty holds; above 7 kW (Nr. 2 b) only
        // „soweit dies erforderlich ist“ for the § 45 Abs. 1 shares — the MSB's
        // rollout decides, so the plant alone makes it conditional.
        let duty = steuerbar || kwh.is_some_and(|k| k > Decimal::from(6000u16));
        let conditional = !duty && kw > Decimal::from(7u8);
        let category = if duty || conditional {
            MessstellenKategorie::Pflichteinbau(PflichtEinstufung {
                jahresverbrauch_kwh: kwh,
                installierte_leistung_kw: (kw > Decimal::ZERO).then_some(kw),
                steuerbare_verbrauchseinrichtung: steuerbar,
            })
        } else {
            MessstellenKategorie::OptionalerEinbau
        };
        let charge = |s| match preisobergrenze_eur_per_jahr(category, s, today) {
            Preisobergrenze::Betrag(b) => Some(-b),
            Preisobergrenze::Angemessen | Preisobergrenze::VorSchedule => None,
        };
        let mut facts = vec![(
            "duty",
            if duty {
                "yes"
            } else if conditional {
                "conditional"
            } else {
                "no"
            }
            .to_string(),
        )];
        if let Some(nb) = charge(Entgeltschuldner::Netzbetreiber) {
            facts.push(("netzbetreiber_eur", (-nb).to_string()));
        }
        Decision {
            topic: Topic::Messsystem,
            device: None,
            choices: vec![Choice {
                key: if duty || conditional {
                    "pflichteinbau"
                } else {
                    "optionaler_einbau"
                },
                eur_per_year: charge(Entgeltschuldner::Letztverbraucher),
                chosen: duty,
            }],
            missing: if self.consumption_kwh.is_none() {
                vec!["consumption_kwh"]
            } else {
                Vec::new()
            },
            facts,
            source: Q_MSBG,
        }
    }
}
