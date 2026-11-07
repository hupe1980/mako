//! Verbrauchsteuerliche Begünstigungen — StromStG and EnergieStG.
//!
//! German excise law knows **three** different things, and only two of them
//! change what a supplier may invoice:
//!
//! | Instrument | Who acts | Effect on the invoice |
//! |---|---|---|
//! | **Steuerbefreiung** (§ 9 Abs. 1 StromStG, §§ 25–29 EnergieStG) | the supplier, against an Erlaubnis (§ 9 Abs. 4 StromStG) | the levy is **not** invoiced |
//! | **Steuerermäßigung** (§ 9 Abs. 2/3 StromStG) | the supplier | the levy is invoiced at the **reduced** rate |
//! | **Steuerentlastung** (§ 9b StromStG, §§ 53a, 54 EnergieStG) | the *customer*, afterwards, at the Hauptzollamt | **none** — the supply is invoiced in full |
//!
//! Collapsing the third into the first is the mistake this module exists to
//! make unrepresentable. A Unternehmen des Produzierenden Gewerbes pays the
//! full 2,05 ct/kWh on every kWh it buys and reclaims 2,00 ct/kWh from the
//! Hauptzollamt afterwards (§ 9b StromStG, permanent at the EU minimum rate
//! since 01.01.2026). A supplier that zero-rates the levy instead has under-
//! declared its own Stromsteueranmeldung — the customer's later Entlastungs-
//! antrag does not repair that, it duplicates it.
//!
//! What an invoice *should* do for an Entlastung is say so: the customer needs
//! to know how much levy it carried in order to file. [`Steuerentlastung`]
//! renders that as an informational position and never touches an amount.

use rust_decimal::Decimal;
use rust_decimal::dec;
use serde::{Deserialize, Serialize};

// ── Stromsteuer ───────────────────────────────────────────────────────────────

/// § 9 Abs. 1 StromStG — grounds on which a supply carries **no** Stromsteuer,
/// each claimed by name so the invoice can cite it.
///
/// Nr. 3, the ground a Mieterstrom or GGV supply stands on, is not here: it
/// depends on facts of the plant and the supply, so it is
/// [`StromsteuerTarif::Kleinanlage`] and carries them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StromsteuerBefreiung {
    /// Nr. 1 — Strom aus Anlagen > 2 MW ausschließlich aus Wind, Sonne,
    /// Erdwärme oder Wasserkraft (≤ 10 MW Generatorleistung), vom Betreiber am
    /// Ort der Erzeugung zum Selbstverbrauch entnommen.
    ErneuerbarSelbstverbrauch,
    /// Nr. 2 — Strom, der zur Stromerzeugung oder zur Aufrechterhaltung der
    /// Erzeugungsfähigkeit entnommen wird (Kraftwerkseigenverbrauch).
    ZurStromerzeugung,
    /// Nr. 4 — Notstromanlagen.
    Notstrom,
    /// Nr. 5 — Bordnetze von Wasser-, Luft- und Schienenfahrzeugen, erzeugt und
    /// verbraucht an Bord.
    Bordnetz,
    /// Nr. 6 — Strom aus versteuerten Energieerzeugnissen in Anlagen bis 2 MW,
    /// am Ort der Erzeugung ohne Netzdurchleitung entnommen.
    VersteuerteEnergieerzeugnisse,
    /// Nr. 7/8 — ausländische Streitkräfte (NATO-Truppenstatut) und
    /// zwischenstaatliche Einrichtungen.
    ZwischenstaatlicheEinrichtung,
}

impl StromsteuerBefreiung {
    /// The § reference to print on the invoice.
    #[must_use]
    pub const fn citation(self) -> &'static str {
        match self {
            Self::ErneuerbarSelbstverbrauch => "§ 9 Abs. 1 Nr. 1 StromStG",
            Self::ZurStromerzeugung => "§ 9 Abs. 1 Nr. 2 StromStG",
            Self::Notstrom => "§ 9 Abs. 1 Nr. 4 StromStG",
            Self::Bordnetz => "§ 9 Abs. 1 Nr. 5 StromStG",
            Self::VersteuerteEnergieerzeugnisse => "§ 9 Abs. 1 Nr. 6 StromStG",
            Self::ZwischenstaatlicheEinrichtung => "§ 9 Abs. 1 Nr. 7/8 StromStG",
        }
    }

    /// The invoice line describing the exemption.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::ErneuerbarSelbstverbrauch => {
                "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 1 StromStG — Selbstverbrauch \
                 erneuerbar erzeugten Stroms am Ort der Erzeugung)"
            }
            Self::ZurStromerzeugung => {
                "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 2 StromStG — Strom zur Stromerzeugung)"
            }
            Self::Notstrom => {
                "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 4 StromStG — Notstromanlage)"
            }
            Self::Bordnetz => "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 5 StromStG — Bordnetz)",
            Self::VersteuerteEnergieerzeugnisse => {
                "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 6 StromStG — Strom aus versteuerten \
                 Energieerzeugnissen, Anlage bis 2 MW)"
            }
            Self::ZwischenstaatlicheEinrichtung => {
                "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 7/8 StromStG — Streitkräfte / \
                 zwischenstaatliche Einrichtung)"
            }
        }
    }
}

/// § 9 Abs. 2 / Abs. 3 StromStG — supplies carrying a **reduced** Stromsteuer.
///
/// A reduction is not an exemption: the levy is still invoiced, at the rate the
/// statute names. Treating either as an exemption drops a real tax line off the
/// invoice — for Fahrstrom, 1,142 ct/kWh of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StromsteuerErmaessigung {
    /// § 9 Abs. 2 — Verkehr mit Oberleitungsomnibussen und Fahrbetrieb im
    /// Schienenbahnverkehr: **11,42 EUR/MWh**.
    Fahrstrom,
    /// § 9 Abs. 3 — landseitige Stromversorgung von Wasserfahrzeugen
    /// (Landstrom): **0,50 EUR/MWh**.
    Landstrom,
}

impl StromsteuerErmaessigung {
    /// The statutory rate in ct/kWh.
    #[must_use]
    pub const fn rate_ct_per_kwh(self) -> Decimal {
        match self {
            // 11,42 EUR/MWh = 1,142 ct/kWh
            Self::Fahrstrom => dec!(1.142),
            // 0,50 EUR/MWh = 0,05 ct/kWh
            Self::Landstrom => dec!(0.05),
        }
    }

    /// The § reference to print on the invoice.
    #[must_use]
    pub const fn citation(self) -> &'static str {
        match self {
            Self::Fahrstrom => "§ 9 Abs. 2 StromStG",
            Self::Landstrom => "§ 9 Abs. 3 StromStG",
        }
    }

    /// The invoice line label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Fahrstrom => "Stromsteuer (ermäßigt, Fahrstrom)",
            Self::Landstrom => "Stromsteuer (ermäßigt, Landstrom)",
        }
    }
}

/// How the Stromsteuer applies to one supply.
///
/// The default is [`Self::Regel`] — a supply is taxed unless a ground says
/// otherwise, which is the direction the statute runs in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum StromsteuerTarif {
    /// Regelsteuersatz nach § 3 StromStG (2,05 ct/kWh since 01.04.2003).
    ///
    /// On a supply that would qualify for § 9 Abs. 1 Nr. 1–3, this is the
    /// operator's Verzicht nach § 9 Abs. 1b StromStG — irrevocable, by taxing
    /// the quantity under § 3.
    ///
    /// An empty struct variant rather than a unit one: serde's
    /// `deny_unknown_fields` holds only for struct variants of an internally
    /// tagged enum, and the default treatment is the one a typo would hit.
    Regel {},
    /// Steuerbefreiung nach § 9 Abs. 1 StromStG.
    Befreiung {
        /// The Nr. of § 9 Abs. 1 the exemption rests on.
        grund: StromsteuerBefreiung,
    },
    /// Steuerbefreiung nach § 9 Abs. 1 Nr. 3 StromStG, with the facts it rests
    /// on; [`Kleinanlage::pruefen`] holds them to the statute.
    Kleinanlage(Kleinanlage),
    /// Ermäßigter Steuersatz nach § 9 Abs. 2 / Abs. 3 StromStG.
    Ermaessigung {
        /// The reduction claimed, which fixes the rate invoiced.
        grund: StromsteuerErmaessigung,
    },
}

impl Default for StromsteuerTarif {
    fn default() -> Self {
        Self::Regel {}
    }
}

impl StromsteuerTarif {
    /// The rate actually invoiced, given the standard rate for the period.
    #[must_use]
    pub fn rate_ct_per_kwh(&self, regelsatz_ct_per_kwh: Decimal) -> Decimal {
        match self {
            Self::Regel {} => regelsatz_ct_per_kwh,
            Self::Befreiung { .. } | Self::Kleinanlage(_) => Decimal::ZERO,
            Self::Ermaessigung { grund } => grund.rate_ct_per_kwh(),
        }
    }

    /// `true` when the supply carries no Stromsteuer at all.
    #[must_use]
    pub const fn is_befreit(&self) -> bool {
        matches!(self, Self::Befreiung { .. } | Self::Kleinanlage(_))
    }
}

/// The plant a § 9 Abs. 1 Nr. 3 StromStG supply comes from, and how it is
/// drawn — each fact one the statute or the StromStV makes the exemption
/// depend on, stated by the operator who knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Kleinanlage {
    /// The Energieträger the Anlage generates from.
    pub energietraeger: KleinanlageEnergietraeger,
    /// Elektrische Nennleistung of the Anlage in kW: every Stromerzeugungs-
    /// einheit of the same operator at the site with the same Energieträger
    /// (§ 12b Abs. 1 StromStV), not one module string.
    pub nennleistung_kw: Decimal,
    /// Who draws the electricity.
    pub entnahme: KleinanlageEntnahme,
    /// The farthest Entnahmestelle from its Stromerzeugungseinheit, in km —
    /// the räumliche Zusammenhang reaches 4,5 km (§ 12b Abs. 3 StromStV).
    pub entfernung_km: Decimal,
    /// The Hauptzollamt's förmliche Einzelerlaubnis (§ 9 Abs. 4 StromStG), by
    /// Aktenzeichen. Absent, the supply relies on the allgemeine Erlaubnis of
    /// § 10 Abs. 2 StromStV.
    #[serde(default)]
    pub erlaubnis_aktenzeichen: Option<String>,
}

/// The Energieträger § 9 Abs. 1 Nr. 3 StromStG names — „ausschließlich aus
/// Windkraft, Sonnenenergie, Erdwärme oder Wasserkraft oder in hocheffizienten
/// KWK-Anlagen". Biomass outside a KWK process is not among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KleinanlageEnergietraeger {
    /// Windkraft.
    Windkraft,
    /// Sonnenenergie.
    Sonnenenergie,
    /// Erdwärme.
    Erdwaerme,
    /// Wasserkraft.
    Wasserkraft,
    /// Hocheffiziente KWK-Anlage. Its allgemeine Erlaubnis also needs the
    /// Marktstammdatenregister entry (§ 10 Abs. 2 Satz 1 Nr. 2 StromStV).
    HocheffizienteKwk,
}

/// Who draws the electricity — the two Buchstaben of § 9 Abs. 1 Nr. 3 StromStG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum KleinanlageEntnahme {
    /// Buchst. a — the operator, as Eigenerzeuger, for its own consumption.
    Selbstverbrauch,
    /// Buchst. b — Letztverbraucher, supplied by whoever operates the plant
    /// or has it operated, with nobody else in the supply relationship
    /// (§ 12b Abs. 2 StromStV). A Lieferant that neither operates the plant
    /// nor has it operated cannot claim it.
    Letztverbraucher,
}

impl Kleinanlage {
    /// The two thresholds of § 9 Abs. 1 Nr. 3 StromStG and § 10 Abs. 2
    /// StromStV, in kW.
    const HOECHSTLEISTUNG_KW: Decimal = dec!(2000);
    const ALLGEMEINE_ERLAUBNIS_KW: Decimal = dec!(1000);
    const RADIUS_KM: Decimal = dec!(4.5);

    /// Why the stated facts do not carry the exemption, or `Ok` when they do.
    ///
    /// # Errors
    ///
    /// The sentence naming the rule the facts fail.
    #[must_use = "an unchecked claim bills no Stromsteuer on facts nobody held to the statute"]
    pub fn pruefen(&self) -> Result<(), String> {
        let kw = self.nennleistung_kw;
        if kw <= Decimal::ZERO {
            return Err(format!("Nennleistung {kw} kW ist keine Anlagenleistung"));
        }
        if kw > Self::HOECHSTLEISTUNG_KW {
            return Err(format!(
                "§ 9 Abs. 1 Nr. 3 StromStG trägt Anlagen bis 2 MW; die Anlage hat {kw} kW"
            ));
        }
        if self.entfernung_km < Decimal::ZERO || self.entfernung_km > Self::RADIUS_KM {
            return Err(format!(
                "der räumliche Zusammenhang reicht 4,5 km (§ 12b Abs. 3 StromStV); \
                 angegeben sind {} km",
                self.entfernung_km
            ));
        }
        let einzelerlaubnis = self
            .erlaubnis_aktenzeichen
            .as_deref()
            .is_some_and(|a| !a.trim().is_empty());
        if !einzelerlaubnis {
            let allgemein = match self.energietraeger {
                KleinanlageEnergietraeger::HocheffizienteKwk => kw < Self::ALLGEMEINE_ERLAUBNIS_KW,
                _ => kw <= Self::ALLGEMEINE_ERLAUBNIS_KW,
            };
            if !allgemein {
                return Err(format!(
                    "die allgemeine Erlaubnis des § 10 Abs. 2 StromStV reicht bis 1 MW; \
                     für {kw} kW braucht die Befreiung die Einzelerlaubnis des \
                     Hauptzollamts (§ 9 Abs. 4 StromStG) — erlaubnis_aktenzeichen fehlt"
                ));
            }
        }
        Ok(())
    }

    /// The Buchstabe the supply stands on.
    #[must_use]
    pub const fn citation(&self) -> &'static str {
        match self.entnahme {
            KleinanlageEntnahme::Selbstverbrauch => "§ 9 Abs. 1 Nr. 3 Buchst. a StromStG",
            KleinanlageEntnahme::Letztverbraucher => "§ 9 Abs. 1 Nr. 3 Buchst. b StromStG",
        }
    }

    /// The invoice line: the ground and the Erlaubnis it is claimed under.
    #[must_use]
    pub fn description(&self) -> String {
        let erlaubnis = match self.erlaubnis_aktenzeichen.as_deref().map(str::trim) {
            Some(az) if !az.is_empty() => format!("Erlaubnis des Hauptzollamts, Az. {az}"),
            _ => "allgemeine Erlaubnis nach § 10 Abs. 2 StromStV".to_owned(),
        };
        format!(
            "Stromsteuer: steuerfrei ({} — Anlage bis 2 MW, Entnahme im räumlichen \
             Zusammenhang; {erlaubnis})",
            self.citation()
        )
    }
}

// ── Energiesteuer (Erdgas) ────────────────────────────────────────────────────

/// §§ 25–28 EnergieStG — grounds on which gas is supplied **untaxed**.
///
/// Untaxed supply requires the customer to hold an Erlaubnis als Verwender
/// (§ 24 Abs. 2 EnergieStG); the supplier bills no Energiesteuer and records
/// the Erlaubnisnummer. Without an Erlaubnis the supply is taxed in full and
/// any relief the customer is entitled to is a [`Steuerentlastung`], claimed
/// afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnergiesteuerBefreiung {
    /// § 25 EnergieStG — Verwendung zu anderen Zwecken als als Kraft- oder
    /// Heizstoff (stoffliche Verwendung, z. B. als Reduktionsmittel).
    StofflicheVerwendung,
    /// § 26 EnergieStG — Eigenverbrauch im Herstellerbetrieb.
    HerstellerbetriebEigenverbrauch,
    /// § 27 EnergieStG — Schiff- und Luftfahrt.
    SchiffUndLuftfahrt,
    /// § 28 EnergieStG — gasförmige Energieerzeugnisse aus Biomasse,
    /// Deponie-, Klär- und Grubengas.
    GasfoermigeBiogeneErzeugnisse,
}

impl EnergiesteuerBefreiung {
    /// The § reference to print on the invoice.
    #[must_use]
    pub const fn citation(self) -> &'static str {
        match self {
            Self::StofflicheVerwendung => "§ 25 EnergieStG",
            Self::HerstellerbetriebEigenverbrauch => "§ 26 EnergieStG",
            Self::SchiffUndLuftfahrt => "§ 27 EnergieStG",
            Self::GasfoermigeBiogeneErzeugnisse => "§ 28 EnergieStG",
        }
    }

    /// The invoice line describing the exemption.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::StofflicheVerwendung => {
                "Energiesteuer: steuerfreie Verwendung (§ 25 EnergieStG i. V. m. \
                 § 24 Abs. 2 EnergieStG — Erlaubnisschein liegt vor)"
            }
            Self::HerstellerbetriebEigenverbrauch => {
                "Energiesteuer: steuerfreie Verwendung (§ 26 EnergieStG — Eigenverbrauch \
                 im Herstellerbetrieb)"
            }
            Self::SchiffUndLuftfahrt => {
                "Energiesteuer: steuerfreie Verwendung (§ 27 EnergieStG — Schiff- und Luftfahrt)"
            }
            Self::GasfoermigeBiogeneErzeugnisse => {
                "Energiesteuer: steuerfrei (§ 28 EnergieStG — gasförmige Energieerzeugnisse \
                 aus Biomasse bzw. Deponie-, Klär- und Grubengas)"
            }
        }
    }
}

/// How the Energiesteuer applies to one gas supply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
pub enum EnergiesteuerTarif {
    /// Regelsteuersatz für Erdgas als Heizstoff (§ 2 Abs. 3 Satz 1 Nr. 4
    /// EnergieStG — 5,50 EUR/MWh). An empty struct variant so an unknown key
    /// is refused (see [`StromsteuerTarif::Regel`]).
    Regel {},
    /// Steuerfreie Verwendung gegen Erlaubnisschein (§ 24 Abs. 2 EnergieStG).
    Befreiung {
        /// The ground of the steuerfreie Verwendung, which also fixes the invoice note.
        grund: EnergiesteuerBefreiung,
    },
}

impl Default for EnergiesteuerTarif {
    fn default() -> Self {
        Self::Regel {}
    }
}

impl EnergiesteuerTarif {
    /// The rate actually invoiced, given the standard rate for the period.
    #[must_use]
    pub fn rate_ct_per_kwh(self, regelsatz_ct_per_kwh: Decimal) -> Decimal {
        match self {
            Self::Regel {} => regelsatz_ct_per_kwh,
            Self::Befreiung { .. } => Decimal::ZERO,
        }
    }

    /// `true` when the supply carries no Energiesteuer at all.
    #[must_use]
    pub const fn is_befreit(self) -> bool {
        matches!(self, Self::Befreiung { .. })
    }
}

// ── Steuerentlastung ──────────────────────────────────────────────────────────

/// A relief the **customer** claims after the fact, at the Hauptzollamt.
///
/// It changes nothing about what the supplier invoices — the supply is taxed in
/// full and the customer files for a refund. Recorded on the product so the
/// invoice can *tell* the customer what it carried and under which provision
/// they may reclaim it; the filing itself is theirs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Steuerentlastung {
    /// § 9b StromStG — Unternehmen des Produzierenden Gewerbes und der Land-
    /// und Forstwirtschaft. Permanent at the EU minimum rate since 01.01.2026:
    /// 20,00 EUR/MWh of the 20,50 EUR/MWh is refundable, from 12 500 kWh a year.
    Stromsteuer9b,
    /// § 9a StromStG — Strom für bestimmte Prozesse und Verfahren
    /// (Elektrolyse, Metallerzeugung, Glas, Keramik, Zement …).
    Stromsteuer9a,
    /// § 9c StromStG — öffentlicher Personennahverkehr.
    Stromsteuer9c,
    /// § 53a EnergieStG — gekoppelte Erzeugung von Kraft und Wärme. Only the
    /// partial relief (Abs. 1 / Abs. 4) remains available for use from
    /// 01.01.2024; the full relief under Abs. 6 does not.
    Energiesteuer53a,
    /// § 54 EnergieStG — Unternehmen des Produzierenden Gewerbes und der Land-
    /// und Forstwirtschaft (rund ein Viertel der Steuer auf Erdgas).
    Energiesteuer54,
}

impl Steuerentlastung {
    /// The § reference to print on the invoice.
    #[must_use]
    pub const fn citation(self) -> &'static str {
        match self {
            Self::Stromsteuer9b => "§ 9b StromStG",
            Self::Stromsteuer9a => "§ 9a StromStG",
            Self::Stromsteuer9c => "§ 9c StromStG",
            Self::Energiesteuer53a => "§ 53a EnergieStG",
            Self::Energiesteuer54 => "§ 54 EnergieStG",
        }
    }

    /// Which levy the relief is claimed against — decides which levy total the
    /// invoice note quantifies.
    #[must_use]
    pub const fn levy_tag(self) -> &'static str {
        match self {
            Self::Stromsteuer9a | Self::Stromsteuer9b | Self::Stromsteuer9c => "stromsteuer",
            Self::Energiesteuer53a | Self::Energiesteuer54 => "energiesteuer_gas",
        }
    }

    /// The invoice note telling the customer what they may reclaim, and where.
    #[must_use]
    pub const fn hinweis(self) -> &'static str {
        match self {
            Self::Stromsteuer9b => {
                "Hinweis: Für die ausgewiesene Stromsteuer kommt eine Steuerentlastung nach \
                 § 9b StromStG in Betracht (Antrag beim Hauptzollamt, ab 12 500 kWh/Jahr). \
                 Die Lieferung wird in voller Höhe versteuert."
            }
            Self::Stromsteuer9a => {
                "Hinweis: Für die ausgewiesene Stromsteuer kommt eine Steuerentlastung nach \
                 § 9a StromStG (begünstigte Prozesse und Verfahren) in Betracht — Antrag \
                 beim Hauptzollamt. Die Lieferung wird in voller Höhe versteuert."
            }
            Self::Stromsteuer9c => {
                "Hinweis: Für die ausgewiesene Stromsteuer kommt eine Steuerentlastung nach \
                 § 9c StromStG (öffentlicher Personennahverkehr) in Betracht — Antrag beim \
                 Hauptzollamt. Die Lieferung wird in voller Höhe versteuert."
            }
            Self::Energiesteuer53a => {
                "Hinweis: Für die ausgewiesene Energiesteuer kommt eine teilweise \
                 Steuerentlastung nach § 53a Abs. 1/Abs. 4 EnergieStG (KWK) in Betracht — \
                 Antrag beim Hauptzollamt. Die Lieferung wird in voller Höhe versteuert."
            }
            Self::Energiesteuer54 => {
                "Hinweis: Für die ausgewiesene Energiesteuer kommt eine Steuerentlastung nach \
                 § 54 EnergieStG in Betracht — Antrag beim Hauptzollamt. Die Lieferung wird \
                 in voller Höhe versteuert."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The distinction this module exists for: an Entlastung never moves an
    /// amount, an Ermäßigung moves it to the statutory rate, and only a
    /// Befreiung removes the levy.
    /// A mistyped key in a tax treatment is refused, never ignored: the
    /// treatment decides the levy, and a silently dropped field reads as the
    /// operator's statement.
    #[test]
    fn a_tax_treatment_with_an_unknown_key_is_refused() {
        let strom = |v: serde_json::Value| serde_json::from_value::<StromsteuerTarif>(v);
        assert!(strom(serde_json::json!({ "art": "REGEL" })).is_ok());
        assert!(strom(serde_json::json!({ "art": "BEFREIUNG", "grund": "NOTSTROM" })).is_ok());
        assert!(strom(serde_json::json!({ "art": "REGEL", "grnd": "x" })).is_err());
        assert!(
            strom(serde_json::json!({ "art": "BEFREIUNG", "grund": "NOTSTROM", "nr": 3 })).is_err()
        );
        let gas = |v: serde_json::Value| serde_json::from_value::<EnergiesteuerTarif>(v);
        assert!(gas(serde_json::json!({ "art": "REGEL" })).is_ok());
        assert!(gas(serde_json::json!({ "art": "REGEL", "satz": "0" })).is_err());
    }

    #[test]
    fn only_a_befreiung_zeroes_the_levy() {
        let regel = dec!(2.05);
        assert_eq!(StromsteuerTarif::Regel {}.rate_ct_per_kwh(regel), regel);
        assert_eq!(
            StromsteuerTarif::Befreiung {
                grund: StromsteuerBefreiung::Notstrom
            }
            .rate_ct_per_kwh(regel),
            Decimal::ZERO
        );
        assert_eq!(
            StromsteuerTarif::Kleinanlage(dach(dec!(99))).rate_ct_per_kwh(regel),
            Decimal::ZERO
        );
        assert_eq!(
            StromsteuerTarif::Ermaessigung {
                grund: StromsteuerErmaessigung::Fahrstrom
            }
            .rate_ct_per_kwh(regel),
            dec!(1.142)
        );
        assert_eq!(
            StromsteuerTarif::Ermaessigung {
                grund: StromsteuerErmaessigung::Landstrom
            }
            .rate_ct_per_kwh(regel),
            dec!(0.05)
        );
    }

    /// A `Steuerentlastung` carries no rate at all — there is nothing on it
    /// that could be mistaken for one.
    #[test]
    fn an_entlastung_has_no_rate() {
        // Compile-time property, asserted by construction: the only things a
        // Steuerentlastung answers are a citation, a levy tag and a note.
        assert_eq!(Steuerentlastung::Stromsteuer9b.citation(), "§ 9b StromStG");
        assert_eq!(Steuerentlastung::Stromsteuer9b.levy_tag(), "stromsteuer");
        assert_eq!(
            Steuerentlastung::Energiesteuer54.levy_tag(),
            "energiesteuer_gas"
        );
        assert!(
            Steuerentlastung::Energiesteuer53a
                .hinweis()
                .contains("§ 53a")
        );
    }

    /// The default is "taxed" in both directions — a product that says nothing
    /// gets the standard rate, never a silent exemption.
    #[test]
    fn the_default_is_taxed() {
        assert_eq!(StromsteuerTarif::default(), StromsteuerTarif::Regel {});
        assert_eq!(EnergiesteuerTarif::default(), EnergiesteuerTarif::Regel {});
        assert!(!StromsteuerTarif::default().is_befreit());
        assert!(!EnergiesteuerTarif::default().is_befreit());
    }

    /// A rooftop plant supplying the building's tenants under the allgemeine
    /// Erlaubnis.
    fn dach(kw: Decimal) -> Kleinanlage {
        Kleinanlage {
            energietraeger: KleinanlageEnergietraeger::Sonnenenergie,
            nennleistung_kw: kw,
            entnahme: KleinanlageEntnahme::Letztverbraucher,
            entfernung_km: dec!(0.2),
            erlaubnis_aktenzeichen: None,
        }
    }

    /// The statute's thresholds, each on both sides of its line.
    #[test]
    fn a_kleinanlage_claim_is_held_to_its_thresholds() {
        assert_eq!(dach(dec!(1000)).pruefen(), Ok(()));
        let err = dach(dec!(1000.1)).pruefen().unwrap_err();
        assert!(err.contains("§ 10 Abs. 2 StromStV"), "{err}");
        let mut einzel = dach(dec!(2000));
        einzel.erlaubnis_aktenzeichen = Some("E-123/26".to_owned());
        assert_eq!(einzel.pruefen(), Ok(()));
        assert!(einzel.description().contains("Az. E-123/26"));
        einzel.nennleistung_kw = dec!(2000.1);
        assert!(einzel.pruefen().unwrap_err().contains("bis 2 MW"));

        let mut weit = dach(dec!(99));
        weit.entfernung_km = dec!(4.5);
        assert_eq!(weit.pruefen(), Ok(()));
        weit.entfernung_km = dec!(4.6);
        assert!(
            weit.pruefen()
                .unwrap_err()
                .contains("§ 12b Abs. 3 StromStV")
        );

        // A KWK plant's allgemeine Erlaubnis stops below 1 MW, not at it.
        let mut kwk = dach(dec!(1000));
        kwk.energietraeger = KleinanlageEnergietraeger::HocheffizienteKwk;
        assert!(kwk.pruefen().is_err());
        kwk.nennleistung_kw = dec!(999.9);
        assert_eq!(kwk.pruefen(), Ok(()));
        assert_eq!(
            dach(dec!(99)).citation(),
            "§ 9 Abs. 1 Nr. 3 Buchst. b StromStG"
        );
    }

    /// Serde round-trip: the tagged representation is what `productd` stores.
    #[test]
    fn tagged_serde_round_trip() {
        let t = StromsteuerTarif::Befreiung {
            grund: StromsteuerBefreiung::Notstrom,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"art":"BEFREIUNG","grund":"NOTSTROM"}"#);
        assert_eq!(serde_json::from_str::<StromsteuerTarif>(&json).unwrap(), t);

        let k: StromsteuerTarif = serde_json::from_str(
            r#"{"art":"KLEINANLAGE","energietraeger":"SONNENENERGIE","nennleistung_kw":"99",
                "entnahme":"LETZTVERBRAUCHER","entfernung_km":"0.2"}"#,
        )
        .unwrap();
        assert_eq!(k, StromsteuerTarif::Kleinanlage(dach(dec!(99))));
        assert!(
            serde_json::from_str::<StromsteuerTarif>(
                r#"{"art":"KLEINANLAGE","energietraeger":"SONNENENERGIE","nennleistung_kw":"99",
                    "entnahme":"LETZTVERBRAUCHER","entfernung_km":"0.2","kwp":"99"}"#,
            )
            .is_err(),
            "an unknown fact is refused, not dropped"
        );

        let e = EnergiesteuerTarif::Befreiung {
            grund: EnergiesteuerBefreiung::StofflicheVerwendung,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(
            json,
            r#"{"art":"BEFREIUNG","grund":"STOFFLICHE_VERWENDUNG"}"#
        );
        assert_eq!(
            serde_json::from_str::<EnergiesteuerTarif>(&json).unwrap(),
            e
        );
    }
}
