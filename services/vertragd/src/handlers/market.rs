//! What the market tells the contract without being asked: the Netzbetreiber
//! ending this supplier's Zuordnung, an Ersatzversorgung it assigned, and the
//! previous supplier's answer to a Kündigung sent on the customer's behalf.
//!
//! Each runs on the inbound event's transaction, beside its idempotency
//! marker, so a redelivery changes nothing twice.

use mako_service::{ApiError, ApiResult};
use serde_json::Value;
use time::Date;
use uuid::Uuid;

use super::Ctx;
use crate::kuendigung_vorversorger::{self as kv, Klasse, KuendigungStatus, Reaktion};
use crate::{events, outbound, pg};

fn internal(e: impl Into<anyhow::Error>) -> ApiError {
    ApiError::Internal(e.into())
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// `YYYYMMDD` or `YYYY-MM-DD`, the two forms a process date travels in.
fn datum(s: &str) -> Option<Date> {
    let compact = time::macros::format_description!("[year][month][day]");
    let iso = time::macros::format_description!("[year]-[month]-[day]");
    Date::parse(s, compact)
        .or_else(|_| Date::parse(s, iso))
        .ok()
}

/// The process an event is about: `data.process_id` where an emitter states
/// one, else the CloudEvent `subject`, where makod names it.
fn process_id(ce: &Value) -> Option<&str> {
    ce.get("data")
        .and_then(|d| text(d, "process_id"))
        .or_else(|| text(ce, "subject"))
}

/// Route one inbound CloudEvent to the market facts it may carry.
pub(super) async fn apply(
    ctx: &Ctx,
    tx: &mut sqlx::PgConnection,
    ce_type: &str,
    ce: &Value,
) -> ApiResult<()> {
    let Some(data) = ce.get("data") else {
        return Ok(());
    };
    let pid = data.get("pid").and_then(Value::as_u64);
    match ce_type {
        mako_events::mako::PROCESS_INITIATED if matches!(pid, Some(55_037 | 44_037)) => {
            zuordnung_beendet(ctx, tx, data).await
        }
        mako_events::mako::PROCESS_COMPLETED => {
            if pid == Some(55_013) && text(data, "eog_art") == Some("ERSATZVERSORGUNG") {
                ersatzversorgung(ctx, tx, data).await?;
            }
            if let Some(pid) = process_id(ce) {
                kuendigung_antwort(ctx, tx, pid, ce).await?;
            }
            Ok(())
        }
        mako_events::mako::ANTWORTFRIST_ABGELAUFEN => match process_id(ce) {
            Some(pid) => kuendigung_frist_abgelaufen(ctx, tx, pid).await,
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

// ── The Netzbetreiber ends this supplier's Zuordnung (55037 / 44037) ─────────

/// Write the Zuordnungsende back to every running component the sender is
/// the Netzbetreiber of, and say so.
///
/// GPKE Teil 2 Kap. 2.1.2 SD Lieferbeginn Nr. 10 (S. 23): „teilt der NB dem
/// LFA insbesondere den Grund der Beendigung sowie das Zuordnungsende mit";
/// Gas AWH GeLi Gas Kap. 2.5.2 Nr. 6 (S. 45). Only a component whose
/// `nb_mp_id` is the sender is touched: another party's Meldung ends nothing
/// (SR-086).
async fn zuordnung_beendet(ctx: &Ctx, tx: &mut sqlx::PgConnection, data: &Value) -> ApiResult<()> {
    let (Some(malo_id), Some(nb)) = (text(data, "malo_id"), text(data, "grid_operator")) else {
        return Ok(());
    };
    let Some(ende) = text(data, "process_date").and_then(datum) else {
        // A missing date is no date: nothing is ended on a guess.
        tracing::warn!(
            malo_id,
            "vertragd: Beendigung der Zuordnung without Zuordnungsende — ignored"
        );
        return Ok(());
    };
    let grund = text(data, "transaktionsgrund")
        .unwrap_or_default()
        .to_owned();
    let quelle = match data.get("pid").and_then(Value::as_u64) {
        Some(44_037) => "44037",
        _ => "55037",
    };
    let komponenten: Vec<pg::VertragskomponenteRow> = sqlx::query_as(
        "SELECT k.* FROM vertragskomponenten k
           JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          WHERE k.tenant = $1 AND v.tenant = $1 AND k.malo_id = $2 AND k.nb_mp_id = $3
            AND k.status IN ('BESTAETIGT','AKTIV')",
    )
    .bind(ctx.tenant())
    .bind(malo_id)
    .bind(nb)
    .fetch_all(&mut *tx)
    .await
    .map_err(internal)?;
    if komponenten.is_empty() {
        tracing::info!(
            malo_id,
            nb,
            "vertragd: Beendigung der Zuordnung names no running component of this Netzbetreiber"
        );
    }
    for k in komponenten {
        if k.lieferende.is_some_and(|own| own <= ende) || ende < k.lieferbeginn {
            continue;
        }
        sqlx::query(
            "UPDATE vertragskomponenten
                SET lieferende = $2, zuordnungsende_quelle = $3, updated_at = now()
              WHERE id = $1",
        )
        .bind(k.id)
        .bind(ende)
        .bind(quelle)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        // The Schlussrechnung needs its reading whoever ended the supply.
        let ablesung = outbound::ablesung(k.id, malo_id, &k.sparte, true, ende);
        outbound::enqueue_superseding(&mut *tx, ctx.tenant(), &ablesung)
            .await
            .map_err(internal)?;
        let vertrag = pg::vertraege::vertragsbezug(&mut *tx, k.vertrag_id, ctx.tenant())
            .await
            .map_err(internal)?;
        let ce = events::typed_event(
            k.vertrag_id,
            ctx.tenant(),
            &events::ZuordnungBeendet {
                vertrag,
                komponente: komponentenbezug(&k),
                zuordnungsende: ende.to_string(),
                wechsel: grund == "E03",
                grund: grund.clone(),
                netzbetreiber: nb.to_owned(),
            },
        )
        .map_err(internal)?;
        mako_service::outbox::enqueue(&mut *tx, &ce)
            .await
            .map_err(internal)?;
    }
    Ok(())
}

fn komponentenbezug(k: &pg::VertragskomponenteRow) -> events::Komponentenbezug {
    events::Komponentenbezug {
        komp_id: k.id,
        sparte: k.sparte.clone(),
        malo_id: k.malo_id.clone(),
        lieferbeginn: k.lieferbeginn.to_string(),
    }
}

// ── An Ersatzversorgung the Netzbetreiber assigned (55013) ───────────────────

/// Open the Ersatzversorgung this E/G accepted on a 55013.
///
/// Keyed on (tenant, MaLo, Zuordnungsbeginn): a redelivery opens nothing
/// (SR-091). It ends three months after it begins at the latest (§ 38 Abs. 4
/// Satz 1 EnWG) and needs no Anmeldung — the Netzbetreiber assigned it.
async fn ersatzversorgung(ctx: &Ctx, tx: &mut sqlx::PgConnection, data: &Value) -> ApiResult<()> {
    if text(data, "new_supplier") != Some(ctx.cfg.lf_mp_id.as_str()) {
        return Ok(());
    }
    let (Some(malo_id), Some(beginn)) = (
        text(data, "malo_id"),
        text(data, "process_date").and_then(datum),
    ) else {
        return Ok(());
    };
    let Some(produkt) = ctx.cfg.ersatzversorgung_produkt.as_deref() else {
        tracing::warn!(
            malo_id,
            "vertragd: Ersatzversorgung accepted, but no ersatzversorgung_produkt is configured — \
             no contract opened"
        );
        return Ok(());
    };
    let eroeffnet: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM ersatzversorgung_eroeffnungen
                         WHERE tenant = $1 AND malo_id = $2 AND zuordnungsbeginn = $3)",
    )
    .bind(ctx.tenant())
    .bind(malo_id)
    .bind(beginn)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    if eroeffnet {
        return Ok(());
    }

    let haushaltskunde = data
        .get("haushaltskunde")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let gp = data.get("kunde_name").and_then(geschaeftspartner);
    let kunden_id: Uuid = sqlx::query_scalar(
        "INSERT INTO kunden (tenant, kundentyp, haushaltskunde, geschaeftspartner)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(ctx.tenant())
    .bind(if haushaltskunde { "B2C" } else { "B2B_SLP" })
    .bind(haushaltskunde)
    .bind(gp)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;

    let ende = crate::domain::add_months(beginn, 3);
    let input = pg::CreateVersorgungsvertragInput {
        rahmenvertrag_id: None,
        kundentyp: (if haushaltskunde { "B2C" } else { "B2B_SLP" }).to_owned(),
        verbraucher: None,
        vertragsart: Some("ERSATZVERSORGUNG".to_owned()),
        bundle_code: None,
        vertragsbeginn: beginn,
        vertragsende: Some(ende),
        kuendigungsfrist_monate: None,
        preisgarantie_bis: None,
        abrechnungszyklus: None,
        auto_renewal: None,
        renewal_monate: None,
        standort_bezeichnung: None,
        standort_adresse: None,
        zahlungsziel_tage: None,
        erp_contract_id: None,
        notizen: None,
        komponenten: vec![pg::CreateKomponenteInput {
            sparte: "STROM".to_owned(),
            malo_id: Some(malo_id.to_owned()),
            melo_id: None,
            nb_mp_id: text(data, "grid_operator").map(str::to_owned),
            bilanzkreis: text(data, "bilanzkreis").map(str::to_owned),
            product_code: produkt.to_owned(),
            lieferbeginn: beginn,
            lieferende: Some(ende),
            jahresverbrauch_kwh: None,
            lieferbeginn_anlass: None,
            netznutzungsvertrag: None,
            netznutzung_zahler: None,
            gemeinderabatt_prozent: None,
            enfg_grundlage: None,
            enfg_grund: None,
            fulfillment_data: None,
            sect14a: None,
            vorversorger: None,
            kuendigung_beim_vorversorger: None,
        }],
        eog_quelle: Some("55013"),
    };
    let inserted = pg::insert_versorgungsvertrag_tx(
        &mut *tx,
        kunden_id,
        ctx.tenant(),
        &ctx.cfg.lf_mp_id,
        &input,
        None,
    )
    .await
    .map_err(internal)?;
    sqlx::query(
        "INSERT INTO ersatzversorgung_eroeffnungen (tenant, malo_id, zuordnungsbeginn, vertrag_id)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(ctx.tenant())
    .bind(malo_id)
    .bind(beginn)
    .bind(inserted.id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;

    // The Netzbetreiber's Zuordnung is the confirmation: the component goes
    // into supply exactly as a confirmed Anmeldung would.
    for k in &inserted.komponenten {
        let row: pg::VertragskomponenteRow =
            sqlx::query_as("SELECT * FROM vertragskomponenten WHERE id = $1")
                .bind(k.id)
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?;
        let outcome = events::MakoOutcome {
            process_id: None,
            malo_id: Some(malo_id.to_owned()),
            confirmed: true,
            erc_code: None,
            codeliste: None,
            reason: None,
            antwortcode: None,
            antwort_codeliste: None,
            process_date: None,
        };
        super::inbound::apply_outcome(ctx, &mut *tx, &row, &outcome).await?;
    }
    Ok(())
}

/// The NB's `NAD+Z67` as a BO4E `Geschaeftspartner`: a Personenname (`Z01`)
/// is Nachname and Vorname, a Firmenbezeichnung (`Z02`) the
/// Organisationsname. `None` where it names nobody.
///
/// Built typed, so `rubo4e` stamps the discriminant and spells every field.
fn geschaeftspartner(name: &Value) -> Option<Value> {
    let teile: Vec<&str> = name
        .get("teile")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .filter(|t| !t.is_empty())
        .collect();
    let (erster, rest) = teile.split_first()?;
    let gp = if name.get("namensformat").and_then(Value::as_str) == Some("Z02") {
        rubo4e::current::Geschaeftspartner {
            organisationsname: Some(teile.join(" ")),
            ..Default::default()
        }
    } else {
        rubo4e::current::Geschaeftspartner {
            nachname: Some((*erster).to_owned()),
            vorname: rest.first().map(|v| (*v).to_owned()),
            ..Default::default()
        }
    };
    mako_markt::bo4e::ensure_conformant(&gp).ok()?;
    serde_json::to_value(gp).ok()
}

// ── The previous supplier's answer to the Kündigung (55017 / 55018) ─────────

/// Apply the old supplier's answer to the component waiting on it.
async fn kuendigung_antwort(
    ctx: &Ctx,
    tx: &mut sqlx::PgConnection,
    process_id: &str,
    ce: &Value,
) -> ApiResult<()> {
    let Some(k) = wartende_komponente(tx, ctx.tenant(), process_id).await? else {
        return Ok(());
    };
    let Some(outcome) = events::parse_mako_outcome(ce) else {
        return Ok(());
    };
    let klasse = match (
        outcome.antwort_codeliste.as_deref(),
        outcome.antwortcode.as_deref(),
    ) {
        (Some(liste), Some(code)) => kv::klasse(liste, code),
        _ => None,
    };
    // A confirmation without a readable code is still a confirmation; a
    // refusal mako cannot read is held, never guessed into a class.
    let klasse = klasse.or(outcome.confirmed.then_some(Klasse::Zustimmung));
    let reaktion = klasse.map_or(Reaktion::Gehalten, |k| {
        kv::reaktion(k, ctx.cfg.policy.nach_kuendigungsablehnung.get())
    });
    let status = if outcome.confirmed {
        KuendigungStatus::Bestaetigt
    } else {
        KuendigungStatus::Abgelehnt
    };
    sqlx::query(
        "UPDATE vertragskomponenten
            SET kuendigung_status = $2, kuendigung_antwortcode = $3, kuendigung_codeliste = $4,
                kuendigung_bestaetigt_zum = CASE WHEN $5 THEN kuendigung_zum END,
                updated_at = now()
          WHERE id = $1",
    )
    .bind(k.komp_id)
    .bind(status.as_db())
    .bind(&outcome.antwortcode)
    .bind(&outcome.antwort_codeliste)
    .bind(outcome.confirmed)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    if reaktion == Reaktion::Angemeldet {
        anmeldung_freigeben(tx, ctx.tenant(), &k).await?;
    }

    let row: pg::VertragskomponenteRow =
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE id = $1")
            .bind(k.komp_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
    let vertrag = pg::vertraege::vertragsbezug(&mut *tx, row.vertrag_id, ctx.tenant())
        .await
        .map_err(internal)?;
    let ce = if outcome.confirmed {
        events::typed_event(
            row.vertrag_id,
            ctx.tenant(),
            &events::KuendigungVorversorgerBestaetigt {
                vertrag,
                komponente: komponentenbezug(&row),
                vorversorger_mp_id: k.vorversorger_mp_id.clone(),
                kuendigung_zum: k.kuendigung_zum.to_string(),
                antwortcode: outcome.antwortcode.clone(),
                codeliste: outcome.antwort_codeliste.clone(),
                reaktion,
            },
        )
    } else {
        events::typed_event(
            row.vertrag_id,
            ctx.tenant(),
            &events::KuendigungVorversorgerAbgelehnt {
                vertrag,
                komponente: komponentenbezug(&row),
                vorversorger_mp_id: k.vorversorger_mp_id.clone(),
                antwortcode: outcome.antwortcode.clone(),
                codeliste: outcome.antwort_codeliste.clone(),
                reason: outcome.reason.clone(),
                klasse,
                reaktion,
            },
        )
    }
    .map_err(internal)?;
    mako_service::outbox::enqueue(&mut *tx, &ce)
        .await
        .map_err(internal)?;
    Ok(())
}

/// No answer inside the old supplier's Frist: the Anmeldung goes out, and the
/// Netzbetreiber's 55010 settles the Zuordnung — its lapse „gilt … als
/// Bestätigung" (GPKE Teil 2 Kap. 2.1.2, S. 22).
async fn kuendigung_frist_abgelaufen(
    ctx: &Ctx,
    tx: &mut sqlx::PgConnection,
    process_id: &str,
) -> ApiResult<()> {
    let Some(k) = wartende_komponente(tx, ctx.tenant(), process_id).await? else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE vertragskomponenten SET kuendigung_status = $2, updated_at = now() WHERE id = $1",
    )
    .bind(k.komp_id)
    .bind(KuendigungStatus::FristAbgelaufen.as_db())
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    anmeldung_freigeben(tx, ctx.tenant(), &k).await
}

/// A component whose Kündigung is still waiting on `process_id`.
pub(super) struct Wartend {
    pub komp_id: Uuid,
    pub vorversorger_mp_id: String,
    pub kuendigung_zum: Date,
    pub anmeldung: Option<Value>,
}

async fn wartende_komponente(
    tx: &mut sqlx::PgConnection,
    tenant: &str,
    process_id: &str,
) -> ApiResult<Option<Wartend>> {
    let row: Option<(Uuid, String, Date, Option<Value>)> = sqlx::query_as(
        "SELECT id, vorversorger_mp_id, kuendigung_zum, kuendigung_wartende_anmeldung
           FROM vertragskomponenten
          WHERE tenant = $1 AND kuendigung_process_id = $2 AND kuendigung_status = $3",
    )
    .bind(tenant)
    .bind(process_id)
    .bind(KuendigungStatus::Ausstehend.as_db())
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;
    Ok(row.map(
        |(komp_id, vorversorger_mp_id, kuendigung_zum, anmeldung)| Wartend {
            komp_id,
            vorversorger_mp_id,
            kuendigung_zum,
            anmeldung,
        },
    ))
}

/// Enqueue the held Anmeldung. Supply under the new contract begins no earlier
/// than the day the old one ends.
pub(super) async fn anmeldung_freigeben(
    tx: &mut sqlx::PgConnection,
    tenant: &str,
    k: &Wartend,
) -> ApiResult<()> {
    let Some(payload) = k.anmeldung.clone() else {
        return Ok(());
    };
    let lieferbeginn: Option<Date> =
        sqlx::query_scalar("SELECT lieferbeginn FROM vertragskomponenten WHERE id = $1")
            .bind(k.komp_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
    let ab = lieferbeginn
        .filter(|lb| *lb < k.kuendigung_zum)
        .map(|_| k.kuendigung_zum);
    let task = outbound::gehaltener_lieferbeginn(k.komp_id, payload, ab);
    outbound::enqueue(&mut *tx, tenant, &task)
        .await
        .map_err(internal)?;
    sqlx::query(
        "UPDATE vertragskomponenten SET kuendigung_wartende_anmeldung = NULL, updated_at = now()
          WHERE id = $1",
    )
    .bind(k.komp_id)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_personenname_is_nachname_then_vorname() {
        let gp = geschaeftspartner(&serde_json::json!({
            "namensformat": "Z01", "teile": ["Mustermann", "Erika"]
        }))
        .expect("a name");
        assert_eq!(
            (gp["nachname"].as_str(), gp["vorname"].as_str()),
            (Some("Mustermann"), Some("Erika"))
        );
        let firma = geschaeftspartner(&serde_json::json!({
            "namensformat": "Z02", "teile": ["Stadtwerke", "Musterstadt GmbH"]
        }))
        .expect("a name");
        assert_eq!(firma["organisationsname"], "Stadtwerke Musterstadt GmbH");
        assert!(geschaeftspartner(&serde_json::json!({ "teile": [] })).is_none());
    }

    #[test]
    fn both_process_date_forms_are_read() {
        assert_eq!(datum("20270401"), Some(time::macros::date!(2027 - 04 - 01)));
        assert_eq!(
            datum("2027-04-01"),
            Some(time::macros::date!(2027 - 04 - 01))
        );
        assert_eq!(datum("01.04.2027"), None);
    }
}
