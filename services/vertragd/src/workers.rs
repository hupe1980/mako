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
//! with retry and a dead-letter. That also removes the previous escape hatch,
//! where a deployment without a webhook marked the § 41 Abs. 5 notice "sent"
//! purely to stop the log repeating.
//!
//! Each worker is idempotent per contract and per date, so a missed day is
//! caught up rather than skipped, and a repeated day does not resend.

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
    for (name, delay, worker) in [
        ("preisanpassung", 15u64, Worker::Preisanpassung),
        ("auto-renewal", 30, Worker::AutoRenewal),
        ("ablauf", 45, Worker::Ablauf),
    ] {
        let pool = pool.clone();
        let cfg = Arc::clone(&cfg);
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            // Stagger the first run so three workers do not hit the pool at once.
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
            }
            loop {
                if let Err(e) = worker.run_once(&pool, &cfg).await {
                    tracing::error!(worker = name, error = %e, "vertragd: lifecycle worker failed");
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

#[derive(Clone, Copy)]
enum Worker {
    Preisanpassung,
    AutoRenewal,
    Ablauf,
}

impl Worker {
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
///
/// # Errors
///
/// Propagates storage errors; one contract's failure is logged and the run
/// continues, because one bad row must not hold up every other notice.
pub async fn preisanpassung(pool: &PgPool, cfg: &VertragdConfig) -> anyhow::Result<()> {
    let today = mako_fristen::heute();
    let outputd = cfg
        .outputd_url
        .as_deref()
        .filter(|u| !u.is_empty())
        .map(|u| crate::dokumente::OutputdClient::new(u, cfg.outputd_api_key.clone()));
    let outputd = outputd.as_ref();

    for row in pg::offene_preisanpassungen(pool, &cfg.tenant, today).await? {
        let regime = domain::preisanpassungsregime(
            Vertragsart::from_db(&row.vertragsart),
            row.haushaltskunde,
        );
        let vorlauf_tage = (row.wirksam_ab - today).whole_days();
        let frist_gewahrt = regime.frist.gewahrt(today, row.wirksam_ab);
        if !frist_gewahrt {
            // The API refuses a Wirksamkeit this close, so reaching here means
            // the slice predates that guard or was written around it. Say so
            // loudly — a notice that is late is a breach, not a warning.
            tracing::error!(
                komp_id = %row.komp_id, vorlauf_tage, erforderlich = %regime.bezeichnung,
                rechtsgrundlage = regime.rechtsgrundlage,
                "vertragd: Preisänderungsanzeige verspätet — die gesetzliche Frist ist nicht gewahrt"
            );
        }
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
        // Persisted in the same transaction as the flag: the notice is now
        // guaranteed to be delivered, so marking it sent is not a claim about a
        // webhook that may or may not have answered.
        pg::produkte::mark_notif_sent(&mut *tx, row.slice_id).await?;
        tx.commit().await?;
        tracing::info!(
            komp_id = %row.komp_id, wirksamkeit = %row.wirksam_ab,
            frist = %regime.bezeichnung, "vertragd: Preisänderungsanzeige versandt"
        );

        // ── The document ─────────────────────────────────────────────────────
        // Outside the transaction, and after the event: the CloudEvent is the
        // durable obligation and must not depend on a renderer being up. This
        // is the *additional* path — an operator with no ERP gets a letter
        // instead of nothing.
        if let Some(outputd) = outputd {
            match issue_preisanpassung_document(pool, cfg, outputd, &row, &regime).await {
                Ok(Some(document_id)) => {
                    if let Err(e) =
                        pg::produkte::mark_dokument(pool, row.slice_id, document_id).await
                    {
                        tracing::warn!(slice_id = %row.slice_id, error = %e, "vertragd: could not stamp the notice document id");
                    }
                    tracing::info!(
                        komp_id = %row.komp_id, %document_id,
                        "vertragd: Preisänderungsanzeige als Dokument versandt"
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(
                        komp_id = %row.komp_id, error = %e,
                        "vertragd: the § 41 Abs. 5 notice document could not be issued — the \
                         CloudEvent went out, so an ERP-driven letter is unaffected"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Build and issue the § 41 Abs. 5 EnWG notice for one scheduled change.
///
/// `Ok(None)` — deliberately not an error — when the notice cannot be made
/// valid:
///
/// * **no announced prices** on the slice. § 41 Abs. 5 Satz 1 wants the
///   *Umfang* of the change, and a page that states none is not a
///   Preisänderungsanzeige. Scheduling a Tarifwechsel with `preise` supplies
///   them.
/// * **no addressee** in the customer master. § 126b BGB names the recipient as
///   part of the form.
/// * **no configured `[absender]`**. § 126b names the *declarant* too, and a
///   notice that does not say who is declaring is not Textform.
///
/// Each is logged with what is missing, because "the notice did not go out" has
/// to be answerable without reading code.
async fn issue_preisanpassung_document(
    pool: &PgPool,
    cfg: &VertragdConfig,
    outputd: &crate::dokumente::OutputdClient,
    row: &pg::AnzupassenderPreis,
    regime: &domain::Preisanpassungsregime,
) -> anyhow::Result<Option<uuid::Uuid>> {
    use crate::dokumente::{PartyView, PreisPosition, PreisanpassungView, SonderkuendigungView};

    if cfg.absender.is_none() {
        tracing::warn!(
            komp_id = %row.komp_id,
            "vertragd: no [absender] configured — the § 41 Abs. 5 notice cannot name its              declarant (§ 126b BGB), so no document was issued"
        );
        return Ok(None);
    }
    let Some(positionen) = row
        .angekuendigte_preise
        .clone()
        .and_then(|v| serde_json::from_value::<Vec<PreisPosition>>(v).ok())
        .filter(|p: &Vec<PreisPosition>| !p.is_empty())
    else {
        tracing::warn!(
            komp_id = %row.komp_id, wirksamkeit = %row.wirksam_ab,
            "vertragd: the scheduled Tarifwechsel carries no announced prices, so the § 41              Abs. 5 Satz 1 Umfang cannot be stated and no notice document was issued —              supply `preise` on POST /vertraege/{{id}}/tarifwechsel"
        );
        return Ok(None);
    };

    let Some(malo_id) = row.malo_id.as_deref() else {
        tracing::warn!(
            komp_id = %row.komp_id,
            "vertragd: the component has no Marktlokation, so the notice cannot be addressed"
        );
        return Ok(None);
    };
    let Some(buyer) = pg::fetch_rechnungsempfaenger_by_malo(pool, malo_id, &cfg.tenant).await?
    else {
        tracing::warn!(
            komp_id = %row.komp_id, malo_id,
            "vertragd: no customer master for this Marktlokation — the § 41 Abs. 5 notice              cannot be addressed (§ 126b BGB), so no document was issued"
        );
        return Ok(None);
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
    Ok(Some(issued.document_id))
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
        // and re-deriving it daily is what made the ERP see thirty of them.
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

    // `only_unnotified`: without it this re-derived the same expiry every
    // morning and the ERP received one notice per day for thirty days.
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
