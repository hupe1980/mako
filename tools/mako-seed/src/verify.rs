//! `--verify`: the outcomes the seeded world must show, asserted against the
//! running estate — the figure, the code, the cent, never only a status.
//!
//! Each check names what it proves; a failure names what was found instead.

use anyhow::{Context as _, bail, ensure};
use serde_json::{Value, json};

use crate::api::Estate;
use crate::seed::{decision, invoice_for, list, nn_receipt};
use crate::world;

/// Assert every outcome, then that every Desk source has content; every
/// failure is named, not only the first.
pub async fn run(estate: &Estate) -> anyhow::Result<()> {
    let mut failed = Vec::new();
    let mut checks = 0;
    for (what, result) in [
        ("Anmeldung", anmeldung(estate).await),
        ("Freigabe", freigabe(estate).await),
        ("Klärfall", klaerfall(estate).await),
        ("EEG", eeg(estate).await),
        ("order-to-cash", o2c(estate).await),
        ("Rechnung nachrechnen", nachrechnen(estate).await),
        ("online order", bestellung(estate).await),
        ("Gas", gas(estate).await),
        ("invoice dispute", disput(estate).await),
        ("Gegenpartei-Akte", gegenpartei(estate).await),
        ("the Desk's sources", desk(estate).await),
    ] {
        match result {
            Ok(()) => checks += 1,
            Err(e) => {
                println!("  ✗ {what}: {e:#}");
                failed.push(what);
            }
        }
    }
    if !failed.is_empty() {
        bail!(
            "verify: {} of {} failed — {}",
            failed.len(),
            checks + failed.len(),
            failed.join(", ")
        );
    }
    println!("✓ verified: {checks} of {checks}");
    Ok(())
}

fn pass(what: &str) {
    println!("  ✓ {what}");
}

/// Two decimal strings equal by value, whatever scale each service chose.
fn same_amount(found: &Value, want: &str) -> bool {
    let norm = |s: &str| {
        let s = s.trim();
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        let frac = frac.trim_end_matches('0');
        let int = int.trim_start_matches('0');
        format!("{}.{frac}", if int.is_empty() { "0" } else { int })
    };
    let found = match found {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return false,
    };
    norm(&found) == norm(want)
}

/// Gasnetz Nachbarstadt's Akte as the NB the Stadtwerk's Lieferant
/// addressed: the late answer with its delay, and the refusal with its code
/// and the BDEW's wording.
async fn gegenpartei(estate: &Estate) -> anyhow::Result<()> {
    let heute = mako_fristen::heute();
    let path = format!(
        "/api/v1/counterparties/{}/NB?von={}&bis={heute}",
        world::GASNETZ_NACHBARSTADT,
        heute - time::Duration::days(40),
    );
    let akte = estate
        .get("processd", &path)
        .await?
        .ok("GET counterparty record")?;
    ensure!(
        akte["verspaetet"].as_u64() >= Some(1),
        "no late answer in the neighbour's Akte: {akte}"
    );
    let minuten = akte["verspaetung"]["max_minuten"]
        .as_i64()
        .unwrap_or_default();
    ensure!(
        minuten >= world::NACHBAR_VERSPAETUNG_STUNDEN * 60,
        "the late answer is {minuten} minutes late, not three hours: {akte}"
    );
    let (code, _) = world::NACHBAR_ABLEHNUNG;
    let refused = akte["ablehnungen"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["code"] == code).cloned())
        .ok_or_else(|| anyhow::anyhow!("no {code} refusal in the neighbour's Akte: {akte}"))?;
    ensure!(
        refused["bedeutung"].as_str().is_some_and(|b| !b.is_empty()),
        "the {code} refusal carries no wording: {refused}"
    );
    pass(&format!(
        "Gasnetz Nachbarstadt answered one Anmeldung {minuten} minutes late and refused one {code}"
    ));
    Ok(())
}

/// Nordlicht's 55001 is decided `Accept` by processd — the Prüfschritte of
/// `E_0622` all pass and the answer code is `E_0623`'s `A51` „Zustimmung" —
/// and makod answers it with the 55002 Bestätigung.
async fn anmeldung(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::anmeldung_malo();
    let d = decision(estate, &malo)
        .await?
        .with_context(|| format!("no decision recorded for MaLo {malo}"))?;
    ensure!(
        d["decision"] == "Accept",
        "processd decided {} (Antwortcode {}), expected Accept: {}",
        d["decision"],
        d["antwortcode"],
        d["detail"]
    );
    pass(&format!("55001 for {malo} → processd Accept"));
    if let Some(code) = d.get("antwortcode").filter(|c| !c.is_null()) {
        ensure!(
            code == "A51",
            "Antwortcode {code}, expected A51 (E_0623 Zustimmung)"
        );
        pass("Antwortcode A51 (E_0622 → E_0623 Zustimmung)");
    }
    // The Bestätigung on makod's side: the process history shows the 55002.
    let process_id = d["process_id"]
        .as_str()
        .context("the decision names no process")?;
    estate
        .until(30, "makod's 55002 Bestätigung", || async {
            let h = estate
                .tool(
                    "makod",
                    "get_process_history",
                    json!({ "process_id": process_id }),
                )
                .await?;
            Ok(h.to_string().contains("55002").then_some(()))
        })
        .await
        .with_context(|| format!("makod's history of process {process_id}"))?;
    pass("makod answered with the 55002 Bestätigung");
    Ok(())
}

/// The Anmeldung on a Marktlokation marktd holds no supply record of is a
/// Klärfall: whether its Inbetriebnahme lies ahead (`E_0622` Prüfschritt 50)
/// is no store's answer, so processd escalates it to a person.
async fn klaerfall(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::klaerfall_malo();
    let d = decision(estate, &malo)
        .await?
        .with_context(|| format!("no decision recorded for MaLo {malo}"))?;
    ensure!(
        d["decision"] == "Escalate" && d["detail"].to_string().contains("Prüfschritt 50"),
        "processd decided {} on {malo}, expected Escalate at E_0622 Prüfschritt 50: {}",
        d["decision"],
        d["detail"]
    );
    pass(&format!(
        "55001 for {malo} escalated as a Klärfall (E_0622 Prüfschritt 50)"
    ));
    Ok(())
}

/// The affiliated supplier's 55001 waits for a person: § 20 Abs. 1 EnWG
/// parity holds an affiliate's Anmeldung, so processd queues it.
async fn freigabe(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::freigabe_malo();
    let pending = estate
        .tool("processd", "list_pending_approvals", json!({}))
        .await?;
    ensure!(
        pending.to_string().contains(&malo),
        "no pending approval names {malo}: {pending}"
    );
    pass(&format!(
        "55001 for {malo} from the affiliated supplier is held for a Freigabe"
    ));
    Ok(())
}

/// June 2026's feed-in is 2880 kWh, and its § 21 EEG 2023 Vergütung at
/// 8.11 ct/kWh is EUR 233.568 — the entitlement at sub-cent precision, rounded
/// once in the Gutschrift.
async fn eeg(estate: &Estate) -> anyhow::Result<()> {
    let series = estate
        .get(
            "edmd",
            &format!(
                "/api/v1/energy/{}?direction=EINSPEISUNG&from=2026-06-01T00:00:00%2B02:00&to=2026-07-01T00:00:00%2B02:00",
                world::EEG_MALO
            ),
        )
        .await?
        .ok("GET energy")?;
    // Summed in thousandths of a kWh, so no float touches the figure.
    let mut milli: i64 = 0;
    for i in series["intervals"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let kwh = match &i["kwh"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let (int, frac) = kwh.split_once('.').unwrap_or((&kwh, ""));
        let frac = format!("{frac:0<3}");
        milli += int.parse::<i64>().unwrap_or(0) * 1000 + frac[..3].parse::<i64>().unwrap_or(0);
    }
    ensure!(
        milli == 2_880_000,
        "edmd reports {}.{:03} kWh Einspeisung for June 2026, expected 2880",
        milli / 1000,
        milli % 1000
    );
    pass("edmd: 2880 kWh Einspeisung in June 2026 (OBIS 1-0:2.8.0)");

    let receipts = estate
        .get(
            "einsd",
            &format!(
                "/api/v1/anlagen/{}/settlements?year=2026&month=6",
                world::EEG_TR
            ),
        )
        .await?
        .ok("GET settlements")?;
    let r = list(&receipts).into_iter().next().unwrap_or(receipts);
    ensure!(
        r["status"] == "calculated" && r["billing_month"] == 6,
        "the June receipt is {} for month {}, expected calculated for 6",
        r["status"],
        r["billing_month"]
    );
    ensure!(
        same_amount(&r["settlement_eur"], "233.568"),
        "settlement_eur is {}, expected 233.568 (2880 kWh × 8.11 ct)",
        r["settlement_eur"]
    );
    pass("einsd: June settled, EUR 233.568 (2880 kWh × 8.11 ct/kWh, § 21 EEG 2023)");
    Ok(())
}

/// Erika's January: Grundpreis 20 ct × 31 Tage 6.20, Arbeitspreis 32 ct ×
/// 250 kWh 80.00, Stromsteuer 2.05 ct × 250 kWh 5.13, Netznutzung 8 ct ×
/// 250 kWh 20.00 — netto 111.33, USt 19 % 21.15, brutto 132.48; issued as a
/// PDF document, booked as an Offener Posten of 13 248 ct, closed by a payment
/// matched by the MaLo in its Verwendungszweck.
/// Erika Mustermann's invoice recomputes „Stimmt überein" on portald's
/// public page, reached by the link her invoice carries — the customer's
/// path, with no sign-in.
async fn nachrechnen(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::ERIKA_MALO;
    let invoice = invoice_for(estate, malo)
        .await?
        .with_context(|| format!("no invoice for {malo}"))?;
    let id = invoice["id"].as_str().context("invoice id")?;
    let view = estate
        .get("billingd", &format!("/api/v1/billing/{id}/nachrechnung"))
        .await?
        .ok("GET nachrechnung")?;
    let link = view["public_link"]
        .as_str()
        .context("the invoice carries no Nachrechnen link — is billingd's [nachrechnen] set?")?;
    // The link names the host a customer's browser reaches; the seeder takes
    // the token from it and asks portald where the seeder reaches it.
    let token = link
        .rsplit('/')
        .next()
        .filter(|t| !t.is_empty())
        .context("a link without a token")?;
    let page = estate
        .get("portald", &format!("/nachrechnen/{token}"))
        .await?
        .ok("GET /nachrechnen/{token}")?;
    let html = page.as_str().unwrap_or_default();
    ensure!(
        html.contains("Stimmt überein"),
        "portald's page does not say „Stimmt überein\""
    );
    ensure!(
        !html.contains(malo),
        "the public page shows the MaLo-ID unmasked"
    );
    pass("portald /nachrechnen: Erika Mustermann's invoice „Stimmt überein\", the MaLo-ID masked");
    Ok(())
}

async fn o2c(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::ERIKA_MALO;
    let invoice = invoice_for(estate, malo)
        .await?
        .with_context(|| format!("no invoice for {malo}"))?;
    let id = invoice["id"].as_str().context("invoice id")?;
    let record = estate
        .get("billingd", &format!("/api/v1/billing/{id}"))
        .await?
        .ok("GET invoice")?;
    let netto = record
        .get("total_netto_eur")
        .or_else(|| record.get("netto_eur"))
        .cloned()
        .unwrap_or(Value::Null);
    let brutto = record
        .get("total_brutto_eur")
        .or_else(|| record.get("brutto_eur"))
        .cloned()
        .unwrap_or(Value::Null);
    ensure!(
        same_amount(&netto, "111.33"),
        "netto is {netto}, expected 111.33"
    );
    ensure!(
        same_amount(&brutto, "132.48"),
        "brutto is {brutto}, expected 132.48"
    );
    pass("billingd: netto 111.33, brutto 132.48 (19 % USt kaufmännisch)");

    let positionen = &record["rechnung_json"]["rechnungspositionen"];
    let nne: Vec<&Value> = positionen
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|p| p["positionstext"] == "Netznutzungsentgelt Arbeitspreis")
                .collect()
        })
        .unwrap_or_default();
    ensure!(
        nne.len() == 1 && same_amount(&nne[0]["gesamtpreis"]["wert"], "20.00"),
        "the Netznutzungsentgelt line is {nne:?}, expected one of 20.00"
    );
    pass("the Netznutzungsentgelt Arbeitspreis line is 20.00 (8 ct × 250 kWh)");

    let empf = &record["rechnung_json"]["rechnungsempfaenger"];
    ensure!(
        empf["organisationsname"] == "Erika Mustermann" && empf["adresse"]["ort"] == "Musterstadt",
        "the BO4E Rechnung names {empf} as Rechnungsempfänger"
    );
    ensure!(
        record["en16931_json"]["invoice"]["buyer"]["name"] == "Erika Mustermann",
        "the EN 16931 buyer is {}",
        record["en16931_json"]["invoice"]["buyer"]
    );
    pass("the invoice names Erika Mustermann in BO4E and EN 16931 (§ 14 Abs. 4 Nr. 1 UStG)");

    let docs = estate
        .get("outputd", &format!("/api/v1/documents?malo_id={malo}"))
        .await?
        .ok("GET documents")?;
    let doc = list(&docs)
        .into_iter()
        .find(|d| d["kind"] == "INVOICE")
        .with_context(|| format!("no INVOICE document for {malo}: {docs}"))?;
    ensure!(
        doc["media_type"] == "application/pdf",
        "the document is {}",
        doc["media_type"]
    );
    let doc_id = doc["document_id"].as_str().context("document_id")?;
    let bytes = estate
        .get("outputd", &format!("/api/v1/documents/{doc_id}/content"))
        .await?;
    ensure!(
        bytes.status.is_success(),
        "the content route answered {}",
        bytes.status
    );
    pass("outputd: the invoice is an issued PDF document");

    let open = estate
        .get("accountingd", "/api/v1/offene-posten?min_balance_eur=0.01")
        .await?
        .ok("GET offene-posten")?;
    ensure!(
        !list(&open).iter().any(|p| p["malo_id"] == malo),
        "the Offener Posten for {malo} is still open: {open}"
    );
    let all = estate
        .get("accountingd", "/api/v1/offene-posten")
        .await?
        .ok("GET offene-posten")?;
    if let Some(p) = list(&all).into_iter().find(|p| p["malo_id"] == malo) {
        ensure!(
            p["balance_ct"] == 0,
            "the account for {malo} stands at {} ct",
            p["balance_ct"]
        );
    }
    pass("accountingd: the 132.48 receivable is settled by the payment");
    Ok(())
}

/// Clara Sommer's online order concluded a contract on her Marktlokation —
/// under `[policy] vertragsschluss = "mit_bestellung"` the order is the
/// contract, stored with its Bestellnachweis in the same transaction.
async fn bestellung(estate: &Estate) -> anyhow::Result<()> {
    let malo = world::bestellung_malo();
    let kunden = estate
        .tool("vertragd", "kunden_zu_malos", json!({ "malos": [malo] }))
        .await?;
    let k = list(&kunden["kunden"])
        .into_iter()
        .find(|k| k["malo_id"] == malo.as_str())
        .with_context(|| format!("no customer on {malo}: {kunden}"))?;
    ensure!(
        k["name"] == "Clara Sommer",
        "the customer on {malo} is {}",
        k["name"]
    );
    pass(&format!(
        "the online order concluded Clara Sommer's contract on {malo}"
    ));
    Ok(())
}

/// The Stadtwerk acts in Gas on its DVGW-Codenummer: marktd serves every Gas
/// Marktlokation as Sparte Gas in the Stadtwerk's Gas grid, supplied by its
/// Gas Lieferant, and vertragd names the Gas customer on the first.
async fn gas(estate: &Estate) -> anyhow::Result<()> {
    for n in 0..world::GAS_MALOS {
        let malo = world::gas_malo(n);
        let record = estate
            .get("marktd", &format!("/api/v1/malos/{malo}"))
            .await?
            .ok("GET malo")?;
        ensure!(
            record["sparte"] == "GAS",
            "marktd serves {malo} as Sparte {}",
            record["sparte"]
        );
        ensure!(
            record["rollenzuordnung"]
                .as_array()
                .is_some_and(|r| r
                    .iter()
                    .any(|z| z["zuordnungstyp"] == "NB"
                        && z["rollencodenummer"] == world::STADTWERK_GAS)),
            "{malo} names no NB {}: {}",
            world::STADTWERK_GAS,
            record["rollenzuordnung"]
        );
        let versorgung = estate
            .get("marktd", &format!("/api/v1/versorgung/{malo}"))
            .await?
            .ok("GET versorgung")?;
        ensure!(
            versorgung
                .to_string()
                .contains(&format!("\"lf_mp_id\":\"{}\"", world::STADTWERK_GAS)),
            "the supply record of {malo} names no Lieferant {}: {versorgung}",
            world::STADTWERK_GAS
        );
    }
    pass(&format!(
        "marktd: {} Gas Marktlokationen in the grid of NB {}, supplied by LF {}",
        world::GAS_MALOS,
        world::STADTWERK_GAS,
        world::STADTWERK_GAS
    ));

    let malo = world::gas_malo(0);
    let kunden = estate
        .tool("vertragd", "kunden_zu_malos", json!({ "malos": [malo] }))
        .await?;
    let (vorname, nachname) = world::GAS_KUNDE;
    let k = list(&kunden["kunden"])
        .into_iter()
        .find(|k| k["malo_id"] == malo.as_str())
        .with_context(|| format!("no customer on {malo}: {kunden}"))?;
    ensure!(
        k["name"] == format!("{vorname} {nachname}").as_str(),
        "the customer on {malo} is {}",
        k["name"]
    );
    pass(&format!(
        "vertragd: {vorname} {nachname}'s Gas contract on {malo}"
    ));
    Ok(())
}

/// Gasnetz Nachbarstadt's NN-Rechnung bills an Arbeitspreis of 1.42 ct/kWh
/// where its Preisblatt publishes 1.12 ct/kWh: invoicd records it disputed,
/// on the price deviation and nothing else, and lists it among the disputes.
async fn disput(estate: &Estate) -> anyhow::Result<()> {
    let receipt = nn_receipt(estate)
        .await?
        .with_context(|| format!("invoicd holds no receipt of {}", world::NN_RECHNUNG))?;
    ensure!(
        receipt["pid"] == 31002 && receipt["outcome"] == "Dispute",
        "the receipt of {} is PID {} with outcome {}, expected 31002 Dispute",
        world::NN_RECHNUNG,
        receipt["pid"],
        receipt["outcome"]
    );
    let disputes = estate
        .get("invoicd", "/api/v1/disputes")
        .await?
        .ok("GET disputes")?;
    ensure!(
        list(&disputes)
            .iter()
            .any(|d| d["rechnungsnummer"] == world::NN_RECHNUNG),
        "GET /api/v1/disputes does not list {}: {disputes}",
        world::NN_RECHNUNG
    );
    pass(&format!(
        "invoicd: NN-Rechnung {} from {} disputed and listed",
        world::NN_RECHNUNG,
        world::GASNETZ_NACHBARSTADT
    ));

    let offen = estate.tool("invoicd", "list_disputes", json!({})).await?;
    let disput = list(&offen["disputes"])
        .into_iter()
        .find(|d| d["rechnungsnummer"] == world::NN_RECHNUNG)
        .with_context(|| format!("list_disputes has no {}: {offen}", world::NN_RECHNUNG))?;
    let arten: Vec<&str> = disput["befunde"]
        .as_array()
        .map(|b| b.iter().filter_map(|f| f["art"].as_str()).collect())
        .unwrap_or_default();
    ensure!(
        arten == ["TariffDeviation"],
        "the dispute rests on {arten:?}, expected the price deviation alone: {}",
        disput["befunde"]
    );
    pass(&format!(
        "the dispute rests on the price: {} EUR/kWh billed, {} EUR/kWh in the Preisblatt",
        world::NN_ARBEITSPREIS_RECHNUNG,
        world::NN_ARBEITSPREIS_PREISBLATT
    ));
    Ok(())
}

/// Every Desk view's source answers with at least one row, through the very
/// tool the Desk calls.
async fn desk(estate: &Estate) -> anyhow::Result<()> {
    let sources: &[(&str, &str, Value)] = &[
        ("processd", "list_pending_approvals", json!({})),
        (
            "processd",
            "list_open_processes",
            json!({ "pids": [55001, 55077, 44001], "limit": 50 }),
        ),
        (
            "vertragd",
            "search_kunden",
            json!({ "query": "Mustermann" }),
        ),
        ("billingd", "list_review_queue", json!({ "limit": 50 })),
        ("makod", "list_dead_letters", json!({})),
        ("invoicd", "list_disputes", json!({})),
    ];
    for (service, tool, args) in sources {
        let answer = estate.tool(service, tool, args.clone()).await?;
        ensure!(
            has_rows(&answer),
            "{service} {tool} answers no rows: {answer}"
        );
        pass(&format!("Desk source {service} {tool}"));
    }
    Ok(())
}

/// Whether a tool's answer carries at least one row.
fn has_rows(v: &Value) -> bool {
    match v {
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => o.values().any(|x| match x {
            Value::Array(a) => !a.is_empty(),
            _ => false,
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_compare_by_value_not_spelling() {
        assert!(same_amount(&json!("233.56800"), "233.568"));
        assert!(same_amount(&json!("132.48"), "132.480"));
        assert!(same_amount(&json!(20), "20.00"));
        assert!(!same_amount(&json!("233.57"), "233.568"));
        assert!(!same_amount(&Value::Null, "0"));
    }

    #[test]
    fn rows_are_found_under_any_key() {
        assert!(has_rows(&json!([1])));
        assert!(has_rows(&json!({"eintraege": [1], "gesamt": 1})));
        assert!(!has_rows(&json!({"eintraege": [], "gesamt": 0})));
        assert!(!has_rows(&json!([])));
    }
}
