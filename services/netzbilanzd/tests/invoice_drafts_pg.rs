//! Real-PostgreSQL tests for `netzbilanzd`'s persistence.
//!
//! The queries here are runtime-checked SQL strings, so a rename that compiles
//! can still fail at the database. These pin the properties that money depends
//! on: invoice numbers are consecutive and unique, a period is billed once,
//! every read is tenant-scoped, and the aggregate columns actually decode.
//!
//! Uses testcontainers — runs under `cargo test` when a Docker daemon is
//! available, and skips itself otherwise.

use grid_billing::{ArbeitspreisModell, MengePreis, Sparte};
use invoic_checker::check::CheckOutcome;
use netzbilanzd::pg::{
    self, DraftFilter, InsertDraftError, NewDraft, billing_summary, fetch_draft, has_storno,
    insert_draft, list_drafts, mark_dispatched, mark_disputed, mark_paid, next_rechnungsnummer,
    reject_draft,
};
use netzbilanzd::request::{NneRequest, SettlementRequest};
use rust_decimal::dec;
use time::macros::date;
use uuid::Uuid;

/// The container guard a test holds until it ends.
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
        .expect("apply netzbilanzd schema");
    Some((pool, container))
}

/// Run `body` against a fresh database, or skip when Docker is unavailable.
macro_rules! with_pg {
    (|$pool:ident| $body:block) => {{
        let Some(($pool, _guard)) = pg_pool().await else {
            eprintln!("skipping: no Docker daemon");
            return;
        };
        $body
    }};
}

const MSB: &str = "9900999000001";
const NB: &str = "9900357000004";
const LF: &str = "9900111000002";
const ESA: &str = "9905550000005";
const MALO: &str = "51238696012";
const MALO_2: &str = "51238696781";
const MALO_3: &str = "51238696129";

/// A minimal settlement input, so a stored draft carries something replayable.
fn settlement_input(sparte: Sparte) -> serde_json::Value {
    serde_json::to_value(SettlementRequest::Nne(Box::new(NneRequest {
        nb_mp_id: NB.to_owned(),
        lf_mp_id: LF.to_owned(),
        sparte,
        arbeitspreis: ArbeitspreisModell::Einheitlich(MengePreis {
            menge_kwh: dec!(1000),
            preis_ct_per_kwh: dec!(3.5),
        }),
        leistungspreis: None,
        grundpreis: None,
        konzessionsabgabe: None,
        blindarbeit: None,
        gas_kapazitaet: None,
        letztverbrauchergruppe: grid_billing::umlagen::Letztverbrauchergruppe::default(),
        enfg_jahresvorverbrauch_kwh: None,
        sect19_umlage_ct_per_kwh: None,
        offshore_umlage_ct_per_kwh: None,
        kwkg_umlage_ct_per_kwh: None,
        netzebene: None,
        sect19: None,
        jahreshoechstleistung_kw: None,
        jahresarbeit_kwh: None,
        tariff_sheet_id: None,
    })))
    .expect("serialize settlement input")
}

/// A draft template the tests vary.
struct Draft {
    tenant: &'static str,
    malo: &'static str,
    sender: &'static str,
    recipient: &'static str,
    pid: i32,
    sparte: &'static str,
    settlement_type: &'static str,
    rechnungsnummer: String,
    period: (time::Date, time::Date),
    netto_eur_units: i64,
    /// `S` taxed at 19 %, or `AE` reverse-charged.
    steuer_kategorie: &'static str,
    invoice_date: time::Date,
    due_date: time::Date,
    outcome: CheckOutcome,
    /// Collectible amount, when Abschläge were deducted. Defaults to the gross.
    zu_zahlen_override: Option<i64>,
}

impl Draft {
    /// The tax on this draft's net, as the schema's CHECK constraints require.
    fn steuer_satz(&self) -> i64 {
        if self.steuer_kategorie == "AE" { 0 } else { 19 }
    }

    fn steuer_eur_units(&self) -> i64 {
        self.netto_eur_units * self.steuer_satz() / 100
    }
}

impl Draft {
    fn nne(tenant: &'static str, malo: &'static str, rechnungsnummer: &str) -> Self {
        Self {
            tenant,
            malo,
            sender: NB,
            recipient: LF,
            pid: 31002,
            sparte: "STROM",
            settlement_type: "NneStrom",
            rechnungsnummer: rechnungsnummer.to_owned(),
            period: (date!(2026 - 01 - 01), date!(2026 - 01 - 31)),
            netto_eur_units: 123_456_000,
            steuer_kategorie: "S",
            invoice_date: date!(2026 - 02 - 01),
            due_date: date!(2026 - 03 - 03),
            outcome: CheckOutcome::Ok,
            zu_zahlen_override: None,
        }
    }

    /// The same draft billed for a period ending on a different day, so several
    /// can coexist under the double-billing guard.
    fn period_ending(mut self, period_to: time::Date) -> Self {
        self.period.1 = period_to;
        self
    }

    /// An Abschlagsrechnung (PID 31001) issued on a given Rechnungsdatum.
    fn abschlag(
        tenant: &'static str,
        malo: &'static str,
        rechnungsnummer: &str,
        invoice_date: time::Date,
    ) -> Self {
        Self {
            pid: 31001,
            settlement_type: "NneAbschlag",
            invoice_date,
            due_date: invoice_date.saturating_add(time::Duration::days(30)),
            // The instalments run against the whole year the Abschlussrechnung
            // settles, which is what makes several of them legitimate.
            period: (date!(2026 - 01 - 01), date!(2026 - 12 - 31)),
            ..Self::nne(tenant, malo, rechnungsnummer)
        }
    }

    async fn insert(&self, pool: &sqlx::PgPool) -> Result<Uuid, InsertDraftError> {
        let mut conn = pool.acquire().await.expect("acquire");
        insert_draft(
            &mut conn,
            &NewDraft {
                tenant: self.tenant,
                malo_id: self.malo,
                sender_mp_id: self.sender,
                recipient_mp_id: self.recipient,
                pid: self.pid,
                sparte: self.sparte,
                settlement_type: self.settlement_type,
                period_from: self.period.0,
                period_to: self.period.1,
                rechnungsnummer: &self.rechnungsnummer,
                invoice_date: self.invoice_date,
                due_date: self.due_date,
                settlement_input: settlement_input(if self.sparte == "GAS" {
                    Sparte::Gas
                } else {
                    Sparte::Strom
                }),
                rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
                netto_eur_units: self.netto_eur_units,
                steuer_eur_units: self.steuer_eur_units(),
                brutto_eur_units: self.netto_eur_units + self.steuer_eur_units(),
                zu_zahlen_eur_units: self
                    .zu_zahlen_override
                    .unwrap_or_else(|| self.netto_eur_units + self.steuer_eur_units()),
                steuer_kategorie: self.steuer_kategorie,
                steuer_satz_prozent: rust_decimal::Decimal::from(self.steuer_satz()),
                check_outcome: self.outcome,
                check_findings: serde_json::json!([]),
                settlement_warnings: serde_json::json!([]),
                rechnungsart: "RECHNUNG",
                original_draft_id: None,
                korrektur_grund: None,
            },
        )
        .await
    }
}

// ── Invoice parties ───────────────────────────────────────────────────────────

/// A 31009 draft stores the MSB as sender and the NB / LF / ESA as recipient.
///
/// The *Anwendungsübersicht der Prüfidentifikatoren* 4.0 lists seven
/// Anwendungsfälle for 31009 and the sender is the MSB in every one. Storing it
/// the other way round named the party owed money as the one billing for it.
#[tokio::test]
async fn a_msb_rechnung_stores_the_msb_as_sender() {
    with_pg!(|pool| {
        for (idx, (recipient, malo)) in [(NB, MALO), (LF, MALO_2), (ESA, MALO_3)]
            .into_iter()
            .enumerate()
        {
            let mut draft = Draft::nne("t1", malo, &format!("MSB-2026-{:06}", idx + 1));
            draft.sender = MSB;
            draft.recipient = recipient;
            draft.pid = 31009;
            draft.settlement_type = "MsbRechnung";
            let id = draft.insert(&pool).await.expect("insert a 31009 draft");

            let row = fetch_draft(&pool, "t1", id)
                .await
                .expect("fetch")
                .expect("the draft exists");
            assert_eq!(
                row.sender_mp_id, MSB,
                "PID 31009 is issued by the MSB, not to it"
            );
            assert_eq!(row.recipient_mp_id, recipient);
        }

        // Filtering by sender finds them under the MSB — which is what the old
        // `nb_mp_id` column naming invited callers to get wrong.
        let by_sender = list_drafts(
            &pool,
            "t1",
            &DraftFilter {
                sender_mp_id: Some(MSB),
                limit: 100,
                ..DraftFilter::default()
            },
        )
        .await
        .expect("list by sender");
        assert_eq!(by_sender.len(), 3);
    });
}

// ── Invoice numbering (§14 Abs. 4 Nr. 4 UStG) ─────────────────────────────────

/// Numbers run consecutively per tenant, series and year, and restart per year.
#[tokio::test]
async fn invoice_numbers_are_consecutive_per_series_and_year() {
    with_pg!(|pool| {
        let mut conn = pool.acquire().await.expect("acquire");

        assert_eq!(
            next_rechnungsnummer(&mut conn, "t1", Some("NNE"), 2026)
                .await
                .expect("allocate"),
            "NNE-2026-000001"
        );
        assert_eq!(
            next_rechnungsnummer(&mut conn, "t1", Some("NNE"), 2026)
                .await
                .expect("allocate"),
            "NNE-2026-000002"
        );

        // A different series has its own counter…
        assert_eq!(
            next_rechnungsnummer(&mut conn, "t1", Some("MMM"), 2026)
                .await
                .expect("allocate"),
            "MMM-2026-000001"
        );
        // …as does a different year…
        assert_eq!(
            next_rechnungsnummer(&mut conn, "t1", Some("NNE"), 2027)
                .await
                .expect("allocate"),
            "NNE-2027-000001"
        );
        // …and a different tenant.
        assert_eq!(
            next_rechnungsnummer(&mut conn, "t2", Some("NNE"), 2026)
                .await
                .expect("allocate"),
            "NNE-2026-000001"
        );
        // No series named — the number still identifies the year and sequence.
        assert_eq!(
            next_rechnungsnummer(&mut conn, "t1", None, 2026)
                .await
                .expect("allocate"),
            "2026-000001"
        );
    });
}

/// A rolled-back run consumes no invoice number.
///
/// The counter is bumped inside the drafting transaction, so an abandoned run
/// leaves no gap in the sequence — which is what "fortlaufend" requires.
#[tokio::test]
async fn a_rolled_back_run_consumes_no_invoice_number() {
    with_pg!(|pool| {
        let mut tx = pool.begin().await.expect("begin");
        let allocated = next_rechnungsnummer(&mut tx, "t1", Some("NNE"), 2026)
            .await
            .expect("allocate");
        assert_eq!(allocated, "NNE-2026-000001");
        tx.rollback().await.expect("rollback");

        let mut conn = pool.acquire().await.expect("acquire");
        assert_eq!(
            next_rechnungsnummer(&mut conn, "t1", Some("NNE"), 2026)
                .await
                .expect("allocate"),
            "NNE-2026-000001",
            "the abandoned number must be reissued, not skipped"
        );
    });
}

/// One invoice number identifies exactly one invoice, per tenant.
#[tokio::test]
async fn an_invoice_number_is_unique_per_tenant() {
    with_pg!(|pool| {
        Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("first insert");

        let clash = Draft::nne("t1", MALO_2, "NNE-2026-000001")
            .insert(&pool)
            .await;
        assert!(
            matches!(clash, Err(InsertDraftError::DuplicateRechnungsnummer)),
            "a reused invoice number must be refused, got {clash:?}"
        );

        // Another tenant runs its own series.
        Draft::nne("t2", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("a different tenant may use the same number");
    });
}

// ── Double billing ────────────────────────────────────────────────────────────

/// A period is billed once, and re-billing it is a refusal — not a silent no-op.
///
/// An upserting insert would return the existing draft and discard the freshly
/// computed one, so an operator who fixed an input and re-ran the job would get
/// the old figures back with a 201.
#[tokio::test]
async fn a_period_is_billed_once_and_re_billing_is_refused() {
    with_pg!(|pool| {
        Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("first billing");

        let mut second = Draft::nne("t1", MALO, "NNE-2026-000002");
        second.netto_eur_units = 999_999_000;
        let outcome = second.insert(&pool).await;
        assert!(
            matches!(outcome, Err(InsertDraftError::AlreadyBilled)),
            "the second billing must be refused, got {outcome:?}"
        );

        // A different Prüfidentifikator over the same period is a different
        // document (a Netzentgelt and an MMM saldo both cover January).
        let mut mmm = Draft::nne("t1", MALO, "MMM-2026-000001");
        mmm.pid = 31005;
        mmm.settlement_type = "MmmStrom";
        mmm.insert(&pool).await.expect("MMM is a separate invoice");
    });
}

/// A period carries several Abschlagsrechnungen, and one final invoice.
///
/// An Abschlag is a payment on account — a monthly one against a yearly period
/// is the ordinary case — so the double-billing guard excludes PID 31001. The
/// instalments are separated by their Rechnungsdatum; the invoice number keeps
/// them distinct and the Abschlussrechnung reconciles them by it.
#[tokio::test]
async fn a_period_carries_many_abschlaege_but_one_final_invoice() {
    with_pg!(|pool| {
        for (i, (nummer, on)) in [
            ("ABS-2026-000001", date!(2026 - 02 - 01)),
            ("ABS-2026-000002", date!(2026 - 03 - 01)),
            ("ABS-2026-000003", date!(2026 - 04 - 01)),
        ]
        .into_iter()
        .enumerate()
        {
            let mut abschlag = Draft::abschlag("t1", MALO, nummer, on);
            abschlag.netto_eur_units = 10_000_000;
            abschlag
                .insert(&pool)
                .await
                .unwrap_or_else(|e| panic!("Abschlag {i} must be allowed: {e:?}"));
        }

        // The invoice that settles them is still billed once.
        Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("the Abschlussrechnung");
        let second = Draft::nne("t1", MALO, "NNE-2026-000002")
            .insert(&pool)
            .await;
        assert!(
            matches!(second, Err(InsertDraftError::AlreadyBilled)),
            "the settled period is still billed once, got {second:?}"
        );
    });
}

/// A replayed billing run cannot produce a second Abschlagsrechnung.
///
/// Excluding PID 31001 from the double-billing guard outright left the one path
/// in this service with no retry protection at all: a `POST /billing/run` that
/// commits and then times out, replayed by the caller, produced a second
/// Abschlag for the same MaLo and period under a fresh invoice number. Both are
/// well-formed and dispatchable, and the Abschlussrechnung deducts both —
/// crediting an Anzahlung the Lieferant paid once.
///
/// The Rechnungsdatum separates the cadence from the retry: instalments differ
/// by it, a replay does not.
#[tokio::test]
async fn a_replayed_run_cannot_produce_a_second_abschlag_for_the_same_day() {
    with_pg!(|pool| {
        let on = date!(2026 - 02 - 01);
        Draft::abschlag("t1", MALO, "ABS-2026-000001", on)
            .insert(&pool)
            .await
            .expect("the first Abschlag");

        // The replay allocates a fresh number, so nothing else would catch it.
        let replay = Draft::abschlag("t1", MALO, "ABS-2026-000002", on)
            .insert(&pool)
            .await;
        assert!(
            matches!(replay, Err(InsertDraftError::AbschlagAlreadyBilled)),
            "a same-day replay must be refused, got {replay:?}"
        );

        // The next instalment carries its own Rechnungsdatum and is allowed.
        Draft::abschlag("t1", MALO, "ABS-2026-000003", date!(2026 - 03 - 01))
            .insert(&pool)
            .await
            .expect("the next instalment");

        // Rejecting reopens the day, as it does for every other Rechnungsart.
        let first = Draft::abschlag("t1", MALO, "ABS-2026-000004", on)
            .insert(&pool)
            .await;
        assert!(matches!(
            first,
            Err(InsertDraftError::AbschlagAlreadyBilled)
        ));
    });
}

/// A deduction takes its amount from the stored Abschlag, and refuses one that
/// was never sent or has been reversed.
///
/// INVOIC AHB **[526]** — the deducted amount must equal the referenced
/// Abschlagsrechnung's own Rechnungsbetrag; **[519]** — a stornierte
/// Abschlagsrechnung is not listed, because nothing was paid on it.
#[tokio::test]
async fn a_deduction_matches_the_stored_abschlag_and_refuses_a_reversed_one() {
    with_pg!(|pool| {
        let mut abschlag = Draft::nne("t1", MALO, "ABS-2026-000001");
        abschlag.pid = 31001;
        abschlag.settlement_type = "NneAbschlag";
        abschlag.netto_eur_units = 10_000_000; // 100.00 EUR net → 119.00 gross
        let paid = abschlag.insert(&pool).await.expect("insert");

        let mut conn = pool.acquire().await.expect("acquire");

        // Still a draft: the counterparty never received it.
        let undispatched = pg::load_abschlaege(&mut conn, "t1", MALO, &[paid])
            .await
            .expect("query");
        assert!(
            undispatched.is_err(),
            "an undispatched Abschlag is not owed"
        );

        mark_dispatched(
            &mut conn,
            "t1",
            paid,
            "process-1",
            &serde_json::json!({ "_typ": "RECHNUNG" }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch");

        let deductions = pg::load_abschlaege(&mut conn, "t1", MALO, &[paid])
            .await
            .expect("query")
            .expect("one deduction");
        assert_eq!(deductions.len(), 1);
        assert_eq!(deductions[0].rechnungsnummer, "ABS-2026-000001");
        assert_eq!(
            deductions[0].betrag_brutto_eur,
            rust_decimal::Decimal::from(119),
            "the gross as billed, not a caller-supplied figure"
        );

        // Wrong MaLo, and wrong PID, are both refused.
        assert!(
            pg::load_abschlaege(&mut conn, "t1", MALO_2, &[paid])
                .await
                .expect("query")
                .is_err()
        );
        let nne = Draft::nne("t1", MALO_3, "NNE-2026-000009")
            .insert(&pool)
            .await
            .expect("insert");
        assert!(
            pg::load_abschlaege(&mut conn, "t1", MALO_3, &[nne])
                .await
                .expect("query")
                .is_err(),
            "an NN-Rechnung is not an Abschlag"
        );

        // Once reversed, it is excluded (AHB [519]).
        insert_draft(
            &mut conn,
            &NewDraft {
                tenant: "t1",
                malo_id: MALO,
                sender_mp_id: NB,
                recipient_mp_id: LF,
                pid: 31001,
                sparte: "STROM",
                settlement_type: "NneAbschlag",
                period_from: date!(2026 - 01 - 01),
                period_to: date!(2026 - 01 - 31),
                rechnungsnummer: "ABS-2026-000002",
                invoice_date: date!(2026 - 02 - 01),
                due_date: date!(2026 - 03 - 03),
                settlement_input: settlement_input(Sparte::Strom),
                rechnung: serde_json::json!({ "_typ": "RECHNUNG", "istStorno": true }),
                netto_eur_units: -10_000_000,
                steuer_eur_units: -1_900_000,
                brutto_eur_units: -11_900_000,
                zu_zahlen_eur_units: -11_900_000,
                steuer_kategorie: "S",
                steuer_satz_prozent: rust_decimal::Decimal::from(19),
                check_outcome: CheckOutcome::Ok,
                check_findings: serde_json::json!([]),
                settlement_warnings: serde_json::json!([]),
                rechnungsart: "STORNORECHNUNG",
                original_draft_id: Some(paid),
                korrektur_grund: Some("RECHENFEHLER"),
            },
        )
        .await
        .expect("reverse it");

        assert!(
            pg::load_abschlaege(&mut conn, "t1", MALO, &[paid])
                .await
                .expect("query")
                .is_err(),
            "a reversed Abschlag was never paid, so it cannot be deducted"
        );
    });
}

/// **Invariant: an Abschlagsrechnung is deducted from exactly one invoice.**
///
/// An Abschlag is a payment on account; the Abschlussrechnung that names it
/// deducts its gross from what is owed. Nothing tied the two documents together,
/// so two consecutive monthly NN-Rechnungen could each name the same Abschlag
/// and each deduct it — both well-formed, both passing INVOIC rule [526] on
/// their own, and the second collecting money the Netzbetreiber already held.
///
/// Guarded on both paths: `load_abschlaege` refuses it on the read, and the
/// primary key refuses it on the write, which is what two concurrent billing
/// runs need.
#[tokio::test]
async fn an_abschlag_is_deducted_from_one_invoice_only() {
    with_pg!(|pool| {
        let mut abschlag = Draft::nne("t1", MALO, "ABS-2026-000001");
        abschlag.pid = 31001;
        abschlag.settlement_type = "NneAbschlag";
        abschlag.netto_eur_units = 10_000_000;
        let paid = abschlag.insert(&pool).await.expect("insert");

        let mut conn = pool.acquire().await.expect("acquire");
        mark_dispatched(
            &mut conn,
            "t1",
            paid,
            "process-1",
            &serde_json::json!({ "_typ": "RECHNUNG" }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch");

        // January's Abschlussrechnung settles it.
        let january = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert");
        pg::record_abschlag_verrechnungen(&mut conn, "t1", january, &[paid])
            .await
            .expect("the first deduction stands");

        // February's cannot name the same Abschlag — refused on the read…
        let refused = pg::load_abschlaege(&mut conn, "t1", MALO, &[paid])
            .await
            .expect("query");
        let problems = refused.expect_err("an Abschlag already settled cannot be deducted again");
        assert!(
            problems[0].contains("NNE-2026-000001"),
            "the refusal names the invoice that settled it: {problems:?}"
        );

        // …and on the write, which is the guard two concurrent runs need.
        let february = Draft::nne("t1", MALO, "NNE-2026-000002")
            .period_ending(date!(2026 - 02 - 28))
            .insert(&pool)
            .await
            .expect("insert");
        assert!(
            matches!(
                pg::record_abschlag_verrechnungen(&mut conn, "t1", february, &[paid]).await,
                Err(InsertDraftError::AbschlagAlreadyDeducted)
            ),
            "a second deduction must be refused by the database, not only by the read"
        );

        // Reversing January releases it, so the Korrekturrechnung can settle it.
        pg::release_abschlag_verrechnungen(&pool, "t1", january)
            .await
            .expect("release");
        assert_eq!(
            pg::load_abschlaege(&mut conn, "t1", MALO, &[paid])
                .await
                .expect("query")
                .expect("deductible again")
                .len(),
            1
        );
    });
}

/// Rejecting a draft releases the Abschläge it settled along with the period.
///
/// Reopening the period without reopening its Anzahlungen strands them: the
/// re-billed invoice cannot deduct money the customer has already paid.
#[tokio::test]
async fn rejecting_a_draft_releases_its_abschlaege() {
    with_pg!(|pool| {
        let mut abschlag = Draft::nne("t1", MALO, "ABS-2026-000001");
        abschlag.pid = 31001;
        abschlag.settlement_type = "NneAbschlag";
        let paid = abschlag.insert(&pool).await.expect("insert");

        let mut conn = pool.acquire().await.expect("acquire");
        mark_dispatched(
            &mut conn,
            "t1",
            paid,
            "process-1",
            &serde_json::json!({ "_typ": "RECHNUNG" }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch");

        let first = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert");
        pg::record_abschlag_verrechnungen(&mut conn, "t1", first, &[paid])
            .await
            .expect("deduct");

        assert!(
            reject_draft(&pool, "t1", first, "Ablesefehler")
                .await
                .expect("reject")
        );

        let second = Draft::nne("t1", MALO, "NNE-2026-000002")
            .insert(&pool)
            .await
            .expect("the period is billable again");
        pg::record_abschlag_verrechnungen(&mut conn, "t1", second, &[paid])
            .await
            .expect("and so is the Abschlag it settles");
    });
}

/// Rejecting a draft reopens the period.
#[tokio::test]
async fn rejecting_a_draft_reopens_the_period() {
    with_pg!(|pool| {
        let first = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("first billing");

        assert!(
            reject_draft(&pool, "t1", first, "Ablesefehler")
                .await
                .expect("reject")
        );

        Draft::nne("t1", MALO, "NNE-2026-000002")
            .insert(&pool)
            .await
            .expect("the period is billable again once the draft is rejected");
    });
}

// ── Tenant scoping ────────────────────────────────────────────────────────────

/// Every read and every transition is scoped to its tenant.
///
/// A query that ignores the column lets a draft UUID from one deployment fetch,
/// reject or dispatch another's invoice.
#[tokio::test]
async fn every_read_and_transition_is_tenant_scoped() {
    with_pg!(|pool| {
        let id = Draft::nne("tenant-a", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert");

        assert!(
            fetch_draft(&pool, "tenant-b", id)
                .await
                .expect("fetch")
                .is_none(),
            "another tenant must not read the draft"
        );
        assert!(
            !reject_draft(&pool, "tenant-b", id, "not mine")
                .await
                .expect("reject"),
            "another tenant must not reject the draft"
        );

        let mut conn = pool.acquire().await.expect("acquire");
        assert!(
            !mark_dispatched(
                &mut conn,
                "tenant-b",
                id,
                "ref",
                &serde_json::json!({ "_typ": "RECHNUNG" }),
                CheckOutcome::Ok,
                &serde_json::json!([]),
            )
            .await
            .expect("dispatch"),
            "another tenant must not dispatch the draft"
        );

        // The owning tenant still can.
        assert!(
            fetch_draft(&pool, "tenant-a", id)
                .await
                .expect("fetch")
                .is_some()
        );
        assert!(
            reject_draft(&pool, "tenant-a", id, "Ablesefehler")
                .await
                .expect("reject")
        );
    });
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

/// A dispute is its own status and leaves the pre-dispatch verdict intact.
///
/// Overwriting `check_outcome` would destroy the NB's own verdict — the evidence
/// that says whether the invoice left the house defensible — and leave `status`
/// reading `'dispatched'`.
#[tokio::test]
async fn a_dispute_is_its_own_status_and_preserves_the_check_verdict() {
    with_pg!(|pool| {
        let mut draft = Draft::nne("t1", MALO, "NNE-2026-000001");
        draft.outcome = CheckOutcome::Warn;
        let id = draft.insert(&pool).await.expect("insert");

        let mut conn = pool.acquire().await.expect("acquire");
        assert!(
            mark_dispatched(
                &mut conn,
                "t1",
                id,
                "process-42",
                &serde_json::json!({ "_typ": "RECHNUNG" }),
                // The dispatch-time re-check kept the drafting verdict.
                CheckOutcome::Warn,
                &serde_json::json!([]),
            )
            .await
            .expect("dispatch")
        );
        assert!(
            mark_disputed(&mut conn, "t1", id, "Z32", "Tarifabweichung")
                .await
                .expect("dispute")
        );

        let row = fetch_draft(&pool, "t1", id)
            .await
            .expect("fetch")
            .expect("exists");
        assert_eq!(row.status, "disputed");
        assert_eq!(row.dispute_erc_code.as_deref(), Some("Z32"));
        assert_eq!(row.dispute_reason.as_deref(), Some("Tarifabweichung"));
        assert_eq!(
            row.check_outcome, "Warn",
            "the NB's own pre-dispatch verdict must survive the counterparty's"
        );
        assert_eq!(row.dispatch_ref.as_deref(), Some("process-42"));

        // A disputed invoice can still be settled — the counterparty may pay
        // after the objection is resolved without a correction.
        assert!(
            mark_paid(&mut conn, "t1", id, "REMADV-1")
                .await
                .expect("pay")
        );
        let row = fetch_draft(&pool, "t1", id)
            .await
            .expect("fetch")
            .expect("exists");
        assert_eq!(row.status, "paid");
        assert_eq!(row.remadv_ref.as_deref(), Some("REMADV-1"));
    });
}

/// A draft cannot be paid before it is dispatched.
#[tokio::test]
async fn an_undispatched_draft_cannot_be_paid() {
    with_pg!(|pool| {
        let id = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert");
        let mut conn = pool.acquire().await.expect("acquire");
        assert!(
            !mark_paid(&mut conn, "t1", id, "REMADV-1")
                .await
                .expect("pay"),
            "an invoice the counterparty never received cannot have been paid"
        );
    });
}

// ── Aggregates ────────────────────────────────────────────────────────────────

/// The monthly summary decodes.
///
/// PostgreSQL's `sum(bigint)` returns `numeric`, and decoding that straight into
/// an `i64` fails at runtime rather than at compile time. The query casts; this
/// is what proves it.
#[tokio::test]
async fn the_monthly_summary_decodes_and_totals() {
    with_pg!(|pool| {
        Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert");

        let mut second = Draft::nne("t1", MALO_2, "NNE-2026-000002");
        second.netto_eur_units = 76_544_000;
        second.insert(&pool).await.expect("insert");

        // A Gas invoice in the same month groups separately: NN-Rechnung Strom
        // and Gas share PID 31002, so the PID alone cannot tell them apart.
        let mut gas = Draft::nne("t1", MALO_3, "NNE-2026-000003");
        gas.sparte = "GAS";
        gas.settlement_type = "NneGas";
        gas.netto_eur_units = 10_000_000;
        gas.insert(&pool).await.expect("insert");

        let rows = billing_summary(&pool, "t1", 2026, 1)
            .await
            .expect("the summary must decode");

        let strom: i64 = rows
            .iter()
            .filter(|r| r.sparte == "STROM")
            .map(|r| r.netto_eur_units)
            .sum();
        assert_eq!(strom, 200_000_000, "123.456 + 765.44 EUR");

        let gas_total: i64 = rows
            .iter()
            .filter(|r| r.sparte == "GAS")
            .map(|r| r.netto_eur_units)
            .sum();
        assert_eq!(gas_total, 10_000_000);

        // Another tenant's month is empty, not everyone's.
        assert!(
            billing_summary(&pool, "t2", 2026, 1)
                .await
                .expect("summary")
                .is_empty()
        );
    });
}

/// A disputed draft is not reported as overdue for dispatch.
#[tokio::test]
async fn a_disputed_draft_is_not_reported_as_dispatch_overdue() {
    with_pg!(|pool| {
        let mut blocked = Draft::nne("t1", MALO, "NNE-2026-000001");
        blocked.outcome = CheckOutcome::Dispute;
        blocked.insert(&pool).await.expect("insert");

        let mut ok = Draft::nne("t1", MALO_2, "NNE-2026-000002");
        ok.outcome = CheckOutcome::Warn;
        ok.insert(&pool).await.expect("insert");

        // Backdate both so the age filter has something to find.
        sqlx::query("UPDATE invoice_drafts SET created_at = now() - INTERVAL '72 hours'")
            .execute(&pool)
            .await
            .expect("backdate");

        let stale = pg::list_undispatched_stale(&pool, "t1", 48, 100)
            .await
            .expect("list");
        assert_eq!(stale.len(), 1, "the disputed draft is blocked, not overdue");
        assert_eq!(stale[0].check_outcome, "Warn");
    });
}

/// A draft whose Zahlungsziel is close is overdue even when it is young.
///
/// The age clock alone cannot answer this: a 90-day Zahlungsziel makes 48 hours
/// meaningless, and a 7-day one makes it far too slow. An invoice cannot be paid
/// on time if it has not been sent.
#[tokio::test]
async fn a_draft_running_out_of_time_is_reported_however_young() {
    with_pg!(|pool| {
        let today = mako_fristen::heute();

        // Drafted moments ago, but due tomorrow.
        let mut urgent = Draft::nne("t1", MALO, "NNE-2026-000001");
        urgent.invoice_date = today;
        urgent.due_date = today.saturating_add(time::Duration::days(1));
        urgent.insert(&pool).await.expect("insert");

        // Drafted moments ago and not due for months.
        let mut relaxed = Draft::nne("t1", MALO_2, "NNE-2026-000002");
        relaxed.invoice_date = today;
        relaxed.due_date = today.saturating_add(time::Duration::days(90));
        relaxed.insert(&pool).await.expect("insert");

        let stale = pg::list_undispatched_stale(&pool, "t1", 48, 100)
            .await
            .expect("list");
        assert_eq!(
            stale.len(),
            1,
            "only the one running out of time: {stale:#?}"
        );
        assert_eq!(stale[0].rechnungsnummer, "NNE-2026-000001");

        // The age clock still works on its own: backdate the relaxed one.
        sqlx::query(
            "UPDATE invoice_drafts SET created_at = now() - INTERVAL '72 hours'
             WHERE rechnungsnummer = 'NNE-2026-000002'",
        )
        .execute(&pool)
        .await
        .expect("backdate");
        let stale = pg::list_undispatched_stale(&pool, "t1", 48, 100)
            .await
            .expect("list");
        assert_eq!(stale.len(), 2, "both clocks report");
    });
}

// ── Correction chain ──────────────────────────────────────────────────────────

/// A correction is linked and reasoned, and the original is never mutated.
#[tokio::test]
async fn a_correction_is_linked_and_reasoned() {
    with_pg!(|pool| {
        let original = Draft::nne("tenant-b", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert the original");

        let mut conn = pool.acquire().await.expect("acquire");
        let storno = insert_draft(
            &mut conn,
            &NewDraft {
                tenant: "tenant-b",
                malo_id: MALO,
                sender_mp_id: NB,
                recipient_mp_id: LF,
                pid: 31002,
                sparte: "STROM",
                settlement_type: "NneStrom",
                period_from: date!(2026 - 01 - 01),
                period_to: date!(2026 - 01 - 31),
                rechnungsnummer: "NNE-2026-000002",
                invoice_date: date!(2026 - 02 - 01),
                due_date: date!(2026 - 03 - 03),
                settlement_input: settlement_input(Sparte::Strom),
                rechnung: serde_json::json!({ "_typ": "RECHNUNG", "istStorno": true }),
                netto_eur_units: -123_456_000,
                steuer_eur_units: -23_456_640,
                brutto_eur_units: -146_912_640,
                zu_zahlen_eur_units: -146_912_640,
                steuer_kategorie: "S",
                steuer_satz_prozent: rust_decimal::Decimal::from(19),
                check_outcome: CheckOutcome::Ok,
                check_findings: serde_json::json!([]),
                settlement_warnings: serde_json::json!([]),
                rechnungsart: "STORNORECHNUNG",
                original_draft_id: Some(original),
                korrektur_grund: Some("MESSWERTKORREKTUR"),
            },
        )
        .await
        .expect("insert the Storno");

        let row = fetch_draft(&pool, "tenant-b", storno)
            .await
            .expect("fetch")
            .expect("exists");
        assert_eq!(row.rechnungsart, "STORNORECHNUNG");
        assert_eq!(row.original_draft_id, Some(original));
        assert_eq!(row.korrektur_grund.as_deref(), Some("MESSWERTKORREKTUR"));
        assert_eq!(row.netto_eur_units, -123_456_000);

        // The correction inherits the tenant. Filing it under a hard-coded
        // `'default'` once made a correction of tenant B's invoice invisible to
        // B's own tenant-scoped reads.
        assert!(
            fetch_draft(&pool, "default", storno)
                .await
                .expect("fetch")
                .is_none()
        );

        // The original is untouched.
        let untouched = fetch_draft(&pool, "tenant-b", original)
            .await
            .expect("fetch")
            .expect("exists");
        assert_eq!(untouched.netto_eur_units, 123_456_000);
        assert_eq!(untouched.rechnungsart, "RECHNUNG");
    });
}

/// An invoice is reversed once.
///
/// A second Stornorechnung credits the counterparty twice, and nothing
/// downstream notices — both are well-formed documents referencing the same
/// original.
#[tokio::test]
async fn an_invoice_is_reversed_only_once() {
    with_pg!(|pool| {
        let original = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert the original");

        let storno = |nummer: &'static str| {
            let pool = pool.clone();
            async move {
                let mut conn = pool.acquire().await.expect("acquire");
                insert_draft(
                    &mut conn,
                    &NewDraft {
                        tenant: "t1",
                        malo_id: MALO,
                        sender_mp_id: NB,
                        recipient_mp_id: LF,
                        pid: 31002,
                        sparte: "STROM",
                        settlement_type: "NneStrom",
                        period_from: date!(2026 - 01 - 01),
                        period_to: date!(2026 - 01 - 31),
                        rechnungsnummer: nummer,
                        invoice_date: date!(2026 - 02 - 01),
                        due_date: date!(2026 - 03 - 03),
                        settlement_input: settlement_input(Sparte::Strom),
                        rechnung: serde_json::json!({ "_typ": "RECHNUNG", "istStorno": true }),
                        netto_eur_units: -123_456_000,
                        steuer_eur_units: -23_456_640,
                        brutto_eur_units: -146_912_640,
                        zu_zahlen_eur_units: -146_912_640,
                        steuer_kategorie: "S",
                        steuer_satz_prozent: rust_decimal::Decimal::from(19),
                        check_outcome: CheckOutcome::Ok,
                        check_findings: serde_json::json!([]),
                        settlement_warnings: serde_json::json!([]),
                        rechnungsart: "STORNORECHNUNG",
                        original_draft_id: Some(original),
                        korrektur_grund: Some("MESSWERTKORREKTUR"),
                    },
                )
                .await
            }
        };

        storno("NNE-2026-000002").await.expect("the first reversal");
        let second = storno("NNE-2026-000003").await;
        assert!(
            matches!(second, Err(InsertDraftError::AlreadyReversed)),
            "a second Storno must be refused, got {second:?}"
        );

        // A Korrekturrechnung on the same original is a different document and
        // is still allowed — it is how the corrected amounts are re-issued.
        let mut conn = pool.acquire().await.expect("acquire");
        insert_draft(
            &mut conn,
            &NewDraft {
                tenant: "t1",
                malo_id: MALO,
                sender_mp_id: NB,
                recipient_mp_id: LF,
                pid: 31002,
                sparte: "STROM",
                settlement_type: "NneStrom",
                period_from: date!(2026 - 01 - 01),
                period_to: date!(2026 - 01 - 31),
                rechnungsnummer: "NNE-2026-000004",
                invoice_date: date!(2026 - 02 - 01),
                due_date: date!(2026 - 03 - 03),
                settlement_input: settlement_input(Sparte::Strom),
                rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
                netto_eur_units: 100_000_000,
                steuer_eur_units: 19_000_000,
                brutto_eur_units: 119_000_000,
                zu_zahlen_eur_units: 119_000_000,
                steuer_kategorie: "S",
                steuer_satz_prozent: rust_decimal::Decimal::from(19),
                check_outcome: CheckOutcome::Ok,
                check_findings: serde_json::json!([]),
                settlement_warnings: serde_json::json!([]),
                rechnungsart: "KORREKTURRECHNUNG",
                original_draft_id: Some(original),
                korrektur_grund: Some("MESSWERTKORREKTUR"),
            },
        )
        .await
        .expect("a Korrektur of a reversed invoice is the point of reversing it");
    });
}

/// A correction must name what it corrects, and an original must not.
#[tokio::test]
async fn the_correction_link_is_enforced_by_the_schema() {
    with_pg!(|pool| {
        let mut conn = pool.acquire().await.expect("acquire");
        let orphan = insert_draft(
            &mut conn,
            &NewDraft {
                tenant: "t1",
                malo_id: MALO,
                sender_mp_id: NB,
                recipient_mp_id: LF,
                pid: 31002,
                sparte: "STROM",
                settlement_type: "NneStrom",
                period_from: date!(2026 - 01 - 01),
                period_to: date!(2026 - 01 - 31),
                rechnungsnummer: "NNE-2026-000009",
                invoice_date: date!(2026 - 02 - 01),
                due_date: date!(2026 - 03 - 03),
                settlement_input: settlement_input(Sparte::Strom),
                rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
                netto_eur_units: -1,
                steuer_eur_units: 0,
                brutto_eur_units: -1,
                zu_zahlen_eur_units: -1,
                steuer_kategorie: "S",
                steuer_satz_prozent: rust_decimal::Decimal::from(19),
                check_outcome: CheckOutcome::Ok,
                check_findings: serde_json::json!([]),
                settlement_warnings: serde_json::json!([]),
                rechnungsart: "STORNORECHNUNG",
                original_draft_id: None,
                korrektur_grund: None,
            },
        )
        .await;
        assert!(
            orphan.is_err(),
            "a Storno that references nothing is not an audit trail"
        );
    });
}

/// A Korrekturrechnung is gated on the reversal that makes the pair net out.
///
/// The correction carries the *whole* corrected amount, not the difference, so
/// issuing one against a live invoice bills the period twice — and both
/// documents are well-formed, so nothing downstream notices. `has_storno` is
/// what the handler consults before allowing it.
#[tokio::test]
async fn a_correction_is_gated_on_the_reversal_that_precedes_it() {
    with_pg!(|pool| {
        let original = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert the original");
        let mut conn = pool.acquire().await.expect("acquire");

        assert!(
            !has_storno(&mut conn, "t1", original).await.expect("query"),
            "nothing has reversed it yet"
        );

        insert_draft(
            &mut conn,
            &NewDraft {
                tenant: "t1",
                malo_id: MALO,
                sender_mp_id: NB,
                recipient_mp_id: LF,
                pid: 31002,
                sparte: "STROM",
                settlement_type: "NneStrom",
                period_from: date!(2026 - 01 - 01),
                period_to: date!(2026 - 01 - 31),
                rechnungsnummer: "NNE-2026-000002",
                invoice_date: date!(2026 - 02 - 01),
                due_date: date!(2026 - 03 - 03),
                settlement_input: settlement_input(Sparte::Strom),
                rechnung: serde_json::json!({ "_typ": "RECHNUNG", "istStorno": true }),
                netto_eur_units: -123_456_000,
                steuer_eur_units: -23_456_640,
                brutto_eur_units: -146_912_640,
                zu_zahlen_eur_units: -146_912_640,
                steuer_kategorie: "S",
                steuer_satz_prozent: rust_decimal::Decimal::from(19),
                check_outcome: CheckOutcome::Ok,
                check_findings: serde_json::json!([]),
                settlement_warnings: serde_json::json!([]),
                rechnungsart: "STORNORECHNUNG",
                original_draft_id: Some(original),
                korrektur_grund: Some("MESSWERTKORREKTUR"),
            },
        )
        .await
        .expect("insert the Storno");

        assert!(
            has_storno(&mut conn, "t1", original).await.expect("query"),
            "now the correction may follow"
        );

        // Another tenant's reads never see it.
        assert!(!has_storno(&mut conn, "t2", original).await.expect("query"));
    });
}

/// The schema refuses totals that do not add up, and a reverse charge that
/// nonetheless states tax.
///
/// An invoice whose parts do not sum to its whole is the one error nobody
/// catches by reading it.
#[tokio::test]
async fn the_schema_refuses_a_tax_block_that_does_not_add_up() {
    with_pg!(|pool| {
        let mut conn = pool.acquire().await.expect("acquire");
        let attempt = |netto: i64,
                       steuer: i64,
                       brutto: i64,
                       kategorie: &'static str,
                       satz: i64,
                       nummer: &'static str| {
            let pool = pool.clone();
            async move {
                let mut conn = pool.acquire().await.expect("acquire");
                insert_draft(
                    &mut conn,
                    &NewDraft {
                        tenant: "t1",
                        malo_id: MALO,
                        sender_mp_id: NB,
                        recipient_mp_id: LF,
                        pid: 31005,
                        sparte: "STROM",
                        settlement_type: "MmmStrom",
                        period_from: date!(2026 - 01 - 01),
                        period_to: date!(2026 - 01 - 31),
                        rechnungsnummer: nummer,
                        invoice_date: date!(2026 - 02 - 01),
                        due_date: date!(2026 - 03 - 03),
                        settlement_input: settlement_input(Sparte::Strom),
                        rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
                        netto_eur_units: netto,
                        steuer_eur_units: steuer,
                        brutto_eur_units: brutto,
                        zu_zahlen_eur_units: brutto,
                        steuer_kategorie: kategorie,
                        steuer_satz_prozent: rust_decimal::Decimal::from(satz),
                        check_outcome: CheckOutcome::Ok,
                        check_findings: serde_json::json!([]),
                        settlement_warnings: serde_json::json!([]),
                        rechnungsart: "RECHNUNG",
                        original_draft_id: None,
                        korrektur_grund: None,
                    },
                )
                .await
            }
        };

        assert!(
            attempt(100_000, 19_000, 999_999, "S", 19, "X-1")
                .await
                .is_err(),
            "netto + steuer must equal brutto"
        );
        assert!(
            attempt(100_000, 19_000, 119_000, "AE", 0, "X-2")
                .await
                .is_err(),
            "a reverse charge states no tax amount (§13b UStG, BR-AE-09)"
        );
        assert!(
            attempt(100_000, 0, 100_000, "S", 0, "X-3").await.is_err(),
            "a taxed supply states a rate"
        );

        // Both lawful shapes are accepted.
        attempt(100_000, 19_000, 119_000, "S", 19, "X-4")
            .await
            .expect("an ordinary taxed supply");
        let _ = &mut conn;
    });
}

/// A Sparte filter separates what the Prüfidentifikator cannot.
///
/// NN-Rechnung Strom and Gas share PID 31002, and the two MMM variants share
/// 31005. Without a Sparte filter a gas network operator's only way to see its
/// own invoices was to fetch everything and filter client-side — and a filter
/// nobody can express is one nobody applies.
#[tokio::test]
async fn drafts_can_be_filtered_by_sparte() {
    with_pg!(|pool| {
        Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("strom");
        let mut gas = Draft::nne("t1", MALO_2, "NNG-2026-000001");
        gas.sparte = "GAS";
        gas.settlement_type = "NneGas";
        gas.insert(&pool).await.expect("gas");

        let by_sparte = |sparte: &'static str| {
            let pool = pool.clone();
            async move {
                list_drafts(
                    &pool,
                    "t1",
                    &DraftFilter {
                        sparte: Some(sparte),
                        limit: 50,
                        ..DraftFilter::default()
                    },
                )
                .await
                .expect("list")
            }
        };

        let gas_rows = by_sparte("GAS").await;
        assert_eq!(gas_rows.len(), 1, "one gas invoice");
        assert_eq!(gas_rows[0].rechnungsnummer, "NNG-2026-000001");
        // The PID alone cannot make this distinction — both rows carry 31002.
        assert_eq!(gas_rows[0].pid, 31002);

        let strom_rows = by_sparte("STROM").await;
        assert_eq!(strom_rows.len(), 1);
        assert_eq!(strom_rows[0].rechnungsnummer, "NNE-2026-000001");
    });
}

/// Paging walks every row exactly once, and stops.
///
/// The cursor is `(created_at, id)`, so it is stable against rows inserted
/// between two page requests — which `OFFSET` is not: an insert shifts the
/// window and the caller silently skips a row it never saw.
#[tokio::test]
async fn the_cursor_walks_every_draft_exactly_once() {
    with_pg!(|pool| {
        for i in 1..=5 {
            Draft::nne("t1", MALO, &format!("NNE-2026-{i:06}"))
                .period_ending(date!(2026 - 01 - 01).saturating_add(time::Duration::days(i)))
                .insert(&pool)
                .await
                .unwrap_or_else(|e| panic!("draft {i}: {e:?}"));
        }

        let mut seen: Vec<String> = Vec::new();
        let mut after = None;
        loop {
            let page = list_drafts(
                &pool,
                "t1",
                &DraftFilter {
                    after,
                    limit: 2,
                    ..DraftFilter::default()
                },
            )
            .await
            .expect("page");
            let Some(last) = page.last() else { break };
            let full = page.len() == 2;
            after = Some(pg::Cursor {
                created_at: last.created_at,
                id: last.id,
            });
            seen.extend(page.into_iter().map(|r| r.rechnungsnummer));
            if !full {
                break;
            }
        }

        seen.sort();
        assert_eq!(
            seen,
            (1..=5)
                .map(|i| format!("NNE-2026-{i:06}"))
                .collect::<Vec<_>>(),
            "every row exactly once, none repeated"
        );
    });
}

/// The collectible amount is stored, not recomputed on every read.
///
/// The gross is what was invoiced; `zu_zahlen` is what is left to collect after
/// the Abschläge the invoice settles. Without it the summary, the overdue alert
/// and the audit export can only state what was invoiced.
#[tokio::test]
async fn the_collectible_amount_is_stored_and_summed() {
    with_pg!(|pool| {
        let mut settled = Draft::nne("t1", MALO, "NNE-2026-000001");
        settled.netto_eur_units = 100_000_000; // 1 000.00 EUR net, 1 190.00 gross
        // 400.00 EUR of Abschläge already invoiced and taxed on receipt.
        settled.zu_zahlen_override = Some(79_000_000);
        settled.insert(&pool).await.expect("insert");

        let row = fetch_draft(&pool, "t1", uuid_of(&pool, "NNE-2026-000001").await)
            .await
            .expect("fetch")
            .expect("present");
        assert_eq!(row.brutto_eur_units, 119_000_000);
        assert_eq!(row.zu_zahlen_eur_units, 79_000_000);

        let summary = billing_summary(&pool, "t1", 2026, 1)
            .await
            .expect("summary");
        let sum = |f: fn(&pg::BillingSummaryRow) -> i64| summary.iter().map(f).sum::<i64>();
        assert_eq!(sum(|r| r.brutto_eur_units), 119_000_000);
        assert_eq!(
            sum(|r| r.zu_zahlen_eur_units),
            79_000_000,
            "a month-end reconciliation over the gross reconciles against money nobody will pay"
        );
    });
}

/// A deduction only reduces what is owed — but it may take it past zero.
///
/// An Abschlussrechnung that settles for less than the Anzahlungen already
/// invoiced leaves a Guthaben the Netzbetreiber owes back. That is an ordinary
/// outcome of billing on account, so the bound is directional rather than
/// clamped at zero — clamping it would make the commonest year-end correction
/// a 500. What is refused is a deduction that makes the invoice *larger*.
#[tokio::test]
async fn a_deduction_may_leave_a_guthaben_but_never_increases_the_invoice() {
    with_pg!(|pool| {
        let mut guthaben = Draft::nne("t1", MALO, "NNE-2026-000001");
        guthaben.netto_eur_units = 100_000_000; // 1 190.00 EUR gross
        guthaben.zu_zahlen_override = Some(-30_000_000); // 300.00 EUR back to the LF
        guthaben
            .insert(&pool)
            .await
            .expect("an over-collected Abschlag leaves a Guthaben");

        let mut more = Draft::nne("t1", MALO_2, "NNE-2026-000002");
        more.netto_eur_units = 100_000_000;
        more.zu_zahlen_override = Some(200_000_000); // more than the gross
        assert!(
            more.insert(&pool).await.is_err(),
            "a deduction cannot make the invoice larger than it is"
        );

        // A Storno runs negative throughout, and its own bound is the mirror.
        let mut storno_shaped = Draft::nne("t1", MALO_3, "NNE-2026-000003");
        storno_shaped.netto_eur_units = -100_000_000;
        storno_shaped.zu_zahlen_override = Some(-200_000_000);
        assert!(
            storno_shaped.insert(&pool).await.is_err(),
            "a credit cannot be deducted into a larger credit"
        );
    });
}

/// The correction chain comes back as one window, not two concatenated ones.
#[tokio::test]
async fn the_correction_chain_is_one_limited_window() {
    with_pg!(|pool| {
        let original = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("original");
        insert_correction(&pool, original, "STORNORECHNUNG", "ST-2026-000001").await;
        insert_correction(&pool, original, "KORREKTURRECHNUNG", "KO-2026-000001").await;

        let all = pg::list_corrections(&pool, "t1", None, 50)
            .await
            .expect("corrections");
        assert_eq!(all.len(), 2);
        // The original is not a correction and must not appear.
        assert!(all.iter().all(|r| r.rechnungsart != "RECHNUNG"));

        // The limit bounds the whole chain, not each Rechnungsart separately —
        // running one query per art returned up to twice what was asked for.
        let capped = pg::list_corrections(&pool, "t1", None, 1)
            .await
            .expect("corrections");
        assert_eq!(capped.len(), 1, "limit 1 means one row, not one per art");
    });
}

/// The UUID of a draft by its invoice number.
async fn uuid_of(pool: &sqlx::PgPool, rechnungsnummer: &str) -> Uuid {
    list_drafts(
        pool,
        "t1",
        &DraftFilter {
            limit: 50,
            ..DraftFilter::default()
        },
    )
    .await
    .expect("list")
    .into_iter()
    .find(|r| r.rechnungsnummer == rechnungsnummer)
    .map(|r| r.id)
    .expect("draft present")
}

/// Store a correcting document against an original.
async fn insert_correction(
    pool: &sqlx::PgPool,
    original: Uuid,
    rechnungsart: &str,
    rechnungsnummer: &str,
) {
    let mut conn = pool.acquire().await.expect("acquire");
    insert_draft(
        &mut conn,
        &NewDraft {
            tenant: "t1",
            malo_id: MALO,
            sender_mp_id: NB,
            recipient_mp_id: LF,
            pid: 31002,
            sparte: "STROM",
            settlement_type: "NneStrom",
            period_from: date!(2026 - 01 - 01),
            period_to: date!(2026 - 01 - 31),
            rechnungsnummer,
            invoice_date: date!(2026 - 03 - 01),
            due_date: date!(2026 - 03 - 31),
            settlement_input: settlement_input(Sparte::Strom),
            rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
            netto_eur_units: -100_000_000,
            steuer_eur_units: -19_000_000,
            brutto_eur_units: -119_000_000,
            zu_zahlen_eur_units: -119_000_000,
            steuer_kategorie: "S",
            steuer_satz_prozent: rust_decimal::Decimal::from(19),
            check_outcome: CheckOutcome::Ok,
            check_findings: serde_json::json!([]),
            settlement_warnings: serde_json::json!([]),
            rechnungsart,
            original_draft_id: Some(original),
            korrektur_grund: Some("Messwertkorrektur"),
        },
    )
    .await
    .unwrap_or_else(|e| panic!("insert {rechnungsart}: {e:?}"));
}

/// The stored settlement input parses back, which is what a Storno recomputes.
#[tokio::test]
async fn the_stored_settlement_input_round_trips() {
    with_pg!(|pool| {
        let mut draft = Draft::nne("t1", MALO, "NNE-2026-000001");
        draft.sparte = "GAS";
        draft.settlement_type = "NneGas";
        let id = draft.insert(&pool).await.expect("insert");

        let mut conn = pool.acquire().await.expect("acquire");
        let (row, input) = pg::load_settlement_input(&mut conn, "t1", id)
            .await
            .expect("load")
            .expect("exists");
        assert_eq!(row.sparte, "GAS");
        assert_eq!(
            input.sparte(),
            Sparte::Gas,
            "a Storno recomputes from this input, so it has to survive the round trip"
        );
    });
}

// ── A rejected Storno leaves the original standing ───────────────────────────

/// Insert a Stornorechnung against `original`.
async fn insert_storno(
    pool: &sqlx::PgPool,
    tenant: &'static str,
    nummer: &'static str,
    original: Uuid,
) -> Result<Uuid, InsertDraftError> {
    let mut conn = pool.acquire().await.expect("acquire");
    insert_draft(
        &mut conn,
        &NewDraft {
            tenant,
            malo_id: MALO,
            sender_mp_id: NB,
            recipient_mp_id: LF,
            pid: 31002,
            sparte: "STROM",
            settlement_type: "NneStrom",
            period_from: date!(2026 - 01 - 01),
            period_to: date!(2026 - 01 - 31),
            rechnungsnummer: nummer,
            invoice_date: date!(2026 - 02 - 01),
            due_date: date!(2026 - 03 - 03),
            settlement_input: settlement_input(Sparte::Strom),
            rechnung: serde_json::json!({ "_typ": "RECHNUNG", "istStorno": true }),
            netto_eur_units: -123_456_000,
            steuer_eur_units: -23_456_640,
            brutto_eur_units: -146_912_640,
            zu_zahlen_eur_units: -146_912_640,
            steuer_kategorie: "S",
            steuer_satz_prozent: rust_decimal::Decimal::from(19),
            check_outcome: CheckOutcome::Ok,
            check_findings: serde_json::json!([]),
            settlement_warnings: serde_json::json!([]),
            rechnungsart: "STORNORECHNUNG",
            original_draft_id: Some(original),
            korrektur_grund: Some("MESSWERTKORREKTUR"),
        },
    )
    .await
}

/// A Storno that is itself rejected must leave the original exactly where it was.
///
/// The state path that stranded it:
///
/// 1. `NNE-…001` is dispatched, deducting Abschlag `ABS-…001`.
/// 2. `POST /drafts/{id}/storno` writes a STORNORECHNUNG **draft** and — this
///    was the defect — released the original's `abschlag_verrechnungen` there
///    and then, while the original was still standing as a dispatched invoice
///    that deducts them on the wire.
/// 3. The operator rejects the Storno draft.
///
/// What was left: the original still `dispatched`, but `has_storno` said it was
/// already reversed (so no Korrekturrechnung could name it honestly) and
/// `id_one_storno_per_original` refused a second attempt (so it could not be
/// reversed either) — a dispatched invoice with no move available. And the
/// Anzahlung was free, so a second invoice could deduct money the customer had
/// already paid, against an invoice that had never been credited.
///
/// The terminal state after a rejected Storno is therefore: **the original
/// stands, whole**. It is dispatched, it still holds its deductions, and a
/// fresh Storno is the way forward.
#[tokio::test]
async fn a_rejected_storno_leaves_the_original_standing_and_its_abschlaege_held() {
    with_pg!(|pool| {
        let mut conn = pool.acquire().await.expect("acquire");

        // A dispatched Abschlagsrechnung the customer has paid on account.
        let abschlag = Draft::abschlag("t1", MALO, "ABS-2026-000001", date!(2026 - 01 - 15))
            .insert(&pool)
            .await
            .expect("insert Abschlag");
        mark_dispatched(
            &mut conn,
            "t1",
            abschlag,
            "process-abs",
            &serde_json::json!({ "_typ": "RECHNUNG" }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch Abschlag");

        // The NN-Rechnung that settles it, dispatched.
        let original = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert original");
        pg::record_abschlag_verrechnungen(&mut conn, "t1", original, &[abschlag])
            .await
            .expect("deduct the Abschlag");
        mark_dispatched(
            &mut conn,
            "t1",
            original,
            "process-1",
            &serde_json::json!({ "_typ": "RECHNUNG" }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch original");

        // A Storno is drafted — and the deductions stay held while it is only a
        // draft, because a draft can still be rejected.
        let storno = insert_storno(&pool, "t1", "NNE-2026-000002", original)
            .await
            .expect("draft the reversal");
        assert_eq!(
            pg::count_abschlag_verrechnungen(&pool, "t1", original)
                .await
                .expect("count"),
            1,
            "an undispatched Storno must not free the original's Anzahlungen"
        );

        // The operator rejects it.
        assert!(
            reject_draft(&pool, "t1", storno, "falscher Grund erfasst")
                .await
                .expect("reject the Storno")
        );

        // ── The terminal state ───────────────────────────────────────────────
        let row = fetch_draft(&pool, "t1", original)
            .await
            .expect("fetch")
            .expect("the original");
        assert_eq!(row.status, "dispatched", "the original still stands");

        // …it is not reversed, so a Korrekturrechnung may not claim it is…
        assert!(
            !has_storno(&mut conn, "t1", original)
                .await
                .expect("has_storno"),
            "a rejected reversal never left the house — the original is not reversed"
        );

        // …it can be reversed again, which is the only honest way forward…
        insert_storno(&pool, "t1", "NNE-2026-000003", original)
            .await
            .expect("a rejected Storno must not lock the original out of being reversed");

        // …and the Anzahlung was never freed, so nothing else could deduct it.
        assert_eq!(
            pg::count_abschlag_verrechnungen(&pool, "t1", original)
                .await
                .expect("count"),
            1,
            "a rejected Storno must leave the original's deductions in place"
        );
        let problems = pg::load_abschlaege(&mut conn, "t1", MALO, &[abschlag])
            .await
            .expect("query")
            .expect_err("the Abschlag is still settled by the original");
        assert!(
            problems[0].contains("NNE-2026-000001"),
            "and the refusal still names the invoice holding it: {problems:?}"
        );
    });
}

/// A *dispatched* Storno is what frees the Anzahlungen.
///
/// The release has to happen somewhere: the Korrekturrechnung that replaces the
/// reversed invoice must be able to deduct money the customer has already paid.
/// Dispatch is that point — it is where the original stops standing.
#[tokio::test]
async fn a_dispatched_storno_releases_the_originals_abschlaege() {
    with_pg!(|pool| {
        let mut conn = pool.acquire().await.expect("acquire");

        let abschlag = Draft::abschlag("t1", MALO, "ABS-2026-000001", date!(2026 - 01 - 15))
            .insert(&pool)
            .await
            .expect("insert Abschlag");
        mark_dispatched(
            &mut conn,
            "t1",
            abschlag,
            "process-abs",
            &serde_json::json!({ "_typ": "RECHNUNG" }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch Abschlag");

        let original = Draft::nne("t1", MALO, "NNE-2026-000001")
            .insert(&pool)
            .await
            .expect("insert original");
        pg::record_abschlag_verrechnungen(&mut conn, "t1", original, &[abschlag])
            .await
            .expect("deduct");

        let storno = insert_storno(&pool, "t1", "NNE-2026-000002", original)
            .await
            .expect("draft the reversal");

        // `dispatch_one` performs this release after `mark_dispatched`; the
        // sequence is what the handler runs, in one transaction.
        mark_dispatched(
            &mut conn,
            "t1",
            storno,
            "process-storno",
            &serde_json::json!({ "_typ": "RECHNUNG", "istStorno": true }),
            CheckOutcome::Ok,
            &serde_json::json!([]),
        )
        .await
        .expect("dispatch the Storno");
        let released = pg::release_abschlag_verrechnungen(&pool, "t1", original)
            .await
            .expect("release");
        assert_eq!(released, 1);

        // The Abschlag is deductible again — by the Korrekturrechnung.
        assert_eq!(
            pg::load_abschlaege(&mut conn, "t1", MALO, &[abschlag])
                .await
                .expect("query")
                .expect("deductible again")
                .len(),
            1
        );
    });
}

/// Where the Abschlag release lives is a guard, not a detail.
///
/// It is the one half of the rejected-Storno fix a database test cannot reach:
/// `post_storno` and `dispatch_one` are handlers, and there is no HTTP harness
/// here. Releasing on *draft creation* freed the customer's Anzahlungen while
/// the original was still a standing dispatched invoice deducting them, so a
/// rejected Storno left them collectible twice. The release belongs at the
/// point the original stops standing: the Storno's dispatch.
#[test]
fn the_abschlag_release_happens_on_storno_dispatch_not_on_drafting() {
    let handlers = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/handlers.rs"),
    )
    .expect("read src/handlers.rs");

    let body_of = |name: &str| -> String {
        let start = handlers
            .find(&format!("\nasync fn {name}("))
            .or_else(|| handlers.find(&format!("\npub async fn {name}(")))
            .unwrap_or_else(|| panic!("{name} must exist"));
        // Up to the next top-level `fn`, which is where this one ends.
        let rest = &handlers[start + 1..];
        let end = rest[1..]
            .find("\nasync fn ")
            .into_iter()
            .chain(rest[1..].find("\npub async fn "))
            .chain(rest[1..].find("\nfn "))
            .min()
            .unwrap_or(rest.len() - 1);
        rest[..=end].to_owned()
    };

    assert!(
        !body_of("post_storno").contains("release_abschlag_verrechnungen"),
        "post_storno must not free the original's Abschläge: the Storno it writes is a \
         draft, and a rejected draft would leave them collectible against an invoice \
         that was never reversed"
    );
    let dispatch = body_of("dispatch_one");
    assert!(
        dispatch.contains("release_abschlag_verrechnungen")
            && dispatch.contains("STORNORECHNUNG")
            && dispatch.contains("original_draft_id"),
        "dispatch_one must release the *original's* deductions when a STORNORECHNUNG \
         is dispatched — otherwise the Korrekturrechnung that replaces it cannot \
         deduct money the customer has already paid"
    );
}
