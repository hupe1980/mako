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
}
