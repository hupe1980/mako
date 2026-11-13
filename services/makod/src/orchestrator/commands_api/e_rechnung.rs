//! Whether an outbound INVOIC may go out as EDI to this recipient.
//!
//! Every command that issues an INVOIC — `invoic.nne-abschlag.stellen`,
//! `invoic.nne.stellen`, `invoic.mmm.stellen`, `wim.msb-rechnung.stellen` and
//! `invoic.sonstige-leistung.stellen` —
//! asks [`mako_fristen::e_rechnung::edi_zulaessig`] before it reaches the
//! issuer's process. A Marktpartner is an Unternehmer, so § 14 Abs. 2 Satz 2
//! Nr. 1 UStG makes the E-Rechnung mandatory, and the EDI invoice stands only
//! as an E-Rechnung (§ 14 Abs. 1 Satz 6 Nr. 2 UStG) or under a § 27 Abs. 38
//! UStG grace — each only with the recipient's Zustimmung.
//!
//! The command is the boundary because it is the one place every outbound
//! INVOIC passes, and the caller (`netzbilanzd`, `invoicd`) learns the refusal
//! in the same response, before it records the invoice as dispatched.
//!
//! | `EdiFall` field | Source |
//! |---|---|
//! | `ausgefuehrt` | `rechnung.rechnungsperiode.enddatum` — inclusive in BO4E |
//! | `ausgestellt` | `rechnung.rechnungsdatum`, the date in the offset it carries |
//! | `uebermittelt` | [`mako_fristen::heute`] when the command is accepted |
//! | `zustimmung` / `extrahierbar` | marktd's partner record of the recipient |
//!
//! Each is refused when absent: a missing period end, a missing document
//! date, an unconfigured or unreachable marktd. A partner marktd does not
//! know has nothing recorded, and the rule refuses that itself.

use mako_fristen::e_rechnung::{EdiFall, Zulaessig, edi_zulaessig};
use mako_markt::marktd_client::MarktdClient;
use time::Date;

use super::DispatchError;

/// The two invoice dates the rule reads, projected out of the BO4E `Rechnung`.
#[derive(serde::Deserialize)]
struct RechnungDaten {
    #[serde(default)]
    rechnungsperiode: Option<rubo4e::current::Zeitraum>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    rechnungsdatum: Option<time::OffsetDateTime>,
}

/// `(ausgefuehrt, ausgestellt)` from the payload's `rechnung`.
fn invoice_dates(payload: &serde_json::Value) -> Result<(Date, Date), DispatchError> {
    let rechnung = payload.get("rechnung").ok_or_else(|| {
        DispatchError::InvalidPayload(
            "payload must contain \"rechnung\" (BO4E RECHNUNG): its Rechnungsperiode and \
             Rechnungsdatum decide whether the INVOIC may go out (§ 27 Abs. 38 UStG)"
                .into(),
        )
    })?;
    let daten: RechnungDaten = serde_json::from_value(rechnung.clone()).map_err(|e| {
        DispatchError::InvalidPayload(format!(
            "rechnung.rechnungsperiode / rechnung.rechnungsdatum do not decode: {e}"
        ))
    })?;
    let ausgefuehrt = daten
        .rechnungsperiode
        .and_then(|z| z.enddatum)
        .ok_or_else(|| {
            DispatchError::InvalidPayload(
                "rechnung.rechnungsperiode.enddatum is absent — when the Umsatz was \
                 ausgeführt decides which § 27 Abs. 38 UStG grace applies, and it is not \
                 assumed"
                    .into(),
            )
        })?;
    let ausgestellt = daten.rechnungsdatum.map(|t| t.date()).ok_or_else(|| {
        DispatchError::InvalidPayload(
            "rechnung.rechnungsdatum is absent — the date the invoice is ausgestellt \
             decides § 27 Abs. 38 Nr. 3 UStG, and it is not assumed"
                .into(),
        )
    })?;
    Ok((ausgefuehrt, ausgestellt))
}

/// Check an outbound INVOIC to `recipient`, transmitted today.
pub(super) async fn edi_pruefen(
    marktd: Option<&MarktdClient>,
    payload: &serde_json::Value,
    recipient: &str,
) -> Result<Zulaessig, DispatchError> {
    edi_pruefen_am(marktd, payload, recipient, mako_fristen::heute()).await
}

/// Check an outbound INVOIC to `recipient`, transmitted on `uebermittelt`.
pub(super) async fn edi_pruefen_am(
    marktd: Option<&MarktdClient>,
    payload: &serde_json::Value,
    recipient: &str,
    uebermittelt: Date,
) -> Result<Zulaessig, DispatchError> {
    let (ausgefuehrt, ausgestellt) = invoice_dates(payload)?;
    if recipient.is_empty() {
        return Err(DispatchError::InvalidPayload(
            "the INVOIC names no recipient MP-ID — whose EDI agreement it rests on cannot \
             be read"
                .into(),
        ));
    }
    let Some(marktd) = marktd else {
        return Err(DispatchError::EdiUnzulaessig(format!(
            "no marktd is configured, so the EDI agreement of {recipient} (§ 14 Abs. 1 \
             Satz 6 Nr. 2, § 27 Abs. 38 UStG) cannot be read — the INVOIC is not sent \
             without it"
        )));
    };
    let edi = marktd
        .get_partner_edi(recipient)
        .await
        .map_err(|e| DispatchError::MarktdUnavailable(e.to_string()))?
        .unwrap_or_default();
    edi_zulaessig(EdiFall {
        ausgefuehrt,
        ausgestellt,
        uebermittelt,
        zustimmung: edi.edi_zustimmung,
        extrahierbar: edi.edi_extrahierbar,
    })
    .map_err(|u| DispatchError::EdiUnzulaessig(format!("INVOIC an {recipient}: {u}")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use axum::{Router, extract::Path, response::IntoResponse as _, routing::get};
    use secrecy::SecretString;
    use time::macros::date;

    use super::*;

    const LF: &str = "9900000000003";

    /// A stand-in marktd serving `GET /api/v1/partners/{mp_id}` from `partners`;
    /// an MP-ID it does not hold answers 404.
    async fn marktd(partners: HashMap<&'static str, serde_json::Value>) -> MarktdClient {
        let partners = Arc::new(partners);
        let app = Router::new().route(
            "/api/v1/partners/{mp_id}",
            get(move |Path(mp_id): Path<String>| {
                let partners = Arc::clone(&partners);
                async move {
                    match partners.get(mp_id.as_str()) {
                        Some(body) => axum::Json(body.clone()).into_response(),
                        None => axum::http::StatusCode::NOT_FOUND.into_response(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stand-in marktd");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        MarktdClient::new(
            format!("http://{addr}"),
            SecretString::from("test-key"),
            reqwest::Client::new(),
        )
    }

    fn partner(zustimmung: Option<bool>, extrahierbar: Option<bool>) -> serde_json::Value {
        serde_json::json!({
            "mp_id": LF,
            "edi_zustimmung": zustimmung,
            "edi_extrahierbar": extrahierbar,
        })
    }

    /// The payload `netzbilanzd` sends: the BO4E `Rechnung` beside the parties.
    fn payload(start: &str, ende: &str, rechnungsdatum: &str) -> serde_json::Value {
        serde_json::json!({
            "invoice_ref": "NNE-1",
            "recipient_mp_id": LF,
            "rechnung": {
                "_typ": "RECHNUNG",
                "rechnungsnummer": "NNE-1",
                "rechnungsdatum": rechnungsdatum,
                "rechnungsperiode": { "startdatum": start, "enddatum": ende },
            },
        })
    }

    fn dezember_2026() -> serde_json::Value {
        payload("2026-12-01", "2026-12-31", "2027-01-05T00:00:00Z")
    }

    /// A December-2026 supply sent on 05.01.2027 is past Nr. 1 („bis zum
    /// 31. Dezember 2026 übermittelt") and before Nr. 3 (a 2027 supply), so it
    /// stands only as an E-Rechnung — and with no extractability recorded, it
    /// is refused. Zustimmung alone does not carry it.
    #[tokio::test]
    async fn a_december_2026_invoic_in_january_needs_a_recorded_extractability() {
        let client = marktd(HashMap::from([(LF, partner(Some(true), None))])).await;
        let r = edi_pruefen_am(Some(&client), &dezember_2026(), LF, date!(2027 - 01 - 05)).await;
        match r {
            Err(DispatchError::EdiUnzulaessig(msg)) => {
                assert!(msg.contains("extrahierbar"), "{msg}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// The same invoice to a partner whose agreed format is recorded as
    /// extractable is an E-Rechnung (§ 14 Abs. 1 Satz 6 Nr. 2 UStG).
    #[tokio::test]
    async fn the_same_invoic_to_an_extractable_partner_with_zustimmung_passes() {
        let client = marktd(HashMap::from([(LF, partner(Some(true), Some(true)))])).await;
        let r = edi_pruefen_am(Some(&client), &dezember_2026(), LF, date!(2027 - 01 - 05)).await;
        assert!(matches!(r, Ok(Zulaessig::ERechnungVereinbart)), "{r:?}");
    }

    /// A 2027 supply ausgestellt 31.12.2027 is § 27 Abs. 38 Nr. 3 UStG with the
    /// recorded Zustimmung, extractability unrecorded — even transmitted on
    /// 01.01.2028, since Nr. 3 is keyed on ausgestellt.
    #[tokio::test]
    async fn a_2027_supply_ausgestellt_by_the_end_of_2027_passes_under_nr3() {
        let client = marktd(HashMap::from([(LF, partner(Some(true), None))])).await;
        let p = payload("2027-12-01", "2027-12-31", "2027-12-31T00:00:00Z");
        let r = edi_pruefen_am(Some(&client), &p, LF, date!(2028 - 01 - 01)).await;
        assert!(matches!(r, Ok(Zulaessig::Nr3)), "{r:?}");
    }

    /// From 2028 no grace is left: a partner recorded as not extractable, or
    /// with nothing recorded, is refused, and an extractable one is admitted —
    /// which a date cut-off would refuse.
    #[tokio::test]
    async fn a_2028_invoic_turns_on_the_recorded_extractability() {
        let client = marktd(HashMap::from([
            (LF, partner(Some(true), Some(false))),
            ("9900000000010", partner(Some(true), None)),
            ("9900000000027", partner(Some(true), Some(true))),
        ]))
        .await;
        let p = payload("2028-01-01", "2028-01-31", "2028-02-05T00:00:00Z");
        let am = date!(2028 - 02 - 05);
        for recipient in [LF, "9900000000010"] {
            let r = edi_pruefen_am(Some(&client), &p, recipient, am).await;
            assert!(
                matches!(r, Err(DispatchError::EdiUnzulaessig(_))),
                "{recipient}: {r:?}"
            );
        }
        let r = edi_pruefen_am(Some(&client), &p, "9900000000027", am).await;
        assert!(matches!(r, Ok(Zulaessig::ERechnungVereinbart)), "{r:?}");
    }

    /// No Zustimmung recorded, or none given, or no partner record at all:
    /// every route needs it, so each refuses.
    #[tokio::test]
    async fn no_zustimmung_refuses() {
        let client = marktd(HashMap::from([
            (LF, partner(None, Some(true))),
            ("9900000000010", partner(Some(false), Some(true))),
        ]))
        .await;
        let p = payload("2027-06-01", "2027-06-30", "2027-07-05T00:00:00Z");
        for recipient in [LF, "9900000000010", "9900000000027"] {
            let r = edi_pruefen_am(Some(&client), &p, recipient, date!(2027 - 07 - 05)).await;
            match r {
                Err(DispatchError::EdiUnzulaessig(msg)) => {
                    assert!(msg.contains("Zustimmung"), "{recipient}: {msg}");
                }
                other => panic!("{recipient}: expected a refusal, got {other:?}"),
            }
        }
    }

    /// Without marktd the agreement cannot be read, and the INVOIC is not sent
    /// on a default.
    #[tokio::test]
    async fn no_marktd_refuses() {
        let r = edi_pruefen_am(None, &dezember_2026(), LF, date!(2027 - 01 - 05)).await;
        assert!(matches!(r, Err(DispatchError::EdiUnzulaessig(_))), "{r:?}");
    }

    /// marktd failing is an outage, not an unrecorded agreement.
    #[tokio::test]
    async fn an_unreachable_marktd_refuses_as_unavailable() {
        let client = MarktdClient::new(
            "http://127.0.0.1:9",
            SecretString::from("test-key"),
            reqwest::Client::new(),
        );
        let r = edi_pruefen_am(Some(&client), &dezember_2026(), LF, date!(2027 - 01 - 05)).await;
        assert!(
            matches!(r, Err(DispatchError::MarktdUnavailable(_))),
            "{r:?}"
        );
    }

    async fn state(marktd: Option<MarktdClient>) -> super::super::CommandsApiState {
        use super::super::*;
        let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
            .await
            .expect("open in-memory SlateDB");
        CommandsApiState {
            tenant_id: mako_engine::ids::TenantId::from_party_id("9900357000004"),
            sender_party_id: "9900357000004".to_owned(),
            configured_marktrollen: vec!["NB".into(), "LF".into(), "MSB".into()],
            max_body_bytes: 1_048_576,
            snapshot_interval: 100,
            cedar: Arc::new(CedarAuthorizer::unauthenticated().expect("infallible")),
            snapshot_store: store.as_snapshot_store(),
            malo_cache: Arc::new(SlateDbMaloCache::new(
                store.clone(),
                mako_engine::ids::TenantId::from_party_id("9900357000004"),
            )),
            maloid_result_cache: MaloIdentResultCache::new(store.clone()),
            store: Arc::new(store),
            marktd_client: marktd.map(Arc::new),
        }
    }

    /// Every command that issues an INVOIC runs the check before it reaches
    /// its process: with no marktd each is refused `EdiUnzulaessig`, and to an
    /// extractable partner with Zustimmung each passes the check and goes on
    /// to the process lookup. A command that skipped the check would answer
    /// the first half the way it answers the second.
    #[tokio::test]
    async fn every_invoic_issuing_command_runs_the_check() {
        let p = |pid: u32| {
            let mut p = payload("2026-01-01", "2026-01-31", "2026-02-05T00:00:00Z");
            p["pid"] = pid.into();
            p["sender_mp_id"] = "9900357000004".into();
            p["nb_mp_id"] = LF.into();
            p
        };
        let commands = [
            ("invoic.nne-abschlag.stellen", 31001),
            ("invoic.nne.stellen", 31002),
            ("invoic.mmm.stellen", 31005),
            ("wim.msb-rechnung.stellen", 31009),
            ("invoic.sonstige-leistung.stellen", 31011),
        ];
        let ohne = state(None).await;
        for (command, pid) in commands {
            let r = super::super::dispatch_command(&ohne, command, &p(pid)).await;
            assert!(
                matches!(r, Err(DispatchError::EdiUnzulaessig(_))),
                "{command}: {r:?}"
            );
        }
        let client = marktd(HashMap::from([(LF, partner(Some(true), Some(true)))])).await;
        let mit = state(Some(client)).await;
        for (command, pid) in commands {
            let r = super::super::dispatch_command(&mit, command, &p(pid)).await;
            assert!(
                !matches!(
                    r,
                    Err(DispatchError::EdiUnzulaessig(_)
                        | DispatchError::MarktdUnavailable(_)
                        | DispatchError::InvalidPayload(_))
                ),
                "{command}: {r:?}"
            );
        }
    }

    /// A Rechnung without a period end, or without a document date, is
    /// refused before marktd is asked — neither date is assumed.
    #[tokio::test]
    async fn a_missing_date_refuses() {
        let ohne_ende = serde_json::json!({
            "rechnung": {
                "rechnungsdatum": "2027-01-05T00:00:00Z",
                "rechnungsperiode": { "startdatum": "2026-12-01" },
            },
        });
        let ohne_datum = serde_json::json!({
            "rechnung": {
                "rechnungsperiode": { "startdatum": "2026-12-01", "enddatum": "2026-12-31" },
            },
        });
        for p in [ohne_ende, ohne_datum, serde_json::json!({})] {
            let r = edi_pruefen_am(None, &p, LF, date!(2027 - 01 - 05)).await;
            assert!(
                matches!(r, Err(DispatchError::InvalidPayload(_))),
                "{p}: {r:?}"
            );
        }
    }
}
