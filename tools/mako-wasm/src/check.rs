//! `check`: every message of an interchange — named as its MIG and AHB name
//! it, its findings on the segments they concern, and the acknowledgement
//! deadlines from `mako-fristen`.

use std::collections::BTreeSet;

use edi_energy::profile::structure::Kind;
use edi_energy::{
    AnyMessage, EdiEnergyMessage, MessageType, Profile, ReleaseRegistry, ReleaseTrack, RuleOrigin,
};
use edifact::{OwnedSegment, ValidationIssue, ValidationSeverity};
use mako_fristen::ContrlAnlass;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::{MAX_INPUT_BYTES, refused};

/// Check `text`; the deadlines count from `empfangen` (RFC 3339).
pub(crate) fn check(text: &str, empfangen: Option<&str>) -> Value {
    if text.len() > MAX_INPUT_BYTES {
        return refused(
            "eingabe_zu_gross",
            format!(
                "Die Nachricht ist größer als {} MB.",
                MAX_INPUT_BYTES / 1024 / 1024
            ),
        );
    }
    if text.trim().is_empty() {
        return refused("eingabe_leer", "Füge eine EDIFACT-Nachricht ein.");
    }
    let empfangen = match empfangen.map(|e| OffsetDateTime::parse(e, &Rfc3339)) {
        None => None,
        Some(Ok(t)) => Some(t),
        Some(Err(_)) => {
            return refused(
                "empfangen_ungueltig",
                "Der Eingangszeitpunkt ist kein RFC-3339-Zeitpunkt.",
            );
        }
    };
    let wire = Wire::new(text);
    let mut nachrichten = Vec::new();
    for parsed in edi_energy::parse_interchange(wire.bytes.as_slice()) {
        match parsed {
            Ok(msg) => nachrichten.push(nachricht(&msg, empfangen, &wire)),
            Err(e) if nachrichten.is_empty() => {
                return refused("keine_edifact_nachricht", e.to_string());
            }
            Err(e) => {
                nachrichten.push(json!({ "fehler": e.to_string() }));
                break;
            }
        }
    }
    if nachrichten.is_empty() {
        return refused(
            "keine_edifact_nachricht",
            "Die Eingabe enthält keine Nachricht (UNH … UNT).",
        );
    }
    json!({ "ok": { "nachrichten": nachrichten } })
}

/// The bytes the validator reads, and the way back from a byte offset to the
/// character offset the page highlights.
///
/// A message whose every character fits ISO 8859-1 is checked as those bytes,
/// as UNOC carries it on the wire; anything else as UTF-8.
struct Wire {
    bytes: Vec<u8>,
    latin1: bool,
    text: String,
}

impl Wire {
    fn new(text: &str) -> Self {
        let latin1: Option<Vec<u8>> = text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).ok())
            .collect();
        match latin1 {
            Some(bytes) => Self {
                bytes,
                latin1: true,
                text: String::new(),
            },
            None => Self {
                bytes: text.as_bytes().to_vec(),
                latin1: false,
                text: text.to_owned(),
            },
        }
    }

    /// The character offset of byte offset `b`.
    fn at(&self, b: usize) -> usize {
        if self.latin1 {
            b
        } else {
            self.text.get(..b).map_or(b, |s| s.chars().count())
        }
    }
}

/// One message, checked.
fn nachricht(msg: &AnyMessage, empfangen: Option<OffsetDateTime>, wire: &Wire) -> Value {
    let typ = msg.try_message_type();
    let release = msg.detect_release().ok();
    let profile = typ
        .zip(release)
        .and_then(|(t, r)| ReleaseRegistry::global().profile(t, r).ok());
    let pid = msg.detect_pruefidentifikator().ok().map(|p| p.as_u32());
    let af = profile.zip(pid).and_then(|(p, pid)| p.anwendungsfall(pid));
    let segments = msg.segments();

    let report = msg.validate();
    let gueltig = report.as_ref().is_ok_and(|r| r.is_valid());
    let befunde: Vec<Value> = match &report {
        Ok(r) => r
            .iter_issues()
            .map(|i| befund(i, segments, profile, wire))
            .collect(),
        Err(e) => vec![json!({
            "schwere": "fehler",
            "herkunft": "Prüfung",
            "text": e.to_string(),
        })],
    };

    let antworten = pid.and_then(edi_energy::answer_pids).map(|(ja, nein)| {
        [ja, nein]
            .iter()
            .map(|p| {
                json!({
                    "pid": p,
                    "name": profile.and_then(|pr| pr.anwendungsfall(*p)).map(|a| a.name.as_str()),
                })
            })
            .collect::<Vec<_>>()
    });

    json!({
        "typ": typ.map(|t| t.as_str()),
        "pid": pid,
        "anwendungsfall": af.map(|a| a.name.as_str()),
        "kommunikation": af.and_then(|a| a.communication.as_deref()),
        "kapitel": af.and_then(|a| a.chapter.as_deref()),
        "formatversion": release.map(|r| r.as_str()),
        "gilt_ab": profile.and_then(Profile::valid_from).map(|d| d.to_string()),
        "gilt_bis": profile.and_then(Profile::valid_until).map(|d| d.to_string()),
        "ahb": profile.map(Profile::ahb_version),
        "absender": msg.nad_sender(),
        "empfaenger": msg.nad_receiver(),
        "nachrichtenreferenz": msg.message_ref(),
        "gueltig": gueltig,
        "sparte": ist_gas(release.map(|r| r.track()), msg.nad_sender()).map(|g| if g { "Gas" } else { "Strom" }),
        "aperak": aperak(typ, ist_gas(release.map(|r| r.track()), msg.nad_sender()), gueltig),
        "segmente": segmente(segments, profile, af, wire),
        "befunde": befunde,
        "antworten": antworten,
        "route": pid.and_then(route),
        "antwort": pid.and_then(|p| antwort(p, empfangen)),
        "fristen": empfangen.map(|t| fristen(typ, release.map(|r| r.track()), pid, msg.nad_sender(), t)),
    })
}

/// Each segment with its MIG name and the meaning of every value in it.
fn segmente(
    segments: &[OwnedSegment],
    profile: Option<&Profile>,
    af: Option<&edi_energy::profile::model::Anwendungsfall>,
    wire: &Wire,
) -> Vec<Value> {
    let resolution = profile.map(|p| p.resolve(segments));
    segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let place = profile.zip(resolution.as_ref()).and_then(|(p, r)| {
                let node = r.assigned.get(i).copied().flatten()?.node;
                let n = &p.structure.nodes[node];
                match &n.kind {
                    Kind::Segment { nr, layout, .. } => {
                        Some((n, nr.as_str(), &p.structure.layouts[layout.to_owned()]))
                    }
                    Kind::Group { .. } => None,
                }
            });
            let werte = place
                .map(|(_, _, layout)| werte(seg, layout, wire))
                .unwrap_or_default();
            json!({
                "start": wire.at(seg.span.start),
                "end": wire.at(seg.span.end),
                "tag": seg.tag,
                "name": place.map(|(n, _, _)| n.name.as_str()),
                "status": place.and_then(|(_, nr, _)| af.and_then(|a| a.segment_status(nr))),
                "werte": werte,
            })
        })
        .collect()
}

/// The values of `seg`, each named by the MIG layout and, when coded, with
/// the code's meaning.
fn werte(
    seg: &OwnedSegment,
    layout: &edi_energy::profile::model::SegmentNode,
    wire: &Wire,
) -> Vec<Value> {
    let mut out = Vec::new();
    for (ei, el) in seg.elements.iter().enumerate() {
        let Some(def) = layout.elements.get(ei) else {
            continue;
        };
        for (ci, value) in el.components().enumerate() {
            if value.is_empty() {
                continue;
            }
            let comp = if def.components.is_empty() {
                (ci == 0).then_some(def)
            } else {
                def.components.get(ci)
            };
            let Some(comp) = comp else { continue };
            let Some(span) = el.component_span(ci) else {
                continue;
            };
            let bedeutung = comp
                .codes
                .iter()
                .find(|c| c.code == value)
                .map(|c| c.name.as_str());
            out.push(json!({
                "start": wire.at(span.start),
                "end": wire.at(span.end),
                "de": comp.id,
                "name": comp.name,
                "bedeutung": bedeutung,
            }));
        }
    }
    out
}

/// One finding: severity, the layer it comes from, the segment it concerns,
/// and the AHB conditions its text names.
fn befund(
    issue: &ValidationIssue,
    segments: &[OwnedSegment],
    profile: Option<&Profile>,
    wire: &Wire,
) -> Value {
    let schwere = match issue.severity {
        ValidationSeverity::Critical | ValidationSeverity::Error => "fehler",
        ValidationSeverity::Warning => "warnung",
        _ => "hinweis",
    };
    // A Formatbedingung (`FMT-…`) is the AHB's [9xx] condition on a value's
    // format, which `RuleOrigin` files under no layer.
    let herkunft = match issue.rule_id.as_deref() {
        Some(r) if r.starts_with("FMT-") => "Format",
        r => r.and_then(RuleOrigin::classify).map_or("Prüfung", herkunft),
    };
    let segment = issue
        .span
        .and_then(|s| {
            segments.iter().position(|seg| {
                seg.span.start <= s.start && s.start < seg.span.end.max(seg.span.start + 1)
            })
        })
        .or_else(|| {
            // DE 0096 counts from this message's UNH, which is segment 0 here
            // unless the interchange's UNB precedes it.
            let unh = segments.iter().position(|s| s.tag == "UNH")?;
            let pos = usize::try_from(issue.segment_position?).ok()?;
            Some(unh + pos - 1).filter(|i| *i < segments.len())
        });
    let bedingungen: Vec<Value> = profile
        .map(|p| {
            bedingung_nummern(&issue.message)
                .into_iter()
                .filter_map(|n| {
                    p.ahb
                        .conditions
                        .get(&n)
                        .map(|t| json!({ "nr": n, "text": t }))
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "schwere": schwere,
        "herkunft": herkunft,
        "text": issue.message,
        "regel": issue.rule_id,
        "segment": segment,
        "start": issue.span.map(|s| wire.at(s.start)),
        "end": issue.span.map(|s| wire.at(s.end)),
        "bedingungen": bedingungen,
    })
}

/// The layer a rule comes from, as the page names it.
fn herkunft(origin: RuleOrigin) -> &'static str {
    match origin {
        RuleOrigin::Parse => "Syntax",
        RuleOrigin::Directory => "Verzeichnis",
        RuleOrigin::Mig => "MIG",
        RuleOrigin::Ahb => "AHB",
        RuleOrigin::Semantic => "Inhalt",
        _ => "Prüfung",
    }
}

/// The AHB condition numbers `[12]` a finding's text names.
fn bedingung_nummern(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let inner = &after[..close];
        if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit()) {
            out.insert(inner.to_owned());
        }
        rest = &after[close..];
    }
    out
}

/// Whether the message is Gas: the release track where it names one, else the
/// sender's code number — a DVGW-Codenummer (`98…`) is Gas, a BDEW-Codenummer
/// (`99…`) Strom (Identifikatoren AWH V1.2 Kap. 2.1).
fn ist_gas(track: Option<ReleaseTrack>, absender: Option<&str>) -> Option<bool> {
    match track {
        Some(ReleaseTrack::Gas) => Some(true),
        Some(ReleaseTrack::Strom) => Some(false),
        _ => absender.and_then(|a| {
            if a.starts_with("98") {
                Some(true)
            } else if a.starts_with("99") {
                Some(false)
            } else {
                None
            }
        }),
    }
}

/// The APERAK the receiver owes, by APERAK AHB 1.1: a Geschäftsvorfall short
/// of its Prüfschablone is refused with a Verarbeitbarkeitsfehlermeldung in
/// both Sparten (Kap. 2.1.2); a processable Strom message gets its
/// Anerkennungsmeldung (Kap. 2.4), a processable Gas message none — Gas
/// reports errors only (Kap. 2.3). An APERAK or CONTRL is answered by no
/// APERAK. `None` where the Sparte is unknown.
fn aperak(typ: Option<MessageType>, gas: Option<bool>, gueltig: bool) -> Option<Value> {
    if matches!(typ?, MessageType::Aperak | MessageType::Contrl) {
        return None;
    }
    let gas = gas?;
    let (art, quelle) = match (gueltig, gas) {
        (false, _) => ("verarbeitbarkeitsfehler", "APERAK AHB 1.1 Kap. 2.1.2"),
        (true, false) => ("anerkennung", "APERAK AHB 1.1 Kap. 2.4"),
        (true, true) => ("keine", "APERAK AHB 1.1 Kap. 2.3"),
    };
    Some(json!({ "art": art, "quelle": quelle }))
}

/// When the receiver's CONTRL and APERAK are due for a message received at
/// `t`, from `mako-fristen`, each with the chapter it rests on.
fn fristen(
    typ: Option<MessageType>,
    track: Option<ReleaseTrack>,
    pid: Option<u32>,
    absender: Option<&str>,
    t: OffsetDateTime,
) -> Vec<Value> {
    let Some(typ) = typ else { return Vec::new() };
    let gas = ist_gas(track, absender);
    let utilmd_orders = matches!(typ, MessageType::Utilmd | MessageType::Orders);
    // Whole seconds in UTC: the page renders Berlin time itself, and a
    // deadline at the end of a day („Ablauf des 3. WT") reads 23:59:59.
    let fmt = |d: OffsetDateTime| {
        d.to_offset(time::UtcOffset::UTC)
            .replace_nanosecond(0)
            .ok()
            .and_then(|d| d.format(&Rfc3339).ok())
    };
    let mut out = Vec::new();
    if typ != MessageType::Contrl {
        let (anlass, quelle) = if gas == Some(false) && utilmd_orders {
            (
                ContrlAnlass::StromUtilmdOderOrders,
                "CONTRL AHB 1.0 Kap. 2.4.1",
            )
        } else if gas == Some(true) {
            (ContrlAnlass::Regelfall, "CONTRL AHB 1.0 Kap. 2.3.1")
        } else {
            (ContrlAnlass::Regelfall, "CONTRL AHB 1.0 Kap. 2.4.1")
        };
        out.push(json!({
            "was": "CONTRL",
            "faellig": fmt(mako_fristen::contrl_due_at(t, anlass)),
            "quelle": quelle,
        }));
    }
    if !matches!(typ, MessageType::Contrl | MessageType::Aperak) {
        match gas {
            Some(false) => {
                let due = if utilmd_orders {
                    mako_fristen::aperak_strom_due_at(t)
                } else {
                    mako_fristen::aperak_strom_allgemein_due_at(t)
                };
                out.push(json!({
                    "was": "APERAK",
                    "faellig": fmt(due),
                    "quelle": "APERAK AHB 1.1 Kap. 2.4.1",
                }));
            }
            Some(true) => {
                if let Some(pid) = pid {
                    let (label, due) = mako_fristen::aperak_gas_due_at(pid, t);
                    out.push(json!({
                        "was": "APERAK",
                        "faellig": fmt(due),
                        "quelle": "APERAK AHB 1.1 Kap. 2.3.1",
                        "fall": if label == mako_fristen::APERAK_GAS_FOLGEPROZESS_LABEL { "Folgeprozess" } else { "Initialprozess" },
                    }));
                }
            }
            None => {}
        }
    }
    out
}

/// The PID reference's routing column — the crate and workflow makod hands a
/// PID to. Embedded rather than restated: `cargo xtask` holds that column to
/// makod's `PidRouter`, so this is the router's own answer.
const PID_REFERENCE: &str = include_str!("../../../site/content/docs/regulatory/pid-reference.md");

/// The crate and workflow `pid` is routed to; `None` for a PID no workflow
/// registers.
fn route(pid: u32) -> Option<Value> {
    let key = format!("| {pid} |");
    let row = PID_REFERENCE.lines().find(|l| l.starts_with(&key))?;
    let last = row.trim_end_matches('|').rsplit('|').next()?;
    let mut ticks = last.split('`').skip(1).step_by(2);
    let (krate, workflow) = (ticks.next()?, ticks.next()?);
    Some(json!({ "crate": krate, "workflow": workflow }))
}

/// The answer obligation `pid` starts, from `mako-fristen`: what the process
/// is, who owes the answer, the Entscheidungsbaum that decides it, and — given
/// an arrival time — by when, with its source.
fn antwort(pid: u32, empfangen: Option<OffsetDateTime>) -> Option<Value> {
    let o = mako_fristen::antwort::antwort_obligation(pid)?;
    let faellig = empfangen
        .and_then(|t| mako_fristen::antwort::antwortfrist(pid, t))
        .and_then(|f| {
            f.due_at
                .to_offset(time::UtcOffset::UTC)
                .replace_nanosecond(0)
                .ok()?
                .format(&Rfc3339)
                .ok()
        });
    Some(json!({
        "name": o.name,
        "durch": o.answered_by,
        "pids": [o.antwort_pids.0, o.antwort_pids.1],
        "entscheidungsbaum": o.ebd,
        "faellig": faellig,
        "quelle": o.source,
    }))
}
