//! Online Vertragsschluss: the operator's legal documents, the order with its
//! Bestellnachweis, the operator's acceptance, and the consumer's Widerruf.
//!
//! Every refusal carries a stable English `code`; the German sentence
//! a consumer reads is the front end's.

use std::sync::Arc;

use axum::{
    Extension,
    extract::Path,
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
};
use mako_service::{ApiError, ApiResult, Json, oidc::Claims};
use serde::Deserialize;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use super::{CedarEnforcer, Ctx, authorize, ok, require_vertrag};
use crate::{
    bestellung::{
        self, BestellungInput, DokumentArt, Wiederholung, fehlende_angaben, idempotency_codes,
    },
    config::{LieferbeginnInWiderrufsfrist, Vertragsschluss},
    dokumente::{self, PartyView},
    endkunden_angebot::{self, Abweichung, EndkundenAngebot, ProductdClient},
    outbound,
    pg::{
        self,
        bestellungen::{self as store, GespeicherterNachweis, OnlineVertragsschluss},
    },
};

const CAL: mako_fristen::HolidayCalendar = mako_fristen::HolidayCalendar::BdewMaKo;

fn internal(e: impl Into<anyhow::Error>) -> ApiError {
    ApiError::Internal(e.into())
}

// ── Rechtsdokumente ───────────────────────────────────────────────────────────

/// `PUT /api/v1/rechtsdokumente/{art}`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RechtsdokumentInput {
    /// The document's text, exactly as the consumer is shown it. Text, not a
    /// PDF: the Widerrufsbelehrung and the Muster-Widerrufsformular are
    /// printed into the Vertragsbestätigung from these bytes.
    pub text: String,
    pub gueltig_ab: Date,
}

/// `PUT /api/v1/rechtsdokumente/{art}` — register a version of an operator
/// legal document (§ 312i Abs. 1 Satz 1 Nr. 4 BGB). Append-only: the same bytes return the version
/// already held, new bytes are a new version.
pub async fn put_rechtsdokument(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(art): Path<String>,
    Json(input): Json<RechtsdokumentInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&enforcer, &claims, "pflegen-rechtsdokument", ctx.tenant())?;
    let art = DokumentArt::from_db(&art).ok_or_else(|| {
        ApiError::unprocessable(format!(
            "`{art}` is no legal-document kind — expected AGB, WIDERRUFSBELEHRUNG, \
             MUSTER_WIDERRUFSFORMULAR or DATENSCHUTZ"
        ))
        .with_code("rechtsdokument_art_unbekannt")
    })?;
    if input.text.trim().is_empty() {
        return Err(ApiError::unprocessable("text is empty").with_code("rechtsdokument_leer"));
    }
    let (row, neu) = store::insert_rechtsdokument(
        &ctx.pool,
        ctx.tenant(),
        art,
        input.text.as_bytes(),
        "text/plain; charset=utf-8",
        input.gueltig_ab,
    )
    .await
    .map_err(internal)?;
    Ok((
        if neu {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(serde_json::json!({
            "art": row.art, "version": row.version, "sha256": row.sha256,
            "gueltig_ab": row.gueltig_ab.to_string(),
        })),
    ))
}

/// `GET /api/v1/rechtsdokumente/{sha256}` — the bytes an order's hash names.
pub async fn get_rechtsdokument(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(sha256): Path<String>,
) -> ApiResult<impl IntoResponse> {
    authorize(&enforcer, &claims, "read-rechtsdokument", ctx.tenant())?;
    let row = store::fetch_rechtsdokument(&ctx.pool, ctx.tenant(), &sha256)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)?;
    Ok((
        [
            (header::CONTENT_TYPE, row.media_type.clone()),
            (header::ETAG, format!("\"{}\"", row.sha256)),
        ],
        row.inhalt,
    ))
}

// ── The order ─────────────────────────────────────────────────────────────────

/// `POST /api/v1/bestellungen` — an online order (US1).
///
/// The `Idempotency-Key` header is required once; the same key with the same
/// body answers with the first response, with another body `422
/// idempotency_key_reused`, and while the first is still being processed `409
/// idempotency_request_in_flight`.
///
/// Under `[policy] vertragsschluss = "mit_bestellung"` the Bestellnachweis,
/// the Bestellung, the contract, the Anmeldung's outbox command, the
/// documents' commands and the `de.vertrag.bestellung.*` events commit in one
/// transaction (`201`). Under `"mit_annahme"` the order is stored
/// `EINGEGANGEN` and the contract waits for `…/annehmen` (`202`).
pub async fn bestellen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    headers: HeaderMap,
    Json(input): Json<BestellungInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&enforcer, &claims, "bestellen", ctx.tenant())?;
    let regel = vertragsschluss_regel(&ctx)?;
    let key = idempotency_key(&headers)?;
    let partner = claims.sub().to_owned();
    let request_sha256 = bestellung::kanonisch(&serde_json::to_value(&input).map_err(internal)?)
        .map_err(internal)?
        .1;

    if let Some(b) = store::fetch_bestellung_by_key(&ctx.pool, ctx.tenant(), &partner, &key)
        .await
        .map_err(internal)?
    {
        return wiederholen(&b, &request_sha256);
    }

    let (angebot, angebot_json) = pruefe_bestellung(&ctx, &input).await?;
    let jetzt = OffsetDateTime::now_utc();

    let mut tx = ctx.pool.begin().await.map_err(internal)?;
    // A concurrent request under the same key waits on the unique index; past
    // this bound it is told the first is still in flight rather than held.
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    let nachweis = store::insert_nachweis(
        &mut tx,
        &store::NeuerNachweis {
            tenant: ctx.tenant(),
            partner: &partner,
            input: &input,
            eingegangen_am: jetzt,
        },
    )
    .await
    .map_err(internal)?;
    let claimed = store::insert_bestellung(
        &mut tx,
        store::NeueBestellung {
            tenant: ctx.tenant(),
            partner: &partner,
            idempotency_key: &key,
            request_sha256: &request_sha256,
            bestellreferenz: &input.bestellreferenz,
            nachweis: &nachweis,
            auftrag: serde_json::to_value(&input).map_err(internal)?,
            angebot: angebot_json,
        },
    )
    .await;
    let bestellung_id = match claimed {
        Ok(Some(id)) => id,
        Ok(None) => {
            drop(tx);
            let b = store::fetch_bestellung_by_key(&ctx.pool, ctx.tenant(), &partner, &key)
                .await
                .map_err(internal)?
                .ok_or_else(in_flight)?;
            return wiederholen(&b, &request_sha256);
        }
        Err(e) if is_lock_timeout(&e) => return Err(in_flight()),
        Err(e) => return Err(internal(e)),
    };

    enqueue_bestell_event(
        &mut tx,
        &ctx,
        mako_events::vertrag::BESTELLUNG_EINGEGANGEN,
        &input.bestellreferenz,
        bestellung_id,
        serde_json::json!({
            "bestellung_id": bestellung_id,
            "bestellreferenz": input.bestellreferenz,
            "bestellnachweis": { "id": nachweis.id(), "sha256": nachweis.sha256() },
            "eingegangen_am": dokumente::berlin_iso(jetzt),
        }),
    )
    .await?;

    let (status, antwort) = match regel {
        Vertragsschluss::MitBestellung => {
            let abschluss = schliesse_vertrag(
                &mut tx,
                &ctx,
                bestellung_id,
                &input,
                &angebot,
                &nachweis,
                jetzt,
                None,
            )
            .await?;
            (
                StatusCode::CREATED,
                abschluss.antwort(bestellung_id, &nachweis),
            )
        }
        Vertragsschluss::MitAnnahme => (
            StatusCode::ACCEPTED,
            serde_json::json!({
                "bestellung_id": bestellung_id,
                "status": "EINGEGANGEN",
                "bestellnachweis": { "id": nachweis.id(), "sha256": nachweis.sha256() },
            }),
        ),
    };
    store::set_antwort(
        &mut tx,
        ctx.tenant(),
        bestellung_id,
        &antwort,
        status.as_u16(),
    )
    .await
    .map_err(internal)?;
    tx.commit().await.map_err(|e| {
        if is_lock_timeout(&anyhow::Error::from(e)) {
            in_flight()
        } else {
            ApiError::Internal(anyhow::anyhow!("commit failed"))
        }
    })?;
    Ok((status, Json(antwort)))
}

/// The operator's `[policy] vertragsschluss`, or `409 policy_nicht_gesetzt`.
fn vertragsschluss_regel(ctx: &Ctx) -> ApiResult<Vertragsschluss> {
    ctx.cfg
        .policy
        .vertragsschluss
        .map(|p| p.get())
        .ok_or_else(|| {
            ApiError::conflict(
                "[policy] vertragsschluss is not set — whether an online order concludes the \
                 contract or awaits acceptance is the operator's setting",
            )
            .with_code("policy_nicht_gesetzt")
        })
}

/// The single `Idempotency-Key` header.
fn idempotency_key(headers: &HeaderMap) -> ApiResult<String> {
    let key = mako_service::headers::single_str(headers, "Idempotency-Key")
        .map_err(|e| ApiError::bad_request(e.to_string()).with_code("idempotency_key_duplicate"))?
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("the Idempotency-Key header is required")
                .with_code(idempotency_codes::MISSING)
        })?;
    if key.len() > 255 {
        return Err(
            ApiError::bad_request("Idempotency-Key is longer than 255 characters")
                .with_code("idempotency_key_invalid"),
        );
    }
    Ok(key.to_owned())
}

fn in_flight() -> ApiError {
    ApiError::conflict("a request with this Idempotency-Key is still being processed")
        .with_code(idempotency_codes::IN_FLIGHT)
}

/// `55P03` lock_not_available — the `lock_timeout` ran out.
fn is_lock_timeout(e: &anyhow::Error) -> bool {
    e.downcast_ref::<sqlx::Error>()
        .and_then(sqlx::Error::as_database_error)
        .and_then(|d| d.code().map(std::borrow::Cow::into_owned))
        .is_some_and(|c| c == "55P03")
}

/// A repeated key: the first answer, or `422 idempotency_key_reused`.
fn wiederholen(
    b: &store::BestellungRow,
    request_sha256: &str,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    match bestellung::wiederholung(&b.request_sha256, request_sha256) {
        Wiederholung::Replay => {
            let status = u16::try_from(b.antwort_status)
                .ok()
                .and_then(|s| StatusCode::from_u16(s).ok())
                .ok_or_else(in_flight)?;
            Ok((status, Json(b.antwort.clone())))
        }
        Wiederholung::KeyReused => Err(ApiError::unprocessable(
            "this Idempotency-Key was used with a different request body",
        )
        .with_code(idempotency_codes::REUSED)),
    }
}

/// Every check an order passes before anything is stored — completeness, the
/// button label, the documents, the offer, the Lieferbeginn — each refusal by
/// name. Answers the offer it accepts.
async fn pruefe_bestellung(
    ctx: &Ctx,
    input: &BestellungInput,
) -> ApiResult<(EndkundenAngebot, serde_json::Value)> {
    let fehlend = fehlende_angaben(input);
    if !fehlend.is_empty() {
        return Err(ApiError::unprocessable(
            "the order lacks elements the Bestellnachweis must hold (§ 312j Abs. 2 BGB)",
        )
        .with_code("bestellnachweis_unvollstaendig")
        .with_fields(serde_json::json!({ "fehlend": fehlend })));
    }
    if !bestellung::beschriftung_zulaessig(
        &input.button_beschriftung,
        &ctx.cfg.policy.bestellbutton_beschriftungen,
    ) {
        return Err(ApiError::unprocessable(
            "the order button's label is neither „zahlungspflichtig bestellen“ nor one the \
             operator lists (§ 312j Abs. 3 BGB)",
        )
        .with_code("bestellbutton_nicht_zulaessig")
        .with_fields(serde_json::json!({
            "button_beschriftung": input.button_beschriftung,
            "zugelassen": ctx.cfg.policy.bestellbutton_beschriftungen,
        })));
    }
    if let Some(malo) = input.kunde.malo_id.as_deref()
        && malo.parse::<rubo4e::identifiers::MaloId>().is_err()
    {
        return Err(
            ApiError::unprocessable("kunde.malo_id fails its check digit")
                .with_code("malo_id_ungueltig")
                .with_fields(serde_json::json!({ "malo_id": malo })),
        );
    }
    let unbekannt = store::unbekannte_dokumente(&ctx.pool, ctx.tenant(), &input.dokumente)
        .await
        .map_err(internal)?;
    if !unbekannt.is_empty() {
        return Err(dokument_unbekannt(unbekannt));
    }

    let client = productd(ctx)?;
    let (angebot, raw) = client
        .angebot(input.angebot_ref.id)
        .await
        .map_err(|e| ApiError::upstream("productd", e))?
        .ok_or_else(|| {
            ApiError::unprocessable("productd holds no Endkunden-Angebot with this id")
                .with_code("angebot_unbekannt")
        })?;
    // The Preisblatt shown is the product version the offer priced.
    let preisblatt_falsch: Vec<String> = input
        .dokumente
        .iter()
        .filter(|d| d.art == DokumentArt::Preisblatt && d.sha256 != angebot.product_version)
        .map(|d| format!("PREISBLATT:{}", d.sha256))
        .collect();
    if !preisblatt_falsch.is_empty() {
        return Err(dokument_unbekannt(preisblatt_falsch));
    }
    if let Some(a) = endkunden_angebot::pruefe(
        &angebot,
        &input.angebot_ref.sha256,
        input.kunde.malo_id.as_deref(),
        input.gewuenschter_lieferbeginn,
        OffsetDateTime::now_utc(),
    ) {
        return Err(match a {
            Abweichung::Hash => ApiError::unprocessable(
                "the offer's hash is not the one productd holds — the order accepts another \
                 offer than the one shown",
            )
            .with_code("angebot_hash_abweichend"),
            Abweichung::Abgelaufen => ApiError::unprocessable("the offer has expired")
                .with_code("angebot_abgelaufen")
                .with_fields(serde_json::json!({
                    "gueltig_bis": dokumente::berlin_iso(angebot.gueltig_bis),
                })),
            Abweichung::MaloId => ApiError::unprocessable("the order's MaLo-ID is not the offer's")
                .with_code("malo_id_abweichend")
                .with_fields(serde_json::json!({ "angebot_malo_id": angebot.malo_id })),
            Abweichung::LieferbeginnZuFrueh(d) => lieferbeginn_zu_frueh(d),
        });
    }
    if let Some(fruehestens) =
        fruehester_lieferbeginn_in_widerrufsfrist(ctx, input, mako_fristen::heute())
        && input.gewuenschter_lieferbeginn < fruehestens
    {
        return Err(lieferbeginn_zu_frueh(fruehestens));
    }
    let fehlend = anmeldung_fehlend(input, &angebot);
    if !fehlend.is_empty() {
        return Err(
            ApiError::unprocessable("the Anmeldung needs facts the order does not state")
                .with_code("anmeldedaten_unvollstaendig")
                .with_fields(serde_json::json!({ "fehlend": fehlend })),
        );
    }
    Ok((angebot, raw))
}

fn dokument_unbekannt(unbekannt: Vec<String>) -> ApiError {
    ApiError::unprocessable(
        "a cited document hash resolves to no document mako holds (§ 312i Abs. 1 Satz 1 Nr. 4 BGB)",
    )
    .with_code("dokument_unbekannt")
    .with_fields(serde_json::json!({ "unbekannt": unbekannt }))
}

fn lieferbeginn_zu_frueh(fruehestens: Date) -> ApiError {
    ApiError::unprocessable("the desired Lieferbeginn is earlier than admissible")
        .with_code("lieferbeginn_zu_frueh")
        .with_fields(serde_json::json!({ "fruehestens": fruehestens.to_string() }))
}

/// § 357a Abs. 2 Satz 1 Nr. 1 BGB: without the consumer's express request, and under
/// `lieferbeginn_in_widerrufsfrist = "nur_auf_verlangen"`, supply starts no
/// earlier than the day after the Widerrufsfrist ends — computed as if the
/// contract came about on `vertragsschluss`. `None` when no such bound applies.
fn fruehester_lieferbeginn_in_widerrufsfrist(
    ctx: &Ctx,
    input: &BestellungInput,
    vertragsschluss: Date,
) -> Option<Date> {
    if !input.verbraucher
        || input.lieferung_in_widerrufsfrist_verlangt.verlangt
        || ctx.cfg.policy.lieferbeginn_in_widerrufsfrist.get()
            == LieferbeginnInWiderrufsfrist::Immer
    {
        return None;
    }
    let frist = mako_fristen::vorlauf::widerrufsfrist(
        vertragsschluss,
        input.belehrung_angezeigt.then_some(vertragsschluss),
        CAL,
    );
    frist.ende.next_day()
}

/// The supply component an order concludes into.
fn komponente(input: &BestellungInput, angebot: &EndkundenAngebot) -> pg::CreateKomponenteInput {
    let a = &input.anmeldung;
    pg::CreateKomponenteInput {
        sparte: angebot.sparte.clone(),
        malo_id: input.kunde.malo_id.clone(),
        melo_id: None,
        nb_mp_id: Some(angebot.nb_mp_id.clone()),
        bilanzkreis: a.bilanzkreis.clone(),
        product_code: angebot.product_code.clone(),
        lieferbeginn: input.gewuenschter_lieferbeginn,
        lieferende: None,
        jahresverbrauch_kwh: angebot.verbrauch_kwh.parse().ok(),
        lieferbeginn_anlass: a.lieferbeginn_anlass,
        netznutzungsvertrag: a.netznutzungsvertrag,
        netznutzung_zahler: a.netznutzung_zahler,
        gemeinderabatt_prozent: a.gemeinderabatt_prozent,
        enfg_grundlage: a.enfg_grundlage.clone(),
        enfg_grund: a.enfg_grund.clone(),
        fulfillment_data: None,
    }
}

/// Every Anmeldung fact the order does not state — the contract would be
/// refused on insert otherwise.
fn anmeldung_fehlend(input: &BestellungInput, angebot: &EndkundenAngebot) -> Vec<String> {
    let k = komponente(input, angebot);
    let gp = input.kunde.geschaeftspartner.canonical_json().ok();
    let kunde = pg::KundeAnmeldedaten {
        haushaltskunde: input
            .kunde
            .haushaltskunde
            .unwrap_or(input.kunde.kundentyp == "B2C"),
        kunde: crate::domain::utilmd_kunde(gp.as_ref(), None, None),
    };
    let mut fehlend = Vec::new();
    if let Err(f) = k.strom_anmeldung(&kunde) {
        fehlend.extend(f);
    }
    if let Err(f) = k.gas_anmeldung(
        input.abrechnungszyklus.as_deref().unwrap_or("JAEHRLICH"),
        &kunde,
    ) {
        fehlend.extend(f);
    }
    if let Some(bk) = k.bilanzkreis.as_deref()
        && let Err(e) = crate::domain::pruefe_bilanzkreis(&k.sparte, bk)
    {
        fehlend.push(e);
    }
    fehlend
}

fn productd(ctx: &Ctx) -> ApiResult<ProductdClient> {
    let url = ctx.cfg.productd_url.as_deref().ok_or_else(|| {
        ApiError::unavailable("productd_url is not configured — no offer can be checked")
            .with_code("productd_nicht_konfiguriert")
    })?;
    Ok(ProductdClient::new(
        url,
        ctx.cfg.productd_api_key.clone(),
        ctx.http.clone(),
    ))
}

/// What concluding an order produced.
struct Abschluss {
    vertrag_id: Uuid,
    vertrags_nr: String,
    am: OffsetDateTime,
    widerrufsfrist: Option<mako_fristen::vorlauf::Widerrufsfrist>,
}

impl Abschluss {
    fn antwort(&self, bestellung_id: Uuid, nachweis: &GespeicherterNachweis) -> serde_json::Value {
        serde_json::json!({
            "bestellung_id": bestellung_id,
            "status": "ANGENOMMEN",
            "vertrag_id": self.vertrag_id,
            "vertrags_nr": self.vertrags_nr,
            "vertragsschluss_am": dokumente::berlin_iso(self.am),
            "bestellnachweis": { "id": nachweis.id(), "sha256": nachweis.sha256() },
            "widerrufsfrist": self.widerrufsfrist.map(frist_json),
        })
    }
}

fn frist_json(f: mako_fristen::vorlauf::Widerrufsfrist) -> serde_json::Value {
    serde_json::json!({
        "beginn": f.beginn.map(|d| d.to_string()),
        "ende": f.ende.to_string(),
        "erloeschen": f.erloeschen.to_string(),
    })
}

/// Conclude an order into a contract inside `tx`: the Kunde, the contract
/// with its Anmeldung command, the Bestellung `ANGENOMMEN`, the
/// `de.vertrag.bestellung.angenommen` event and the documents the statute owes
/// — the § 41 Abs. 4 EnWG Vertragszusammenfassung always, the § 312f Abs. 2
/// BGB Vertragsbestätigung for a consumer.
#[expect(
    clippy::too_many_arguments,
    reason = "one conclusion, called from the order route and the acceptance route with the same facts"
)]
async fn schliesse_vertrag(
    tx: &mut sqlx::PgConnection,
    ctx: &Ctx,
    bestellung_id: Uuid,
    input: &BestellungInput,
    angebot: &EndkundenAngebot,
    nachweis: &GespeicherterNachweis,
    am: OffsetDateTime,
    bonitaet_ref: Option<&str>,
) -> ApiResult<Abschluss> {
    let tenant = ctx.tenant();
    let kunden_id = store::insert_bestellkunde(&mut *tx, tenant, &input.kunde)
        .await
        .map_err(internal)?;
    let vertrag_input = pg::CreateVersorgungsvertragInput {
        rahmenvertrag_id: None,
        kundentyp: input.kunde.kundentyp.clone(),
        verbraucher: Some(input.verbraucher),
        vertragsart: None,
        bundle_code: None,
        vertragsbeginn: input.gewuenschter_lieferbeginn,
        vertragsende: None,
        kuendigungsfrist_monate: Some(input.kuendigungsfrist_monate),
        preisgarantie_bis: None,
        abrechnungszyklus: input.abrechnungszyklus.clone(),
        auto_renewal: None,
        renewal_monate: None,
        standort_bezeichnung: None,
        standort_adresse: None,
        zahlungsziel_tage: None,
        erp_contract_id: None,
        notizen: None,
        komponenten: vec![komponente(input, angebot)],
    };
    let online = OnlineVertragsschluss::new(nachweis, am);
    let inserted = pg::insert_versorgungsvertrag_tx(
        &mut *tx,
        kunden_id,
        tenant,
        &ctx.cfg.lf_mp_id,
        &vertrag_input,
        Some(&online),
    )
    .await
    .map_err(internal)?;
    store::annehmen(&mut *tx, tenant, bestellung_id, inserted.id, bonitaet_ref)
        .await
        .map_err(internal)?;

    let schluss = mako_fristen::berlin_date(am);
    let widerrufsfrist = input
        .verbraucher
        .then(|| mako_fristen::vorlauf::widerrufsfrist(schluss, nachweis.belehrung_am(), CAL));
    let abschluss = Abschluss {
        vertrag_id: inserted.id,
        vertrags_nr: inserted.vertrags_nr.clone(),
        am,
        widerrufsfrist,
    };

    enqueue_bestell_event(
        &mut *tx,
        ctx,
        mako_events::vertrag::BESTELLUNG_ANGENOMMEN,
        &input.bestellreferenz,
        bestellung_id,
        serde_json::json!({
            "bestellung_id": bestellung_id,
            "bestellreferenz": input.bestellreferenz,
            "vertrag_id": inserted.id,
            "vertrags_nr": inserted.vertrags_nr,
            "vertragsschluss_am": dokumente::berlin_iso(am),
            "widerrufsfrist": widerrufsfrist.map(frist_json),
        }),
    )
    .await?;

    owe_documents(&mut *tx, ctx, input, angebot, &abschluss).await?;
    Ok(abschluss)
}

/// The Vertragszusammenfassung for every conclusion, the Vertragsbestätigung
/// for a consumer's — each an outbox command in the conclusion's transaction.
async fn owe_documents(
    tx: &mut sqlx::PgConnection,
    ctx: &Ctx,
    input: &BestellungInput,
    angebot: &EndkundenAngebot,
    abschluss: &Abschluss,
) -> ApiResult<()> {
    let gp = input
        .kunde
        .geschaeftspartner
        .canonical_json()
        .map_err(internal)?;
    let empfaenger = dokumente::empfaenger(&gp, input.kunde.email.as_deref());
    let datum = mako_fristen::heute().to_string();
    let verbrauchsstelle = input.kunde.malo_id.as_deref().map_or_else(
        || {
            empfaenger
                .postal_address()
                .map_or_else(String::new, |a| a.to_string())
        },
        |m| format!("Marktlokation {m}"),
    );
    let preise: Vec<dokumente::PreisZeile> = angebot
        .preise
        .iter()
        .map(|p| dokumente::PreisZeile {
            bezeichnung: p.bezeichnung.clone(),
            einheit: p.einheit.clone(),
            wert: p.wert.clone(),
        })
        .collect();
    let kuendigungsfrist = match input.kuendigungsfrist_monate {
        1 => "1 Monat".to_owned(),
        n => format!("{n} Monate"),
    };
    let subject = abschluss.vertrag_id.to_string();
    let malo = input.kunde.malo_id.as_deref();
    let info = &input.angezeigte_informationen;

    let vz = dokumente::VertragszusammenfassungView {
        datum: datum.clone(),
        absender: dokumente::absender(&ctx.cfg),
        empfaenger: empfaenger.clone(),
        vertragsnummer: abschluss.vertrags_nr.clone(),
        sparte: Some(angebot.sparte.clone()),
        verbrauchsstelle: verbrauchsstelle.clone(),
        malo_id: malo.map(str::to_owned),
        preise: preise.clone(),
        belieferungsbeginn: input.gewuenschter_lieferbeginn.to_string(),
        kuendigungsfrist: kuendigungsfrist.clone(),
        bonus: None,
        mindestvertragslaufzeit: info.mindestdauer.clone(),
    };
    let task = outbound::dokument(
        "VERTRAGSZUSAMMENFASSUNG",
        &subject,
        dokumente::issue_body(
            serde_json::to_value(&vz).map_err(internal)?,
            &empfaenger,
            &subject,
            malo,
            &datum,
        ),
    );
    outbound::enqueue(&mut *tx, ctx.tenant(), &task)
        .await
        .map_err(internal)?;

    if !input.verbraucher {
        return Ok(());
    }
    let belehrung_text =
        rechtsdokument_text(tx, ctx, input, DokumentArt::Widerrufsbelehrung).await?;
    let muster = rechtsdokument_text(tx, ctx, input, DokumentArt::MusterWiderrufsformular).await?;
    let frist = match abschluss.widerrufsfrist.and_then(|f| f.beginn) {
        Some(_) => "14 Tage ab Vertragsschluss (§ 355 Abs. 2, § 356 Abs. 2 Nr. 2 BGB)".to_owned(),
        None => "14 Tage ab Erhalt dieser Bestätigung mit der Widerrufsbelehrung \
                 (§ 355 Abs. 2, § 356 Abs. 3 BGB)"
            .to_owned(),
    };
    let vb = dokumente::VertragsbestaetigungView {
        datum: datum.clone(),
        absender: dokumente::absender(&ctx.cfg),
        empfaenger: empfaenger.clone(),
        vertragsnummer: abschluss.vertrags_nr.clone(),
        vertragsschluss_am: dokumente::berlin_iso(abschluss.am),
        sparte: Some(angebot.sparte.clone()),
        verbrauchsstelle,
        malo_id: malo.map(str::to_owned),
        produkt: info
            .wesentliche_eigenschaften
            .clone()
            .unwrap_or_else(|| angebot.product_code.clone()),
        preise,
        belieferungsbeginn: input.gewuenschter_lieferbeginn.to_string(),
        laufzeit: info.laufzeit_kuendigung.clone().unwrap_or_default(),
        kuendigungsfrist,
        widerruf: dokumente::WiderrufsbelehrungView {
            frist,
            belehrung_text,
            muster_widerrufsformular: muster,
            widerrufsfunktion: None,
            wertersatz_hinweis: dokumente::WERTERSATZ_HINWEIS.to_owned(),
        },
        lieferung_in_widerrufsfrist_verlangt: input.lieferung_in_widerrufsfrist_verlangt.verlangt,
    };
    let task = outbound::dokument(
        "VERTRAGSBESTAETIGUNG",
        &subject,
        dokumente::issue_body(
            serde_json::to_value(&vb).map_err(internal)?,
            &empfaenger,
            &subject,
            malo,
            &datum,
        ),
    );
    outbound::enqueue(&mut *tx, ctx.tenant(), &task)
        .await
        .map_err(internal)?;
    Ok(())
}

/// The text of the `art` document the order cited — checked to exist before
/// the transaction opened, and append-only since.
async fn rechtsdokument_text(
    tx: &mut sqlx::PgConnection,
    ctx: &Ctx,
    input: &BestellungInput,
    art: DokumentArt,
) -> ApiResult<String> {
    let sha = input
        .dokumente
        .iter()
        .find(|d| d.art == art)
        .map(|d| d.sha256.as_str())
        .ok_or_else(|| internal(anyhow::anyhow!("{} not cited", art.as_db())))?;
    let row = store::fetch_rechtsdokument(&mut *tx, ctx.tenant(), sha)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal(anyhow::anyhow!("{} {sha} not held", art.as_db())))?;
    String::from_utf8(row.inhalt).map_err(internal)
}

/// Enqueue a `de.vertrag.bestellung.*` / `de.vertrag.widerruf.*` CloudEvent
/// whose `subject` is the partner's `bestellreferenz`.
async fn enqueue_bestell_event(
    tx: &mut sqlx::PgConnection,
    ctx: &Ctx,
    typ: &str,
    bestellreferenz: &str,
    bestellung_id: Uuid,
    data: serde_json::Value,
) -> ApiResult<()> {
    let ce =
        crate::events::build_bestell_event(typ, bestellreferenz, bestellung_id, ctx.tenant(), data);
    mako_service::outbox::enqueue(&mut *tx, &ce)
        .await
        .map_err(|e| ApiError::Internal(e.into()))
}

// ── Acceptance (`mit_annahme`) ────────────────────────────────────────────────

/// `POST /api/v1/bestellungen/{id}/annehmen`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnahmeInput {
    /// The operator's own credit-check reference — stored, never evaluated.
    #[serde(default)]
    pub bonitaet_ref: Option<String>,
}

/// `POST /api/v1/bestellungen/{id}/annehmen` — the operator accepts an order
/// under `mit_annahme`; the contract comes about now.
pub async fn annehmen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(body): Json<AnnahmeInput>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "annehmen-bestellung", ctx.tenant())?;
    let mut tx = ctx.pool.begin().await.map_err(internal)?;
    let b = offene_bestellung(&mut tx, &ctx, id).await?;
    let input: BestellungInput = serde_json::from_value(b.auftrag.clone()).map_err(internal)?;
    let angebot: EndkundenAngebot = serde_json::from_value(b.angebot.clone()).map_err(internal)?;
    let nachweis = store::fetch_nachweis(&mut *tx, ctx.tenant(), b.bestellnachweis_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal(anyhow::anyhow!("Bestellnachweis of {id} missing")))?;
    // The Frist starts at this acceptance, so the Lieferbeginn bound is
    // decided against it.
    if let Some(fruehestens) =
        fruehester_lieferbeginn_in_widerrufsfrist(&ctx, &input, mako_fristen::heute())
        && input.gewuenschter_lieferbeginn < fruehestens
    {
        return Err(lieferbeginn_zu_frueh(fruehestens));
    }
    let abschluss = schliesse_vertrag(
        &mut tx,
        &ctx,
        id,
        &input,
        &angebot,
        &nachweis,
        OffsetDateTime::now_utc(),
        body.bonitaet_ref.as_deref(),
    )
    .await?;
    tx.commit().await.map_err(internal)?;
    Ok(Json(abschluss.antwort(id, &nachweis)))
}

/// `POST /api/v1/bestellungen/{id}/ablehnen`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AblehnungInput {
    pub grund: String,
}

/// `POST /api/v1/bestellungen/{id}/ablehnen` — the operator refuses an order
/// under `mit_annahme`; no contract, no Anmeldung.
pub async fn ablehnen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(body): Json<AblehnungInput>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&enforcer, &claims, "annehmen-bestellung", ctx.tenant())?;
    let mut tx = ctx.pool.begin().await.map_err(internal)?;
    let b = offene_bestellung(&mut tx, &ctx, id).await?;
    store::schliessen(&mut tx, ctx.tenant(), id, "ABGELEHNT", Some(&body.grund))
        .await
        .map_err(internal)?;
    enqueue_bestell_event(
        &mut tx,
        &ctx,
        mako_events::vertrag::BESTELLUNG_ABGELEHNT,
        &b.bestellreferenz,
        id,
        serde_json::json!({
            "bestellung_id": id,
            "bestellreferenz": b.bestellreferenz,
            "grund": body.grund,
        }),
    )
    .await?;
    tx.commit().await.map_err(internal)?;
    ok(serde_json::json!({ "bestellung_id": id, "status": "ABGELEHNT" }))
}

/// The order `id`, locked, while it is still `EINGEGANGEN` under `mit_annahme`.
async fn offene_bestellung(
    tx: &mut sqlx::PgConnection,
    ctx: &Ctx,
    id: Uuid,
) -> ApiResult<store::BestellungRow> {
    sqlx::query("SELECT 1 FROM bestellungen WHERE tenant = $1 AND id = $2 FOR UPDATE")
        .bind(ctx.tenant())
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    let b = store::fetch_bestellung(&mut *tx, ctx.tenant(), id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)?;
    if b.status != "EINGEGANGEN" {
        return Err(ApiError::conflict(format!(
            "the order is {} — only an EINGEGANGEN order is accepted or refused",
            b.status
        ))
        .with_code("bestellung_nicht_offen")
        .with_fields(serde_json::json!({ "status": b.status })));
    }
    Ok(b)
}

/// `GET /api/v1/bestellungen/{id}/nachweis` — the Bestellnachweis's canonical
/// bytes, the stored hash and the hash recomputed from the bytes.
pub async fn nachweis(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<serde_json::Value>> {
    use base64::Engine as _;
    authorize(&enforcer, &claims, "read-bestellung", ctx.tenant())?;
    let b = store::fetch_bestellung(&ctx.pool, ctx.tenant(), id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)?;
    let (bytes, sha256) =
        store::fetch_nachweis_bytes(&ctx.pool, ctx.tenant(), b.bestellnachweis_id)
            .await
            .map_err(internal)?
            .ok_or(ApiError::NotFound)?;
    ok(serde_json::json!({
        "bestellnachweis_id": b.bestellnachweis_id,
        "kanonisch": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "sha256": sha256,
        "sha256_neu_berechnet": bestellung::sha256_hex(&bytes),
    }))
}

// ── Widerruf (US2) ────────────────────────────────────────────────────────────

/// A Widerruf as received — the § 356a Abs. 2 BGB data, the channel and the
/// sending instant.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WiderrufInput {
    /// § 356a Abs. 2 Nr. 1 — the consumer's name.
    pub name: String,
    /// § 356a Abs. 2 Nr. 2 — the contract as the consumer identifies it.
    pub vertrag: String,
    /// § 356a Abs. 2 Nr. 3 — where the Eingangsbestätigung goes.
    pub bestaetigung_an: String,
    /// `WIDERRUFSFUNKTION` | `EMAIL` | `BRIEF` | `SONSTIG`.
    pub kanal: WiderrufKanal,
    /// When the Widerruf was sent — for a letter, what the operator reads off
    /// it, not the day it is entered (§ 355 Abs. 1 Satz 5 BGB).
    #[serde(with = "time::serde::rfc3339")]
    pub abgesendet_am: OffsetDateTime,
}

/// The channel a Widerruf came by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WiderrufKanal {
    Widerrufsfunktion,
    Email,
    Brief,
    Sonstig,
}

impl WiderrufKanal {
    pub const ALL: [Self; 4] = [
        Self::Widerrufsfunktion,
        Self::Email,
        Self::Brief,
        Self::Sonstig,
    ];

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Widerrufsfunktion => "WIDERRUFSFUNKTION",
            Self::Email => "EMAIL",
            Self::Brief => "BRIEF",
            Self::Sonstig => "SONSTIG",
        }
    }

    #[must_use]
    pub fn from_db(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_db() == s)
    }
}

/// `POST /api/v1/vertraege/{id}/widerruf` — a consumer's Widerruf of an
/// online contract (US2).
pub async fn widerruf_vertrag(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(input): Json<WiderrufInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&enforcer, &claims, "widerrufen", ctx.tenant())?;
    let b = store::fetch_bestellung_by_vertrag(&ctx.pool, ctx.tenant(), id)
        .await
        .map_err(internal)?
        .ok_or_else(kein_online_vertrag)?;
    widerruf_des_vertrags(&ctx, id, &b, &input).await
}

/// `POST /api/v1/bestellungen/{id}/widerruf` — the Widerruf of an order: an
/// accepted one withdraws its contract, one not yet accepted closes
/// `WIDERRUFEN` with no contract and no Anmeldung.
pub async fn widerruf_bestellung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(ctx): Extension<Arc<Ctx>>,
    Path(id): Path<Uuid>,
    Json(input): Json<WiderrufInput>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    authorize(&enforcer, &claims, "widerrufen", ctx.tenant())?;
    let b = store::fetch_bestellung(&ctx.pool, ctx.tenant(), id)
        .await
        .map_err(internal)?
        .ok_or(ApiError::NotFound)?;
    if let Some(vertrag_id) = b.vertrag_id {
        return widerruf_des_vertrags(&ctx, vertrag_id, &b, &input).await;
    }
    pruefe_abgesendet(&input)?;
    let mut tx = ctx.pool.begin().await.map_err(internal)?;
    if let Some(w) = store::fetch_rechtzeitiger_widerruf(
        &mut *tx,
        ctx.tenant(),
        store::WiderrufGegenstand::Bestellung(id),
    )
    .await
    .map_err(internal)?
    {
        return Ok((StatusCode::OK, Json(widerruf_antwort(&w))));
    }
    if !store::schliessen(&mut tx, ctx.tenant(), id, "WIDERRUFEN", None)
        .await
        .map_err(internal)?
    {
        return Err(ApiError::conflict(format!("the order is {}", b.status))
            .with_code("bestellung_nicht_offen")
            .with_fields(serde_json::json!({ "status": b.status })));
    }
    let w = store::insert_widerruf(
        &mut tx,
        store::NeuerWiderruf {
            tenant: ctx.tenant(),
            gegenstand: store::WiderrufGegenstand::Bestellung(id),
            kanal: input.kanal.as_db(),
            abgesendet_am: input.abgesendet_am,
            eingegangen_am: OffsetDateTime::now_utc(),
            erklaerung: erklaerung(&input),
            frist: None,
            rechtzeitig: true,
            wirkung: Some("BESTELLUNG_GESCHLOSSEN"),
            lieferende: None,
            wertersatz_geschuldet: None,
        },
    )
    .await
    .map_err(internal)?;
    widerruf_folgen(&mut tx, &ctx, &b, &w, &input, &b.bestellreferenz).await?;
    tx.commit().await.map_err(internal)?;
    Ok((StatusCode::CREATED, Json(widerruf_antwort(&w))))
}

fn kein_online_vertrag() -> ApiError {
    ApiError::unprocessable(
        "the contract was not concluded through POST /api/v1/bestellungen — its Widerruf is \
         not taken here",
    )
    .with_code("kein_online_vertrag")
}

fn pruefe_abgesendet(input: &WiderrufInput) -> ApiResult<()> {
    if input.abgesendet_am > OffsetDateTime::now_utc() {
        return Err(ApiError::unprocessable("abgesendet_am lies in the future")
            .with_code("abgesendet_am_in_der_zukunft"));
    }
    Ok(())
}

fn erklaerung(input: &WiderrufInput) -> serde_json::Value {
    serde_json::json!({
        "name": input.name,
        "vertrag": input.vertrag,
        "bestaetigung_an": input.bestaetigung_an,
    })
}

/// The Widerruf of a concluded online contract: the Frist decided from the
/// contract's own dates, then — in one transaction — the record, the
/// Stornierung or the Lieferende, `WIDERRUFEN`, the event and the
/// Eingangsbestätigung. A late one is recorded, confirmed as received and
/// refused with the dates.
async fn widerruf_des_vertrags(
    ctx: &Ctx,
    vertrag_id: Uuid,
    b: &store::BestellungRow,
    input: &WiderrufInput,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    pruefe_abgesendet(input)?;
    let vertrag = require_vertrag(ctx, vertrag_id).await?;
    if vertrag.verbraucher != Some(true) {
        return Err(ApiError::unprocessable(
            "the contract was not concluded by a Verbraucher — no Widerrufsrecht (§ 312g Abs. 1 BGB)",
        )
        .with_code("kein_widerrufsrecht"));
    }
    if let Some(w) = store::fetch_rechtzeitiger_widerruf(
        &ctx.pool,
        ctx.tenant(),
        store::WiderrufGegenstand::Vertrag(vertrag_id),
    )
    .await
    .map_err(internal)?
    {
        return Ok((StatusCode::OK, Json(widerruf_antwort(&w))));
    }
    let schluss_am = vertrag.vertragsschluss_am.ok_or_else(|| {
        internal(anyhow::anyhow!(
            "online contract {vertrag_id} has no Vertragsschluss"
        ))
    })?;
    let nachweis = store::fetch_nachweis(&ctx.pool, ctx.tenant(), b.bestellnachweis_id)
        .await
        .map_err(internal)?
        .ok_or_else(|| internal(anyhow::anyhow!("Bestellnachweis of {vertrag_id} missing")))?;
    let belehrung = match nachweis.belehrung_am() {
        Some(d) => Some(d),
        None => vertragsbestaetigung_zugestellt(ctx, vertrag_id).await?,
    };
    let frist = mako_fristen::vorlauf::widerrufsfrist(
        mako_fristen::berlin_date(schluss_am),
        belehrung,
        CAL,
    );
    let abgesendet = mako_fristen::berlin_date(input.abgesendet_am);
    let rechtzeitig = frist.rechtzeitig(abgesendet);
    let jetzt = OffsetDateTime::now_utc();

    let mut tx = ctx.pool.begin().await.map_err(internal)?;
    let (wirkung, lieferende, wertersatz) = if rechtzeitig {
        let komponenten = pg::list_komponenten(&ctx.pool, vertrag_id)
            .await
            .map_err(internal)?;
        let lieferbeginn = komponenten.iter().map(|k| k.lieferbeginn).min();
        match vertrag.status.as_str() {
            "ANGELEGT" | "IN_BEARBEITUNG" => {
                pg::storniere_vertrag_tx(&mut tx, vertrag_id, ctx.tenant())
                    .await
                    .map_err(internal)?;
                (Some("STORNIERT"), None, None)
            }
            "AKTIV" | "TEILERFUELLUNG" | "GEKÜNDIGT" => {
                let ende = mako_fristen::berlin_date(jetzt)
                    .next_day()
                    .unwrap_or(Date::MAX)
                    .max(lieferbeginn.unwrap_or(Date::MIN));
                pg::beende_komponenten(
                    &mut tx,
                    &vertrag,
                    ende,
                    crate::domain::Kuendigungsgrund::Ordentlich,
                )
                .await
                .map_err(internal)?;
                let wertersatz = lieferbeginn.and_then(|lb| {
                    crate::widerruf::wertersatz(nachweis.wertersatz_nachweis(), lb, abgesendet)
                });
                (Some("LIEFERENDE"), Some(ende), wertersatz)
            }
            other => {
                return Err(ApiError::conflict(format!("the contract is {other}"))
                    .with_code("vertrag_nicht_widerrufbar")
                    .with_fields(serde_json::json!({ "status": other })));
            }
        }
    } else {
        (None, None, None)
    };
    if rechtzeitig {
        store::vertrag_widerrufen(&mut tx, ctx.tenant(), vertrag_id)
            .await
            .map_err(internal)?;
    }
    let w = store::insert_widerruf(
        &mut tx,
        store::NeuerWiderruf {
            tenant: ctx.tenant(),
            gegenstand: store::WiderrufGegenstand::Vertrag(vertrag_id),
            kanal: input.kanal.as_db(),
            abgesendet_am: input.abgesendet_am,
            eingegangen_am: jetzt,
            erklaerung: erklaerung(input),
            frist: Some(frist),
            rechtzeitig,
            wirkung,
            lieferende,
            wertersatz_geschuldet: wertersatz,
        },
    )
    .await
    .map_err(internal)?;
    widerruf_folgen(&mut tx, ctx, b, &w, input, &vertrag.vertrags_nr).await?;
    tx.commit().await.map_err(internal)?;
    if rechtzeitig {
        Ok((StatusCode::CREATED, Json(widerruf_antwort(&w))))
    } else {
        Err(ApiError::unprocessable(
            "the Widerruf was sent after the Widerrufsfrist ended — recorded, its receipt \
             confirmed, and refused",
        )
        .with_code("widerrufsfrist_abgelaufen")
        .with_fields(serde_json::json!({
            "frist": frist_json(frist),
            "abgesendet_am": dokumente::berlin_iso(input.abgesendet_am),
            "widerruf_id": w.id,
        })))
    }
}

/// The day the Vertragsbestätigung carrying the Widerrufsbelehrung was proven
/// delivered, read from `outputd`'s delivery record; `None` while no delivery
/// is recorded (R3: a `SENT` e-mail proves no Zugang).
async fn vertragsbestaetigung_zugestellt(ctx: &Ctx, vertrag_id: Uuid) -> ApiResult<Option<Date>> {
    let Some(url) = ctx.cfg.outputd_url.as_deref() else {
        return Ok(None);
    };
    let client = dokumente::OutputdClient::new(url, ctx.cfg.outputd_api_key.clone());
    let doc = client
        .by_subject("VERTRAGSBESTAETIGUNG", &vertrag_id.to_string())
        .await
        .map_err(|e| ApiError::upstream("outputd", e))?;
    Ok(doc
        .and_then(|d| d.zugestellt_am())
        .map(mako_fristen::berlin_date))
}

/// What every recorded Widerruf owes in its transaction: the
/// `de.vertrag.widerruf.eingegangen` event and the § 356a Abs. 4 BGB
/// Eingangsbestätigung.
async fn widerruf_folgen(
    tx: &mut sqlx::PgConnection,
    ctx: &Ctx,
    b: &store::BestellungRow,
    w: &store::WiderrufRow,
    input: &WiderrufInput,
    vertragsnummer: &str,
) -> ApiResult<()> {
    enqueue_bestell_event(
        &mut *tx,
        ctx,
        mako_events::vertrag::WIDERRUF_EINGEGANGEN,
        &b.bestellreferenz,
        b.id,
        serde_json::json!({
            "bestellung_id": b.id,
            "bestellreferenz": b.bestellreferenz,
            "vertrag_id": w.vertrag_id,
            "widerruf": widerruf_antwort(w),
        }),
    )
    .await?;
    let empfaenger = PartyView {
        name: Some(input.name.clone()),
        email: Some(input.bestaetigung_an.clone()),
        ..PartyView::default()
    };
    let view = dokumente::WiderrufsbestaetigungView {
        datum: mako_fristen::heute().to_string(),
        absender: dokumente::absender(&ctx.cfg),
        empfaenger: empfaenger.clone(),
        vertragsnummer: vertragsnummer.to_owned(),
        eingegangen_am: dokumente::berlin_iso(w.eingegangen_am),
        abgesendet_am: Some(dokumente::berlin_iso(w.abgesendet_am)),
        kanal: w.kanal.clone(),
        erklaerung: dokumente::ErklaerungView {
            name: input.name.clone(),
            vertrag: input.vertrag.clone(),
            bestaetigung_an: input.bestaetigung_an.clone(),
        },
    };
    let subject = w.id.to_string();
    let task = outbound::dokument(
        "WIDERRUFSBESTAETIGUNG",
        &subject,
        dokumente::issue_body(
            serde_json::to_value(&view).map_err(internal)?,
            &empfaenger,
            &subject,
            None,
            &mako_fristen::heute().to_string(),
        ),
    );
    outbound::enqueue(&mut *tx, ctx.tenant(), &task)
        .await
        .map_err(internal)?;
    Ok(())
}

fn widerruf_antwort(w: &store::WiderrufRow) -> serde_json::Value {
    serde_json::json!({
        "widerruf_id": w.id,
        "rechtzeitig": w.rechtzeitig,
        "frist": {
            "beginn": w.frist_beginn.map(|d| d.to_string()),
            "ende": w.frist_ende.map(|d| d.to_string()),
            "erloeschen": w.erloeschen.map(|d| d.to_string()),
        },
        "wirkung": w.wirkung,
        "lieferende": w.lieferende.map(|d| d.to_string()),
        "wertersatz_geschuldet": w.wertersatz_geschuldet,
        "eingegangen_am": dokumente::berlin_iso(w.eingegangen_am),
        "eingangsbestaetigung": w.id,
    })
}
