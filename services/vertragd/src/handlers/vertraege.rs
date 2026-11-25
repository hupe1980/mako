//! Versorgungsverträge: creation, termination, tariff changes, price guarantees.

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
};
use mako_markt::bo4e::Bo4e;
use mako_service::{ApiError, ApiResult, Json, oidc::Claims};
use serde::Deserialize;
use time::Date;
use uuid::Uuid;

use super::{
    CedarEnforcer, Ctx, authorize, canonical_bo4e, ok, refuse_undefined_fields, require_kunde,
    require_vertrag,
};
use crate::{
    domain::{self, Kuendigungsgrund, Vertragsart},
    events, pg,
};

fn heute() -> Date {
    mako_fristen::heute()
}

/// Query parameters of the `by-malo` lookup.
#[derive(Debug, Default, serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ByMaloQuery {
    /// Invoice issuer, required alongside the full billing interval.
    #[param(value_type = Option<String>)]
    pub lf_mp_id: Option<rubo4e::identifiers::MarktpartnerId>,
    pub from: Option<Date>,
    pub to: Option<Date>,
    /// `?stichtag=YYYY-MM-DD` — the date a Kündigungsfrist is measured from.
    /// Absent means today.
    pub stichtag: Option<String>,
    /// `?kunde=<Name>` — the customer a market message names, for `E_0624`
    /// Prüfschritt 50.
    ///
    /// `processd` passes the `SG12 NAD+Z09` „Kunde des LF" of an inbound 55010
    /// (or 44010). The answer comes back as `kunde_identisch_mit_anfrage`, and
    /// it is deliberately tri-state — see [`domain::kundenidentitaet`].
    pub kunde: Option<String>,
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
/// - a gas component registered at the NB whose Anmeldung NN 44001 lacks a
///   contract fact the column makes Muss — the Anlass, the Netznutzungsvertrag
///   and its Zahler, the Gemeinderabatt, an Abrechnungsintervall, or the
///   Kunde's name and address (UTILMD AHB Gas 1.2 Kap. 5.6). Each is named.
#[utoipa::path(
    post,
    path = "/api/v1/kunden/{id}/vertraege",
    tag = "vertraege",
    operation_id = "create_vertrag",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    request_body(content = serde_json::Value, description = "`CreateVersorgungsvertragInput`"),
    responses(
        (status = 201, body = serde_json::Value),
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn create(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
    Json(input): Json<pg::CreateVersorgungsvertragInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&enforcer, &claims, "create-vertrag", ctx.tenant())?;
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
        if let Some(bk) = k.bilanzkreis.as_deref() {
            domain::pruefe_bilanzkreis(&k.sparte, bk).map_err(ApiError::unprocessable)?;
        }
        k.pruefe_vertriebsangaben(vertragsart.as_db(), heute())
            .map_err(|a| ApiError::unprocessable(a.message).with_code(a.code))?;
    }
    // The Anmeldung — the GeLi Gas 44001 or the Strom 55001 — is assembled
    // here as it will be on insert, so a contract that cannot be registered is
    // refused before it is written.
    if input.komponenten.iter().any(|k| input.meldet_an(k)) {
        let mut conn = ctx
            .pool
            .acquire()
            .await
            .map_err(|e| ApiError::Internal(e.into()))?;
        let anmeldedaten = pg::kunde_anmeldedaten(
            &mut conn,
            kunden_id,
            ctx.tenant(),
            input.standort_adresse.as_ref(),
        )
        .await
        .map_err(ApiError::Internal)?
        .ok_or(ApiError::NotFound)?;
        let zyklus = input.abrechnungszyklus.as_deref().unwrap_or("JAEHRLICH");
        let fehlend: Vec<String> = input
            .komponenten
            .iter()
            .filter(|k| input.meldet_an(k))
            .filter_map(|k| k.gas_anmeldung(zyklus, &anmeldedaten).err())
            .flatten()
            .collect();
        if !fehlend.is_empty() {
            return Err(unprocessable_json(serde_json::json!({
                "error": "die Anmeldung NN 44001 braucht Angaben, die der Vertrag nicht enthält \
                          (UTILMD AHB Gas 1.2 Kap. 5.6)",
                "code": "anmeldedaten_unvollstaendig",
                "fehlend": fehlend,
            })));
        }
        let fehlend: Vec<String> = input
            .komponenten
            .iter()
            .filter(|k| input.meldet_an(k))
            .filter_map(|k| k.strom_anmeldung(&anmeldedaten).err())
            .flatten()
            .collect();
        if !fehlend.is_empty() {
            return Err(unprocessable_json(serde_json::json!({
                "error": "die Anmeldung 55001 braucht Angaben, die der Vertrag nicht enthält \
                          (UTILMD AHB Strom 2.2 Kap. 8.2)",
                "code": "anmeldedaten_unvollstaendig",
                "fehlend": fehlend,
            })));
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
#[utoipa::path(
    get,
    path = "/api/v1/vertraege/{id}",
    tag = "vertraege",
    operation_id = "get_vertrag",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
    )
)]
pub async fn get(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    let vertrag = require_vertrag(&ctx, id).await?;
    let komponenten = pg::list_komponenten(&ctx.pool, id)
        .await
        .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "vertrag": vertrag, "komponenten": komponenten }))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListQuery {
    pub limit: Option<i64>,
    /// The contract created with this `erp_contract_id`, in any status.
    pub erp_contract_id: Option<String>,
    /// The contracts of the order with this `bestellreferenz`, in any status.
    pub bestellreferenz: Option<String>,
}

/// `GET /api/v1/vertraege` — open contracts, or the contracts a caller's own
/// reference names (`?erp_contract_id=` / `?bestellreferenz=`), whatever
/// their status.
#[utoipa::path(
    get,
    path = "/api/v1/vertraege",
    tag = "vertraege",
    params(ListQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn list_open(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    let rows = if q.erp_contract_id.is_some() || q.bestellreferenz.is_some() {
        pg::list_vertraege_by_referenz(
            &ctx.pool,
            ctx.tenant(),
            q.erp_contract_id.as_deref(),
            q.bestellreferenz.as_deref(),
        )
        .await
    } else {
        pg::list_offene_vertraege(
            &ctx.pool,
            ctx.tenant(),
            q.limit.unwrap_or(100).clamp(1, 500),
        )
        .await
    }
    .map_err(ApiError::Internal)?;
    ok(serde_json::json!({ "count": rows.len(), "vertraege": rows }))
}

/// `GET /api/v1/kunden/{id}/vertraege`
#[utoipa::path(
    get,
    path = "/api/v1/kunden/{id}/vertraege",
    tag = "vertraege",
    operation_id = "list_vertraege_by_kunde",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
    )
)]
pub async fn list_by_kunde(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(kunden_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    require_kunde(&ctx, kunden_id).await?;
    let rows = pg::list_vertraege_by_kunde(&ctx.pool, kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    ok(rows)
}

/// Bind every billing read to the same supply, including ended contracts.
#[must_use = "missing and ambiguous supplies require distinct responses"]
async fn billing_component(
    ctx: &Ctx,
    malo_id: &str,
    issuer: &str,
    from: Date,
    to: Date,
) -> ApiResult<pg::VertragskomponenteRow> {
    if from > to {
        return Err(ApiError::bad_request("from must not be after to"));
    }
    let mut matches =
        pg::supply_components(&ctx.pool, ctx.tenant(), malo_id, Some(issuer), from, to).await?;
    if matches.len() > 1 {
        return Err(ApiError::conflict(
            "multiple supply components overlap the issuer's billing period",
        )
        .with_code("ambiguous_supply_period"));
    }
    matches
        .pop()
        .filter(|k| k.lieferbeginn <= from && k.lieferende.is_none_or(|end| end >= to))
        .ok_or(ApiError::NotFound)
}

/// `GET /api/v1/vertraege/by-malo/{malo_id}` — the active contract behind a MaLo.
///
/// The lookup `billingd` uses for the § 40 Abs. 1 EnWG contract facts an
/// invoice must state. Besides the contract row it computes the next possible
/// Kündigungstermin under the rules that actually apply to this contract, and
/// resolves the BG-7 buyer so the e-invoice has a real addressee.
///
/// # `?stichtag=YYYY-MM-DD`
///
/// The date the Kündigungsfrist is measured from; today when absent. `processd`
/// passes the **Eingangsdatum der Kündigung**, because that is what `E_0614`
/// Prüfschritt 70 names: „unter Einhaltung der Kündigungsfrist unter
/// Berücksichtigung des Eingangsdatums der Kündigung". Inside the one-Werktag
/// answer window the two usually agree; across a month boundary they do not,
/// and the answer would otherwise state a date a whole notice period out.
#[utoipa::path(
    get,
    path = "/api/v1/vertraege/by-malo/{malo_id}",
    tag = "vertraege",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), ByMaloQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 400, description = "Invalid supply selection", body = ApiError),
        (status = 409, description = "Ambiguous supply period", body = ApiError),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn by_malo(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(malo_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<ByMaloQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    let stichtag = match q.stichtag.as_deref() {
        Some(raw) => Some(
            Date::parse(raw, &time::format_description::well_known::Iso8601::DATE).map_err(
                |_| ApiError::BadRequest(format!("\"stichtag\" is not an ISO-8601 date: {raw:?}")),
            )?,
        ),
        None => None,
    };
    let (vertrag, komponente) = match (q.lf_mp_id.as_deref(), q.from, q.to) {
        (Some(issuer), Some(from), Some(to)) => {
            let component = billing_component(&ctx, &malo_id, issuer, from, to).await?;
            let contract = pg::fetch_vertrag(&ctx.pool, component.vertrag_id, ctx.tenant())
                .await
                .map_err(ApiError::Internal)?
                .ok_or(ApiError::NotFound)?;
            (contract, component)
        }
        (None, None, None) => pg::fetch_vertrag_by_malo(&ctx.pool, &malo_id, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?
            .ok_or(ApiError::NotFound)?,
        _ => {
            return Err(ApiError::bad_request(
                "supply lookup requires lf_mp_id, from and to together",
            ));
        }
    };
    let kunde = pg::fetch_kunde(&ctx.pool, vertrag.kunden_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?;
    let haushaltskunde = kunde.as_ref().is_none_or(|k| k.haushaltskunde);
    let frist = naechster_kuendigungstermin(&vertrag, haushaltskunde, stichtag);

    let rechnungsempfaenger = kunde.map(pg::rechnungsempfaenger_of);

    // `E_0624` Prüfschritt 50 — „Ist der Kunde aus der Anfrage identisch mit
    // dem Kunden beim LFA?" Only `vertragd` knows the contract holder, and the
    // comparison is a domain rule rather than a string equality: the two sides
    // never agree on shape. Absent `?kunde=` it stays `null`, which
    // `processd` reads as unknown and escalates.
    let identitaet = q.kunde.as_deref().map(|aus_anfrage| {
        domain::kundenidentitaet(
            Some(aus_anfrage),
            rechnungsempfaenger.as_ref().and_then(|r| r.name.as_deref()),
        )
    });

    ok(serde_json::json!({
        "vertrag": vertrag,
        "komponente": komponente,
        "rechnungsempfaenger": rechnungsempfaenger,
        "naechstmoeglicher_kuendigungstermin": frist.fruehestens.to_string(),
        "kuendigungsfrist": frist,
        "kunde_identisch_mit_anfrage": identitaet.and_then(domain::Kundenidentitaet::as_option),
        "kundenidentitaet": identitaet,
    }))
}

/// The next date this contract could end if notice arrived today.
///
/// An open-ended contract ends on its notice period; a fixed-term one no
/// earlier than its own end.
fn naechster_kuendigungstermin(
    vertrag: &pg::VersorgungsvertragRow,
    haushaltskunde: bool,
    stichtag: Option<Date>,
) -> domain::Kuendigungsfrist {
    let mut frist = domain::kuendigungsfrist(
        stichtag.unwrap_or_else(heute),
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

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProdukteQuery {
    /// Invoice issuer for a supply-specific period read.
    #[param(value_type = Option<String>)]
    pub lf_mp_id: Option<rubo4e::identifiers::MarktpartnerId>,
    /// Point form: the product in force on this day. Defaults to today.
    pub as_of: Option<Date>,
    /// Period form (with `to`): every slice covering `[from, to]`.
    pub from: Option<Date>,
    /// Period form (with `from`), inclusive.
    pub to: Option<Date>,
}

/// Period of `GET /api/v1/malo/{malo_id}/sect14a`, both ends inclusive.
#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct Sect14aQuery {
    /// Invoice issuer for a supply-specific period read.
    #[param(value_type = Option<String>)]
    pub lf_mp_id: Option<rubo4e::identifiers::MarktpartnerId>,
    pub from: Date,
    pub to: Date,
}

/// `GET /api/v1/malo/{malo_id}/sect14a?from=&to=` — the § 14a EnWG module
/// state of the MaLo's supply over a period, as ordered slices.
///
/// `billingd` selects the Modul 1/2/3 reductions it bills from this answer. A
/// period no slice covers has no recorded state, and one two slices share
/// changed module inside it; the caller sees both rather than a single value.
#[utoipa::path(
    get,
    path = "/api/v1/malo/{malo_id}/sect14a",
    tag = "vertraege",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), Sect14aQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 400, description = "Invalid supply selection", body = ApiError),
        (status = 409, description = "Ambiguous supply period", body = ApiError),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn malo_sect14a(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(malo_id): Path<String>,
    Query(q): Query<Sect14aQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    if q.from > q.to {
        return Err(ApiError::bad_request("from must not be after to"));
    }
    let component = match q.lf_mp_id.as_deref() {
        Some(issuer) => Some(
            billing_component(&ctx, &malo_id, issuer, q.from, q.to)
                .await?
                .id,
        ),
        None => None,
    };
    let slices =
        pg::malo_sect14a_for_component(&ctx.pool, ctx.tenant(), &malo_id, q.from, q.to, component)
            .await
            .map_err(ApiError::Internal)?;
    ok(serde_json::json!({
        "malo_id": malo_id,
        "from": q.from.to_string(),
        "to": q.to.to_string(),
        "slices": slices,
    }))
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct IssuerQuery {
    /// Select the issuer when a MaLo has competing supply components.
    #[param(value_type = Option<String>)]
    pub lf_mp_id: Option<rubo4e::identifiers::MarktpartnerId>,
}

/// `POST /api/v1/malo/{malo_id}/sect14a` — record the § 14a EnWG modules the
/// Anschlussnutzer holds from `gueltig_ab` on.
///
/// The slice in force on that day ends there and the new one opens; a later
/// recorded state is `409`, since history is not rewritten here.
#[utoipa::path(
    post,
    path = "/api/v1/malo/{malo_id}/sect14a",
    tag = "vertraege",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), IssuerQuery),
    request_body(content = serde_json::Value, description = "`Sect14aInput`"),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 409, description = "Ambiguous supply or later module state", body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn record_sect14a(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(malo_id): Path<String>,
    Query(q): Query<IssuerQuery>,
    Json(input): Json<pg::Sect14aInput>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "write-netzentgeltmodule", ctx.tenant())?;
    match pg::record_sect14a_for_issuer(
        &ctx.pool,
        ctx.tenant(),
        &malo_id,
        q.lf_mp_id.as_deref(),
        &input,
    )
    .await
    {
        Ok(module) => ok(serde_json::json!({
            "malo_id": malo_id,
            "module": module,
            "gueltig_ab": input.gueltig_ab.to_string(),
        })),
        Err(pg::Sect14aError::KeineKomponente(_)) => Err(ApiError::NotFound),
        Err(e @ pg::Sect14aError::MehrdeutigeKomponente) => {
            Err(ApiError::conflict(e.to_string()).with_code("ambiguous_supply_period"))
        }
        Err(e @ pg::Sect14aError::KeinModul) => Err(ApiError::unprocessable(e.to_string())),
        Err(e @ pg::Sect14aError::SpaetererStand(_)) => Err(ApiError::conflict(e.to_string())),
        Err(pg::Sect14aError::Db(e)) => Err(ApiError::Internal(e.into())),
    }
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
#[utoipa::path(
    get,
    path = "/api/v1/malo/{malo_id}/produkte",
    tag = "vertraege",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID"), ProdukteQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 400, description = "Invalid supply selection", body = ApiError),
        (status = 409, description = "Ambiguous supply period", body = ApiError),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn malo_produkte(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(malo_id): Path<String>,
    Query(q): Query<ProdukteQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    if q.from.is_some() != q.to.is_some()
        || (q.as_of.is_some() && q.from.is_some())
        || (q.lf_mp_id.is_some() && q.from.is_none())
    {
        return Err(ApiError::bad_request(
            "use as_of or a complete from/to period; issuer selection requires a period",
        ));
    }
    if let (Some(from), Some(to)) = (q.from, q.to) {
        if from > to {
            return Err(ApiError::bad_request("from must not be after to"));
        }
        let component = match q.lf_mp_id.as_deref() {
            Some(issuer) => Some(
                billing_component(&ctx, &malo_id, issuer, from, to)
                    .await?
                    .id,
            ),
            None => None,
        };
        let slices =
            pg::malo_slices_for_component(&ctx.pool, ctx.tenant(), &malo_id, from, to, component)
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
#[utoipa::path(
    get,
    path = "/api/v1/vertraege/{id}/kuendigungsfrist",
    tag = "vertraege",
    params(("id" = uuid::Uuid, Path, description = "The resource's id"), KuendigungsfristQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn kuendigungsfrist(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Query(q): Query<KuendigungsfristQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
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

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct KuendigungsfristQuery {
    /// When the notice arrives; defaults to today.
    pub eingang: Option<Date>,
    /// For a § 41 Abs. 5 Satz 4 Sonderkündigung.
    pub preisanpassung_wirksam_zum: Option<Date>,
}

/// Inclusive range of ended supplies to enumerate beside the active roster.
#[derive(Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct CandidateWindow {
    pub ended_from: Option<Date>,
    pub ended_through: Option<Date>,
}

/// `GET /api/v1/vertraege/billing-candidates` — § 40b EnWG billing cadence feed.
#[utoipa::path(
    get,
    path = "/api/v1/vertraege/billing-candidates",
    params(CandidateWindow),
    tag = "vertraege",
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
    )
)]
pub async fn billing_candidates(
    Query(window): Query<CandidateWindow>,
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    let today = mako_fristen::heute();
    let span = i64::from(pg::vertraege::ENDED_SUPPLY_WINDOW_DAYS);
    let (from, through) =
        match (window.ended_from, window.ended_through) {
            (None, None) => (today - time::Duration::days(span), today),
            (Some(from), Some(through))
                if from <= through && through <= today && (through - from).whole_days() <= span =>
            {
                (from, through)
            }
            _ => return Err(ApiError::bad_request(
                "supply both dates in order, at most 84 days apart and ending no later than today",
            )
            .with_code("invalid_candidate_window")),
        };
    let rows = pg::billing_candidates_in_window(&ctx.pool, ctx.tenant(), from, through)
        .await
        .map_err(ApiError::Internal)?;
    ok(
        serde_json::json!({ "count": rows.len(), "candidates": rows, "ended_from":from, "ended_through":through }),
    )
}

#[derive(Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ExpiringQuery {
    /// Calendar days to look ahead. Default 30.
    pub days: Option<i64>,
}

/// `GET /api/v1/vertraege/expiring` — contracts whose term or price guarantee
/// runs out soon.
#[utoipa::path(
    get,
    path = "/api/v1/vertraege/expiring",
    tag = "vertraege",
    params(ExpiringQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn expiring(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<ExpiringQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
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
/// `de.mako.vertrag.kuendigung` CloudEvent — which carries the § 41 Abs. 8 Nr. 2
/// EnWG Textform confirmation the supplier owes the customer — goes into the
/// outbox.
#[utoipa::path(
    post,
    path = "/api/v1/vertraege/{id}/kuendigen",
    tag = "vertraege",
    operation_id = "kuendigen_vertrag",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    request_body(content = serde_json::Value, description = "`KuendigungInput`"),
    responses(
        (status = 202, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn kuendigen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(input): Json<pg::KuendigungInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&enforcer, &claims, "kuendigen-vertrag", ctx.tenant())?;
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
    let result = pg::kuendige_vertrag(&mut tx, &vertrag, &input, eingang)
        .await
        .map_err(ApiError::Internal)?;

    // § 41 Abs. 8 Nr. 2 EnWG obliges the supplier to confirm receipt of the
    // Kündigung to the customer in Textform. The document itself is produced
    // downstream; what has to be guaranteed here is that the instruction to
    // produce it survives — so it commits with the termination and the
    // timestamp records that the obligation was discharged.
    let ce = events::typed_event(
        id,
        ctx.tenant(),
        &events::Kuendigung {
            vertrag_id: id,
            vertrags_nr: vertrag.vertrags_nr.clone(),
            kunden_id: vertrag.kunden_id,
            lieferende: input.lieferende.to_string(),
            eingang: eingang.to_string(),
            grund: input.grund,
            frist: frist.frist.clone(),
            rechtsgrundlage: frist.rechtsgrundlage,
            mako_dispatched: result.dispatched.len(),
            kuendigungsbestaetigung: events::Bestaetigung::zum(input.lieferende),
        },
    )
    .map_err(|e| ApiError::Internal(e.into()))?;
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
/// Allowed while the Lieferende is still ahead. The queued Lieferende UTILMD
/// and Schlussablesung are parked in the same transaction; one that already
/// reached `processd` is the operator's to cancel there, and the response
/// names its component in `lieferende_bereits_versandt`.
#[utoipa::path(
    post,
    path = "/api/v1/vertraege/{id}/widerruf-kuendigung",
    tag = "vertraege",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
    )
)]
pub async fn widerruf_kuendigung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "kuendigen-vertrag", ctx.tenant())?;
    let mut tx = ctx.pool.begin().await.map_err(anyhow_from)?;
    let outcome = match pg::widerruf_kuendigung(&mut tx, id, ctx.tenant()).await {
        Ok(o) => o,
        Err(e) => {
            let msg = e.to_string();
            return Err(if msg.contains("not found") {
                ApiError::NotFound
            } else {
                ApiError::conflict(msg)
            });
        }
    };
    let ce = events::typed_event(
        id,
        ctx.tenant(),
        &events::KuendigungWiderrufen { vertrag_id: id },
    )
    .map_err(|e| ApiError::Internal(e.into()))?;
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    tx.commit().await.map_err(anyhow_from)?;
    let versandt = &outcome.lieferende_bereits_versandt;
    ok(serde_json::json!({
        "vertrag_id": id,
        "status": "AKTIV",
        // The queued Lieferende and Schlussablesung are parked; only one that
        // already reached processd needs the operator.
        "lieferende_bereits_versandt": versandt,
        "message": if versandt.is_empty() {
            "Kündigung widerrufen — kein Lieferende-UTILMD hatte die Warteschlange verlassen"
        } else {
            "Kündigung widerrufen — das bereits versandte Lieferende-UTILMD ist über \
             processd zu stornieren"
        },
    }))
}

/// `POST /api/v1/vertraege/{id}/stornieren` — cancel before supply began.
///
/// Valid for `ANGELEGT` and `IN_BEARBEITUNG`. A registration still waiting in
/// the outbound queue is withdrawn with it; one already sent to `processd` has
/// to be cancelled there, and the response says which case this was.
#[utoipa::path(
    post,
    path = "/api/v1/vertraege/{id}/stornieren",
    tag = "vertraege",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
    )
)]
pub async fn stornieren(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "stornieren-vertrag", ctx.tenant())?;
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
/// ## Two different acts
///
/// `initiator` is required, because the law treats them differently.
/// § 41 Abs. 5 Satz 1 EnWG binds a supplier who exercises a reserved right to
/// change the contract, and Satz 4 gives the customer a fee-free termination
/// right *because* the supplier exercised it. A tariff the customer asked for
/// is an agreed change: it is confirmed, it carries no Sonderkündigungsrecht,
/// and it is not held to the supplier's notice periods.
///
/// ## What is enforced for a supplier-initiated future change
///
/// - **§ 41 Abs. 5 Satz 3 EnWG.** `preise[]` states the Umfang of the change.
///   Required where this deployment renders the Preisänderungsanzeige itself
///   (`outputd_url` configured), because the lines are the document's content.
///   Where the CloudEvent is the notice, the ERP composing the letter states
///   the Umfang from its own price sheets, and the lines are optional — sent
///   along when the caller has them, and marked absent on the event when not.
/// - **§ 41 Abs. 5 Satz 2 EnWG.** The customer is owed a month's notice (two
///   weeks outside households); a Grundversorgungspreis needs the six weeks of
///   § 5 Abs. 2 StromGVV/GasGVV. A Wirksamkeit too close to today is refused
///   *here*, so the notice period cannot be missed by scheduling.
/// - **§ 5 Abs. 2 GVV.** A Grundversorgungspreis changes only at the start of a
///   month.
///
/// ## What is enforced for every Tarifwechsel
///
/// - **Preisgarantie.** A change taking effect inside the guarantee window is
///   refused unless the operator overrides it, and every override is written to
///   `preisgarantie_override_log` with the operator's token subject.
///
/// A retroactive correction (`wirksamkeit <= today`) is exempt from the notice
/// rules — it is not an announced price change but the repair of one already
/// agreed.
#[utoipa::path(
    post,
    path = "/api/v1/vertraege/{id}/tarifwechsel",
    tag = "vertraege",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    request_body(content = serde_json::Value, description = "`TarifwechselInput`"),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn tarifwechsel(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(vertrag_id): Path<Uuid>,
    Json(input): Json<pg::TarifwechselInput>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "tarifwechsel-vertrag", ctx.tenant())?;
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
    // Only the supplier exercising a change right owes the § 41 Abs. 5 notice,
    // and only a change still ahead of its Wirksamkeit can be announced.
    let anzeigepflichtig = is_future && input.initiator.owes_preisanpassungsanzeige();

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

    // ── The announceable Umfang (§ 41 Abs. 5 Satz 3 EnWG) ────────────────────
    // The requirement is on the notice, so it binds whoever composes it. Where
    // this deployment renders the Preisänderungsanzeige itself, the lines *are*
    // the page's content: without them the sweep would produce a document that
    // states no Umfang, so the change is refused here instead. Where the
    // CloudEvent is the notice, the ERP receiving it composes the letter from
    // its own price sheets and states the Umfang there — the lines are welcome
    // and travel with the event, but demanding them would make that integration
    // impossible without telling any customer anything more.
    if anzeigepflichtig && input.preise.is_empty() && ctx.cfg.outputd_url.is_some() {
        return Err(unprocessable_json(serde_json::json!({
            "error": "der Umfang der Preisänderung fehlt",
            "hinweis": "preise[] mit {bezeichnung, einheit, bisher, neu} je geänderter \
                        Preiskomponente angeben — diese Instanz erstellt die \
                        Preisänderungsanzeige selbst und kann den Umfang nur aus ihnen \
                        angeben",
            "rechtsgrundlage": "§ 41 Abs. 5 Satz 3 EnWG",
        })));
    }

    // ── Notice period (§ 41 Abs. 5 EnWG / § 5 Abs. 2 GVV) ────────────────────
    // Both rules bind the supplier changing its own prices. A tariff the
    // customer asked for is an agreed change and is not held to them.
    if anzeigepflichtig {
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
    // and nothing to apply later.
    //
    // The notice is owed only where the supplier exercises a change right and
    // the change has not happened yet; a retroactive correction announces a
    // date that has already passed, and a switch the customer asked for is
    // confirmed rather than announced.
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
        input.initiator,
        !anzeigepflichtig,
        preise.as_ref(),
    )
    .await
    .map_err(|e| ApiError::conflict(e.to_string()))?;

    let payload = events::Tarifwechsel {
        vertrag_id,
        komp_id: input.komp_id,
        malo_id: komp.malo_id,
        old_product_code: bisheriges_produkt,
        new_product_code: input.new_product_code.clone(),
        wirksamkeit: input.wirksamkeit.to_string(),
        geplant: is_future,
        initiator: input.initiator,

        preisanpassungsanzeige_erforderlich: anzeigepflichtig,
        sonderkuendigungsrecht: if anzeigepflichtig {
            events::Sonderkuendigungsrecht::zum(input.wirksamkeit)
        } else {
            events::Sonderkuendigungsrecht::Keines { besteht: false }
        },
    };
    let ce = if is_future {
        events::typed_event(
            vertrag_id,
            ctx.tenant(),
            &events::TarifwechselGeplant(payload),
        )
    } else {
        events::typed_event(vertrag_id, ctx.tenant(), &payload)
    }
    .map_err(|e| ApiError::Internal(e.into()))?;
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
        "initiator": input.initiator,
        "preisanpassungsanzeige_erforderlich": anzeigepflichtig,
        "ankuendigungsfrist": if anzeigepflichtig { Some(regime) } else { None },
    }))
}

// ── Preisgarantie ─────────────────────────────────────────────────────────────

/// `PUT /api/v1/vertraege/{id}/preisgarantie` — store the BO4E `Preisgarantie`.
///
/// The body is a `Bo4e<Preisgarantie>`, so the gate runs as it deserialises —
/// for a `Preisgarantie` that includes the `Zeitraum` in `zeitlicheGueltigkeit`,
/// the very field `preisgarantie_bis` is derived from below and the one the
/// Tarifwechsel guard reads. A key BO4E does not define is refused
/// ([`Bo4e::ensure_no_undefined_fields`](mako_markt::bo4e::Bo4e::ensure_no_undefined_fields)): a misspelt `zeitlicheGueltigkeit`
/// would otherwise store a guarantee with no end date.
///
/// Deriving `preisgarantie_bis` from the stored document means the Tarifwechsel
/// guard and the document the customer holds can never disagree.
#[utoipa::path(
    put,
    path = "/api/v1/vertraege/{id}/preisgarantie",
    tag = "vertraege",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    request_body(content = serde_json::Value, description = "`Bo4e<Preisgarantie>`"),
    responses(
        (status = 204, description = "Done"),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
        (status = 422, description = "A refusal naming the rule that decided", body = ApiError),
    )
)]
pub async fn put_preisgarantie(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(vertrag_id): Path<Uuid>,
    Json(preisgarantie): Json<Bo4e<rubo4e::current::Preisgarantie>>,
) -> ApiResult<StatusCode> {
    authorize(&enforcer, &claims, "write-preisgarantie", ctx.tenant())?;
    refuse_undefined_fields(&preisgarantie)?;
    let canonical = canonical_bo4e(&preisgarantie)?;
    let bis = canonical
        .pointer("/zeitlicheGueltigkeit/enddatum")
        .and_then(serde_json::Value::as_str)
        .map(|s| mako_service::dates::parse_iso_date("zeitlicheGueltigkeit.enddatum", s))
        .transpose()?;

    let mut tx = ctx.pool.begin().await.map_err(anyhow_from)?;
    if !pg::upsert_preisgarantie(&mut *tx, vertrag_id, ctx.tenant(), &canonical, bis)
        .await
        .map_err(ApiError::Internal)?
    {
        return Err(ApiError::NotFound);
    }
    let ce = events::typed_event(
        vertrag_id,
        ctx.tenant(),
        &events::PreisgarantieHinterlegt {
            vertrag_id,
            preisgarantie_bis: bis.map(|d| d.to_string()),
        },
    )
    .map_err(|e| ApiError::Internal(e.into()))?;
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    tx.commit().await.map_err(anyhow_from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/vertraege/{id}/preisgarantie`
#[utoipa::path(
    get,
    path = "/api/v1/vertraege/{id}/preisgarantie",
    tag = "vertraege",
    params(("id" = uuid::Uuid, Path, description = "The resource's id")),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, body = ApiError),
    )
)]
pub async fn get_preisgarantie(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(vertrag_id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    pg::fetch_preisgarantie(&ctx.pool, vertrag_id, ctx.tenant())
        .await
        .map_err(ApiError::Internal)?
        .map(Json)
        .ok_or(ApiError::NotFound)
}

fn anyhow_from(e: sqlx::Error) -> ApiError {
    ApiError::Internal(anyhow::Error::new(e))
}

// ── The sales edge ────────────────────────────────────────────────────────────

/// Query of `GET /api/v1/lieferbeginn/fruehestens`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FruehestensQuery {
    /// `STROM` or `GAS`.
    pub sparte: String,
    /// `WECHSEL` (default), `UMZUG`, `NEUANLAGE` or `ENDE_ERSATZVERSORGUNG` —
    /// only a Gas Wechsel carries a lead time of its own.
    #[serde(default)]
    #[param(inline)]
    pub lieferbeginn_anlass: Option<domain::Lieferbeginnanlass>,
    /// § 13 BGB — a consumer has a Widerrufsrecht. Default `true`, the
    /// tighter reading.
    #[serde(default)]
    pub verbraucher: Option<bool>,
    /// § 357a Abs. 2 Satz 1 Nr. 1 BGB — the express request to start supply
    /// inside the Widerrufsfrist.
    #[serde(default)]
    pub lieferung_in_widerrufsfrist_verlangt: bool,
    /// The Widerrufsbelehrung is shown before the order.
    #[serde(default)]
    pub belehrung_angezeigt: bool,
}

/// `GET /api/v1/lieferbeginn/fruehestens` — the earliest Lieferbeginn an order
/// placed today may name, and each bound that sets it.
///
/// The answer is what the order route and processd's Anmeldung check accept
/// today: both compute it with the same function.
#[utoipa::path(
    get,
    path = "/api/v1/lieferbeginn/fruehestens",
    tag = "vertraege",
    params(FruehestensQuery),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "A Sparte other than STROM or GAS", body = ApiError),
    )
)]
pub async fn fruehester_lieferbeginn(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Query(q): Query<FruehestensQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "read-vertrag", ctx.tenant())?;
    if !matches!(q.sparte.as_str(), "STROM" | "GAS") {
        return Err(
            ApiError::unprocessable("sparte must be STROM or GAS").with_code("sparte_ungueltig")
        );
    }
    let heute = heute();
    let f = super::bestellungen::fruehester_lieferbeginn(
        ctx.cfg.policy.lieferbeginn_in_widerrufsfrist.get(),
        &q.sparte,
        q.lieferbeginn_anlass
            .is_none_or(|a| a == domain::Lieferbeginnanlass::Wechsel),
        q.verbraucher.unwrap_or(true),
        q.lieferung_in_widerrufsfrist_verlangt,
        q.belehrung_angezeigt,
        heute,
    );
    ok(serde_json::json!({
        "fruehestens": f.datum.to_string(),
        "stand": heute.to_string(),
        "grenzen": f.grenzen.iter().map(|g| serde_json::json!({
            "regel": g.regel,
            "datum": g.datum.to_string(),
            "quelle": g.quelle,
        })).collect::<Vec<_>>(),
    }))
}

/// `POST /api/v1/vertraege/{id}/komponenten/{komp}/kuendigung-vorversorger/freigeben`
/// — send the Anmeldung a refused Kündigung beim Vorversorger is holding.
///
/// The person releasing it and the instant are recorded on the component.
#[utoipa::path(
    post,
    path = "/api/v1/vertraege/{id}/komponenten/{komp}/kuendigung-vorversorger/freigeben",
    tag = "vertraege",
    params(
        ("id" = Uuid, Path, description = "The contract"),
        ("komp" = Uuid, Path, description = "The component"),
    ),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 409, description = "`kuendigung_nicht_gehalten` — no Anmeldung is held", body = ApiError),
    )
)]
pub async fn kuendigung_vorversorger_freigeben(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((id, komp)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "create-vertrag", ctx.tenant())?;
    gehaltene_anmeldung(&ctx, id, komp, claims.sub(), true).await
}

/// `POST /api/v1/vertraege/{id}/komponenten/{komp}/kuendigung-vorversorger/zurueckziehen`
/// — withdraw a component whose Anmeldung a refused Kündigung is holding.
///
/// Nothing reached the Netzbetreiber, so nothing is sent: the component is
/// `STORNIERT` and the held Anmeldung dropped.
#[utoipa::path(
    post,
    path = "/api/v1/vertraege/{id}/komponenten/{komp}/kuendigung-vorversorger/zurueckziehen",
    tag = "vertraege",
    params(
        ("id" = Uuid, Path, description = "The contract"),
        ("komp" = Uuid, Path, description = "The component"),
    ),
    responses(
        (status = 200, body = serde_json::Value),
        (status = 401, description = "No valid token", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 409, description = "`kuendigung_nicht_gehalten` — no Anmeldung is held", body = ApiError),
    )
)]
pub async fn kuendigung_vorversorger_zurueckziehen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path((id, komp)): Path<(Uuid, Uuid)>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "stornieren-vertrag", ctx.tenant())?;
    gehaltene_anmeldung(&ctx, id, komp, claims.sub(), false).await
}

/// Release (`freigeben`) or withdraw a held Anmeldung, recording who did it.
async fn gehaltene_anmeldung(
    ctx: &Ctx,
    vertrag_id: Uuid,
    komp_id: Uuid,
    aktor: &str,
    freigeben: bool,
) -> ApiResult<Json<serde_json::Value>> {
    use crate::kuendigung_vorversorger::KuendigungStatus;
    let internal = |e: sqlx::Error| ApiError::Internal(e.into());
    let mut tx = ctx.pool.begin().await.map_err(internal)?;
    let gehalten: Option<(String, Date, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT vorversorger_mp_id, kuendigung_zum, kuendigung_wartende_anmeldung
           FROM vertragskomponenten
          WHERE id = $1 AND vertrag_id = $2 AND tenant = $3
            AND kuendigung_wartende_anmeldung IS NOT NULL
            AND kuendigung_status IN ('ABGELEHNT','BESTAETIGT')
          FOR UPDATE",
    )
    .bind(komp_id)
    .bind(vertrag_id)
    .bind(ctx.tenant())
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;
    let Some((vorversorger_mp_id, kuendigung_zum, anmeldung)) = gehalten else {
        return Err(ApiError::conflict("this component holds no Anmeldung")
            .with_code("kuendigung_nicht_gehalten"));
    };
    let status = if freigeben {
        KuendigungStatus::Freigegeben
    } else {
        KuendigungStatus::Zurueckgezogen
    };
    sqlx::query(
        "UPDATE vertragskomponenten
            SET kuendigung_status = $2, kuendigung_aktor = $3, kuendigung_aktion_am = now(),
                status = CASE WHEN $4 THEN status ELSE 'STORNIERT' END,
                kuendigung_wartende_anmeldung = CASE WHEN $4 THEN kuendigung_wartende_anmeldung END,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(komp_id)
    .bind(status.as_db())
    .bind(aktor)
    .bind(freigeben)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    if freigeben {
        super::market::anmeldung_freigeben(
            &mut tx,
            ctx.tenant(),
            &super::market::Wartend {
                komp_id,
                vorversorger_mp_id,
                kuendigung_zum,
                anmeldung,
            },
        )
        .await?;
    } else {
        let alle = pg::list_komponenten(&mut *tx, vertrag_id)
            .await
            .map_err(ApiError::Internal)?;
        pg::update_vertrag_status(
            &mut *tx,
            vertrag_id,
            ctx.tenant(),
            pg::derive_vertrag_status(&alle),
        )
        .await
        .map_err(ApiError::Internal)?;
    }
    tx.commit().await.map_err(internal)?;
    ok(serde_json::json!({
        "vertrag_id": vertrag_id,
        "komponente_id": komp_id,
        "kuendigung_status": status.as_db(),
    }))
}
