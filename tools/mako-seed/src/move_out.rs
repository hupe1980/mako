//! A household's confirmed Ersatzversorgung, move-out and final meter reading.
use anyhow::{Context as _, ensure};
use mako_service::{CloudEvent, oidc::demo};
use serde_json::{Value, json};
use time::{Date, format_description::well_known::Rfc3339};

use crate::{api::Estate, world};

pub struct Household {
    pub malo: String,
    from: Date,
    pub through: Date,
}

impl Household {
    pub fn previous_month(today: Date) -> anyhow::Result<Self> {
        let last = today
            .replace_day(1)?
            .previous_day()
            .context("previous month")?;
        // Each month has its own MaLo so an earlier departure keeps its supply and invoice.
        let base = format!("45{:04}{:02}01", last.year(), u8::from(last.month()));
        Ok(Self {
            malo: rubo4e::identifiers::MaloId::from_base(&base)?.to_string(),
            from: last.replace_day(1)?,
            through: last.replace_day(7)?,
        })
    }

    fn meter_number(&self) -> String {
        format!("DEMO-{}", self.malo)
    }

    pub async fn register_meter(&self, estate: &Estate, melo: &str) -> anyhow::Result<()> {
        let number = self.meter_number();
        estate
            .put(
                "marktd",
                &format!("/api/v1/zaehler/{number}"),
                &json!({
                    "melo_id": melo,
                    "data": {"zaehlernummer": number, "zaehlertyp": "DREHSTROMZAEHLER"}
                }),
            )
            .await?
            .ok("register final-reading meter")?;
        Ok(())
    }

    async fn invoices(&self, estate: &Estate) -> anyhow::Result<Vec<Value>> {
        let records = estate
            .get(
                "billingd",
                &format!(
                    "/api/v1/billing?malo_id={}&lf_mp_id={}&is_correction=false",
                    self.malo,
                    world::STADTWERK_LF
                ),
            )
            .await?
            .ok("read moved-out household invoices")?;
        Ok(records
            .as_array()
            .context("billingd invoices array")?
            .iter()
            .filter(|r| {
                r["outcome"] != "cancelled"
                    && r["period_from"] == self.from.to_string()
                    && r["period_to"] == self.through.to_string()
            })
            .cloned()
            .collect())
    }

    fn due_on(&self) -> Date {
        mako_fristen::vorlauf::abrechnung_faellig(
            self.through,
            mako_fristen::vorlauf::Abrechnungsart::Schlussrechnung,
        )
    }

    async fn watch(&self, estate: &Estate) -> anyhow::Result<Option<Value>> {
        let response: Value = estate
            .http
            .get(format!(
                "{}/api/v1/billing/unbilled?open=false&limit=500",
                estate.base("billingd")
            ))
            .bearer_auth(lf_token()?)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let rows = response["periods"]
            .as_array()
            .context("billing watch periods")?;
        Ok(rows
            .iter()
            .find(|r| {
                r["malo_id"] == self.malo
                    && r["lf_mp_id"] == world::STADTWERK_LF
                    && r["period_from"] == self.from.to_string()
                    && r["period_to"] == self.through.to_string()
            })
            .cloned())
    }

    async fn market_event(
        &self,
        estate: &Estate,
        kind: &str,
        pid: u32,
        data: Value,
    ) -> anyhow::Result<()> {
        let reference = format!("seed-move-out-{}-{pid}", self.malo);
        let event = CloudEvent::new(
            mako_service::source("mako-seed", "stadtwerke-musterstadt"),
            kind,
            &reference,
            data,
        )
        .with_id(&reference);
        mako_service::post_ce_with_retry(
            &estate.http,
            &format!("{}/api/v1/events", estate.base("vertragd")),
            &event,
            Some(b"dev-vertragd-webhook-secret"),
        )
        .await?;
        Ok(())
    }
}

pub async fn seed(estate: &Estate, household: &Household) -> anyhow::Result<()> {
    if !household.invoices(estate).await?.is_empty() {
        return Ok(());
    }
    household
        .market_event(
            estate,
            mako_events::mako::PROCESS_COMPLETED,
            55_013,
            json!({
                "pid": 55013,
                "malo_id": household.malo,
                "new_supplier": world::STADTWERK_LF,
                "grid_operator": world::STADTWERK,
                "process_date": household.from,
                "eog_art": "ERSATZVERSORGUNG",
                "haushaltskunde": true,
                "accepted": true,
                "kunde_name": {"namensformat": "Z01", "teile": ["Lenz", "Nora"]}
            }),
        )
        .await?;
    household
        .market_event(
            estate,
            mako_events::mako::PROCESS_INITIATED,
            55_037,
            json!({
                "pid": 55037,
                "malo_id": household.malo,
                "grid_operator": world::STADTWERK,
                "process_date": household.through,
                "transaktionsgrund": "ZC8"
            }),
        )
        .await?;

    // The assignment identifies the household by name. Its forwarding address
    // comes from the customer record and is needed on the final document.
    let contract = estate
        .get(
            "vertragd",
            &format!(
                "/api/v1/vertraege/by-malo/{}?lf_mp_id={}&from={}&to={}",
                household.malo,
                world::STADTWERK_LF,
                household.from,
                household.through
            ),
        )
        .await?
        .ok("read confirmed supply")?;
    let id = contract["vertrag"]["kunden_id"]
        .as_str()
        .context("supply customer")?;
    let customer_path = format!("/api/v1/kunden/{id}");
    let customer = estate
        .get("vertragd", &customer_path)
        .await?
        .ok("read household")?;
    let mut party = customer["kunde"]["geschaeftspartner"].clone();
    party["adresse"] = json!({"strasse":"Gartenweg","hausnummer":"8","postleitzahl":"12345","ort":"Musterstadt","landescode":"DE"});
    estate
        .put(
            "vertragd",
            &customer_path,
            &json!({"geschaeftspartner":party}),
        )
        .await?
        .ok("record forwarding address")?;

    if household.watch(estate).await?.is_none() {
        run_sweep(estate).await?;
    }
    let open = household
        .watch(estate)
        .await?
        .context("missing reading did not open a period")?;
    ensure!(
        open["settled_by"].is_null()
            && open["reason_code"] == "NO_METER_DATA"
            && open["due_on"] == household.due_on().to_string(),
        "unexpected refusal before the final reading: {open}"
    );

    let last_reading = household
        .through
        .next_day()
        .context("final meter reading")?;
    let mut readings = Vec::new();
    let mut day = household.from;
    while day <= last_reading {
        let days = (day - household.from).whole_days();
        readings.push(json!({
            "at": mako_fristen::berlin_midnight(day).format(&Rfc3339)?,
            "value": (1200 + 5 * days).to_string(),
            "quality": "MEASURED"
        }));
        day = day.next_day().context("next reading")?;
    }
    estate
        .post(
            "edmd",
            &format!("/api/v1/zaehlerstandsgang/{}", household.malo),
            &json!({
                "session_id": format!("seed-final-reading-{}", household.malo),
                "sparte": "STROM",
                "melo_id": world::melo_for(&household.malo),
                "obis_code": "1-0:1.8.0",
                "sender_mp_id": world::MSB_MESSWERTE,
                "readings": readings
            }),
        )
        .await?
        .ok("submit final register readings")?;

    run_sweep(estate).await
}

fn lf_token() -> anyhow::Result<String> {
    Ok(demo::mint(demo::persona("lf").context("LF demo persona")?))
}

async fn run_sweep(estate: &Estate) -> anyhow::Result<()> {
    let key = uuid::Uuid::new_v4();
    let response = estate
        .http
        .post(format!(
            "{}/api/v1/billing/runs/sweep",
            estate.base("billingd")
        ))
        .bearer_auth(lf_token()?)
        .json(&json!({}))
        .header("Idempotency-Key", key.to_string())
        .send()
        .await?;
    let status = response.status();
    let outcome: Value = response.json().await?;
    ensure!(
        status.is_success() && outcome["run_id"] == key.to_string(),
        "billing sweep {key} answered {status}: {outcome}"
    );
    Ok(())
}

pub async fn verify(estate: &Estate) -> anyhow::Result<()> {
    let household = Household::previous_month(mako_fristen::heute())?;
    let rows = household.invoices(estate).await?;
    ensure!(
        rows.len() == 1,
        "{} has {} invoices for {} through {}, expected one",
        household.malo,
        rows.len(),
        household.from,
        household.through
    );
    let row = &rows[0];
    let invoice = &row["rechnung_json"];
    ensure!(
        row["outcome"] == "dispatched" && invoice["rechnungstyp"] == "ABSCHLUSSRECHNUNG",
        "expected issued Schlussrechnung, got {} and {}",
        row["outcome"],
        invoice["rechnungstyp"]
    );
    ensure!(
        invoice["zaehler"][0]["zaehlernummer"] == household.meter_number(),
        "the final reading's meter is missing from the invoice"
    );
    let billed_kwh = invoice["rechnungspositionen"]
        .as_array()
        .context("final invoice positions")?
        .iter()
        .map(|p| &p["positionsMenge"])
        .any(|q| q["einheit"] == "KWH" && crate::verify::same_amount(&q["wert"], "35"));
    ensure!(
        billed_kwh,
        "the final invoice does not bill the measured 35 kWh"
    );
    ensure!(
        invoice["rechnungsempfaenger"]["organisationsname"] == "Nora Lenz",
        "the final invoice is missing its supply's recipient"
    );
    let due = household.due_on();
    let closed = household
        .watch(estate)
        .await?
        .context("the settled refusal is missing")?;
    ensure!(
        closed["reason_code"] == "NO_METER_DATA"
            && closed["settled_by"] == row["id"]
            && closed["due_on"] == due.to_string(),
        "the invoice does not settle the refused reading: {closed}"
    );
    ensure!(
        row["due_on"] == due.to_string(),
        "expected due date {due}, got {}",
        row["due_on"]
    );
    let run = invoice["zusatzAttribute"]
        .as_array()
        .and_then(|values| values.iter().find(|v| v["name"] == "mako:billing_run_id"))
        .and_then(|v| v["wert"].as_str())
        .context("Schlussrechnung lacks a billing run")?;
    uuid::Uuid::parse_str(run).context("invalid billing run in the Schlussrechnung")?;
    println!(
        "  ✓ {}: one dispatched Schlussrechnung for {} through {}, due {due}, run {run}",
        household.malo, household.from, household.through
    );
    Ok(())
}
