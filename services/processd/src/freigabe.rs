//! The typed decision view over one approval-queue entry — what the operator
//! Desk shows a person deciding a „Freigabe".
//!
//! [`ApprovalQueueEntry`](crate::pg::approval::ApprovalQueueEntry) carries
//! the `makod` command bodies an approval or a refusal is dispatched with; this
//! view carries what a person needs to decide and nothing that is only
//! plumbing. Every Antwortcode in it is read inside
//! the Entscheidungsbaum that publishes it, with that tree's own wording
//! ([`mako_pruefung::codes::lookup`]): a bare code means nothing, `A02` is four
//! unrelated refusals across `E_0607`, `E_0622`, `E_0249` and `E_0250`.

use rmcp::schemars;
use schemars::JsonSchema;
use serde::Serialize;

use crate::pg::approval::{ApprovalQueueEntry, QueueStatus, fehlende_fakten};
use mako_pruefung::codes::{self, Cluster};

/// Whether the answer is already decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FreigabeArt {
    /// The module decided the answer and froze it; approving dispatches exactly
    /// [`Freigabe::antwort`], and a code stated beside it is refused.
    Gehalten,
    /// No answer was decided: the person states it, from
    /// [`Freigabe::codes_freigeben`] or [`Freigabe::codes_ablehnen`].
    Eskaliert,
}

/// One code with the BDEW's own wording for it in its tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CodeBedeutung {
    /// DE 9013 — the Antwortcode.
    pub code: String,
    /// The wording the Entscheidungsbaum prints beside it.
    pub bedeutung: String,
}

/// The codes an answer of one cluster may carry, and the tree they come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct CodeAuswahl {
    /// The Entscheidungsbaum (`E_0623`, …); state it as `antwort_tree`.
    pub ebd: String,
    /// Every code of that tree in the cluster, in the catalogue's order.
    pub codes: Vec<CodeBedeutung>,
}

/// The decided answer a held entry is approved with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Antwort {
    /// DE 9013 — the frozen Antwortcode.
    pub code: String,
    /// The tree the code was resolved in, when one is known.
    pub ebd: Option<String>,
    /// The BDEW's wording for `code` in `ebd`; `None` when the tree does not
    /// publish it — never a guess from another tree.
    pub bedeutung: Option<String>,
}

/// One approval-queue entry, as a person deciding it sees it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Freigabe {
    /// The queue entry — what approve / reject address.
    pub id: String,
    /// The market process the decision answers.
    pub process_id: String,
    /// BDEW Prüfidentifikator of the inbound message.
    pub pid: i32,
    /// The Marktlokation (the location for an ESA order), where there is one.
    pub malo_id: Option<String>,
    /// The Marktrolle the answer is dispatched under (`NB`, `LF`, `MSB`).
    pub marktrolle: Option<String>,
    /// Why the entry needs a person, with the Antwortfrist it runs against.
    pub grund: String,
    /// Held answer or escalation.
    pub art: FreigabeArt,
    /// The frozen answer of a held entry.
    pub antwort: Option<Antwort>,
    /// The codes approving is stated with — an escalation's Zustimmungen.
    /// `None` when approving needs no code: a held entry goes as frozen.
    pub codes_freigeben: Option<CodeAuswahl>,
    /// The codes rejecting is stated with: the Ablehnungen, or — overriding a
    /// held Ablehnung — the Zustimmungen, because rejecting a held answer
    /// sends the other cluster. `None` when the refusal carries no code.
    pub codes_ablehnen: Option<CodeAuswahl>,
    /// The facts approving states beside the code, where the entry's own did
    /// not resolve — the Bilanzkreis, Versorgungsart, Abrechnungsintervall or
    /// Grundlage nach EnFG of an E/G Zustimmung. Empty when there are none to
    /// state.
    pub fakten_freigeben: Vec<Fakt>,
    /// The same for rejecting — a held E/G Ablehnung is overridden by the
    /// Bestätigung.
    pub fakten_ablehnen: Vec<Fakt>,
    /// Approving sends a market message.
    pub kann_freigeben: bool,
    /// Rejecting sends a market message.
    pub kann_ablehnen: bool,
    pub status: QueueStatus,
    /// The business Antwortfrist less an hour of headroom, RFC 3339 in UTC.
    #[schemars(extend("format" = "date-time"))]
    pub expires_at: String,
    /// RFC 3339 in UTC.
    #[schemars(extend("format" = "date-time"))]
    pub created_at: String,
    /// `sub` of the principal who decided.
    pub decided_by: Option<String>,
    /// RFC 3339 in UTC.
    #[schemars(extend("format" = "date-time"))]
    pub decided_at: Option<String>,
}

/// A fact a decision states beside its code, under the body key `feld`.
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct Fakt {
    /// The decision body's key: `bilanzkreis`, `versorgungsart`,
    /// `abrechnungsintervall_monate` or `enfg`.
    pub feld: String,
    /// What it is, with the segment it rides.
    pub bezeichnung: String,
    /// The values the segment admits; empty where the value is free text (a
    /// Bilanzkreis is the LF's own code).
    pub werte: Vec<FaktWert>,
}

/// One admissible value of a [`Fakt`].
#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct FaktWert {
    /// What the decision sends — a code, or a number written as text.
    pub wert: String,
    pub bedeutung: String,
}

/// The facts `command` still needs over `frozen`, described for the view.
fn fakten(command: Option<&str>, frozen: Option<&serde_json::Value>) -> Vec<Fakt> {
    let Some(command) = command else {
        return Vec::new();
    };
    fehlende_fakten(command, frozen)
        .into_iter()
        .map(|feld| {
            let wert = |wert: &str, bedeutung: &str| FaktWert {
                wert: wert.to_owned(),
                bedeutung: bedeutung.to_owned(),
            };
            let (bezeichnung, werte) = match feld {
                "versorgungsart" => (
                    "Versorgungsart (SG10 CCI+Z36)",
                    vec![
                        wert("ZD0", "Grundversorgung (§ 36 EnWG)"),
                        wert("ZC9", "Ersatzversorgung (§ 38 EnWG)"),
                        wert("ZE3", "vertragliche Ersatzbelieferung"),
                    ],
                ),
                "abrechnungsintervall_monate" => (
                    "Abrechnungsintervall des LF in Monaten (SG6 DTM+Z20)",
                    mako_markt::commands::ABRECHNUNGSINTERVALLE_MONATE
                        .iter()
                        .map(|n| {
                            let text = if *n == 1 {
                                "1 Monat".to_owned()
                            } else {
                                format!("{n} Monate")
                            };
                            wert(&n.to_string(), &text)
                        })
                        .collect(),
                ),
                "enfg" => (
                    "Grundlage zur Verringerung der Umlagen nach EnFG (SG8 SEQ+Z75 CCI+Z61; \
                     beside ZF9 the Grund der Privilegierung, SG10 CAV)",
                    crate::pg::approval::enfg_werte()
                        .iter()
                        .map(|(w, b)| wert(w, b))
                        .collect(),
                ),
                _ => ("Bilanzkreis des LF", Vec::new()),
            };
            Fakt {
                feld: feld.to_owned(),
                bezeichnung: bezeichnung.to_owned(),
                werte,
            }
        })
        .collect()
}

/// `list_pending_approvals`' answer.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Freigaben {
    /// The earliest `expires_at` first.
    pub freigaben: Vec<Freigabe>,
    /// How many entries `freigaben` holds.
    pub count: usize,
    /// At least `limit` entries matched: more may be waiting than were listed.
    pub saturated: bool,
}

fn rfc3339(t: time::OffsetDateTime) -> String {
    use time::format_description::well_known::Rfc3339;
    t.format(&Rfc3339).unwrap_or_default()
}

fn text<'a>(body: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    body.get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
}

/// The frozen Antwortcode of a body: `antwort_code`, or `antwortcode` on a WiM
/// MSB-Wechsel answer (the key `makod`'s `dispatch_wim_antwort` reads first).
fn frozen_code(body: &serde_json::Value) -> Option<&str> {
    text(body, "antwort_code").or_else(|| text(body, "antwortcode"))
}

/// Every code of `ebd` in `cluster`, or `None` when the tree publishes none.
fn auswahl(ebd: &str, cluster: Cluster) -> Option<CodeAuswahl> {
    let codes: Vec<CodeBedeutung> = codes::codes_im_cluster(ebd, cluster)
        .map(|c| CodeBedeutung {
            code: c.code.to_owned(),
            bedeutung: c.bedeutung.to_owned(),
        })
        .collect();
    (!codes.is_empty()).then(|| CodeAuswahl {
        ebd: ebd.to_owned(),
        codes,
    })
}

/// The tree an operator's Bestätigung draws from, given the tree the module
/// decided in: a Vorprüfung (`E_0622` / `E_3005`) publishes Ablehnungen only
/// and hands a surviving message to its Zustimmungs-tree.
#[must_use]
pub fn zustimmungsbaum(ebd: &str) -> &str {
    codes::zustimmung_tree_of(ebd).unwrap_or(ebd)
}

/// The tree and cluster the code of a decision must come from — `approve` or
/// reject — or `None` where that decision states no code. The one rule the
/// view offers codes by and [`crate::pg::approval::check_operator_code`]
/// refuses by, so the Desk can never offer a code processd would refuse.
#[must_use]
pub fn erwarteter_code(entry: &ApprovalQueueEntry, approve: bool) -> Option<(&str, Cluster)> {
    let ebd = entry.ablehnung_ebd.as_deref()?;
    let zustimmung = match (Freigabe::held_answer(entry), approve) {
        (None, approve) => approve,
        // Frozen: approving sends exactly the held answer.
        (Some(_), true) => return None,
        // Overriding a held answer states the other cluster.
        (Some(antwort), false) => !antwort
            .ebd
            .as_deref()
            .and_then(|t| codes::lookup(t, &antwort.code))
            .and_then(mako_pruefung::AntwortCode::ist_zustimmung)?,
    };
    Some(if zustimmung {
        (zustimmungsbaum(ebd), Cluster::Zustimmung)
    } else {
        (ebd, Cluster::Ablehnung)
    })
}

impl Freigabe {
    /// Whether `entry` holds a decided answer.
    ///
    /// The frozen approval body is not enough on its own: the MSB-Wechsel, the
    /// Steuerungsauftrag and the REQOTE Preisanfrage freeze only what the
    /// answer is *addressed* by (`melo_id`, `tx_id`) so it can be dispatched
    /// at all. A held answer is one whose frozen body carries the decision —
    /// its Antwortcode, or the Begründung of a refusal that has no code (the
    /// ESA Werteanfrage, `reason`).
    #[must_use]
    pub fn art_von(entry: &ApprovalQueueEntry) -> FreigabeArt {
        match &entry.answer_payload {
            Some(body) if frozen_code(body).is_some() || text(body, "reason").is_some() => {
                FreigabeArt::Gehalten
            }
            _ => FreigabeArt::Eskaliert,
        }
    }

    /// The frozen answer of a held entry, read inside its tree.
    fn held_answer(entry: &ApprovalQueueEntry) -> Option<Antwort> {
        let body = entry.answer_payload.as_ref()?;
        let code = frozen_code(body)?;
        // The tree the frozen body names, else the one the module decided
        // in — the answer was drawn from it (the LF and ESA bodies name the
        // wire Codeliste or nothing, the MSB body nothing at all).
        let tree = text(body, "antwort_tree").or(entry.ablehnung_ebd.as_deref());
        Some(Antwort {
            code: code.to_owned(),
            ebd: tree.map(ToOwned::to_owned),
            bedeutung: tree
                .and_then(|t| codes::lookup(t, code))
                .map(|c| c.bedeutung.to_owned()),
        })
    }

    /// The decision view of `entry`.
    #[must_use]
    pub fn from_entry(entry: &ApprovalQueueEntry) -> Self {
        let art = Self::art_von(entry);
        let antwort = Self::held_answer(entry);
        Self {
            id: entry.id.to_string(),
            process_id: entry.process_id.to_string(),
            pid: entry.pid,
            malo_id: entry.malo_id.clone(),
            marktrolle: entry.marktrolle.clone(),
            grund: entry.reason.clone(),
            art,
            antwort,
            codes_freigeben: entry
                .approve_command
                .as_ref()
                .and_then(|_| erwarteter_code(entry, true))
                .and_then(|(tree, cluster)| auswahl(tree, cluster)),
            codes_ablehnen: entry
                .reject_command
                .as_ref()
                .and_then(|_| erwarteter_code(entry, false))
                .and_then(|(tree, cluster)| auswahl(tree, cluster)),
            fakten_freigeben: fakten(
                entry.approve_command.as_deref(),
                entry.answer_payload.as_ref(),
            ),
            fakten_ablehnen: fakten(
                entry.reject_command.as_deref(),
                entry.reject_payload.as_ref(),
            ),
            kann_freigeben: entry.approve_command.is_some(),
            kann_ablehnen: entry.reject_command.is_some(),
            status: entry.status,
            expires_at: rfc3339(entry.expires_at),
            created_at: rfc3339(entry.created_at),
            decided_by: entry.decided_by.clone(),
            decided_at: entry.decided_at.map(rfc3339),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_pruefung::codes::{EBD_ANMELDUNG_DIREKT_ABLEHNBAR, EBD_LIEFERBEGINN};
    use uuid::Uuid;

    fn entry() -> ApprovalQueueEntry {
        ApprovalQueueEntry::pending(
            Uuid::nil(),
            55_001,
            Some("51238696012".into()),
            "test".into(),
            time::macros::datetime!(2026-07-09 09:00 UTC),
            "t".into(),
        )
        .with_commands(
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
            Some("NB"),
        )
    }

    /// A held Accept shows the code it was frozen with and the BDEW's wording
    /// for it in its own tree, and offers no Zustimmungscodes.
    #[test]
    fn a_held_entry_is_gehalten_with_its_frozen_code_and_wording() {
        let e = entry()
            .with_ebd(EBD_LIEFERBEGINN)
            .with_answer_payload(serde_json::json!({
                "antwort_code": "A51", "antwort_tree": EBD_LIEFERBEGINN, "zustimmung": true,
            }));
        let f = Freigabe::from_entry(&e);
        assert_eq!(f.art, FreigabeArt::Gehalten);
        let antwort = f.antwort.expect("a held entry shows its answer");
        assert_eq!(antwort.code, "A51");
        assert_eq!(antwort.ebd.as_deref(), Some(EBD_LIEFERBEGINN));
        assert_eq!(
            antwort.bedeutung.as_deref(),
            Some(
                codes::lookup(EBD_LIEFERBEGINN, "A51")
                    .expect("E_0623 publishes A51")
                    .bedeutung
            )
        );
        assert!(f.codes_freigeben.is_none(), "approved as frozen");
        assert_eq!(f.marktrolle.as_deref(), Some("NB"));
        assert_eq!(f.expires_at, "2026-07-09T09:00:00Z");
    }

    /// An escalation carries no answer; the person picks one from the tree's
    /// two clusters — Ablehnungen from the Vorprüfung it stopped in, the
    /// Zustimmung from the tree the Vorprüfung hands over to.
    #[test]
    fn an_escalated_entry_is_eskaliert_with_no_antwort() {
        let f = Freigabe::from_entry(&entry().with_ebd(EBD_ANMELDUNG_DIREKT_ABLEHNBAR));
        assert_eq!(f.art, FreigabeArt::Eskaliert);
        assert!(f.antwort.is_none());
        assert!(f.kann_freigeben && f.kann_ablehnen);
        let zustimmung = f
            .codes_freigeben
            .expect("an escalation is approved with a code");
        assert_eq!(zustimmung.ebd, EBD_LIEFERBEGINN);
        assert!(zustimmung.codes.iter().all(|c| {
            codes::lookup(EBD_LIEFERBEGINN, &c.code)
                .is_some_and(|a| a.cluster == Cluster::Zustimmung)
        }));
    }

    /// `ablehnung.codes` are exactly the tree's Ablehnungs-Cluster, with the
    /// tree's own wording.
    #[test]
    fn ablehnung_codes_are_exactly_the_trees_ablehnung_codes() {
        let f = Freigabe::from_entry(&entry().with_ebd(EBD_ANMELDUNG_DIREKT_ABLEHNBAR));
        let ablehnung = f.codes_ablehnen.expect("E_0622 publishes Ablehnungen");
        assert_eq!(ablehnung.ebd, EBD_ANMELDUNG_DIREKT_ABLEHNBAR);
        let want: Vec<(String, String)> = codes::E_0622_CODES
            .iter()
            .filter(|c| c.cluster == Cluster::Ablehnung)
            .map(|c| (c.code.to_owned(), c.bedeutung.to_owned()))
            .collect();
        let got: Vec<(String, String)> = ablehnung
            .codes
            .into_iter()
            .map(|c| (c.code, c.bedeutung))
            .collect();
        assert_eq!(got, want);
    }

    /// A body that only addresses the answer is not a held answer.
    #[test]
    fn an_address_only_body_is_an_escalation() {
        let msb = ApprovalQueueEntry::pending(
            Uuid::nil(),
            55_042,
            None,
            "x".into(),
            time::OffsetDateTime::UNIX_EPOCH,
            "t".into(),
        )
        .with_commands(
            mako_markt::commands::WIM_GERAETEWECHSEL_BESTAETIGEN,
            mako_markt::commands::WIM_GERAETEWECHSEL_ABLEHNEN,
            Some("NB"),
        )
        .with_answer_payload(serde_json::json!({ "melo_id": "DE0001234567890123456789012345678" }));
        assert_eq!(Freigabe::art_von(&msb), FreigabeArt::Eskaliert);
        let held = msb.with_answer_payload(serde_json::json!({
            "melo_id": "DE0001234567890123456789012345678", "antwortcode": "E15",
        }));
        assert_eq!(Freigabe::art_von(&held), FreigabeArt::Gehalten);
        // No tree recorded, no codes offered — never a guess.
        let f = Freigabe::from_entry(&held);
        assert!(f.codes_ablehnen.is_none());
        assert_eq!(f.antwort.and_then(|a| a.ebd), None);
    }

    /// Rejecting a held answer sends the other cluster, so the codes offered
    /// for „Ablehnen" are the Zustimmungen when the held answer is a refusal
    /// — the same rule processd checks the stated code against.
    #[test]
    fn overriding_a_held_ablehnung_offers_the_zustimmungen() {
        let held_ablehnung = codes::codes_im_cluster(EBD_LIEFERBEGINN, Cluster::Ablehnung)
            .next()
            .expect("E_0623 publishes Ablehnungen")
            .code;
        let e = entry()
            .with_ebd(EBD_LIEFERBEGINN)
            .with_answer_payload(serde_json::json!({
                "antwort_code": held_ablehnung, "antwort_tree": EBD_LIEFERBEGINN,
            }));
        let f = Freigabe::from_entry(&e);
        assert!(f.codes_freigeben.is_none(), "approved as frozen");
        let ablehnen = f.codes_ablehnen.expect("the override states a code");
        let zustimmungen: Vec<&str> =
            codes::codes_im_cluster(zustimmungsbaum(EBD_LIEFERBEGINN), Cluster::Zustimmung)
                .map(|c| c.code)
                .collect();
        assert_eq!(
            ablehnen
                .codes
                .iter()
                .map(|c| c.code.as_str())
                .collect::<Vec<_>>(),
            zustimmungen
        );
        // And processd accepts exactly what the view offers.
        let first = ablehnen.codes[0].code.clone();
        let operator = crate::pg::approval::OperatorAnswer {
            antwort_code: Some(first),
            antwort_tree: Some(ablehnen.ebd.clone()),
            antwort_codeliste: None,
            bemerkung: None,
            ..crate::pg::approval::OperatorAnswer::default()
        };
        assert!(crate::pg::approval::check_operator_code(&e, false, &operator).is_ok());
    }

    /// The view asks for exactly the facts `decision_body` refuses without: a
    /// Gas E/G entry whose Abrechnungsintervall froze lists the Bilanzkreis
    /// only, as free text; a bare Strom one lists its three facts, the
    /// Versorgungsart with the three codes `CCI+Z36` admits and the Grundlage
    /// nach EnFG with each Grund der Privilegierung beside `ZF9`.
    #[test]
    fn the_view_lists_the_facts_a_release_still_needs() {
        use mako_markt::commands as c;
        let gas = ApprovalQueueEntry::pending(
            Uuid::nil(),
            44_013,
            Some("51238696012".into()),
            "test".into(),
            time::macros::datetime!(2026-07-09 09:00 UTC),
            "t".into(),
        )
        .with_commands(c::GELI_EOG_BESTAETIGEN, c::GELI_EOG_ABLEHNEN, Some("LF"))
        .with_answer_payload(serde_json::json!({ "abrechnungsintervall_monate": 12 }));
        let f = Freigabe::from_entry(&gas);
        assert_eq!(f.art, FreigabeArt::Eskaliert, "frozen facts are no answer");
        let felder: Vec<&str> = f.fakten_freigeben.iter().map(|x| x.feld.as_str()).collect();
        assert_eq!(felder, ["bilanzkreis"]);
        assert!(f.fakten_freigeben[0].werte.is_empty());
        assert!(f.fakten_ablehnen.is_empty(), "a 44015 states no facts");

        let strom = ApprovalQueueEntry::pending(
            Uuid::nil(),
            55_013,
            Some("51238696012".into()),
            "test".into(),
            time::macros::datetime!(2026-07-09 09:00 UTC),
            "t".into(),
        )
        .with_commands(c::GPKE_EOG_BESTAETIGEN, c::GPKE_EOG_ABLEHNEN, Some("LF"));
        let f = Freigabe::from_entry(&strom);
        let felder: Vec<&str> = f.fakten_freigeben.iter().map(|x| x.feld.as_str()).collect();
        assert_eq!(felder, ["versorgungsart", "bilanzkreis", "enfg"]);
        // The Grundlage nach EnFG: ZG0, ZG1, and ZF9 once per Grund.
        let enfg: Vec<&str> = f.fakten_freigeben[2]
            .werte
            .iter()
            .map(|w| w.wert.as_str())
            .collect();
        assert_eq!(enfg.len(), 11, "{enfg:?}");
        assert!(enfg.contains(&"ZG0") && enfg.contains(&"ZF9+ZU6") && !enfg.contains(&"ZF9"));
        let codes: Vec<&str> = f.fakten_freigeben[0]
            .werte
            .iter()
            .map(|w| w.wert.as_str())
            .collect();
        for code in &codes {
            assert!(
                mako_gpke::Versorgungsart::from_code(code).is_some(),
                "{code}"
            );
        }
        assert_eq!(codes.len(), 3);
    }
}
