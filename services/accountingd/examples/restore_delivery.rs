//! Signed local delivery and ledger witness for the isolated LF restore drill.
use std::{
    fs::{File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use accountingd::{Accountingd, config::AccountingdConfig, ledger::PgLedger, pg};
use anyhow::{Context as _, Result, ensure};
use axum::{
    Extension, Router,
    body::Bytes,
    http::{HeaderMap, StatusCode},
    response::IntoResponse as _,
    routing::post,
};
use mako_service::{
    CloudEvent, Daemon as _,
    outbox::{self, OutboxConfig},
    subscriptions::{self, Subscriptions},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::Notify;
use uuid::Uuid;

const TENANT: &str = "stadtwerke-musterstadt";
const LF: &str = "9900357000028";
const MALO: &str = "51238696012";

async fn seed(billing: &PgPool, accounts: &PgPool, ledger: &PgLedger) -> Result<()> {
    let today = mako_fristen::heute();
    pg::upsert_account(accounts, MALO, LF, TENANT).await?;
    sqlx::query(
        "UPDATE accounts SET abschlag_ct=16117 WHERE tenant=$1 AND malo_id=$2 AND mp_id=$3",
    )
    .bind(TENANT)
    .bind(MALO)
    .bind(LF)
    .execute(accounts)
    .await?;
    let account = pg::fetch_account(accounts, MALO, LF, TENANT)
        .await?
        .context("issuer account")?;
    let period = today.replace_day(1)?;
    pg::raise_abschlagsforderung(ledger, accounts, TENANT, &account, period, period, today).await?;
    pg::post_entry(
        ledger,
        accounts,
        TENANT,
        MALO,
        LF,
        "ZAHLUNG",
        -16117,
        "restore-reference-payment",
        None,
        None,
        today,
        today,
        None,
        None,
    )
    .await?;

    let run = Uuid::new_v4();
    let record = Uuid::new_v4();
    let number = format!("RE-{}-000001", today.year());
    let eur = |wert| json!({"wert":wert,"waehrung":"EUR"});
    let rechnung = json!({
        "_typ":"RECHNUNG", "rechnungstyp":"ABSCHLUSSRECHNUNG", "sparte":"STROM",
        "rechnungsnummer":number,"rechnungsdatum":format!("{today}T00:00:00Z"),
        "gesamtnetto":eur("100.00"), "gesamtsteuer":eur("19.00"), "gesamtbrutto":eur("119.00"),
        "vorauszahlungen":[{"betrag":eur("161.17"),"datum":format!("{today}T00:00:00Z")}], "zuZahlen":eur("-42.17"),
        "zusatzAttribute":[{"name":"mako:billing_run_id","wert":run}]
    });
    let rechnung = serde_json::to_value(serde_json::from_value::<rubo4e::current::Rechnung>(
        rechnung,
    )?)?;
    let ce = CloudEvent::new(
        mako_service::source("billingd", TENANT),
        mako_events::billing::RECHNUNG_ERSTELLT,
        MALO,
        json!({"record_id":record,"malo_id":MALO,"lf_mp_id":LF,"is_correction":false,"schlussrechnung":true,"rechnung":rechnung}),
    );
    let due = mako_fristen::vorlauf::abrechnung_faellig(
        today,
        mako_fristen::vorlauf::Abrechnungsart::Schlussrechnung,
    );
    let mut tx = billing.begin().await?;
    sqlx::query(
        "INSERT INTO invoice_number_series(tenant,series,year,last_value) VALUES ($1,'RE',$2,1)",
    )
    .bind(TENANT)
    .bind(i16::try_from(today.year())?)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO billing_discovery
            (tenant, run_id, business_date, origin, ended_from, ended_through, status, candidate_count, finished_at)
         VALUES ($1, $2, $3, 'manual', $3, $3, 'completed', 1, clock_timestamp())",
    )
    .bind(TENANT)
    .bind(run)
    .bind(today)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO billing_discovery_issuers (tenant, run_id, lf_mp_id, candidates, billed, skipped, errors)
         VALUES ($1, $2, $3, 1, 1, 0, 0)",
    )
    .bind(TENANT)
    .bind(run)
    .bind(LF)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO billing_records
            (id, tenant, malo_id, lf_mp_id, product_code, category, rechnungsnummer, period_from, period_to,
             rechnung_json, total_netto_eur, total_brutto_eur, outcome, dispatched_at, due_on)
         VALUES ($1, $2, $3, $4, 'STROM-REFERENZ', 'STROM', $5, $6, $7, $8, 100, 119, 'dispatched', clock_timestamp(), $9)",
    )
    .bind(record)
    .bind(TENANT)
    .bind(MALO)
    .bind(LF)
    .bind(number)
    .bind(period)
    .bind(today)
    .bind(rechnung)
    .bind(due)
    .execute(&mut *tx)
    .await?;
    outbox::enqueue(&mut tx, &ce).await?;
    tx.commit().await?;
    Ok(())
}

fn receipt(endpoint_id: Uuid, event_id: &str, body: &[u8]) -> Value {
    json!({"endpoint_id":endpoint_id,"event_id":event_id,"body_blake3":blake3::hash(body).to_hex().as_str()})
}

fn persist_receipt(path: &Path, value: &Value) -> Result<()> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&serde_json::to_vec(value)?)?;
    file.sync_all()?;
    File::open(path.parent().context("receipt directory")?)?.sync_all()?;
    Ok(())
}

// The coordinator holds this exact loopback address across both database lifetimes.
#[cfg(unix)]
fn recipient_listener() -> Result<tokio::net::TcpListener> {
    use std::os::fd::AsFd as _;
    let fd = std::io::stdin().as_fd().try_clone_to_owned()?;
    let listener = std::net::TcpListener::from(fd);
    ensure!(
        listener.local_addr()?.ip() == std::net::Ipv4Addr::LOCALHOST,
        "expected loopback recipient"
    );
    listener.set_nonblocking(true)?;
    Ok(tokio::net::TcpListener::from_std(listener)?)
}

#[cfg(not(unix))]
fn recipient_listener() -> Result<tokio::net::TcpListener> {
    anyhow::bail!("the isolated drill needs a POSIX socket descriptor; run it in WSL on Windows")
}

async fn exercise(first: bool, port: u16, path: PathBuf) -> Result<()> {
    let password = std::env::var("MAKO_DRILL_PASSWORD")?;
    let signing_key = std::env::var("MAKO_DRILL_SIGNING_KEY")?;
    // Only the loopback bridge opened for these two scratch databases can be addressed.
    let url =
        |name| format!("postgres://postgres:{password}@127.0.0.1:{port}/{name}?sslmode=disable");
    let mut cfg: AccountingdConfig =
        toml::from_str(include_str!("../../../deploy/demo/accountingd.toml"))?;
    cfg.database.url = url("accountingd");
    cfg.database.migration_url = None;
    cfg.erp_hmac_secret = Some(signing_key.clone().into());
    let billing = cfg
        .database
        .connect(&url("billingd"), "restore-billing")
        .await?;
    let accounts = cfg
        .database
        .connect(&cfg.database.url, "restore-accounting")
        .await?;
    if first {
        Accountingd::migrate(&cfg, &accounts).await?;
        sqlx::migrate!("../billingd/migrations")
            .run(&billing)
            .await?;
        outbox::ensure_schema(&billing).await?;
        subscriptions::ensure_schema(&billing).await?;
    }
    let ledger =
        Arc::new(PgLedger::connect(&cfg.database.url, TENANT, mako_fristen::heute()).await?);
    let listener = recipient_listener()?;
    let endpoint = format!("http://{}/webhook", listener.local_addr()?);
    let worker = Subscriptions::new(billing.clone(), "billingd", TENANT, Default::default())
        .with_default_endpoint(Some(endpoint.clone()), Some(signing_key.clone().into()))
        .with_policy(OutboxConfig {
            lease: Duration::from_secs(1),
            ..Default::default()
        });
    if first {
        worker.sync_config().await?;
        seed(&billing, &accounts, &ledger).await?;
        ensure!(
            worker.fan_out_once().await? == 1,
            "the cut needs one fanned invoice event"
        );
    }
    // Check every persisted endpoint and delivery before giving the sender any network access.
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM webhook_endpoints), (SELECT count(*) FROM webhook_deliveries),
                (SELECT count(*) FROM event_outbox)"
    ).fetch_one(&billing).await?;
    ensure!(
        counts == (1, 1, 1),
        "the drill must have a single pending invoice and recipient"
    );
    let (endpoint_id, envelope): (Uuid, Value) = sqlx::query_as(
        "SELECT w.id, e.envelope FROM webhook_deliveries d
          JOIN webhook_endpoints w ON w.id=d.endpoint_id AND w.tenant=d.tenant
          JOIN event_outbox e ON e.event_id=d.event_id AND e.ce_type=d.ce_type
         WHERE d.delivered_at IS NULL AND d.dead_lettered_at IS NULL AND d.attempts=$1
           AND e.fanned_out_at IS NOT NULL AND w.active AND w.name='default' AND w.owner='config'
           AND w.tenant=$2 AND w.url=$3 AND w.secret=$4 AND w.previous_secret IS NULL
           AND e.event_id=e.envelope->>'id' AND e.ce_type=$5 AND e.envelope->>'source'=$6
           AND e.envelope#>>'{data,lf_mp_id}'=$7",
    )
    .bind(if first { 0i16 } else { 1i16 })
    .bind(TENANT)
    .bind(&endpoint)
    .bind(signing_key)
    .bind(mako_events::billing::RECHNUNG_ERSTELLT)
    .bind(mako_service::source("billingd", TENANT))
    .bind(LF)
    .fetch_one(&billing)
    .await
    .context("pending invoice and original loopback subscription")?;
    let event_id = envelope["id"].as_str().context("event ID")?;
    let expected = receipt(endpoint_id, event_id, &serde_json::to_vec(&envelope)?);
    if !first {
        ensure!(
            serde_json::from_slice::<Value>(&std::fs::read(&path)?)? == expected,
            "the independent recipient receipt differs from the pending send"
        );
    }
    let confirmed = Arc::new(Notify::new());
    let requests = Arc::new(AtomicUsize::new(0));
    let route = post({
        let cfg = Arc::new(cfg);
        let accounts = accounts.clone();
        let requests = requests.clone();
        let confirmed = confirmed.clone();
        let ledger = ledger.clone();
        move |headers: HeaderMap, body: Bytes| {
            let (cfg, accounts, ledger) = (cfg.clone(), accounts.clone(), ledger.clone());
            let (expected, path, requests, confirmed) = (
                expected.clone(),
                path.clone(),
                requests.clone(),
                confirmed.clone(),
            );
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                let Some(id) = headers.get("webhook-id").and_then(|v| v.to_str().ok()) else {
                    return StatusCode::BAD_REQUEST;
                };
                if receipt(endpoint_id, id, &body) != expected {
                    return StatusCode::CONFLICT;
                }
                let response = accountingd::handlers::ingest_webhook(
                    Extension(accounts),
                    Extension(ledger),
                    Extension(cfg),
                    headers,
                    body,
                )
                .await
                .into_response();
                if first && response.status().is_success() {
                    if persist_receipt(&path, &expected).is_err() {
                        return StatusCode::INTERNAL_SERVER_ERROR;
                    }
                    confirmed.notify_one();
                    // Hold the acknowledgement until the source sender disappears.
                    std::future::pending::<()>().await;
                }
                response.status()
            }
        }
    });
    let app = Router::new().route("/webhook", route);
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    if first {
        tokio::select! {
            () = confirmed.notified() => {}
            result = worker.deliver_once() => {
                let status: Option<i16> = sqlx::query_scalar("SELECT last_status FROM webhook_deliveries")
                    .fetch_one(&billing).await?;
                anyhow::bail!("source delivery ended before the committed receipt (claimed {}, HTTP {status:?})", result?);
            }
        }
    } else {
        ensure!(
            worker.deliver_once().await? == 1,
            "the restored sender must claim the pending event"
        );
        ensure!(
            worker.deliver_once().await? == 0,
            "a repeated drain must be empty"
        );
    }
    server.abort();
    let attempts: i16 =
        sqlx::query_scalar("SELECT attempts FROM webhook_deliveries WHERE event_id=$1")
            .bind(event_id)
            .fetch_one(&billing)
            .await?;
    ensure!(
        attempts == if first { 1 } else { 2 },
        "unexpected send attempts"
    );
    ensure!(
        requests.load(Ordering::SeqCst) == 1,
        "expected a single wire attempt"
    );
    ensure!(
        ledger.balance_ct(LF, MALO).await? == -4217,
        "settled customer balance differs from the credit"
    );
    println!(
        "{}",
        json!({"wire_attempts":1,"attempts":attempts,"event_id":event_id,"credit_cents":4217})
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args == ["--help"] {
        println!(
            "The isolated sender and recipient for the two-database restore drill. Run just recovery-drill."
        );
        return Ok(());
    }
    ensure!(
        args.len() == 3 && matches!(args[0].as_str(), "seed" | "resume"),
        "usage: restore_delivery seed|resume PORT RECEIPT_PATH"
    );
    tokio::time::timeout(
        Duration::from_secs(45),
        exercise(args[0] == "seed", args[1].parse()?, PathBuf::from(&args[2])),
    )
    .await??;
    Ok(())
}
