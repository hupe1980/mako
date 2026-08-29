//! ESA consent-registry REST handlers (§49 Abs. 2 Nr. 9 MsbG).
//!
//! Routes:
//!   POST   /api/v1/esa/einwilligungen                 — grant consent
//!   GET    /api/v1/esa/einwilligungen?esa_mp_id=…      — list active consents
//!   GET    /api/v1/esa/einwilligungen/:id             — fetch one
//!   DELETE /api/v1/esa/einwilligungen/:id             — revoke (Art. 7(3) GDPR)
//!   PUT    /api/v1/esa/framework/:msb_mp_id/:esa_mp_id — upsert framework agreement
//!   GET    /api/v1/esa/framework/:msb_mp_id/:esa_mp_id — fetch framework agreement
//!
//! **Evidence-agnostic** (BNetzA forbids rejecting consent for deviating from
//! the BDEW template): `evidence_uri`/`evidence_hash` are stored verbatim and
//! never validated for form.
//!
//! Revocation emits `de.markt.einwilligung.widerrufen` and fires the **17008
//! Abbestellung** at makod — GDPR Art. 7(3) obliges the ESA to stop, and the
//! only way to stop is the Abbestellung.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    makod_client::MakodClient,
    repository::{
        ConsentPerspective, EinwilligungRecord, EinwilligungRepository, EsaFrameworkAgreement,
        EsaMessproduktAngebot, EsaMessproduktPreis,
    },
};
use serde::Deserialize;
use uuid::Uuid;

use mako_service::cedar::CedarEnforcer;

use super::{Claims, IntoMdmResponse as _, Tenant};

/// Deny response for the consent registry.
///
/// Both actions are gated on the MSB or ESA role: the registry records the
/// relationship between exactly those two parties, and a revocation fires the
/// ORDERS 17008 Abbestellung at makod. An LF has no lawful interest in it.
fn forbidden(action: &'static str) -> axum::response::Response {
    mako_markt::error::MdmError::Forbidden { reason: action }.into_response()
}

/// Injected `Arc<PgEinwilligungRepository>`.
pub type EinwilligungRepoExt = Arc<crate::pg::PgEinwilligungRepository>;
async fn emit(
    pool: &sqlx::PgPool,
    notify: &tokio::sync::Notify,
    tenant: &str,
    ce_type: &str,
    subject: String,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let evt = MarktEvent::new(tenant, ce_type, subject, data);
    crate::outbox::enqueue(pool, &evt, notify).await
}

#[derive(Debug, Deserialize)]
pub struct GrantBody {
    pub anschlussnutzer_ref: String,
    pub esa_mp_id: String,
    pub location_ids: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    #[serde(default)]
    pub valid_to: Option<time::Date>,
    /// Opaque evidence — stored verbatim, never validated for form.
    #[serde(default)]
    pub evidence_uri: Option<String>,
    #[serde(default)]
    pub evidence_hash: Option<String>,
}

/// `POST /api/v1/esa/einwilligungen` — grant an ESA consent.
pub async fn grant_einwilligung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Json(body): Json<GrantBody>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    if body.location_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "location_ids must not be empty" })),
        )
            .into_response();
    }
    let rec = EinwilligungRecord {
        id: Uuid::nil(),
        tenant: tenant.clone(),
        anschlussnutzer_ref: body.anschlussnutzer_ref.clone(),
        esa_mp_id: body.esa_mp_id.clone(),
        location_ids: body.location_ids.clone(),
        scope: body.scope.unwrap_or_else(|| "werte".to_owned()),
        granted_at: time::OffsetDateTime::now_utc(),
        valid_from: body
            .valid_from
            .unwrap_or_else(|| time::OffsetDateTime::now_utc().date()),
        valid_to: body.valid_to,
        revoked_at: None,
        evidence_uri: body.evidence_uri,
        evidence_hash: body.evidence_hash,
    };
    match repo.grant(rec).await {
        Ok(id) => {
            if let Err(e) = emit(
                &pool,
                &notify,
                &tenant,
                mako_events::markt::EINWILLIGUNG_ERTEILT,
                id.to_string(),
                serde_json::json!({
                    "einwilligung_id": id,
                    "esa_mp_id": body.esa_mp_id,
                    "anschlussnutzer_ref": body.anschlussnutzer_ref,
                    "location_ids": body.location_ids,
                }),
            )
            .await
            {
                tracing::error!(error = %e, "einwilligung: durable enqueue failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            (StatusCode::CREATED, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub esa_mp_id: String,
}

/// `GET /api/v1/esa/einwilligungen?esa_mp_id=…` — list active consents.
pub async fn list_einwilligungen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo.list_for_esa(&tenant, &q.esa_mp_id).await {
        Ok(rows) => Json(serde_json::json!({
            "esa_mp_id": q.esa_mp_id,
            "count": rows.len(),
            "einwilligungen": rows,
        }))
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/esa/einwilligungen/:id`.
pub async fn get_einwilligung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo.get(&tenant, id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// `DELETE /api/v1/esa/einwilligungen/:id` — revoke consent (Art. 7(3) GDPR).
///
/// Emits `de.markt.einwilligung.widerrufen` and fires the **17008 Abbestellung**
/// at makod for the covered locations. The Abbestellung dispatch is best-effort
/// (logged on failure) — the revocation itself always succeeds and is the
/// durable signal a consumer can act on.
#[allow(clippy::too_many_arguments)] // axum extractors, not a parameter list
pub async fn revoke_einwilligung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Extension(makod): Extension<Arc<MakodClient>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    let revoked = match repo.revoke(&tenant, id).await {
        Ok(Some(rec)) => rec,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return e.into_response(),
    };

    if let Err(e) = emit(
        &pool,
        &notify,
        &tenant,
        mako_events::markt::EINWILLIGUNG_WIDERRUFEN,
        id.to_string(),
        serde_json::json!({
            "einwilligung_id": id,
            "esa_mp_id": revoked.esa_mp_id,
            "anschlussnutzer_ref": revoked.anschlussnutzer_ref,
            "location_ids": revoked.location_ids,
            // Which of the two ways the basis ended. An audit that cannot tell
            // a Widerruf from an Ablauf cannot answer „did anyone withdraw
            // consent this quarter" — see `consent_lifecycle`.
            "grund": "einwilligung_widerrufen",
        }),
    )
    .await
    {
        tracing::error!(error = %e, "einwilligung: widerrufen durable enqueue failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // GDPR Art. 7(3): stopping value delivery requires an Abbestellung (17008)
    // per covered location. The expiry sweep runs the same code — the two ways
    // a consent stops being a lawful basis owe the market the same message.
    crate::consent_lifecycle::stop_deliveries(&makod, "einwilligung_widerrufen", &revoked).await;

    StatusCode::NO_CONTENT.into_response()
}

#[derive(Debug, Deserialize)]
pub struct ConsentCheckQuery {
    pub esa_mp_id: String,
    pub msb_mp_id: String,
    pub location_id: String,
    /// `msb_inbound` (default, lenient) or `esa_outbound` (strict).
    #[serde(default)]
    pub perspective: ConsentPerspective,
}

/// `GET /api/v1/esa/consent-check?esa_mp_id=…&msb_mp_id=…&location_id=…&perspective=…`.
///
/// Gates an ESA message against the registry. Always answers `200` with a
/// [`ConsentDecision`](mako_markt::repository::ConsentDecision) — a revoked
/// consent or an unestablished framework agreement yields `allowed: false`, the
/// clearing case the caller answers with an Ablehnung.
///
/// `perspective` sets how a *missing* record is read: `msb_inbound` (default)
/// treats it as self-assertion and allows (`BNetzA` forbids form-based
/// rejection); `esa_outbound` treats it as no lawful basis and blocks — the ESA
/// is the consent holder and must not originate a request it has no consent for.
pub async fn consent_check(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<ConsentCheckQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo
        .consent_check(
            &tenant,
            &q.esa_mp_id,
            &q.msb_mp_id,
            &q.location_id,
            q.perspective,
        )
        .await
    {
        Ok(decision) => Json(decision).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct FrameworkBody {
    #[serde(default)]
    pub signed_at: Option<time::OffsetDateTime>,
    #[serde(default)]
    pub edi_agreement: bool,
    #[serde(default)]
    pub cert_state: Option<String>,
}

/// `PUT /api/v1/esa/framework/:msb_mp_id/:esa_mp_id`.
pub async fn put_framework(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    Json(body): Json<FrameworkBody>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    let rec = EsaFrameworkAgreement {
        tenant,
        msb_mp_id,
        esa_mp_id,
        signed_at: body.signed_at,
        edi_agreement: body.edi_agreement,
        cert_state: body.cert_state.unwrap_or_else(|| "pending".to_owned()),
    };
    match repo.upsert_framework(rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// Body of `PUT /api/v1/esa/preise/:msb_mp_id/:esa_mp_id`.
#[derive(Debug, Deserialize)]
pub struct EsaPreiseBody {
    /// Which subscription these prices were agreed for — a subscription is the
    /// (Meldepunkt, Messprodukt) pair.
    pub lokations_id: String,
    pub messprodukt: String,
    /// Belegnummer of the ORDERS 17007 the offer was accepted with.
    #[serde(default)]
    pub bestellung_ref: Option<String>,
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    #[serde(default)]
    pub valid_to: Option<time::Date>,
    /// `SG4 CUX` DE 6345.
    #[serde(default)]
    pub waehrung: Option<String>,
    /// The priced Artikel-IDs of the accepted offer.
    pub preise: Vec<EsaPreisPosition>,
}

/// One `SG27 PIA+Z02` / `SG31 PRI+CAL` pair of an accepted Angebot.
#[derive(Debug, Deserialize)]
pub struct EsaPreisPosition {
    pub artikel_id: String,
    /// `SG31 PRI` DE 5387 — `Z01` / `Z02` / `Z03`.
    pub preistyp: String,
    /// `SG31 PRI` DE 5118, up to six decimals.
    pub betrag: rust_decimal::Decimal,
    /// `SG31 PRI` DE 6411 — `H87` Stück or `DAY` Tag.
    pub einheit: String,
}

/// `PUT /api/v1/esa/preise/:msb_mp_id/:esa_mp_id` — record the prices of an
/// accepted QUOTES 15003 Angebot.
///
/// **The ESA price basis.** `PreisblattMessung` is what an MSB publishes toward
/// the NB and the LF; there is none for the Kapitel-4.6 Messprodukte, because
/// §35 MsbG leaves the Entgelt for a Zusatzleistung to be agreed per request.
/// So the offer the ESA ordered against is the agreement, and `invoicd` checks
/// the MSB's INVOIC 31009 positions against exactly these rows — the invoice
/// names the same Artikel-IDs back (`SG26 LIN` DE 7143 `Z09`).
///
/// Written by `makod` when the MSB confirms the Bestellung (ORDRSP 19011),
/// which is the moment the offer becomes binding.
pub async fn put_esa_preise(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    Json(body): Json<EsaPreiseBody>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    if body.preise.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "preise must not be empty — SG31 PRI is Muss inside the SG27 LIN                           position block, and an offer that prices nothing is the Ablehnung",
            })),
        )
            .into_response();
    }
    let waehrung = body.waehrung.unwrap_or_else(|| "EUR".to_owned());
    let rows: Vec<EsaMessproduktPreis> = body
        .preise
        .into_iter()
        .map(|p| EsaMessproduktPreis {
            tenant: tenant.clone(),
            esa_mp_id: esa_mp_id.clone(),
            msb_mp_id: msb_mp_id.clone(),
            lokations_id: body.lokations_id.clone(),
            messprodukt: body.messprodukt.clone(),
            artikel_id: p.artikel_id,
            preistyp: p.preistyp,
            betrag: p.betrag,
            einheit: p.einheit,
            waehrung: waehrung.clone(),
            bestellung_ref: body.bestellung_ref.clone(),
            valid_from: body.valid_from,
            valid_to: body.valid_to,
        })
        .collect();
    match repo.upsert_esa_preise(&rows).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/esa/preise/:msb_mp_id/:esa_mp_id?at=YYYY-MM-DD` — the prices in
/// force between the pair on `at`.
///
/// Across **all** of the pair's subscriptions, deliberately: an INVOIC 31009
/// bills a Rahmenvertrag rather than a single Meldepunkt, and its positions name
/// Artikel-IDs without saying which subscription each belongs to.
pub async fn get_esa_preise(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<EsaPreiseQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    let at =
        q.at.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    match repo
        .esa_preise_at(&tenant, &esa_mp_id, &msb_mp_id, at)
        .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Body of `PUT /api/v1/esa/messprodukte/:msb_mp_id` — one catalogue entry.
#[derive(Debug, Deserialize)]
pub struct KatalogEintrag {
    /// Messprodukt-Code from Codeliste der Konfigurationen 1.4 Kap. 4.6.
    /// Accepted in the published spaced form or digits only.
    pub messprodukt: String,
    /// `E_0256` Prüfschritt 4 — served as a turnusmäßige Übermittlung.
    #[serde(default = "yes")]
    pub als_abo: bool,
    /// `E_0256` Prüfschritt 5 — served as a single transmission.
    #[serde(default = "yes")]
    pub als_einmalig: bool,
    #[serde(default)]
    pub valid_from: Option<time::Date>,
    #[serde(default)]
    pub valid_to: Option<time::Date>,
}

const fn yes() -> bool {
    true
}

/// `PUT /api/v1/esa/messprodukte/:msb_mp_id` — record which Kapitel-4.6
/// Messprodukte this MSB serves an ESA, and in which Abo mode.
///
/// Answers `E_0252` Prüfschritt 2 and `E_0256` Prüfschritte 4/5, the one
/// commercial question in those walks that the Codeliste cannot: which of the
/// **optional** products this MSB carries. Without it every optional order
/// escalates to an operator.
///
/// A code outside Kapitel 4.6 is refused here rather than stored: the catalogue
/// of orderable products is code (`mako_wim::esa`), not data, and an entry for
/// a product the Marktrolle may not order could only ever mislead a walk.
pub async fn put_esa_messprodukt_katalog(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(msb_mp_id): Path<String>,
    Json(body): Json<Vec<KatalogEintrag>>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("write-einwilligung denied");
    }
    let mut eintraege = Vec::with_capacity(body.len());
    for e in body {
        let code = mako_wim::esa::normalize_code(&e.messprodukt);
        if mako_wim::esa::messprodukt(&code).is_none() {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!(
                        "Messprodukt {:?} steht nicht in der Codeliste der Konfigurationen 1.4                          Kapitel 4.6 — nur diese Produkte darf die Marktrolle ESA bestellen",
                        e.messprodukt
                    ),
                })),
            )
                .into_response();
        }
        eintraege.push(EsaMessproduktAngebot {
            tenant: tenant.clone(),
            msb_mp_id: msb_mp_id.clone(),
            messprodukt: code,
            als_abo: e.als_abo,
            als_einmalig: e.als_einmalig,
            valid_from: e.valid_from,
            valid_to: e.valid_to,
        });
    }
    match repo.upsert_esa_messprodukt_katalog(&eintraege).await {
        Ok(()) => (
            StatusCode::NO_CONTENT,
            Json(serde_json::json!({ "count": eintraege.len() })),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/esa/messprodukte/:msb_mp_id/:messprodukt?at=YYYY-MM-DD` — does
/// this MSB serve the product on `at`, and in which mode?
///
/// The answer folds in the **Pflicht** rule: BNetzA *Mitteilung Nr. 3* makes
/// seven Kapitel-4.6 products mandatory from a dated cut-over and §34 Abs. 2
/// S. 2 Nr. 10 MsbG makes serving an ESA a mandatory Zusatzleistung, so a
/// Pflichtprodukt is served whatever the catalogue holds. An operator cannot
/// refuse one by leaving the table empty.
pub async fn get_esa_messprodukt_angebot(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path((msb_mp_id, messprodukt)): Path<(String, String)>,
    Query(q): Query<EsaPreiseQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    let at =
        q.at.unwrap_or_else(|| time::OffsetDateTime::now_utc().date());
    let code = mako_wim::esa::normalize_code(&messprodukt);
    let Some(produkt) = mako_wim::esa::messprodukt(&code) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let eintrag = match repo
        .esa_messprodukt_angebot(&tenant, &msb_mp_id, &code, at)
        .await
    {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    // „Pflicht" is dated: `9991 00000 077 1` and `078 9` read „Optional ab
    // 01.10.2023, Pflicht ab 06.08.2024", and a Vergangenheitswerte-Bestellung
    // may reach back before the cut-over — where the MSB's discretion still
    // stood and the catalogue is the whole answer.
    let pflicht = produkt.ist_pflicht_am(at);
    Json(serde_json::json!({
        "messprodukt": code,
        "at": at.to_string(),
        "pflicht": pflicht,
        // `null` where nothing is on file and the product is optional: „not
        // carried" is a decision, „nothing recorded" is not, and the walks
        // escalate on the difference.
        "als_abo": if pflicht { Some(true) } else { eintrag.as_ref().map(|e| e.als_abo) },
        "als_einmalig": if pflicht { Some(true) } else { eintrag.as_ref().map(|e| e.als_einmalig) },
        "im_katalog": eintrag.is_some(),
    }))
    .into_response()
}

/// `GET /api/v1/esa/subscriptions/:bestellung_ref` — which **Messprodukt** an
/// ORDERS 17007 Belegnummer subscribed to.
///
/// `edmd`'s Typ-2 delivery surveillance is the caller. The Codeliste der
/// Konfigurationen publishes a delivery cadence **per Messprodukt** — the
/// Rohdaten products state „unverzüglich, jedoch spätestens bis 9:30 Uhr", the
/// aufbereitete-Daten ones defer to WiM Teil 2 Kap. 2.5.5 — but an inbound
/// MSCONS 13027 names only the Belegnummer of the ORDERS it belongs to
/// (`SG1 RFF+AGI`), never the product. Without this the sweep applies one flat
/// threshold to every subscription and alerts late on the fast ones.
///
/// `404` when no accepted Angebot names that Belegnummer — the ordinary state
/// for a delivery whose sender omitted the `RFF+AGI` Muss, and the signal to
/// fall back to the configured threshold rather than to invent a cadence.
pub async fn get_esa_subscription(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(bestellung_ref): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo
        .esa_messprodukt_of_bestellung(&tenant, &bestellung_ref)
        .await
    {
        Ok(Some(messprodukt)) => Json(serde_json::json!({
            "bestellung_ref": bestellung_ref,
            "messprodukt": messprodukt,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// Query of `GET /api/v1/esa/preise/…`.
#[derive(Debug, Deserialize)]
pub struct EsaPreiseQuery {
    /// The day the prices must be in force on. Defaults to today.
    #[serde(default)]
    pub at: Option<time::Date>,
}

/// `GET /api/v1/esa/framework/:msb_mp_id/:esa_mp_id`.
pub async fn get_framework(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<EinwilligungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path((msb_mp_id, esa_mp_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-einwilligung", &tenant)
        .is_err()
    {
        return forbidden("read-einwilligung denied");
    }
    match repo.get_framework(&tenant, &msb_mp_id, &esa_mp_id).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}
