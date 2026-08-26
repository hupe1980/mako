//! Versorgungsverträge: creation, termination, tariff changes, price guarantees.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_service::{ApiError, ApiResult, oidc::Claims};
use serde::Deserialize;
use time::Date;
use uuid::Uuid;

use super::{Ctx, ok, require_kunde, require_vertrag};
use crate::{
    domain::{self, Kuendigungsgrund, Vertragsart},
    events::build_cloud_event,
    pg,
};

fn heute() -> Date {
    time::OffsetDateTime::now_utc().date()
}

// ── Create ────────────────────────────────────────────────────────────────────

/// `POST /api/v1/kunden/{id}/vertraege` — create a supply contract.
///
/// The contract, its components and the intent to register each commodity at
/// the NB commit together; the registrations are then performed by the outbound
/// worker, so neither a crash nor a `processd` outage loses one.
///
/// Idempotent on `erp_contract_id`: a replay returns `200` and dispatches
/// nothing.
///
/// ## What is refused
///
/// - a term § 309 Nr. 9 BGB does not permit for a consumer, or an
///   Ersatzversorgung running past the § 38 Abs. 4 EnWG three months;
/// - a gas component without a Messlokation — `start-supply-gas` needs the
///   Zählpunktbezeichnung and a MaLo-ID is not one.
pub async fn create(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<pg::CreateVersorgungsvertragInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let kunde = require_kunde(&ctx, kunden_id).await?;

    let vertragsart = Vertragsart::from_db(input.vertragsart.as_deref().unwrap_or("SONDERVERTRAG"));
    let verstoesse = domain::pruefe_laufzeit(
        kunde.haushaltskunde,
        vertragsart,
        input.vertragsbeginn,
        input.vertragsende,
        input.kuendigungsfrist_monate.unwrap_or(1),
        input.auto_renewal.unwrap_or(false),
        input.renewal_monate.unwrap_or(0),
    );
    if !verstoesse.is_empty() {
        return Err(unprocessable_json(serde_json::json!({
            "error": "die vereinbarte Laufzeit ist so nicht zulässig",
            "verstoesse": verstoesse,
        })));
    }
    for k in &input.komponenten {
        if k.sparte == "GAS" && k.melo_id.as_deref().unwrap_or("").is_empty() {
            return Err(ApiError::unprocessable(
                "eine GAS-Komponente braucht eine melo_id — start-supply-gas trägt sie als \
                 Zählpunktbezeichnung (RFF+Z13); eine MaLo-ID ist keine",
            ));
        }
    }

    let inserted = pg::insert_versorgungsvertrag(
        &ctx.pool,
        kunden_id,
        ctx.tenant(),
        &ctx.cfg.lf_mp_id,
        &input,
    )
    .await
    .map_err(ApiError::Internal)?;

    let status = if inserted.dispatched > 0 {
        "IN_BEARBEITUNG"
    } else {
        "ANGELEGT"
    };
    let code = if inserted.is_new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        code,
        Json(serde_json::json!({
            "vertrag_id": inserted.id,
            "vertrags_nr": inserted.vertrags_nr,
            "status": status,
            "komponenten": inserted.komponenten.len(),
            "mako_dispatched": inserted.dispatched,
            "idempotent_replay": !inserted.is_new,
        })),
    ))
}

/// A 422 whose body is a structured object rather than a sentence.
fn unprocessable_json(body: serde_json::Value) -> ApiError {
    ApiError::Unprocessable(body.to_string())
}

// ── Read ──────────────────────────────────────────────────────────────────────

/// `GET /api/v1/vertraege/{id}` — contract with its components.
pub async fn get(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let vertrag = require_vertrag(&ctx, id).await?;
    let komponenten = pg::list_komponenten(&ctx.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "vertrag": vertrag, "komponenten": komponenten }))
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub limit: Option<i64>,
}

/// `GET /api/v1/vertraege` — open contracts.
pub async fn list_open(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::list_offene_vertraege(
        &ctx.pool,
        ctx.tenant(),
        q.limit.unwrap_or(100).clamp(1, 500),
    )
    .await
    .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "vertraege": rows }))
}

/// `GET /api/v1/kunden/{id}/vertraege`
pub async fn list_by_kunde(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    require_kunde(&ctx, kunden_id).await?;
    let rows = pg::list_vertraege_by_kunde(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(rows)
}

/// `GET /api/v1/vertraege/by-malo/{malo_id}` — the active contract behind a MaLo.
///
/// The lookup `billingd` uses for the § 40 Abs. 1 EnWG contract facts an
/// invoice must state. Besides the contract row it computes the next possible
/// Kündigungstermin under the rules that actually apply to this contract, and
/// resolves the BG-7 buyer so the e-invoice has a real addressee.
pub async fn by_malo(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(malo_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let (vertrag, komponente) = pg::fetch_vertrag_by_malo(&ctx.pool, &malo_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    let kunde = pg::fetch_kunde(&ctx.pool, vertrag.kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    let haushaltskunde = kunde.as_ref().is_none_or(|k| k.haushaltskunde);
    let frist = naechster_kuendigungstermin(&vertrag, haushaltskunde);

    // Best-effort: a missing Kunde must not fail the contract lookup § 40 Abs. 1
    // needs; the invoice then carries a synthesised buyer and says so.
    let rechnungsempfaenger =
        match pg::fetch_rechnungsempfaenger_by_malo(&ctx.pool, &malo_id, ctx.tenant()).await {
            Ok(re) => re,
            Err(e) => {
                tracing::warn!(
                    malo_id = %malo_id, error = %e,
                    "vertragd: BG-7 buyer lookup failed; the invoice falls back to the \
                     synthesised buyer and is not XRechnung-conformant",
                );
                None
            }
        };

    ok(serde_json::json!({
        "vertrag": vertrag,
        "komponente": komponente,
        "rechnungsempfaenger": rechnungsempfaenger,
        "naechstmoeglicher_kuendigungstermin": frist.fruehestens.to_string(),
        "kuendigungsfrist": frist,
    }))
}

/// The next date this contract could end if notice arrived today.
///
/// An open-ended contract ends on its notice period; a fixed-term one no
/// earlier than its own end.
fn naechster_kuendigungstermin(
    vertrag: &pg::VersorgungsvertragRow,
    haushaltskunde: bool,
) -> domain::Kuendigungsfrist {
    let mut frist = domain::kuendigungsfrist(
        heute(),
        Vertragsart::from_db(&vertrag.vertragsart),
        haushaltskunde,
        Kuendigungsgrund::Ordentlich,
        vertrag.kuendigungsfrist_monate,
        None,
    );
    if let Some(ende) = vertrag.vertragsende
        && frist.fruehestens < ende
    {
        frist.fruehestens = ende;
        frist.frist = format!("{} (Ende der vereinbarten Laufzeit)", frist.frist);
    }
    frist
}

#[derive(Deserialize)]
pub struct ProdukteQuery {
    /// Point form: the product in force on this day. Defaults to today.
    pub as_of: Option<Date>,
    /// Period form (with `to`): every slice covering `[from, to]`.
    pub from: Option<Date>,
    /// Period form (with `from`), inclusive.
    pub to: Option<Date>,
}

/// `GET /api/v1/malo/{malo_id}/produkte` — which product a MaLo is billed on.
///
/// The single source of truth for the MaLo→product mapping. It is a contract
/// fact — agreeing it is a Tarifwechsel under § 41 Abs. 5 EnWG — so it lives
/// here, with the contract, and not in a catalogue that would have to be told
/// about it.
///
/// **Period form** (`?from=&to=`) is what `billingd` bills from: an invoice
/// covers a period, a Tarifwechsel inside it splits that period, and the answer
/// is the ordered slices tiling it. `[gueltig_von, gueltig_bis)` is half-open,
/// so consecutive slices share no day.
///
/// **Point form** (`?as_of=`, default today) answers the single-product
/// question.
pub async fn malo_produkte(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ProdukteQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    if let (Some(from), Some(to)) = (q.from, q.to) {
        if from > to {
            return Err(ApiError::bad_request("from must not be after to"));
        }
        let slices = pg::malo_slices(&ctx.pool, ctx.tenant(), &malo_id, from, to)
            .await
            .map_err(ApiError::Internal)?;
        let covered: i64 = slices
            .iter()
            .map(|s| {
                let bis = s.gueltig_bis.unwrap_or_else(|| to.next_day().unwrap_or(to));
                (bis - s.gueltig_von).whole_days()
            })
            .sum();
        return ok(serde_json::json!({
            "malo_id": malo_id,
            "from": from.to_string(),
            "to": to.to_string(),
            "slice_count": slices.len(),
            // A period the slices do not tile completely is a real condition —
            // supply began mid-period, or the contract ended — and billing has
            // to see it rather than infer it from arithmetic.
            "fully_covered": covered == (to - from).whole_days() + 1,
            "slices": slices,
        }));
    }

    let am = q.as_of.unwrap_or_else(heute);
    let slices = pg::malo_slices(&ctx.pool, ctx.tenant(), &malo_id, am, am)
        .await
        .map_err(ApiError::Internal)?;
    slices
        .into_iter()
        .next()
        .map(|s| {
            Json(serde_json::json!({ "malo_id": malo_id, "as_of": am.to_string(), "slice": s }))
        })
        .ok_or(ApiError::NotFound)
}

/// `GET /api/v1/vertraege/{id}/kuendigungsfrist` — the earliest end date per
/// reason.
///
/// A portal that lets a customer terminate has to show the date *before* the
/// request, and an operator answering a letter has to quote the rule. Computing
/// it in the portal would put four statutes in a second place; asking here
/// keeps one answer.
pub async fn kuendigungsfrist(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Query(q): Query<KuendigungsfristQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let vertrag = require_vertrag(&ctx, id).await?;
    let kunde = require_kunde(&ctx, vertrag.kunden_id).await?;
    let eingang = q.eingang.unwrap_or_else(heute);
    let art = Vertragsart::from_db(&vertrag.vertragsart);
    let per_grund: serde_json::Map<String, serde_json::Value> = [
        Kuendigungsgrund::Ordentlich,
        Kuendigungsgrund::Preisanpassung,
        Kuendigungsgrund::Umzug,
        Kuendigungsgrund::Lieferantenwechsel,
    ]
    .into_iter()
    .map(|grund| {
        let f = domain::kuendigungsfrist(
            eingang,
            art,
            kunde.haushaltskunde,
            grund,
            vertrag.kuendigungsfrist_monate,
            q.preisanpassung_wirksam_zum,
        );
        (
            grund.as_db().to_owned(),
            serde_json::to_value(f).unwrap_or(serde_json::Value::Null),
        )
    })
    .collect();
    ok(serde_json::json!({
        "vertrag_id": id,
        "vertragsart": vertrag.vertragsart,
        "haushaltskunde": kunde.haushaltskunde,
        "eingang": eingang.to_string(),
        "fristen": per_grund,
    }))
}

#[derive(Deserialize)]
pub struct KuendigungsfristQuery {
    /// When the notice arrives; defaults to today.
    pub eingang: Option<Date>,
    /// For a § 41 Abs. 5 Satz 4 Sonderkündigung.
    pub preisanpassung_wirksam_zum: Option<Date>,
}

/// `GET /api/v1/vertraege/billing-candidates` — § 40b EnWG billing cadence feed.
pub async fn billing_candidates(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
) -> ApiResult<Json<serde_json::Value>> {
    let rows = pg::list_billing_candidates(&ctx.pool, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "candidates": rows }))
}

#[derive(Deserialize)]
pub struct ExpiringQuery {
    /// Calendar days to look ahead. Default 30.
    pub days: Option<i64>,
}

/// `GET /api/v1/vertraege/expiring` — contracts whose term or price guarantee
/// runs out soon.
pub async fn expiring(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<ExpiringQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let days = q.days.unwrap_or(30).clamp(1, 365);
    let rows = pg::find_expiring_vertraege(&ctx.pool, ctx.tenant(), days, false)
        .await
        .map_err(ApiError::Internal)?;
    ok(serde_json::json!({
        "count": rows.len(),
        "look_ahead_days": days,
        "vertraege": rows,
    }))
}

// ── Kündigung ─────────────────────────────────────────────────────────────────

/// `POST /api/v1/vertraege/{id}/kuendigen` — terminate the contract.
///
/// The notice period comes from the *reason*, not from the contract: an
/// ordinary termination runs on the agreed period (capped for consumers by
/// § 309 Nr. 9 lit. c BGB), a Grundversorgungsvertrag on the two weeks of
/// § 20 Abs. 1 StromGVV/GasGVV, a move on the six weeks of § 41b Abs. 5 EnWG,
/// and a price change ends the contract without notice on the day the change
/// takes effect (§ 41 Abs. 5 Satz 4 EnWG). A `lieferende` earlier than the rule
/// allows is refused with the rule quoted.
///
/// Everything then happens in one transaction: the components leave supply, the
/// Lieferende UTILMDs and the Schlussablesung are enqueued, and the
/// `de.vertrag.kuendigung` CloudEvent — which carries the § 41 Abs. 8 Nr. 2
/// EnWG Textform confirmation the supplier owes the customer — goes into the
/// outbox.
pub async fn kuendigen(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(input): Json<pg::KuendigungInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let vertrag = require_vertrag(&ctx, id).await?;
    if !matches!(vertrag.status.as_str(), "AKTIV" | "TEILERFUELLUNG") {
        return Err(ApiError::conflict(format!(
            "Vertrag im Status '{}' kann nicht gekündigt werden — erforderlich ist \
             AKTIV oder TEILERFUELLUNG",
            vertrag.status
        )));
    }
    let kunde = require_kunde(&ctx, vertrag.kunden_id).await?;
    let today = heute();
    let eingang = input.eingang.unwrap_or(today);
    if eingang > today {
        return Err(ApiError::unprocessable(
            "eingang liegt in der Zukunft — eine Kündigung kann nicht vorab zugehen",
        ));
    }
    let frist = domain::kuendigungsfrist(
        eingang,
        Vertragsart::from_db(&vertrag.vertragsart),
        kunde.haushaltskunde,
        input.grund,
        vertrag.kuendigungsfrist_monate,
        input.preisanpassung_wirksam_zum,
    );
    if input.lieferende < frist.fruehestens {
        return Err(unprocessable_json(serde_json::json!({
            "error": "lieferende liegt vor dem frühestmöglichen Kündigungstermin",
            "lieferende": input.lieferende.to_string(),
            "fruehestens": frist.fruehestens.to_string(),
            "frist": frist.frist,
            "rechtsgrundlage": frist.rechtsgrundlage,
            "grund": input.grund,
        })));
    }

    let mut tx = ctx.pool.begin().await.map_err(anyhow_from)?;
    let result = pg::kuendige_vertrag(&mut tx, &vertrag, &input, eingang, &ctx.cfg.lf_mp_id)
        .await
        .map_err(ApiError::Internal)?;

    // § 41 Abs. 8 Nr. 2 EnWG obliges the supplier to confirm receipt of the
    // Kündigung to the customer in Textform. The document itself is produced
    // downstream; what has to be guaranteed here is that the instruction to
    // produce it survives — so it commits with the termination and the
    // timestamp records that the obligation was discharged.
    let ce = build_cloud_event(
        mako_events::vertrag::KUENDIGUNG,
        id,
        ctx.tenant(),
        serde_json::json!({
            "vertrag_id": id,
            "vertrags_nr": vertrag.vertrags_nr,
            "kunden_id": vertrag.kunden_id,
            "lieferende": input.lieferende.to_string(),
            "eingang": eingang.to_string(),
            "grund": input.grund,
            "frist": frist.frist,
            "rechtsgrundlage": frist.rechtsgrundlage,
            "mako_dispatched": result.dispatched.len(),
            "kuendigungsbestaetigung": {
                "erforderlich": true,
                "form": "Textform",
                "rechtsgrundlage": "§ 41 Abs. 8 Nr. 2 EnWG",
                "vertragsende": input.lieferende.to_string(),
            },
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    pg::mark_kuendigung_bestaetigt(&mut *tx, id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    tx.commit().await.map_err(anyhow_from)?;

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "vertrag_id": id,
            "vertrags_nr": vertrag.vertrags_nr,
            "status": "GEKÜNDIGT",
            "lieferende": input.lieferende.to_string(),
            "grund": input.grund,
            "frist": frist.frist,
            "rechtsgrundlage": frist.rechtsgrundlage,
            "mako_dispatched": result.dispatched.len(),
            "direkt_beendet": result.direkt_beendet.len(),
        })),
    ))
}

/// `POST /api/v1/vertraege/{id}/widerruf-kuendigung` — withdraw a Kündigung.
///
/// Allowed while the Lieferende is still ahead. The in-flight Lieferende UTILMD
/// is the operator's to cancel in `processd`; the response says so.
pub async fn widerruf_kuendigung(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut tx = ctx.pool.begin().await.map_err(anyhow_from)?;
    if let Err(e) = pg::widerruf_kuendigung(&mut tx, id, ctx.tenant()).await {
        let msg = e.to_string();
        return Err(if msg.contains("not found") {
            ApiError::NotFound
        } else {
            ApiError::conflict(msg)
        });
    }
    let ce = build_cloud_event(
        mako_events::vertrag::KUENDIGUNG_WIDERRUFEN,
        id,
        ctx.tenant(),
        serde_json::json!({ "vertrag_id": id }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    tx.commit().await.map_err(anyhow_from)?;
    ok(serde_json::json!({
        "vertrag_id": id,
        "status": "AKTIV",
        "message": "Kündigung widerrufen — ein bereits versandtes Lieferende-UTILMD \
                    ist über processd zu stornieren",
    }))
}

/// `POST /api/v1/vertraege/{id}/stornieren` — cancel before supply began.
///
/// Valid for `ANGELEGT` and `IN_BEARBEITUNG`. A registration still waiting in
/// the outbound queue is withdrawn with it; one already sent to `processd` has
/// to be cancelled there, and the response says which case this was.
pub async fn stornieren(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    let vertrag = require_vertrag(&ctx, id).await?;
    if !matches!(vertrag.status.as_str(), "ANGELEGT" | "IN_BEARBEITUNG") {
        return Err(ApiError::conflict(format!(
            "Vertrag im Status '{}' kann nicht storniert werden — nur ANGELEGT oder \
             IN_BEARBEITUNG; laufende Belieferung wird gekündigt, nicht storniert",
            vertrag.status
        )));
    }
    pg::storniere_vertrag(&ctx.pool, id, ctx.tenant())
        .await
        .map_err(|e| ApiError::conflict(e.to_string()))?;
    ok(serde_json::json!({
        "vertrag_id": id,
        "status": "STORNIERT",
        "hinweis": if vertrag.status == "IN_BEARBEITUNG" {
            Some("Bereits an processd übergebene MaKo-Prozesse sind dort zu stornieren.")
        } else {
            None
        },
    }))
}

// ── Tarifwechsel ──────────────────────────────────────────────────────────────

/// `POST /api/v1/vertraege/{id}/tarifwechsel` — change a component's product.
///
/// A Tarifwechsel changes price, not supply: no UTILMD is sent, and the MaKo
/// status is untouched.
///
/// ## What is enforced
///
/// - **Preisgarantie.** A change taking effect inside the guarantee window is
///   refused unless the operator overrides it, and every override is written to
///   `preisgarantie_override_log` with the operator's token subject.
/// - **§ 41 Abs. 5 Satz 2 EnWG.** The customer is owed a month's notice (two
///   weeks outside households); a Grundversorgungspreis needs the six weeks of
///   § 5 Abs. 2 StromGVV/GasGVV. A Wirksamkeit too close to today is refused
///   *here*, so the notice period cannot be missed by scheduling.
/// - **§ 5 Abs. 2 GVV.** A Grundversorgungspreis changes only at the start of a
///   month.
///
/// A retroactive correction (`wirksamkeit <= today`) is exempt from the notice
/// rules — it is not an announced price change but the repair of one already
/// agreed.
pub async fn tarifwechsel(
    claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(vertrag_id): Path<Uuid>,
    Json(input): Json<pg::TarifwechselInput>,
) -> ApiResult<Json<serde_json::Value>> {
    let vertrag = require_vertrag(&ctx, vertrag_id).await?;
    let kunde = require_kunde(&ctx, vertrag.kunden_id).await?;
    let komp = pg::fetch_komponente(&ctx.pool, input.komp_id)
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
    if komp.vertrag_id != vertrag_id {
        return Err(ApiError::unprocessable(
            "die Komponente gehört nicht zu diesem Vertrag",
        ));
    }

    let today = heute();
    let is_future = input.wirksamkeit > today;
    let art = Vertragsart::from_db(&vertrag.vertragsart);
    let regime = domain::preisanpassungsregime(art, kunde.haushaltskunde);

    // ── Preisgarantie ────────────────────────────────────────────────────────
    if !input.override_preisgarantie
        && let Some(bis) = vertrag.preisgarantie_bis
        && input.wirksamkeit <= bis
    {
        return Err(unprocessable_json(serde_json::json!({
            "error": "Tarifwechsel durch Preisgarantie gesperrt",
            "preisgarantie_bis": bis.to_string(),
            "wirksamkeit": input.wirksamkeit.to_string(),
            "hinweis": "override_preisgarantie=true nur mit dokumentiertem Kundenverzicht",
        })));
    }

    // ── Notice period (§ 41 Abs. 5 EnWG / § 5 Abs. 2 GVV) ────────────────────
    if is_future {
        let fruehestens = regime.fruehestens_wirksam(today);
        if input.wirksamkeit < fruehestens {
            return Err(unprocessable_json(serde_json::json!({
                "error": "die Wirksamkeit wahrt die gesetzliche Ankündigungsfrist nicht",
                "wirksamkeit": input.wirksamkeit.to_string(),
                "fruehestens": fruehestens.to_string(),
                "frist": regime.frist,
                "rechtsgrundlage": regime.rechtsgrundlage,
            })));
        }
        if regime.nur_zum_monatsersten && input.wirksamkeit.day() != 1 {
            return Err(unprocessable_json(serde_json::json!({
                "error": "Änderungen der Allgemeinen Preise werden nur zum Monatsbeginn wirksam",
                "wirksamkeit": input.wirksamkeit.to_string(),
                "naechster_zulaessiger_termin":
                    domain::naechster_monatserster(fruehestens).to_string(),
                "rechtsgrundlage": "§ 5 Abs. 2 StromGVV / GasGVV",
            })));
        }
    }

    let mut tx = ctx.pool.begin().await.map_err(anyhow_from)?;

    // The product being replaced is the one in force on the day the change
    // takes effect — which for a future-dated change is not necessarily the one
    // in force today.
    let bisher = pg::produkte::produkt_am(&mut *tx, input.komp_id, input.wirksamkeit)
        .await
        .map_err(ApiError::Internal)?;
    let bisheriges_produkt = bisher.as_ref().map(|s| s.product_code.clone());

    if input.override_preisgarantie
        && let Some(bis) = vertrag.preisgarantie_bis
    {
        tracing::warn!(
            %vertrag_id, komp_id = %input.komp_id, wirksamkeit = %input.wirksamkeit,
            operator = %claims.sub(),
            "vertragd: Preisgarantie OVERRIDE — price lock bypassed by operator"
        );
        sqlx::query(
            r"INSERT INTO preisgarantie_override_log
              (tenant, vertrag_id, komp_id, preisgarantie_bis, wirksamkeit,
               old_product_code, new_product_code, operator_identity, override_reason)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(ctx.tenant())
        .bind(vertrag_id)
        .bind(input.komp_id)
        .bind(bis)
        .bind(input.wirksamkeit)
        .bind(bisheriges_produkt.as_deref().unwrap_or(""))
        .bind(&input.new_product_code)
        .bind(claims.sub())
        .bind(&input.grund)
        .execute(&mut *tx)
        .await
        .map_err(anyhow_from)?;
    }

    // One act, whatever the date: open a slice from `wirksamkeit`. A future
    // change is a slice that starts in the future — there is no pending state
    // and nothing to apply later, which is what let three columns and a daily
    // worker phase go.
    //
    // The notice is owed only for a change that has not happened yet; a
    // retroactive correction announces a date that has already passed.
    let preise = (!input.preise.is_empty())
        .then(|| serde_json::to_value(&input.preise))
        .transpose()
        .map_err(|e| ApiError::Internal(e.into()))?;
    pg::produkte::tarifwechsel(
        &mut tx,
        ctx.tenant(),
        input.komp_id,
        &input.new_product_code,
        input.wirksamkeit,
        input.grund.as_deref(),
        !is_future,
        preise.as_ref(),
    )
    .await
    .map_err(|e| ApiError::conflict(e.to_string()))?;

    let ce_type = if is_future {
        mako_events::vertrag::TARIFWECHSEL_GEPLANT
    } else {
        mako_events::vertrag::TARIFWECHSEL
    };
    let ce = build_cloud_event(
        ce_type,
        vertrag_id,
        ctx.tenant(),
        serde_json::json!({
            "vertrag_id": vertrag_id,
            "komp_id": input.komp_id,
            "malo_id": komp.malo_id,
            "old_product_code": bisheriges_produkt,
            "new_product_code": input.new_product_code,
            "wirksamkeit": input.wirksamkeit.to_string(),
            "geplant": is_future,
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    tx.commit().await.map_err(anyhow_from)?;

    ok(serde_json::json!({
        "vertrag_id": vertrag_id,
        "komp_id": input.komp_id,
        "new_product_code": input.new_product_code,
        "wirksamkeit": input.wirksamkeit.to_string(),
        "wirksam_ab": input.wirksamkeit.to_string(),
        "rueckwirkend": !is_future,
        "ankuendigungsfrist": if is_future { Some(regime) } else { None },
    }))
}

// ── Preisgarantie ─────────────────────────────────────────────────────────────

/// `PUT /api/v1/vertraege/{id}/preisgarantie` — store the BO4E `Preisgarantie`.
///
/// `preisgarantie_bis` is derived from the COM's `zeitlicheGueltigkeit`, so the
/// Tarifwechsel guard and the document the customer holds can never disagree.
pub async fn put_preisgarantie(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(vertrag_id): Path<Uuid>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<StatusCode> {
    use rubo4e::current::Preisgarantie;
    // The BO4E gate, which for a `Preisgarantie` also checks the `Zeitraum` in
    // `zeitlicheGueltigkeit` — the very field `preisgarantie_bis` is derived
    // from below, and the one the Tarifwechsel guard reads.
    let typed: Preisgarantie = mako_markt::bo4e::decode(body)
        .map_err(|e| ApiError::unprocessable_with(e.to_string(), e.detail().into()))?;
    let canonical =
        serde_json::to_value(&typed).map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    let bis = canonical
        .pointer("/zeitlicheGueltigkeit/enddatum")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_iso_date);

    let mut tx = ctx.pool.begin().await.map_err(anyhow_from)?;
    if !pg::upsert_preisgarantie(&mut *tx, vertrag_id, ctx.tenant(), &canonical, bis)
        .await
        .map_err(ApiError::Internal)?
    {
        return Err(ApiError::NotFound);
    }
    let ce = build_cloud_event(
        mako_events::vertrag::PREISGARANTIE_HINTERLEGT,
        vertrag_id,
        ctx.tenant(),
        serde_json::json!({
            "vertrag_id": vertrag_id,
            "preisgarantie_bis": bis.map(|d| d.to_string()),
        }),
    );
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    tx.commit().await.map_err(anyhow_from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/vertraege/{id}/preisgarantie`
pub async fn get_preisgarantie(
    _claims: Claims,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(vertrag_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    pg::fetch_preisgarantie(&ctx.pool, vertrag_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

/// Parse the date prefix of a BO4E date or date-time string.
fn parse_iso_date(s: &str) -> Option<Date> {
    let head: String = s.chars().take(10).collect();
    Date::parse(&head, &time::format_description::well_known::Iso8601::DATE).ok()
}

fn anyhow_from(e: sqlx::Error) -> ApiError {
    ApiError::Internal(anyhow::Error::new(e))
}

#[cfg(test)]
mod tests {
    use super::parse_iso_date;
    use time::macros::date;

    #[test]
    fn a_plain_date_parses() {
        assert_eq!(parse_iso_date("2027-06-30"), Some(date!(2027 - 06 - 30)));
    }

    #[test]
    fn a_date_time_yields_its_date() {
        // BO4E serialises `zeitlicheGueltigkeit` as a date-time in some
        // producers; taking the date prefix keeps the guard working for both.
        assert_eq!(
            parse_iso_date("2027-06-30T00:00:00Z"),
            Some(date!(2027 - 06 - 30))
        );
    }

    #[test]
    fn nonsense_is_none_rather_than_a_wrong_date() {
        assert_eq!(parse_iso_date("bald"), None);
    }
}
