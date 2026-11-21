//! The calculators: the § 30 MsbG price cap for a smart meter, a plant's
//! anzulegender Wert under the EEG, the statutory parts of a kWh by year, and
//! § 14a Modul 1 against Modul 2. Each answers from the crate the settlement
//! uses (`grid-billing`, `eeg-billing`); none restates a rate.

use eeg_billing::ErzeugungsArt;
use grid_billing::msbg::{
    Entgeltschuldner, MessstellenKategorie, PflichtEinstufung, Preisobergrenze,
    STEUEREINRICHTUNG_OBERGRENZE_EUR_PER_JAHR, preisobergrenze_eur_per_jahr,
};
use grid_billing::umlagen::{self, Letztverbrauchergruppe};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use time::Date;

use crate::refused;

fn datum(s: &str) -> Option<Date> {
    Date::parse(s, &time::format_description::well_known::Iso8601::DATE).ok()
}

fn betrag(p: Preisobergrenze) -> Value {
    match p {
        Preisobergrenze::Betrag(b) => json!({ "eur": b.to_string() }),
        Preisobergrenze::Angemessen => json!({ "angemessen": true }),
        Preisobergrenze::VorSchedule => json!({ "vor_schedule": true }),
    }
}

/// `pog`: what an intelligentes Messsystem may cost per year (§ 30 MsbG),
/// split between the Netzbetreiber and the Letztverbraucher.
pub(crate) fn pog(
    verbrauch_kwh: Option<Decimal>,
    leistung_kw: Option<Decimal>,
    sect14a: bool,
    optional: bool,
    stichtag: &str,
) -> Value {
    let Some(tag) = datum(stichtag) else {
        return refused("datum_ungueltig", "Das Datum ist kein Kalenderdatum.");
    };
    let (kategorie, band) = if optional {
        (MessstellenKategorie::OptionalerEinbau, None)
    } else {
        let e = PflichtEinstufung {
            jahresverbrauch_kwh: verbrauch_kwh,
            installierte_leistung_kw: leistung_kw,
            steuerbare_verbrauchseinrichtung: sect14a,
        };
        (MessstellenKategorie::Pflichteinbau(e), Some(e.band()))
    };
    json!({ "ok": {
        "band": band.map(|b| format!("{b:?}")),
        "letztverbraucher": betrag(preisobergrenze_eur_per_jahr(kategorie, Entgeltschuldner::Letztverbraucher, tag)),
        "netzbetreiber": betrag(preisobergrenze_eur_per_jahr(kategorie, Entgeltschuldner::Netzbetreiber, tag)),
        "steuereinrichtung_eur": STEUEREINRICHTUNG_OBERGRENZE_EUR_PER_JAHR.to_string(),
    } })
}

/// The plant types the calculator offers, by the name the page sends: the
/// two solar Bauformen whose statutory rates the page explains.
fn art(name: &str) -> Option<ErzeugungsArt> {
    use ErzeugungsArt as A;
    Some(match name {
        "solar_aufdach" => A::SolarAufdach,
        "solar_freiflaeche" => A::SolarFreiflaeche,
        _ => return None,
    })
}

/// `eeg`: a plant's anzulegender Wert at commissioning (§§ 46–49 EEG 2023
/// with the degression), the § 53 deduction under Einspeisevergütung, and
/// the end of its payment period.
pub(crate) fn eeg(name: &str, leistung_kw: Decimal, inbetriebnahme: &str, voll: bool) -> Value {
    let Some(a) = art(name) else {
        return refused(
            "anlage_unbekannt",
            "Diese Anlagenart bietet der Rechner nicht an.",
        );
    };
    let Some(tag) = datum(inbetriebnahme) else {
        return refused(
            "datum_ungueltig",
            "Das Inbetriebnahmedatum ist kein Kalenderdatum.",
        );
    };
    if leistung_kw <= Decimal::ZERO {
        return refused(
            "leistung_ungueltig",
            "Die Leistung muss größer als 0 kW sein.",
        );
    }
    let aw = if voll && a == ErzeugungsArt::SolarAufdach {
        eeg_billing::rates::solar_pv_volleinspeisung_aw_ct(leistung_kw, tag)
    } else {
        eeg_billing::rates::aw_ct_bei_inbetriebnahme(a, leistung_kw, tag)
    };
    let abzug = eeg_billing::rates::sect53_deduction(a);
    json!({ "ok": {
        "aw_ct": aw.map(|v| v.to_string()),
        "abzug_ct": abzug.to_string(),
        "verguetung_ct": aw.map(|v| (v - abzug).max(Decimal::ZERO).to_string()),
        "foerderende": eeg_billing::foerderendedatum_eeg(tag).ok().map(|d| d.to_string()),
    } })
}

/// `preise`: the statutory parts of a kWh per year for a Letztverbrauchergruppe
/// — the Netzumlagen with their sources, the Stromsteuer and the
/// Umsatzsteuer; a year whose figure is not published answers none.
pub(crate) fn preise(gruppe: &str, von: i32, bis: i32) -> Value {
    let g = match gruppe {
        "A" => Letztverbrauchergruppe::A,
        "B" => Letztverbrauchergruppe::B,
        "C" => Letztverbrauchergruppe::C,
        _ => {
            return refused(
                "gruppe_unbekannt",
                "Die Letztverbrauchergruppe ist A, B oder C.",
            );
        }
    };
    let s = |d: Option<Decimal>| d.map(|v| v.to_string());
    let jahre: Vec<Value> = (von..=bis.min(von + 20))
        .map(|j| {
            let ust = Date::from_calendar_date(j, time::Month::January, 1)
                .ok()
                .zip(Date::from_calendar_date(j, time::Month::December, 31).ok())
                .and_then(|(a, b)| {
                    grid_billing::umsatzsteuer::regelsatz_prozent(
                        grid_billing::umsatzsteuer::Leistungsart::LieferungStrom,
                        a,
                        b,
                    )
                });
            json!({
                "jahr": j,
                "sect19": s(umlagen::sect19_stromnev_ct_per_kwh(j, g)),
                "sect19_quelle": umlagen::sect19_stromnev_quelle(j),
                "offshore": s(umlagen::offshore_netzumlage_ct_per_kwh(j, g)),
                "offshore_quelle": umlagen::offshore_netzumlage_quelle(j),
                "kwkg": s(umlagen::kwkg_umlage_ct_per_kwh(j, g)),
                "kwkg_quelle": umlagen::kwkg_umlage_quelle(j),
                "stromsteuer": s(grid_billing::stromsteuer::stromsteuer_for_year(j)),
                "ust_prozent": s(ust),
            })
        })
        .collect();
    json!({ "ok": { "jahre": jahre } })
}

/// `modul14a`: § 14a EnWG Modul 1 (the Netzbetreiber's pauschale) against
/// Modul 2 (the device's Arbeitspreis at the fixed BK8-22/010-A share), net,
/// with the consumption at which Modul 2 starts to pay more.
pub(crate) fn modul14a(
    arbeitspreis_ct: Decimal,
    geraet_kwh: Decimal,
    pauschale_eur: Decimal,
) -> Value {
    if arbeitspreis_ct <= Decimal::ZERO
        || geraet_kwh < Decimal::ZERO
        || pauschale_eur < Decimal::ZERO
    {
        return refused(
            "eingabe_ungueltig",
            "Arbeitspreis über 0, Verbrauch und Pauschale nicht negativ.",
        );
    }
    let anteil = grid_billing::Reduktionsfaktor::REGELFALL.get();
    let ersparnis_pro_kwh_eur = arbeitspreis_ct * (Decimal::ONE - anteil) / Decimal::ONE_HUNDRED;
    let modul2 = (ersparnis_pro_kwh_eur * geraet_kwh)
        .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    let schwelle = (pauschale_eur / ersparnis_pro_kwh_eur)
        .round_dp_with_strategy(0, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
    json!({ "ok": {
        "anteil_prozent": (anteil * Decimal::ONE_HUNDRED).normalize().to_string(),
        "modul1_eur": pauschale_eur.to_string(),
        "modul2_eur": modul2.to_string(),
        "schwelle_kwh": schwelle.to_string(),
    } })
}
