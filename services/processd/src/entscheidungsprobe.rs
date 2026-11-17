//! Rehearse a decision on a Freigabe: run the checks `approve` / `reject`
//! run before they dispatch, and answer with the body the decision would send
//! to makod — or the refusal — without claiming the entry or sending anything.
//!
//! The checks are the decision route's own (`decision_body` then
//! `check_operator_code`), so a rehearsal refuses exactly what a decision
//! refuses, in the same order: a frozen answer refuses an edit before any code
//! is judged.

use rmcp::schemars;
use schemars::JsonSchema;
use serde::Serialize;

use crate::pg::approval::{
    ApprovalQueueEntry, DecisionRefusal, OperatorAnswer, QueueStatus, check_operator_code,
    decision_body,
};

/// What the decision route says about a rehearsed decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntscheidungsUrteil {
    /// The decision passes; `command` and `payload` are what makod would get.
    Besteht,
    /// The decision is refused; `grund` says why.
    Abgelehnt,
    /// The decision passes and sends no market message — the entry stores no
    /// command for it.
    OhneNachricht,
}

/// Why a decision is refused: a stable code, the refusal's sentence, and the
/// Entscheidungsbaum where the refusal is about a code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct EntscheidungsGrund {
    /// `not_pending`, `unknown_command`, `answer_frozen`, `missing_fields`,
    /// `wrong_tree`, `code_outside_tree`, `fact_not_taken` or `invalid_fact`.
    pub code: String,
    /// The refusal the decision route answers with.
    pub text: String,
    /// The Entscheidungsbaum the stated code was checked in.
    pub ebd: Option<String>,
}

/// `rehearse_decision`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct EntscheidungsProbe {
    /// The queue entry.
    pub id: String,
    /// `true` for approve, `false` for reject.
    pub freigeben: bool,
    pub urteil: EntscheidungsUrteil,
    /// The makod command the decision dispatches.
    pub command: Option<String>,
    /// The Marktrolle it is dispatched under.
    pub marktrolle: Option<String>,
    /// The body makod would receive — the frozen answer with what the
    /// decision states filled in; absent on a refusal.
    pub payload: Option<serde_json::Value>,
    /// SHA-256 of the command and `payload` in RFC 8785 form
    /// ([`crate::pg::approval::vorschlag_sha256`]) — what an approval states
    /// as `body_sha256` to say it approves exactly this body.
    pub body_sha256: Option<String>,
    /// Present exactly when `urteil` is `abgelehnt`.
    pub grund: Option<EntscheidungsGrund>,
}

/// The stable code of a refusal.
#[must_use]
pub fn refusal_code(refusal: &DecisionRefusal) -> &'static str {
    match refusal {
        DecisionRefusal::UnknownCommand(_) => "unknown_command",
        DecisionRefusal::AnswerFrozen => "answer_frozen",
        DecisionRefusal::Missing { .. } => "missing_fields",
        DecisionRefusal::WrongTree { .. } => "wrong_tree",
        DecisionRefusal::CodeOutsideTree { .. } => "code_outside_tree",
        DecisionRefusal::FactNotTaken { .. } => "fact_not_taken",
        DecisionRefusal::InvalidFact { .. } => "invalid_fact",
    }
}

/// Rehearse deciding `entry` — approve when `freigeben`, else reject — with
/// `operator`'s answer. Pure: reads the entry, writes nothing.
#[must_use]
pub fn rehearse(
    entry: &ApprovalQueueEntry,
    freigeben: bool,
    operator: &OperatorAnswer,
) -> EntscheidungsProbe {
    let mut probe = EntscheidungsProbe {
        id: entry.id.to_string(),
        freigeben,
        urteil: EntscheidungsUrteil::Abgelehnt,
        command: None,
        marktrolle: entry.marktrolle.clone(),
        payload: None,
        body_sha256: None,
        grund: None,
    };
    if entry.status != QueueStatus::Pending {
        probe.grund = Some(EntscheidungsGrund {
            code: "not_pending".to_owned(),
            text: format!(
                "the entry is {} — only a pending entry is decided",
                entry.status
            ),
            ebd: None,
        });
        return probe;
    }
    let (command, frozen) = if freigeben {
        (
            entry.approve_command.as_deref(),
            entry.answer_payload.as_ref(),
        )
    } else {
        (
            entry.reject_command.as_deref(),
            entry.reject_payload.as_ref(),
        )
    };
    let Some(command) = command else {
        probe.urteil = EntscheidungsUrteil::OhneNachricht;
        return probe;
    };
    probe.command = Some(command.to_owned());
    match decision_body(entry, command, frozen, operator)
        .and_then(|body| check_operator_code(entry, freigeben, operator).map(|()| body))
    {
        Ok(body) => {
            probe.urteil = EntscheidungsUrteil::Besteht;
            probe.body_sha256 = Some(crate::pg::approval::vorschlag_sha256(command, &body));
            probe.payload = Some(body);
        }
        Err(refusal) => {
            let ebd = match &refusal {
                DecisionRefusal::CodeOutsideTree { ebd, .. } => Some(ebd.clone()),
                DecisionRefusal::WrongTree { expected, .. } => Some(expected.clone()),
                _ => None,
            };
            probe.grund = Some(EntscheidungsGrund {
                code: refusal_code(&refusal).to_owned(),
                text: refusal.to_string(),
                ebd,
            });
        }
    }
    probe
}

/// The approval states the body it was shown (`body_sha256`, as
/// [`rehearse`] reports it): refuse it when the body the decision would
/// dispatch is another — the frozen answer changed, or the person approved a
/// stale rendering. No stated hash checks nothing.
///
/// # Errors
///
/// `412 stale_body` naming both hashes.
pub fn gezeigter_koerper(
    operator: &OperatorAnswer,
    command: &str,
    body: &serde_json::Value,
) -> Result<(), mako_service::ApiError> {
    let Some(shown) = operator.body_sha256.as_deref().filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let sent = crate::pg::approval::vorschlag_sha256(command, body);
    if shown == sent {
        Ok(())
    } else {
        Err(mako_service::ApiError::precondition_failed(format!(
            "the body shown ({shown}) is not the body this decision dispatches ({sent}); \
             reload the Freigabe"
        ))
        .with_code("stale_body"))
    }
}

/// makod's pre-send check on the message a decision dispatches, run before
/// the decision is marked taken: makod rehearses `cmd` — its door up to the
/// check the outbox worker runs before sending — and writes nothing.
///
/// Only a passing rehearsal lets the decision through. A refusal is answered
/// in place, naming makod's sentence and the profile rules; a body makod
/// declined to rehearse is unchecked and refused just the same, so no
/// approval — whoever proposed the answer, with or without agentd — sends a
/// body the gate has not passed.
///
/// # Errors
///
/// `422 pruefung_abgelehnt` (the check refuses), `422 nicht_geprueft` (makod
/// did not rehearse it), `502` when makod cannot be asked.
pub async fn makod_prueft(
    makod: &mako_markt::makod_client::MakodClient,
    cmd: &mako_markt::makod_client::ForwardCommand,
) -> Result<(), mako_service::ApiError> {
    use mako_markt::makod_client::ProbeUrteil;
    let probe = makod
        .probe_command(cmd)
        .await
        .map_err(|e| mako_service::ApiError::upstream("makod", anyhow::Error::new(e)))?;
    let grund = probe.grund.clone().unwrap_or_default();
    match probe.urteil {
        ProbeUrteil::Besteht => Ok(()),
        ProbeUrteil::Abgelehnt => Err(mako_service::ApiError::unprocessable(format!(
            "makod's pre-send check refuses the {}: {grund}",
            cmd.command
        ))
        .with_code("pruefung_abgelehnt")
        .with_fields(serde_json::json!({
            "command": cmd.command,
            "stufe": probe.stufe,
            "grund": probe.grund,
            "regeln": probe.regeln,
        }))),
        ProbeUrteil::NichtGeprueft => Err(mako_service::ApiError::unprocessable(format!(
            "makod did not check the {} — an unchecked answer is not sent: {grund}",
            cmd.command
        ))
        .with_code("nicht_geprueft")
        .with_fields(serde_json::json!({
            "command": cmd.command,
            "makod_code": probe.code,
            "grund": probe.grund,
        }))),
    }
}

/// A decision changes the market, so its request must be one a cross-site
/// form cannot send: `Content-Type: application/json` — which a browser sends
/// cross-origin only after a CORS preflight — and nothing else, even on an
/// empty body.
///
/// # Errors
///
/// `415 json_required`.
pub fn nur_json(headers: &axum::http::HeaderMap) -> Result<(), mako_service::ApiError> {
    let json = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
    if json {
        Ok(())
    } else {
        Err(mako_service::ApiError::UnsupportedMediaType(
            "a decision is posted as application/json".to_owned(),
        )
        .with_code("json_required"))
    }
}

/// Parameters for `rehearse_decision`.
#[derive(Debug, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RehearseDecisionParams {
    /// The queue entry's UUID.
    pub id: String,
    /// `true` to rehearse approving, `false` to rehearse rejecting.
    pub freigeben: bool,
    /// The Antwortcode the decision would state; none for a held answer
    /// approved as frozen.
    pub antwort_code: Option<String>,
    /// The Entscheidungsbaum the code is stated from, where the caller names
    /// one; it must be the entry's.
    pub antwort_tree: Option<String>,
}

#[rmcp::tool_router(router = decision_probe_tool_router, vis = "pub(crate)")]
impl crate::mcp_server::ProcessdMcpHandler {
    #[rmcp::tool(
        description = "Rehearse a decision on a Freigabe without making it: run the checks \
                       approve_queue_entry / reject_queue_entry run before they dispatch — the \
                       frozen answer, the facts the command needs, the Antwortcode against the \
                       entry's Entscheidungsbaum — and answer `urteil` besteht with the makod \
                       `command` and the `payload` it would be sent with, abgelehnt with \
                       `grund` (stable `code`, the refusal, the tree), or ohne_nachricht. \
                       Claims nothing, sends nothing.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<EntscheidungsProbe>()
    )]
    async fn rehearse_decision(
        &self,
        rmcp::handler::server::wrapper::Parameters(p): rmcp::handler::server::wrapper::Parameters<
            RehearseDecisionParams,
        >,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let Ok(id) = p.id.parse::<uuid::Uuid>() else {
            return Err(rmcp::ErrorData::invalid_params(
                "id must be a valid UUID",
                None,
            ));
        };
        let queue = crate::pg::approval::PgApprovalQueue::new(self.state.pool.clone());
        let entry = queue
            .find_by_id(id, &self.state.tenant)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))?
            .ok_or_else(|| {
                rmcp::ErrorData::invalid_params(
                    format!("approval queue entry {id} not found"),
                    None,
                )
            })?;
        let operator = OperatorAnswer {
            antwort_code: p.antwort_code,
            antwort_tree: p.antwort_tree,
            ..OperatorAnswer::default()
        };
        serde_json::to_value(rehearse(&entry, p.freigeben, &operator))
            .map(rmcp::model::CallToolResult::structured)
            .map_err(|e| rmcp::ErrorData::internal_error(e.to_string(), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_pruefung::codes::{self, Cluster, EBD_ANMELDUNG_DIREKT_ABLEHNBAR, EBD_LIEFERBEGINN};
    use uuid::Uuid;

    fn escalated() -> ApprovalQueueEntry {
        ApprovalQueueEntry::pending(
            Uuid::nil(),
            55_001,
            Some("51238696012".into()),
            "test".into(),
            time::macros::datetime!(2026-07-09 09:00 UTC),
            "t".into(),
            "test-regel",
        )
        .with_commands(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
            Some("NB"),
        )
        .with_ebd(EBD_ANMELDUNG_DIREKT_ABLEHNBAR)
    }

    fn code(c: &str) -> OperatorAnswer {
        OperatorAnswer {
            antwort_code: Some(c.to_owned()),
            ..OperatorAnswer::default()
        }
    }

    /// A refusal with a Zustimmungscode is refused naming the tree the code
    /// was checked in — the check the reject route runs.
    #[test]
    fn a_code_outside_the_trees_cluster_is_refused_with_its_tree() {
        let zustimmung = codes::codes_im_cluster(EBD_LIEFERBEGINN, Cluster::Zustimmung)
            .next()
            .expect("E_0623 publishes a Zustimmung")
            .code;
        let p = rehearse(&escalated(), false, &code(zustimmung));
        assert_eq!(p.urteil, EntscheidungsUrteil::Abgelehnt);
        let grund = p.grund.expect("a refusal says why");
        assert_eq!(grund.code, "code_outside_tree");
        assert_eq!(grund.ebd.as_deref(), Some(EBD_ANMELDUNG_DIREKT_ABLEHNBAR));
        assert!(p.payload.is_none());
    }

    /// A code the tree publishes passes, and the body is what the reject
    /// route would dispatch: the code, the process, the MaLo.
    #[test]
    fn a_code_of_the_tree_passes_with_the_body_makod_would_get() {
        let ablehnung = codes::codes_im_cluster(EBD_ANMELDUNG_DIREKT_ABLEHNBAR, Cluster::Ablehnung)
            .next()
            .expect("E_0622 publishes Ablehnungen")
            .code;
        let p = rehearse(&escalated(), false, &code(ablehnung));
        assert_eq!(p.urteil, EntscheidungsUrteil::Besteht, "{p:?}");
        assert_eq!(
            p.command.as_deref(),
            Some(mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN)
        );
        assert_eq!(
            p.body_sha256.as_deref(),
            p.payload
                .as_ref()
                .map(|b| crate::pg::approval::vorschlag_sha256(
                    mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
                    b
                ))
                .as_deref()
        );
        let body = p.payload.expect("the body");
        assert_eq!(body["antwort_code"], ablehnung);
        assert_eq!(body["malo_id"], "51238696012");
        assert!(body.get("rejected_by").is_none(), "nobody decided");
    }

    /// A held answer is approved as frozen: a code stated beside it is the
    /// edit the route refuses.
    #[test]
    fn a_code_beside_a_frozen_answer_is_refused() {
        let held = escalated()
            .with_ebd(EBD_LIEFERBEGINN)
            .with_answer_payload(serde_json::json!({
                "antwort_code": "A51", "antwort_tree": EBD_LIEFERBEGINN,
            }));
        let p = rehearse(&held, true, &code("A51"));
        assert_eq!(p.grund.map(|g| g.code).as_deref(), Some("answer_frozen"));
        let p = rehearse(&held, true, &OperatorAnswer::default());
        assert_eq!(p.urteil, EntscheidungsUrteil::Besteht);
    }

    /// A decided entry is not rehearsed, and a decision without a command
    /// sends nothing.
    #[test]
    fn a_decided_entry_is_refused_and_a_commandless_one_sends_nothing() {
        let mut decided = escalated();
        decided.status = QueueStatus::Approved;
        let p = rehearse(&decided, true, &OperatorAnswer::default());
        assert_eq!(p.grund.map(|g| g.code).as_deref(), Some("not_pending"));

        let ohne = escalated()
            .with_reject_command(mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN, Some("NB"));
        let p = rehearse(&ohne, true, &OperatorAnswer::default());
        assert_eq!(p.urteil, EntscheidungsUrteil::OhneNachricht);
        assert!(p.command.is_none());
    }

    /// An approval of a stale rendering is refused 412; the body it was
    /// shown passes, and no stated hash checks nothing.
    #[test]
    fn an_approval_of_a_stale_rendering_is_refused_412() {
        let body = serde_json::json!({ "antwort_code": "A51" });
        let cmd = mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN;
        let said = |h: &str| OperatorAnswer {
            body_sha256: Some(h.to_owned()),
            ..OperatorAnswer::default()
        };
        let sha = crate::pg::approval::vorschlag_sha256(cmd, &body);
        assert!(gezeigter_koerper(&said(&sha), cmd, &body).is_ok());
        assert!(gezeigter_koerper(&OperatorAnswer::default(), cmd, &body).is_ok());
        let stale = gezeigter_koerper(&said("00"), cmd, &body).expect_err("stale");
        assert_eq!(stale.status(), axum::http::StatusCode::PRECONDITION_FAILED);
    }

    /// A makod that answers every rehearsal with `answer` (and `status`),
    /// counting what it is asked to send.
    async fn makod(
        status: u16,
        answer: serde_json::Value,
    ) -> (
        mako_markt::makod_client::MakodClient,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let sent = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = sent.clone();
        let app = axum::Router::new()
            .route(
                "/api/v1/probe/{command}",
                axum::routing::post(move || {
                    let answer = answer.clone();
                    async move {
                        (
                            axum::http::StatusCode::from_u16(status).expect("status"),
                            axum::Json(answer),
                        )
                    }
                }),
            )
            .route(
                "/api/v1/commands",
                axum::routing::post(move || {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { axum::http::StatusCode::ACCEPTED }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (
            mako_markt::makod_client::MakodClient::new(
                format!("http://{addr}"),
                secrecy::SecretString::from("k".to_owned()),
            ),
            sent,
        )
    }

    fn answer() -> mako_markt::makod_client::ForwardCommand {
        mako_markt::makod_client::ForwardCommand {
            marktrolle: Some("NB".into()),
            command: mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN.into(),
            malo_id: Some("51238696012".into()),
            melo_id: None,
            payload: serde_json::json!({ "antwort_code": "A51" }),
        }
    }

    async fn refusal(err: mako_service::ApiError) -> (u16, serde_json::Value) {
        use axum::response::IntoResponse as _;
        let resp = err.into_response();
        let status = resp.status().as_u16();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .expect("body");
        (status, serde_json::from_slice(&body).expect("json"))
    }

    /// The gate refuses the body: the decision is answered 422 with makod's
    /// sentence and the profile rule, and nothing is sent.
    #[tokio::test]
    async fn a_refused_body_is_answered_with_the_rule() {
        let (makod, sent) = makod(
            200,
            serde_json::json!({
                "urteil": "abgelehnt", "stufe": "pruefung",
                "grund": "the UTILMD violates its AHB profile: SG4 STS",
                "nachrichten": [{ "segmente": [], "befunde": [{ "text": "t", "regel": "AHB-55002-7" }] }]
            }),
        )
        .await;
        let err = makod_prueft(&makod, &answer()).await.expect_err("refused");
        let (status, body) = refusal(err).await;
        assert_eq!(status, 422);
        assert_eq!(body["code"], "pruefung_abgelehnt");
        assert_eq!(body["regeln"][0], "AHB-55002-7");
        assert_eq!(sent.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// makod declined to rehearse the command: the body is unchecked and is
    /// refused like a failing one.
    #[tokio::test]
    async fn an_unchecked_body_is_refused() {
        let (makod, _) = makod(
            422,
            serde_json::json!({ "code": "not_rehearsable", "detail": "cannot be rehearsed" }),
        )
        .await;
        let (status, body) =
            refusal(makod_prueft(&makod, &answer()).await.expect_err("refused")).await;
        assert_eq!(status, 422);
        assert_eq!(body["code"], "nicht_geprueft");
        assert_eq!(body["makod_code"], "not_rehearsable");
    }

    /// A passing rehearsal lets the decision through; an unreachable makod
    /// is a 502, never a pass.
    #[tokio::test]
    async fn only_a_passing_rehearsal_lets_the_decision_through() {
        let (makod, _) = makod(200, serde_json::json!({ "urteil": "besteht" })).await;
        assert!(makod_prueft(&makod, &answer()).await.is_ok());
        let gone = mako_markt::makod_client::MakodClient::new(
            "http://127.0.0.1:1",
            secrecy::SecretString::from("k".to_owned()),
        );
        let (status, _) = refusal(
            makod_prueft(&gone, &answer())
                .await
                .expect_err("unreachable"),
        )
        .await;
        assert_eq!(status, 502);
    }

    /// A cross-site form posts `text/plain` or a form encoding; only JSON
    /// decides.
    #[test]
    fn a_decision_that_is_not_json_is_refused() {
        let mut h = axum::http::HeaderMap::new();
        assert!(nur_json(&h).is_err(), "no content type");
        h.insert(
            axum::http::header::CONTENT_TYPE,
            "text/plain".parse().unwrap(),
        );
        let refused = nur_json(&h).expect_err("text/plain");
        assert_eq!(
            refused.status(),
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "a body of the wrong media type is 415, not a malformed body"
        );
        assert_eq!(refused.code(), Some("json_required"));
        h.insert(
            axum::http::header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert!(nur_json(&h).is_ok());
    }
}
