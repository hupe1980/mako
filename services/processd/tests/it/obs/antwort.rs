//! The counterparty's Antwortcode reaches the read model with the list it
//! named, against real PostgreSQL.
//!
//! makod states the code of an answered Lieferbeginn on the
//! `de.mako.process.completed` it emits (`data.antwortcode`,
//! `data.codeliste`). Without it a refused Anmeldung is a `completed` row and
//! nothing else: the reason the Netzbetreiber gave is on the wire and in
//! makod's Verlauf, and on no read of the process.

use std::collections::HashSet;
use std::sync::Arc;

use processd::obs::project::{ObsState, project};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::datetime;
use uuid::Uuid;

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

async fn pg_pool() -> Option<(sqlx::PgPool, PgContainer)> {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;

    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply processd schema");
    Some((pool, container))
}

const TENANT: &str = "9900357000004";

fn state(pool: &sqlx::PgPool) -> ObsState {
    ObsState {
        repo: processd::obs::pg::PgProcessProjectionRepository::new(pool.clone()),
        tenant: TENANT.to_owned(),
        verbundene_lieferanten: Arc::new(HashSet::new()),
    }
}

fn event(
    ce_type: &str,
    process: Uuid,
    pid: u32,
    at: OffsetDateTime,
    data: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "specversion": "1.0",
        "id": Uuid::new_v4().to_string(),
        "source": "makod",
        "type": ce_type,
        "subject": process.to_string(),
        "time": at.format(&Rfc3339).expect("rfc3339"),
        "makopid": pid,
        "makoworkflow": "gpke-lf-anmeldung",
        "data": data,
    })
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_refused_anmeldung_keeps_the_code_the_netzbetreiber_sent() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let obs = state(&pool);
    let process = Uuid::new_v4();
    let t0 = datetime!(2026-11-02 09:00 UTC);

    project(
        &obs,
        &event(
            mako_events::mako::PROCESS_INITIATED,
            process,
            55_001,
            t0,
            serde_json::json!({ "malo_id": "51238696012" }),
        ),
    )
    .await
    .expect("initiated");
    let open = obs.repo.get(process, TENANT).await.unwrap().expect("row");
    assert_eq!(open.antwort_code, None, "no answer yet");

    project(
        &obs,
        &event(
            mako_events::mako::PROCESS_COMPLETED,
            process,
            55_003,
            t0 + time::Duration::hours(1),
            serde_json::json!({
                "pid": 55_003,
                "malo_id": "51238696012",
                "accepted": false,
                "outcome": "rejected",
                "antwortcode": "A02",
                "codeliste": "E_0622",
            }),
        ),
    )
    .await
    .expect("completed");

    let answered = obs.repo.get(process, TENANT).await.unwrap().expect("row");
    assert_eq!(
        answered.state,
        processd::obs::domain::ProcessState::Rejected,
        "an Ablehnung completes the process without doing what it set out to do"
    );
    assert_eq!(answered.antwort_code.as_deref(), Some("A02"));
    assert_eq!(answered.antwort_codeliste.as_deref(), Some("E_0622"));
    assert_eq!(
        answered.erc_code, None,
        "an answer is not an APERAK refusal"
    );
    assert_eq!(
        answered.bedeutung(),
        mako_pruefung::codes::lookup("E_0622", "A02").map(|c| c.bedeutung)
    );
    assert!(answered.bedeutung().is_some());

    // A redelivered APERAK after the answer carries no code: the answer stays.
    project(
        &obs,
        &event(
            mako_events::mako::APERAK_ACCEPTED,
            process,
            55_001,
            t0 + time::Duration::hours(2),
            serde_json::json!({}),
        ),
    )
    .await
    .expect("aperak");
    let kept = obs.repo.get(process, TENANT).await.unwrap().expect("row");
    assert_eq!(kept.antwort_code.as_deref(), Some("A02"));
    assert_eq!(kept.antwort_codeliste.as_deref(), Some("E_0622"));
}

/// A code without a list replaces the earlier pair whole: the old list never
/// stands beside the new code and words it from the wrong tree.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_later_code_replaces_the_pair_and_never_borrows_the_earlier_list() {
    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let obs = state(&pool);
    let process = Uuid::new_v4();
    let t0 = datetime!(2026-11-02 09:00 UTC);
    for (n, data) in [
        serde_json::json!({ "antwortcode": "A02", "codeliste": "E_0622" }),
        serde_json::json!({ "antwortcode": "A07" }),
    ]
    .into_iter()
    .enumerate()
    {
        project(
            &obs,
            &event(
                mako_events::mako::PROCESS_COMPLETED,
                process,
                55_003,
                t0 + time::Duration::minutes(i64::try_from(n).unwrap()),
                data,
            ),
        )
        .await
        .expect("completed");
    }
    let row = obs.repo.get(process, TENANT).await.unwrap().expect("row");
    assert_eq!(row.antwort_code.as_deref(), Some("A07"));
    assert_eq!(row.antwort_codeliste, None);
    assert_eq!(row.bedeutung(), None, "no list, no wording");
}

/// The payload makod's `gpke-lf-anmeldung` builds for a 55003 is the payload
/// this read model takes the code from: built by the real workflow, not by
/// hand, so a key renamed on one side fails here.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_code_the_workflow_states_is_the_code_the_read_model_keeps() {
    use mako_engine::envelope::Antwortcode;
    use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
    use mako_engine::workflow::Workflow as _;
    use mako_gpke::lf_anmeldung::{
        GpkeLfAnmeldungWorkflow, LfAnmeldungCommand, LfAnmeldungEvent, LfAnmeldungState,
    };

    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let pending = GpkeLfAnmeldungWorkflow::apply(
        LfAnmeldungState::New,
        &LfAnmeldungEvent::Initiated {
            pruefidentifikator: Pruefidentifikator::new(55_001).unwrap(),
            location_id: MaLo::new("51238696012"),
            sender: MarktpartnerCode::new("9900000000001"),
            receiver: MarktpartnerCode::new("9900000000002"),
            process_date: "2026-12-01".to_owned(),
        },
    );
    let out = GpkeLfAnmeldungWorkflow::handle(
        &pending,
        LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55_003).unwrap(),
            accepted: false,
            reason: None,
            response_ref: MessageRef::new("NB-RESP-001"),
            antwort: Some(Antwortcode::new("A02", Some("E_0622"))),
        },
    )
    .expect("the 55003 is handled");
    let completed = out
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "ProcessCompleted")
        .expect("a ProcessCompleted for the ERP outbox");

    let obs = state(&pool);
    let process = Uuid::new_v4();
    project(
        &obs,
        &event(
            mako_events::mako::PROCESS_COMPLETED,
            process,
            55_003,
            datetime!(2026-11-02 10:00 UTC),
            completed.payload.clone(),
        ),
    )
    .await
    .expect("completed");
    let row = obs.repo.get(process, TENANT).await.unwrap().expect("row");
    assert_eq!(row.antwort_code.as_deref(), Some("A02"));
    assert_eq!(row.antwort_codeliste.as_deref(), Some("E_0622"));
    assert!(row.bedeutung().is_some());
    assert_eq!(
        row.state,
        processd::obs::domain::ProcessState::Rejected,
        "the verdict the workflow states is the state the read model keeps"
    );
}

/// Project the `ProcessCompleted` a workflow queued, as makod's ERP worker
/// delivers it, and read the row back.
async fn project_completion(
    pool: &sqlx::PgPool,
    workflow: &str,
    pid: u32,
    payload: serde_json::Value,
) -> processd::obs::domain::ProcessProjection {
    let obs = state(pool);
    let process = Uuid::new_v4();
    let mut ce = event(
        mako_events::mako::PROCESS_COMPLETED,
        process,
        pid,
        datetime!(2026-11-02 10:00 UTC),
        payload,
    );
    ce["makoworkflow"] = serde_json::json!(workflow);
    project(&obs, &ce).await.expect("completed");
    obs.repo.get(process, TENANT).await.unwrap().expect("row")
}

/// The MSBA's ORDRSP 19016 refusing a Gerätewechselabsicht with `E17` ends
/// the process, and its code reaches the read model with the Codeliste the
/// ORDRSP names — worded by the BDEW, not by a code alone.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_wim_19016_e17_reaches_the_read_model_with_its_wording() {
    use mako_engine::types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator};
    use mako_engine::workflow::Workflow as _;
    use mako_wim::geraeteubernahme::{
        GeraeteubernahmeCommand, GeraeteubernahmeState, WimGeraeteubernahmeWorkflow,
    };

    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    // Anzeige on Mon 2026-03-02 for Wed 2026-03-04 — inside the
    // Mindestvorlaufzeit, so the 19016 `E17` goes out at once.
    let out = WimGeraeteubernahmeWorkflow::handle(
        &GeraeteubernahmeState::default(),
        GeraeteubernahmeCommand::ReceiveOrders {
            pid: Pruefidentifikator::new(17_009).unwrap(),
            sender: MarktpartnerCode::new("9900000000001"),
            receiver: MarktpartnerCode::new(TENANT),
            melo_id: MeLo::new("DE0000000001234567890000000000001"),
            device_id: DeviceId::new("1ESY1161234567"),
            document_date: "20260302".to_owned(),
            termin: Some("20260304".to_owned()),
            message_ref: MessageRef::new("ORD-1"),
            orders_nummer: "ORD-DOC-1".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
            sparte: mako_engine::types::Sparte::Strom,
            received_at: datetime!(2026-03-02 09:00 UTC),
        },
    )
    .expect("the 17009 is answered");
    let completed = out
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "ProcessCompleted")
        .expect("the refusal queues its completion");
    let row = project_completion(
        &pool,
        "wim-geraeteubernahme",
        19_016,
        completed.payload.clone(),
    )
    .await;
    assert_eq!(row.antwort_code.as_deref(), Some("E17"));
    assert_eq!(row.antwort_codeliste.as_deref(), Some("S_0066"));
    assert_eq!(
        row.bedeutung(),
        mako_pruefung::codes::lies("S_0066", "E17").map(|c| c.bedeutung)
    );
    assert!(row.bedeutung().is_some());
    assert_eq!(row.state, processd::obs::domain::ProcessState::Rejected);
}

/// The NB's own Ablehnung 55003 states its code too: the reason it gave
/// reaches its own read model, not only the LF's.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_nbs_55003_reaches_the_read_model_with_its_wording() {
    use mako_engine::types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator};
    use mako_engine::workflow::Workflow as _;
    use mako_gpke::{
        GpkeSupplierChangeWorkflow as W, LfAntwort, SupplierChangeCommand, SupplierChangeState,
    };

    let Some((pool, _pg)) = pg_pool().await else {
        return;
    };
    let received = W::handle(
        &SupplierChangeState::default(),
        SupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55_001).unwrap(),
            sender: MarktpartnerCode::new("9911111111111"),
            receiver: MarktpartnerCode::new(TENANT),
            location_id: MaLo::new("51238696012"),
            document_date: "20261020".to_owned(),
            process_date: "20261201".to_owned(),
            bilanzierungsgebiet: None,
            bilanzierungsmethode: None,
            fallgruppe: None,
            transaktionsgrund: Some("E01".to_owned()),
            transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
            veraeusserungsform: None,
            tranchengroesse_prozent: None,
            vorgangsnummer: None,
            produktpaket_id: Some("1".to_owned()),
            kunde_name: None,
            kunde_namensformat: None,
            message_ref: MessageRef::new("ANM-1"),
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("the 55001 is received");
    let pending = received
        .events
        .iter()
        .fold(SupplierChangeState::default(), W::apply);
    let answered = W::handle(
        &pending,
        SupplierChangeCommand::SendAntwort {
            antwort: LfAntwort::ablehnung("A02", "E_0622"),
            obligations: vec![],
            lfa_lieferende: None,
        },
    )
    .expect("the 55003 is queued");
    let completed = answered
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "ProcessCompleted")
        .expect("the 55003 queues its completion");
    let row = project_completion(
        &pool,
        "gpke-supplier-change",
        55_003,
        completed.payload.clone(),
    )
    .await;
    assert_eq!(row.antwort_code.as_deref(), Some("A02"));
    assert_eq!(row.antwort_codeliste.as_deref(), Some("E_0622"));
    assert!(row.bedeutung().is_some());
    assert_eq!(row.state, processd::obs::domain::ProcessState::Rejected);
}
