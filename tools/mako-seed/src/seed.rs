//! Writing the world: one phase per part of the Stadtwerk, each idempotent on
//! its natural key — a `PUT`, an `Idempotency-Key`, or a read that finds the
//! row already there.

use anyhow::Context as _;
use serde_json::{Value, json};
use time::{Date, Month};

use crate::api::Estate;
use crate::world::{self, STADTWERK};

/// Fill the estate.
pub async fn run(estate: &Estate) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    say("Marktlokationen, Messlokationen and the Preisblatt (marktd)");
    stammdaten(estate).await?;
    say("Marktpartner (marktd, makod) and the BKV's Zuordnungsermächtigung");
    partner(estate).await?;
    say("Anmeldungen: one answered, one held for a Freigabe, one a Klärfall (makod → processd)");
    anmeldungen(estate).await?;
    say("an EEG plant and its June feed-in, settled (einsd, edmd)");
    eeg(estate).await?;
    say("a household customer: tariff, contract, invoice, document, payment");
    o2c(estate).await?;
    say("an online order with its Bestellnachweis (productd, vertragd)");
    bestellung(estate).await?;
    say("a dozen more households, one invoice held for review (vertragd, billingd)");
    haushalte(estate).await?;
    say(
        "Gas: Marktlokationen, Messlokationen, MSB, supply, a customer and contract (marktd, productd, vertragd)",
    );
    gas(estate).await?;
    say("a Gas NN-Rechnung from the neighbouring Netzbetreiber, disputed (makod → invoicd)");
    nn_rechnung(estate).await?;
    say("the neighbouring Netzbetreiber's late answer and refusal (processd's read model)");
    gegenpartei(estate).await?;
    say("last month's meter readings for every household (edmd)");
    ablesungen(estate).await?;
    say("a message no workflow takes, in makod's dead letters");
    dead_letter(estate).await?;
    println!(
        "✓ Stadtwerke Musterstadt seeded in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn say(what: &str) {
    println!("  › {what}");
}

/// The first of the month after next, Berlin: a Lieferbeginn every Frist of
/// an Anmeldung sent today admits.
pub fn lieferbeginn() -> Date {
    let today = mako_fristen::heute();
    let (mut year, mut month) = (today.year(), today.month());
    for _ in 0..2 {
        if month == Month::December {
            year += 1;
        }
        month = month.next();
    }
    Date::from_calendar_date(year, month, 1).unwrap_or(today)
}

async fn stammdaten(estate: &Estate) -> anyhow::Result<()> {
    estate
        .put(
            "marktd",
            &format!("/api/v1/preisblaetter/{STADTWERK}"),
            &world::body("preisblatt-nb.json"),
        )
        .await?
        .ok("PUT preisblatt")?;
    // The gMSB's Messentgelte and the Konzessionsabgabe for Tarifkunden: what
    // productd prices an Endkunden-Angebot from, beside the Netznutzung.
    estate
        .put(
            "marktd",
            &format!("/api/v1/preisblaetter-messung/{STADTWERK}"),
            &world::body("marktd-preisblatt-messung.json"),
        )
        .await?
        .ok("PUT preisblatt messung")?;
    estate
        .put(
            "marktd",
            &format!("/api/v1/preisblaetter-ka/{STADTWERK}"),
            &world::body("marktd-preisblatt-ka.json"),
        )
        .await?
        .ok("PUT preisblatt ka")?;

    // The KAV-Grenzpreis the Konzessionsabgabe check reads — a synthetic
    // demo value, imported with the seed file itself as its source, never a
    // published figure. marktd keys an import on its bytes, so a second run
    // is `bereits_importiert`.
    estate
        .post_text(
            "marktd",
            &format!(
                "/api/v1/referenzreihen/kav_grenzpreis_strom/import?format=generisch&status=vorlaeufig&quelle_url={}",
                world::REFERENZWERT_QUELLE.replace('#', "%23")
            ),
            world::REFERENZWERT_CSV,
            "text/csv; charset=utf-8",
        )
        .await?
        .ok("POST referenzreihen import")?;

    let mut malos: Vec<String> = (0..world::GRID_MALOS).map(world::grid_malo).collect();
    malos.push(world::ERIKA_MALO.to_owned());
    for (n, malo) in malos.iter().enumerate() {
        let mut body = world::body("malo.json");
        body["data"]["marktlokationsId"] = json!(malo);
        body["data"]["lokationsadresse"]["hausnummer"] = json!((n + 1).to_string());
        body["data"]["lokationsadresse"]["strasse"] = json!(STRASSEN[n % STRASSEN.len()]);
        body["data"]["lokationsadresse"]["postleitzahl"] = json!("12345");
        body["data"]["lokationsadresse"]["ort"] = json!("Musterstadt");
        estate
            .put("marktd", &format!("/api/v1/malos/{malo}"), &body)
            .await?
            .ok("PUT malo")?;
        estate
            .put(
                "marktd",
                &format!("/api/v1/malos/{malo}/grid"),
                &world::body("marktd-grid.json"),
            )
            .await?
            .ok("PUT malo grid")?;
        let melo = world::melo_for(malo);
        let mut melo_body = world::body("marktd-melo.json");
        melo_body["malo_id"] = json!(malo);
        melo_body["data"]["messlokationsId"] = json!(melo);
        estate
            .put("marktd", &format!("/api/v1/melos/{melo}"), &melo_body)
            .await?
            .ok("PUT melo")?;
        let mut edge = world::body("marktd-lokationszuordnung.json");
        edge["von_id"] = json!(malo);
        edge["nach_id"] = json!(melo);
        estate
            .put("marktd", "/api/v1/lokationszuordnungen", &edge)
            .await?
            .ok("PUT lokationszuordnung")?;
        estate
            .put(
                "marktd",
                &format!("/api/v1/melos/{melo}/msb"),
                &world::body("marktd-melo-msb.json"),
            )
            .await?
            .ok("PUT melo msb")?;
        // Supplied until the end of June and free since — an Anmeldung on it
        // needs no Beendigung of a running Zuordnung first — except the
        // Klärfall's MaLo, of which marktd holds no supply record at all.
        if *malo != world::klaerfall_malo() {
            let mut versorgung = world::body("marktd-versorgung.json");
            if malo == world::ERIKA_MALO {
                versorgung = json!({
                    "lieferstatus": "Beliefert",
                    "lf_mp_id": STADTWERK,
                    "lieferbeginn": "2026-01-01",
                    "msb_mp_id": STADTWERK,
                    "nb_mp_id": STADTWERK,
                });
            }
            estate
                .put("marktd", &format!("/api/v1/versorgung/{malo}"), &versorgung)
                .await?
                .ok("PUT versorgung")?;
        }
    }
    Ok(())
}

const STRASSEN: &[&str] = &[
    "Am Markt",
    "Lindenallee",
    "Bahnhofstraße",
    "Gartenweg",
    "Schulstraße",
    "Mühlenweg",
];

async fn partner(estate: &Estate) -> anyhow::Result<()> {
    for (mp_id, name) in [
        (world::NORDLICHT, "Nordlicht Energie GmbH"),
        (world::VERTRIEB, "Musterstadt Energievertrieb GmbH"),
    ] {
        let mut body = world::body("marktd-partner-lf.json");
        body["display_name"] = json!(name);
        estate
            .put("marktd", &format!("/api/v1/partners/{mp_id}"), &body)
            .await?
            .ok("PUT marktd partner")?;
        let mut record = world::body("makod-partner-lf.json");
        record["mp_id"] = json!(mp_id);
        record["display_name"] = json!(name);
        estate
            .put("makod", &format!("/admin/partners/{mp_id}"), &record)
            .await?
            .ok("PUT makod partner")?;
    }
    estate
        .put(
            "makod",
            &format!("/admin/partners/{}", world::BKV),
            &world::body("makod-partner-bkv.json"),
        )
        .await?
        .ok("PUT makod partner BKV")?;
    // One Zuordnungsermächtigung per supplier: E_0622 Prüfschritt 60 asks
    // for it „je ZRT, BG, BK und LF" (BK6-24-174 Anlage 3 Kap. 10.2.1).
    for (lf, reference) in [
        (world::NORDLICHT, "SEEDZE01"),
        (world::VERTRIEB, "SEEDZE02"),
    ] {
        let answer = estate
            .edifact(&world::zuordnungsermaechtigung_55071(lf, reference))
            .await?
            .ok("POST /edifact 55071")?;
        let status = answer["messages"][0]["status"].as_str().unwrap_or_default();
        // A repeated interchange is makod's duplicate, which is this step already done.
        anyhow::ensure!(
            status == "routed" || answer.to_string().contains("duplicate"),
            "55071 for {lf} not routed: {answer}"
        );
    }
    Ok(())
}

/// processd's decision on `malo`, if it has made one.
pub async fn decision(estate: &Estate, malo: &str) -> anyhow::Result<Option<Value>> {
    let all = estate
        .get("processd", "/api/v1/decisions")
        .await?
        .ok("GET decisions")?;
    Ok(all
        .as_array()
        .and_then(|a| a.iter().find(|d| d["malo_id"] == malo).cloned()))
}

async fn anmeldungen(estate: &Estate) -> anyhow::Result<()> {
    let beginn = lieferbeginn();
    for (malo, sender, reference) in [
        (world::anmeldung_malo(), world::NORDLICHT, "SEED55001A"),
        (world::freigabe_malo(), world::VERTRIEB, "SEED55001B"),
        (world::klaerfall_malo(), world::NORDLICHT, "SEED55001C"),
    ] {
        // Done when processd has decided it and makod still holds the
        // process — makod keeps its event store in memory, so after a restart
        // the Anmeldung arrives again.
        if decision(estate, &malo).await?.is_some() {
            let known = estate
                .tool("makod", "get_process", json!({ "business_key": malo }))
                .await
                .is_ok_and(|p| !p.is_null());
            if known {
                continue;
            }
        }
        let answer = estate
            .edifact(&world::anmeldung_55001(&malo, sender, beginn, reference))
            .await?
            .ok("POST /edifact 55001")?;
        let status = answer["messages"][0]["status"].as_str().unwrap_or_default();
        anyhow::ensure!(
            status == "routed" || status == "accepted",
            "55001 for {malo}: {answer}"
        );
        estate
            .until(60, &format!("processd's decision on {malo}"), || async {
                decision(estate, &malo).await
            })
            .await?;
    }
    Ok(())
}

async fn eeg(estate: &Estate) -> anyhow::Result<()> {
    estate
        .put(
            "marktd",
            &format!("/api/v1/malos/{}", world::EEG_MALO),
            &world::body("malo-erzeugung.json"),
        )
        .await?
        .ok("PUT Erzeugungs-MaLo")?;
    estate
        .put(
            "einsd",
            &format!("/api/v1/einspeiser/{}", world::EEG_EINSPEISER),
            &world::body("einsd-einspeiser.json"),
        )
        .await?
        .ok("PUT einspeiser")?;
    estate
        .put(
            "einsd",
            &format!("/api/v1/anlagen/{}", world::EEG_TR),
            &world::body("einsd-anlage.json"),
        )
        .await?
        .ok("PUT anlage")?;

    // June 2026: the 1st in quarter hours of 1 kWh, the other 29 days as daily
    // buckets of 96 kWh — 2880 kWh in all, under OBIS 1-0:2.8.0 (Einspeisung).
    let mut quarters = Vec::new();
    for i in 0..96u32 {
        let (from, to) = (i * 15, i * 15 + 15);
        let at = |m: u32| {
            if m == 24 * 60 {
                "2026-06-02T00:00:00Z".to_owned()
            } else {
                format!("2026-06-01T{:02}:{:02}:00Z", m / 60, m % 60)
            }
        };
        quarters
            .push(json!({"from": at(from), "to": at(to), "value": "1.0", "quality": "MEASURED"}));
    }
    let mut days = Vec::new();
    for day in 2..=30u32 {
        let to = if day == 30 {
            "2026-07-01T00:00:00Z".to_owned()
        } else {
            format!("2026-06-{:02}T00:00:00Z", day + 1)
        };
        days.push(json!({"from": format!("2026-06-{day:02}T00:00:00Z"), "to": to, "value": "96.0", "quality": "MEASURED"}));
    }
    for intervals in [quarters, days] {
        estate
            .post(
                "edmd",
                &format!("/api/v1/meter-reads/rlm/{}", world::EEG_MALO),
                &json!({
                    "intervals": intervals,
                    "source": "DIRECT_PUSH",
                    "obis_code": "1-0:2.8.0",
                    "sender_mp_id": world::MSB_MESSWERTE,
                }),
            )
            .await?
            .ok("POST meter-reads")?;
    }
    let settled = estate
        .post(
            "einsd",
            &format!("/api/v1/anlagen/{}/settle/2026/6", world::EEG_TR),
            &json!({}),
        )
        .await?;
    // A month already settled is answered as such; the receipt is what counts.
    if !settled.status.is_success() && settled.status.as_u16() != 409 {
        settled.ok("POST settle 2026/6")?;
    }
    Ok(())
}

/// Erika Mustermann's supply, priced, invoiced, issued, booked and paid.
async fn o2c(estate: &Estate) -> anyhow::Result<()> {
    let lf = STADTWERK;
    estate
        .put(
            "productd",
            &format!("/api/v1/products/{lf}/{}", world::PRODUCT_STROM),
            &world::body("productd-product-strom.json"),
        )
        .await?
        .ok("PUT product")?;

    let kunde_id = match kunde_by_erp(estate, "MUSTERSTADT-K-0001").await? {
        Some(id) => id,
        None => {
            let created = estate
                .post(
                    "vertragd",
                    "/api/v1/kunden",
                    &world::body("vertragd-kunde.json"),
                )
                .await?
                .ok("POST kunde")?;
            id_of(&created).context("no customer id")?
        }
    };
    estate
        .put(
            "vertragd",
            &format!("/api/v1/kunden/{kunde_id}/zahlungsinformation"),
            &world::body("vertragd-zahlungsinformation.json"),
        )
        .await?
        .ok("PUT zahlungsinformation")?;
    let vertraege = estate
        .get("vertragd", &format!("/api/v1/kunden/{kunde_id}/vertraege"))
        .await?
        .ok("GET vertraege")?;
    if list(&vertraege).is_empty() {
        estate
            .post(
                "vertragd",
                &format!("/api/v1/kunden/{kunde_id}/vertraege"),
                &world::body("vertragd-vertrag.json"),
            )
            .await?
            .ok("POST vertrag")?;
    }

    // The invoice: one per MaLo and period.
    let mut fresh = false;
    let invoice = match invoice_for(estate, world::ERIKA_MALO).await? {
        Some(record) => record,
        None => {
            fresh = true;
            estate
                .post(
                    "billingd",
                    &format!("/api/v1/billing/{}/calculate", world::ERIKA_MALO),
                    &world::body("billingd-calculate.json"),
                )
                .await?
                .ok("POST calculate")?
        }
    };
    let billing_id = invoice["id"].as_str().context("no invoice id")?.to_owned();
    let rechnungsnummer = invoice["rechnungsnummer"]
        .as_str()
        .unwrap_or_default()
        .to_owned();

    // outputd renders no layout of its own: the reference template, proven
    // and rolled out, is the Stadtwerk's.
    let current = estate
        .get("outputd", "/api/v1/templates/INVOICE/current")
        .await?;
    if !current.status.is_success() {
        let source = estate
            .get("outputd", "/api/v1/templates/reference/INVOICE")
            .await?
            .ok("GET reference template")?;
        let source = source
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| source.to_string());
        let published = estate
            .post(
                "outputd",
                "/api/v1/templates",
                &json!({"kind": "INVOICE", "source": source}),
            )
            .await?
            .ok("POST template")?;
        let hash = published["hash"].as_str().context("no template hash")?;
        estate
            .put(
                "outputd",
                "/api/v1/templates/INVOICE/current",
                &json!({"hash": hash}),
            )
            .await?
            .ok("PUT template current")?;
    }

    let docs = estate
        .get(
            "outputd",
            &format!("/api/v1/documents?malo_id={}", world::ERIKA_MALO),
        )
        .await?
        .ok("GET documents")?;
    if list(&docs).is_empty() {
        fresh = true;
        estate
            .post(
                "billingd",
                &format!("/api/v1/billing/{billing_id}/versenden"),
                &json!({}),
            )
            .await?
            .ok("POST versenden")?;
    }

    if !fresh {
        // Invoiced, issued and paid by an earlier run: the payment import is
        // keyed on its bank_transaction_id, so nothing is left to do.
        return Ok(());
    }
    // The receivable reaches accountingd through billingd's outbox; the
    // payment is matched by the MaLo in the Verwendungszweck, and the bank
    // statement is today's — the account opens when the invoice posts.
    estate
        .until(60, "the Offener Posten", || async {
            let all = estate
                .get("accountingd", "/api/v1/offene-posten")
                .await?
                .ok("GET offene-posten")?;
            Ok(list(&all)
                .iter()
                .any(|p| p["malo_id"] == world::ERIKA_MALO)
                .then_some(()))
        })
        .await?;
    let mut payment = world::body("accountingd-payment.json");
    payment[0]["date"] = json!(mako_fristen::heute().to_string());
    payment[0]["reference"] = json!(format!(
        "Rechnung {rechnungsnummer} Marktlokation {}",
        world::ERIKA_MALO
    ));
    estate
        .post("accountingd", "/api/v1/payments/import", &payment)
        .await?
        .ok("POST payments/import")?;
    Ok(())
}

/// Clara Sommer orders online: productd prices and freezes the
/// Endkunden-Angebot, the Stadtwerk's legal documents are registered, and the
/// order — concluding the contract under `[policy] vertragsschluss =
/// "mit_bestellung"` — stores its Bestellnachweis.
async fn bestellung(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::bestellung_malo();
    let known = estate
        .tool("vertragd", "kunden_zu_malos", json!({ "malos": [malo] }))
        .await?;
    if list(&known["kunden"])
        .iter()
        .any(|k| k["malo_id"] == malo.as_str())
    {
        return Ok(());
    }
    let beginn = lieferbeginn();
    let angebot = estate
        .post(
            "productd",
            &format!("/api/v1/products/{STADTWERK}/endkunden-angebote"),
            &json!({
                "product_code": world::PRODUCT_STROM,
                "sparte": "STROM",
                "verbrauch_kwh": "2800",
                "malo_id": malo,
                "gewuenschter_lieferbeginn": beginn.to_string(),
            }),
        )
        .await?
        .ok("POST endkunden-angebot")?;
    let mut dokumente = vec![json!({ "art": "PREISBLATT", "sha256": angebot["product_version"] })];
    for art in [
        "AGB",
        "WIDERRUFSBELEHRUNG",
        "MUSTER_WIDERRUFSFORMULAR",
        "DATENSCHUTZ",
    ] {
        let doc = estate
            .put(
                "vertragd",
                &format!("/api/v1/rechtsdokumente/{art}"),
                &json!({
                    "text": format!("{art} der {}, Fassung 1", world::STADTWERK_NAME),
                    "gueltig_ab": "2026-11-01",
                }),
            )
            .await?
            .ok("PUT rechtsdokument")?;
        dokumente.push(json!({ "art": art, "sha256": doc["sha256"] }));
    }
    let mut order = world::body("vertragd-bestellung.json");
    order["angebot_ref"] = json!({ "id": angebot["id"], "sha256": angebot["content_sha256"] });
    order["kunde"]["malo_id"] = json!(malo);
    order["gewuenschter_lieferbeginn"] = json!(beginn.to_string());
    order["dokumente"] = json!(dokumente);
    order["angezeigte_informationen"]["gesamtpreis"] = json!(format!(
        "{} EUR/Jahr",
        angebot["jahreskosten_brutto_eur"].as_str().unwrap_or("")
    ));
    order["angezeigte_informationen"]["gesamtpreis_pro_abrechnungszeitraum"] = json!(format!(
        "{} EUR/Monat",
        angebot["abschlag_monat_brutto_eur"].as_str().unwrap_or("")
    ));
    order["bestellt_am"] = json!(
        mako_fristen::berlin_now()
            .format(&time::format_description::well_known::Rfc3339)
            .context("the order instant")?
    );
    estate
        .post_once(
            "vertragd",
            "/api/v1/bestellungen",
            &order,
            &format!("seed-bestellung-{malo}"),
        )
        .await?
        .ok("POST bestellung")?;
    Ok(())
}

/// The households: a customer and a contract each. The first one's meter
/// read nothing in a period overlapping its January invoice, so its second
/// invoice scores `ZERO_ENERGY` and `PERIOD_OVERLAP` and billingd's risk gate
/// holds it for an analyst.
async fn haushalte(estate: &Estate) -> anyhow::Result<()> {
    for (i, (vorname, nachname)) in world::HAUSHALTE.iter().enumerate() {
        let malo = world::haushalt_malo(i);
        let erp = format!("MUSTERSTADT-K-{:04}", i + 2);
        let kunde_id = match kunde_by_erp(estate, &erp).await? {
            Some(id) => id,
            None => {
                let mut kunde = world::body("vertragd-kunde.json");
                let email = format!(
                    "{}.{}@example.org",
                    vorname.to_lowercase(),
                    nachname.to_lowercase()
                );
                kunde["erp_kunde_id"] = json!(erp);
                kunde["email"] = json!(email);
                kunde["geschaeftspartner"]["anrede"] =
                    json!(if i % 2 == 0 { "HERR" } else { "FRAU" });
                kunde["geschaeftspartner"]["vorname"] = json!(vorname);
                kunde["geschaeftspartner"]["nachname"] = json!(nachname);
                kunde["geschaeftspartner"]["adresse"]["strasse"] =
                    json!(STRASSEN[(i + 3) % STRASSEN.len()]);
                kunde["geschaeftspartner"]["adresse"]["hausnummer"] = json!((i + 4).to_string());
                kunde["geschaeftspartner"]["kontaktwege"][0]["kontaktwert"] = json!(email);
                let created = estate
                    .post("vertragd", "/api/v1/kunden", &kunde)
                    .await?
                    .ok("POST kunde")?;
                id_of(&created).context("no customer id")?
            }
        };
        let vertraege = estate
            .get("vertragd", &format!("/api/v1/kunden/{kunde_id}/vertraege"))
            .await?
            .ok("GET vertraege")?;
        if list(&vertraege).is_empty() {
            let mut vertrag = world::body("vertragd-vertrag.json");
            vertrag["erp_contract_id"] = json!(format!("MUSTERSTADT-V-{malo}"));
            vertrag["komponenten"][0]["malo_id"] = json!(malo);
            vertrag["komponenten"][0]["jahresverbrauch_kwh"] = json!(format!("{}", 1800 + i * 250));
            estate
                .post(
                    "vertragd",
                    &format!("/api/v1/kunden/{kunde_id}/vertraege"),
                    &vertrag,
                )
                .await?
                .ok("POST vertrag")?;
        }
    }

    let leer = world::haushalt_malo(0);
    let all = estate
        .get("billingd", &format!("/api/v1/billing?malo_id={leer}"))
        .await?
        .ok("GET billing")?;
    let mine: Vec<Value> = list(&all)
        .into_iter()
        .filter(|r| r["malo_id"] == leer.as_str())
        .collect();
    if mine.is_empty() {
        estate
            .post(
                "billingd",
                &format!("/api/v1/billing/{leer}/calculate"),
                &world::body("billingd-calculate.json"),
            )
            .await?
            .ok("POST calculate (January)")?;
    }
    if mine.len() < 2 {
        let mut zweite = world::body("billingd-calculate.json");
        zweite["period_from"] = json!("2026-01-15");
        zweite["period_to"] = json!("2026-02-14");
        zweite["meter"]["arbeitsmenge_kwh"] = json!("0");
        estate
            .post(
                "billingd",
                &format!("/api/v1/billing/{leer}/calculate"),
                &zweite,
            )
            .await?
            .ok("POST calculate (the overlapping zero)")?;
    }
    Ok(())
}

/// The Stadtwerk's Gas grid, on its DVGW-Codenummer: each Marktlokation with
/// its Messlokation, MSB and supply record as the Strom ones carry, the Gas
/// household product, and one Gas customer with her contract.
async fn gas(estate: &Estate) -> anyhow::Result<()> {
    let gas = world::STADTWERK_GAS;
    for (i, n) in (0..world::GAS_MALOS).enumerate() {
        let malo = world::gas_malo(n);
        let mut body = world::body("malo-gas.json");
        body["data"]["marktlokationsId"] = json!(malo);
        body["data"]["lokationsadresse"]["hausnummer"] = json!((i + 1).to_string());
        body["data"]["lokationsadresse"]["strasse"] = json!(STRASSEN[(i + 2) % STRASSEN.len()]);
        estate
            .put("marktd", &format!("/api/v1/malos/{malo}"), &body)
            .await?
            .ok("PUT Gas malo")?;
        estate
            .put(
                "marktd",
                &format!("/api/v1/malos/{malo}/grid"),
                &world::body("marktd-grid-gas.json"),
            )
            .await?
            .ok("PUT Gas malo grid")?;
        let melo = world::melo_for(&malo);
        let mut melo_body = world::body("marktd-melo.json");
        melo_body["malo_id"] = json!(malo);
        melo_body["data"]["messlokationsId"] = json!(melo);
        melo_body["data"]["sparte"] = json!("GAS");
        estate
            .put("marktd", &format!("/api/v1/melos/{melo}"), &melo_body)
            .await?
            .ok("PUT Gas melo")?;
        let mut edge = world::body("marktd-lokationszuordnung.json");
        edge["von_id"] = json!(malo);
        edge["nach_id"] = json!(melo);
        estate
            .put("marktd", "/api/v1/lokationszuordnungen", &edge)
            .await?
            .ok("PUT Gas lokationszuordnung")?;
        let mut msb = world::body("marktd-melo-msb.json");
        msb["msb_mp_id"] = json!(gas);
        estate
            .put("marktd", &format!("/api/v1/melos/{melo}/msb"), &msb)
            .await?
            .ok("PUT Gas melo msb")?;
        estate
            .put(
                "marktd",
                &format!("/api/v1/versorgung/{malo}"),
                &json!({
                    "lieferstatus": "Beliefert",
                    "lf_mp_id": gas,
                    "lieferbeginn": "2026-01-01",
                    "msb_mp_id": gas,
                    "nb_mp_id": gas,
                }),
            )
            .await?
            .ok("PUT Gas versorgung")?;
    }

    estate
        .put(
            "productd",
            &format!("/api/v1/products/{STADTWERK}/{}", world::PRODUCT_GAS),
            &world::body("productd-product-gas.json"),
        )
        .await?
        .ok("PUT Gas product")?;

    let (vorname, nachname) = world::GAS_KUNDE;
    let erp = "MUSTERSTADT-KG-0001";
    let kunde_id = match kunde_by_erp(estate, erp).await? {
        Some(id) => id,
        None => {
            let mut kunde = world::body("vertragd-kunde.json");
            let email = format!(
                "{}.{}@example.org",
                vorname.to_lowercase(),
                nachname.to_lowercase()
            );
            kunde["erp_kunde_id"] = json!(erp);
            kunde["email"] = json!(email);
            kunde["geschaeftspartner"]["vorname"] = json!(vorname);
            kunde["geschaeftspartner"]["nachname"] = json!(nachname);
            kunde["geschaeftspartner"]["adresse"]["strasse"] = json!("Gartenweg");
            kunde["geschaeftspartner"]["kontaktwege"][0]["kontaktwert"] = json!(email);
            let created = estate
                .post("vertragd", "/api/v1/kunden", &kunde)
                .await?
                .ok("POST Gas kunde")?;
            id_of(&created).context("no customer id")?
        }
    };
    let vertraege = estate
        .get("vertragd", &format!("/api/v1/kunden/{kunde_id}/vertraege"))
        .await?
        .ok("GET vertraege")?;
    if list(&vertraege).is_empty() {
        let malo = world::gas_malo(0);
        let mut vertrag = world::body("vertragd-vertrag-gas.json");
        vertrag["erp_contract_id"] = json!(format!("MUSTERSTADT-VG-{malo}"));
        vertrag["komponenten"][0]["malo_id"] = json!(malo);
        estate
            .post(
                "vertragd",
                &format!("/api/v1/kunden/{kunde_id}/vertraege"),
                &vertrag,
            )
            .await?
            .ok("POST Gas vertrag")?;
    }
    Ok(())
}

/// invoicd's receipt of the disputed NN-Rechnung, if it has one.
pub async fn nn_receipt(estate: &Estate) -> anyhow::Result<Option<Value>> {
    let all = estate
        .get(
            "invoicd",
            &format!(
                "/api/v1/receipts?sender_mp_id={}",
                world::GASNETZ_NACHBARSTADT
            ),
        )
        .await?
        .ok("GET receipts")?;
    Ok(list(&all)
        .into_iter()
        .find(|r| r["rechnungsnummer"] == world::NN_RECHNUNG))
}

/// Gasnetz Nachbarstadt — its Marktpartner record and its published
/// Preisblatt — and the NN-Rechnung it sends the Stadtwerk's Lieferant at a
/// price that Preisblatt does not publish. makod receives it at its EDIFACT
/// door and announces it; invoicd checks it against the Preisblatt marktd
/// holds and disputes it.
async fn nn_rechnung(estate: &Estate) -> anyhow::Result<()> {
    let gnb = world::GASNETZ_NACHBARSTADT;
    estate
        .put(
            "marktd",
            &format!("/api/v1/partners/{gnb}"),
            &world::body("marktd-partner-gnb.json"),
        )
        .await?
        .ok("PUT marktd partner GNB")?;
    estate
        .put(
            "makod",
            &format!("/admin/partners/{gnb}"),
            &world::body("makod-partner-gnb.json"),
        )
        .await?
        .ok("PUT makod partner GNB")?;
    estate
        .put(
            "marktd",
            &format!("/api/v1/preisblaetter/{gnb}"),
            &world::body("marktd-preisblatt-gas.json"),
        )
        .await?
        .ok("PUT Gas preisblatt")?;

    // invoicd keeps its receipts; makod's in-memory process of an earlier
    // run is not needed for it to stay disputed.
    if nn_receipt(estate).await?.is_some() {
        return Ok(());
    }
    let answer = estate
        .edifact(&world::nn_rechnung_31002(mako_fristen::heute(), "SEEDNN01"))
        .await?
        .ok("POST /edifact 31002")?;
    let status = answer["messages"][0]["status"].as_str().unwrap_or_default();
    anyhow::ensure!(
        status == "routed" || answer.to_string().contains("duplicate"),
        "the NN-Rechnung was not routed: {answer}"
    );
    estate
        .until(60, "invoicd's receipt of the NN-Rechnung", || async {
            nn_receipt(estate).await
        })
        .await?;
    Ok(())
}

/// Gasnetz Nachbarstadt's Gegenpartei-Akte: the read-model events of two
/// Anmeldungen it owed an answer — one answered three hours late, one refused
/// `A16` — posted to processd's webhook as makod's fan-out would. Keyed on fixed
/// process ids: a second run finds the rows and moves neither the start, the
/// Frist nor the answer.
async fn gegenpartei(estate: &Estate) -> anyhow::Result<()> {
    for event in world::nachbar_ereignisse(time::OffsetDateTime::now_utc()) {
        estate
            .post("processd", "/webhook", &event)
            .await?
            .ok("POST processd /webhook")?;
    }
    Ok(())
}

/// Last calendar month's consumption of every household, one value per
/// Berlin day under OBIS 1-0:1.8.0 (Bezug), as its MSB — the Stadtwerk — would
/// push it. edmd keys a reading on its MaLo, register and interval, so a
/// second run stores nothing new.
async fn ablesungen(estate: &Estate) -> anyhow::Result<()> {
    let today = mako_fristen::heute();
    let first_this = today.replace_day(1).context("the first of this month")?;
    let last_prev = first_this
        .previous_day()
        .context("the last of last month")?;
    let first_prev = last_prev
        .replace_day(1)
        .context("the first of last month")?;
    let rfc = |d: Date| {
        mako_fristen::berlin_midnight(d)
            .to_offset(time::UtcOffset::UTC)
            .format(&time::format_description::well_known::Rfc3339)
    };
    let mut malos: Vec<(String, usize)> = vec![(world::ERIKA_MALO.to_owned(), 3000)];
    for i in 0..world::HAUSHALTE.len() {
        malos.push((world::haushalt_malo(i), 1800 + i * 250));
    }
    for (n, (malo, jahr_kwh)) in malos.iter().enumerate() {
        let mut intervals = Vec::new();
        let mut day = first_prev;
        let mut k = 0usize;
        while day <= last_prev {
            let next = day.next_day().context("the next day")?;
            // A day's share of the year, in Wh, varied a little day by day so a
            // chart shows a household rather than a flat line.
            let wh = jahr_kwh * 1000 / 365 + ((k * 7 + n * 3) % 11) * 120;
            intervals.push(json!({
                "from": rfc(day)?,
                "to": rfc(next)?,
                "value": format!("{}.{:03}", wh / 1000, wh % 1000),
                "quality": "MEASURED",
            }));
            day = next;
            k += 1;
        }
        // The first household's meter read nothing — the Leerstand its held
        // invoice is about.
        if n == 1 {
            for i in &mut intervals {
                i["value"] = json!("0.000");
            }
        }
        estate
            .post(
                "edmd",
                &format!("/api/v1/meter-reads/rlm/{malo}"),
                &json!({
                    "intervals": intervals,
                    "source": "DIRECT_PUSH",
                    "obis_code": "1-0:1.8.0",
                    "sender_mp_id": world::MSB_MESSWERTE,
                }),
            )
            .await?
            .ok("POST meter-reads")?;
    }
    Ok(())
}

/// A UTILMD whose Prüfidentifikator no Anwendungsübersicht publishes — a
/// counterparty's typo — reaches makod and is dead-lettered as `unknown_pid`:
/// an inbound message no workflow can take waits for a person.
async fn dead_letter(estate: &Estate) -> anyhow::Result<()> {
    let held = estate.tool("makod", "list_dead_letters", json!({})).await?;
    if held["gesamt"].as_u64().unwrap_or(0) > 0 {
        return Ok(());
    }
    let edi = world::anmeldung_55001(
        &world::grid_malo(20),
        world::NORDLICHT,
        lieferbeginn(),
        "SEEDDLQ01",
    )
    .replace(
        "RFF+Z13:55001",
        &format!("RFF+Z13:{}", world::UNPUBLISHED_PID),
    );
    let answer = estate
        .edifact(&edi)
        .await?
        .ok("POST /edifact (unknown PID)")?;
    anyhow::ensure!(
        answer["messages"][0]["status"] == "unknown_pid",
        "the unknown PID was not dead-lettered: {answer}"
    );
    Ok(())
}

/// The id of the customer whose `erp_kunde_id` is `erp`, if there is one.
async fn kunde_by_erp(estate: &Estate, erp: &str) -> anyhow::Result<Option<String>> {
    let all = estate
        .get("vertragd", "/api/v1/kunden?limit=500")
        .await?
        .ok("GET kunden")?;
    Ok(list(&all)
        .iter()
        .find(|k| k["erp_kunde_id"] == erp || k["kunde"]["erp_kunde_id"] == erp)
        .and_then(id_of))
}

/// billingd's invoice for `malo`, if there is one.
pub async fn invoice_for(estate: &Estate, malo: &str) -> anyhow::Result<Option<Value>> {
    let all = estate
        .get("billingd", &format!("/api/v1/billing?malo_id={malo}"))
        .await?
        .ok("GET billing")?;
    Ok(list(&all).into_iter().find(|r| r["malo_id"] == malo))
}

/// The rows of a list answer: a bare array, or the array under a usual key.
pub fn list(v: &Value) -> Vec<Value> {
    if let Some(a) = v.as_array() {
        return a.clone();
    }
    for key in [
        "items",
        "posten",
        "documents",
        "vertraege",
        "kunden",
        "records",
        "data",
    ] {
        if let Some(a) = v[key].as_array() {
            return a.clone();
        }
    }
    Vec::new()
}

fn id_of(v: &Value) -> Option<String> {
    for key in ["id", "kunden_id", "kunde_id"] {
        if let Some(s) = v[key].as_str() {
            return Some(s.to_owned());
        }
        if let Some(s) = v["kunde"][key].as_str() {
            return Some(s.to_owned());
        }
    }
    None
}
