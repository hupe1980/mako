//! SQL-level tests for the template store, against a real PostgreSQL.
//!
//! The properties these guard live in the SQL, not in Rust: a primary key on
//! the content hash, a foreign key from the current-pointer, and the
//! proof-matches-kind constraint. Only a real database proves them.
//!
//! PostgreSQL is self-managed via testcontainers (a Docker daemon is the only
//! requirement); the tests skip gracefully when Docker is unavailable:
//!
//! ```bash
//! just test-outputd-db
//! ```
//!
//! Every test provisions its own schema, so they leave nothing behind.

use outputd::document::gate::Proof;
use outputd::template_store::{self, TemplateKind};
use sqlx::PgPool;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// The Postgres container guard a test holds until it ends — dropping it removes
/// the container (testcontainers cleans up on `Drop`; no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a fresh throwaway `postgres:17-alpine` and return its URL plus the
/// container guard. `None` when Docker is unavailable (tests skip gracefully).
async fn pg_container() -> Option<(String, PgContainer)> {
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
    Some((url, container))
}

/// Connect and provision a fresh schema, or skip when Docker is unavailable.
async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

/// Publishing is content-addressed and idempotent; the pointer moves, the
/// templates it points at do not.
///
/// The property that matters is the last one: a template an issued document was
/// rendered with must stay resolvable, because § 147 AO / GoBD keep that
/// document for 8 years and its appearance has to remain explicable. The pin
/// itself lives in the issuing service's database (billingd's
/// `billing_records.template_hash`), so no foreign key reaches it — this
/// store's append-only policy is the whole guarantee.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_published_template_stays_resolvable_after_the_pointer_moves() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let tenant = "9900000000001";

    let v1 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#set page(paper: \"a4\")\n= Rechnung v1",
        Some("a-3b"),
        Proof::RenderedPdfa,
        Some("ops@example"),
    )
    .await
    .expect("publish v1");

    // Same source → same identity, and no duplicate row.
    let again = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#set page(paper: \"a4\")\n= Rechnung v1",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("re-publish is a no-op");
    assert_eq!(
        again, v1,
        "content-addressed: identical source, identical hash"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM document_templates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "re-publishing must not duplicate");

    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v1)
        .await
        .expect("roll out v1");
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.hash),
        Some(v1.clone()),
    );

    // Publish a redesign and roll it out.
    let v2 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#set page(paper: \"a4\")\n= Rechnung v2 (neues Logo)",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("publish v2");
    assert_ne!(v2, v1);
    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v2)
        .await
        .expect("roll out v2");

    // The point of the whole design: v1 is still there, unchanged, and an
    // invoice rendered with it can still explain how it looked.
    let old = template_store::by_hash(&pool, tenant, &v1)
        .await
        .unwrap()
        .expect("v1 survives the rollout");
    assert!(old.source.contains("v1"), "the old source is intact");
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.hash),
        Some(v2),
        "only the pointer moved",
    );

    // A pointer into nothing is refused — that is why it is a table with a
    // foreign key rather than a free-text column.
    assert!(
        template_store::set_current(&pool, tenant, TemplateKind::Invoice, "deadbeef")
            .await
            .is_err(),
        "cannot point at an unpublished template",
    );

    // Textform kinds share the store, and their pointers are independent.
    let mahnung = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Mahnung,
        "= Zahlungserinnerung",
        None,
        Proof::RenderedTextform,
        None,
    )
    .await
    .expect("publish Mahnung");
    template_store::set_current(&pool, tenant, TemplateKind::Mahnung, &mahnung)
        .await
        .unwrap();
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.kind),
        Some("INVOICE".to_owned()),
        "rolling out a Mahnung must not disturb the invoice pointer",
    );
}

/// The schema refuses an invoice template that was not fully proven.
///
/// The gate is a code path and code paths can be bypassed — by a future caller,
/// by a migration script, by a hand-written `INSERT`. The constraint is what
/// makes "an INVOICE row is always a rendered, conformant carrier" a property of
/// the data rather than of the current call graph.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_database_refuses_an_unproven_invoice_template() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let tenant = "9910000000002";

    assert!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Invoice,
            "#let render(i) = []",
            Some("a-3b"),
            Proof::RenderedTextform,
            None,
        )
        .await
        .is_err(),
        "an invoice template may not be stored on a Textform proof — it has a carrier to meet",
    );

    assert!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Invoice,
            "#let render(i) = []",
            None,
            Proof::RenderedPdfa,
            None,
        )
        .await
        .is_err(),
        "an invoice template must record the PDF/A level it met",
    );

    // A Textform kind may not be stored on the carrier proof either: it has no
    // PDF/A to meet, so claiming one is a claim about a document that does not
    // exist.
    assert!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Mahnung,
            "#let render(i) = [Zahlungserinnerung]",
            Some("a-3b"),
            Proof::RenderedPdfa,
            None,
        )
        .await
        .is_err(),
        "a MAHNUNG row may not claim a PDF/A carrier proof",
    );

    // Both Textform kinds store on the same proof, and only on it: a level
    // that established merely that the template compiled would let a
    // PREISANPASSUNG layout printing none of what § 41 Abs. 5 EnWG requires be
    // rolled out.
    for kind in [TemplateKind::Mahnung, TemplateKind::Preisanpassung] {
        let hash = template_store::publish(
            &pool,
            tenant,
            kind,
            &format!("#let render(i) = [{}]", kind.as_str()),
            None,
            Proof::RenderedTextform,
            None,
        )
        .await
        .expect("a Textform template stores on the Textform proof");
        assert_eq!(
            template_store::by_hash(&pool, tenant, &hash)
                .await
                .unwrap()
                .map(|t| t.proof),
            Some("RENDERED_TEXTFORM".to_owned()),
            "the store records which proof was obtained",
        );
    }
}

/// The identical source cannot be published under a second identity.
///
/// Within a tenant the hash is the identity of the *source*, and the row keeps
/// the kind and proof it was admitted on. Accepting the second publish would
/// return a hash whose row carries the first kind: rollout then succeeds (the FK
/// checks only existence) and every render answers 422 with nothing naming the
/// cause — two green API calls, then a bricked tenant.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_same_source_cannot_become_a_second_identity() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let tenant = "9900000000001";
    let source = "#let render(i) = [Gemeinsame Vorlage]";

    let hash = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        source,
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("first publish");

    // Same source, same kind: the documented idempotent no-op.
    assert_eq!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Invoice,
            source,
            Some("a-3b"),
            Proof::RenderedPdfa,
            None,
        )
        .await
        .expect("same-kind re-publish stays idempotent"),
        hash,
    );

    // Same source, different kind: refused with the cause, not swallowed.
    let err = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Mahnung,
        source,
        None,
        Proof::RenderedTextform,
        None,
    )
    .await
    .expect_err("a second identity for the same source must be refused");
    assert!(
        matches!(
            &err,
            template_store::StoreError::IdentityCollision { existing_kind }
                if existing_kind == "INVOICE"
        ),
        "{err}"
    );

    // And a rollout cannot point a kind at a row of another kind — the
    // failure surfaces at the PUT, not at the first render when documents
    // are due.
    let err = template_store::set_current(&pool, tenant, TemplateKind::Mahnung, &hash)
        .await
        .expect_err("rolling out an INVOICE row as the MAHNUNG template must be refused");
    assert!(
        matches!(&err, template_store::StoreError::NotPublished(h, k) if h == &hash && *k == "MAHNUNG"),
        "{err}"
    );
}

/// An operator can find the hash to roll back to.
///
/// The store never deletes so a previous layout stays restorable, and the API
/// documents rollback as "PUT the previous hash" — which is not a performable
/// instruction unless something says what the previous hash was. `current`
/// names exactly one template and `by_hash` needs the answer already, so
/// without a listing the documented recovery path could not be walked.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rollback_can_discover_the_hash_to_roll_back_to() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let tenant = "9910000000002";

    let v1 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#let render(i) = [v1]",
        Some("a-3b"),
        Proof::RenderedPdfa,
        Some("ops@example"),
    )
    .await
    .expect("publish v1");
    let v2 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#let render(i) = [v2]",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("publish v2");
    let mahnung = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Mahnung,
        "#let render(i) = [Mahnung]",
        None,
        Proof::RenderedTextform,
        None,
    )
    .await
    .expect("publish Mahnung");
    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v2)
        .await
        .expect("roll out v2");

    let all = template_store::list(&pool, tenant, None, 100)
        .await
        .expect("list");
    assert_eq!(all.len(), 3, "every kind, newest first");
    assert_eq!(all[0].hash, mahnung, "ordered by publication, newest first");

    // Exactly one INVOICE row is current, and it is the one rolled out.
    let invoices = template_store::list(&pool, tenant, Some(TemplateKind::Invoice), 100)
        .await
        .expect("list invoices");
    assert_eq!(invoices.len(), 2, "the kind filter applies");
    let current: Vec<&String> = invoices
        .iter()
        .filter(|t| t.is_current)
        .map(|t| &t.hash)
        .collect();
    assert_eq!(current, vec![&v2]);

    // And the previous one is right there, which is the whole point.
    let previous = invoices
        .iter()
        .find(|t| !t.is_current)
        .expect("the layout to roll back to");
    assert_eq!(previous.hash, v1);
    assert_eq!(previous.proof, "RENDERED_PDFA");
    assert_eq!(previous.pdf_standard.as_deref(), Some("a-3b"));
    assert_eq!(previous.published_by.as_deref(), Some("ops@example"));

    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &previous.hash)
        .await
        .expect("roll back");
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.hash),
        Some(v1),
        "the rollback the listing made discoverable actually works",
    );

    // A tenant sees only its own templates.
    assert!(
        template_store::list(&pool, "9900000000004", None, 100)
            .await
            .expect("list")
            .is_empty(),
        "listings are tenant-scoped",
    );
}

/// One tenant's template source is not readable by another.
///
/// `by_hash` resolved on the hash alone, on the reasoning that "the hash *is*
/// the identity, and a document carrying it has already established the right
/// to see it". That holds for a document. It does not hold for
/// `GET /api/v1/templates/by-hash/{hash}`, where the *caller* supplies the hash
/// and nothing has established anything — so in a shared database one
/// operator's complete template source, Briefkopf and all, was readable by any
/// other operator who came by a hash.
///
/// The lock is the query, not a check in the handler: a condition each
/// caller-facing path has to remember is a condition one of them eventually
/// forgets.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_foreign_tenants_template_does_not_resolve() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let ours = "9900357000004";
    let theirs = "9910000000002";
    let source = "#let render(invoice) = [Briefkopf der Stadtwerke]";

    let hash = template_store::publish(
        &pool,
        theirs,
        TemplateKind::Mahnung,
        source,
        None,
        Proof::RenderedTextform,
        Some("their-operator"),
    )
    .await
    .expect("the other tenant publishes");

    assert!(
        template_store::by_hash(&pool, theirs, &hash)
            .await
            .expect("query")
            .is_some(),
        "its owner resolves it",
    );
    assert!(
        template_store::by_hash(&pool, ours, &hash)
            .await
            .expect("query")
            .is_none(),
        "another tenant must not read the source of a template it does not own",
    );

    // And it is invisible to the listing, which was already tenant-scoped —
    // the two reads now agree about what this tenant can see.
    let listed = template_store::list(&pool, ours, None, 100)
        .await
        .expect("list");
    assert!(
        listed.is_empty(),
        "a foreign template must not appear in this tenant's listing: {listed:?}",
    );
}

/// Two operators may publish the same template source.
///
/// Identity is `(tenant, hash)`. A globally unique hash makes the first tenant
/// to publish a source the owner of that identity for everyone: outputd ships a
/// reference layout and tells operators to start from it, so in a shared
/// database exactly one tenant could publish it unchanged and the rest were
/// told to insert a cosmetic comment — which makes the audit identity of an
/// eight-year document depend on filler. It also answered them "already
/// published **by another tenant**", disclosing another operator's template
/// inventory to anyone who could guess a source.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn two_tenants_may_publish_the_same_source() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let source = outputd::document::REFERENCE_INVOICE_TEMPLATE;

    let mut hashes = Vec::new();
    for tenant in ["9900000000001", "9900000000002"] {
        hashes.push(
            template_store::publish(
                &pool,
                tenant,
                TemplateKind::Invoice,
                source,
                Some("a-3b"),
                Proof::RenderedPdfa,
                None,
            )
            .await
            .unwrap_or_else(|e| {
                panic!("{tenant} must be able to publish the reference layout: {e}")
            }),
        );
    }
    assert_eq!(hashes[0], hashes[1], "same bytes, same content address");

    // Each tenant rolls out and reads back its own row.
    for tenant in ["9900000000001", "9900000000002"] {
        template_store::set_current(&pool, tenant, TemplateKind::Invoice, &hashes[0])
            .await
            .expect("each tenant points at its own row");
        let current = template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .expect("read back")
            .expect("a current template");
        assert_eq!(current.tenant, tenant, "no tenant resolves another's row");
        assert_eq!(current.source, source);
    }

    // And a listing shows one row per tenant, not one row shared.
    for tenant in ["9900000000001", "9900000000002"] {
        let listed = template_store::list(&pool, tenant, None, 10)
            .await
            .expect("list");
        assert_eq!(listed.len(), 1, "{tenant} sees exactly its own template");
    }
}

// ── Issued documents and delivery ─────────────────────────────────────────────

use outputd::delivery::{Channel, store as docs};

/// The issuing operator these tests act as.
const TENANT: &str = "9900000000001";

fn a_document<'a>(subject: &'a str, content: &'a [u8]) -> docs::NewDocument<'a> {
    docs::NewDocument {
        tenant: TENANT,
        kind: "MAHNUNG",
        template_hash: "0000000000000000000000000000000000000000000000000000000000000000",
        subject_ref: subject,
        malo_id: Some("51238696012"),
        kunden_nr: Some("K-4711"),
        content,
        media_type: "application/pdf",
        recipient: docs::Recipient {
            name: Some("Erika Mustermann".to_owned()),
            email: Some("erika@example.test".to_owned()),
            address: Some(serde_json::json!({ "plz": "10115", "ort": "Berlin" })),
        },
        issued_by: Some("operator-sub"),
    }
}

/// Issuing is idempotent on `(tenant, kind, subject_ref)`, and the stored bytes
/// come back byte-for-byte.
///
/// A duplicate invoice row is untidy; a duplicate **Mahnung** is a second
/// statutory notice with its own deadline and a second § 41f EnWG clock.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn issuing_the_same_subject_twice_sends_one_document() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let pdf = b"%PDF-1.7 first".as_slice();
    let (first, created) = docs::issue(&pool, &a_document("case-1", pdf), &[Channel::Portal])
        .await
        .expect("issue");
    assert!(created);
    assert_eq!(first.deliveries.len(), 1);

    // A retry — with *different* bytes, as a re-render after a template rollout
    // would produce. The stored document must not move: § 147 AO asks for what
    // was issued.
    let (again, created) = docs::issue(
        &pool,
        &a_document("case-1", b"%PDF-1.7 second".as_slice()),
        &[Channel::Portal, Channel::Email],
    )
    .await
    .expect("issue again");
    assert!(!created, "a retry issues nothing");
    assert_eq!(again.document.document_id, first.document.document_id);
    assert_eq!(
        again.deliveries.len(),
        1,
        "and queues no second delivery either"
    );

    let (bytes, media) = docs::content(&pool, TENANT, first.document.document_id)
        .await
        .expect("content")
        .expect("present");
    assert_eq!(bytes, pdf, "the bytes that were sent, not a re-render");
    assert_eq!(media, "application/pdf");
    assert_eq!(first.document.content_sha256, docs::content_hash(pdf));
}

/// A channel with nothing to send to is stored `SUPPRESSED` **with a reason**,
/// never omitted — "why did this never go out" has to be answerable from the
/// row rather than from its absence.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_channel_with_no_target_is_suppressed_with_its_reason() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let mut doc = a_document("case-2", b"%PDF-1.7".as_slice());
    doc.recipient.email = None;
    let (issued, _) = docs::issue(&pool, &doc, &[Channel::Portal, Channel::Email])
        .await
        .expect("issue");

    let email = issued
        .deliveries
        .iter()
        .find(|d| d.channel == "EMAIL")
        .expect("the EMAIL row exists even though it cannot be sent");
    assert_eq!(email.status, "SUPPRESSED");
    assert!(
        email
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("no e-mail address")),
        "the reason is on the row: {:?}",
        email.last_error
    );
    let portal = issued
        .deliveries
        .iter()
        .find(|d| d.channel == "PORTAL")
        .expect("portal queued");
    assert_eq!(portal.status, "PENDING");
}

/// The worker's claim moves a delivery out of the due set, so two replicas
/// never send the same document twice — and the claim releases on the backoff,
/// so a replica that dies mid-send loses nothing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_claimed_delivery_is_not_claimed_again() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let (issued, _) = docs::issue(
        &pool,
        &a_document("case-3", b"%PDF-1.7".as_slice()),
        &[Channel::Portal],
    )
    .await
    .expect("issue");

    let first = docs::claim_due(&pool, TENANT, 10, time::Duration::minutes(5), true)
        .await
        .expect("claim");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].attempts, 1, "the claim counts the attempt");
    assert_eq!(first[0].channel, Channel::Portal);

    let second = docs::claim_due(&pool, TENANT, 10, time::Duration::minutes(5), true)
        .await
        .expect("claim again");
    assert!(second.is_empty(), "the backoff holds the claim");

    docs::record_success(
        &pool,
        TENANT,
        first[0].delivery_id,
        true,
        Some(serde_json::json!({"published": "portal-inbox"})),
    )
    .await
    .expect("record success");

    let after = docs::deliveries_of(&pool, issued.document.document_id)
        .await
        .expect("deliveries");
    assert_eq!(after[0].status, "DELIVERED");
    assert!(
        after[0].delivered_at.is_some(),
        "a DELIVERED row states when — the schema refuses one that does not"
    );

    // The portal read receipt: more than § 126b BGB asks for, and exactly what
    // a dispute over a § 41f notice asks about.
    assert!(
        docs::record_read(&pool, TENANT, first[0].delivery_id)
            .await
            .expect("record read")
    );
    let after = docs::deliveries_of(&pool, issued.document.document_id)
        .await
        .expect("deliveries");
    assert!(after[0].read_at.is_some());
}

/// A failed attempt stays `PENDING` until the ceiling, then becomes `FAILED` —
/// the state that says a customer never received something the platform
/// believes it sent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_failing_delivery_retries_and_then_gives_up() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let (issued, _) = docs::issue(
        &pool,
        &a_document("case-4", b"%PDF-1.7".as_slice()),
        &[Channel::Email],
    )
    .await
    .expect("issue");
    let delivery_id = issued.deliveries[0].delivery_id;

    docs::record_failure(
        &pool,
        TENANT,
        delivery_id,
        "relay answered 503",
        false,
        time::Duration::ZERO,
    )
    .await
    .expect("record failure");
    let rows = docs::deliveries_of(&pool, issued.document.document_id)
        .await
        .unwrap();
    assert_eq!(rows[0].status, "PENDING", "retryable");
    assert_eq!(rows[0].last_error.as_deref(), Some("relay answered 503"));

    docs::record_failure(
        &pool,
        TENANT,
        delivery_id,
        "relay answered 503",
        true,
        time::Duration::ZERO,
    )
    .await
    .expect("give up");
    let rows = docs::deliveries_of(&pool, issued.document.document_id)
        .await
        .unwrap();
    assert_eq!(rows[0].status, "FAILED");
    assert!(rows[0].delivered_at.is_none());
}

/// A `POST` delivery with no postal relay is a letter waiting to be collected,
/// and it stays in the spool however long nobody collects it.
///
/// The pull model — the print service calls `GET /api/v1/spool`, fetches the
/// bytes and reports back — is how most Druckdienstleister integrate, and it is
/// what a deployment with no `postal_relay_url` has chosen. The worker used to
/// treat it as a failed push instead: every tick claimed the row, `deliver`
/// bailed with "no postal_relay_url configured", and at `max_attempts` the row
/// became `FAILED`. `postal_spool` lists `PENDING` rows, so about half a day
/// after issuing, every letter silently left the spool — unprinted, unsent, and
/// visible only in a status column nobody queries.
///
/// The test runs the real worker loop past the retry ceiling, advancing the
/// backoff clock between passes so the budget would genuinely be spent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_postal_delivery_without_a_relay_stays_in_the_spool() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let (issued, _) = docs::issue(
        &pool,
        &a_document("case-post", b"%PDF-1.7".as_slice()),
        &[Channel::Post],
    )
    .await
    .expect("issue");
    assert_eq!(issued.deliveries[0].status, "PENDING");

    // The pull deployment: delivery is on, and no relay of any kind exists.
    let cfg = outputd::config::DeliveryConfig::default();
    assert!(cfg.postal_relay_url.is_none());
    let http = reqwest::Client::new();

    for _ in 0..=cfg.max_attempts + 1 {
        outputd::delivery::worker::tick(&pool, TENANT, &cfg, &http)
            .await
            .expect("tick");
        // Advance the backoff clock: without this the ceiling is days away and
        // the loop proves nothing about what happens when it is reached.
        sqlx::query("UPDATE document_deliveries SET next_attempt_at = now()")
            .execute(&pool)
            .await
            .expect("advance the backoff clock");
    }

    let rows = docs::deliveries_of(&pool, issued.document.document_id)
        .await
        .expect("deliveries");
    assert_eq!(
        rows[0].status, "PENDING",
        "a letter nobody has collected yet is not a failed delivery"
    );
    assert_eq!(
        rows[0].attempts, 0,
        "no push was attempted, so no attempt may be counted against the budget"
    );

    let spool = docs::postal_spool(&pool, TENANT, 100).await.expect("spool");
    assert!(
        spool
            .iter()
            .any(|d| d.delivery_id == issued.deliveries[0].delivery_id),
        "the print service must still find the letter in GET /api/v1/spool"
    );

    // And the pull half closes the loop: the print service reports back and the
    // row leaves the spool the way a delivered letter should.
    docs::record_success(
        &pool,
        TENANT,
        issued.deliveries[0].delivery_id,
        true,
        Some(serde_json::json!({ "batch": "DRUCK-2026-09-05" })),
    )
    .await
    .expect("report collected");
    let spool = docs::postal_spool(&pool, TENANT, 100).await.expect("spool");
    assert!(spool.is_empty(), "a collected letter leaves the spool");
}

/// The document list refuses to answer without a customer scope. `portald`
/// forwards a customer's scope into this query, and a filter that silently
/// degrades to "everything" is one bug away from serving the whole portfolio.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unscoped_document_query_returns_nothing() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    docs::issue(
        &pool,
        &a_document("case-5", b"%PDF-1.7".as_slice()),
        &[Channel::Portal],
    )
    .await
    .expect("issue");

    let unscoped = docs::list(&pool, TENANT, &docs::DocumentFilter::default())
        .await
        .expect("list");
    assert!(unscoped.is_empty(), "no scope, no rows");

    let scoped = docs::list(
        &pool,
        TENANT,
        &docs::DocumentFilter {
            malo_id: Some("51238696012".to_owned()),
            ..Default::default()
        },
    )
    .await
    .expect("list");
    assert_eq!(scoped.len(), 1);

    // Another tenant's identical scope sees nothing: tenant equality is in the
    // query, not only in the policy.
    let other = docs::list(
        &pool,
        "9910000000002",
        &docs::DocumentFilter {
            malo_id: Some("51238696012".to_owned()),
            ..Default::default()
        },
    )
    .await
    .expect("list");
    assert!(other.is_empty());
}

/// The print spool lists what a Druckdienstleister has not collected yet.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_postal_spool_lists_what_is_waiting() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    docs::issue(
        &pool,
        &a_document("case-6", b"%PDF-1.7".as_slice()),
        &[Channel::Post, Channel::Portal],
    )
    .await
    .expect("issue");

    let spool = docs::postal_spool(&pool, TENANT, 100).await.expect("spool");
    assert_eq!(spool.len(), 1, "the POST row, not the portal one");
    assert_eq!(spool[0].channel, Channel::Post);
    assert!(
        spool[0]
            .target
            .as_deref()
            .is_some_and(|t| t.contains("Berlin")),
        "the address travels with the spool entry"
    );
}
