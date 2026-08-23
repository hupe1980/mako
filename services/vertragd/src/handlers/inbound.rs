//! Signed inbound webhooks: MaKo process outcomes and accepted CPQ quotations.
//!
//! Neither route carries an operator token — they are called by `makod`,
//! `processd` and `tarifbd`, which authenticate with the shared
//! Standard Webhooks signature over the raw body. `main` refuses to start without
//! `inbound_secret` unless the deployment asked for an unauthenticated posture
//! by name: a forged CloudEvent here creates contracts and moves supply.

use std::sync::Arc;

use axum::{Extension, Json, body::Bytes, http::HeaderMap, http::StatusCode};
use mako_service::{ApiError, ApiResult};
use time::Date;
use uuid::Uuid;

use super::Ctx;
use crate::{
    angebot_bo4e,
    domain::{self, Vertragsart},
    events::{build_cloud_event, parse_mako_outcome},
    outbound, pg,
};

/// Reject a body whose signature does not match before it can mutate anything.
fn verify_signature(ctx: &Ctx, headers: &HeaderMap, body: &Bytes) -> ApiResult<()> {
    mako_service::webhook::verify_request(
        ctx.cfg.inbound_secret.as_deref().map(str::as_bytes),
        headers,
        body,
    )
    .map(|_| ())
    .map_err(|err| {
        tracing::warn!(%err, "vertragd: inbound webhook refused");
        ApiError::Unauthorized
    })
}

// ── MaKo process outcomes ─────────────────────────────────────────────────────

/// `POST /api/v1/events` — inbound CloudEvents from `makod` / `processd`.
///
/// A confirmation moves the component into supply and, in the same
/// transaction, enqueues everything that follows from it: the GPKE
/// Beginnablesung, the tariff assignment `billingd` prices from, the billing
/// account, and — once every commodity is in supply — `de.vertrag.aktiv`. None
/// of that is a detached task any more, so a restart mid-confirmation costs a
/// retry rather than a lost obligation.
pub async fn cloud_event(
    Extension(ctx): Extension<Arc<Ctx>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    verify_signature(&ctx, &headers, &body)?;
    let ce: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("malformed CloudEvent: {e}")))?;

    // `id` is the inbox primary key. Defaulting it to "" made the first id-less
    // event occupy that key and every later one look like its duplicate, so all
    // of them were dropped in silence.
    let ce_id = ce
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::bad_request("CloudEvent `id` is required — it is the deduplication key")
        })?;
    let ce_type = ce
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();

    if !pg::idempotent_event(&ctx.pool, ce_id, ce_type, &ce)
        .await
        .map_err(ApiError::Internal)?
    {
        return Ok(StatusCode::OK);
    }

    let Some(outcome) = parse_mako_outcome(&ce) else {
        return Ok(StatusCode::OK);
    };
    let Some(process_id) = outcome.process_id.as_deref() else {
        tracing::warn!(
            ce_type,
            "vertragd: MaKo outcome without process_id — ignored"
        );
        return Ok(StatusCode::OK);
    };

    let komponenten: Vec<pg::VertragskomponenteRow> = sqlx::query_as(
        "SELECT k.* FROM vertragskomponenten k
         JOIN versorgungsvertraege v ON v.id = k.vertrag_id
         WHERE k.mako_process_id=$1 AND v.tenant=$2",
    )
    .bind(process_id)
    .bind(ctx.tenant())
    .fetch_all(&ctx.pool)
    .await
    .map_err(|e| ApiError::Internal(e.into()))?;

    for k in &komponenten {
        apply_outcome(&ctx, k, &outcome).await?;
    }
    Ok(StatusCode::OK)
}

/// Apply one process outcome to one component and everything downstream of it.
async fn apply_outcome(
    ctx: &Ctx,
    k: &pg::VertragskomponenteRow,
    outcome: &crate::events::MakoOutcome,
) -> ApiResult<()> {
    let mut tx = ctx
        .pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    let new_status = if outcome.confirmed {
        "BESTAETIGT"
    } else {
        "ABGELEHNT"
    };
    pg::update_komponente_status(
        &mut *tx,
        k.id,
        new_status,
        None,
        outcome.malo_id.as_deref(),
        outcome.erc_code.as_deref(),
        outcome.reason.as_deref(),
    )
    .await
    .map_err(ApiError::Internal)?;

    if outcome.confirmed {
        // The MaLo the NB confirmed wins over the one requested — a
        // Lieferbeginn may be answered with a corrected identifier.
        let malo_id = outcome.malo_id.clone().or_else(|| k.malo_id.clone());
        if let Some(malo_id) = malo_id.as_deref() {
            for task in [
                outbound::ablesung(k.id, malo_id, false, k.lieferbeginn),
                outbound::abrechnungskonto(k.id, malo_id, &ctx.cfg.lf_mp_id),
            ] {
                outbound::enqueue(&mut *tx, ctx.tenant(), &task)
                    .await
                    .map_err(ApiError::Internal)?;
            }
        }
    }
    tx.commit()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;

    // Recompute the contract status from the components as they now stand.
    let alle = pg::list_komponenten(&ctx.pool, k.vertrag_id)
        .await
        .map_err(ApiError::Internal)?;
    let status = pg::derive_vertrag_status(&alle);
    let mut tx = ctx
        .pool
        .begin()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    pg::update_vertrag_status(&mut *tx, k.vertrag_id, ctx.tenant(), status)
        .await
        .map_err(ApiError::Internal)?;
    if status == "AKTIV" {
        let ce = build_cloud_event(
            mako_events::vertrag::AKTIV,
            k.vertrag_id,
            ctx.tenant(),
            serde_json::json!({ "vertrag_id": k.vertrag_id }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce)
            .await
            .map_err(|e| ApiError::Internal(e.into()))?;
    }
    tx.commit()
        .await
        .map_err(|e| ApiError::Internal(e.into()))?;
    Ok(())
}

// ── CPQ: accepted quotation → contract ───────────────────────────────────────

/// `POST /api/v1/webhooks/angebot` — `de.tarif.angebot.angenommen` from
/// `tarifbd`.
///
/// Creates the Rahmenvertrag and one Versorgungsvertrag per site, from the
/// **accepted variant of the BO4E `Angebot`**: what the customer was quoted and
/// what is contracted come from one document, so they cannot drift. The scalar
/// fields on the event are the fallback for a quotation accepted before it was
/// ever priced.
///
/// Idempotent twice over — on the CloudEvent id, and on
/// `erp_rahmenvertrag_id = angebot_id` — so a redelivery creates nothing.
pub async fn angebot(
    Extension(ctx): Extension<Arc<Ctx>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    verify_signature(&ctx, &headers, &body)?;
    let ce: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("malformed CloudEvent: {e}")))?;

    if ce.get("type").and_then(serde_json::Value::as_str)
        != Some(mako_events::tarif::ANGEBOT_ANGENOMMEN)
    {
        return Err(ApiError::bad_request(format!(
            "expected {}",
            mako_events::tarif::ANGEBOT_ANGENOMMEN
        )));
    }
    let ce_id = ce
        .get("id")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("CloudEvent `id` is required"))?;
    let data = ce
        .get("data")
        .ok_or_else(|| ApiError::bad_request("missing data field"))?;

    let angebot_id: Uuid = data
        .get("angebot_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| ce.get("subject").and_then(serde_json::Value::as_str))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ApiError::bad_request("invalid or missing angebot_id"))?;

    // Deduplicate on the event as well as on the contract: the second delivery
    // must not even attempt the work.
    if !pg::idempotent_event(
        &ctx.pool,
        ce_id,
        mako_events::tarif::ANGEBOT_ANGENOMMEN,
        &ce,
    )
    .await
    .map_err(ApiError::Internal)?
    {
        return Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "angebot_id": angebot_id,
                "idempotent_replay": true,
            })),
        ));
    }

    let gewaehlte_variante = data
        .get("gewaehlte_variante")
        .and_then(serde_json::Value::as_i64)
        .and_then(|v| i16::try_from(v).ok());
    let accepted = angebot_bo4e::from_ce_data(data)
        .as_ref()
        .and_then(|a| angebot_bo4e::read_accepted(a, gewaehlte_variante));

    let laufzeit_monate: i32 = accepted
        .as_ref()
        .and_then(|a| a.laufzeit_monate)
        .or_else(|| {
            data.get("laufzeit_monate")
                .and_then(serde_json::Value::as_i64)
                .and_then(|v| i32::try_from(v).ok())
        })
        .unwrap_or(12);

    let lieferbeginn = accepted
        .as_ref()
        .and_then(|a| a.lieferbeginn)
        .or_else(|| {
            data.get("lieferbeginn")
                .and_then(serde_json::Value::as_str)
                .and_then(|s| {
                    Date::parse(s, &time::format_description::well_known::Iso8601::DATE).ok()
                })
        })
        .unwrap_or_else(|| domain::naechster_monatserster(time::OffsetDateTime::now_utc().date()));

    // The last day of the term, not the same day one year on: a twelve-month
    // contract starting 1 January ends on 31 December. `year + monate / 12`
    // truncated every term that was not a whole number of years — an eighteen-
    // month contract became twelve.
    let vertragsende = accepted.as_ref().and_then(|a| a.lieferende).or_else(|| {
        (laufzeit_monate > 0)
            .then(|| domain::add_months(lieferbeginn, laufzeit_monate) - time::Duration::days(1))
    });

    let angebotsnummer = accepted
        .as_ref()
        .and_then(|a| a.angebotsnummer.clone())
        .or_else(|| {
            data.get("angebotsnummer")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| angebot_id.to_string());

    let kunden_id = resolve_kunde(&ctx, data, angebot_id, &angebotsnummer).await?;

    let rahmen_input = pg::CreateRahmenvertragInput {
        gueltig_von: lieferbeginn,
        gueltig_bis: vertragsende,
        kuendigungsfrist_monate: Some(3),
        // A quotation is a fixed-term commitment; it does not roll over.
        auto_renewal: Some(false),
        renewal_monate: Some(0),
        preisanpassungsformel: None,
        portfolio_rabatt_prozent: rabatt(data, gewaehlte_variante),
        rechnungsstellung: Some("SAMMEL".to_owned()),
        sammelrechnung_intervall: Some("JAEHRLICH".to_owned()),
        erp_rahmenvertrag_id: Some(angebot_id.to_string()),
        angebot_id: Some(angebot_id),
        notizen: Some(format!("CPQ-Angebot {angebotsnummer} angenommen")),
    };
    let rahmenvertrag_id =
        pg::insert_rahmenvertrag(&ctx.pool, kunden_id, ctx.tenant(), &rahmen_input)
            .await
            .map_err(ApiError::Internal)?;

    // One Versorgungsvertrag per site. The supply points come from the accepted
    // variant when the quotation carried a BO4E `Angebot`; only a quotation
    // that never had one falls back to the flat `positionen` array.
    let sites = collect_sites(data, accepted.as_ref());
    let mut vertrag_ids = Vec::new();
    let mut fehler = Vec::new();
    for (standort, komponenten) in sites {
        if komponenten.is_empty() {
            continue;
        }
        let vv_input = pg::CreateVersorgungsvertragInput {
            rahmenvertrag_id: Some(rahmenvertrag_id),
            kundentyp: "B2B_RLM".to_owned(),
            vertragsart: Some(Vertragsart::Sondervertrag.as_db().to_owned()),
            bundle_code: None,
            vertragsbeginn: lieferbeginn,
            vertragsende,
            kuendigungsfrist_monate: Some(3),
            // A fixed-term quotation is priced for its whole term.
            preisgarantie_bis: vertragsende,
            abrechnungszyklus: None,
            auto_renewal: Some(false),
            renewal_monate: Some(0),
            standort_bezeichnung: Some(standort.clone()),
            standort_adresse: None,
            zahlungsziel_tage: None,
            erp_contract_id: Some(format!("{angebot_id}-{standort}")),
            notizen: None,
            komponenten: komponenten
                .into_iter()
                .map(|k| pg::CreateKomponenteInput {
                    lieferbeginn,
                    lieferende: vertragsende,
                    ..k
                })
                .collect(),
        };
        match pg::insert_versorgungsvertrag(
            &ctx.pool,
            kunden_id,
            ctx.tenant(),
            &ctx.cfg.lf_mp_id,
            &vv_input,
        )
        .await
        {
            Ok(inserted) => vertrag_ids.push(inserted.id),
            Err(e) => {
                tracing::error!(error = %e, standort, "vertragd: CPQ site could not be contracted");
                fehler.push(serde_json::json!({ "standort": standort, "fehler": e.to_string() }));
            }
        }
    }

    tracing::info!(
        %angebot_id, %rahmenvertrag_id, sites = vertrag_ids.len(), fehler = fehler.len(),
        "vertragd: CPQ-Angebot angenommen → Rahmenvertrag und Versorgungsverträge erstellt"
    );
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "rahmenvertrag_id": rahmenvertrag_id,
            "kunden_id": kunden_id,
            "versorgungsvertrag_ids": vertrag_ids,
            "fehler": fehler,
        })),
    ))
}

/// The customer the quotation belongs to, creating a prospect when the
/// quotation named none.
async fn resolve_kunde(
    ctx: &Ctx,
    data: &serde_json::Value,
    angebot_id: Uuid,
    angebotsnummer: &str,
) -> ApiResult<Uuid> {
    if let Some(kid) = data
        .get("kunden_id")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse::<Uuid>().ok())
    {
        return pg::fetch_kunde(&ctx.pool, kid, ctx.tenant())
            .await
            .map_err(ApiError::Internal)?
            .map(|k| k.id)
            .ok_or_else(|| ApiError::bad_request("kunden_id not found in this tenant"));
    }
    let name = data
        .get("interessent_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Unbekannt")
        .to_owned();
    let contact_email = data
        .get("contact_email")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    // A real BO4E `Geschaeftspartner`, so `fetch_rechnungsempfaenger_by_malo`'s
    // typed read finds it: a CPQ prospect is an organisation, so the name is
    // `organisationsname`, and the contact address rides as a `Kontaktweg`
    // where `outputd` can find it.
    let mut geschaeftspartner = serde_json::json!({
        "_typ": "GESCHAEFTSPARTNER",
        "organisationsname": name,
    });
    if let Some(ref mail) = contact_email {
        geschaeftspartner["kontaktwege"] = serde_json::json!([{
            "_typ": "KONTAKTWEG",
            "kontaktart": "E_MAIL",
            "kontaktwert": mail,
            "istBevorzugterKontaktweg": true,
        }]);
    }
    let input = pg::CreateKundeInput {
        kunden_nr: Some(angebotsnummer.to_owned()),
        oidc_sub: None,
        email: contact_email,
        kundentyp: "B2B_SLP".to_owned(),
        // A CPQ quotation is a commercial one; the § 41 Abs. 5 and § 309 Nr. 9
        // consumer rules do not apply to it.
        haushaltskunde: Some(false),
        geschaeftspartner: Some(geschaeftspartner),
        organisations_id: None,
        umsatzsteuer_id: None,
        zahlungsziel_tage: Some(30),
        sepa_erlaubt: Some(false),
        erp_kunde_id: Some(angebot_id.to_string()),
        stromwiederverkaeufer: None,
        notizen: Some(format!("Interessent aus CPQ-Angebot {angebotsnummer}")),
    };
    pg::upsert_kunde(&ctx.pool, ctx.tenant(), &input)
        .await
        .map_err(ApiError::Internal)
}

/// Group the accepted supply points by site.
fn collect_sites(
    data: &serde_json::Value,
    accepted: Option<&angebot_bo4e::AcceptedQuotation>,
) -> std::collections::BTreeMap<String, Vec<pg::CreateKomponenteInput>> {
    let mut by_standort: std::collections::BTreeMap<String, Vec<pg::CreateKomponenteInput>> =
        std::collections::BTreeMap::new();
    let placeholder = Date::from_calendar_date(2000, time::Month::January, 1)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH.date());

    if let Some(accepted) = accepted.filter(|a| !a.supply_points.is_empty()) {
        for sp in &accepted.supply_points {
            let (Some(sparte), Some(product_code)) = (sp.sparte.clone(), sp.product_code.clone())
            else {
                continue;
            };
            let standort = sp
                .standort_bezeichnung
                .clone()
                .unwrap_or_else(|| "Hauptstandort".to_owned());
            by_standort
                .entry(standort)
                .or_default()
                .push(pg::CreateKomponenteInput {
                    sparte,
                    malo_id: sp.malo_id.clone(),
                    melo_id: sp.melo_id.clone(),
                    nb_mp_id: sp.nb_mp_id.clone(),
                    product_code,
                    lieferbeginn: placeholder,
                    lieferende: None,
                    fulfillment_data: None,
                });
        }
        if !by_standort.is_empty() {
            return by_standort;
        }
    }

    for pos in data
        .get("positionen")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let s = |k: &str| {
            pos.get(k)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        };
        let (Some(sparte), Some(product_code)) = (s("sparte"), s("product_code")) else {
            continue;
        };
        let standort = s("standort_bezeichnung").unwrap_or_else(|| "Hauptstandort".to_owned());
        by_standort
            .entry(standort)
            .or_default()
            .push(pg::CreateKomponenteInput {
                sparte,
                malo_id: s("malo_id"),
                melo_id: s("melo_id"),
                nb_mp_id: s("nb_mp_id"),
                product_code,
                lieferbeginn: placeholder,
                lieferende: None,
                fulfillment_data: None,
            });
    }
    by_standort
}

/// The discount of the accepted variant, when the quotation carried one.
fn rabatt(
    data: &serde_json::Value,
    gewaehlte_variante: Option<i16>,
) -> Option<rust_decimal::Decimal> {
    let idx = usize::try_from(gewaehlte_variante.unwrap_or(0).max(0)).ok()?;
    data.get("varianten")?
        .as_array()?
        .get(idx)?
        .get("rabatt_pct")?
        .as_str()?
        .parse()
        .ok()
}
