//! A daemon's readiness report, the route that serves it, and the checklist
//! across a deployment's daemons.
//!
//! [`Einrichtung`] is what one daemon says about its configuration: its name
//! and version, every [`Befund`] its [`Daemon::validate`](crate::Daemon::validate)
//! returned, the [`PolicyReport`] it runs under and the identities it acts
//! as. The runner builds it once from the loaded config — the same value that
//! decides the boot — and a daemon serves it read-only at
//! `GET /admin/einrichtung` through [`router`] (Cedar action `read-config`),
//! beside `GET /api/v1/policy`. `--check-config --json` prints the same JSON.
//!
//! [`checkliste`] folds the reports of every daemon of a deployment into the
//! deployment-wide checklist: one item per [`Bereich`], green, amber or red,
//! with the findings that colour it, plus the findings no single daemon can
//! make — two daemons acting under different MP-IDs for one Rolle, a daemon
//! that cannot be reached. makod serves it at
//! `GET /admin/einrichtung/deployment`, and `just setup` prints it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::befund::{Befund, Bereich, Schwere};
use crate::policy::PolicyReport;

/// The version every daemon of this workspace is built at.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// An identity a daemon acts as: an MP-ID in a Marktrolle, for a Sparte where
/// the daemon knows it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Partei {
    /// The Marktrolle (`LF`, `NB`, `MSB`, …).
    pub rolle: String,
    /// `strom` / `gas`, where the daemon knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparte: Option<String>,
    /// The 13-digit BDEW-/DVGW-Codenummer or GLN, or a 16-character EIC.
    pub mp_id: String,
}

/// What one daemon reports about its configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Einrichtung {
    /// The daemon's name.
    pub dienst: String,
    /// The version it is built at.
    pub version: String,
    /// No finding blocks.
    pub bereit: bool,
    /// Every finding, blocking ones first.
    pub befunde: Vec<Befund>,
    /// The operator policy and posture it runs under.
    pub policy: PolicyReport,
    /// The identities it acts as; empty for a daemon that holds none.
    #[serde(default)]
    pub parteien: Vec<Partei>,
}

impl Einrichtung {
    /// The report of `dienst`, `bereit` derived from `befunde`.
    #[must_use]
    pub fn neu(
        dienst: &str,
        mut befunde: Vec<Befund>,
        policy: PolicyReport,
        parteien: Vec<Partei>,
    ) -> Self {
        befunde.sort_by_key(|b| b.schwere);
        Self {
            dienst: dienst.to_owned(),
            version: VERSION.to_owned(),
            bereit: !befunde.iter().any(Befund::ist_blockiert),
            befunde,
            policy,
            parteien,
        }
    }

    /// The terminal rendering `--check-config` prints.
    #[must_use]
    pub fn text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} {}: {}",
            self.dienst,
            self.version,
            if self.bereit { "bereit" } else { "blockiert" }
        );
        for b in &self.befunde {
            let _ = writeln!(out, "  {:<9} {b}", b.schwere.as_str());
        }
        if self.befunde.is_empty() {
            out.push_str("  keine Befunde\n");
        }
        out
    }
}

// ── The route ────────────────────────────────────────────────────────────────

/// `GET /admin/einrichtung` (Cedar `read-config`) and `GET /api/v1/policy`
/// (Cedar `read-policy`), both read-only, both from the report the runner
/// built at boot.
///
/// Merge it into the daemon's router **before** the layers that supply
/// `CedarEnforcer`, `OidcVerifier` and `ExpectedTenant`, as
/// [`crate::policy::router`], which it includes.
#[cfg(all(feature = "cedar", feature = "oidc"))]
pub fn router(einrichtung: std::sync::Arc<Einrichtung>) -> axum::Router {
    use std::sync::Arc;

    use axum::Extension;
    use axum::routing::get;

    async fn get_einrichtung(
        claims: crate::oidc::Claims,
        Extension(enforcer): Extension<Arc<crate::cedar::CedarEnforcer>>,
        tenant: Option<Extension<crate::oidc::ExpectedTenant>>,
        Extension(e): Extension<Arc<Einrichtung>>,
    ) -> crate::ApiResult<axum::Json<Einrichtung>> {
        let principal = claims.principal();
        let resource = tenant.map_or_else(|| principal.tenant.clone(), |Extension(t)| t.0);
        enforcer
            .check(&principal, "read-config", &resource)
            .map_err(|e| crate::ApiError::Denied(e.to_string()))?;
        Ok(axum::Json((*e).clone()))
    }

    axum::Router::new()
        .route("/admin/einrichtung", get(get_einrichtung))
        .layer(Extension(Arc::clone(&einrichtung)))
        .merge(crate::policy::router(einrichtung.policy.clone()))
}

// ── The checklist ────────────────────────────────────────────────────────────

/// What came back from asking one daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "zustand", rename_all = "snake_case")]
pub enum Antwort {
    /// The daemon answered with its report.
    Erreicht {
        /// The report.
        einrichtung: Einrichtung,
    },
    /// The daemon could not be reached or refused the request.
    Unerreichbar {
        /// Why.
        grund: String,
    },
    /// The daemon answered with something that is not a report — shown
    /// verbatim, never dropped.
    Unlesbar {
        /// Why it did not parse.
        grund: String,
        /// The body as received.
        roh: String,
    },
}

/// One daemon of the deployment and what it answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DienstBericht {
    /// The daemon's name as configured.
    pub dienst: String,
    /// Where it was asked; empty for a local `--check-config`.
    #[serde(default)]
    pub url: String,
    /// The answer.
    #[serde(flatten)]
    pub antwort: Antwort,
}

/// The colour of a checklist item.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Ampel {
    /// Nothing found.
    Gruen,
    /// Warnings only.
    Gelb,
    /// At least one blocking finding.
    Rot,
    /// A daemon that would answer it could not be asked.
    Unbekannt,
}

impl Ampel {
    /// `grün` / `gelb` / `rot` / `unbekannt`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gruen => "grün",
            Self::Gelb => "gelb",
            Self::Rot => "rot",
            Self::Unbekannt => "unbekannt",
        }
    }
}

/// A finding with the daemon that reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DienstBefund {
    /// The reporting daemon; `deployment` for a finding across daemons.
    pub dienst: String,
    /// The finding.
    #[serde(flatten)]
    pub befund: Befund,
}

/// One readiness question of the deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Punkt {
    /// The area.
    pub bereich: Bereich,
    /// The question.
    pub frage: String,
    /// Its colour.
    pub ampel: Ampel,
    /// The findings that colour it.
    pub befunde: Vec<DienstBefund>,
}

/// The state of one daemon in the checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DienstStatus {
    /// Answered, nothing blocks.
    Bereit,
    /// Answered with a blocking finding.
    Blockiert,
    /// Could not be asked — neither ready nor failing.
    Unerreichbar,
    /// Answered with something that is not a report.
    Unlesbar,
}

/// One daemon's line in the checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DienstZeile {
    /// The daemon.
    pub dienst: String,
    /// Where it was asked.
    pub url: String,
    /// Its state.
    pub status: DienstStatus,
    /// Its version, when it answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// The deployment-wide checklist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Checkliste {
    /// Every item green or amber and every daemon answered.
    pub bereit: bool,
    /// One line per daemon.
    pub dienste: Vec<DienstZeile>,
    /// One item per [`Bereich`], in [`Bereich::ALLE`] order.
    pub punkte: Vec<Punkt>,
    /// Every answer as received, so nothing a daemon said is lost.
    pub berichte: Vec<DienstBericht>,
}

/// The name cross-daemon findings are reported under.
pub const DEPLOYMENT: &str = "deployment";

/// Fold the answers of a deployment's daemons into its checklist.
#[must_use]
pub fn checkliste(berichte: Vec<DienstBericht>) -> Checkliste {
    let mut befunde: Vec<DienstBefund> = Vec::new();
    let mut dienste = Vec::with_capacity(berichte.len());
    let mut unerreichbar = false;
    let mut identitaeten = Identitaeten::new();
    for b in &berichte {
        let (status, version) = match &b.antwort {
            Antwort::Erreicht { einrichtung } => {
                for f in &einrichtung.befunde {
                    befunde.push(DienstBefund {
                        dienst: b.dienst.clone(),
                        befund: f.clone(),
                    });
                }
                for p in &einrichtung.parteien {
                    identitaeten
                        .entry((p.rolle.to_uppercase(), p.sparte.clone()))
                        .or_default()
                        .entry(p.mp_id.clone())
                        .or_default()
                        .push(b.dienst.clone());
                }
                let status = if einrichtung.bereit {
                    DienstStatus::Bereit
                } else {
                    DienstStatus::Blockiert
                };
                (status, Some(einrichtung.version.clone()))
            }
            Antwort::Unerreichbar { grund } => {
                unerreichbar = true;
                befunde.push(DienstBefund {
                    dienst: b.dienst.clone(),
                    befund: Befund::warnung("", format!("nicht erreichbar: {grund}"))
                        .bereich(Bereich::Erreichbarkeit),
                });
                (DienstStatus::Unerreichbar, None)
            }
            Antwort::Unlesbar { grund, roh } => {
                befunde.push(DienstBefund {
                    dienst: b.dienst.clone(),
                    befund: Befund::blockiert(
                        "",
                        format!("Antwort ist kein Einrichtungsbericht ({grund}): {roh}"),
                    )
                    .bereich(Bereich::Erreichbarkeit),
                });
                (DienstStatus::Unlesbar, None)
            }
        };
        dienste.push(DienstZeile {
            dienst: b.dienst.clone(),
            url: b.url.clone(),
            status,
            version,
        });
    }
    befunde.extend(widersprueche(&identitaeten));

    let punkte: Vec<Punkt> = Bereich::ALLE
        .iter()
        .map(|&bereich| {
            let eigene: Vec<DienstBefund> = befunde
                .iter()
                .filter(|f| f.befund.bereich == bereich)
                .cloned()
                .collect();
            let ampel = if bereich == Bereich::Erreichbarkeit && unerreichbar {
                Ampel::Unbekannt
            } else if eigene
                .iter()
                .any(|f| f.befund.schwere == Schwere::Blockiert)
            {
                Ampel::Rot
            } else if eigene.is_empty() {
                Ampel::Gruen
            } else {
                Ampel::Gelb
            };
            Punkt {
                bereich,
                frage: bereich.frage().to_owned(),
                ampel,
                befunde: eigene,
            }
        })
        .collect();
    Checkliste {
        bereit: punkte
            .iter()
            .all(|p| matches!(p.ampel, Ampel::Gruen | Ampel::Gelb)),
        dienste,
        punkte,
        berichte,
    }
}

/// (Rolle, Sparte) → MP-ID → the daemons acting under it.
type Identitaeten = BTreeMap<(String, Option<String>), BTreeMap<String, Vec<String>>>;

/// A finding per (Rolle, Sparte) the daemons name different MP-IDs for.
fn widersprueche(identitaeten: &Identitaeten) -> Vec<DienstBefund> {
    let mut befunde = Vec::new();
    for ((rolle, sparte), mp_ids) in identitaeten {
        if mp_ids.len() < 2 {
            continue;
        }
        let liste = mp_ids
            .iter()
            .map(|(id, d)| format!("{id} ({})", d.join(", ")))
            .collect::<Vec<_>>()
            .join(" / ");
        let rolle_sparte = sparte
            .as_ref()
            .map_or_else(|| rolle.clone(), |s| format!("{rolle} {s}"));
        befunde.push(DienstBefund {
            dienst: DEPLOYMENT.to_owned(),
            befund: Befund::blockiert(
                "",
                format!(
                    "die Dienste nennen für {rolle_sparte} verschiedene MP-IDs: {liste} — \
                     je Sparte und Rolle hat ein Marktteilnehmer genau eine MP-ID"
                ),
            )
            .bereich(Bereich::Identitaeten)
            .quelle("BDEW-AWH Identifikatoren V1.2, Kap. 2.1"),
        });
    }

    befunde
}

impl Checkliste {
    /// The terminal rendering `just setup` prints.
    #[must_use]
    pub fn text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        out.push_str("Dienste\n");
        for d in &self.dienste {
            let status = match d.status {
                DienstStatus::Bereit => "bereit",
                DienstStatus::Blockiert => "blockiert",
                DienstStatus::Unerreichbar => "unerreichbar",
                DienstStatus::Unlesbar => "unlesbar",
            };
            let _ = writeln!(
                out,
                "  {:<14} {:<12} {}",
                d.dienst,
                status,
                d.version.as_deref().unwrap_or("")
            );
        }
        out.push_str("\nCheckliste\n");
        for p in &self.punkte {
            let _ = writeln!(out, "  [{:<9}] {}", p.ampel.as_str(), p.frage);
            for f in &p.befunde {
                let _ = writeln!(
                    out,
                    "      {} {:<9} {}",
                    f.dienst,
                    f.befund.schwere.as_str(),
                    f.befund
                );
            }
        }
        let _ = writeln!(
            out,
            "\n{}",
            if self.bereit {
                "Einrichtung bereit."
            } else {
                "Einrichtung nicht bereit."
            }
        );
        out
    }
}

/// A daemon to ask for its report.
#[derive(Debug, Clone)]
pub struct Dienst {
    /// Its name, as the checklist shows it.
    pub name: String,
    /// Its base URL; `/admin/einrichtung` is appended.
    pub url: String,
    /// The bearer credential its `read-config` admits.
    pub token: Option<secrecy::SecretString>,
}

/// Ask every daemon for `GET /admin/einrichtung`, one after another.
pub async fn abfragen(http: &reqwest::Client, dienste: &[Dienst]) -> Vec<DienstBericht> {
    let fragen = dienste.iter().map(|d| async move {
        let url = format!("{}/admin/einrichtung", d.url.trim_end_matches('/'));
        let mut req = http.get(&url);
        if let Some(t) = &d.token {
            req = req.bearer_auth(secrecy::ExposeSecret::expose_secret(t));
        }
        let antwort = match req.send().await {
            Err(e) => Antwort::Unerreichbar {
                grund: e.to_string(),
            },
            Ok(r) if !r.status().is_success() => Antwort::Unerreichbar {
                grund: format!("HTTP {}", r.status()),
            },
            Ok(r) => match r.text().await {
                Err(e) => Antwort::Unerreichbar {
                    grund: e.to_string(),
                },
                Ok(roh) => lies(&roh),
            },
        };
        DienstBericht {
            dienst: d.name.clone(),
            url: d.url.clone(),
            antwort,
        }
    });
    let mut out = Vec::with_capacity(dienste.len());
    for f in fragen {
        out.push(f.await);
    }
    out
}

/// A report body as received: parsed, or kept verbatim.
#[must_use]
pub fn lies(roh: &str) -> Antwort {
    match serde_json::from_str::<Einrichtung>(roh) {
        Ok(einrichtung) => Antwort::Erreicht { einrichtung },
        Err(e) => Antwort::Unlesbar {
            grund: e.to_string(),
            roh: roh.to_owned(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bericht(dienst: &str, befunde: Vec<Befund>, parteien: Vec<Partei>) -> DienstBericht {
        DienstBericht {
            dienst: dienst.to_owned(),
            url: format!("http://{dienst}"),
            antwort: Antwort::Erreicht {
                einrichtung: Einrichtung::neu(dienst, befunde, PolicyReport::new(), parteien),
            },
        }
    }

    fn partei(rolle: &str, sparte: &str, mp_id: &str) -> Partei {
        Partei {
            rolle: rolle.to_owned(),
            sparte: Some(sparte.to_owned()),
            mp_id: mp_id.to_owned(),
        }
    }

    /// The planted fixture: a Gas Rolle without a DVGW-Codenummer, a
    /// Bilanzkreis row without its Sparte and an expired AS4 certificate,
    /// each reported by the daemon that owns it, give exactly three red items;
    /// a certificate inside its warning window is amber, not red.
    #[test]
    fn three_planted_defects_give_exactly_three_red_items() {
        let berichte = vec![
            bericht(
                "makod",
                vec![
                    Befund::blockiert("party[1].mp_id", "Rolle LF Gas mit BDEW-Codenummer")
                        .bereich(Bereich::Identitaeten),
                    Befund::blockiert("party[0].as4.signing_cert_pem_file", "abgelaufen")
                        .bereich(Bereich::Zertifikate),
                    Befund::warnung("party[0].as4.tls_cert_pem_file", "läuft in 10 Tagen ab")
                        .bereich(Bereich::Zertifikate),
                ],
                vec![partei("LF", "strom", "9900000000003")],
            ),
            bericht(
                "processd",
                vec![
                    Befund::blockiert("lf.bilanzkreise[0]", "missing field `sparte`")
                        .bereich(Bereich::Bilanzierung),
                ],
                vec![],
            ),
            bericht("billingd", vec![], vec![]),
        ];
        let c = checkliste(berichte);
        let rot: Vec<Bereich> = c
            .punkte
            .iter()
            .filter(|p| p.ampel == Ampel::Rot)
            .map(|p| p.bereich)
            .collect();
        assert_eq!(
            rot,
            vec![
                Bereich::Identitaeten,
                Bereich::Zertifikate,
                Bereich::Bilanzierung
            ]
        );
        assert!(!c.bereit);
        assert_eq!(c.dienste[2].status, DienstStatus::Bereit);
        assert_eq!(c.dienste[0].status, DienstStatus::Blockiert);
        let text = c.text();
        assert!(text.contains("lf.bilanzkreise[0]"), "{text}");

        // Fixed: only the amber certificate is left.
        let c = checkliste(vec![bericht(
            "makod",
            vec![
                Befund::warnung("party[0].as4.tls_cert_pem_file", "läuft in 10 Tagen ab")
                    .bereich(Bereich::Zertifikate),
            ],
            vec![],
        )]);
        assert!(c.bereit);
        assert_eq!(c.punkte[1].ampel, Ampel::Gelb);
    }

    /// An unreachable daemon is neither ready nor failing: its line says
    /// unreachable, the reachability item is unknown, and the deployment is
    /// not ready.
    #[test]
    fn an_unreachable_daemon_is_unknown_not_failing() {
        let c = checkliste(vec![DienstBericht {
            dienst: "edmd".into(),
            url: "http://edmd".into(),
            antwort: Antwort::Unerreichbar {
                grund: "connection refused".into(),
            },
        }]);
        assert_eq!(c.dienste[0].status, DienstStatus::Unerreichbar);
        let erreichbarkeit = c
            .punkte
            .iter()
            .find(|p| p.bereich == Bereich::Erreichbarkeit)
            .unwrap();
        assert_eq!(erreichbarkeit.ampel, Ampel::Unbekannt);
        assert!(c.punkte.iter().all(|p| p.ampel != Ampel::Rot));
        assert!(!c.bereit);
    }

    /// Two daemons acting under different MP-IDs for one Rolle and Sparte is
    /// its own finding, which no single daemon could make.
    #[test]
    fn two_daemons_disagreeing_about_an_identity_is_a_finding() {
        let c = checkliste(vec![
            bericht(
                "makod",
                vec![],
                vec![partei("LF", "strom", "9900000000003")],
            ),
            bericht(
                "processd",
                vec![],
                vec![partei("lf", "strom", "9900000000010")],
            ),
        ]);
        let ident = &c.punkte[0];
        assert_eq!(ident.ampel, Ampel::Rot);
        assert_eq!(ident.befunde.len(), 1);
        assert_eq!(ident.befunde[0].dienst, DEPLOYMENT);
        assert!(
            ident.befunde[0]
                .befund
                .text
                .contains("9900000000010 (processd)"),
            "{}",
            ident.befunde[0].befund.text
        );
    }

    /// A body that is not a report is kept verbatim.
    #[test]
    fn an_unreadable_answer_is_kept_verbatim() {
        let Antwort::Unlesbar { roh, .. } = lies("{\"neu\": 1}") else {
            panic!("not a report")
        };
        assert_eq!(roh, "{\"neu\": 1}");
    }
}
