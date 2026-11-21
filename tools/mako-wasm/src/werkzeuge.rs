//! The validator's three small tools: an Antwortcode explained by the
//! Entscheidungsbaum that publishes it (`mako-pruefung`), a process's
//! Antwortfrist (`mako-fristen`), and an identifier's check digit (`rubo4e`).
//! Each answers from the crate makod uses; none restates a rule.

use mako_pruefung::codes::CODELISTEN;
use rubo4e::identifiers::{EicCode, MaloId, MarktpartnerId, MeloId};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::refused;

/// How many matches a search answers with: a list to pick from, not a dump.
const MAX_TREFFER: usize = 30;

/// `codes`: a code inside its tree, every tree that publishes a code, or a
/// tree's codes — whichever the request names.
pub(crate) fn codes(ebd: Option<&str>, code: Option<&str>) -> Value {
    let ebd = ebd
        .map(|e| e.trim().to_uppercase())
        .filter(|e| !e.is_empty());
    let code = code
        .map(|c| c.trim().to_uppercase())
        .filter(|c| !c.is_empty());
    let eintrag = |tree: &str, c: &mako_pruefung::codes::AntwortCode| {
        json!({
            "ebd": tree,
            "code": c.code,
            "art": c.cluster.label(),
            "zustimmung": c.ist_zustimmung(),
            "bedeutung": c.bedeutung,
            "bemerkung_noetig": c.braucht_bemerkung,
        })
    };
    let treffer: Vec<Value> = CODELISTEN
        .iter()
        .filter(|(tree, _)| ebd.as_deref().is_none_or(|e| *tree == e))
        .flat_map(|(tree, codes)| {
            codes
                .iter()
                .filter(|c| code.as_deref().is_none_or(|k| c.code == k))
                .map(move |c| eintrag(tree, c))
        })
        .collect();
    if ebd.is_some() && !CODELISTEN.iter().any(|(t, _)| Some(*t) == ebd.as_deref()) {
        return refused(
            "ebd_unbekannt",
            "Diesen Entscheidungsbaum kennt mako nicht.",
        );
    }
    json!({ "ok": {
        "baeume": CODELISTEN.iter().map(|(t, c)| json!({ "ebd": t, "codes": c.len() })).collect::<Vec<_>>(),
        "treffer": treffer,
    } })
}

/// `frist`: the answer obligations whose PID or name matches `suche`, and —
/// for one exact PID and an arrival time — when its answer is due.
pub(crate) fn frist(suche: &str, empfangen: Option<OffsetDateTime>) -> Value {
    let suche = suche.trim().to_lowercase();
    let pid: Option<u32> = suche.parse().ok();
    let treffer: Vec<Value> = mako_fristen::antwort::all()
        .filter(|o| {
            suche.is_empty()
                || o.trigger_pid.to_string().starts_with(&suche)
                || o.name.to_lowercase().contains(&suche)
        })
        .take(MAX_TREFFER)
        .map(|o| {
            let faellig = (pid == Some(o.trigger_pid))
                .then_some(())
                .and(empfangen)
                .and_then(|t| mako_fristen::antwort::antwortfrist(o.trigger_pid, t))
                .and_then(|f| {
                    f.due_at
                        .to_offset(time::UtcOffset::UTC)
                        .replace_nanosecond(0)
                        .ok()?
                        .format(&Rfc3339)
                        .ok()
                });
            json!({
                "pid": o.trigger_pid,
                "name": o.name,
                "durch": o.answered_by,
                "antworten": [o.antwort_pids.0, o.antwort_pids.1],
                "entscheidungsbaum": o.ebd,
                "quelle": o.source,
                "faellig": faellig,
            })
        })
        .collect();
    json!({ "ok": { "treffer": treffer } })
}

/// `id`: what an identifier is, and whether its check digit holds; for a
/// wrong one, the digit that would.
pub(crate) fn id(wert: &str) -> Value {
    let w: String = wert.chars().filter(|c| !c.is_whitespace()).collect();
    if w.is_empty() {
        return refused("eingabe_leer", "Gib eine ID ein.");
    }
    let alle_ziffern = w.chars().all(|c| c.is_ascii_digit());
    let antwort = if alle_ziffern && w.len() == 11 {
        // Marktlokations-ID: Lok- und Waggon-Kennzeichnungsverfahren.
        let richtig = MaloId::check_digit(&w[..10]).ok();
        json!({
            "art": "malo",
            "gueltig": MaloId::new(&w).is_ok(),
            "pruefziffer": richtig.map(|d| d.to_string()),
        })
    } else if alle_ziffern && w.len() == 13 {
        let mp = MarktpartnerId::new(&w);
        let herkunft = if w.starts_with("99") {
            "bdew"
        } else if w.starts_with("98") {
            "dvgw"
        } else {
            "gln"
        };
        json!({
            "art": "mpid",
            "herkunft": herkunft,
            "bdew_pruefziffer": mp.as_ref().is_ok_and(MarktpartnerId::has_valid_bdew_check_digit),
            "gln_pruefziffer": mp.as_ref().is_ok_and(MarktpartnerId::has_valid_gln_check_digit),
        })
    } else if w.len() == 16 {
        let richtig = EicCode::compute_check_char(&w[..15]);
        json!({
            "art": "eic",
            "gueltig": EicCode::new(&w).is_ok(),
            "pruefziffer": richtig.map(|c| c.to_string()),
        })
    } else if w.len() == 33 {
        json!({ "art": "melo", "gueltig": MeloId::new(&w).is_ok() })
    } else {
        return refused(
            "id_unbekannt",
            "Keine bekannte ID-Form: MaLo-ID (11 Ziffern), MP-ID (13 Ziffern), EIC (16 Zeichen) oder MeLo-ID (33 Zeichen).",
        );
    };
    json!({ "ok": antwort })
}
