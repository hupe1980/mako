//! The Prüfvermerk: a proposal held against the gate before a person reads it.
//!
//! A proposed decision on a Freigabe is rehearsed in two stages, each by the
//! service that owns the rule, never by agentd's own reading of it:
//!
//! 1. **`entscheidung`** — processd `rehearse_decision` runs the checks its
//!    approve / reject route runs before it dispatches (the frozen answer, the
//!    facts the command needs, the Antwortcode against the entry's
//!    Entscheidungsbaum) and answers with the makod command and body the
//!    decision would send.
//! 2. **`probe`** — makod `rehearse_command` runs that command's door up to
//!    its pre-send AHB check. makod rehearses only the doors whose check runs
//!    before they write; a door it does not rehearse leaves this stage
//!    `nicht_geprueft` with makod's own code, `not_rehearsable`.
//!
//! The verdict is `bestanden` only when every stage ran and passed,
//! `abgelehnt` when one refused, and otherwise `nicht_geprueft` with the
//! reason — a service that did not answer never reads as a pass. A Disput or a
//! Buchung proposal has no rehearsal to run and says so.
//!
//! The Prüfvermerk is stored as an assessment on the proposal run (metric
//! [`METRIK`], the verdict as its label, the stages as its rationale), so it
//! is sealed and erased with the run's data and agentplane's track record
//! counts the verdicts per skill.

use std::sync::Arc;

use agentplane::tools::{ToolClient, ToolId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::vorschlag::Art;

/// The assessment metric a Prüfvermerk is stored under.
pub const METRIK: &str = "pruefvermerk";

/// The actor a Prüfvermerk is recorded by: agentd's own check, a channel and
/// not a person.
pub const PRUEFER: &str = "agentd://pruefung";

/// The verdict on a proposal, or on one stage of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PruefUrteil {
    Bestanden,
    Abgelehnt,
    NichtGeprueft,
}

impl PruefUrteil {
    /// The assessment label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bestanden => "bestanden",
            Self::Abgelehnt => "abgelehnt",
            Self::NichtGeprueft => "nicht_geprueft",
        }
    }

    /// The verdict a label names.
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        [Self::Bestanden, Self::Abgelehnt, Self::NichtGeprueft]
            .into_iter()
            .find(|u| u.label() == label)
    }
}

/// Which service's rule a stage held the proposal against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stufe {
    /// processd's decision route: frozen answer, facts, Entscheidungsbaum.
    Entscheidung,
    /// makod's door up to the pre-send AHB check.
    Probe,
}

/// One stage's verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pruefstufe {
    pub stufe: Stufe,
    pub urteil: PruefUrteil,
    /// The stable code of a refusal or of why the stage did not run
    /// (`code_outside_tree`, `not_rehearsable`, `unreachable`, …).
    pub code: Option<String>,
    /// The rule a refusal rests on: the Entscheidungsbaum (`E_0622`) or the
    /// profile rule (`AHB-…`).
    pub regel: Option<String>,
    /// The segment a refusal sits on (`SG4/STS`).
    pub segment: Option<String>,
    /// The service's own sentence.
    pub text: Option<String>,
}

/// The gate's verdict on one proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pruefvermerk {
    pub urteil: PruefUrteil,
    /// Every stage that ran or was meant to, in order.
    pub stufen: Vec<Pruefstufe>,
    /// Why nothing was checked, where no stage applies (`keine_probe`).
    pub grund: Option<String>,
    /// The body the verdict was taken for: processd's `body_sha256` of the
    /// command and body the decision would send (RFC 8785, the hash M81's
    /// „unverändert" compares). A verdict is current only for this body; an
    /// edited one is not checked.
    pub body_sha256: Option<String>,
    /// When the gate was asked, RFC 3339.
    pub geprueft_um: String,
}

impl Pruefvermerk {
    fn aus(stufen: Vec<Pruefstufe>) -> Self {
        let urteil = if stufen.iter().any(|s| s.urteil == PruefUrteil::Abgelehnt) {
            PruefUrteil::Abgelehnt
        } else if !stufen.is_empty() && stufen.iter().all(|s| s.urteil == PruefUrteil::Bestanden) {
            PruefUrteil::Bestanden
        } else {
            PruefUrteil::NichtGeprueft
        };
        Self {
            urteil,
            stufen,
            grund: None,
            body_sha256: None,
            geprueft_um: jetzt(),
        }
    }

    fn ohne_probe(grund: &str) -> Self {
        Self {
            urteil: PruefUrteil::NichtGeprueft,
            stufen: Vec::new(),
            grund: Some(grund.to_owned()),
            body_sha256: None,
            geprueft_um: jetzt(),
        }
    }

    /// The refusal of the stage that refused, for the repair turn and the
    /// Desk.
    #[must_use]
    pub fn ablehnung(&self) -> Option<&Pruefstufe> {
        self.stufen
            .iter()
            .find(|s| s.urteil == PruefUrteil::Abgelehnt)
    }
}

fn jetzt() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The two services a proposal is rehearsed against.
#[derive(Clone, Default)]
pub struct Pruefer {
    pub processd: Option<Arc<dyn ToolClient>>,
    pub makod: Option<Arc<dyn ToolClient>>,
}

impl std::fmt::Debug for Pruefer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pruefer")
            .field("processd", &self.processd.is_some())
            .field("makod", &self.makod.is_some())
            .finish()
    }
}

impl Pruefer {
    /// The clients among the plane's tool servers.
    #[must_use]
    pub fn aus(servers: &[(String, Arc<dyn ToolClient>)]) -> Self {
        let find = |name: &str| {
            servers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| Arc::clone(c))
        };
        Self {
            processd: find("processd"),
            makod: find("makod"),
        }
    }
}

fn nicht_geprueft(stufe: Stufe, code: &str, text: Option<String>) -> Pruefstufe {
    Pruefstufe {
        stufe,
        urteil: PruefUrteil::NichtGeprueft,
        code: Some(code.to_owned()),
        regel: None,
        segment: None,
        text,
    }
}

/// The stable code a service's refusal opens with (`not_rehearsable: …`), or
/// `unreachable` for a call that did not reach it.
fn fehler(e: &agentplane::tools::ToolError) -> (String, String) {
    use agentplane::tools::ToolError;
    match e {
        ToolError::Refused { detail, .. } | ToolError::ToolFailed { detail, .. } => {
            (fehlercode(detail), detail.clone())
        }
        ToolError::Malformed { detail, .. } => ("unreadable_answer".to_owned(), detail.clone()),
        ToolError::Unreachable { detail, .. } | ToolError::TimedOut { detail, .. } => {
            ("unreachable".to_owned(), detail.clone())
        }
    }
}

fn fehlercode(message: &str) -> String {
    let head = message.split(':').next().unwrap_or_default().trim();
    let token = !head.is_empty()
        && head
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if token {
        head.to_owned()
    } else {
        "unreachable".to_owned()
    }
}

/// processd's verdict on the decision, and the command it would send.
fn entscheidung(output: &Value) -> (Pruefstufe, Option<(String, Option<String>, Value)>) {
    let stufe = |urteil, code: Option<&str>, regel: Option<&str>, text: Option<&str>| Pruefstufe {
        stufe: Stufe::Entscheidung,
        urteil,
        code: code.map(str::to_owned),
        regel: regel.map(str::to_owned),
        segment: None,
        text: text.map(str::to_owned),
    };
    match output["urteil"].as_str() {
        Some("besteht") => {
            let command = output["command"].as_str().map(str::to_owned);
            let befehl = command.map(|c| {
                (
                    c,
                    output["marktrolle"].as_str().map(str::to_owned),
                    output["payload"].clone(),
                )
            });
            (stufe(PruefUrteil::Bestanden, None, None, None), befehl)
        }
        Some("ohne_nachricht") => (stufe(PruefUrteil::Bestanden, None, None, None), None),
        Some("abgelehnt") => {
            let g = &output["grund"];
            (
                stufe(
                    PruefUrteil::Abgelehnt,
                    g["code"].as_str(),
                    g["ebd"].as_str(),
                    g["text"].as_str(),
                ),
                None,
            )
        }
        _ => (
            nicht_geprueft(Stufe::Entscheidung, "unreadable_answer", None),
            None,
        ),
    }
}

/// makod's verdict on the message the command builds.
fn probe(output: &Value) -> Pruefstufe {
    match output["urteil"].as_str() {
        Some("besteht") => Pruefstufe {
            stufe: Stufe::Probe,
            urteil: PruefUrteil::Bestanden,
            code: None,
            regel: None,
            segment: None,
            text: None,
        },
        Some("abgelehnt") => {
            // The first finding that sits on a segment names the place; a
            // door refusal has no message and so no segment.
            let befund = output["nachrichten"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|n| n["segmente"].as_array().into_iter().flatten())
                .find_map(|s| {
                    let b = s["befunde"].as_array()?.first()?;
                    let tag = s["tag"].as_str()?;
                    let segment = match s["gruppe"].as_str() {
                        Some(g) => format!("{g}/{tag}"),
                        None => tag.to_owned(),
                    };
                    Some((segment, b["regel"].as_str().map(str::to_owned)))
                });
            Pruefstufe {
                stufe: Stufe::Probe,
                urteil: PruefUrteil::Abgelehnt,
                code: output["stufe"].as_str().map(str::to_owned),
                regel: befund.as_ref().and_then(|(_, r)| r.clone()),
                segment: befund.map(|(s, _)| s),
                text: output["grund"].as_str().map(str::to_owned),
            }
        }
        _ => nicht_geprueft(Stufe::Probe, "unreadable_answer", None),
    }
}

/// Hold `aktion` (with `antwortcode`) on `entry` of `art` against the gate.
pub async fn pruefen(
    pruefer: &Pruefer,
    art: Art,
    entry: &str,
    aktion: &str,
    antwortcode: Option<&str>,
) -> Pruefvermerk {
    if art != Art::Freigabe {
        return Pruefvermerk::ohne_probe("keine_probe");
    }
    let Some(processd) = pruefer.processd.as_ref() else {
        return Pruefvermerk::aus(vec![nicht_geprueft(
            Stufe::Entscheidung,
            "not_configured",
            None,
        )]);
    };
    let args = json!({
        "id": entry,
        "freigeben": aktion == "freigeben",
        "antwort_code": antwortcode,
        "antwort_tree": null,
    });
    let mut body_sha256 = None;
    let (erste, befehl) = match processd
        .call(&ToolId::new("processd", "rehearse_decision"), &args, None)
        .await
    {
        Ok(output) => {
            body_sha256 = output["body_sha256"].as_str().map(str::to_owned);
            entscheidung(&output)
        }
        Err(e) => {
            let (code, text) = fehler(&e);
            (nicht_geprueft(Stufe::Entscheidung, &code, Some(text)), None)
        }
    };
    let mut stufen = vec![erste];
    if let Some((command, marktrolle, payload)) = befehl {
        let zweite = match pruefer.makod.as_ref() {
            None => nicht_geprueft(Stufe::Probe, "not_configured", None),
            Some(makod) => {
                let args = json!({
                    "command": command,
                    "marktrolle": marktrolle,
                    "payload": payload,
                });
                match makod
                    .call(&ToolId::new("makod", "rehearse_command"), &args, None)
                    .await
                {
                    Ok(output) => probe(&output),
                    Err(e) => {
                        let (code, text) = fehler(&e);
                        nicht_geprueft(Stufe::Probe, &code, Some(text))
                    }
                }
            }
        };
        stufen.push(zweite);
    }
    Pruefvermerk {
        body_sha256,
        ..Pruefvermerk::aus(stufen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_code_is_read_off_the_mcp_message_and_anything_else_is_unreachable() {
        assert_eq!(
            fehlercode("not_rehearsable: \"gpke.x\" cannot be rehearsed"),
            "not_rehearsable"
        );
        assert_eq!(
            fehlercode("connection refused (os error 61)"),
            "unreachable"
        );
        assert_eq!(fehlercode(""), "unreachable");
    }

    /// A stage that did not run keeps the whole verdict from passing.
    #[test]
    fn only_every_stage_passing_is_bestanden() {
        let ok = |stufe| Pruefstufe {
            stufe,
            urteil: PruefUrteil::Bestanden,
            code: None,
            regel: None,
            segment: None,
            text: None,
        };
        assert_eq!(
            Pruefvermerk::aus(vec![ok(Stufe::Entscheidung), ok(Stufe::Probe)]).urteil,
            PruefUrteil::Bestanden
        );
        assert_eq!(
            Pruefvermerk::aus(vec![
                ok(Stufe::Entscheidung),
                nicht_geprueft(Stufe::Probe, "not_rehearsable", None)
            ])
            .urteil,
            PruefUrteil::NichtGeprueft
        );
        assert_eq!(Pruefvermerk::aus(vec![]).urteil, PruefUrteil::NichtGeprueft);
        for u in [
            PruefUrteil::Bestanden,
            PruefUrteil::Abgelehnt,
            PruefUrteil::NichtGeprueft,
        ] {
            assert_eq!(PruefUrteil::from_label(u.label()), Some(u));
        }
    }
}
