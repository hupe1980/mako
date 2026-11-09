//! Rehearse a command before go-live: run its door up to the pre-send check
//! and answer with the check's verdict and the message the door built, each
//! segment named as the profile of the message's own format version names its
//! place. Nothing is queued, sent or stored.
//!
//! **The rehearsal runs the door itself.** There is no second copy of what a
//! door reads, derives and refuses: [`rehearse`] calls the command's registered
//! dispatch function inside a rehearsal scope, and the door's own pre-send
//! check (`refuse_what_the_gate_refuses`) is where the
//! scope stops it — the check hands the outbox the workflow's pure `handle`
//! produced to the rehearsal and ends the door with [`DispatchError::Geprobt`],
//! before the duplicate guard and before `spawn_process`. So a
//! rehearsal refuses exactly what the door refuses, in the door's words, and
//! its verdict on the message is `gate_verdict`'s —
//! the refusal the door itself returns.
//!
//! Two things keep a rehearsal from writing:
//!
//! - only a door whose pre-send check runs before anything is written is
//!   rehearsable ([`REHEARSABLE`]); any other command is refused, naming why;
//! - `spawn_process` refuses inside a rehearsal scope, so a door that
//!   reached its spawn would fail before the write rather than after it.

use std::cell::RefCell;

use mako_engine::outbox::PendingOutbox;
use serde::{Deserialize, Serialize};

use super::{COMMAND_REGISTRY, CommandError, CommandsApiState, DispatchError, validate_command};

tokio::task_local! {
    /// The outbox the door built, set by its pre-send check while a rehearsal
    /// runs. Present only inside [`rehearse`].
    static PROBE: RefCell<Option<Vec<PendingOutbox>>>;
}

/// The commands a rehearsal can run: each door renders its UTILMD and holds it
/// against its profile before it writes anything, so stopping at that check
/// leaves nothing behind.
///
/// | Command | Prüfidentifikator |
/// |---|---|
/// | `gpke.lieferbeginn.anmelden` | 55001 Anmeldung (LF → NB) |
/// | `gpke.lieferende.anmelden` | 55004 Abmeldung (LF → NB) |
/// | `gpke.kuendigung.anmelden` | 55016 Kündigung (LFN → LFA) |
/// | `geli.lieferbeginn.anmelden` | 44001 Anmeldung NN (LF → GNB) |
/// | `geli.lieferende.anmelden` | 44004 Abmeldung NN (LF → GNB) |
/// | `geli.kuendigung.anmelden` | 44016 Kündigung (LFN → LFA) |
pub const REHEARSABLE: &[&str] = &[
    "gpke.lieferbeginn.anmelden",
    "gpke.lieferende.anmelden",
    "gpke.kuendigung.anmelden",
    "geli.lieferbeginn.anmelden",
    "geli.lieferende.anmelden",
    "geli.kuendigung.anmelden",
];

/// Whether the current task runs inside a rehearsal.
pub(super) fn active() -> bool {
    PROBE.try_with(|_| ()).is_ok()
}

/// Inside a rehearsal, take the door's outbox and return `true` — the door
/// stops there. Outside one, `false`: the door goes on to its check.
pub(super) fn halt(outbox: &[PendingOutbox]) -> bool {
    PROBE
        .try_with(|cell| *cell.borrow_mut() = Some(outbox.to_vec()))
        .is_ok()
}

// ── Wire types ────────────────────────────────────────────────────────────────

/// The body of `POST /api/v1/probe/{command}`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProbeRequest {
    /// The Marktrolle to rehearse as — required only where the command is
    /// permitted for more than one, as on `POST /api/v1/commands`.
    #[schema(example = "LF")]
    pub marktrolle: Option<String>,
    /// The command's payload, exactly as `POST /api/v1/commands` takes it.
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
}

/// What the pre-send check says about a rehearsed command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Urteil {
    /// The door built the message and the pre-send check passes it.
    Besteht,
    /// The door or the pre-send check refuses; `grund` says which and why.
    Abgelehnt,
}

/// Where a refusal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Stufe {
    /// The door refused the command before a message existed — a missing
    /// field, a Marktlokation not in the cache, a Stammdatum marktd lacks.
    Tuer,
    /// The door built the message and its profile refuses it.
    Pruefung,
}

/// One finding of the pre-send check.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, utoipa::ToSchema)]
pub struct ProbeBefund {
    /// The check's own wording.
    pub text: String,
    /// The profile rule it rests on (`AHB-…`, `MIG-…`), where it names one.
    pub regel: Option<String>,
    /// Zero-based index into the segment's `elemente`, where the finding
    /// names a data element.
    pub element: Option<u16>,
    /// Zero-based component within that data element.
    pub komponente: Option<u16>,
}

/// One segment of the rendered message.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, utoipa::ToSchema)]
pub struct ProbeSegment {
    /// One-based position in the message, `UNH` = 1.
    pub position: u32,
    /// The segment tag.
    pub tag: String,
    /// The MIG's running number of the place the segment sits at.
    pub nr: Option<String>,
    /// The segment groups enclosing the place, outermost first (`SG4/SG8`).
    pub gruppe: Option<String>,
    /// The MIG's name for the place; absent when the segment fits no place.
    pub name: Option<String>,
    /// The segment as it goes on the wire, terminator included.
    pub text: String,
    /// Each data element as written, components joined by `:`.
    pub elemente: Vec<String>,
    /// The findings that sit on this segment.
    pub befunde: Vec<ProbeBefund>,
}

/// One message the door built.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, utoipa::ToSchema)]
pub struct ProbeNachricht {
    /// `UTILMD`, ….
    pub nachrichtentyp: String,
    /// The Prüfidentifikator the message carries.
    pub pruefidentifikator: Option<u32>,
    /// The MP-ID it is addressed to.
    pub empfaenger: String,
    /// The format version whose profile named the segments (`FV2025-10-01`).
    pub formatversion: Option<String>,
    /// The AHB version of that profile.
    pub ahb_version: Option<String>,
    /// The interchange as rendered, `UNA` … `UNZ`.
    pub wire: String,
    /// The message, `UNH` … `UNT`, segment by segment.
    pub segmente: Vec<ProbeSegment>,
    /// Findings the check placed on no single segment.
    pub befunde: Vec<ProbeBefund>,
}

/// The answer of a rehearsal.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema, utoipa::ToSchema)]
pub struct Probe {
    /// The command rehearsed.
    pub command: String,
    /// The Marktrolle it was rehearsed as.
    pub marktrolle: String,
    /// The pre-send check's verdict.
    pub urteil: Urteil,
    /// Where the refusal came from; absent when the message passes.
    pub stufe: Option<Stufe>,
    /// The refusal in the door's own words — the sentence the real command
    /// would be answered with.
    pub grund: Option<String>,
    /// The messages the door built; empty when it refused before building one.
    pub nachrichten: Vec<ProbeNachricht>,
}

/// Why a rehearsal did not run.
#[derive(Debug)]
pub enum ProbeRefusal {
    /// No command of that name is registered.
    UnknownCommand(String),
    /// The command's door has no pre-send check that runs before it writes.
    NotRehearsable(String),
    /// The command needs a Marktrolle the deployment does not declare.
    RolleNichtDeklariert {
        /// The Rolle the command needs.
        rolle: String,
        /// The Rollen `[deployment] rollen` declares.
        deklariert: Vec<String>,
    },
    /// The command is permitted for several Marktrollen and none was named.
    MarktrolleRequired(String),
    /// The named Marktrolle may not issue the command.
    RoleNotPermitted {
        /// The command.
        command: String,
        /// The Rolle named.
        rolle: String,
    },
    /// The store or the engine failed underneath the door.
    Engine(String),
}

impl ProbeRefusal {
    /// The stable machine code of the refusal.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownCommand(_) => "unknown_command",
            Self::NotRehearsable(_) => "not_rehearsable",
            Self::RolleNichtDeklariert { .. } => "role_not_configured",
            Self::MarktrolleRequired(_) => "marktrolle_required",
            Self::RoleNotPermitted { .. } => "role_not_permitted",
            Self::Engine(_) => "engine_error",
        }
    }

    /// The refusal, written for the caller.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::UnknownCommand(c) => format!("Unknown command {c:?}."),
            Self::NotRehearsable(c) => format!(
                "{c:?} cannot be rehearsed: its door has no pre-send check that runs before it \
                 writes, so stopping there would not leave the store untouched. Rehearsable: {}.",
                REHEARSABLE.join(", ")
            ),
            Self::RolleNichtDeklariert { rolle, deklariert } => format!(
                "This deployment does not declare the Marktrolle {rolle}, which the command \
                 needs — [deployment] rollen declares {}.",
                if deklariert.is_empty() {
                    "none".to_owned()
                } else {
                    deklariert.join(", ")
                }
            ),
            Self::MarktrolleRequired(c) => format!(
                "{c:?} is permitted for several Marktrollen — name the one to rehearse as in \
                 \"marktrolle\"."
            ),
            Self::RoleNotPermitted { command, rolle } => {
                format!("Marktrolle {rolle} may not issue {command:?}.")
            }
            Self::Engine(e) => format!("the rehearsal failed underneath the door: {e}"),
        }
    }
}

// ── Admission ─────────────────────────────────────────────────────────────────

/// The Marktrolle a rehearsal of `command` runs as, or why it does not run.
///
/// # Errors
///
/// [`ProbeRefusal`] for an unknown or non-rehearsable command and for a
/// Marktrolle the command does not admit or the deployment does not declare.
pub fn zulassen(
    state: &CommandsApiState,
    command: &str,
    asserted: Option<&str>,
) -> Result<String, ProbeRefusal> {
    let Some(desc) = COMMAND_REGISTRY.iter().find(|d| d.name == command) else {
        return Err(ProbeRefusal::UnknownCommand(command.to_owned()));
    };
    if !REHEARSABLE.contains(&command) {
        return Err(ProbeRefusal::NotRehearsable(command.to_owned()));
    }
    let asserted = asserted.map(str::to_uppercase);
    validate_command(command, asserted.as_deref(), &state.configured_marktrollen).map_err(|e| {
        match e {
            CommandError::RoleNotConfigured => ProbeRefusal::RolleNichtDeklariert {
                rolle: asserted.clone().unwrap_or_else(|| {
                    desc.permitted_roles
                        .iter()
                        .map(|r| r.as_code())
                        .collect::<Vec<_>>()
                        .join(" / ")
                }),
                deklariert: state.configured_marktrollen.clone(),
            },
            CommandError::MarktrolleRequired => {
                ProbeRefusal::MarktrolleRequired(command.to_owned())
            }
            CommandError::RoleNotPermitted => ProbeRefusal::RoleNotPermitted {
                command: command.to_owned(),
                rolle: asserted.clone().unwrap_or_default(),
            },
            CommandError::UnknownCommand => ProbeRefusal::UnknownCommand(command.to_owned()),
        }
    })
}

// ── Rehearsal ─────────────────────────────────────────────────────────────────

/// Run `command`'s door on `payload` up to its pre-send check and report the
/// verdict and the message. Call [`zulassen`] first: it decides the Marktrolle
/// and refuses a command whose door cannot be stopped before it writes.
///
/// # Errors
///
/// [`ProbeRefusal::NotRehearsable`] for a command outside [`REHEARSABLE`],
/// [`ProbeRefusal::Engine`] when the store fails underneath the door.
pub async fn rehearse(
    state: &CommandsApiState,
    command: &str,
    marktrolle: String,
    payload: &serde_json::Value,
) -> Result<Probe, ProbeRefusal> {
    let desc = COMMAND_REGISTRY
        .iter()
        .find(|d| d.name == command && REHEARSABLE.contains(&d.name))
        .ok_or_else(|| ProbeRefusal::NotRehearsable(command.to_owned()))?;

    let (outcome, outbox) = PROBE
        .scope(RefCell::new(None), async {
            let outcome = (desc.dispatch)(state, payload).await;
            let outbox = PROBE.with(|cell| cell.borrow_mut().take());
            (outcome, outbox)
        })
        .await;

    let probe = |urteil, stufe, grund, nachrichten| Probe {
        command: command.to_owned(),
        marktrolle: marktrolle.clone(),
        urteil,
        stufe,
        grund,
        nachrichten,
    };
    match (outcome, outbox) {
        (Err(DispatchError::Geprobt), Some(outbox)) => {
            let nachrichten = outbox
                .iter()
                .filter(|p| &*p.message_type == "UTILMD")
                .filter_map(|p| nachricht(state, p))
                .collect();
            Ok(match super::eog_stammdaten::gate_verdict(state, &outbox) {
                Ok(()) => probe(Urteil::Besteht, None, None, nachrichten),
                Err(e) => probe(
                    Urteil::Abgelehnt,
                    Some(Stufe::Pruefung),
                    Some(crate::api::mcp_server::dispatch_error_to_string(e)),
                    nachrichten,
                ),
            })
        }
        (Err(DispatchError::Engine(e)), _) => Err(ProbeRefusal::Engine(e.to_string())),
        (Err(e), _) => Ok(probe(
            Urteil::Abgelehnt,
            Some(Stufe::Tuer),
            Some(crate::api::mcp_server::dispatch_error_to_string(e)),
            Vec::new(),
        )),
        (Ok(_), _) => {
            tracing::error!(
                command,
                "a rehearsed door completed past its pre-send check"
            );
            Err(ProbeRefusal::Engine(format!(
                "the door of {command:?} completed inside a rehearsal"
            )))
        }
    }
}

/// The rendered message of `pending`, annotated against its own profile;
/// `None` when it does not render — the verdict then carries why.
fn nachricht(state: &CommandsApiState, pending: &PendingOutbox) -> Option<ProbeNachricht> {
    use edi_energy::EdiEnergyMessage as _;

    let check =
        crate::orchestrator::edifact_renderer::utilmd_gate_check(pending, state.tenant_id).ok()?;
    let parsed = edi_energy::parse(&check.bytes).ok()?;
    // The message, `UNH` … `UNT`: the profile names no envelope segment, and
    // a finding's position counts from `UNH` = 1.
    let segments: Vec<edifact_rs::OwnedSegment> = parsed
        .segments()
        .iter()
        .filter(|s| !matches!(&*s.tag, "UNA" | "UNB" | "UNG" | "UNE" | "UNZ"))
        .cloned()
        .collect();
    let segments = segments.as_slice();
    let profile = parsed
        .try_message_type()
        .zip(parsed.detect_release().ok())
        .and_then(|(mt, release)| {
            edi_energy::registry::ReleaseRegistry::global()
                .profile_on(mt, release, mako_fristen::heute())
                .ok()
        });
    let resolution = profile.map(|p| p.resolve(segments));

    let mut segmente: Vec<ProbeSegment> = segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let place = resolution
                .as_ref()
                .and_then(|r| r.assigned.get(i).copied().flatten())
                .zip(profile);
            let gruppe = place
                .map(|(a, p)| p.structure.path(a.node).join("/"))
                .filter(|g| !g.is_empty());
            ProbeSegment {
                position: u32::try_from(i + 1).unwrap_or(u32::MAX),
                tag: seg.tag.to_string(),
                nr: place.and_then(|(a, p)| p.structure.nr(a.node).map(str::to_owned)),
                gruppe,
                name: place.map(|(a, p)| p.structure.nodes[a.node].name.clone()),
                text: edifact_rs::segments_to_bytes(std::slice::from_ref(seg))
                    .map(|b| latin1(&b).trim_end().to_owned())
                    .unwrap_or_default(),
                elemente: seg.elements.iter().map(element_text).collect(),
                befunde: Vec::new(),
            }
        })
        .collect();

    let mut lose = Vec::new();
    for issue in &check.findings {
        let befund = ProbeBefund {
            text: issue.message.clone(),
            regel: issue.rule_id.clone(),
            element: issue.element_index,
            komponente: issue.component_index,
        };
        match platz(issue, segments) {
            Some(i) => segmente[i].befunde.push(befund),
            None => lose.push(befund),
        }
    }

    Some(ProbeNachricht {
        nachrichtentyp: pending.message_type.to_string(),
        pruefidentifikator: parsed.detect_pruefidentifikator().ok().map(|p| p.as_u32()),
        empfaenger: pending.recipient.to_string(),
        formatversion: profile.and_then(|p| p.valid_from()).map(|d| {
            format!(
                "FV{:04}-{:02}-{:02}",
                d.year(),
                u8::from(d.month()),
                d.day()
            )
        }),
        ahb_version: profile.map(|p| p.ahb_version().to_owned()),
        wire: latin1(&check.bytes),
        segmente,
        befunde: lose,
    })
}

/// The index of the segment `issue` sits on: its one-based position when it
/// names one whose tag agrees, else the segment its byte span falls in.
fn platz(
    issue: &edifact_rs::ValidationIssue,
    segments: &[edifact_rs::OwnedSegment],
) -> Option<usize> {
    let tag_agrees = |i: usize| {
        issue
            .segment_tag
            .as_deref()
            .is_none_or(|t| segments[i].tag == t)
    };
    if let Some(p) = issue.segment_position
        && let Some(i) = usize::try_from(p).ok().and_then(|p| p.checked_sub(1))
        && i < segments.len()
        && tag_agrees(i)
    {
        return Some(i);
    }
    let at = issue.span?.start;
    segments
        .iter()
        .position(|s| s.span.start <= at && at < s.span.end && s.span.end > 0)
        .filter(|&i| tag_agrees(i))
}

/// A data element as written: its components joined by `:`, trailing empty
/// components dropped as the wire drops them.
fn element_text(el: &edifact_rs::Element<'_>) -> String {
    let mut parts: Vec<&str> = (0..).map_while(|i| el.get_component(i)).collect();
    while parts.last().is_some_and(|p| p.is_empty()) {
        parts.pop();
    }
    parts.join(":")
}

/// UNOC is ISO 8859-1: one byte, one character.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

/// `POST /api/v1/probe/{command}` — rehearse a command.
///
/// `200` carries the verdict whether the message passes or not: the rehearsal
/// ran. A refusal *to rehearse* — an unknown command, one whose door cannot be
/// stopped before it writes, a Marktrolle the deployment does not declare — is
/// a `422` naming it. No Idempotency-Key: nothing is written, so a repeat is
/// another rehearsal.
#[utoipa::path(
    post,
    path = "/api/v1/probe/{command}",
    tag = "commands",
    params(("command" = String, Path, description = "Dotted command name, e.g. `gpke.lieferbeginn.anmelden`")),
    request_body(content = ProbeRequest, description = "The command's Marktrolle and payload", content_type = "application/json"),
    responses(
        (status = 200, description = "The rehearsal ran: the verdict and the message the door built", body = Probe),
        (status = 400, description = "Malformed request body"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Cedar policy denied RehearseCommand"),
        (status = 422, description = "Unknown command, a command that cannot be rehearsed, or a Marktrolle the deployment does not declare"),
    ),
    security(
        (),
        ("bearer_token" = [])
    )
)]
pub(crate) async fn handle_probe(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<CommandsApiState>>,
    axum::extract::Path(command): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    mako_service::Json(body): mako_service::Json<ProbeRequest>,
) -> mako_service::ApiResult<mako_service::Json<Probe>> {
    use mako_service::ApiError;

    let Some(identity) = state.cedar.authenticate(&headers) else {
        return Err(ApiError::Unauthorized);
    };
    let command = command.to_lowercase();
    let refused = |r: ProbeRefusal| match r {
        ProbeRefusal::Engine(e) => {
            ApiError::internal(anyhow::anyhow!("rehearsal of {command}: {e}"))
        }
        r => ApiError::unprocessable(r.sentence()).with_code(r.code()),
    };
    let marktrolle = zulassen(&state, &command, body.marktrolle.as_deref()).map_err(&refused)?;
    if !state.cedar.authorize_rehearsal(
        &identity,
        &crate::cedar_authz::CommandResource {
            name: &command,
            marktrolle: &marktrolle,
            pid: super::command_primary_pid(&command)
                .map_or(0, mako_engine::types::Pruefidentifikator::as_u32),
            tenant: &state.tenant_id.to_string(),
        },
    ) {
        return Err(ApiError::forbidden(
            "Cedar policy denied RehearseCommand for this command",
        ));
    }
    rehearse(&state, &command, marktrolle, &body.payload)
        .await
        .map(mako_service::Json)
        .map_err(refused)
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
