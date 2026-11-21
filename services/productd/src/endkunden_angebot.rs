//! The frozen **Endkunden-Angebot** — the binding B2C offer an online order is
//! placed against.
//!
//! `POST /api/v1/products/{lf_mp_id}/endkunden-angebote` prices one product for
//! one supply point and freezes the result: the product version, the
//! Netzbetreiber's Netzentgelte, Messentgelt and Konzessionsabgabe valid at the
//! Lieferbeginn (read from `marktd`), the Strom- or Energiesteuer, the BEHG
//! CO₂ price and the netzseitigen Umlagen from their dated series, the
//! Umsatzsteuer, the all-in Arbeits- and Grundpreis, the Jahreskosten and the
//! monthly Abschlag. `vertragd` names the offer by id and hash when the
//! consumer orders, so the offer is stored append-only and is never re-quoted.
//!
//! # A missing input refuses
//!
//! Every input is read, none is assumed: a Preisblatt `marktd` does not hold, a
//! position the sheet does not carry, a year the tax or Umlage series has not
//! read, a product not in force or not published — each is named in one `422
//! eingabe_fehlt` with `fehlend: [...]`. A price taken as zero would misprice
//! every kWh of the offer the consumer then orders.
//!
//! # Which sheets are read
//!
//! | Input | Source |
//! |---|---|
//! | Netzbetreiber | the caller's `nb_mp_id`, or `marktd` `GET /api/v1/malos/{id}/grid` for the MaLo |
//! | Netzentgelt Arbeits- und Grundpreis | `marktd` `GET /api/v1/preisblaetter/{nb}?date=` — positions `ARBEITSPREIS_WIRKARBEIT`, `GRUNDPREIS` |
//! | Grundzuständiger Messstellenbetreiber | `marktd` `GET /api/v1/netzbetreiber/{nb}/grundzustaendiger-msb` — the MP-ID the gMSB of the NB's grid acts under, its own and not the NB's |
//! | Messstellenbetrieb | `marktd` `GET /api/v1/preisblaetter-messung/{gmsb}?date=` — the gMSB's sheet, position `MESSSTELLENBETRIEB` |
//! | Konzessionsabgabe | `marktd` `GET /api/v1/preisblaetter-ka/{nb}?date=&sparte=&kundengruppe=` — position `KONZESSIONS_ABGABE` |
//! | Strom: § 19 StromNEV-, Offshore-Netz-, KWKG-Umlage | `grid_billing::umlagen`, Letztverbrauchergruppe A′ |
//! | Strom: Stromsteuer | `grid_billing::stromsteuer_for_year` (§ 3 StromStG) |
//! | Gas: Energiesteuer, BEHG | `energy_billing::energiesteuer_gas_for_year`, the nEHS series and billingd's `behg_regime` |
//!
//! Every sheet is the one valid on the **gewünschte Lieferbeginn**, and every
//! dated series is read for that day's year: the offer prices the first year of
//! supply.
//!
//! The Konzessionsabgabe sheet for Strom is the Tarifkunden one: KAV § 2 Abs. 7
//! Satz 1 makes a delivery from the Niederspannungsnetz a Tarifkunden-Lieferung
//! „es sei denn, die gemessene Leistung des Kunden überschreitet in mindestens
//! zwei Monaten des Abrechnungsjahres 30 Kilowatt und der Jahresverbrauch
//! beträgt mehr als 30.000 Kilowattstunden". At or below 30 000 kWh the
//! exception cannot hold; above it the measured demand decides, which an offer
//! does not know, so it refuses. For Gas no such rule picks the group, so only
//! a sheet that applies to every Kundengruppe prices the offer.
//!
//! # The hash
//!
//! `content_sha256` is the SHA-256 of the RFC 8785 (JSON Canonicalization
//! Scheme) bytes of the offer body — every field of the answer except `id`,
//! `content_sha256` and `created_at`: `lf_mp_id`, `product_code`,
//! `product_version`, `sparte`, `malo_id`, `nb_mp_id`, `verbrauch_kwh`,
//! `gewuenschter_lieferbeginn`, `lieferbeginn_fruehestens`, `preise`,
//! `jahreskosten_brutto_eur`, `abschlag_monat_brutto_eur`, `gueltig_bis`.
//! Every amount and date in it is a string, so no number formatting stands
//! between two serialisers. The bytes are stored and the answer is read back
//! out of them, so a third party recomputes the hash over what it was served.
//!
//! `product_version` is the same hash over the product's stored `data` — the
//! Tarifpreisblatt version the offer priced.

use std::sync::Arc;

use grid_billing::RoundMoney as _;
use mako_service::{ApiError, http::Upstream};
use rubo4e::current::{Leistungstyp, Mengeneinheit, Preisposition, Waehrungseinheit};
use rust_decimal::{Decimal, dec};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use sqlx::{PgPool, Row as _};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::handlers::PricingContext;

/// How long an Endkunden-Angebot binds: to the end of the fourteenth Berlin
/// calendar day after it is made — the productd default the B2B Angebote
/// carry, and no regulatory Frist. An order after it is refused by `vertragd`
/// and the front end asks for a new offer; the price is never re-quoted.
pub const GUELTIGKEIT_TAGE: i64 = 14;

/// Above this Strom Jahresverbrauch the Konzessionsabgabe's Kundengruppe is
/// decided by the measured demand (KAV § 2 Abs. 7 Satz 1), which an offer does
/// not know.
pub const KA_TARIFKUNDEN_GRENZE_KWH: Decimal = dec!(30000);

/// The stable `code` of every refusal of this route.
pub mod code {
    /// An input is missing; `fehlend` names each one.
    pub const EINGABE_FEHLT: &str = "eingabe_fehlt";
    /// The MaLo-ID fails its check digit (or its format).
    pub const MALO_ID_UNGUELTIG: &str = "malo_id_ungueltig";
    /// The `nb_mp_id` is not a 13-digit Marktpartner-ID.
    pub const NB_MP_ID_UNGUELTIG: &str = "nb_mp_id_ungueltig";
    /// More than one of `malo_id`, `nb_mp_id`, `plz` was given.
    pub const NB_QUELLE_MEHRDEUTIG: &str = "nb_quelle_mehrdeutig";
    /// The Netzbetreiber by Postleitzahl is not offered by this build.
    pub const PLZ_NICHT_UNTERSTUETZT: &str = "plz_nicht_unterstuetzt";
    /// `verbrauch_kwh` is not positive.
    pub const VERBRAUCH_UNGUELTIG: &str = "verbrauch_ungueltig";
    /// `product_code` is empty.
    pub const PRODUKT_FEHLT: &str = "produkt_fehlt";
    /// The desired Lieferbeginn lies before today.
    pub const LIEFERBEGINN_VERGANGEN: &str = "lieferbeginn_in_der_vergangenheit";
    /// The product, or the MaLo, belongs to another Sparte than the request.
    pub const SPARTE_ABWEICHEND: &str = "sparte_abweichend";
    /// No `marktd_url` is configured, so no Preisblatt can be read.
    pub const MARKTD_NICHT_KONFIGURIERT: &str = "marktd_nicht_konfiguriert";
    /// The `vertriebspreis` does not fit its basis product (a band missing
    /// or extra, a negative price, too many decimals).
    pub const VERTRIEBSPREIS_UNGUELTIG: &str = "vertriebspreis_ungueltig";
    /// The basis product cannot carry a front end's price: not published, a
    /// contract price itself, or dynamic (§ 41a EnWG).
    pub const BASIS_UNGEEIGNET: &str = "basis_ungeeignet";
    /// What the front end showed is not what mako prices; `abweichungen`
    /// names each line with both values.
    pub const ANGEZEIGT_ABWEICHEND: &str = "angezeigt_abweichend";
}

/// Strom or Gas — the two Sparten an Endkunden-Angebot is made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Sparte {
    Strom,
    Gas,
}

impl Sparte {
    /// The wire and column value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strom => "STROM",
            Self::Gas => "GAS",
        }
    }
}

/// `POST /api/v1/products/{lf_mp_id}/endkunden-angebote` — the body.
///
/// Exactly one of `malo_id`, `nb_mp_id`, `plz`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndkundenAngebotAnfrage {
    pub product_code: String,
    pub sparte: Sparte,
    /// The annual consumption the offer is priced for, kWh, a decimal string.
    pub verbrauch_kwh: Decimal,
    /// The Marktlokation; the Netzbetreiber is read for it from `marktd`.
    #[serde(default)]
    pub malo_id: Option<String>,
    /// The Netzbetreiber, when the caller knows it and not the MaLo.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    /// The Postleitzahl — refused `plz_nicht_unterstuetzt` by this build.
    #[serde(default)]
    pub plz: Option<String>,
    /// The day supply should start; every sheet and series is read for it.
    pub gewuenschter_lieferbeginn: Date,
    /// A front end's own price on `product_code` as the basis: the offer is
    /// priced from it instead of the catalogue's Energie prices.
    #[serde(default)]
    pub vertriebspreis: Option<Vertriebspreis>,
}

/// The price a front end sold — the supplier's own lines, net. Every
/// regulated line (Netzentgelte, Messstellenbetrieb, Konzessionsabgabe,
/// Umlagen, Steuern) is read from its source, as for a catalogue product.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vertriebspreis {
    /// Arbeitspreis Energie, ct/kWh, Eintarif basis.
    #[serde(default)]
    pub arbeitspreis_ct_kwh: Option<Decimal>,
    /// Arbeitspreis HT and NT, ct/kWh, Zweitarif basis.
    #[serde(default)]
    pub arbeitspreis_ht_ct_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_nt_ct_kwh: Option<Decimal>,
    /// Grundpreis Energie, EUR per year.
    pub grundpreis_eur_jahr: Decimal,
    /// The front end's own reference (its digest of what it sold), echoed.
    #[serde(default)]
    pub referenz: Option<String>,
    /// What the customer was shown, gross; checked at its own precision.
    #[serde(default)]
    pub angezeigt: Option<Angezeigt>,
}

/// The all-in prices and Jahreskosten a front end showed, gross.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Angezeigt {
    #[serde(default)]
    pub arbeitspreis_brutto_ct_kwh: Option<Decimal>,
    #[serde(default)]
    pub grundpreis_brutto_eur_jahr: Option<Decimal>,
    #[serde(default)]
    pub jahreskosten_brutto_eur: Option<Decimal>,
}

/// The Tarifpreisblatt of a contract price: `basis` with the Energie prices
/// replaced by the handed-over ones, one unbounded staffel each. The
/// Grundpreis is stored in the catalogue's unit, ct per day.
///
/// # Errors
///
/// [`code::VERTRIEBSPREIS_UNGUELTIG`] naming what does not fit.
pub fn vertragspreis_daten(
    basis: &serde_json::Value,
    vp: &Vertriebspreis,
) -> Result<serde_json::Value, Ablehnung> {
    let ungueltig = |m: String| Ablehnung::neu(code::VERTRIEBSPREIS_UNGUELTIG, m);
    let positionen = basis
        .get("tarifpreise")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| ungueltig("das Basisprodukt führt kein Tarifpreisblatt".to_owned()))?;
    let hat = |pt: &str| {
        positionen
            .iter()
            .any(|p| mako_markt::bo4e::position_preistyp(p) == pt)
    };
    let zweitarif = hat("ARBEITSPREIS_HT") || hat("ARBEITSPREIS_NT");
    let mut preise: Vec<(&str, Decimal)> = Vec::new();
    match (
        zweitarif,
        vp.arbeitspreis_ct_kwh,
        vp.arbeitspreis_ht_ct_kwh,
        vp.arbeitspreis_nt_ct_kwh,
    ) {
        (false, Some(ap), None, None) => preise.push(("ARBEITSPREIS_EINTARIF", ap)),
        (true, None, Some(ht), Some(nt)) => {
            preise.push(("ARBEITSPREIS_HT", ht));
            preise.push(("ARBEITSPREIS_NT", nt));
        }
        (false, ..) => {
            return Err(ungueltig(
                "das Basisprodukt ist ein Eintarif: genau arbeitspreis_ct_kwh, ohne HT/NT"
                    .to_owned(),
            ));
        }
        (true, ..) => {
            return Err(ungueltig(
                "das Basisprodukt ist ein Zweitarif: arbeitspreis_ht_ct_kwh und arbeitspreis_nt_ct_kwh, \
                 ohne arbeitspreis_ct_kwh"
                    .to_owned(),
            ));
        }
    }
    for (pt, wert) in &preise {
        if wert.is_sign_negative() || wert.scale() > 4 {
            return Err(ungueltig(format!(
                "{pt} {wert}: ein Arbeitspreis ist nicht negativ und hat höchstens 4 Nachkommastellen"
            )));
        }
    }
    let gp = vp.grundpreis_eur_jahr;
    if gp.is_sign_negative() || gp.scale() > 2 {
        return Err(ungueltig(format!(
            "grundpreis_eur_jahr {gp}: nicht negativ, höchstens 2 Nachkommastellen"
        )));
    }
    if !hat("GRUNDPREIS") && !gp.is_zero() {
        return Err(ungueltig(
            "das Basisprodukt führt keinen Grundpreis; grundpreis_eur_jahr ist 0".to_owned(),
        ));
    }
    // The catalogue's Grundpreis is ct per day (`GRUNDPREIS × 365 / 100` is
    // EUR per year), so the EUR-per-year figure is stored in that unit.
    let gp_ct_tag = (gp * Decimal::ONE_HUNDRED / dec!(365)).round_kfm(10);
    if hat("GRUNDPREIS") {
        preise.push(("GRUNDPREIS", gp_ct_tag));
    }
    let mut data = basis.clone();
    for p in data["tarifpreise"].as_array_mut().into_iter().flatten() {
        let pt = mako_markt::bo4e::position_preistyp(p).to_owned();
        if let Some((_, wert)) = preise.iter().find(|(t, _)| *t == pt) {
            p["preisstaffeln"] = serde_json::json!([{ "preis": wert.normalize().to_string() }]);
        }
    }
    Ok(data)
}

/// The lines whose stated value differs from mako's at the stated precision —
/// empty when what the customer saw is what mako prices.
#[must_use]
pub fn abweichungen(angebot: &Angebot, gezeigt: &Angezeigt) -> Vec<serde_json::Value> {
    let zeile = |art: &str| {
        angebot
            .preise
            .iter()
            .find(|z| z.art == art)
            .and_then(|z| z.wert.parse::<Decimal>().ok())
    };
    let pruefe = |name: &str, mako: Option<Decimal>, gezeigt: Option<Decimal>| {
        let (Some(gezeigt), Some(mako)) = (gezeigt, mako) else {
            return None;
        };
        let gerundet = mako.round_kfm(gezeigt.scale());
        (gerundet != gezeigt).then(|| {
            serde_json::json!({
                "zeile": name,
                "mako": gerundet.to_string(),
                "angezeigt": gezeigt.to_string(),
            })
        })
    };
    [
        pruefe(
            "arbeitspreis_brutto_ct_kwh",
            zeile("ARBEITSPREIS_GESAMT_BRUTTO"),
            gezeigt.arbeitspreis_brutto_ct_kwh,
        ),
        pruefe(
            "grundpreis_brutto_eur_jahr",
            zeile("GRUNDPREIS_GESAMT_BRUTTO"),
            gezeigt.grundpreis_brutto_eur_jahr,
        ),
        pruefe(
            "jahreskosten_brutto_eur",
            Some(angebot.jahreskosten_brutto_eur),
            gezeigt.jahreskosten_brutto_eur,
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Where the Netzbetreiber comes from, after the request has been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NbQuelle {
    /// A MaLo-ID with a valid check digit; `marktd` names its NB.
    Marktlokation(String),
    /// The caller named the NB.
    Netzbetreiber(String),
}

/// A refusal of this route: a `422` with a stable `code` and its fields.
#[derive(Debug, Clone, PartialEq)]
pub struct Ablehnung {
    pub code: &'static str,
    pub meldung: String,
    pub felder: serde_json::Value,
}

impl Ablehnung {
    fn neu(code: &'static str, meldung: impl Into<String>) -> Self {
        Self {
            code,
            meldung: meldung.into(),
            felder: serde_json::json!({}),
        }
    }

    /// The missing inputs, named.
    #[must_use]
    pub fn fehlt(fehlend: Vec<String>) -> Self {
        Self {
            code: code::EINGABE_FEHLT,
            meldung: "das Angebot kann nicht bepreist werden: Eingaben fehlen".to_owned(),
            felder: serde_json::json!({ "fehlend": fehlend }),
        }
    }

    /// The names in `fehlend`, empty for another refusal.
    #[must_use]
    pub fn fehlend(&self) -> Vec<String> {
        self.felder
            .get("fehlend")
            .and_then(serde_json::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl From<Ablehnung> for ApiError {
    fn from(a: Ablehnung) -> Self {
        Self::unprocessable(a.meldung)
            .with_code(a.code)
            .with_fields(a.felder)
    }
}

/// Check the request before anything is read or stored.
///
/// # Errors
///
/// [`Ablehnung`] for a wrong MaLo check digit, a malformed `nb_mp_id`, a
/// missing or ambiguous NB source, a `plz`, a non-positive consumption, an
/// empty product code or a Lieferbeginn before `heute`.
pub fn pruefe_anfrage(a: &EndkundenAngebotAnfrage, heute: Date) -> Result<NbQuelle, Ablehnung> {
    let quellen = [a.malo_id.is_some(), a.nb_mp_id.is_some(), a.plz.is_some()]
        .into_iter()
        .filter(|q| *q)
        .count();
    if quellen > 1 {
        return Err(Ablehnung::neu(
            code::NB_QUELLE_MEHRDEUTIG,
            "genau eines von malo_id, nb_mp_id und plz ist anzugeben",
        ));
    }
    let quelle = if let Some(malo) = &a.malo_id {
        rubo4e::identifiers::MaloId::new(malo).map_err(|e| {
            Ablehnung::neu(code::MALO_ID_UNGUELTIG, format!("malo_id {malo}: {e}"))
                .mit_feld("malo_id", malo)
        })?;
        NbQuelle::Marktlokation(malo.clone())
    } else if let Some(nb) = &a.nb_mp_id {
        rubo4e::identifiers::MarktpartnerId::new(nb).map_err(|e| {
            Ablehnung::neu(code::NB_MP_ID_UNGUELTIG, format!("nb_mp_id {nb}: {e}"))
                .mit_feld("nb_mp_id", nb)
        })?;
        NbQuelle::Netzbetreiber(nb.clone())
    } else if a.plz.is_some() {
        return Err(Ablehnung::neu(
            code::PLZ_NICHT_UNTERSTUETZT,
            "der Netzbetreiber wird nicht aus der Postleitzahl bestimmt; malo_id oder \
             nb_mp_id angeben",
        ));
    } else {
        return Err(Ablehnung::fehlt(vec!["malo_id oder nb_mp_id".to_owned()]));
    };
    if a.product_code.trim().is_empty() {
        return Err(Ablehnung::neu(code::PRODUKT_FEHLT, "product_code ist leer"));
    }
    if a.verbrauch_kwh <= Decimal::ZERO {
        return Err(Ablehnung::neu(
            code::VERBRAUCH_UNGUELTIG,
            format!("verbrauch_kwh {} ist nicht positiv", a.verbrauch_kwh),
        ));
    }
    if a.gewuenschter_lieferbeginn < heute {
        return Err(Ablehnung::neu(
            code::LIEFERBEGINN_VERGANGEN,
            format!(
                "gewuenschter_lieferbeginn {} liegt vor heute ({heute})",
                a.gewuenschter_lieferbeginn
            ),
        ));
    }
    Ok(quelle)
}

impl Ablehnung {
    fn mit_feld(mut self, key: &str, value: &str) -> Self {
        if let serde_json::Value::Object(m) = &mut self.felder {
            m.insert(key.to_owned(), serde_json::Value::String(value.to_owned()));
        }
        self
    }
}

/// The product as the catalogue holds it on the Lieferbeginn.
#[derive(Debug, Clone, Copy)]
pub struct Produktstand<'a> {
    /// `DRAFT` or `PUBLISHED`; only a published product is offered.
    pub product_status: &'a str,
    pub sparte: Option<&'a str>,
    /// The product's stored `data`, with its `tarifpreise`.
    pub data: &'a serde_json::Value,
}

/// A Konzessionsabgabe sheet as `marktd` serves it.
#[derive(Debug, Clone, Deserialize)]
pub struct KaBlatt {
    /// `Tarifkunden`, `Sondervertragskunden`, or `None` for both.
    #[serde(default)]
    pub kundengruppe_ka: Option<String>,
    /// The sheet as `marktd` stores it; its `preispositionen` are read.
    pub data: serde_json::Value,
}

/// The Kundengruppe the Konzessionsabgabe sheet is read for.
///
/// # Errors
///
/// For Strom above [`KA_TARIFKUNDEN_GRENZE_KWH`]: the measured demand decides,
/// and the offer does not know it.
pub fn ka_kundengruppe(
    sparte: Sparte,
    verbrauch_kwh: Decimal,
) -> Result<Option<&'static str>, String> {
    match sparte {
        Sparte::Strom if verbrauch_kwh <= KA_TARIFKUNDEN_GRENZE_KWH => Ok(Some("Tarifkunden")),
        Sparte::Strom => Err(format!(
            "Kundengruppe der Konzessionsabgabe: über {KA_TARIFKUNDEN_GRENZE_KWH} kWh entscheidet \
             die gemessene Leistung (§ 2 Abs. 7 KAV), die das Angebot nicht kennt"
        )),
        Sparte::Gas => Ok(None),
    }
}

/// Everything an offer is priced from — read before, so the pricing is pure.
#[derive(Debug, Clone, Copy)]
pub struct Eingaben<'a> {
    pub lf_mp_id: &'a str,
    pub anfrage: &'a EndkundenAngebotAnfrage,
    /// The checked MaLo-ID, when the caller gave one.
    pub malo_id: Option<&'a str>,
    pub nb_mp_id: &'a str,
    /// `None`: no product with this code is in force on the Lieferbeginn.
    pub produkt: Option<Produktstand<'a>>,
    /// The `PreisblattNetznutzung` (`data`), `None` when `marktd` has none.
    pub netznutzung: Option<&'a serde_json::Value>,
    /// The MP-ID the grundzuständige MSB of the NB's grid acts under, `None`
    /// when `marktd` records none — never the NB's own code.
    pub gmsb_mp_id: Option<&'a str>,
    /// The gMSB's `PreisblattMessung` (`data`), `None` when `marktd` has none.
    pub messung: Option<&'a serde_json::Value>,
    /// The Konzessionsabgabe sheet, `None` when `marktd` has none.
    pub konzessionsabgabe: Option<&'a KaBlatt>,
    /// The tax year, BEHG price and Umsatzsteuer context for the Lieferbeginn.
    pub ctx: &'a PricingContext,
    pub gueltig_bis: OffsetDateTime,
}

/// One price line of the offer, with where it was read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Preiszeile {
    /// Machine name — `ARBEITSPREIS`, `NETZENTGELT_ARBEITSPREIS`, …
    pub art: &'static str,
    /// What a reader sees.
    pub bezeichnung: String,
    /// `ct/kWh`, `EUR/Jahr` or `%`.
    pub einheit: &'static str,
    /// Decimal string; net unless the `art` ends in `_BRUTTO`.
    pub wert: String,
    /// The product version, the `marktd` sheet or the dated series it comes from.
    pub quelle: String,
}

/// A priced offer, before it is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Angebot {
    pub lf_mp_id: String,
    pub product_code: String,
    pub product_version: String,
    pub sparte: Sparte,
    pub malo_id: Option<String>,
    pub nb_mp_id: String,
    pub verbrauch_kwh: Decimal,
    pub gewuenschter_lieferbeginn: Date,
    pub lieferbeginn_fruehestens: Date,
    pub preise: Vec<Preiszeile>,
    pub jahreskosten_brutto_eur: Decimal,
    pub abschlag_monat_brutto_eur: Decimal,
    pub gueltig_bis: OffsetDateTime,
    /// The front end's reference, for an offer priced from its price.
    pub vertriebsreferenz: Option<String>,
}

impl Angebot {
    /// The hashed body — every field of the answer but `id`,
    /// `content_sha256` and `created_at`.
    ///
    /// # Errors
    ///
    /// When `gueltig_bis` cannot be formatted as RFC 3339.
    pub fn body(&self) -> Result<serde_json::Value, time::error::Format> {
        let mut body = serde_json::json!({
            "lf_mp_id": self.lf_mp_id,
            "product_code": self.product_code,
            "product_version": self.product_version,
            "sparte": self.sparte.as_str(),
            "malo_id": self.malo_id,
            "nb_mp_id": self.nb_mp_id,
            "verbrauch_kwh": self.verbrauch_kwh.to_string(),
            "gewuenschter_lieferbeginn": self.gewuenschter_lieferbeginn.to_string(),
            "lieferbeginn_fruehestens": self.lieferbeginn_fruehestens.to_string(),
            "preise": self.preise,
            "jahreskosten_brutto_eur": self.jahreskosten_brutto_eur.to_string(),
            "abschlag_monat_brutto_eur": self.abschlag_monat_brutto_eur.to_string(),
            "gueltig_bis": self
                .gueltig_bis
                .format(&time::format_description::well_known::Rfc3339)?,
        });
        if let Some(r) = &self.vertriebsreferenz {
            body["vertriebsreferenz"] = serde_json::Value::String(r.clone());
        }
        Ok(body)
    }
}

/// RFC 8785 canonical bytes of `value`.
///
/// # Errors
///
/// When `value` holds a number JCS cannot represent.
pub fn kanonisch<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_jcs::to_vec(value)
}

/// Lower-case hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// The end of the [`GUELTIGKEIT_TAGE`]th Berlin calendar day after `heute`,
/// in the Berlin offset — the last second the offer binds.
#[must_use]
pub fn gueltig_bis(heute: Date) -> OffsetDateTime {
    let ende = mako_fristen::berlin_midnight(heute + time::Duration::days(GUELTIGKEIT_TAGE + 1));
    mako_fristen::in_berlin(ende - time::Duration::seconds(1))
}

// ── Reading the sheets' positions ────────────────────────────────────────────

fn positionen(blatt: &serde_json::Value, was: &str) -> Result<Vec<Preisposition>, String> {
    let roh = blatt
        .get("preispositionen")
        .cloned()
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    serde_json::from_value(roh).map_err(|e| format!("{was}: preispositionen nicht lesbar ({e})"))
}

/// The one position of `typ` and its price for `menge`.
fn preis_der_position<'p>(
    pos: &'p [Preisposition],
    typ: Leistungstyp,
    menge: Decimal,
    was: &str,
) -> Result<(&'p Preisposition, Decimal), String> {
    use rubo4e::convenience::PreisstaffelSliceExt as _;
    let treffer: Vec<&Preisposition> = pos.iter().filter(|p| p.leistungstyp == Some(typ)).collect();
    let p = match treffer.as_slice() {
        [] => return Err(format!("{was}: Position {}", typ.as_wire())),
        [p] => *p,
        _ => {
            return Err(format!(
                "{was}: eine einzige Position {} — das Blatt führt {}, welche gilt, ist nicht \
                 bestimmt",
                typ.as_wire(),
                treffer.len()
            ));
        }
    };
    let staffeln = p.preisstaffeln.as_deref().unwrap_or_default();
    let preis = staffeln
        .select_for(menge)
        .and_then(|s| s.preis)
        .ok_or_else(|| format!("{was}: Preis der Position {} für {menge}", typ.as_wire()))?;
    Ok((p, preis))
}

/// A per-quantity price in ct/kWh.
fn ct_je_kwh(p: &Preisposition, preis: Decimal, was: &str) -> Result<Decimal, String> {
    let preis = match p.preiseinheit {
        Some(Waehrungseinheit::Eur) => preis * Decimal::ONE_HUNDRED,
        Some(Waehrungseinheit::Ct) => preis,
        _ => return Err(format!("{was}: Preiseinheit (EUR/CT)")),
    };
    match p.bezugsgroesse {
        Some(Mengeneinheit::Kwh) => Ok(preis),
        Some(Mengeneinheit::Mwh) => Ok(preis / dec!(1000)),
        _ => Err(format!("{was}: Bezugsgröße (KWH/MWH)")),
    }
}

/// A per-period price in EUR per year.
fn eur_je_jahr(p: &Preisposition, preis: Decimal, was: &str) -> Result<Decimal, String> {
    let preis = match p.preiseinheit {
        Some(Waehrungseinheit::Eur) => preis,
        Some(Waehrungseinheit::Ct) => preis / Decimal::ONE_HUNDRED,
        _ => return Err(format!("{was}: Preiseinheit (EUR/CT)")),
    };
    match p.zeitbasis {
        Some(Mengeneinheit::Jahr) => Ok(preis),
        Some(Mengeneinheit::Monat) => Ok(preis * dec!(12)),
        // The 365-day year the supply Grundpreis is quoted on.
        Some(Mengeneinheit::Tag) => Ok(preis * dec!(365)),
        _ => Err(format!("{was}: Zeitbasis (JAHR/MONAT/TAG)")),
    }
}

fn arbeitspreis(
    pos: &[Preisposition],
    typ: Leistungstyp,
    menge: Decimal,
    was: &str,
) -> Result<Decimal, String> {
    let (p, preis) = preis_der_position(pos, typ, menge, was)?;
    ct_je_kwh(p, preis, was)
}

fn jahrespreis(
    pos: &[Preisposition],
    typ: Leistungstyp,
    menge: Decimal,
    was: &str,
) -> Result<Decimal, String> {
    let (p, preis) = preis_der_position(pos, typ, menge, was)?;
    eur_je_jahr(p, preis, was)
}

/// `Ok` into `out`, `Err` onto `fehlend`.
fn lies<T>(r: Result<T, String>, fehlend: &mut Vec<String>) -> Option<T> {
    r.map_err(|e| fehlend.push(e)).ok()
}

// ── Pricing ──────────────────────────────────────────────────────────────────

/// Price and freeze one offer from inputs already read.
///
/// # Errors
///
/// [`Ablehnung`] `eingabe_fehlt` naming every missing input at once, or
/// `sparte_abweichend` for a product of another Sparte.
pub fn bepreise(e: &Eingaben<'_>) -> Result<Angebot, Ablehnung> {
    let a = e.anfrage;
    let sparte = a.sparte;
    let tag = a.gewuenschter_lieferbeginn;
    let jahr = tag.year();
    let menge = a.verbrauch_kwh;
    let mut fehlend: Vec<String> = Vec::new();

    // ── The product ──────────────────────────────────────────────────────
    let produkt = match e.produkt {
        None => {
            fehlend.push(format!(
                "Produkt {} im Katalog von {} zum {tag}",
                a.product_code, e.lf_mp_id
            ));
            None
        }
        // A contract price is offered only from its own hand-over.
        Some(p)
            if !(p.product_status == "PUBLISHED"
                || (p.product_status == "VERTRAG" && a.vertriebspreis.is_some())) =>
        {
            fehlend.push(format!(
                "veröffentlichtes Produkt {} (es ist {})",
                a.product_code, p.product_status
            ));
            None
        }
        Some(p) => {
            if let Some(ps) = p.sparte
                && !ps.eq_ignore_ascii_case(sparte.as_str())
            {
                return Err(Ablehnung::neu(
                    code::SPARTE_ABWEICHEND,
                    format!(
                        "Produkt {} gehört zur Sparte {ps}, angefragt ist {}",
                        a.product_code,
                        sparte.as_str()
                    ),
                ));
            }
            Some(p)
        }
    };
    let product_version = match produkt.map(|p| kanonisch(p.data)).transpose() {
        Ok(v) => v.map(|b| sha256_hex(&b)),
        Err(err) => {
            fehlend.push(format!(
                "Produkt {}: data nicht kanonisierbar ({err})",
                a.product_code
            ));
            None
        }
    };

    // ── The Netzbetreiber's sheets ───────────────────────────────────────
    let nb = e.nb_mp_id;
    let nne_was = format!("Preisblatt Netznutzung des Netzbetreibers {nb} zum {tag}");
    let (nne_ap, nne_gp) = match e.netznutzung {
        None => {
            fehlend.push(nne_was.clone());
            (None, None)
        }
        Some(blatt) => match lies(positionen(blatt, &nne_was), &mut fehlend) {
            None => (None, None),
            Some(pos) => (
                lies(
                    arbeitspreis(&pos, Leistungstyp::ArbeitspreisWirkarbeit, menge, &nne_was),
                    &mut fehlend,
                ),
                lies(
                    jahrespreis(&pos, Leistungstyp::Grundpreis, menge, &nne_was),
                    &mut fehlend,
                ),
            ),
        },
    };
    let msb_was = match e.gmsb_mp_id {
        Some(gmsb) => format!(
            "Preisblatt Messung des grundzuständigen Messstellenbetreibers {gmsb} \
             (Netzbetreiber {nb}) zum {tag}"
        ),
        None => format!("grundzuständiger Messstellenbetreiber des Netzbetreibers {nb}"),
    };
    let messung = match (e.gmsb_mp_id, e.messung) {
        (None, _) | (Some(_), None) => {
            fehlend.push(msb_was.clone());
            None
        }
        (Some(_), Some(blatt)) => lies(positionen(blatt, &msb_was), &mut fehlend).and_then(|pos| {
            lies(
                jahrespreis(&pos, Leistungstyp::Messstellenbetrieb, menge, &msb_was),
                &mut fehlend,
            )
        }),
    };
    let ka_gruppe = lies(ka_kundengruppe(sparte, menge), &mut fehlend);
    let ka_was = format!(
        "Preisblatt Konzessionsabgabe des Netzbetreibers {nb} ({}{}) zum {tag}",
        sparte.as_str(),
        ka_gruppe
            .flatten()
            .map(|g| format!(", {g}"))
            .unwrap_or_default()
    );
    let ka = match (ka_gruppe, e.konzessionsabgabe) {
        (None, _) => None,
        (Some(_), None) => {
            fehlend.push(ka_was.clone());
            None
        }
        (Some(gruppe), Some(blatt)) => {
            let passt = match (gruppe, blatt.kundengruppe_ka.as_deref()) {
                (_, None) => true,
                (Some(g), Some(b)) => g == b,
                (None, Some(_)) => false,
            };
            if passt {
                lies(positionen(&blatt.data, &ka_was), &mut fehlend).and_then(|pos| {
                    lies(
                        arbeitspreis(&pos, Leistungstyp::KonzessionsAbgabe, menge, &ka_was),
                        &mut fehlend,
                    )
                })
            } else {
                fehlend.push(format!(
                    "{ka_was}: das Blatt gilt für {}, die Kundengruppe dieser Belieferung ist \
                     nicht bestimmt",
                    blatt.kundengruppe_ka.as_deref().unwrap_or_default()
                ));
                None
            }
        }
    };

    // ── Dated series: Umlagen, Steuern ───────────────────────────────────
    let mut zeilen_serie: Vec<Preiszeile> = Vec::new();
    let mut umlagen = Decimal::ZERO;
    let mut steuer = Decimal::ZERO;
    let ct = |art, bezeichnung: &str, wert: Decimal, quelle: String| Preiszeile {
        art,
        bezeichnung: bezeichnung.to_owned(),
        einheit: "ct/kWh",
        wert: wert.to_string(),
        quelle,
    };
    match sparte {
        Sparte::Strom => {
            use grid_billing::umlagen::{self, Letztverbrauchergruppe as G};
            let reihen: [(&str, &str, Option<Decimal>, Option<&str>); 3] = [
                (
                    "SECT19_STROMNEV_UMLAGE",
                    "Aufschlag für besondere Netznutzung (§ 19 StromNEV-Umlage)",
                    umlagen::sect19_stromnev_ct_per_kwh(jahr, G::A),
                    umlagen::sect19_stromnev_quelle(jahr),
                ),
                (
                    "OFFSHORE_NETZUMLAGE",
                    "Offshore-Netzumlage",
                    umlagen::offshore_netzumlage_ct_per_kwh(jahr, G::A),
                    umlagen::offshore_netzumlage_quelle(jahr),
                ),
                (
                    "KWKG_UMLAGE",
                    "KWKG-Umlage",
                    umlagen::kwkg_umlage_ct_per_kwh(jahr, G::A),
                    umlagen::kwkg_umlage_quelle(jahr),
                ),
            ];
            for (art, bez, satz, quelle) in reihen {
                match satz {
                    Some(s) => {
                        umlagen += s;
                        zeilen_serie.push(ct(art, bez, s, quelle.unwrap_or_default().to_owned()));
                    }
                    None => fehlend.push(format!("{bez} {jahr} (kein Satz in der Reihe)")),
                }
            }
            match grid_billing::stromsteuer_for_year(jahr) {
                Some(s) => {
                    steuer += s;
                    zeilen_serie.push(ct(
                        "STROMSTEUER",
                        "Stromsteuer",
                        s,
                        format!("§ 3 StromStG, Satz für {jahr}"),
                    ));
                }
                None => fehlend.push(format!("Stromsteuer {jahr} (kein Satz in der Reihe)")),
            }
        }
        Sparte::Gas => {
            match energy_billing::energiesteuer_gas_for_year(jahr) {
                Some(s) => {
                    steuer += s;
                    zeilen_serie.push(ct(
                        "ENERGIESTEUER",
                        "Energiesteuer",
                        s,
                        format!("§ 2 Abs. 3 Satz 1 Nr. 4 EnergieStG, Satz für {jahr}"),
                    ));
                }
                None => fehlend.push(format!(
                    "Energiesteuer Erdgas {jahr} (kein Satz in der Reihe)"
                )),
            }
            match &e.ctx.behg_gas_ct_per_kwh {
                Ok(s) => {
                    steuer += *s;
                    zeilen_serie.push(ct(
                        "BEHG",
                        "CO₂-Preis (BEHG)",
                        *s,
                        format!("nEHS-Zertifikatspreis bzw. § 10 BEHG für {jahr}"),
                    ));
                }
                Err(warum) => fehlend.push(format!("CO₂-Preis (BEHG) {jahr}: {warum}")),
            }
        }
    }

    if !fehlend.is_empty() {
        return Err(Ablehnung::fehlt(fehlend));
    }
    // Every Option below is Some: a None pushed onto `fehlend` above.
    let (Some(produkt), Some(product_version), Some(nne_ap), Some(nne_gp), Some(messung), Some(ka)) =
        (produkt, product_version, nne_ap, nne_gp, messung, ka)
    else {
        return Err(Ablehnung::fehlt(vec!["eine Eingabe".to_owned()]));
    };

    // ── The supply price and the totals, through the B2B pricing ─────────
    // The NB's Arbeitspreis carries the netzseitigen Umlagen and its Grundpreis
    // the Messstellenbetrieb, so `compute_cost_breakdown` totals the same
    // components the lines below list.
    let pos = crate::pg::AngebotPositionInput {
        product_code: a.product_code.clone(),
        sparte: sparte.as_str().to_owned(),
        malo_id: e.malo_id.map(str::to_owned),
        melo_id: None,
        nb_mp_id: Some(nb.to_owned()),
        standort_bezeichnung: None,
        jahresverbrauch_kwh: menge,
        jahresverbrauch_ht_kwh: None,
        jahresverbrauch_nt_kwh: None,
        leistung_kw: None,
        szenario_tag: None,
        nne_arbeitspreis_ct_per_kwh: Some(nne_ap + umlagen),
        nne_grundpreis_eur_per_year: Some(nne_gp + messung),
        nne_leistungspreis_eur_per_kw_year: None,
        ka_ct_per_kwh: Some(ka),
        stromsteuer_ct_per_kwh: None,
        energiesteuer_gas_ct_per_kwh: None,
        behg_gas_ct_per_kwh: None,
    };
    let b = crate::handlers::compute_cost_breakdown(produkt.data, &pos, None, e.ctx)
        .map_err(|u| Ablehnung::fehlt(vec![format!("{}: {}", u.grund, u.detail)]))?;
    let Some(liefer_ap) = b.arbeitspreis_ct_per_kwh else {
        return Err(Ablehnung::fehlt(vec![format!(
            "Arbeitspreis im Tarifpreisblatt von Produkt {}",
            a.product_code
        )]));
    };
    let liefer_gp = b.grundpreis_eur_per_year.unwrap_or(Decimal::ZERO);
    let brutto = Decimal::ONE + b.mwst_satz;

    let produkt_quelle = match &a.vertriebspreis {
        Some(vp) => format!(
            "Vertriebspreis auf Basis von Produkt {}{} (Vertragspreis {}, Version {product_version})",
            a.product_code.split('~').next().unwrap_or_default(),
            vp.referenz
                .as_deref()
                .map(|r| format!(", Referenz {r}"))
                .unwrap_or_default(),
            a.product_code,
        ),
        None => format!(
            "productd Produkt {} Version {product_version}",
            a.product_code
        ),
    };
    let marktd = |blatt: &str| format!("marktd {blatt} des Netzbetreibers {nb}, gültig am {tag}");
    let mut preise = vec![Preiszeile {
        art: "ARBEITSPREIS",
        bezeichnung: "Arbeitspreis Energie".to_owned(),
        einheit: "ct/kWh",
        wert: liefer_ap.to_string(),
        quelle: produkt_quelle.clone(),
    }];
    if b.grundpreis_eur_per_year.is_some() {
        preise.push(Preiszeile {
            art: "GRUNDPREIS",
            bezeichnung: "Grundpreis Energie".to_owned(),
            einheit: "EUR/Jahr",
            wert: liefer_gp.to_string(),
            quelle: produkt_quelle,
        });
    }
    preise.push(ct(
        "NETZENTGELT_ARBEITSPREIS",
        "Netzentgelt Arbeitspreis",
        nne_ap.round_kfm(4),
        marktd("Preisblatt Netznutzung"),
    ));
    preise.push(Preiszeile {
        art: "NETZENTGELT_GRUNDPREIS",
        bezeichnung: "Netzentgelt Grundpreis".to_owned(),
        einheit: "EUR/Jahr",
        wert: nne_gp.round_kfm(2).to_string(),
        quelle: marktd("Preisblatt Netznutzung"),
    });
    preise.push(Preiszeile {
        art: "MESSSTELLENBETRIEB",
        bezeichnung: "Messstellenbetrieb".to_owned(),
        einheit: "EUR/Jahr",
        wert: messung.round_kfm(2).to_string(),
        quelle: format!(
            "marktd Preisblatt Messung des grundzuständigen Messstellenbetreibers {} \
             (Netzbetreiber {nb}), gültig am {tag}",
            e.gmsb_mp_id.unwrap_or_default()
        ),
    });
    preise.push(ct(
        "KONZESSIONSABGABE",
        "Konzessionsabgabe",
        ka.round_kfm(4),
        marktd("Preisblatt Konzessionsabgabe"),
    ));
    preise.extend(zeilen_serie);
    preise.push(Preiszeile {
        art: "UMSATZSTEUER",
        bezeichnung: "Umsatzsteuer".to_owned(),
        einheit: "%",
        wert: (b.mwst_satz * Decimal::ONE_HUNDRED).normalize().to_string(),
        quelle: "§ 12 Abs. 1 UStG, sofern das Produkt keinen eigenen Satz führt".to_owned(),
    });
    let ap_brutto = (liefer_ap + nne_ap + umlagen + ka + steuer) * brutto;
    let gp_brutto = (liefer_gp + nne_gp + messung) * brutto;
    preise.push(ct(
        "ARBEITSPREIS_GESAMT_BRUTTO",
        "Arbeitspreis gesamt (brutto)",
        ap_brutto.round_kfm(4),
        "Summe der Arbeitspreise dieses Angebots zuzüglich Umsatzsteuer".to_owned(),
    ));
    preise.push(Preiszeile {
        art: "GRUNDPREIS_GESAMT_BRUTTO",
        bezeichnung: "Grundpreis gesamt (brutto)".to_owned(),
        einheit: "EUR/Jahr",
        wert: gp_brutto.round_kfm(2).to_string(),
        quelle: "Summe der Grundpreise dieses Angebots zuzüglich Umsatzsteuer".to_owned(),
    });

    let jahreskosten = b.total_brutto_eur;
    Ok(Angebot {
        lf_mp_id: e.lf_mp_id.to_owned(),
        product_code: a.product_code.clone(),
        product_version,
        sparte,
        malo_id: e.malo_id.map(str::to_owned),
        nb_mp_id: nb.to_owned(),
        verbrauch_kwh: menge,
        gewuenschter_lieferbeginn: tag,
        // No Wechsel-Vorlauf is applied here: the earliest day is the one
        // asked for.
        lieferbeginn_fruehestens: tag,
        preise,
        jahreskosten_brutto_eur: jahreskosten,
        abschlag_monat_brutto_eur: (jahreskosten / dec!(12)).round_kfm(2),
        gueltig_bis: e.gueltig_bis,
        vertriebsreferenz: a.vertriebspreis.as_ref().and_then(|v| v.referenz.clone()),
    })
}

// ── marktd ───────────────────────────────────────────────────────────────────

/// `marktd`, where the Netzbetreiber's sheets and the MaLo's NB are read.
#[derive(Debug, Clone)]
pub struct MarktdQuelle {
    up: Option<Upstream>,
}

/// A `marktd` sheet answer: only its `data` is read.
#[derive(Debug, Deserialize)]
struct Blatt {
    data: serde_json::Value,
}

/// `GET /api/v1/netzbetreiber/{nb}/grundzustaendiger-msb`.
#[derive(Debug, Deserialize)]
struct Grundzustaendigkeit {
    gmsb_mp_id: String,
}

/// `GET /api/v1/malos/{id}/grid`.
#[derive(Debug, Deserialize)]
struct Netzzuordnung {
    nb_mp_id: String,
    sparte: String,
}

impl MarktdQuelle {
    /// The `marktd` at `base_url`, or none.
    #[must_use]
    pub fn new(base_url: Option<&str>, api_key: Option<String>, http: reqwest::Client) -> Self {
        Self {
            up: base_url.map(|u| {
                Upstream::new("marktd", u, api_key.map(secrecy::SecretString::from), http)
            }),
        }
    }

    fn up(&self) -> Result<&Upstream, ApiError> {
        self.up.as_ref().ok_or_else(|| {
            ApiError::unavailable(
                "productd liest die Preisblätter des Netzbetreibers aus marktd, und marktd_url ist \
                 nicht konfiguriert",
            )
            .with_code(code::MARKTD_NICHT_KONFIGURIERT)
        })
    }

    /// The NB of `malo_id`, and the Sparte `marktd` records for it.
    async fn netzbetreiber(&self, malo_id: &str) -> Result<Option<Netzzuordnung>, ApiError> {
        let up = self.up()?;
        up.json(up.get(&format!("/api/v1/malos/{malo_id}/grid")))
            .await
            .map_err(|e| ApiError::upstream("marktd", e))
    }

    /// The MP-ID the grundzuständige MSB of `nb`'s grid acts under, or none.
    async fn grundzustaendiger_msb(&self, nb: &str) -> Result<Option<String>, ApiError> {
        let up = self.up()?;
        Ok(up
            .json::<Grundzustaendigkeit>(
                up.get(&format!("/api/v1/netzbetreiber/{nb}/grundzustaendiger-msb")),
            )
            .await
            .map_err(|e| ApiError::upstream("marktd", e))?
            .map(|g| g.gmsb_mp_id))
    }

    async fn blatt(
        &self,
        pfad: &str,
        query: &[(&str, &str)],
    ) -> Result<Option<serde_json::Value>, ApiError> {
        let up = self.up()?;
        Ok(up
            .json::<Blatt>(up.get(pfad).query(query))
            .await
            .map_err(|e| ApiError::upstream("marktd", e))?
            .map(|b| b.data))
    }

    async fn ka_blatt(
        &self,
        nb: &str,
        query: &[(&str, &str)],
    ) -> Result<Option<KaBlatt>, ApiError> {
        let up = self.up()?;
        up.json(
            up.get(&format!("/api/v1/preisblaetter-ka/{nb}"))
                .query(query),
        )
        .await
        .map_err(|e| ApiError::upstream("marktd", e))
    }
}

// ── The route's work ─────────────────────────────────────────────────────────

/// Check, read, price, freeze and store one offer; the answer of the route.
///
/// # Errors
///
/// Every refusal of [`pruefe_anfrage`] and [`bepreise`], `marktd` not
/// configured or failing, and storage errors.
pub async fn erstelle(
    pool: &PgPool,
    tenant: &str,
    lf_mp_id: &str,
    marktd: &MarktdQuelle,
    regime: &crate::billingd::BehgRegimeQuelle,
    anfrage: &EndkundenAngebotAnfrage,
) -> Result<serde_json::Value, ApiError> {
    let heute = mako_fristen::heute();
    let quelle = pruefe_anfrage(anfrage, heute)?;
    let tag = anfrage.gewuenschter_lieferbeginn;
    let sparte = anfrage.sparte;

    let (malo_id, nb_mp_id) = match quelle {
        NbQuelle::Netzbetreiber(nb) => (None, nb),
        NbQuelle::Marktlokation(malo) => {
            let Some(z) = marktd.netzbetreiber(&malo).await? else {
                return Err(Ablehnung::fehlt(vec![format!(
                    "Netzbetreiber der Marktlokation {malo} (marktd kennt sie nicht)"
                )])
                .into());
            };
            if !z.sparte.eq_ignore_ascii_case(sparte.as_str()) {
                return Err(Ablehnung::neu(
                    code::SPARTE_ABWEICHEND,
                    format!(
                        "Marktlokation {malo} gehört zur Sparte {}, angefragt ist {}",
                        z.sparte,
                        sparte.as_str()
                    ),
                )
                .into());
            }
            (Some(malo), z.nb_mp_id)
        }
    };

    let produkt =
        crate::pg::fetch_product(pool, lf_mp_id, tenant, &anfrage.product_code, Some(tag))
            .await
            .map_err(ApiError::internal)?;
    // A front end's price: the basis is checked and the contract price derived
    // here, priced below like any product, and stored only once the offer
    // stands — a refused display check leaves nothing behind.
    let vertragspreis = match (&anfrage.vertriebspreis, &produkt) {
        (None, _) => None,
        (Some(_), None) => None,
        (Some(vp), Some(basis)) => {
            if basis.product_status != "PUBLISHED" {
                return Err(Ablehnung::neu(
                    code::BASIS_UNGEEIGNET,
                    format!(
                        "Basis {} ist {}: ein Vertriebspreis setzt auf ein veröffentlichtes Produkt",
                        basis.product_code, basis.product_status
                    ),
                )
                .into());
            }
            if basis.dyn_source.is_some() {
                return Err(Ablehnung::neu(
                    code::BASIS_UNGEEIGNET,
                    format!(
                        "Basis {} ist dynamisch (§ 41a EnWG): sein Arbeitspreis folgt der Börse",
                        basis.product_code
                    ),
                )
                .into());
            }
            if let Some(r) = &vp.referenz
                && (r.is_empty() || r.chars().count() > 128 || r.chars().any(char::is_control))
            {
                return Err(Ablehnung::neu(
                    code::VERTRIEBSPREIS_UNGUELTIG,
                    "referenz: 1 bis 128 Zeichen ohne Steuerzeichen",
                )
                .into());
            }
            let data = vertragspreis_daten(&basis.data, vp)?;
            let sha = sha256_hex(&kanonisch(&data).map_err(ApiError::internal)?);
            let code = format!("{}~{}", basis.product_code, &sha[..12]);
            Some((code, data))
        }
    };
    let preisanfrage;
    let anfrage = match &vertragspreis {
        Some((code, _)) => {
            preisanfrage = EndkundenAngebotAnfrage {
                product_code: code.clone(),
                ..anfrage.clone()
            };
            &preisanfrage
        }
        None => anfrage,
    };
    let datum = tag.to_string();
    let netznutzung = marktd
        .blatt(
            &format!("/api/v1/preisblaetter/{nb_mp_id}"),
            &[("date", datum.as_str())],
        )
        .await?;
    // The Messung sheet is the gMSB's, filed under its own MP-ID; without the
    // recorded link the offer names it as missing.
    let gmsb_mp_id = marktd.grundzustaendiger_msb(&nb_mp_id).await?;
    let messung = match &gmsb_mp_id {
        None => None,
        Some(gmsb) => {
            marktd
                .blatt(
                    &format!("/api/v1/preisblaetter-messung/{gmsb}"),
                    &[("date", datum.as_str())],
                )
                .await?
        }
    };
    let konzessionsabgabe = match ka_kundengruppe(sparte, anfrage.verbrauch_kwh) {
        Err(_) => None,
        Ok(gruppe) => {
            let mut q = vec![("date", datum.as_str()), ("sparte", sparte.as_str())];
            if let Some(g) = gruppe {
                q.push(("kundengruppe", g));
            }
            marktd.ka_blatt(&nb_mp_id, &q).await?
        }
    };
    let ctx = crate::handlers::pricing_context(pool, regime, tag, false).await;

    let angebot = bepreise(&Eingaben {
        lf_mp_id,
        anfrage,
        malo_id: malo_id.as_deref(),
        nb_mp_id: &nb_mp_id,
        produkt: produkt.as_ref().map(|p| match &vertragspreis {
            Some((_, data)) => Produktstand {
                product_status: "VERTRAG",
                sparte: p.sparte.as_deref(),
                data,
            },
            None => Produktstand {
                product_status: &p.product_status,
                sparte: p.sparte.as_deref(),
                data: &p.data,
            },
        }),
        netznutzung: netznutzung.as_ref(),
        gmsb_mp_id: gmsb_mp_id.as_deref(),
        messung: messung.as_ref(),
        konzessionsabgabe: konzessionsabgabe.as_ref(),
        ctx: &ctx,
        gueltig_bis: gueltig_bis(heute),
    })?;
    if let Some(gezeigt) = anfrage
        .vertriebspreis
        .as_ref()
        .and_then(|v| v.angezeigt.as_ref())
    {
        let abweichend = abweichungen(&angebot, gezeigt);
        if !abweichend.is_empty() {
            let mut a = Ablehnung::neu(
                code::ANGEZEIGT_ABWEICHEND,
                "was der Kunde gesehen hat, ist nicht, was mako bepreist",
            );
            a.felder = serde_json::json!({ "abweichungen": abweichend });
            return Err(a.into());
        }
    }
    // The contract price and the offer that cites it commit together: an
    // offer without its price version would answer a code billing cannot read.
    let mut tx = pool.begin().await.map_err(ApiError::internal)?;
    if let (Some((code, data)), Some(basis)) = (&vertragspreis, &produkt) {
        crate::pg::insert_vertragspreis(
            &mut *tx,
            tenant,
            basis,
            code,
            data,
            anfrage
                .vertriebspreis
                .as_ref()
                .and_then(|v| v.referenz.as_deref()),
        )
        .await
        .map_err(ApiError::internal)?;
    }
    let gespeichert = speichere(&mut *tx, tenant, &angebot).await?;
    tx.commit().await.map_err(ApiError::internal)?;
    Ok(gespeichert)
}

/// Insert `angebot` and answer with it as stored.
///
/// # Errors
///
/// Serialisation and storage errors.
pub async fn speichere<'e>(
    exec: impl sqlx::PgExecutor<'e>,
    tenant: &str,
    angebot: &Angebot,
) -> Result<serde_json::Value, ApiError> {
    let body = angebot.body().map_err(ApiError::internal)?;
    let bytes = kanonisch(&body).map_err(ApiError::internal)?;
    let sha = sha256_hex(&bytes);
    let id = Uuid::new_v4();
    let preise = serde_json::to_value(&angebot.preise).map_err(ApiError::internal)?;
    let created_at: OffsetDateTime = sqlx::query_scalar(
        "INSERT INTO endkunden_angebote
            (id, tenant, lf_mp_id, product_code, product_version, sparte, malo_id, nb_mp_id,
             verbrauch_kwh, gewuenschter_lieferbeginn, lieferbeginn_fruehestens, preise,
             jahreskosten_brutto_eur, abschlag_monat_brutto_eur, gueltig_bis, kanonisch,
             content_sha256)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
         RETURNING created_at",
    )
    .bind(id)
    .bind(tenant)
    .bind(&angebot.lf_mp_id)
    .bind(&angebot.product_code)
    .bind(&angebot.product_version)
    .bind(angebot.sparte.as_str())
    .bind(&angebot.malo_id)
    .bind(&angebot.nb_mp_id)
    .bind(angebot.verbrauch_kwh)
    .bind(angebot.gewuenschter_lieferbeginn)
    .bind(angebot.lieferbeginn_fruehestens)
    .bind(preise)
    .bind(angebot.jahreskosten_brutto_eur)
    .bind(angebot.abschlag_monat_brutto_eur)
    .bind(angebot.gueltig_bis)
    .bind(&bytes)
    .bind(&sha)
    .fetch_one(exec)
    .await
    .map_err(ApiError::internal)?;
    antwort(id, &bytes, &sha, created_at)
}

/// The stored offer of this tenant, as the route answers it; `None` when there
/// is none.
///
/// # Errors
///
/// Storage errors, and stored bytes that are not a JSON object.
pub async fn lade(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
) -> Result<Option<serde_json::Value>, ApiError> {
    let Some(row) = sqlx::query(
        "SELECT kanonisch, content_sha256, created_at
           FROM endkunden_angebote WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::internal)?
    else {
        return Ok(None);
    };
    let bytes: Vec<u8> = row.try_get("kanonisch").map_err(ApiError::internal)?;
    let sha: String = row.try_get("content_sha256").map_err(ApiError::internal)?;
    let created_at: OffsetDateTime = row.try_get("created_at").map_err(ApiError::internal)?;
    antwort(id, &bytes, &sha, created_at).map(Some)
}

/// The answer: the hashed body read back out of its bytes, plus `id`,
/// `content_sha256` and `created_at`.
fn antwort(
    id: Uuid,
    kanonisch: &[u8],
    sha: &str,
    created_at: OffsetDateTime,
) -> Result<serde_json::Value, ApiError> {
    let mut body: serde_json::Value =
        serde_json::from_slice(kanonisch).map_err(ApiError::internal)?;
    let serde_json::Value::Object(m) = &mut body else {
        return Err(ApiError::internal(anyhow::anyhow!(
            "endkunden_angebote {id}: kanonisch is not a JSON object"
        )));
    };
    m.insert("id".to_owned(), serde_json::json!(id));
    m.insert("content_sha256".to_owned(), serde_json::json!(sha));
    m.insert(
        "created_at".to_owned(),
        serde_json::json!(
            created_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(ApiError::internal)?
        ),
    );
    Ok(body)
}

/// The shared reader, as the router injects it.
pub type MarktdExt = axum::Extension<Arc<MarktdQuelle>>;
