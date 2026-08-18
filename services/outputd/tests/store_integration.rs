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
            Proof::Parsed,
            None,
        )
        .await
        .is_err(),
        "an invoice template may not be stored on the weaker proof",
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

    // A Mahnung has a view and a specimen now, so the parse proof is no longer
    // admissible for it — the constraint refuses the downgrade.
    assert!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Mahnung,
            "#let render(i) = [Zahlungserinnerung]",
            None,
            Proof::Parsed,
            None,
        )
        .await
        .is_err(),
        "a MAHNUNG row may not be stored on the parse proof",
    );

    // PREISANPASSUNG is the kind that still stores on the proof it can offer.
    let hash = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Preisanpassung,
        "#let render(i) = [Preisanpassung nach § 41 Abs. 5 EnWG]",
        None,
        Proof::Parsed,
        None,
    )
    .await
    .expect("a Preisanpassung stores on the parse proof");
    assert_eq!(
        template_store::by_hash(&pool, tenant, &hash)
            .await
            .unwrap()
            .map(|t| t.proof),
        Some("PARSED".to_owned()),
        "the store records which proof was obtained",
    );
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
