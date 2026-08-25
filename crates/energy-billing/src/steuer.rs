//! Verbrauchsteuerliche Begünstigungen — StromStG and EnergieStG.
//!
//! German excise law knows **three** different things, and only two of them
//! change what a supplier may invoice:
//!
//! | Instrument | Who acts | Effect on the invoice |
//! |---|---|---|
//! | **Steuerbefreiung** (§ 9 Abs. 1 StromStG, §§ 25–29 EnergieStG) | the supplier, against the customer's Erlaubnis | the levy is **not** invoiced |
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

/// § 9 Abs. 1 StromStG — grounds on which a supply carries **no** Stromsteuer.
///
/// Every ground requires the customer to hold the corresponding Erlaubnis
/// (§ 9 Abs. 4 StromStG) and the supplier to have it on file; the enum records
/// which one was claimed so the invoice can cite it.
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
    /// Nr. 3 — Anlagen bis 2 MW aus erneuerbaren Energieträgern oder
    /// hocheffizienter KWK, zum Selbstverbrauch oder an Letztverbraucher im
    /// räumlichen Zusammenhang zur Anlage geleistet.
    ///
    /// This is the ground a rooftop-PV self-consumption and a Quartiers-BHKW
    /// actually stand on — **not** § 9a, which is a Steuerentlastung for
    /// industrial processes.
    Kleinanlage,
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
            Self::Kleinanlage => "§ 9 Abs. 1 Nr. 3 StromStG",
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
            Self::Kleinanlage => {
                "Stromsteuer: steuerfrei (§ 9 Abs. 1 Nr. 3 StromStG — Anlage bis 2 MW, \
                 Selbstverbrauch bzw. Belieferung im räumlichen Zusammenhang)"
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StromsteuerTarif {
    /// Regelsteuersatz nach § 3 StromStG (2,05 ct/kWh since 01.04.2003).
    #[default]
    Regel,
    /// Steuerbefreiung nach § 9 Abs. 1 StromStG.
    Befreiung { grund: StromsteuerBefreiung },
    /// Ermäßigter Steuersatz nach § 9 Abs. 2 / Abs. 3 StromStG.
    Ermaessigung { grund: StromsteuerErmaessigung },
}

impl StromsteuerTarif {
    /// The rate actually invoiced, given the standard rate for the period.
    #[must_use]
    pub fn rate_ct_per_kwh(self, regelsatz_ct_per_kwh: Decimal) -> Decimal {
        match self {
            Self::Regel => regelsatz_ct_per_kwh,
            Self::Befreiung { .. } => Decimal::ZERO,
            Self::Ermaessigung { grund } => grund.rate_ct_per_kwh(),
        }
    }

    /// `true` when the supply carries no Stromsteuer at all.
    #[must_use]
    pub const fn is_befreit(self) -> bool {
        matches!(self, Self::Befreiung { .. })
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "art", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnergiesteuerTarif {
    /// Regelsteuersatz für Erdgas als Heizstoff (§ 2 Abs. 3 Satz 1 Nr. 4
    /// EnergieStG — 5,50 EUR/MWh).
    #[default]
    Regel,
    /// Steuerfreie Verwendung gegen Erlaubnisschein (§ 24 Abs. 2 EnergieStG).
    Befreiung { grund: EnergiesteuerBefreiung },
}

impl EnergiesteuerTarif {
    /// The rate actually invoiced, given the standard rate for the period.
    #[must_use]
    pub fn rate_ct_per_kwh(self, regelsatz_ct_per_kwh: Decimal) -> Decimal {
        match self {
            Self::Regel => regelsatz_ct_per_kwh,
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
    #[test]
    fn only_a_befreiung_zeroes_the_levy() {
        let regel = dec!(2.05);
        assert_eq!(StromsteuerTarif::Regel.rate_ct_per_kwh(regel), regel);
        assert_eq!(
            StromsteuerTarif::Befreiung {
                grund: StromsteuerBefreiung::Kleinanlage
            }
            .rate_ct_per_kwh(regel),
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
        assert_eq!(StromsteuerTarif::default(), StromsteuerTarif::Regel);
        assert_eq!(EnergiesteuerTarif::default(), EnergiesteuerTarif::Regel);
        assert!(!StromsteuerTarif::default().is_befreit());
        assert!(!EnergiesteuerTarif::default().is_befreit());
    }

    /// Serde round-trip: the tagged representation is what `productd` stores.
    #[test]
    fn tagged_serde_round_trip() {
        let t = StromsteuerTarif::Befreiung {
            grund: StromsteuerBefreiung::Kleinanlage,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"art":"BEFREIUNG","grund":"KLEINANLAGE"}"#);
        assert_eq!(serde_json::from_str::<StromsteuerTarif>(&json).unwrap(), t);

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
