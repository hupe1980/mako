//! Stadtwerke Musterstadt — the one world the seeder writes, in one place.
//!
//! The Stadtwerk is Netzbetreiber, Lieferant and Messstellenbetreiber in one
//! tenant, in Strom and in Gas. Its Marktpartner-IDs are synthetic on purpose: a check-digit-valid
//! MP-ID names a real company, so none of these satisfies either published
//! procedure (`every_mp_id_is_synthetic`). Its Marktlokationen carry the BDEW
//! check digit the storage layer demands, derived here, never typed.
//!
//! The request bodies live beside this file in `world/`, where each service's
//! `seed_payloads` suite deserialises them into the real request type; the
//! seeder fills in the identifiers and the dates.

use rubo4e::identifiers::MaloId;
use serde_json::Value;
use time::Date;

/// The Stadtwerk: tenant and MP-ID of its NB, LF and MSB (Strom).
pub const STADTWERK: &str = "9900357000004";
/// Its display name.
pub const STADTWERK_NAME: &str = "Stadtwerke Musterstadt GmbH";
/// The Stadtwerk in Gas: the DVGW-Codenummer of its NB, LF and MSB. A
/// Marktteilnehmer holds one MP-ID per Sparte and role (BDEW-AWH
/// Identifikatoren V1.2 Kap. 2.1, S. 4), so Gas is not the Strom number.
pub const STADTWERK_GAS: &str = "9800357000004";
/// Gasnetz Nachbarstadt — the neighbouring Gas Netzbetreiber whose
/// NN-Rechnung the Stadtwerk's Lieferant disputes.
pub const GASNETZ_NACHBARSTADT: &str = "9800468000003";
/// Musterstadt Energievertrieb — the Stadtwerk's affiliated supplier, whose
/// Anmeldungen processd holds for a person (`[identity] verbundene_lieferanten`).
pub const VERTRIEB: &str = "9912345000005";
/// Nordlicht Energie — an unaffiliated supplier, the sender of the fixture's
/// 55001 (a GS1-style party, NAD agency `9`).
pub const NORDLICHT: &str = "4012345000024";
/// The Bilanzkreisverantwortlicher whose 55071 gives the
/// Zuordnungsermächtigung E_0622 Prüfschritt 60 asks for.
pub const BKV: &str = "9900000000019";

/// The MSB that delivers the seeded readings. meterstore keys a reading's
/// version scope on the sender's Codenummer and refuses one without a valid
/// check digit, so this is the one MP-ID of the world that satisfies the BDEW
/// procedure.
pub const MSB_MESSWERTE: &str = "9900000000003";

/// The canonical Marktlokation: Erika Mustermann's household, supplied and
/// invoiced by the Stadtwerk.
pub const ERIKA_MALO: &str = "51238696012";
/// The plant's Erzeugungs-MaLo, settled under § 21 EEG 2023.
pub const EEG_MALO: &str = "17835382008";
/// The plant.
pub const EEG_TR: &str = "TR0000000001";
/// Its operator.
pub const EEG_EINSPEISER: &str = "EIN-0001";

/// The Stadtwerk's household customers besides Erika Mustermann: first and
/// last name, each on grid MaLo `3 + index`.
pub const HAUSHALTE: &[(&str, &str)] = &[
    ("Max", "Leerstand"),
    ("Anna", "Becker"),
    ("Jonas", "Schulz"),
    ("Leonie", "Wagner"),
    ("Paul", "Hoffmann"),
    ("Mia", "Koch"),
    ("Felix", "Richter"),
    ("Emma", "Klein"),
    ("Lukas", "Wolf"),
    ("Sophie", "Neumann"),
    ("Ben", "Schwarz"),
    ("Lina", "Zimmermann"),
];

/// Clara Sommer's Marktlokation: grid MaLo 16, ordered online.
pub fn bestellung_malo() -> String {
    grid_malo(16)
}

/// The grid MaLo of household `index` in [`HAUSHALTE`].
pub fn haushalt_malo(index: usize) -> String {
    grid_malo(3 + u32::try_from(index).unwrap_or(0))
}

/// The Stadtwerk's household product (productd), what the contract names.
pub const PRODUCT_STROM: &str = "MUSTERSTADT-STROM-ZUHAUSE";
/// Its Gas household product.
pub const PRODUCT_GAS: &str = "MUSTERSTADT-GAS-ZUHAUSE";

/// How many Marktlokationen the Stadtwerk's Gas grid holds.
pub const GAS_MALOS: u32 = 6;

/// The `n`-th Marktlokation of the Gas grid (0-based), its BDEW check digit
/// computed.
pub fn gas_malo(n: u32) -> String {
    malo_from_base(&format!("42{:08}", 200 + n * 41))
}

/// The Stadtwerk's Gas household customer, first and last name, on Gas
/// MaLo 0.
pub const GAS_KUNDE: (&str, &str) = ("Hannah", "Brandt");

/// A Marktlokation in Gasnetz Nachbarstadt's grid that the Stadtwerk's
/// Lieferant supplies — what the disputed NN-Rechnung bills.
pub fn nachbar_malo() -> String {
    malo_from_base("4300000317")
}

/// The disputed NN-Rechnung's number (`BGM` DE 1004).
pub const NN_RECHNUNG: &str = "GNN-2026-10-0001";

/// What Gasnetz Nachbarstadt's Preisblatt publishes as its Arbeitspreis, in
/// EUR/kWh (`world/marktd-preisblatt-gas.json`), and what its NN-Rechnung
/// bills instead — the deviation invoicd disputes.
pub const NN_ARBEITSPREIS_PREISBLATT: &str = "0.0112";
/// The Arbeitspreis the NN-Rechnung bills (`PRI+CAL`).
pub const NN_ARBEITSPREIS_RECHNUNG: &str = "0.0142";

/// A UTILMD Prüfidentifikator the PID overview 4.0
/// (crates/edi-energy/profiles/pid-overview.json) does not publish: the
/// world's misaddressed message, which makod dead-letters.
pub const UNPUBLISHED_PID: u32 = 55998;

/// The world's one Referenzreihe value: a KAV-Grenzpreis Strom for 2024 that
/// is **synthetic demo data**, not the Destatis figure. Its source is this
/// file, so the value names where it came from; no EPEX price is ever seeded,
/// because mako may not distribute one.
pub const REFERENZWERT_CSV: &str = include_str!("../world/marktd-referenzwert.csv");
/// The source the imported value carries.
pub const REFERENZWERT_QUELLE: &str = "https://github.com/hupe1980/mako/blob/main/tools/mako-seed/world/marktd-referenzwert.csv#synthetic-demo-value";

/// How many Marktlokationen the Stadtwerk's grid holds besides the named ones.
pub const GRID_MALOS: u32 = 36;

/// The `n`-th Marktlokation of the grid (0-based): a stable base, its BDEW
/// check digit computed.
pub fn grid_malo(n: u32) -> String {
    malo_from_base(&format!("41{:08}", 100 + n * 37))
}

/// A MaLo-ID from its ten-digit base, the check digit computed.
fn malo_from_base(base: &str) -> String {
    MaloId::from_base(base)
        .map(|id| id.as_ref().to_owned())
        .unwrap_or_else(|e| panic!("base {base}: {e}"))
}

/// The Marktlokation Nordlicht's 55001 names: grid MaLo 0, with no supplier.
pub fn anmeldung_malo() -> String {
    grid_malo(0)
}

/// The Marktlokation the affiliated supplier's 55001 names: grid MaLo 1.
pub fn freigabe_malo() -> String {
    grid_malo(1)
}

/// A Marktlokation marktd has no supply record of: grid MaLo 2. Whether its
/// Inbetriebnahme still lies ahead is a question no store answers, so an
/// Anmeldung on it escalates to a person (`E_0622` Prüfschritt 50) — the
/// world's Klärfall.
pub fn klaerfall_malo() -> String {
    grid_malo(2)
}

/// A Messlokation (Zählpunktbezeichnung, 33 characters: `DE`, a six-digit
/// Netzbetreibernummer, 25 alphanumerics) for a Marktlokation, derived from it.
pub fn melo_for(malo: &str) -> String {
    format!("DE000562668000000000{malo:0>13}")
}

/// A world body by its file name, parsed.
pub fn body(name: &str) -> Value {
    let text = match name {
        "malo.json" => include_str!("../world/malo.json"),
        "malo-erzeugung.json" => include_str!("../world/malo-erzeugung.json"),
        "malo-gas.json" => include_str!("../world/malo-gas.json"),
        "marktd-grid-gas.json" => include_str!("../world/marktd-grid-gas.json"),
        "marktd-partner-gnb.json" => include_str!("../world/marktd-partner-gnb.json"),
        "marktd-preisblatt-gas.json" => include_str!("../world/marktd-preisblatt-gas.json"),
        "makod-partner-gnb.json" => include_str!("../world/makod-partner-gnb.json"),
        "productd-product-gas.json" => include_str!("../world/productd-product-gas.json"),
        "vertragd-vertrag-gas.json" => include_str!("../world/vertragd-vertrag-gas.json"),
        "preisblatt-nb.json" => include_str!("../world/preisblatt-nb.json"),
        "marktd-partner-lf.json" => include_str!("../world/marktd-partner-lf.json"),
        "marktd-grid.json" => include_str!("../world/marktd-grid.json"),
        "marktd-melo.json" => include_str!("../world/marktd-melo.json"),
        "marktd-lokationszuordnung.json" => {
            include_str!("../world/marktd-lokationszuordnung.json")
        }
        "marktd-melo-msb.json" => include_str!("../world/marktd-melo-msb.json"),
        "marktd-versorgung.json" => include_str!("../world/marktd-versorgung.json"),
        "marktd-preisblatt-messung.json" => include_str!("../world/marktd-preisblatt-messung.json"),
        "marktd-preisblatt-ka.json" => include_str!("../world/marktd-preisblatt-ka.json"),
        "vertragd-bestellung.json" => include_str!("../world/vertragd-bestellung.json"),
        "makod-partner-lf.json" => include_str!("../world/makod-partner-lf.json"),
        "makod-partner-bkv.json" => include_str!("../world/makod-partner-bkv.json"),
        "einsd-einspeiser.json" => include_str!("../world/einsd-einspeiser.json"),
        "einsd-anlage.json" => include_str!("../world/einsd-anlage.json"),
        "productd-product-strom.json" => include_str!("../world/productd-product-strom.json"),
        "vertragd-kunde.json" => include_str!("../world/vertragd-kunde.json"),
        "vertragd-zahlungsinformation.json" => {
            include_str!("../world/vertragd-zahlungsinformation.json")
        }
        "vertragd-vertrag.json" => include_str!("../world/vertragd-vertrag.json"),
        "billingd-calculate.json" => include_str!("../world/billingd-calculate.json"),
        "accountingd-payment.json" => include_str!("../world/accountingd-payment.json"),
        other => panic!("no world body {other}"),
    };
    serde_json::from_str(text).unwrap_or_else(|e| panic!("world/{name}: {e}"))
}

/// The inbound 55001 of the fixture, for `malo` from `sender`, Lieferbeginn
/// `lieferbeginn`, under the interchange reference `reference` (unique per
/// message, so makod's interchange deduplication sees a new one).
pub fn anmeldung_55001(malo: &str, sender: &str, lieferbeginn: Date, reference: &str) -> String {
    let fixture = include_str!("../world/utilmd-55001.edi");
    // DTM+92 carries the Lieferbeginn as the instant Strom's day begins:
    // 22:00 UTC (MESZ) or 23:00 UTC (MEZ) on the day before.
    let beginn = mako_fristen::berlin_midnight(lieferbeginn).to_offset(time::UtcOffset::UTC);
    let dtm92 = format!(
        "{:04}{:02}{:02}{:02}00",
        beginn.year(),
        u8::from(beginn.month()),
        beginn.day(),
        beginn.hour()
    );
    let mut out = fixture
        .replace("DEMO-2026-001", reference)
        .replace("MSG-001", &format!("M{reference}"))
        .replace(ERIKA_MALO, malo)
        .replace("202609302200", &dtm92);
    if sender != NORDLICHT {
        // The sender in UNB and NAD+MS, each with the agency its range takes:
        // a BDEW-Codenummer is `500` in UNB and `293` in NAD.
        out = out
            .replace(&format!("{NORDLICHT}:14"), &format!("{sender}:500"))
            .replace(
                &format!("NAD+MS+{NORDLICHT}::9"),
                &format!("NAD+MS+{sender}::293"),
            );
    }
    out
}

/// The BKV's 55071 Zuordnungsermächtigung for the supplier `lf`, under
/// `reference`.
pub fn zuordnungsermaechtigung_55071(lf: &str, reference: &str) -> String {
    let fixture = include_str!("../world/utilmd-55071.edi")
        .replace("DEMO-ZE-001", reference)
        .replace("MSG-ZE-001", &format!("M{reference}"))
        .replace("ZE0001", &format!("Z{reference}"));
    if lf == NORDLICHT {
        fixture
    } else {
        fixture.replace(
            &format!("NAD+VY+{NORDLICHT}::9"),
            &format!("NAD+VY+{lf}::293"),
        )
    }
}

/// Gasnetz Nachbarstadt's NN-Rechnung 31002 for October 2026 on
/// MaLo `43000003179`, issued `heute` and due 15 Werktage later, under the
/// interchange reference `reference`.
///
/// 1000 kWh at 1.42 ct/kWh where the sender's own Preisblatt publishes
/// 1.12 ct/kWh: the price disagrees with the Preisblatt, so invoicd disputes
/// it. Everything else is a conformant invoice — the Zahlungsziel is past the
/// 10 Werktage the AHB asks of a Gas recipient (INVOIC AHB 1.0b `[24]`), the
/// arithmetic and the Umsatzsteuer add up — so the price is the one finding.
pub fn nn_rechnung_31002(heute: Date, reference: &str) -> String {
    // A DTM 303 value at the Berlin day's Beginn, in UTC (`[931]` ZZZ = +00).
    let tagesbeginn = |d: Date| {
        let t = mako_fristen::berlin_midnight(d).to_offset(time::UtcOffset::UTC);
        format!(
            "{:04}{:02}{:02}{:02}00",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour()
        )
    };
    let faellig = mako_fristen::add_werktage(heute, 15, mako_fristen::HolidayCalendar::BdewMaKo);
    // The fixture is a conformant invoice issued 02.11.2026 (edi-energy's
    // `seed_fixtures` suite validates it as it lies); its dates and reference
    // are rewritten for the day the seed runs.
    include_str!("../world/invoic-31002.edi")
        .replace("DEMO-INV-001", reference)
        .replace(
            "261102:0800",
            &format!(
                "{:02}{:02}{:02}:0800",
                heute.year() % 100,
                u8::from(heute.month()),
                heute.day()
            ),
        )
        .replace("202611012300", &tagesbeginn(heute))
        .replace("202611232300", &tagesbeginn(faellig))
}

/// The Stadtwerk's two Anmeldungen at Gasnetz Nachbarstadt that its
/// Gegenpartei-Akte shows: one answered after its Frist, one refused.
pub const NACHBAR_SPAET: &str = "0d6b7c1e-3f6a-4c7e-9a51-6c1f2b8e4a01";
/// See [`NACHBAR_SPAET`].
pub const NACHBAR_ABGELEHNT: &str = "0d6b7c1e-3f6a-4c7e-9a51-6c1f2b8e4a02";

/// The refusal Gasnetz Nachbarstadt answered with: `E_3005` `A16`
/// „identifizierte Marktlokation nimmt nicht an der Marktkommunikation teil".
pub const NACHBAR_ABLEHNUNG: (&str, &str) = ("A16", "E_3005");

/// How late Gasnetz Nachbarstadt answered the first Anmeldung: three hours
/// past the end of its Frist.
pub const NACHBAR_VERSPAETUNG_STUNDEN: i64 = 3;

/// The CloudEvents makod would have sent processd for the two Anmeldungen
/// (44001, LF → GNB), dated relative to `now`: each Anmeldung's APERAK
/// acceptance, then its answer. They carry no `process.initiated`, so no
/// decision module of processd acts on them; only its read model does.
pub fn nachbar_ereignisse(now: time::OffsetDateTime) -> Vec<Value> {
    use time::format_description::well_known::Rfc3339;
    let heute = mako_fristen::berlin_date(now);
    let um = |tage: i64| {
        mako_fristen::berlin_midnight(heute - time::Duration::days(tage)) + time::Duration::hours(9)
    };
    let event =
        |id: String, ce_type: &str, process: &str, at: time::OffsetDateTime, extra: Value| {
            let mut data = serde_json::json!({
                "pid": 44_001,
                "partner_mp_id": GASNETZ_NACHBARSTADT,
                "malo_id": nachbar_malo(),
            });
            if let (Some(d), Value::Object(e)) = (data.as_object_mut(), extra) {
                d.extend(e);
            }
            serde_json::json!({
                "specversion": "1.0",
                "id": id,
                "source": "/mako-seed",
                "type": ce_type,
                "subject": process,
                "time": at.format(&Rfc3339).unwrap_or_default(),
                "makopid": 44_001,
                "makoworkflow": "geli-gas-lf-anmeldung",
                "marktrole": "LF",
                "datacontenttype": "application/json",
                "data": data,
            })
        };
    let spaet_start = um(20);
    let frist =
        mako_fristen::antwort::antwortfrist(44_001, spaet_start).map_or(spaet_start, |f| f.due_at);
    let abgelehnt_start = um(10);
    let (code, liste) = NACHBAR_ABLEHNUNG;
    vec![
        event(
            format!("{NACHBAR_SPAET}-aperak"),
            "de.mako.engine.aperak.accepted",
            NACHBAR_SPAET,
            spaet_start,
            Value::Null,
        ),
        event(
            format!("{NACHBAR_SPAET}-antwort"),
            "de.mako.engine.process.completed",
            NACHBAR_SPAET,
            frist + time::Duration::hours(NACHBAR_VERSPAETUNG_STUNDEN),
            serde_json::json!({ "accepted": true }),
        ),
        event(
            format!("{NACHBAR_ABGELEHNT}-aperak"),
            "de.mako.engine.aperak.accepted",
            NACHBAR_ABGELEHNT,
            abgelehnt_start,
            Value::Null,
        ),
        event(
            format!("{NACHBAR_ABGELEHNT}-antwort"),
            "de.mako.engine.process.completed",
            NACHBAR_ABGELEHNT,
            abgelehnt_start + time::Duration::hours(2),
            serde_json::json!({ "accepted": false, "antwortcode": code, "codeliste": liste }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use rubo4e::identifiers::MarktpartnerId;

    /// A check-digit-valid MP-ID names a real company, so every party of the
    /// world satisfies neither the BDEW nor the GS1 procedure.
    #[test]
    fn every_mp_id_is_synthetic() {
        for id in [
            STADTWERK,
            VERTRIEB,
            NORDLICHT,
            BKV,
            STADTWERK_GAS,
            GASNETZ_NACHBARSTADT,
        ] {
            let mp = MarktpartnerId::new(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(
                !mp.has_valid_bdew_check_digit() && !mp.has_valid_gln_check_digit(),
                "{id} satisfies a published check-digit procedure — it would name a real company"
            );
        }
    }

    /// Gas is its own Sparte with its own Codenummern: the Stadtwerk and its
    /// Gas neighbour act on DVGW-Codenummern (`98…`), never on the Strom
    /// BDEW-Codenummer.
    #[test]
    fn the_gas_parties_hold_dvgw_codenummern() {
        for id in [STADTWERK_GAS, GASNETZ_NACHBARSTADT] {
            let mp = MarktpartnerId::new(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(mp.is_dvgw(), "{id} is no DVGW-Codenummer");
        }
        assert!(MarktpartnerId::new(STADTWERK).is_ok_and(|mp| mp.is_bdew()));
        for name in [
            "malo-gas.json",
            "marktd-grid-gas.json",
            "vertragd-vertrag-gas.json",
        ] {
            assert!(
                body(name).to_string().contains(STADTWERK_GAS),
                "{name} names the Stadtwerk's Gas Netzbetreiber"
            );
        }
    }

    /// The NN-Rechnung carries the day the seed runs: none of the fixture's
    /// own dates or its reference survives the rewrite.
    #[test]
    fn the_nn_rechnung_is_dated_the_day_it_is_sent() {
        let heute = time::macros::date!(2026 - 12 - 01);
        let edi = nn_rechnung_31002(heute, "SEED-X");
        for fixture in [
            "DEMO-INV-001",
            "261102:0800",
            "202611012300",
            "202611232300",
        ] {
            assert!(
                !edi.contains(fixture),
                "{fixture} survived the rewrite:\n{edi}"
            );
        }
        assert!(edi.contains("DTM+137:202611302300?+00:303'"), "{edi}");
    }

    /// The NN-Rechnung is one makod accepts at its door — no AHB error under
    /// the profile in force today — and its one defect is the price: it bills
    /// a different Arbeitspreis than the sender's Preisblatt publishes.
    #[test]
    fn the_nn_rechnung_passes_its_profile_and_disagrees_with_the_preisblatt() {
        use edi_energy::EdiEnergyMessage as _;
        let heute = mako_fristen::heute();
        let edi = nn_rechnung_31002(heute, "SEEDNN01");
        let msg = edi_energy::Platform::with_all_profiles()
            .parse(edi.as_bytes())
            .unwrap_or_else(|e| panic!("{e}\n{edi}"));
        let report = msg
            .validate_on_date(heute)
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(
            report.errors().is_empty(),
            "makod would reject the NN-Rechnung: {:#?}",
            report.errors()
        );
        assert!(edi.contains("RFF+Z13:31002'"));
        assert!(edi.contains(&format!("LOC+172+{}'", nachbar_malo())));
        assert!(edi.contains(&format!("NAD+MS+{GASNETZ_NACHBARSTADT}::332")));
        assert!(edi.contains(&format!("NAD+MR+{STADTWERK_GAS}::332")));
        assert!(edi.contains(&format!("PRI+CAL:{NN_ARBEITSPREIS_RECHNUNG}'")));
        let preis = &body("marktd-preisblatt-gas.json")["data"]["preispositionen"][0]["preisstaffeln"]
            [0]["preis"];
        assert_eq!(preis.to_string(), NN_ARBEITSPREIS_PREISBLATT);
        assert_ne!(NN_ARBEITSPREIS_PREISBLATT, NN_ARBEITSPREIS_RECHNUNG);
    }

    /// The one MP-ID that must carry a valid check digit — meterstore
    /// refuses any other sender — does.
    #[test]
    fn the_reading_sender_is_the_one_valid_mp_id() {
        let mp = MarktpartnerId::new(MSB_MESSWERTE).unwrap_or_else(|e| panic!("{e}"));
        assert!(mp.has_valid_bdew_check_digit());
    }

    /// The dead letter's PID is one no Anwendungsübersicht publishes, so it is
    /// a counterparty's error and never a process mako fails to run.
    #[test]
    fn the_dead_letter_pid_is_unpublished() {
        let overview = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../crates/edi-energy/profiles/pid-overview.json"),
        )
        .expect("the PID overview is in the repository");
        assert!(
            overview.contains("\"55001\""),
            "the overview lists PIDs as strings"
        );
        assert!(!overview.contains(&format!("\"{UNPUBLISHED_PID}\"")));
    }

    /// Every Marktlokation the world writes is one the storage layer accepts,
    /// and no two coincide.
    #[test]
    fn every_malo_carries_its_check_digit_and_is_distinct() {
        let mut all: Vec<String> = (0..GRID_MALOS).map(grid_malo).collect();
        all.extend((0..GAS_MALOS).map(gas_malo));
        all.extend([ERIKA_MALO.to_owned(), EEG_MALO.to_owned(), nachbar_malo()]);
        for m in &all {
            assert!(MaloId::new(m).is_ok(), "{m}");
            assert_eq!(melo_for(m).len(), 33, "{m}");
        }
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len());
    }

    /// The 55001 names the MaLo, sender and Lieferbeginn it is asked to.
    #[test]
    fn the_anmeldung_is_rewritten_for_its_malo_sender_and_day() {
        let day = Date::from_calendar_date(2027, time::Month::January, 1).unwrap();
        let edi = anmeldung_55001(&freigabe_malo(), VERTRIEB, day, "SEED0001");
        assert!(edi.contains(&format!("LOC+Z16+{}'", freigabe_malo())));
        assert!(edi.contains(&format!("NAD+MS+{VERTRIEB}::293'")));
        assert!(edi.contains(&format!("UNB+UNOC:3+{VERTRIEB}:500+")));
        // 01.01.2027 begins at 23:00 UTC on 31.12.2026 (MEZ).
        assert!(edi.contains("DTM+92:202612312300?+00:303'"), "{edi}");
        assert!(!edi.contains(NORDLICHT));
        for name in [
            "malo.json",
            "preisblatt-nb.json",
            "vertragd-kunde.json",
            "einsd-anlage.json",
            "accountingd-payment.json",
        ] {
            assert!(!body(name).is_null(), "{name}");
        }
    }

    /// The late answer lands after its Frist and the refusal inside it, both
    /// under the neighbouring GNB, and none of the events opens a process a
    /// decision module would act on.
    #[test]
    fn the_neighbours_answers_are_one_late_and_one_refusal() {
        let now = time::macros::datetime!(2026-11-17 12:00 UTC);
        let events = nachbar_ereignisse(now);
        assert_eq!(events.len(), 4);
        assert!(
            events
                .iter()
                .all(|e| e["type"] != "de.mako.engine.process.initiated")
        );
        assert!(
            events
                .iter()
                .all(|e| e["data"]["partner_mp_id"] == GASNETZ_NACHBARSTADT)
        );
        let t = |e: &Value| {
            time::OffsetDateTime::parse(
                e["time"].as_str().unwrap_or_default(),
                &time::format_description::well_known::Rfc3339,
            )
            .expect("RFC 3339")
        };
        let frist = mako_fristen::antwort::antwortfrist(44_001, t(&events[0]))
            .expect("44001 has an Antwortfrist")
            .due_at;
        assert!(t(&events[1]) > frist, "answered after the Frist");
        assert_eq!(events[3]["data"]["antwortcode"], "A16");
        assert!(t(&events[3]) < t(&events[2]) + time::Duration::days(1));
    }
}
