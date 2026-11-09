//! `GET /admin/einrichtung`, `GET /admin/einrichtung/deployment` and
//! `GET /api/v1/policy` — makod's configuration findings, the checklist across
//! the deployment's daemons, and the policy report. All read-only, all behind
//! the Cedar action `AdminEinrichtung`.
//!
//! The report is the one [`crate::einrichtung::pruefe`] built at boot from the
//! loaded configuration — the same value `--check-config --json` prints — so
//! what the operator is shown is what decided the boot.

use std::sync::Arc;

use axum::{Json, Router, extract::State, http::HeaderMap, routing::get};
use mako_service::einrichtung::{
    Antwort, Checkliste, Dienst, DienstBericht, Einrichtung, abfragen, checkliste,
};
use mako_service::policy::PolicyReport;
use mako_service::{ApiError, ApiResult};

use crate::cedar_authz::CedarAuthorizer;

/// Shared state for the Einrichtung routes.
pub struct EinrichtungState {
    /// makod's own report, built at boot.
    pub einrichtung: Arc<Einrichtung>,
    /// The deployment's other daemons, from `[[einrichtung.dienst]]`.
    pub dienste: Vec<Dienst>,
    /// The client the checklist asks them with.
    pub http: reqwest::Client,
    /// Cedar authorizer.
    pub cedar: Arc<CedarAuthorizer>,
    /// Operator tenant (MP-ID).
    pub tenant: String,
}

/// The routes.
pub fn router(state: Arc<EinrichtungState>) -> Router {
    Router::new()
        .route("/admin/einrichtung", get(get_einrichtung))
        .route("/admin/einrichtung/deployment", get(get_deployment))
        .route("/api/v1/policy", get(get_policy))
        .with_state(state)
}

fn zulassen(state: &EinrichtungState, headers: &HeaderMap) -> ApiResult<()> {
    let identity = state
        .cedar
        .authenticate(headers)
        .ok_or(ApiError::Unauthorized)?;
    if state.cedar.authorize_einrichtung(&identity, &state.tenant) {
        Ok(())
    } else {
        Err(ApiError::forbidden("AdminEinrichtung permission denied"))
    }
}

async fn get_einrichtung(
    State(state): State<Arc<EinrichtungState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Einrichtung>> {
    zulassen(&state, &headers)?;
    Ok(Json((*state.einrichtung).clone()))
}

async fn get_policy(
    State(state): State<Arc<EinrichtungState>>,
    headers: HeaderMap,
) -> ApiResult<Json<PolicyReport>> {
    zulassen(&state, &headers)?;
    Ok(Json(state.einrichtung.policy.clone()))
}

async fn get_deployment(
    State(state): State<Arc<EinrichtungState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Checkliste>> {
    zulassen(&state, &headers)?;
    let andere = abfragen(&state.http, &state.dienste).await;
    Ok(Json(deployment(&state.einrichtung, andere)))
}

/// makod's own report first, then every other daemon's answer, folded into
/// the checklist.
#[must_use]
pub fn deployment(eigene: &Einrichtung, andere: Vec<DienstBericht>) -> Checkliste {
    let mut berichte = Vec::with_capacity(andere.len() + 1);
    berichte.push(DienstBericht {
        dienst: eigene.dienst.clone(),
        url: String::new(),
        antwort: Antwort::Erreicht {
            einrichtung: eigene.clone(),
        },
    });
    berichte.extend(andere);
    checkliste(berichte)
}

#[cfg(test)]
mod tests {
    use mako_service::befund::{Befund, Bereich};
    use mako_service::einrichtung::{Ampel, DienstStatus, Partei};

    use super::*;

    /// `GET /admin/einrichtung` serves exactly the report `--check-config
    /// --json` prints for the same config — the value `pruefe` built — and
    /// refuses an unauthenticated caller.
    #[tokio::test]
    async fn the_route_and_check_config_agree() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt as _;

        use crate::cedar_authz::{DefaultPolicy, NamedKey};
        use crate::config::{As4Config, EinrichtungConfig, MakodPolicy, PartyConfig};
        use crate::deployment::Rolle;

        let parties = [PartyConfig {
            mp_id: "9900001000001".to_owned(),
            roles: vec!["GNB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }];
        let as4 = As4Config::default();
        let policy = MakodPolicy::default();
        let einrichtung = EinrichtungConfig::default();
        let pruefung = crate::einrichtung::pruefe(&crate::einrichtung::Eingabe {
            parties: &parties,
            rollen: &[Rolle::NbGas],
            preflight: None,
            cedar_policy_dir_fehler: None,
            as4: &as4,
            policy: &policy,
            marktd_url: Some("http://marktd:8180"),
            marktd_webhook_url: None,
            http_enabled: true,
            webdienste_signing_key: false,
            webdienste_signing_cert: false,
            posture: &[],
            jetzt: time::OffsetDateTime::now_utc(),
            einrichtung: &einrichtung,
        });
        let printed = serde_json::to_value(&pruefung.einrichtung).unwrap();
        let cedar = CedarAuthorizer::new(
            vec![NamedKey::from_arg("admin-readonly=0123456789abcdef").unwrap()],
            None,
            None,
            Some("9900001000001".to_owned()),
            DefaultPolicy::PermitAll,
        )
        .unwrap();
        let app = router(Arc::new(EinrichtungState {
            einrichtung: Arc::new(pruefung.einrichtung),
            dienste: Vec::new(),
            http: reqwest::Client::new(),
            cedar: Arc::new(cedar),
            tenant: "9900001000001".to_owned(),
        }));
        let call = |auth: Option<&'static str>| {
            let mut req = Request::get("/admin/einrichtung");
            if let Some(a) = auth {
                req = req.header("authorization", format!("Bearer {a}"));
            }
            app.clone().oneshot(req.body(Body::empty()).unwrap())
        };
        assert_eq!(call(None).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        let ok = call(Some("0123456789abcdef")).await.unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body = axum::body::to_bytes(ok.into_body(), 1 << 20).await.unwrap();
        let served: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(served, printed);
        assert_eq!(served["bereit"], false);
        assert!(served["befunde"].as_array().is_some_and(|b| b.len() >= 2));
    }

    /// The Desk's checklist fixture is makod's answer for a planted
    /// deployment: a Gas Rolle under a BDEW-Codenummer, a daemon that cannot
    /// be reached and one that answers with a gateway page. A change to the
    /// report's shape or wording fails here until the fixture is blessed
    /// (`BLESS_DESK_FIXTURE=1`), so the Desk's tests never run against an
    /// answer makod would not give.
    #[test]
    fn the_desk_fixture_is_makods_answer() {
        use crate::config::{As4Config, EinrichtungConfig, MakodPolicy, PartyConfig};
        use crate::deployment::Rolle;

        let parties = [PartyConfig {
            mp_id: "9900001000001".to_owned(),
            roles: vec!["GNB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }];
        let as4 = As4Config::default();
        let policy = MakodPolicy::default();
        let einrichtung = EinrichtungConfig::default();
        let pruefung = crate::einrichtung::pruefe(&crate::einrichtung::Eingabe {
            parties: &parties,
            rollen: &[Rolle::NbGas],
            preflight: None,
            cedar_policy_dir_fehler: None,
            as4: &as4,
            policy: &policy,
            marktd_url: Some("http://127.0.0.1:8180"),
            marktd_webhook_url: None,
            http_enabled: true,
            webdienste_signing_key: false,
            webdienste_signing_cert: false,
            posture: &[],
            jetzt: time::macros::datetime!(2026-11-09 09:00 UTC),
            einrichtung: &einrichtung,
        });
        let andere = vec![
            DienstBericht {
                dienst: "processd".into(),
                url: "http://127.0.0.1:8580".into(),
                antwort: Antwort::Erreicht {
                    einrichtung: Einrichtung::neu("processd", vec![], PolicyReport::new(), vec![]),
                },
            },
            DienstBericht {
                dienst: "edmd".into(),
                url: "http://127.0.0.1:8380".into(),
                antwort: Antwort::Unerreichbar {
                    grund:
                        "error sending request for url (http://127.0.0.1:8380/admin/einrichtung)"
                            .into(),
                },
            },
            DienstBericht {
                dienst: "billingd".into(),
                url: "http://127.0.0.1:9280".into(),
                antwort: Antwort::Unlesbar {
                    grund: "expected value at line 1 column 1".into(),
                    roh: "<html><body>502 Bad Gateway</body></html>".into(),
                },
            },
        ];
        let checkliste = deployment(&pruefung.einrichtung, andere);
        // The version moves with every release; the fixture names a fixed one.
        let mut json = serde_json::to_string_pretty(&checkliste).unwrap().replace(
            &format!("\"{}\"", mako_service::einrichtung::VERSION),
            "\"1.0.0\"",
        );
        json.push('\n');
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../desk/fixtures/einrichtung-checkliste.json"
        );
        if std::env::var_os("BLESS_DESK_FIXTURE").is_some() {
            std::fs::write(path, &json).unwrap();
        }
        let fixture = std::fs::read_to_string(path).unwrap_or_default();
        assert_eq!(
            fixture, json,
            "desk/fixtures/einrichtung-checkliste.json is not makod's answer — \
             rerun with BLESS_DESK_FIXTURE=1"
        );
    }

    /// makod's own report is a line of the checklist, beside the daemons it
    /// asked; a daemon that names another MP-ID for makod's role is a finding
    /// across both, and an unreachable one is unknown.
    #[test]
    fn makod_folds_its_own_report_with_the_others() {
        let eigene = Einrichtung::neu(
            "makod",
            vec![
                Befund::warnung("as4.partner[0].signing_cert_pem_files[0]", "bald")
                    .bereich(Bereich::Zertifikate),
            ],
            PolicyReport::new(),
            vec![Partei {
                rolle: "LF".into(),
                sparte: Some("strom".into()),
                mp_id: "9900001000001".into(),
            }],
        );
        let processd = DienstBericht {
            dienst: "processd".into(),
            url: "http://processd".into(),
            antwort: Antwort::Erreicht {
                einrichtung: Einrichtung::neu(
                    "processd",
                    vec![],
                    PolicyReport::new(),
                    vec![Partei {
                        rolle: "LF".into(),
                        sparte: Some("strom".into()),
                        mp_id: "9900001000002".into(),
                    }],
                ),
            },
        };
        let edmd = DienstBericht {
            dienst: "edmd".into(),
            url: "http://edmd".into(),
            antwort: Antwort::Unerreichbar {
                grund: "connection refused".into(),
            },
        };
        let c = deployment(&eigene, vec![processd, edmd]);
        assert_eq!(c.dienste[0].dienst, "makod");
        assert_eq!(c.dienste[0].status, DienstStatus::Bereit);
        assert_eq!(c.dienste[2].status, DienstStatus::Unerreichbar);
        let ampel = |b: Bereich| c.punkte.iter().find(|p| p.bereich == b).unwrap().ampel;
        assert_eq!(ampel(Bereich::Identitaeten), Ampel::Rot);
        assert_eq!(ampel(Bereich::Zertifikate), Ampel::Gelb);
        assert_eq!(ampel(Bereich::Erreichbarkeit), Ampel::Unbekannt);
        assert!(!c.bereit);
    }
}
