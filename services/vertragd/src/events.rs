//! CloudEvent dispatch and inbound routing for `vertragd`.
//!
//! # Emitted
//!
//! Every emission goes through [`build_cloud_event`] and is HMAC-signed by the
//! caller. The table is checked against the sources by
//! `the_emitted_event_table_lists_every_type_vertragd_emits`, so a new emitter
//! without a row here fails the build rather than leaving a subscriber with no
//! way to learn the event exists.
//!
//! | Event type | When |
//! |---|---|
//! | `de.mako.vertrag.aktiv` | All components NB-confirmed, billing may start |
//! | `de.mako.vertrag.lieferbeginn.bestaetigt` | The grid operator confirmed one component's Anmeldung; with the Lieferbeginn and the caller's references |
//! | `de.mako.vertrag.lieferbeginn.abgelehnt` | The grid operator refused one component's Anmeldung; with the Antwortcode and its Codeliste |
//! | `de.mako.vertrag.ersatzversorgung.begonnen` | An Ersatzversorgung contract was written — from the NB's 55013 or the contract route |
//! | `de.mako.vertrag.zuordnung.beendet` | The NB ended this supplier's Zuordnung (55037/44037); the component's Lieferende is the Zuordnungsende |
//! | `de.mako.vertrag.kuendigung-neulieferant.eingegangen` | A new supplier's Kündigung reached this deployment as the old supplier (55016/44016) |
//! | `de.mako.vertrag.kuendigung-vorversorger.bestaetigt` | The previous supplier confirmed the Kündigung sent on the customer's behalf |
//! | `de.mako.vertrag.kuendigung-vorversorger.abgelehnt` | It refused, with the Antwortcode's class and whether the Anmeldung went out |
//! | `de.mako.vertrag.gekuendigt` | Lieferende dispatched (Rahmenvertrag cascade, per child) |
//! | `de.mako.vertrag.kuendigung` | Kündigung accepted, Lieferende dispatched |
//! | `de.mako.vertrag.kuendigung-widerrufen` | Kündigung withdrawn before Lieferende |
//! | `de.mako.vertrag.tarifwechsel` | Product change applied immediately |
//! | `de.mako.vertrag.tarifwechsel-geplant` | Future-dated product change stored |
//! | `de.mako.vertrag.preisgarantie-hinterlegt` | Price guarantee stored/replaced |
//! | `de.mako.vertrag.preisaenderung.ankuendigung` | Notice worker, ≤ 42 days before Wirksamkeit |
//! | `de.mako.vertrag.autoerneuerung.ankuendigung` | 30 days before auto-renewal |
//! | `de.mako.vertrag.ablauf.ankuendigung` | 30 days before vertragsende / preisgarantie_bis |
//! | `de.mako.vertrag.abgeschlossen` | Every component's Lieferende has passed; supply is over and the contract is `ABGELAUFEN` (`workers::ablauf`) |
//! | `de.mako.vertrag.bestellung.eingegangen` | An online order and its Bestellnachweis are stored (`subject` = `bestellreferenz`) |
//! | `de.mako.vertrag.bestellung.angenommen` | The order became a contract — at the click (`mit_bestellung`) or at acceptance (`mit_annahme`) |
//! | `de.mako.vertrag.bestellung.abgelehnt` | The operator refused the order (`mit_annahme`) |
//! | `de.mako.vertrag.widerruf.eingegangen` | A consumer's Widerruf was recorded, with `rechtzeitig`, `wirkung` and `wertersatz_geschuldet` |
//!
//! `abgeschlossen` is not `gekuendigt`. A Kündigung is *accepted* on the day it
//! is filed and `gekuendigt` fires then — months before supply stops, with
//! billing running throughout. `abgeschlossen` fires when supply actually ends,
//! and it is the event a Schlussrechnung and the § 147 AO retention clock hang
//! off. A subscriber that took `gekuendigt` for the end of supply would invoice
//! a final bill for a customer still being supplied.

use serde_json::Value;
use uuid::Uuid;

/// Build a `de.mako.vertrag.*` CloudEvent.
///
/// `event_type` is a full CloudEvents type from the workspace catalog
/// (`mako_events::vertrag::*`).
///
/// Every event carries the workspace-standard tracing attributes:
/// `tenantid` (data-isolation scope) and `correlationid` (the Vertrag the
/// event belongs to — same value as `subject`, so consumers correlate all
/// lifecycle events of one contract without parsing `data`).
pub fn build_cloud_event(
    event_type: &str,
    vertrag_id: Uuid,
    tenant: &str,
    data: Value,
) -> mako_service::CloudEvent {
    mako_service::CloudEvent::new(
        mako_service::source("vertragd", tenant),
        event_type,
        vertrag_id.to_string(),
        data,
    )
    .extension("tenantid", tenant)
    .extension("correlationid", vertrag_id.to_string())
}

/// Build a `de.mako.vertrag.bestellung.*` / `de.mako.vertrag.widerruf.*` CloudEvent.
///
/// `subject` is the partner's own `bestellreferenz` — the reference the front
/// end that posted the order knows it by — and `correlationid` the
/// Bestellung, so every event of one order correlates without parsing `data`.
pub fn build_bestell_event(
    event_type: &str,
    bestellreferenz: &str,
    bestellung_id: Uuid,
    tenant: &str,
    data: Value,
) -> mako_service::CloudEvent {
    mako_service::CloudEvent::new(
        mako_service::source("vertragd", tenant),
        event_type,
        bestellreferenz.to_owned(),
        data,
    )
    .extension("tenantid", tenant)
    .extension("correlationid", bestellung_id.to_string())
}

/// Read a MaKo process outcome off an inbound CloudEvent.
///
/// **The verdict is in `data`, not in the type.** `de.mako.engine.process.completed`
/// says a process reached a terminal state, not that it succeeded: `mako-gpke`
/// emits it for both the NB's Bestätigung (PID 55002/55078) and its **Ablehnung**
/// (55003/55080), and the two are told apart only by `data.accepted` /
/// `data.outcome`. Reading the type suffix alone lets a refused Anmeldung set
/// the contract `BESTAETIGT`, which enqueues the Beginnablesung and the
/// Abrechnungskonto and bills a Marktlokation this Lieferant was never assigned.
///
/// So the payload decides whenever it carries a verdict, and the suffix is the
/// fallback for the event types that carry none. A type whose suffix says
/// confirmed and whose payload says rejected is resolved as **rejected**: the
/// two disagreeing is exactly the case that must not activate supply.
pub fn parse_mako_outcome(ce: &Value) -> Option<MakoOutcome> {
    let ce_type = ce.get("type")?.as_str()?;
    let data = ce.get("data")?;
    // makod names the process in the CloudEvent `subject` and marktd relays it
    // unchanged; a `data.process_id` is honoured where an emitter states one.
    let process_id = data
        .get("process_id")
        .or_else(|| ce.get("subject"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let malo_id = data
        .get("malo_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    // A process answer names its code as `antwortcode` beside the `codeliste`
    // that defines it; an APERAK rejection as `erc_code`.
    let erc_code = data
        .get("antwortcode")
        .or_else(|| data.get("erc_code"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let codeliste = data
        .get("codeliste")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let reason = data
        .get("reason")
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    // The explicit verdict, when the emitter supplied one.
    let payload_verdict = data.get("accepted").and_then(Value::as_bool).or_else(|| {
        match data.get("outcome").and_then(Value::as_str) {
            Some("accepted") => Some(true),
            Some("rejected") => Some(false),
            _ => None,
        }
    });

    let suffix_verdict = match ce_type {
        t if t.ends_with(".bestaetigt")
            || t.ends_with(".confirmed")
            || t.ends_with(".completed") =>
        {
            Some(true)
        }
        t if t.ends_with(".abgelehnt") || t.ends_with(".rejected") => Some(false),
        _ => None,
    };

    // A type this service does not recognise is not an outcome, even when its
    // payload carries an `accepted` key — that key belongs to some other
    // process and joining on it would apply a foreign verdict to a contract.
    suffix_verdict?;

    // `false` from either side wins: a payload and a suffix that disagree is
    // the shape that must not activate supply.
    let confirmed = payload_verdict.unwrap_or(true) && suffix_verdict.unwrap_or(true);

    Some(MakoOutcome {
        process_id,
        malo_id,
        confirmed,
        // Carried on a refusal only: a Bestätigung has no Ablehnungsgrund, and
        // leaving a stale one on it would put an ERC code beside an accepted
        // Marktlokation.
        antwortcode: erc_code.clone(),
        antwort_codeliste: codeliste.clone(),
        process_date: data
            .get("process_date")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        erc_code: if confirmed { None } else { erc_code },
        codeliste: if confirmed { None } else { codeliste },
        reason: if confirmed { None } else { reason },
    })
}

pub struct MakoOutcome {
    pub process_id: Option<String>,
    pub malo_id: Option<String>,
    pub confirmed: bool,
    /// The Antwortcode of a refusal.
    pub erc_code: Option<String>,
    /// The Entscheidungsbaum or Codeliste that defines `erc_code`; a code
    /// means nothing without it.
    pub codeliste: Option<String>,
    pub reason: Option<String>,
    /// The answer's Antwortcode whatever the verdict — a Zustimmung's code
    /// distinguishes a plain one from one that moved the date (`E_3001` Z01).
    pub antwortcode: Option<String>,
    /// The Codeliste of [`Self::antwortcode`].
    pub antwort_codeliste: Option<String>,
    /// The date the process asked for, as `data.process_date` names it.
    pub process_date: Option<String>,
}

/// The contract a component belongs to, as its events name it: mako's ids and
/// the references the system that handed the contract over knows it by.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema, sqlx::FromRow)]
pub struct Vertragsbezug {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub vertrag_id: Uuid,
    pub vertrags_nr: String,
    /// The Kunde the contract is with — the customer number of the portfolio
    /// route.
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub kunden_id: Uuid,
    /// The caller's contract reference, where the contract was created with one.
    pub erp_contract_id: Option<String>,
    /// The front end's order reference, where the contract came from an order.
    pub bestellreferenz: Option<String>,
}

/// One component of a contract.
#[derive(Debug, Clone, serde::Serialize, schemars::JsonSchema)]
pub struct Komponentenbezug {
    #[schemars(with = "String", extend("format" = "uuid"))]
    pub komp_id: Uuid,
    pub sparte: String,
    pub malo_id: Option<String>,
    /// The day supply begins, as the contract holds it.
    #[schemars(extend("format" = "date"))]
    pub lieferbeginn: String,
}

/// The `data` of `de.mako.vertrag.lieferbeginn.bestaetigt`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct LieferbeginnBestaetigt {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
}

impl mako_service::EventPayload for LieferbeginnBestaetigt {
    const TYPE: &'static str = mako_events::vertrag::LIEFERBEGINN_BESTAETIGT;
}

/// The `data` of `de.mako.vertrag.lieferbeginn.abgelehnt`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct LieferbeginnAbgelehnt {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
    /// The grid operator's Antwortcode, where its answer carried one.
    pub antwortcode: Option<String>,
    /// The Entscheidungsbaum or Codeliste that defines `antwortcode`.
    pub codeliste: Option<String>,
    /// The answer's text, where it carried one.
    pub reason: Option<String>,
}

impl mako_service::EventPayload for LieferbeginnAbgelehnt {
    const TYPE: &'static str = mako_events::vertrag::LIEFERBEGINN_ABGELEHNT;
}

/// The `data` of `de.mako.vertrag.ersatzversorgung.begonnen`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ErsatzversorgungBegonnen {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
    /// The day the Ersatzversorgung ends at the latest: „spätestens aber drei
    /// Monate nach Beginn" (§ 38 Abs. 4 Satz 1 EnWG).
    #[schemars(extend("format" = "date"))]
    pub ende: String,
    /// The household's name as the Netzbetreiber stated it (`NAD+Z67`, Kann)
    /// or as the contract's Kunde holds it; absent where neither names one.
    pub kunde_name: Option<String>,
    /// `55013` when the Netzbetreiber's Zuordnung opened it, `api` when it
    /// was created through the contract route.
    pub quelle: String,
}

impl mako_service::EventPayload for ErsatzversorgungBegonnen {
    const TYPE: &'static str = mako_events::vertrag::ERSATZVERSORGUNG_BEGONNEN;
}

/// The `data` of `de.mako.vertrag.zuordnung.beendet`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct ZuordnungBeendet {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
    /// The Zuordnungsende the Netzbetreiber stated (`DTM+93`), now the
    /// component's Lieferende.
    #[schemars(extend("format" = "date"))]
    pub zuordnungsende: String,
    /// `SG4 STS+7` — why the Zuordnung ended.
    pub grund: String,
    /// `true` when another supplier takes the Marktlokation (`E03` Wechsel).
    pub wechsel: bool,
    /// The Netzbetreiber that sent the 55037 / 44037.
    pub netzbetreiber: String,
}

impl mako_service::EventPayload for ZuordnungBeendet {
    const TYPE: &'static str = mako_events::vertrag::ZUORDNUNG_BEENDET;
}

/// The `data` of `de.mako.vertrag.kuendigung-neulieferant.eingegangen`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct KuendigungNeulieferantEingegangen {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
    /// The new supplier that sent the Kündigung.
    pub neulieferant: String,
    /// The end it asks for (`DTM+93`); absent where it asks for the
    /// nächstmöglicher Termin.
    #[schemars(extend("format" = "date"))]
    pub kuendigung_zum: Option<String>,
    /// It asks for the nächstmöglicher Termin (`DTM+471`).
    pub naechstmoeglicher_termin: bool,
}

impl mako_service::EventPayload for KuendigungNeulieferantEingegangen {
    const TYPE: &'static str = mako_events::vertrag::KUENDIGUNG_NEULIEFERANT_EINGEGANGEN;
}

/// The `data` of `de.mako.vertrag.kuendigung-vorversorger.bestaetigt`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct KuendigungVorversorgerBestaetigt {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
    /// The previous supplier that confirmed.
    pub vorversorger_mp_id: String,
    /// The end of the old contract the Kündigung named.
    #[schemars(extend("format" = "date"))]
    pub kuendigung_zum: String,
    pub antwortcode: Option<String>,
    pub codeliste: Option<String>,
    /// Whether the Anmeldung went out (`angemeldet`) or waits (`gehalten`).
    pub reaktion: crate::kuendigung_vorversorger::Reaktion,
}

impl mako_service::EventPayload for KuendigungVorversorgerBestaetigt {
    const TYPE: &'static str = mako_events::vertrag::KUENDIGUNG_VORVERSORGER_BESTAETIGT;
}

/// The `data` of `de.mako.vertrag.kuendigung-vorversorger.abgelehnt`.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct KuendigungVorversorgerAbgelehnt {
    #[serde(flatten)]
    pub vertrag: Vertragsbezug,
    #[serde(flatten)]
    pub komponente: Komponentenbezug,
    pub vorversorger_mp_id: String,
    pub antwortcode: Option<String>,
    pub codeliste: Option<String>,
    pub reason: Option<String>,
    /// What the code means for the Anmeldung.
    pub klasse: Option<crate::kuendigung_vorversorger::Klasse>,
    pub reaktion: crate::kuendigung_vorversorger::Reaktion,
}

impl mako_service::EventPayload for KuendigungVorversorgerAbgelehnt {
    const TYPE: &'static str = mako_events::vertrag::KUENDIGUNG_VORVERSORGER_ABGELEHNT;
}

/// A typed `de.mako.vertrag.*` event of one contract: subject and `correlationid`
/// the Vertrag, like [`build_cloud_event`].
///
/// # Errors
///
/// The `serde_json` error if the payload cannot be encoded.
pub fn typed_event<P: mako_service::EventPayload>(
    vertrag_id: Uuid,
    tenant: &str,
    payload: &P,
) -> Result<mako_service::CloudEvent, serde_json::Error> {
    Ok(mako_service::CloudEvent::typed(
        mako_service::source("vertragd", tenant),
        vertrag_id.to_string(),
        payload,
    )?
    .extension("tenantid", tenant)
    .extension("correlationid", vertrag_id.to_string()))
}

/// The schema of every payload vertragd emits through a binding.
#[cfg(test)]
fn event_schemas() -> Vec<(&'static str, Value)> {
    vec![
        mako_service::cloudevent::event_schema::<LieferbeginnBestaetigt>(),
        mako_service::cloudevent::event_schema::<LieferbeginnAbgelehnt>(),
        mako_service::cloudevent::event_schema::<ErsatzversorgungBegonnen>(),
        mako_service::cloudevent::event_schema::<ZuordnungBeendet>(),
        mako_service::cloudevent::event_schema::<KuendigungVorversorgerBestaetigt>(),
        mako_service::cloudevent::event_schema::<KuendigungVorversorgerAbgelehnt>(),
        mako_service::cloudevent::event_schema::<KuendigungNeulieferantEingegangen>(),
    ]
}

#[cfg(test)]
mod tests {
    /// `events/` holds the schema of every bound payload (`BLESS_MCP_SNAPSHOT=1`
    /// rewrites it).
    #[test]
    fn the_committed_event_schemas_are_current() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("events");
        if let Err(e) =
            mako_service::cloudevent::compare_event_schemas(&dir, &super::event_schemas())
        {
            panic!("{e}");
        }
    }

    /// A process answer names its code as `antwortcode` with its `codeliste`;
    /// reading only `erc_code` leaves a refused Anmeldung without the reason
    /// the grid operator gave.
    #[test]
    fn a_refusals_antwortcode_and_codeliste_are_read() {
        let ce = serde_json::json!({
            "type": "de.mako.engine.process.completed",
            "data": { "process_id": "p", "accepted": false, "outcome": "rejected",
                      "antwortcode": "A02", "codeliste": "E_0622", "reason": "x" }
        });
        let outcome = super::parse_mako_outcome(&ce).expect("an outcome");
        assert!(!outcome.confirmed);
        assert_eq!(outcome.erc_code.as_deref(), Some("A02"));
        assert_eq!(outcome.codeliste.as_deref(), Some("E_0622"));

        let ok = serde_json::json!({
            "type": "de.mako.engine.process.completed",
            "data": { "process_id": "p", "accepted": true, "antwortcode": "A01",
                      "codeliste": "E_0622" }
        });
        let outcome = super::parse_mako_outcome(&ok).expect("an outcome");
        assert!(outcome.erc_code.is_none() && outcome.codeliste.is_none());
    }

    /// makod names the process in the CloudEvent `subject`, and marktd relays
    /// it unchanged: an outcome without `data.process_id` still finds its
    /// component, or a confirmed Anmeldung would never activate supply.
    #[test]
    fn the_process_is_read_from_the_subject() {
        let ce = serde_json::json!({
            "type": "de.mako.engine.process.completed",
            "subject": "0f8e3c1a-0000-4000-8000-000000000001",
            "data": { "accepted": true, "malo_id": "51238696012" }
        });
        let outcome = super::parse_mako_outcome(&ce).expect("an outcome");
        assert_eq!(
            outcome.process_id.as_deref(),
            Some("0f8e3c1a-0000-4000-8000-000000000001")
        );
    }

    /// The emitted-event table names every `de.mako.vertrag.*` type vertragd emits.
    ///
    /// The table is the only place a subscriber can read what this service puts
    /// on the bus — the constants live in `mako-events`, which says nothing
    /// about who emits them, and the emission sites are spread over the workers
    /// and the handlers. A missing row is therefore an event nobody outside the
    /// code knows exists.
    ///
    /// `de.mako.vertrag.abgeschlossen`, emitted by `workers::ablauf` on every closed
    /// supply, is the event a Schlussrechnung hangs off — a missing row would
    /// hide it from the one consumer that most needs it.
    ///
    /// Read off the sources rather than from a second hand-kept list: a list
    /// drifts from the emitters exactly the way the table can.
    #[test]
    fn the_emitted_event_table_lists_every_type_vertragd_emits() {
        /// `MODULE_DOC` is this file; every other `.rs` under `src/` is scanned
        /// for emissions.
        const MARKER: &str = "mako_events::vertrag::";

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = String::new();
        collect_rs(&root, &mut sources);

        let mut emitted: Vec<String> = Vec::new();
        for (i, _) in sources.match_indices(MARKER) {
            let rest = &sources[i + MARKER.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_uppercase() || c == '_'))
                .unwrap_or(rest.len());
            let name = &rest[..end];
            if !name.is_empty() {
                emitted.push(name.to_owned());
            }
        }
        emitted.sort();
        emitted.dedup();
        assert!(
            emitted.len() > 5,
            "found only {} emitted types — the scanner broke, not the service",
            emitted.len()
        );

        // `SCREAMING_SNAKE` → the wire type, the way `mako-events` spells it:
        // `KUENDIGUNG_WIDERRUFEN` is `de.mako.vertrag.kuendigung-widerrufen`, and
        // `PREISAENDERUNG_ANKUENDIGUNG` is `de.mako.vertrag.preisaenderung.ankuendigung`
        // — one uses a hyphen, the other a dot, so the constant's *value* is the
        // only reliable source. Take it from the catalogue.
        let table = include_str!("events.rs");
        let table = table
            .split("# Emitted")
            .nth(1)
            .expect("the emitted section");
        let table = table
            .split("`abgeschlossen` is not")
            .next()
            .unwrap_or(table);

        let missing: Vec<&str> = emitted
            .iter()
            .filter(|name| {
                let wire = wire_type(name);
                !table.contains(&format!("`{wire}`"))
            })
            .map(String::as_str)
            .collect();

        assert!(
            missing.is_empty(),
            "these `de.mako.vertrag.*` types are emitted by vertragd but absent from the \
             emitted-event table in this module's doc comment: {missing:?}"
        );
    }

    /// The catalogue value for `mako_events::vertrag::<name>`.
    fn wire_type(name: &str) -> String {
        for ty in mako_events::all() {
            let Some(suffix) = ty.strip_prefix("de.mako.vertrag.") else {
                continue;
            };
            if suffix.replace(['-', '.'], "_").to_uppercase() == name {
                return (*ty).to_owned();
            }
        }
        panic!("`mako_events::vertrag::{name}` names no catalogued type");
    }

    /// Append every `.rs` file under `dir` to `out`.
    fn collect_rs(dir: &std::path::Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(s) = std::fs::read_to_string(&path)
            {
                out.push_str(&s);
            }
        }
    }
}
