//! Daily contract-lifecycle workers.
//!
//! Three obligations run on a clock rather than on a request:
//!
//! | Worker | Obligation |
//! |---|---|
//! | [`preisanpassung`] | Send the § 41 Abs. 5 EnWG price-change notice for a scheduled Tarifwechsel |
//! | [`auto_renewal`] | Announce an automatic extension, then apply it in the shape § 309 Nr. 9 lit. b BGB permits |
//! | [`ablauf`] | Close supply that has run out, and announce a term or price guarantee about to |
//!
//! Every notice goes through the **CloudEvent outbox**, not a direct webhook
//! call. A notice the supplier owes must not depend on the ERP being reachable
//! at the moment the worker happens to run: it is persisted, then delivered
//! with retry and a dead-letter.
//!
//! A notice is recorded as sent only once one exists. Where that cannot be
//! achieved, the reason is written to the slice and the next run tries again —
//! the obligation stays open rather than being closed by a log line.
//!
//! Each worker is idempotent per contract and per date, so a missed day is
//! caught up rather than skipped, and a repeated day does not resend.
//!
//! **One replica runs a worker per cycle.** Idempotency serialises repeats of
//! the *same* run; it does nothing about two replicas reading the same unmarked
//! slice in the same second, which is what a two-instance deployment does every
//! night: both build the § 41 Abs. 5 EnWG notice, both enqueue it, and the
//! customer gets it twice. Each worker therefore takes its own session-level
//! PostgreSQL advisory lock ([`pg::try_worker_lock`], re-exported from the
//! shared `mako_service::worker_lock` that `accountingd`'s money workers use
//! too — the keys are per service, so the two never contend) and skips the
//! cycle when another instance holds it. The idempotency marks stay — the lock
//! is the first line, not the only one.

use std::sync::Arc;

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::{
    config::VertragdConfig,
    domain::{self, Verlaengerung, Vertragsart},
    events::build_cloud_event,
    pg,
};

/// 23 hours rather than 24: the loop then drifts earlier instead of skipping a
/// calendar day whenever a run is delayed, and it is DST-safe either way.
const DAILY: std::time::Duration = std::time::Duration::from_secs(23 * 3600);

/// Spawn the lifecycle workers. Each stops when `shutdown` fires.
pub fn spawn_all(pool: PgPool, cfg: Arc<VertragdConfig>, shutdown: CancellationToken) {
    for (delay, worker) in [
        (15u64, Worker::Preisanpassung),
        (30, Worker::AutoRenewal),
        (45, Worker::Ablauf),
    ] {
        let pool = pool.clone();
        let cfg = Arc::clone(&cfg);
        let shutdown = shutdown.clone();
        let name = worker.name();
        tokio::spawn(async move {
            // Stagger the first run so three workers do not hit the pool at once.
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
            }
            loop {
                match worker.run_once_locked(&pool, &cfg).await {
                    Ok(true) => {}
                    Ok(false) => tracing::debug!(
                        worker = name,
                        "vertragd: another replica holds this worker's lock — cycle skipped"
                    ),
                    Err(e) => {
                        tracing::error!(worker = name, error = %e, "vertragd: lifecycle worker failed");
                    }
                }
                tokio::select! {
                    () = shutdown.cancelled() => {
                        tracing::info!(worker = name, "vertragd: lifecycle worker stopping");
                        return;
                    }
                    () = tokio::time::sleep(DAILY) => {}
                }
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Worker {
    Preisanpassung,
    AutoRenewal,
    Ablauf,
}

impl Worker {
    /// The log label — also the name in the operator's dashboards.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Preisanpassung => "preisanpassung",
            Self::AutoRenewal => "auto-renewal",
            Self::Ablauf => "ablauf",
        }
    }

    /// This worker's advisory-lock key. Distinct per worker, so the three do
    /// not serialise against one another.
    #[must_use]
    pub const fn lock_key(self) -> i64 {
        match self {
            Self::Preisanpassung => pg::LOCK_PREISANPASSUNG,
            Self::AutoRenewal => pg::LOCK_AUTO_RENEWAL,
            Self::Ablauf => pg::LOCK_ABLAUF,
        }
    }

    /// Every lifecycle worker, in spawn order — the list the leader-election
    /// guard enumerates so a fourth worker cannot be added lock-less.
    #[must_use]
    pub const fn all() -> [Self; 3] {
        [Self::Preisanpassung, Self::AutoRenewal, Self::Ablauf]
    }

    /// Run one cycle **if** this instance wins the worker's advisory lock.
    ///
    /// `Ok(false)` means another replica is running it; that is the ordinary
    /// outcome on every instance but one, so it is not an error.
    ///
    /// # Errors
    ///
    /// Propagates whatever the cycle itself failed with. The lock is released
    /// either way — a failed run must not lock the worker out until the pod
    /// restarts.
    pub async fn run_once_locked(
        self,
        pool: &PgPool,
        cfg: &VertragdConfig,
    ) -> anyhow::Result<bool> {
        let key = self.lock_key();
        let Some(mut guard) = pg::try_worker_lock(pool, key).await else {
            return Ok(false);
        };
        let outcome = self.run_once(pool, cfg).await;
        pg::release_worker_lock(&mut guard, key).await;
        outcome.map(|()| true)
    }

    async fn run_once(self, pool: &PgPool, cfg: &VertragdConfig) -> anyhow::Result<()> {
        match self {
            Self::Preisanpassung => preisanpassung(pool, cfg).await,
            Self::AutoRenewal => auto_renewal(pool, cfg).await,
            Self::Ablauf => ablauf(pool, cfg).await,
        }
    }
}

// ── Preisanpassungsanzeige (§ 41 Abs. 5 EnWG) ────────────────────────────────

/// Send the § 41 Abs. 5 EnWG price-change notices still owed.
///
/// There is nothing to *apply*: a future-dated Tarifwechsel is a slice that
/// starts in the future, and on the day it starts it is the slice in force.
/// That is exactly why the notice may not be recorded as sent before one
/// exists — the change lands whatever happened here.
///
/// Only supplier-initiated slices are swept: a switch the customer asked for is
/// not an exercise of a change right, so § 41 Abs. 5 EnWG owes nothing for it.
///
/// # Errors
///
/// Propagates storage errors; one contract's failure is recorded on its slice
/// and the run continues, because one bad row must not hold up every other
/// notice.
pub async fn preisanpassung(pool: &PgPool, cfg: &VertragdConfig) -> anyhow::Result<()> {
    let today = mako_fristen::heute();
    let outputd = cfg
        .outputd_url
        .as_deref()
        .filter(|u| !u.is_empty())
        .map(|u| crate::dokumente::OutputdClient::new(u, cfg.outputd_api_key.clone()));
    let outputd = outputd.as_ref();

    // A price change that took effect without its notice is a breach that
    // nothing can repair after the fact, so it is reported every run until an
    // operator resolves it. The API refuses to create this state; a row written
    // around it still has to be visible.
    for row in pg::unangekuendigt_wirksame(pool, &cfg.tenant, today).await? {
        tracing::error!(
            komp_id = %row.komp_id, vertrag_id = %row.vertrag_id, slice_id = %row.slice_id,
            wirksam_ab = %row.wirksam_ab, versuche = row.notif_versuche,
            letzter_fehler = row.notif_letzter_fehler.as_deref().unwrap_or("—"),
            "vertragd: Preisänderung ohne Anzeige wirksam geworden — der Kunde wurde nicht \
             nach § 41 Abs. 5 Satz 1 EnWG unterrichtet und hatte kein Sonderkündigungsrecht"
        );
    }

    for row in pg::offene_preisanpassungen(pool, &cfg.tenant, today).await? {
        let regime = domain::preisanpassungsregime(
            Vertragsart::from_db(&row.vertragsart),
            row.haushaltskunde,
        );
        if let Err(e) = kuendige_preisaenderung_an(pool, cfg, outputd, &row, &regime, today).await {
            // Recorded on the slice, not only logged: the notice stays owed,
            // the next run retries it, and why it failed is answerable from the
            // data. The slice is never marked sent on this path.
            tracing::error!(
                komp_id = %row.komp_id, slice_id = %row.slice_id,
                wirksam_ab = %row.wirksam_ab, versuche = row.notif_versuche, error = %e,
                "vertragd: die § 41 Abs. 5 Preisänderungsanzeige konnte nicht erstellt werden"
            );
            if let Err(e) =
                pg::produkte::record_notif_failure(pool, row.slice_id, &e.to_string()).await
            {
                tracing::error!(slice_id = %row.slice_id, error = %e, "vertragd: could not record the failed notice attempt");
            }
        }
    }
    Ok(())
}

/// Announce one scheduled price change, and record that it was announced.
///
/// The order is the whole point. The document is issued first, and only then do
/// the CloudEvent and the `preisanpassung_notif_sent` flag commit together — so
/// the flag never claims a notice that does not exist. Re-running after a crash
/// between the two re-issues the document, which `outputd` answers with the one
/// it already recorded for this slice.
///
/// Where no `outputd` is configured the CloudEvent *is* the notice: it carries
/// the Anlass, whatever Umfang was scheduled and the Sonderkündigungsrecht, an
/// ERP composes the letter from its own price sheets, and the outbox guarantees
/// delivery.
///
/// # Errors
///
/// Every reason the notice cannot be made valid on the channel this deployment
/// uses — for a rendered document: no announced Umfang, no addressee, no
/// configured declarant, a renderer that refused. The caller records it against
/// the slice and the next run tries again.
async fn kuendige_preisaenderung_an(
    pool: &PgPool,
    cfg: &VertragdConfig,
    outputd: Option<&crate::dokumente::OutputdClient>,
    row: &pg::AnzupassenderPreis,
    regime: &domain::Preisanpassungsregime,
    today: time::Date,
) -> anyhow::Result<()> {
    use crate::dokumente::PreisPosition;

    let vorlauf_tage = (row.wirksam_ab - today).whole_days();
    let frist_gewahrt = regime.frist.gewahrt(today, row.wirksam_ab);
    if !frist_gewahrt {
        // The API refuses a Wirksamkeit this close, so reaching here means the
        // slice was written around that guard. Say so loudly — a notice that is
        // late is a breach, not a warning.
        tracing::error!(
            komp_id = %row.komp_id, vorlauf_tage, erforderlich = %regime.bezeichnung,
            rechtsgrundlage = regime.rechtsgrundlage,
            "vertragd: Preisänderungsanzeige verspätet — die gesetzliche Frist ist nicht gewahrt"
        );
    }

    // § 41 Abs. 5 Satz 3 EnWG: the notice states the Umfang of the change.
    // Whoever composes the notice owes that — which is this service where it
    // renders the document, and the ERP where the CloudEvent is the notice.
    let positionen: Vec<PreisPosition> = row
        .angekuendigte_preise
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| anyhow::anyhow!("angekuendigte_preise sind keine Preiszeilen: {e}"))?
        .unwrap_or_default();

    let document_id = match outputd {
        Some(client) => {
            // The lines are the page's content: a rendered notice that states
            // no Umfang is not a Preisänderungsanzeige, so nothing goes out and
            // nothing is marked. The API refuses such a change where this
            // deployment renders, so reaching here means the slice was written
            // around that guard.
            anyhow::ensure!(
                !positionen.is_empty(),
                "der geplante Tarifwechsel führt keine angekündigten Preise; ohne den Umfang \
                 der Änderung (§ 41 Abs. 5 Satz 3 EnWG) lässt sich keine wirksame \
                 Preisänderungsanzeige erstellen"
            );
            Some(
                issue_preisanpassung_document(pool, cfg, client, row, regime, positionen.clone())
                    .await?,
            )
        }
        None => None,
    };

    let mut tx = pool.begin().await?;
    let ce = build_cloud_event(
        mako_events::vertrag::PREISAENDERUNG_ANKUENDIGUNG,
        row.vertrag_id,
        &cfg.tenant,
        serde_json::json!({
            "vertrag_id": row.vertrag_id,
            "kunden_id": row.kunden_id,
            "komp_id": row.komp_id,
            "malo_id": row.malo_id,
            "sparte": row.sparte,
            "current_product_code": row.bisheriges_produkt,
            "new_product_code": row.neues_produkt,
            "wirksamkeit": row.wirksam_ab.to_string(),
            "vorlauf_tage": vorlauf_tage,
            "erforderliche_frist": regime.bezeichnung,
            "fruehestens_wirksam": regime.fruehestens_wirksam(today).to_string(),
            "frist_gewahrt": frist_gewahrt,
            "rechtsgrundlage": regime.rechtsgrundlage,
            "dokument_id": document_id,
            // § 41 Abs. 5 Satz 3 EnWG: the notice states Anlass and Umfang, so
            // a recipient that composes its own letter has both. `umfang` is
            // empty where the change was scheduled without price lines, and
            // `umfang_vollstaendig` says so: the composer holds the price
            // sheets and states the Umfang from them, and a letter that states
            // none is not a valid notice on any channel.
            "anlass": row.grund,
            "umfang": positionen,
            "umfang_vollstaendig": !positionen.is_empty(),
            // § 41 Abs. 5 Satz 1 EnWG obliges the supplier to state the
            // customer's termination right in the same notice, and Satz 4
            // gives them one without notice to the day the change lands.
            "sonderkuendigungsrecht": {
                "besteht": true,
                "rechtsgrundlage": "§ 41 Abs. 5 Satz 4 EnWG",
                "wirksam_zum": row.wirksam_ab.to_string(),
                "entgeltfrei": true,
            },
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
    if let Some(document_id) = document_id {
        pg::produkte::mark_dokument(&mut *tx, row.slice_id, document_id).await?;
    }
    // Marked in the same transaction as the notice that has now been issued:
    // the document exists, the event is in the outbox and will be delivered.
    pg::produkte::mark_notif_sent(&mut *tx, row.slice_id).await?;
    tx.commit().await?;

    tracing::info!(
        komp_id = %row.komp_id, wirksamkeit = %row.wirksam_ab,
        frist = %regime.bezeichnung, dokument = ?document_id,
        "vertragd: Preisänderungsanzeige versandt"
    );
    Ok(())
}

/// Build and issue the § 41 Abs. 5 EnWG notice document for one change.
///
/// # Errors
///
/// * **no addressee** in the customer master. § 126b BGB names the recipient as
///   part of the form.
/// * **no configured `[absender]`**. § 126b names the *declarant* too, and a
///   notice that does not say who is declaring is not Textform.
/// * whatever `outputd` refused — most often no rolled-out PREISANPASSUNG
///   layout.
///
/// Each says what is missing, because "the notice did not go out" has to be
/// answerable without reading code.
async fn issue_preisanpassung_document(
    pool: &PgPool,
    cfg: &VertragdConfig,
    outputd: &crate::dokumente::OutputdClient,
    row: &pg::AnzupassenderPreis,
    regime: &domain::Preisanpassungsregime,
    positionen: Vec<crate::dokumente::PreisPosition>,
) -> anyhow::Result<uuid::Uuid> {
    use crate::dokumente::{PartyView, PreisanpassungView, SonderkuendigungView};

    anyhow::ensure!(
        cfg.absender.is_some(),
        "kein [absender] konfiguriert — die Preisänderungsanzeige kann ihren Erklärenden \
         nicht benennen (§ 126b BGB)"
    );
    let Some(malo_id) = row.malo_id.as_deref() else {
        anyhow::bail!(
            "die Komponente führt keine Marktlokation, die Anzeige ist nicht adressierbar"
        )
    };
    let Some(buyer) = pg::fetch_rechnungsempfaenger_by_malo(pool, malo_id, &cfg.tenant).await?
    else {
        anyhow::bail!(
            "keine Kundenstammdaten zur Marktlokation {malo_id} — die Preisänderungsanzeige \
             ist nicht adressierbar (§ 126b BGB)"
        )
    };

    let today = mako_fristen::heute();
    let mut channels = vec!["PORTAL".to_owned()];
    if buyer.email.is_some() {
        channels.push("EMAIL".to_owned());
    }
    if buyer.post_code.is_some() && buyer.city.is_some() {
        channels.push("POST".to_owned());
    }

    let view = PreisanpassungView {
        datum: today.to_string(),
        absender: crate::dokumente::absender(cfg),
        empfaenger: PartyView {
            name: buyer.name,
            vat_id: buyer.vat_id,
            line1: buyer.line1,
            post_code: buyer.post_code,
            city: buyer.city,
            country: buyer.country,
            email: buyer.email,
            ..PartyView::default()
        },
        vertragsnummer: Some(row.vertrags_nr.clone()),
        malo_id: Some(malo_id.to_owned()),
        sparte: Some(row.sparte.clone()),
        wirksam_ab: row.wirksam_ab.to_string(),
        // The operator's own words when the Tarifwechsel carried a `grund`;
        // otherwise the neutral statement the statute's minimum needs. Never
        // invented prose about *why* prices moved — that is the operator's
        // assertion to make, not this service's.
        anlass: row
            .grund
            .clone()
            .unwrap_or_else(|| "Anpassung der Preisbestandteile Ihres Liefervertrags.".to_owned()),
        ankuendigungsfrist: format!("{} ({})", regime.bezeichnung, regime.rechtsgrundlage),
        positionen,
        // § 41 Abs. 5 Satz 4: without notice, to the day the change takes
        // effect, free of charge. Satz 1 obliges the supplier to state it in
        // this same notice — which is why it is not optional in the view.
        sonderkuendigung: SonderkuendigungView {
            wirksam_zum: row.wirksam_ab.to_string(),
            rechtsgrundlage: "§ 41 Abs. 5 Satz 4 EnWG".to_owned(),
            entgeltfrei: true,
        },
        hinweis: None,
    };

    let issued = outputd
        .issue_preisanpassung(
            &view,
            &row.slice_id.to_string(),
            Some(malo_id),
            &channels,
            row.wirksam_ab,
        )
        .await?;
    Ok(issued.document_id)
}

// ── Auto-renewal (§ 309 Nr. 9 lit. b BGB) ────────────────────────────────────

/// Announce and then apply automatic extensions.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn auto_renewal(pool: &PgPool, cfg: &VertragdConfig) -> anyhow::Result<()> {
    let today = mako_fristen::heute();

    // Phase 1: the advance notice, once per term.
    for row in pg::find_auto_renewal_due(pool, &cfg.tenant, 30).await? {
        let neu = domain::verlaengerung(
            row.haushaltskunde,
            row.vertragsende,
            row.renewal_monate,
            today,
        );
        let mut tx = pool.begin().await?;
        let ce = build_cloud_event(
            mako_events::vertrag::AUTOERNEUERUNG_ANKUENDIGUNG,
            row.id,
            &cfg.tenant,
            serde_json::json!({
                "vertrag_id": row.id,
                "vertrags_nr": row.vertrags_nr,
                "kunden_id": row.kunden_id,
                "vertragsende": row.vertragsende.to_string(),
                "verlaengerung": match neu {
                    Verlaengerung::Unbefristet => serde_json::json!({
                        "art": "UNBEFRISTET",
                        "kuendigungsfrist": "1 Monat, jederzeit",
                        "rechtsgrundlage": "§ 309 Nr. 9 lit. b BGB",
                    }),
                    Verlaengerung::Befristet(bis) => serde_json::json!({
                        "art": "BEFRISTET",
                        "neues_vertragsende": bis.to_string(),
                        "monate": row.renewal_monate,
                    }),
                },
                "kuendigungsfrist_monate": row.kuendigungsfrist_monate,
            }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
        tx.commit().await?;
        // Recorded whatever the delivery does: the notice is due once per term,
        // and the flag is what keeps a daily run from re-deriving it.
        pg::mark_auto_renewal_notified(pool, row.id, row.vertragsende).await?;
    }

    // Phase 2: apply the ones whose term has run out, including any missed
    // while the service was down.
    for row in pg::find_auto_renewal_overdue(pool, &cfg.tenant, today).await? {
        let neu = domain::verlaengerung(
            row.haushaltskunde,
            row.vertragsende,
            row.renewal_monate,
            today,
        );
        pg::apply_auto_renewal(pool, row.id, neu).await?;
        tracing::info!(
            vertrag_id = %row.id, ?neu, haushaltskunde = row.haushaltskunde,
            "vertragd: Vertrag automatisch verlängert"
        );
    }
    Ok(())
}

// ── Ablauf-Ankündigung ────────────────────────────────────────────────────────

/// Close supply that has run out, then announce what is about to.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn ablauf(pool: &PgPool, cfg: &VertragdConfig) -> anyhow::Result<()> {
    // Phase 0: supply whose Lieferende has passed leaves supply, and a contract
    // with nothing left under it is over. A Kündigung only *schedules* the end
    // date — the component stays billable until it arrives, because the
    // customer is still being supplied and still owed a Schlussrechnung — so
    // this is the only place the transition happens.
    let geschlossen = pg::close_due_supply(pool, &cfg.tenant).await?;
    for vertrag_id in &geschlossen {
        let mut tx = pool.begin().await?;
        let ce = build_cloud_event(
            mako_events::vertrag::ABGESCHLOSSEN,
            *vertrag_id,
            &cfg.tenant,
            serde_json::json!({ "vertrag_id": vertrag_id, "status": "ABGELAUFEN" }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
        tx.commit().await?;
        tracing::info!(%vertrag_id, "vertragd: Belieferung beendet — Vertrag abgelaufen");
    }

    // `only_unnotified`: an expiry is announced once, not re-derived every
    // morning for the thirty days it stays within the window.
    let rows = pg::find_expiring_vertraege(pool, &cfg.tenant, 30, true).await?;
    if rows.is_empty() {
        return Ok(());
    }
    tracing::info!(count = rows.len(), "vertragd: Ablauf-Ankündigungen");
    for row in &rows {
        let mut tx = pool.begin().await?;
        let ce = build_cloud_event(
            mako_events::vertrag::ABLAUF_ANKUENDIGUNG,
            row.id,
            &cfg.tenant,
            serde_json::json!({
                "vertrag_id": row.id,
                "vertrags_nr": row.vertrags_nr,
                "kunden_id": row.kunden_id,
                "vertragsart": row.vertragsart,
                "vertragsende": row.vertragsende.map(|d| d.to_string()),
                "preisgarantie_bis": row.preisgarantie_bis.map(|d| d.to_string()),
                "faellig_am": row.faellig_am.to_string(),
                "auto_renewal": row.auto_renewal,
                "kundentyp": row.kundentyp,
                "standort_bezeichnung": row.standort_bezeichnung,
            }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
        tx.commit().await?;
        pg::mark_ablauf_notified(pool, row.id, row.faellig_am).await?;
    }
    Ok(())
}
