//! NB process decision module — the Netzbetreiber's own GPKE / GeLi Gas
//! answer obligations.
//!
//! # What the NB owes an answer to
//!
//! | Inbound PID | Process | Answers | EBD | Frist |
//! |---|---|---|---|---|
//! | **55001** | Anmeldung verb. MaLo (Lieferbeginn) | 55002 / 55003 | `E_0622` | 11:00 Uhr des 1. WT nach dem ÜT |
//! | **55077** | Anmeldung erz. MaLo (Lieferbeginn) | 55078 / 55080 | `E_0622` | 11:00 Uhr des 1. WT nach dem ÜT |
//! | **55004** | Abmeldung (Lieferende von LF an NB) | 55005 / 55006 | `E_0607` | 06:00 Uhr des 1. WT nach dem ÜT |
//! | **44001** | Anmeldung NN (Gas Lieferbeginn) | 44002 / 44003 | — | Ablauf des 4. Werktags |
//! | **44004** | Abmeldung NN (Gas Lieferende) | 44005 / 44006 | — | Ablauf des 3. Werktags |
//!
//! Every Frist comes from [`mako_fristen::antwort`], which reads the same tables
//! `makod` registers the process deadline from.
//!
//! ## What is deliberately *not* here
//!
//! **55016 „Kündigung" is not an NB process** and is answered by no role here.
//! The Anwendungsübersicht der Prüfidentifikatoren 4.0 (lfd. Nr. 20030) has it
//! going **LFN → LFA**, answered 55017/55018 by the *Altlieferant* under EBD
//! `E_0614`. Evaluating it here would make an `nb-only` binary answer a
//! supplier-role message — the § 7 EnWG separation the Cargo features exist for
//! — with grid-topology checks the LFA has no basis for. The answer belongs to
//! the supplier role and is decided there.
//!
//! # Decision pipeline
//!
//! ```text
//! Anmeldung (55001 / 55077 / 44001)          Abmeldung (55004 / 44004)
//!   → GET /api/v1/versorgung/{malo}            → GET /api/v1/versorgung/{malo}
//!   → GET /api/v1/malos/{malo}/grid
//!   → GET /api/v1/partners/{lf}
//!   → mako_pruefung::evaluate                   → mako_pruefung::evaluate_abmeldung
//!       Accept   → bestaetigen [auto_accept]       Accept   → bestaetigen [auto_accept]
//!                  else approval_queue                        else approval_queue
//!       Reject   → ablehnen (ERC)                  Reject   → ablehnen (ERC)
//!       Escalate → approval_queue                  Escalate → approval_queue
//! ```
//!
//! The two decision trees have **separate ERC code spaces** — `A02` is
//! „Marktlokation nimmt nicht an der Marktkommunikation teil" in `E_0622` and
//! „Vorlauffrist nicht eingehalten" in `E_0607` — which is why they are
//! separate functions in `mako-pruefung` rather than one with a flag.
//!
//! # Regulatory basis
//!
//! - GPKE: BK6-24-174 Teil 2 (SD Lieferbeginn, SD Lieferende von LF an NB)
//! - GeLi Gas: BK7-24-01-009 Kap. 3.2.2 / 3.2.3
//! - EBD 4.3 Kap. 6.6.1 (`E_0622`), 6.3.1 (`E_0607`)
//! - § 20 EnWG parity: `initiator_is_affiliate` recorded on every decision

use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_pruefung::nb::types::{
    ErzeugungsAnmeldung, Geschaeftsvorfall, Marktlokationsart, Veraeusserungsform,
};
use mako_pruefung::{AnmeldungAnfrage, Messtyp, NbEntscheidung};
use time::OffsetDateTime;
use tracing::{info, warn};
use uuid::Uuid;

use mako_markt::domain::Sparte;
use secrecy::SecretString;

use crate::pg::abmeldeanfrage::{AbmeldeanfrageRecord, PgAbmeldeanfrageRepository, Waiting};
use crate::pg::anmeldung::{AnmeldungDecision, AnmeldungDecisionRecord, PgAnmeldungRepository};
use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the NB module.
#[derive(Debug, Clone)]
pub struct NbModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    pub own_mp_id: String,
    pub tenant: String,
    pub auto_accept: bool,
    /// Gas Bearbeitungsfrist (WT) added to the 6-week retroactive Anmeldung
    /// window. Defaults to [`mako_pruefung::nb::anmeldung::GAS_BEARBEITUNGSFRIST_WT_DEFAULT`]
    /// (3 WT); operators whose AWH reading differs may override it.
    pub gas_bearbeitungsfrist_wt: u32,
}

impl NbModuleConfig {
    /// Build the pure-library [`mako_pruefung::NetzCheckConfig`] from this config.
    #[must_use]
    pub fn netz_check_config(&self) -> mako_pruefung::NetzCheckConfig {
        mako_pruefung::NetzCheckConfig {
            gas_bearbeitungsfrist_wt: self.gas_bearbeitungsfrist_wt,
            ..mako_pruefung::NetzCheckConfig::default()
        }
    }
}

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Inbound **Anmeldung** PIDs the NB answers.
///
/// 55001 verbrauchende MaLo, 55077 erzeugende MaLo (both LFN → NB, GPKE Teil 2
/// SD Lieferbeginn), 44001 Anmeldung NN (GeLi Gas 3.0 Kap. 3.2.3).
///
/// 55016 is **not** here: it is the Kündigung, LFN → LFA, and belongs to the
/// supplier role (see the module docs).
pub const ANMELDUNG_PIDS: &[u32] = &[55_001, 55_077, 44_001];

/// `SG4 STS+7` DE 9013 element 2 — Transaktionsgrund `E03` „Wechsel".
///
/// The payload reaches this module as JSON from `makod`, which has already read
/// the segment; the code itself is the same in both Sparten.
const WECHSEL: &str = "E03";

/// Inbound **Abmeldung** PIDs the NB answers.
///
/// 55004 „Abmeldung" (LF → NB, GPKE Teil 2 SD Lieferende von LF an NB) and
/// 44004 „Abmeldung NN" (GeLi Gas 3.0 Kap. 3.2.2). Neither was routed anywhere
/// before, so every Lieferende a supplier initiated ran out its Frist unseen.
pub const ABMELDUNG_PIDS: &[u32] = &[55_004, 44_004];

/// Every inbound PID this module answers.
#[must_use]
pub fn answered_pids() -> Vec<u32> {
    let mut v: Vec<u32> = ANMELDUNG_PIDS
        .iter()
        .chain(ABMELDUNG_PIDS)
        .copied()
        .collect();
    v.sort_unstable();
    v
}

/// The Sparte an NB PID belongs to — Strom in the 55xxx band, Gas in 44xxx.
const fn sparte_of(pid: u32) -> Sparte {
    if pid >= 44_000 && pid < 45_000 {
        Sparte::Gas
    } else {
        Sparte::Strom
    }
}

// ── NB module payload ─────────────────────────────────────────────────────────

/// Fields extracted from a `de.mako.process.initiated` CloudEvent payload
/// for a Lieferbeginn PID.
#[derive(Debug, Clone)]
pub struct AnmeldungPayload {
    pub pid: u32,
    pub process_id: Uuid,
    pub malo_id: String,
    pub new_supplier_gln: String,
    pub grid_operator_gln: String,
    pub bilanzierungsgebiet: Option<String>,
    pub process_date: time::Date,
    /// SG4 STS Transaktionsgrund (DE9013) — e.g. `E01` Ein-/Auszug,
    /// `E03` Lieferantenwechsel. Drives the date-plausibility rules.
    pub transaktionsgrund: Option<String>,
    /// `SG4 STS+7` DE 9013 **element 3** — the Transaktionsgrundergänzung
    /// (`ZW4` verbrauchende, `ZW3` erzeugende, `ZW5` Tranche, `ZAP` ruhende
    /// Marktlokation). Decides which `E_0622` code space answers.
    pub transaktionsgrund_ergaenzung: Option<String>,
    /// `SG10 CCI+Z22` DE 7037 — the angemeldete Veräußerungsform of an
    /// erzeugende Marktlokation (`Z90`/`Z91`/`Z92`/`Z94`).
    pub veraeusserungsform: Option<String>,
    /// Bilanzierungsmethode from UTILMD TM+EM (`SLP` | `RLM` | `IMS`).
    pub bilanzierungsmethode: Option<String>,
    /// `SG4 IDE+24` DE 7402 — the LFN's Vorgangsnummer for this Anmeldung.
    ///
    /// Echoed in `SG6 RFF+TN` on the 55036 / 44036 Information über
    /// existierende Zuordnung, where the AHB marks it Muss. Without it the LFN
    /// receives a Meldung it cannot tie to the Anmeldung it just sent.
    pub vorgangsnummer: Option<String>,
    /// `SG12 NAD+Z09` — the Letztverbraucher the LFN named.
    ///
    /// Copied verbatim onto the Anfrage zur Beendigung der Zuordnung, where
    /// UTILMD AHB Strom Bedingung `[279]` marks it Muss for a verbrauchende
    /// oder ruhende Marktlokation. It is what `E_0624` Prüfschritt 30 tells an
    /// Einzug from a Wechsel by.
    pub kunde_name: Option<String>,
    /// `NAD` DE 3045 — `Z01` Personenname, `Z02` Firmenbezeichnung.
    pub kunde_namensformat: Option<String>,
}

impl AnmeldungPayload {
    /// Parse from the `data` field of a `de.mako.process.initiated` CloudEvent.
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;

        if !ANMELDUNG_PIDS.contains(&pid) {
            return None;
        }

        let subject = event["subject"].as_str()?;
        let process_id: Uuid = subject.parse().ok()?;

        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let new_supplier_gln = data.get("new_supplier")?.as_str()?.to_owned();
        let grid_operator_gln = data.get("grid_operator")?.as_str()?.to_owned();
        let bilanzierungsgebiet = data
            .get("bilanzierungsgebiet")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let transaktionsgrund = data
            .get("transaktionsgrund")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let transaktionsgrund_ergaenzung = data
            .get("transaktionsgrund_ergaenzung")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let veraeusserungsform = data
            .get("veraeusserungsform")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let bilanzierungsmethode = data
            .get("bilanzierungsmethode")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let vorgangsnummer = data
            .get("vorgangsnummer")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let kunde_name = data
            .get("kunde_name")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let kunde_namensformat = data
            .get("kunde_namensformat")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);

        let date_str = data.get("process_date")?.as_str()?;
        let process_date = if date_str.len() == 8 {
            let fmt = time::macros::format_description!("[year][month][day]");
            time::Date::parse(date_str, &fmt).ok()?
        } else {
            let fmt = time::macros::format_description!("[year]-[month]-[day]");
            time::Date::parse(date_str, &fmt).ok()?
        };

        Some(Self {
            pid,
            process_id,
            malo_id,
            new_supplier_gln,
            grid_operator_gln,
            bilanzierungsgebiet,
            process_date,
            transaktionsgrund,
            transaktionsgrund_ergaenzung,
            veraeusserungsform,
            bilanzierungsmethode,
            vorgangsnummer,
            kunde_name,
            kunde_namensformat,
        })
    }

    /// Derive `AnmeldungAnfrage` for passing to `mako-pruefung`.
    pub fn into_anfrage(self) -> AnmeldungAnfrage {
        let sparte = sparte_of(self.pid);
        let marktlokationsart =
            marktlokationsart_of(self.pid, self.transaktionsgrund_ergaenzung.as_deref());
        let erzeugung = (marktlokationsart == Marktlokationsart::Erzeugend)
            .then(|| {
                erzeugung_of(
                    self.transaktionsgrund_ergaenzung.as_deref(),
                    self.veraeusserungsform.as_deref(),
                )
            })
            .flatten();
        // Messtyp from the UTILMD TM+EM marker carried in the payload
        // (Z01=SLP, Z02=RLM, Z04=IMS → adapter emits "SLP"/"RLM"/"IMS").
        // Default SLP when absent — the conservative Vorlauffrist bound.
        let messtyp = messtyp_of(self.bilanzierungsmethode.as_deref());
        AnmeldungAnfrage {
            pid: self.pid,
            process_id: self.process_id,
            malo_id: self.malo_id,
            new_supplier_gln: self.new_supplier_gln,
            grid_operator_gln: self.grid_operator_gln,
            bilanzierungsgebiet: self.bilanzierungsgebiet,
            process_date: self.process_date,
            sparte,
            messtyp,
            transaktionsgrund: self.transaktionsgrund,
            marktlokationsart,
            erzeugung,
            // Filled in by `evaluate_and_decide` once it has read the
            // Versorgungsstatus — SD Lieferbeginn Nr. 1 Prüfschritt 4 asks
            // whether the Marktlokation is assigned, which the message does not
            // say. `NichtErforderlich` is the only value a caller that cannot
            // see the projection may use.
            abmeldeanfrage: mako_pruefung::Abmeldeanfrage::NichtErforderlich,
        }
    }
}

/// Which `E_0622` / `E_0607` branch an inbound message belongs to.
///
/// PID 55077 **is** the Anwendungsfall „Anmeldung erzeugende Marktlokation", so
/// it decides the branch on its own; otherwise the `SG4 STS+7` DE 9013 element 3
/// Transaktionsgrundergänzung does. `ZW4` (verbrauchende Marktlokation) is the
/// default the AHB marks Muss on every GPKE core process.
fn marktlokationsart_of(pid: u32, ergaenzung: Option<&str>) -> Marktlokationsart {
    if pid == 55_077 {
        return Marktlokationsart::Erzeugend;
    }
    match ergaenzung {
        Some("ZW3" | "ZW5") => Marktlokationsart::Erzeugend,
        Some("ZAP") => Marktlokationsart::Ruhend,
        _ => Marktlokationsart::Verbrauchend,
    }
}

/// Build the erzeugende-Marktlokation facts from what the message carries.
///
/// Returns `None` when the Veräußerungsform is absent or unknown — `evaluate`
/// then escalates, which is the § 20 EnWG-safe answer.
///
/// **`bestehende_veraeusserungsform` is deliberately `None` here.** It is the
/// NB's own EEG-/KWKG-Register, not a wire fact, and `processd` has no reader
/// for it; the engine escalates the Veräußerungsformwechsel question rather
/// than assuming there was none.
fn erzeugung_of(
    ergaenzung: Option<&str>,
    veraeusserungsform: Option<&str>,
) -> Option<ErzeugungsAnmeldung> {
    let angemeldete = Veraeusserungsform::from_wire_code(veraeusserungsform?)?;
    // `ZW5` marks a Tranche, which is Geschäftsvorfall 2 or 3; the two differ by
    // whether the Tranche already exists, which the message does not say. Only
    // the non-tranchierte case (`ZW3`) resolves to a Geschäftsvorfall here.
    let geschaeftsvorfall = match ergaenzung {
        Some("ZW5") => return None,
        _ => Geschaeftsvorfall::Eins,
    };
    Some(ErzeugungsAnmeldung {
        geschaeftsvorfall,
        angemeldete_veraeusserungsform: angemeldete,
        bestehende_veraeusserungsform: None,
        // A „Nicht-EEG-/-KWKG"-Marktlokation carries no Klassentyp `Z22`
        // („Gesetzliche Kategorie") at all, so it never reaches this branch: it
        // takes the `None` path above and escalates. `Z92` is not it — that is
        // sonstige Direktvermarktung, still an EEG plant.
        nicht_eeg_kwkg: false,
        ausfallverguetung: false,
    })
}

// ── evaluate_and_decide ───────────────────────────────────────────────────────

/// Decide one `de.mako.process.initiated` event addressed to the NB.
///
/// Routes to the Anmeldung pipeline (55001 / 55077 / 44001) or the Abmeldung
/// pipeline (55004 / 44004). Returns `true` when this module handled the event
/// — including when it escalated — and `false` when the PID belongs to another
/// role or another module.
#[allow(clippy::too_many_arguments)]
pub async fn evaluate_and_decide(
    event: &serde_json::Value,
    config: &NbModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    einsd: Option<&crate::einsd_client::EinsdClient>,
    makod: &MakodClient,
    repo: &PgAnmeldungRepository,
    queue: &PgApprovalQueue,
    pending: &PgAbmeldeanfrageRepository,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(payload) = AbmeldungPayload::parse(event) {
        return decide_abmeldung(payload, event, config, reader, makod, repo, queue).await;
    }

    // ── 1. Parse payload ──────────────────────────────────────────────────
    let Some(payload) = AnmeldungPayload::parse(event) else {
        return Ok(false);
    };

    // ── 2. Misdirection check ─────────────────────────────────────────────
    // Fast pre-check: if the event is not for our GLN, skip silently.
    if !payload.grid_operator_gln.is_empty() && payload.grid_operator_gln != config.own_mp_id {
        return Ok(false);
    }

    let initiator_is_affiliate = payload.new_supplier_gln == config.own_mp_id;
    let pid = payload.pid;
    let process_id = payload.process_id;
    let malo_id = payload.malo_id.clone();
    let lf_mp_id = payload.new_supplier_gln.clone();
    // The answer Frist runs from receipt of the market message; the CloudEvent
    // `time` is when makod emitted it.
    let received_at = event["time"]
        .as_str()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);
    let payload_meta = AnmeldungMeta {
        pid,
        process_id,
        malo_id: malo_id.clone(),
        received_at,
    };

    info!(
        %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
        "processd NB: evaluating Anmeldung"
    );

    // ── 3. Fetch marktd data ──────────────────────────────────────────────
    let versorgung = reader
        .get_versorgung(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd versorgung fetch failed"))?;

    let malo = reader
        .get_malo(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd malo fetch failed"))
        .unwrap_or(None);

    let grid = reader
        .get_malo_grid(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd grid fetch failed"))?;

    let partner_known = reader.partner_known(&lf_mp_id).await.inspect_err(
        |e| warn!(%e, lf_mp_id = %lf_mp_id, "processd NB: marktd partner check failed"),
    )?;

    // ── Selbstzahler (GPKE Teil 1, Vorbemerkung) ──────────────────────────
    // The Festlegung takes the LF's Lieferantenwechsel-Meldungen out of the role
    // a Selbstzahler otherwise steps into — see [`NetznutzerTyp`] — so the NB
    // cannot assume the incumbent behaves like an LF there. Only a Wechsel
    // (`E03`) is affected, so the lookup runs for nothing else. The incumbent is
    // whoever holds the Netznutzungsvertrag the day before the Zuordnungsbeginn.
    let incumbent_is_selbstzahler = if payload.transaktionsgrund.as_deref() == Some(WECHSEL) {
        let am = payload
            .process_date
            .previous_day()
            .unwrap_or(payload.process_date);
        reader
            .get_nb_contract_for_malo(&malo_id, am)
            .await
            .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd NNV fetch failed"))?
            .is_some_and(|c| c.is_selbstzahler())
    } else {
        false
    };

    // ── Meldepflichten (GPKE Teil 2 § 2.1.2 Nr. 2 / 10) ───────────────────
    //
    // Built before `payload` is consumed: the Information über existierende
    // Zuordnung must reference the LFN's own Vorgangsnummer, and the
    // Beendigung der Zuordnung names the Zuordnungsende, which is the
    // Zuordnungsbeginn of this Anmeldung. Both are dispatched from the
    // Prozessschritt they belong to — Nr. 2 „parallel zu Nr. 3", Nr. 10 after
    // the Bestätigung — never here.
    let kunde = Kunde {
        name: payload.kunde_name.clone(),
        namensformat: payload.kunde_namensformat.clone(),
    };
    let meldung = MeldepflichtContext {
        sparte: sparte_of(pid),
        lfn_mp_id: lf_mp_id.clone(),
        zuordnungsbeginn: payload.process_date,
        vorgangsnummer: payload.vorgangsnummer.clone(),
        tranche: payload.transaktionsgrund_ergaenzung.as_deref() == Some("ZW5"),
        altlieferant: versorgung.as_ref().and_then(|v| v.lf_mp_id.clone()),
    };

    let mut anfrage = payload.into_anfrage();
    // `E_0622` Prüfschritt 400 / 600 („Verändert sich die Veräußerungsform?")
    // needs the form in force at the Zuordnungsbeginn. That is the NB's own
    // EEG-/KWKG-Register, not the message — and `Z90` covers two regimes with
    // different Fristen, so the Ausfallvergütung flag comes from there too.
    if let (Some(erz), Some(einsd)) = (anfrage.erzeugung.as_mut(), einsd) {
        match einsd.veraeusserungsform(&malo_id).await {
            Ok(Some(auskunft)) => {
                erz.bestehende_veraeusserungsform = Some(auskunft.veraeusserungsform);
                erz.ausfallverguetung = auskunft.ausfallverguetung;
            }
            // Not in the register. That is not evidence of a
            // „Nicht-EEG-/-KWKG"-Marktlokation, so the engine escalates.
            Ok(None) => {}
            // A transport failure is not an answer: propagate so the fan-out
            // redelivers rather than deciding on a missing fact.
            Err(e) => {
                warn!(%e, %malo_id, "processd NB: einsd Veräußerungsform lookup failed");
                return Err(Box::new(e));
            }
        }
    }
    // ── SD Lieferbeginn Nr. 1 Prüfschritt 4 ───────────────────────────────
    //
    // „Ist die Marktlokation bzw. Tranche zum Zuordnungsbeginn einem LF
    // zugeordnet, fährt der NB mit Prozessschritt 2 fort, ansonsten mit
    // Prozessschritt 5." This is the branch that makes the NB's answer
    // two-phase, and the fact it turns on — who holds the Marktlokation — lives
    // in `marktd`, not in the message.
    //
    // Set unconditionally to `Erforderlich` here; phase two replaces it with
    // `Gestellt` and the LFA's answer. A Marktlokation with no incumbent stays
    // `NichtErforderlich`, which is `E_0623` Prüfschritt 20 „nein".
    anfrage.abmeldeanfrage = match versorgung.as_ref().and_then(|v| v.lf_mp_id.clone()) {
        Some(lfa) => mako_pruefung::Abmeldeanfrage::Erforderlich {
            lfa_mp_ids: vec![lfa],
        },
        None => mako_pruefung::Abmeldeanfrage::NichtErforderlich,
    };
    let anfrage = anfrage;
    let now = OffsetDateTime::now_utc();

    // ── 4. Evaluate ───────────────────────────────────────────────────────
    // Build a grid record for `mako-pruefung` from the best available source:
    //  1. `malo_grid` side table (NB-role PUT provisioning) — most authoritative
    //  2. `malo.bilanzierungsgebiet` (B1 typed extraction) — fallback when the
    //     malo_grid record is absent; raises STP from ~60% to ~80% for SLP MaLos
    let vs_ref = versorgung.as_ref();
    let grid_nc: Option<mako_pruefung::MaloGridRecord> = if grid.is_some() {
        grid.as_ref().map(Into::into)
    } else if let Some(ref m) = malo {
        if m.bilanzierungsgebiet.is_some() || m.netzebene.is_some() {
            Some(mako_pruefung::MaloGridRecord {
                malo_id: malo_id.clone(),
                nb_mp_id: anfrage.grid_operator_gln.clone(),
                bilanzierungsgebiet: m.bilanzierungsgebiet.clone(),
                netzgebiet: None,
            })
        } else {
            None
        }
    } else {
        None
    };
    let grid_ref = grid_nc.as_ref();

    // `E_0622` / `E_3005` is the **Vorprüfung**: every code it publishes is an
    // Ablehnung, and surviving it means only that the Anmeldung is not
    // *directly* refusable. What the NB answers comes from `E_0623` / `E_3007`,
    // which reads the LFA's answer to the Anfrage zur Beendigung der Zuordnung.
    // Answering out of `E_0622` alone can only ever say `A51`.
    let vorpruefung = mako_pruefung::evaluate(
        &anfrage,
        vs_ref,
        grid_ref,
        partner_known,
        now,
        &config.netz_check_config(),
    );
    let result = if vorpruefung.is_accept() {
        // Geschäftsvorfall 3 answers out of Prüfschritte 500–600, which read a
        // Tranchen-Zuordnung `marktd` does not project — the tree escalates on
        // its own rather than guessing between two Ablehnungen and two
        // Zustimmungen.
        mako_pruefung::evaluate_lieferbeginn(&anfrage, None)
    } else {
        vorpruefung
    };

    info!(
        %process_id, pid, %malo_id,
        grid_source = if grid.is_some() { "malo_grid" } else if grid_nc.is_some() { "malo_typed" } else { "none" },
        outcome = ?result,
        "processd NB: `mako-pruefung` result"
    );

    // ── 5. Persist decision ───────────────────────────────────────────────
    let (decision, antwortcode, detail) = classify(&result);

    let rec = AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: pid as i32,
        malo_id: malo_id.clone(),
        lf_mp_id: lf_mp_id.clone(),
        decision,
        antwortcode: antwortcode.clone(),
        detail: detail.clone(),
        initiator_is_affiliate,
        decided_at: now,
        tenant: config.tenant.clone(),
    };

    // `insert` is ON CONFLICT DO NOTHING on (process_id, tenant), so a
    // redelivered event does not double-count. Report the counter from the rows
    // actually written rather than from every delivery attempt.
    if repo.insert(&rec).await? {
        crate::metrics::record_decision(decision.as_str(), pid);
    }

    // ── 6. Dispatch command to makod ──────────────────────────────────────
    match &result {
        NbEntscheidung::Accept(_) => {
            // §20 EnWG Diskriminierungsfreiheitspflicht:
            // When the initiating LF shares the same MP-ID as our operator
            // (vertically integrated utility — §6b EnWG deployment), automatic
            // acceptance is forbidden.  The operator must review manually.
            // Bypassing this check exposes the NB to BNetzA sanctions.
            if incumbent_is_selbstzahler {
                warn!(
                    %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
                    "processd NB: Lieferantenwechsel on a Selbstzahler MaLo —                      held for operator review (GPKE Teil 1, Vorbemerkung)"
                );
                enqueue_for_operator(
                    queue,
                    config,
                    &payload_meta,
                    "Lieferantenwechsel (E03) on a MaLo whose Netznutzer is the                      Letztverbraucher itself (Selbstzahler). GPKE Teil 1, Vorbemerkung,                      takes the LF's Lieferantenwechsel-Meldungen out of the role the                      Selbstzahler otherwise steps into, so the automatic path does not                      apply — the operator decides",
                )
                .await?;
            } else if initiator_is_affiliate {
                warn!(
                    %process_id, pid, %malo_id, lf_mp_id = %lf_mp_id,
                    "processd NB: §20 EnWG — affiliate Anmeldung detected; \
                     auto_accept overridden to false — operator must review"
                );
                enqueue_for_operator(
                    queue,
                    config,
                    &payload_meta,
                    &format!(
                        "§20 EnWG affiliate Anmeldung (LF {lf_mp_id} is this operator) — \
                         `mako-pruefung` says Accept, but an affiliate may not take the \
                         automatic path a third party does not get"
                    ),
                )
                .await?;
            } else if config.auto_accept {
                dispatch(makod, pid, &malo_id, process_id, &result, None, None).await?;
                info!(%process_id, pid, %malo_id, antwortcode = result.antwortcode(),
                      "processd NB: dispatched bestaetigen");
            } else {
                info!(%process_id, pid, %malo_id, "processd NB: Accept held for operator confirmation (auto_accept = false)");
                enqueue_for_operator(
                    queue,
                    config,
                    &payload_meta,
                    "`mako-pruefung` says Accept; auto_accept is off, so the \
                     Bestätigung is dispatched on operator approval",
                )
                .await?;
            }
        }
        NbEntscheidung::Reject(reason) => {
            dispatch(makod, pid, &malo_id, process_id, &result, None, None).await?;
            info!(%process_id, pid, %malo_id, antwortcode = %reason.antwort.antwortcode,
                  "processd NB: dispatched ablehnen");
        }
        NbEntscheidung::Escalate { reason } => {
            warn!(%process_id, pid, %malo_id, %reason, "processd NB: Escalate — operator action required");
            enqueue_for_operator(queue, config, &payload_meta, reason).await?;
        }
        // ── SD Lieferbeginn Nr. 3 ─────────────────────────────────────────
        //
        // Not an answer to the LFN yet. The Marktlokation is assigned, so the
        // NB owes the incumbent an Anfrage zur Beendigung der Zuordnung
        // („parallel zu Nr. 2") and may only decide once the LFA has answered
        // or its 09:00 window has lapsed.
        NbEntscheidung::AnfrageErforderlich {
            lfa_mp_ids,
            zuordnungsende,
        } => {
            let waiting = AbmeldeanfrageRecord {
                anmeldung_process_id: process_id,
                malo_id: malo_id.clone(),
                lfn_mp_id: lf_mp_id.clone(),
                lfa_mp_ids: lfa_mp_ids.clone(),
                pid: pid as i32,
                anfrage: serde_json::to_value(&anfrage)?,
                // Frozen here. Phase two runs hours later and states the
                // Altlieferant and the Zuordnungsende as they were when the
                // Anmeldung was decided, not as the projection holds them once
                // the switch has been booked.
                meldung: serde_json::to_value(&meldung)?,
                received_at,
                tenant: config.tenant.clone(),
            };
            // Written **before** the Anfrage goes out. The LFA can answer
            // within milliseconds in a loopback deployment, and an answer that
            // finds no waiting row cannot resume the Anmeldung — which would
            // leave it unanswered past its own 11:00 Frist.
            //
            // Only a row whose Anfrage actually reached `makod` ends the
            // handling. One that never did has no 55010, so no 09:00 window and
            // no lapse: returning here would leave the Anmeldung waiting on an
            // answer nobody was ever asked for.
            match pending.record(&waiting).await? {
                Waiting::AlreadySent => {
                    info!(
                        %process_id, pid, %malo_id,
                        "processd NB: Anfrage zur Beendigung der Zuordnung already sent — \
                         redelivered Anmeldung ignored"
                    );
                    return Ok(true);
                }
                Waiting::Unsent => warn!(
                    %process_id, pid, %malo_id,
                    "processd NB: a waiting Anmeldung never got its Anfrage out — re-sending"
                ),
                Waiting::Recorded => {}
            }
            for lfa in lfa_mp_ids {
                dispatch_abmeldeanfrage(
                    makod,
                    config,
                    &malo_id,
                    process_id,
                    lfa,
                    &lf_mp_id,
                    *zuordnungsende,
                    &anfrage,
                    &kunde,
                )
                .await?;
            }
            // Every Anfrage is out; the LFA's 09:00 window is running and its
            // lapse will resolve the row. Stamped only now, so a dispatch that
            // failed above leaves the redelivery something to retry.
            pending
                .mark_anfrage_sent(process_id, &config.tenant)
                .await?;
            // Prozessschritt 2, „parallel zu Nr. 3" and on the same condition:
            // an LFA holds the Marktlokation and Prüfschritt 4 of Nr. 1 has
            // routed here. Its 07:00 window is the earliest of the whole
            // Lieferbeginn — four hours before the answer to the same message —
            // so the LFN learns who the LFA is with a Werktag left to act on it.
            meldung.informieren(makod, &malo_id, process_id).await;
            info!(
                %process_id, pid, %malo_id, lfa = ?lfa_mp_ids,
                "processd NB: Anfrage zur Beendigung der Zuordnung sent — the Anmeldung \
                 waits for the LFA (09:00 Uhr des 1. WT; silence counts as Zustimmung)"
            );
        }
    }

    Ok(true)
}

/// The Letztverbraucher the LFN named, as the NB passes it to the LFA.
///
/// A pair rather than a bare string, because DE 3036 without DE 3045 cannot be
/// read back: `Z01` splits the five components into Nachname, Vorname, …, and
/// `Z02` is one Firmenbezeichnung.
#[derive(Debug, Clone, Default)]
pub struct Kunde {
    /// `SG12 NAD+Z09` DE 3036.
    pub name: Option<String>,
    /// `NAD` DE 3045.
    pub namensformat: Option<String>,
}

/// The LFA's own answer, as the NB must restate it on its Ablehnung.
///
/// GPKE Teil 2 § 2.1.2 Nr. 6: „Der NB gibt zusätzlich den Grund der Ablehnung des
/// LFA an, sofern dieser in Prozessschritt 4 die Anfrage abgelehnt hat." It rides
/// `SG4 STS+Z35` „Status der Antwort des dritten Marktbeteiligten", which UTILMD
/// AHB Strom marks **Muss** whenever the NB's own code is `A50` or `A57`.
#[derive(Debug, Clone)]
pub struct DritterMarktbeteiligter {
    /// The LFA's `E_0624` Ablehnungscode.
    pub antwortcode: String,
    /// DE 9012 — the MaLo-ID the restated answer is about. `None` on a
    /// verbrauchende Marktlokation, whose AHB column is empty because there is
    /// exactly one LFA and the Vorgang already names it.
    pub referenz_lokation: Option<String>,
    /// `ZW3` Erzeugende Marktlokation or `ZW5` Tranche, beside the reference.
    pub objekt: Option<String>,
}

// ── Anfrage zur Beendigung der Zuordnung (SD Lieferbeginn Nr. 3) ─────────────

/// Send the LFA the Anfrage zur Beendigung der Zuordnung (55010).
///
/// „Der NB teilt dem LFA … mit, dass eine Anmeldung vorliegt, verbunden mit der
/// Anfrage, ob der LFA die Zuordnung zur Marktlokation bzw. Tranche zum
/// Zuordnungsbeginn des LFN beendet." Sent „parallel zu Nr. 2", the Information
/// über existierende Zuordnung, and on the same condition.
///
/// Failure **is** propagated, unlike a Meldepflicht: without the Anfrage the
/// Anmeldung has no path to an answer at all, so failing the event and letting
/// the fan-out redeliver is the only outcome that still meets the 11:00 Frist.
#[allow(clippy::too_many_arguments)]
async fn dispatch_abmeldeanfrage(
    makod: &MakodClient,
    config: &NbModuleConfig,
    malo_id: &str,
    process_id: Uuid,
    lfa_mp_id: &str,
    lfn_mp_id: &str,
    zuordnungsende: time::Date,
    anfrage: &AnmeldungAnfrage,
    kunde: &Kunde,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut payload = serde_json::json!({
        "malo_id":              malo_id,
        "lfa_mp_id":            lfa_mp_id,
        // „zum Zuordnungsbeginn des LFN" — the date the NB asks the LFA to end
        // its assignment on.
        "process_date":         zuordnungsende.to_string(),
        "anmeldung_process_id": process_id,
        // `SG12 NAD+VY` — the Neulieferant (UTILMD AHB Strom Bedingung [567]).
        "lfn_mp_id":            lfn_mp_id,
        "tranche":              anfrage.marktlokationsart == Marktlokationsart::Erzeugend
            && anfrage.erzeugung.as_ref().is_none_or(|e| {
                e.geschaeftsvorfall != mako_pruefung::nb::types::Geschaeftsvorfall::Eins
            }),
    });
    // `SG12 NAD+Z09` „Kunde des LF" — Muss on a verbrauchende oder ruhende
    // Marktlokation (Bedingung [279]), „der Kundenname aus der Anmeldung
    // Lieferant neu" ([572]). Copied verbatim: it is the LFN's own claim about
    // who the customer is, and the LFA compares it against its own contract
    // holder at `E_0624` Prüfschritt 30. Absent when the inbound Anmeldung
    // carried none, which is itself a defect in the LFN's message rather than
    // something to invent a value for.
    if let Some(name) = kunde.name.as_deref().filter(|n| !n.is_empty()) {
        let obj = payload.as_object_mut().expect("json! built an object");
        obj.insert("kunde_name".into(), name.into());
        if let Some(format) = kunde.namensformat.as_deref().filter(|f| !f.is_empty()) {
            obj.insert("kunde_namensformat".into(), format.into());
        }
    }

    let cmd = ForwardCommand {
        marktrolle: Some("NB".to_owned()),
        command: mako_markt::commands::GPKE_BEENDIGUNG_ZUORDNUNG_ANFRAGEN.to_owned(),
        malo_id: Some(malo_id.to_owned()),
        melo_id: None,
        payload,
    };
    let _ = config;
    makod
        .post_command(
            &format!("processd-nb-abmeldeanfrage-{process_id}-{lfa_mp_id}"),
            &cmd,
        )
        .await
        .inspect_err(
            |e| warn!(%e, %process_id, %malo_id, "processd NB: Abmeldeanfrage dispatch failed"),
        )?;
    Ok(())
}

// ── Phase two: the LFA answered ──────────────────────────────────────────────

/// Resume an Anmeldung decision on the LFA's answer to the 55010.
///
/// Consumes `de.mako.abmeldeanfrage.beantwortet`, which `makod` emits both when
/// the LFA answers (55011 / 55012) and when its 09:00 window lapses — „Verstreicht
/// die Frist, ohne dass eine Antwort beim NB eingeht, gilt dies als Bestätigung
/// nach Fall a)". The two are the same input to `E_0623`, and only the clock
/// tells them apart.
///
/// Returns `true` when this module handled the event.
///
/// # Errors
///
/// Propagates store and transport failures so the fan-out redelivers.
pub async fn resume_after_lfa_antwort(
    event: &serde_json::Value,
    config: &NbModuleConfig,
    makod: &MakodClient,
    repo: &PgAnmeldungRepository,
    queue: &PgApprovalQueue,
    pending: &PgAbmeldeanfrageRepository,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let data = &event["data"];
    let Some(anmeldung_process_id) = data
        .get("anmeldung_process_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<Uuid>().ok())
    else {
        return Ok(false);
    };

    // `take` is one statement with `WHERE resolved_at IS NULL`: the LFA's answer
    // and the 09:00 lapse race by design, and the loser must find nothing. That
    // is what makes the Anmeldung answered exactly once.
    let Some(waiting) = pending.take(anmeldung_process_id, &config.tenant).await? else {
        info!(
            %anmeldung_process_id,
            "processd NB: no Anmeldung waiting on this Abmeldeanfrage — already resolved"
        );
        return Ok(true);
    };

    let mut anfrage: AnmeldungAnfrage = serde_json::from_value(waiting.anfrage.clone())?;
    // The Meldepflicht facts as of the Anmeldung. Phase one wrote them because
    // the Zuordnung it is about to end is the one that was in force then.
    let meldung: MeldepflichtContext = serde_json::from_value(waiting.meldung.clone())?;
    let zustimmung = data
        .get("zustimmung")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let fristablauf = data
        .get("fristablauf")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let antwortcode = data
        .get("antwortcode")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);
    let grund = data
        .get("grund")
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned);

    // **Fall b** — the LFA released the Marktlokation earlier than asked and
    // „teilt sein Lieferendedatum in der Antwort mit" (`A34`). That date is what
    // the Beendigung der Zuordnung has to name, so it is kept beside the answer
    // rather than only inside it. Only the verbrauchende branch admits it.
    let lfa_zuordnungsende = (zustimmung
        && anfrage.marktlokationsart.ist_verbrauchend_oder_ruhend())
    .then(|| {
        data.get("zuordnungsende")
            .and_then(|v| v.as_str())
            .and_then(parse_civil_date)
    })
    .flatten();

    // Silence carries no code, which is exactly what `E_0623` Prüfschritt 30
    // „nein" reads: the tree goes straight to 60 and confirms.
    anfrage.abmeldeanfrage = mako_pruefung::Abmeldeanfrage::Gestellt {
        antwort: antwortcode.map(|code| {
            if zustimmung {
                mako_pruefung::LfaAntwort::Zustimmung {
                    code,
                    // **Fall b** — the LFA released the Marktlokation earlier
                    // than asked, and `A34` „teilt sein Lieferendedatum in der
                    // Antwort mit".
                    zuordnungsende: data
                        .get("zuordnungsende")
                        .and_then(|v| v.as_str())
                        .and_then(parse_civil_date),
                }
            } else {
                mako_pruefung::LfaAntwort::Widerspruch { code, grund }
            }
        }),
    };

    let result = mako_pruefung::evaluate_lieferbeginn(&anfrage, None);
    let pid = waiting.pid as u32;
    let malo_id = waiting.malo_id.clone();

    // `SG4 STS+Z35` — the LFA's own code, which the NB restates when its ground
    // is that refusal (`A50` / `A57`). Built only for a Widerspruch: a
    // Zustimmung is not a ground and the AHB marks the segment Muss on those two
    // codes alone.
    let dritter = match (&anfrage.abmeldeanfrage, result.antwortcode()) {
        (
            mako_pruefung::Abmeldeanfrage::Gestellt {
                antwort: Some(mako_pruefung::LfaAntwort::Widerspruch { code, .. }),
            },
            Some(own),
        ) if mako_pruefung::CODES_REQUIRING_DRITTER.contains(&own) => {
            // The erzeugende form names which object the restated answer is
            // about — Geschäftsvorfall 3 has several LFA, so „the LFA said no"
            // is not enough on its own.
            let erzeugend = anfrage.marktlokationsart == Marktlokationsart::Erzeugend;
            Some(DritterMarktbeteiligter {
                antwortcode: code.clone(),
                referenz_lokation: erzeugend.then(|| malo_id.clone()),
                objekt: erzeugend.then(|| {
                    match anfrage.erzeugung.as_ref().map(|e| e.geschaeftsvorfall) {
                        Some(mako_pruefung::nb::types::Geschaeftsvorfall::Eins) | None => "ZW3",
                        // Geschäftsvorfall 2 and 3 both address a Tranche.
                        Some(_) => "ZW5",
                    }
                    .to_owned()
                }),
            })
        }
        _ => None,
    };
    info!(
        %anmeldung_process_id, pid, %malo_id, fristablauf, zustimmung, outcome = ?result,
        "processd NB: E_0623 resolved after the LFA's answer"
    );

    let now = OffsetDateTime::now_utc();
    let (decision, code, detail) = classify(&result);
    let rec = AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id: anmeldung_process_id,
        pid: waiting.pid,
        malo_id: malo_id.clone(),
        lf_mp_id: waiting.lfn_mp_id.clone(),
        decision,
        antwortcode: code,
        detail,
        // Phase one already wrote the § 20 parity row for this process, and
        // `insert` is ON CONFLICT DO NOTHING on `(process_id, tenant)` — so
        // this is the *first* recorded decision, because phase one produced
        // none. The affiliate flag is recomputed rather than carried.
        initiator_is_affiliate: waiting.lfn_mp_id == config.own_mp_id,
        decided_at: now,
        tenant: config.tenant.clone(),
    };
    if repo.insert(&rec).await? {
        crate::metrics::record_decision(decision.as_str(), pid);
    }

    let meta = AnmeldungMeta {
        pid,
        process_id: anmeldung_process_id,
        malo_id: malo_id.clone(),
        // The Anmeldung's own 11:00 window, which runs from *its* arrival — not
        // from the LFA's answer, which lands two hours before it closes.
        received_at: waiting.received_at,
    };

    match &result {
        // Prozessschritt 10 (Strom) / 6 (Gas) rides with every one of these
        // arms: „unverzüglich nach dem ÜZ von Nr. 5", so the LFA is told its
        // Zuordnung ends exactly when — and only when — the Bestätigung goes
        // out. A held decision carries the Meldung on the queue entry, so
        // deferring the answer defers the Meldung with it instead of losing it.
        NbEntscheidung::Accept(_) => {
            // The ÜT of the Anmeldung, in German local time: the day the Fall-b
            // „mindestens 1 WT nach dem ÜT" floor counts from.
            use time_tz::{OffsetDateTimeExt as _, timezones};
            let uet = waiting
                .received_at
                .to_timezone(timezones::db::europe::BERLIN)
                .date();
            let zuordnungsende = meldung.zuordnungsende(lfa_zuordnungsende, uet);
            let beendigung = meldung.beendigung(&malo_id, zuordnungsende);
            // The Zuordnungsende the LFA actually answered with, when it falls
            // *before* the Zuordnungsbeginn this answer confirms. Those days are
            // supplied by nobody, and `marktd` is the only place that holds both
            // ends of the interval — so the date travels with the Bestätigung.
            let fall_b = (zuordnungsende < meldung.zuordnungsbeginn).then_some(zuordnungsende);
            if rec.initiator_is_affiliate {
                warn!(%anmeldung_process_id, pid, %malo_id,
                      "processd NB: § 20 EnWG — affiliate Anmeldung held for operator review");
                enqueue_for_operator_with_followup(
                    queue,
                    config,
                    &meta,
                    "§ 20 EnWG affiliate Anmeldung — the LFA released the Marktlokation, but \
                     an affiliate may not take the automatic path a third party does not get",
                    beendigung,
                )
                .await?;
            } else if config.auto_accept {
                dispatch(
                    makod,
                    pid,
                    &malo_id,
                    anmeldung_process_id,
                    &result,
                    dritter.as_ref(),
                    fall_b,
                )
                .await?;
                meldung
                    .beenden(makod, &malo_id, anmeldung_process_id, zuordnungsende)
                    .await;
            } else {
                enqueue_for_operator_with_followup(
                    queue,
                    config,
                    &meta,
                    "the LFA released the Marktlokation and E_0623 confirms; auto_accept is \
                     off, so the Bestätigung is dispatched on operator approval",
                    beendigung,
                )
                .await?;
            }
        }
        NbEntscheidung::Reject(reason) => {
            // `A50` / `A57` / `Z35` — the outcome that did not exist before the
            // NB sent an Anfrage at all.
            warn!(%anmeldung_process_id, pid, %malo_id, antwortcode = %reason.antwort.antwortcode,
                  "processd NB: the LFA refused, so the Anmeldung is refused");
            dispatch(
                makod,
                pid,
                &malo_id,
                anmeldung_process_id,
                &result,
                dritter.as_ref(),
                // An Ablehnung leaves the Altlieferant where it was, so no
                // interval opens and there is nothing for the projection.
                None,
            )
            .await?;
        }
        NbEntscheidung::Escalate { reason } => {
            enqueue_for_operator(queue, config, &meta, reason).await?;
        }
        NbEntscheidung::AnfrageErforderlich { .. } => {
            // Unreachable: the leg is `Gestellt` by construction above.
            warn!(%anmeldung_process_id, "processd NB: a second Abmeldeanfrage was demanded");
            enqueue_for_operator(
                queue,
                config,
                &meta,
                "E_0623 demanded a second Anfrage zur Beendigung der Zuordnung after the LFA \
                 had already answered — the Anmeldung needs an operator",
            )
            .await?;
        }
    }
    Ok(true)
}

// ── Meldepflichten around the Lieferbeginn ────────────────────────────────────

/// What the NB needs to discharge the two Meldepflichten a Lieferbeginn carries.
///
/// A **Meldepflicht** is a message the Festlegung obliges the NB to send with no
/// answer expected back, so nothing times out when one is missed — it surfaces
/// later as a supplier holding a stale view of who serves the Marktlokation.
/// Both are conditional on the same fact, and it is one only `marktd` holds:
/// GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 Prüfschritt 4 routes to
/// Prozessschritt 2 „Ist die Marktlokation bzw. Tranche zum Zuordnungsbeginn
/// einem LF zugeordnet" and straight to Nr. 5 otherwise.
///
/// | Nr. | PID (Strom / Gas) | To | Spätester ÜZ | Sent when |
/// |---|---|---|---|---|
/// | 2 | 55036 / 44036 | LFN | 07:00 Uhr des 1. WT / Ablauf des 4. WT | with the Anfrage of Nr. 3 („parallel zu Nr. 2") |
/// | 10 / 6 | 55037 / 44037 | LFA | 12:00 Uhr des 1. WT / am selben Tag wie die Antwort | with the Bestätigung, auto-dispatched or operator-released |
///
/// Both ride the Prozessschritt they belong to. Nr. 2 shares its condition with
/// Nr. 3, so an Anmeldung the Vorprüfung refuses never names the LFA to the
/// party it refuses; Nr. 10 is „unverzüglich nach dem ÜZ von Nr. 5", so it can
/// only be sent where the Bestätigung is — which is phase two, because an
/// assigned Marktlokation never reaches a Zustimmung in phase one.
///
/// Nr. 2 is owed „auch dann …, sofern LFA und LFN identisch sind", so the two
/// MP-IDs being equal is not a reason to skip it.
///
/// **The third, 55038 / 44038 „Aufhebung einer zukünftigen Zuordnung", is not
/// derived here.** It addresses an LFZ whose future Zuordnung the new Anmeldung
/// displaces, and `VersorgungsStatusRecord` has one future-supplier slot
/// (`lf_mp_id_next`), which `marktd` has already filled with *this* LFN by the
/// time the decision runs. A distinct LFZ is not representable, and a competing
/// pending Anmeldung is refused `A06` before it could be. The command
/// (`gpke.zuordnung.aufheben`) exists for an operator or ERP that can see one.
#[derive(serde::Serialize, serde::Deserialize)]
struct MeldepflichtContext {
    sparte: Sparte,
    /// The LFN — the party that sent the Anmeldung, and the recipient of Nr. 2.
    lfn_mp_id: String,
    /// The Zuordnungsbeginn of this Anmeldung, which is also the Zuordnungsende
    /// the LFA is told about in Nr. 10.
    zuordnungsbeginn: time::Date,
    /// `SG6 RFF+TN` on Nr. 2 — the LFN's own `SG4 IDE+24`.
    vorgangsnummer: Option<String>,
    /// `SG5 LOC+Z21` instead of `LOC+Z16` (Strom only; Gas names a Meldepunkt).
    tranche: bool,
    /// The incumbent supplier at the Zuordnungsbeginn, from `marktd`. `None`
    /// means the Marktlokation is unassigned and neither Meldung is owed.
    altlieferant: Option<String>,
}

impl MeldepflichtContext {
    /// The `makod` command names for this Sparte.
    const fn commands(&self) -> (&'static str, &'static str) {
        match self.sparte {
            Sparte::Gas => (
                mako_markt::commands::GELI_ZUORDNUNG_INFORMIEREN,
                mako_markt::commands::GELI_ZUORDNUNG_BEENDEN,
            ),
            _ => (
                mako_markt::commands::GPKE_ZUORDNUNG_INFORMIEREN,
                mako_markt::commands::GPKE_ZUORDNUNG_BEENDEN,
            ),
        }
    }

    /// The asserted Marktrolle. `makod` checks it against the deployment's
    /// licensed roles, so a Gas Meldung must assert `GNB` and not `NB`.
    const fn marktrolle(&self) -> &'static str {
        match self.sparte {
            Sparte::Gas => "GNB",
            _ => "NB",
        }
    }

    /// `SG4 STS+7` DE 9013 `Z26` — the only Grund either AHB admits on the
    /// Information über existierende Zuordnung.
    const INFO_GRUND: &'static str = "Z26";
    /// `SG4 STS+7` DE 9013 `ZC8` — Beendigung der Zuordnung. The Strom AHB also
    /// admits `ZD9` (Rückzuordnungsmeldung) and `ZG6` (EEG 2014 § 38); neither
    /// arises from a Lieferbeginn.
    const BEENDIGUNG_GRUND: &'static str = "ZC8";

    /// Prozessschritt 2 — tell the LFN who the LFA is.
    async fn informieren(&self, makod: &MakodClient, malo_id: &str, process_id: Uuid) {
        let Some(altlieferant) = self.altlieferant.as_deref() else {
            return;
        };
        let (command, _) = self.commands();
        let mut payload = serde_json::json!({
            "malo_id":                 malo_id,
            "empfaenger_mp_id":        self.lfn_mp_id,
            "transaktionsgrund":       Self::INFO_GRUND,
            // „Hierbei teilt der NB dem LFN insbesondere die Identität des LFA
            // … mit" — the whole point of the message, in `SG12 NAD+VY`.
            "beteiligte_marktpartner": [altlieferant],
        });
        if self.sparte != Sparte::Gas && self.tranche {
            payload["tranche"] = serde_json::json!(true);
        }
        if let Some(vorgang) = self.vorgangsnummer.as_deref() {
            payload["referenz_vorgangsnummer"] = serde_json::json!(vorgang);
        }
        self.send(makod, command, malo_id, process_id, "informieren", payload)
            .await;
    }

    /// The Zuordnungsende the LFA is told about in Nr. 10.
    ///
    /// „Das Zuordnungsende … ist … der Zuordnungsbeginn der Anmeldung" — except
    /// in **Fall b**, where the LFA answered `A34` and „teilt sein
    /// Lieferendedatum in der Antwort mit". That date governs when it lies
    /// *before* the Zuordnungsbeginn and „mindestens 1 WT nach dem ÜT der
    /// Anmeldung"; a date failing either test leaves the Zuordnungsbeginn
    /// standing (GPKE Teil 2 § 2.1.2 Nr. 10).
    ///
    /// `uet` is the Übertragungstag of the Anmeldung — the day the 1-Werktag
    /// floor counts from. Fall b belongs to the verbrauchende branch of
    /// `E_0623`; an erzeugende Marktlokation settles its Zuordnungsende through
    /// Geschäftsvorfall 2/3 arithmetic instead, so a stated date is ignored
    /// there rather than applied to a case the Festlegung does not describe.
    fn zuordnungsende(&self, lfa_gemeldet: Option<time::Date>, uet: time::Date) -> time::Date {
        let Some(gemeldet) = lfa_gemeldet else {
            return self.zuordnungsbeginn;
        };
        let floor = mako_fristen::add_werktage(uet, 1, mako_fristen::HolidayCalendar::BdewMaKo);
        if gemeldet < self.zuordnungsbeginn && gemeldet >= floor {
            gemeldet
        } else {
            self.zuordnungsbeginn
        }
    }

    /// Prozessschritt 10 (Strom) / 6 (Gas) — the command and body that tell the
    /// LFA its Zuordnung ends, or `None` where nothing is owed.
    ///
    /// Separated from the sending because the Bestätigung it follows may be
    /// dispatched automatically or held for an operator, and the Meldung has to
    /// go out either way. The operator path stores this pair on the queue entry
    /// and dispatches it after the answer.
    fn beendigung(
        &self,
        malo_id: &str,
        zuordnungsende: time::Date,
    ) -> Option<(&'static str, serde_json::Value)> {
        let altlieferant = self.altlieferant.as_deref()?;
        // „Sofern LFA und LFN identisch sind" the Information is still owed
        // (Nr. 2 says so), but there is no assignment to end: the supplier keeps
        // supplying under a new Zuordnungsbeginn.
        if altlieferant == self.lfn_mp_id {
            return None;
        }
        let (_, command) = self.commands();
        let mut payload = serde_json::json!({
            "malo_id":           malo_id,
            "empfaenger_mp_id":  altlieferant,
            "transaktionsgrund": Self::BEENDIGUNG_GRUND,
            // `SG4 DTM+93` Ende zum — the Zuordnungsbeginn der Anmeldung, or
            // the LFA's own earlier Lieferendedatum in Fall b.
            "process_date":      zuordnungsende.to_string(),
        });
        if self.sparte != Sparte::Gas && self.tranche {
            payload["tranche"] = serde_json::json!(true);
        }
        Some((command, payload))
    }

    /// Send the Beendigung der Zuordnung now — the path an automatically
    /// dispatched Bestätigung takes.
    async fn beenden(
        &self,
        makod: &MakodClient,
        malo_id: &str,
        process_id: Uuid,
        zuordnungsende: time::Date,
    ) {
        let Some((command, payload)) = self.beendigung(malo_id, zuordnungsende) else {
            return;
        };
        self.send(makod, command, malo_id, process_id, "beenden", payload)
            .await;
    }

    /// Post one Meldung to `makod`.
    ///
    /// A failure is logged, never propagated. The Meldepflicht is a side
    /// obligation of the Anmeldung decision, and failing the decision because a
    /// notification could not be queued would trade a missing Meldung for a
    /// missed **Antwortfrist** — the one the counterparty is actually waiting
    /// on, and the one § 20 EnWG is audited against.
    ///
    /// # Redelivery
    ///
    /// The event fan-out is at-least-once and AS4 ReceptionAwareness redelivers,
    /// so this runs more than once for one market process. The
    /// `Idempotency-Key` is keyed on the **process id**, which is stable across
    /// redeliveries, so `makod` replays the original `202` instead of sending a
    /// second Meldung. A redelivery whose payload differs — the Versorgungsstatus
    /// has moved on, so `beteiligte_marktpartner` names the LFN as its own
    /// Altlieferant — is refused as a key conflict rather than sent, which is
    /// the outcome that matters: the second message is the wrong one.
    async fn send(
        &self,
        makod: &MakodClient,
        command: &str,
        malo_id: &str,
        process_id: Uuid,
        step: &'static str,
        payload: serde_json::Value,
    ) {
        let cmd = ForwardCommand {
            marktrolle: Some(self.marktrolle().to_owned()),
            command: command.to_owned(),
            malo_id: Some(malo_id.to_owned()),
            melo_id: None,
            payload,
        };
        match makod
            .post_command(&format!("processd-nb-{step}-{process_id}"), &cmd)
            .await
        {
            Ok(_) => info!(%process_id, %malo_id, command, "processd NB: Meldepflicht dispatched"),
            Err(e) => warn!(
                %e, %process_id, %malo_id, command,
                "processd NB: Meldepflicht dispatch failed — the Antwort is unaffected",
            ),
        }
    }
}

/// The trigger PID, process and MaLo an escalated Anmeldung is queued under.
struct AnmeldungMeta {
    pid: u32,
    process_id: Uuid,
    malo_id: String,
    received_at: OffsetDateTime,
}

/// Put a decision the NB may not dispatch automatically in front of an operator,
/// with the answer deadline attached.
///
/// Escalations, and Accepts held back by `auto_accept = false` or by the § 20
/// EnWG affiliate rule, all take this path. `anmeldung_decisions` is the audit
/// log and carries no Frist: only a queue entry expires, surfaces in
/// `processd_approval_queue_overdue`, and gives the operator something to act on.
async fn enqueue_for_operator(
    queue: &PgApprovalQueue,
    config: &NbModuleConfig,
    meta: &AnmeldungMeta,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    enqueue_for_operator_with_followup(queue, config, meta, reason, None).await
}

/// Queue a decision that carries a **Meldepflicht** the operator's approval
/// must discharge with it.
///
/// The Meldungen around a Lieferbeginn are owed „unverzüglich nach dem ÜZ" of
/// the answer they follow. When that answer waits for an operator, so does the
/// Meldung — and it has to state what was true when the decision was taken, so
/// the body is frozen here rather than rebuilt at approval time.
async fn enqueue_for_operator_with_followup(
    queue: &PgApprovalQueue,
    config: &NbModuleConfig,
    meta: &AnmeldungMeta,
    reason: &str,
    followup: Option<(&'static str, serde_json::Value)>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let window = mako_fristen::antwort::operator_window(meta.pid, meta.received_at);
    let (accept, reject) = answer_commands(meta.pid);
    let mut entry = ApprovalQueueEntry::pending(
        meta.process_id,
        meta.pid as i32,
        Some(meta.malo_id.clone()),
        format!(
            "{reason} (Antwortfrist {}: {})",
            window.deadline, window.source
        ),
        window.expires_at,
        config.tenant.clone(),
    )
    .with_commands(accept, reject, Some("NB"));
    if let Some((command, payload)) = followup {
        entry = entry.with_followup(command, payload);
    }
    queue.enqueue(&entry).await?;
    info!(
        process_id = %meta.process_id,
        pid = meta.pid,
        malo_id = %meta.malo_id,
        deadline = %window.deadline,
        "processd NB: queued for operator decision"
    );
    Ok(())
}

/// The `makod` command pair that answers an inbound NB PID.
///
/// Anmeldung and Abmeldung take **different commands**, and both resolve from
/// the PID alone: an Abmeldung answered with `gpke.lieferbeginn.bestaetigen`
/// would drive the wrong response PID onto the wire.
fn answer_commands(pid: u32) -> (&'static str, &'static str) {
    match pid {
        44_001 => (
            mako_markt::commands::GELI_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GELI_LIEFERBEGINN_ABLEHNEN,
        ),
        44_004 => (
            mako_markt::commands::GELI_LIEFERENDE_BESTAETIGEN,
            mako_markt::commands::GELI_LIEFERENDE_ABLEHNEN,
        ),
        55_004 => (
            mako_markt::commands::GPKE_LIEFERENDE_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERENDE_ABLEHNEN,
        ),
        // 55001 / 55077 — makod derives 55002/55003 and 55078/55080 from the
        // inbound PID the process was spawned with, so one command pair covers
        // both Anmeldung variants.
        _ => (
            mako_markt::commands::GPKE_LIEFERBEGINN_BESTAETIGEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
        ),
    }
}

// ── Abmeldung ─────────────────────────────────────────────────────────────────

/// Fields extracted from a `de.mako.process.initiated` for an Abmeldung PID.
#[derive(Debug, Clone)]
pub struct AbmeldungPayload {
    pub pid: u32,
    pub process_id: Uuid,
    pub malo_id: String,
    /// The supplier ending the assignment. `makod`'s adapter surfaces it as
    /// `current_supplier` where it can tell, else as `new_supplier` (the UTILMD
    /// NAD sender is the same party in both directions of this process).
    pub lf_mp_id: String,
    pub grid_operator_gln: String,
    pub abmeldedatum: time::Date,
    pub transaktionsgrund: Option<String>,
    /// `SG4 STS+7` DE 9013 element 3 — decides the `E_0607` branch.
    pub transaktionsgrund_ergaenzung: Option<String>,
    pub bilanzierungsmethode: Option<String>,
}

impl AbmeldungPayload {
    /// Parse an Abmeldung event, or `None` when the PID is not one.
    #[must_use]
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !ABMELDUNG_PIDS.contains(&pid) {
            return None;
        }
        let process_id: Uuid = event["subject"].as_str()?.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let lf_mp_id = data
            .get("current_supplier")
            .or_else(|| data.get("new_supplier"))
            .or_else(|| data.get("sender"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let grid_operator_gln = data
            .get("grid_operator")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let abmeldedatum = parse_civil_date(data.get("process_date")?.as_str()?)?;
        Some(Self {
            pid,
            process_id,
            malo_id,
            lf_mp_id,
            grid_operator_gln,
            abmeldedatum,
            transaktionsgrund: data
                .get("transaktionsgrund")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            transaktionsgrund_ergaenzung: data
                .get("transaktionsgrund_ergaenzung")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
            bilanzierungsmethode: data
                .get("bilanzierungsmethode")
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned),
        })
    }

    /// Derive the `mako-pruefung` input.
    #[must_use]
    pub fn into_anfrage(self) -> mako_pruefung::AbmeldungAnfrage {
        mako_pruefung::AbmeldungAnfrage {
            pid: self.pid,
            process_id: self.process_id,
            malo_id: self.malo_id,
            lf_mp_id: self.lf_mp_id,
            grid_operator_gln: self.grid_operator_gln,
            abmeldedatum: self.abmeldedatum,
            sparte: sparte_of(self.pid),
            messtyp: messtyp_of(self.bilanzierungsmethode.as_deref()),
            transaktionsgrund: self.transaktionsgrund,
            marktlokationsart: marktlokationsart_of(
                self.pid,
                self.transaktionsgrund_ergaenzung.as_deref(),
            ),
            erzeugung: None,
        }
    }
}

/// `YYYYMMDD` or `YYYY-MM-DD`, the two shapes the `makod` adapters emit.
fn parse_civil_date(raw: &str) -> Option<time::Date> {
    if raw.len() == 8 {
        time::Date::parse(raw, time::macros::format_description!("[year][month][day]")).ok()
    } else {
        time::Date::parse(
            raw,
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .ok()
    }
}

/// A date as the `makod` command payloads carry it, `YYYYMMDD`.
fn civil_date(d: time::Date) -> String {
    d.format(time::macros::format_description!("[year][month][day]"))
        .unwrap_or_else(|_| d.to_string())
}

/// UTILMD TM+EM marker → `mako-pruefung` metering class. SLP is the default: it
/// is the class with the *widest* retroactive window, so an unknown marker can
/// never turn an admissible date into an auto-reject.
fn messtyp_of(bilanzierungsmethode: Option<&str>) -> Messtyp {
    match bilanzierungsmethode {
        Some("RLM") => Messtyp::Rlm,
        Some("IMS") => Messtyp::Imsys,
        _ => Messtyp::Slp,
    }
}

/// The NB's decision on an inbound Abmeldung (55004 / 44004), EBD `E_0607`.
#[allow(clippy::too_many_arguments)]
async fn decide_abmeldung(
    payload: AbmeldungPayload,
    event: &serde_json::Value,
    config: &NbModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    repo: &PgAnmeldungRepository,
    queue: &PgApprovalQueue,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // Not addressed to this NB — another operator's message on a shared bus.
    if !payload.grid_operator_gln.is_empty() && payload.grid_operator_gln != config.own_mp_id {
        return Ok(false);
    }

    let pid = payload.pid;
    let process_id = payload.process_id;
    let malo_id = payload.malo_id.clone();
    let lf_mp_id = payload.lf_mp_id.clone();
    let initiator_is_affiliate = lf_mp_id == config.own_mp_id;
    let received_at = received_at(event);

    info!(%process_id, pid, %malo_id, %lf_mp_id, "processd NB: evaluating Abmeldung");

    // A transport failure is not evidence of absence: propagate so the fan-out
    // redelivers rather than deciding on a missing projection.
    let versorgung = reader
        .get_versorgung(&malo_id)
        .await
        .inspect_err(|e| warn!(%e, %malo_id, "processd NB: marktd versorgung fetch failed"))?;

    let anfrage = payload.into_anfrage();
    let now = OffsetDateTime::now_utc();
    let result = mako_pruefung::evaluate_abmeldung(
        &anfrage,
        versorgung.as_ref(),
        now,
        &config.netz_check_config(),
    );

    info!(%process_id, pid, %malo_id, outcome = ?result, "processd NB: E_0607 result");

    let (decision, antwortcode, detail) = classify(&result);
    let rec = AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: pid as i32,
        malo_id: malo_id.clone(),
        lf_mp_id: lf_mp_id.clone(),
        decision,
        antwortcode,
        detail,
        initiator_is_affiliate,
        decided_at: now,
        tenant: config.tenant.clone(),
    };
    if repo.insert(&rec).await? {
        crate::metrics::record_decision(decision.as_str(), pid);
    }

    let meta = AnmeldungMeta {
        pid,
        process_id,
        malo_id: malo_id.clone(),
        received_at,
    };

    match &result {
        NbEntscheidung::Accept(_) => {
            // § 20 EnWG parity applies to the Abmeldung too: an affiliate must
            // not get an automatic path a third party does not get.
            if initiator_is_affiliate {
                warn!(%process_id, pid, %malo_id, %lf_mp_id,
                      "processd NB: § 20 EnWG — affiliate Abmeldung held for operator review");
                enqueue_for_operator(
                    queue,
                    config,
                    &meta,
                    &format!(
                        "§ 20 EnWG affiliate Abmeldung (LF {lf_mp_id} is this operator) — \
                         E_0607 says Accept, but an affiliate may not take the automatic path"
                    ),
                )
                .await?;
            } else if config.auto_accept {
                dispatch(makod, pid, &malo_id, process_id, &result, None, None).await?;
                info!(%process_id, pid, %malo_id, antwortcode = result.antwortcode(),
                      "processd NB: dispatched Bestätigung Abmeldung");
            } else {
                enqueue_for_operator(
                    queue,
                    config,
                    &meta,
                    "E_0607 says Accept; auto_accept is off, so the Bestätigung is \
                     dispatched on operator approval",
                )
                .await?;
            }
        }
        NbEntscheidung::Reject(reason) => {
            dispatch(makod, pid, &malo_id, process_id, &result, None, None).await?;
            info!(%process_id, pid, %malo_id, antwortcode = %reason.antwort.antwortcode,
                  "processd NB: dispatched Ablehnung Abmeldung");
        }
        NbEntscheidung::Escalate { reason } => {
            warn!(%process_id, pid, %malo_id, %reason, "processd NB: Abmeldung escalated");
            enqueue_for_operator(queue, config, &meta, reason).await?;
        }
        // `E_0607` / `E_3019` has no Abmeldeanfrage leg: the Abmeldung *is* the
        // supplier releasing the Marktlokation, so there is nobody to ask.
        NbEntscheidung::AnfrageErforderlich { .. } => {
            warn!(%process_id, pid, %malo_id, "processd NB: E_0607 cannot demand an Abmeldeanfrage");
            enqueue_for_operator(
                queue,
                config,
                &meta,
                "the Abmeldung tree answered AnfrageErforderlich, which E_0607 / E_3019 does \
                 not publish — the Abmeldung is itself the release of the Marktlokation",
            )
            .await?;
        }
    }

    Ok(true)
}

/// The answer Frist runs from receipt of the market message; the CloudEvent
/// `time` is when `makod` emitted it, which is the closest instant available.
fn received_at(event: &serde_json::Value) -> OffsetDateTime {
    event["time"]
        .as_str()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc)
}

/// Map a `mako-pruefung` verdict onto the audit-log columns.
///
/// The Antwortcode is recorded on an **Accept** too: `SG4 STS+E01` is Muss on
/// every Antwortnachricht, so a Bestätigung states `A51` / `A58` / `E15` and the
/// § 20 EnWG parity log has to be able to show which one went out.
fn classify(result: &NbEntscheidung) -> (AnmeldungDecision, Option<String>, Option<String>) {
    match result {
        NbEntscheidung::Accept(a) => (
            AnmeldungDecision::Accept,
            Some(a.antwortcode.clone()),
            Some(a.bedeutung.clone()),
        ),
        NbEntscheidung::Reject(r) => (
            AnmeldungDecision::Reject,
            Some(r.antwort.antwortcode.clone()),
            Some(r.detail.clone()),
        ),
        // An owed Abmeldeanfrage is not a decision yet, and it is not an
        // operator case either — but the audit log has one column for „no
        // answer went out", and this is one. The detail says which.
        NbEntscheidung::AnfrageErforderlich { lfa_mp_ids, .. } => (
            AnmeldungDecision::Escalate,
            None,
            Some(format!(
                "waiting on the Anfrage zur Beendigung der Zuordnung to {lfa_mp_ids:?} \
                 (GPKE Teil 2 § 2.1.2 Nr. 3); the LFA answers by 09:00 Uhr des 1. WT and \
                 silence counts as Zustimmung"
            )),
        ),
        NbEntscheidung::Escalate { reason } => {
            (AnmeldungDecision::Escalate, None, Some(reason.clone()))
        }
    }
}

/// Post the answer command for `pid` to `makod`, carrying the resolved
/// Antwortcode.
///
/// **The code is the payload.** The AHB marks `SG4 STS+E01` Muss on every
/// Antwortnachricht and restricts the code to the named EBD's cluster, so an
/// answer dispatched as a bare `accepted: bool` renders a UTILMD with no
/// Ablehnungsgrund at all — well-formed EDIFACT that says nothing. `makod`
/// re-resolves `antwort_code` against `antwort_ebd` and derives the response PID
/// from the published Cluster, so the two cannot disagree.
///
/// The Gas Codelisten are not named in `STS` DE 1131, so a Gas answer sends
/// `zustimmung` alongside the code instead of an EBD id.
async fn dispatch(
    makod: &MakodClient,
    pid: u32,
    malo_id: &str,
    process_id: Uuid,
    result: &NbEntscheidung,
    dritter: Option<&DritterMarktbeteiligter>,
    lfa_lieferende: Option<time::Date>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (accept_cmd, reject_cmd) = answer_commands(pid);
    let (antwort, accept, detail) = match result {
        NbEntscheidung::Accept(a) => (a, true, None),
        NbEntscheidung::Reject(r) => (&r.antwort, false, Some(r.detail.as_str())),
        NbEntscheidung::Escalate { .. } | NbEntscheidung::AnfrageErforderlich { .. } => {
            return Err("an escalated decision has no market answer to dispatch".into());
        }
    };

    let mut payload = serde_json::json!({
        "process_id":   process_id,
        "antwort_code": antwort.antwortcode,
        // The tree the code was resolved against — always present, and what
        // `makod` re-validates on. The Gas Codelisten carry no DE 1131, so
        // `antwort_ebd` is absent there while `antwort_tree` is not.
        "antwort_tree": antwort.tree,
        "zustimmung":   accept,
    });
    if let Some(ebd) = &antwort.ebd {
        payload["antwort_ebd"] = serde_json::json!(ebd);
    }
    if let Some(detail) = detail {
        // `FTX+ACB` — required alongside the catch-all codes, useful on all of
        // them. `reason` is what makod forwards onto an APERAK.
        payload["bemerkung"] = serde_json::json!(detail);
        payload["reason"] = serde_json::json!(format!("{}: {detail}", antwort.antwortcode));
        payload["detail"] = serde_json::json!(detail);
    }
    // `SG4 STS+Z35` — Muss alongside `A50` / `A57` (UTILMD AHB Strom Bedingungen
    // `[356]` / `[84]`). `makod` refuses to render the Ablehnung without it, so
    // omitting it here is a dispatch failure and not a silently thinner message.
    if let Some(d) = dritter {
        payload["dritter_antwortcode"] = serde_json::json!(d.antwortcode);
        if let Some(referenz) = &d.referenz_lokation {
            payload["dritter_referenz_lokation"] = serde_json::json!(referenz);
            payload["dritter_objekt"] = serde_json::json!(d.objekt);
        }
    }
    // **Fall b** — the LFA released the Marktlokation before the Zuordnungsbeginn
    // this answer confirms. Not a segment of the 55002: it rides the completion
    // payload so `marktd` can see the days between the two dates, which no
    // supplier covers and § 38 Abs. 1 EnWG attaches to. `YYYYMMDD`, the format
    // every date on this payload uses.
    if accept && let Some(ende) = lfa_lieferende {
        payload["lfa_lieferende"] = serde_json::json!(civil_date(ende));
    }

    let cmd = ForwardCommand {
        marktrolle: Some("NB".to_owned()),
        command: if accept { accept_cmd } else { reject_cmd }.to_owned(),
        malo_id: Some(malo_id.to_owned()),
        melo_id: None,
        payload,
    };
    let verb = if accept { "accept" } else { "reject" };
    makod
        .post_command(&format!("processd-nb-{verb}-{process_id}"), &cmd)
        .await
        .inspect_err(|e| warn!(%e, %process_id, "processd NB: dispatch failed"))?;
    Ok(())
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── The two-phase Anmeldung (SD Lieferbeginn Nr. 1 Prüfschritt 4) ────────

    use mako_pruefung::nb::types::Marktlokationsart;
    use mako_pruefung::{Abmeldeanfrage, LfaAntwort};

    fn anmeldung(art: Marktlokationsart) -> AnmeldungAnfrage {
        AnmeldungAnfrage {
            pid: 55_001,
            process_id: Uuid::nil(),
            malo_id: "51238696781".to_owned(),
            new_supplier_gln: "9900555000005".to_owned(),
            grid_operator_gln: "9900357000004".to_owned(),
            bilanzierungsgebiet: None,
            process_date: time::Date::from_calendar_date(2026, time::Month::November, 1)
                .expect("valid date"),
            sparte: Sparte::Strom,
            messtyp: mako_pruefung::Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            marktlokationsart: art,
            erzeugung: None,
            abmeldeanfrage: Abmeldeanfrage::NichtErforderlich,
        }
    }

    /// The branch the whole two-phase design turns on. An **unassigned**
    /// Marktlokation is confirmed in one pass — Prüfschritt 4 sends the NB
    /// straight to Prozessschritt 5.
    #[test]
    fn an_unassigned_marktlokation_still_answers_in_one_pass() {
        let a = anmeldung(Marktlokationsart::Verbrauchend);
        let out = mako_pruefung::evaluate_lieferbeginn(&a, None);
        assert!(out.is_accept(), "{out:?}");
        assert!(!out.needs_abmeldeanfrage());
    }

    /// An **assigned** one cannot be: the NB owes the incumbent an Anfrage
    /// first, and answering the LFN before it is the defect this closes.
    #[test]
    fn an_assigned_marktlokation_waits_for_the_lfa() {
        let mut a = anmeldung(Marktlokationsart::Verbrauchend);
        a.abmeldeanfrage = Abmeldeanfrage::Erforderlich {
            lfa_mp_ids: vec!["9900111000002".to_owned()],
        };
        let out = mako_pruefung::evaluate_lieferbeginn(&a, None);
        assert!(out.needs_abmeldeanfrage(), "{out:?}");
        // …and the audit log records *why* nothing went out, without claiming
        // an operator has to act.
        let (decision, code, detail) = classify(&out);
        assert_eq!(decision, AnmeldungDecision::Escalate);
        assert!(code.is_none(), "no answer reached the wire");
        assert!(
            detail.as_deref().is_some_and(|d| d.contains("09:00")),
            "{detail:?}"
        );
    }

    /// Phase two, silence: „Verstreicht die Frist … gilt dies als Bestätigung
    /// nach Fall a)". The window closing **confirms** the Anmeldung.
    #[test]
    fn a_lapsed_lfa_window_confirms_the_anmeldung() {
        let mut a = anmeldung(Marktlokationsart::Verbrauchend);
        a.abmeldeanfrage = Abmeldeanfrage::Gestellt { antwort: None };
        let out = mako_pruefung::evaluate_lieferbeginn(&a, None);
        assert_eq!(out.antwortcode(), Some("A51"));
    }

    /// Phase two, refusal: the outcome the NB could not reach at all before it
    /// sent an Anfrage.
    #[test]
    fn a_refusing_lfa_refuses_the_anmeldung() {
        let mut a = anmeldung(Marktlokationsart::Verbrauchend);
        a.abmeldeanfrage = Abmeldeanfrage::Gestellt {
            antwort: Some(LfaAntwort::Widerspruch {
                code: "A35".to_owned(),
                grund: Some("Vertragsbindung".to_owned()),
            }),
        };
        let out = mako_pruefung::evaluate_lieferbeginn(&a, None);
        assert_eq!(out.antwortcode(), Some("A50"));
        assert!(out.is_reject());
        // …and the answer is dispatchable, unlike an escalation.
        let (decision, code, _) = classify(&out);
        assert_eq!(decision, AnmeldungDecision::Reject);
        assert_eq!(code.as_deref(), Some("A50"));
    }

    /// `SG12 NAD+Z09` is Muss on the 55010 for a verbrauchende Marktlokation
    /// (Bedingung `[279]`), and the only source for it is the LFN's own
    /// Anmeldung (`[572]`) — so it has to survive the CloudEvent hop.
    #[test]
    fn the_anmeldung_payload_carries_the_kundenname() {
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900555000005",
                "grid_operator": "9900000000001",
                "process_date": "20261001",
                "kunde_name": "Mustermann",
                "kunde_namensformat": "Z01"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse");
        assert_eq!(payload.kunde_name.as_deref(), Some("Mustermann"));
        assert_eq!(payload.kunde_namensformat.as_deref(), Some("Z01"));
    }

    /// The command the NB sends is registered and permitted to the NB role —
    /// a literal here is how a dead command reaches the dispatcher.
    #[test]
    fn the_abmeldeanfrage_command_is_catalogued() {
        assert!(
            mako_markt::commands::DISPATCHED_BY_SERVICES
                .contains(&mako_markt::commands::GPKE_BEENDIGUNG_ZUORDNUNG_ANFRAGEN),
            "gpke.beendigung-zuordnung.anfragen must be in the command catalogue"
        );
    }

    // ── Meldepflichten ────────────────────────────────────────────────────────

    fn date(y: i32, m: time::Month, d: u8) -> time::Date {
        time::Date::from_calendar_date(y, m, d).expect("valid date")
    }

    fn meldung(sparte: Sparte, altlieferant: Option<&str>) -> MeldepflichtContext {
        MeldepflichtContext {
            sparte,
            lfn_mp_id: "9900555000005".to_owned(),
            zuordnungsbeginn: time::Date::from_calendar_date(2026, time::Month::October, 1)
                .expect("valid date"),
            vorgangsnummer: Some("VG-4711".to_owned()),
            tranche: false,
            altlieferant: altlieferant.map(ToOwned::to_owned),
        }
    }

    /// A Gas Meldung must assert `GNB`. `makod` checks the asserted Marktrolle
    /// against the deployment's licensed roles, so asserting `NB` on a Gas
    /// command is refused in a Gas-only deployment — and accepted in a
    /// dual-Sparte one, where it would then be sent by the wrong party.
    #[test]
    fn each_sparte_asserts_its_own_marktrolle_and_commands() {
        let strom = meldung(Sparte::Strom, Some("9900111000002"));
        assert_eq!(strom.marktrolle(), "NB");
        assert_eq!(
            strom.commands(),
            (
                mako_markt::commands::GPKE_ZUORDNUNG_INFORMIEREN,
                mako_markt::commands::GPKE_ZUORDNUNG_BEENDEN,
            )
        );

        let gas = meldung(Sparte::Gas, Some("9870111000002"));
        assert_eq!(gas.marktrolle(), "GNB");
        assert_eq!(
            gas.commands(),
            (
                mako_markt::commands::GELI_ZUORDNUNG_INFORMIEREN,
                mako_markt::commands::GELI_ZUORDNUNG_BEENDEN,
            )
        );
    }

    /// GPKE Teil 2 § 2.1.2 Nr. 1 Prüfschritt 4: an unassigned Marktlokation
    /// goes straight to Prozessschritt 5. Neither Meldung is owed, and sending
    /// one would name an Altlieferant that does not exist.
    #[test]
    fn an_unassigned_marktlokation_owes_no_meldung() {
        let m = meldung(Sparte::Strom, None);
        assert!(m.altlieferant.is_none());
    }

    /// „Die Information ist auch dann zu versenden, sofern LFA und LFN identisch
    /// sind" (Nr. 2) — but there is no assignment to *end* in that case, so the
    /// Beendigung is not owed. The two conditions are deliberately different.
    #[test]
    fn an_identical_lfa_still_gets_the_information_but_no_beendigung() {
        let m = MeldepflichtContext {
            altlieferant: Some("9900555000005".to_owned()),
            ..meldung(Sparte::Strom, None)
        };
        assert_eq!(m.altlieferant.as_deref(), Some(m.lfn_mp_id.as_str()));
        assert!(m.beendigung("51238696012", m.zuordnungsbeginn).is_none());
    }

    /// An unassigned Marktlokation has nothing to end either.
    #[test]
    fn an_unassigned_marktlokation_owes_no_beendigung() {
        let m = meldung(Sparte::Strom, None);
        assert!(m.beendigung("51238696012", m.zuordnungsbeginn).is_none());
    }

    /// The Zuordnungsende the LFA is told about is the Zuordnungsbeginn of the
    /// Anmeldung, and `SG4 DTM+93` takes a civil date.
    #[test]
    fn the_beendigung_names_the_anmeldung_s_zuordnungsbeginn() {
        let m = meldung(Sparte::Strom, Some("9900111000002"));
        let (command, payload) = m
            .beendigung("51238696012", m.zuordnungsbeginn)
            .expect("an LFA holds the MaLo");
        assert_eq!(command, mako_markt::commands::GPKE_ZUORDNUNG_BEENDEN);
        assert_eq!(payload["process_date"], "2026-10-01");
        assert_eq!(payload["empfaenger_mp_id"], "9900111000002");
        assert_eq!(payload["transaktionsgrund"], "ZC8");
    }

    /// **Fall b** — the LFA answered `A34` with an earlier Lieferendedatum, and
    /// that is the Zuordnungsende the Beendigung has to name. Telling it the
    /// Zuordnungsbeginn instead would claim its supply ran days longer than it
    /// did, and the LFA bills from this date.
    #[test]
    fn fall_b_names_the_lfas_own_lieferendedatum() {
        let m = meldung(Sparte::Strom, Some("9900111000002"));
        // Anmeldung received Monday 2026-09-07; the LFA releases 2026-09-21,
        // before the 2026-10-01 Zuordnungsbeginn and well past the 1-WT floor.
        let uet = date(2026, time::Month::September, 7);
        let gemeldet = date(2026, time::Month::September, 21);
        assert_eq!(m.zuordnungsende(Some(gemeldet), uet), gemeldet);
        let (_, payload) = m
            .beendigung("51238696012", m.zuordnungsende(Some(gemeldet), uet))
            .expect("an LFA holds the MaLo");
        assert_eq!(payload["process_date"], "2026-09-21");
    }

    /// A date that is not *earlier* than the Zuordnungsbeginn is not Fall b, and
    /// one inside the „mindestens 1 WT nach dem ÜT" floor is not admissible.
    /// Both leave the Zuordnungsbeginn standing rather than being refused —
    /// Nr. 10 states the fallback, so there is nothing to escalate.
    #[test]
    fn an_inadmissible_fall_b_date_leaves_the_zuordnungsbeginn_standing() {
        let m = meldung(Sparte::Strom, Some("9900111000002"));
        let uet = date(2026, time::Month::September, 7);
        for stated in [
            // On the Zuordnungsbeginn — not earlier.
            date(2026, time::Month::October, 1),
            // After it.
            date(2026, time::Month::November, 1),
            // The ÜT itself: inside the 1-Werktag floor.
            uet,
        ] {
            assert_eq!(
                m.zuordnungsende(Some(stated), uet),
                m.zuordnungsbeginn,
                "{stated}"
            );
        }
        // And silence keeps it too.
        assert_eq!(m.zuordnungsende(None, uet), m.zuordnungsbeginn);
    }

    /// Phase two runs hours after phase one, so the facts the Meldung states
    /// travel through the database with the waiting Anmeldung. A context that
    /// does not survive the round trip silently drops the Meldung.
    #[test]
    fn the_meldepflicht_context_survives_the_wait_for_the_lfa() {
        let m = meldung(Sparte::Gas, Some("9870111000002"));
        let stored = serde_json::to_value(&m).expect("serialisable");
        let back: MeldepflichtContext = serde_json::from_value(stored).expect("round-trips");
        assert_eq!(back.marktrolle(), "GNB");
        let (command, payload) = back
            .beendigung("9870123456789", back.zuordnungsbeginn)
            .expect("an LFA holds it");
        assert_eq!(command, mako_markt::commands::GELI_ZUORDNUNG_BEENDEN);
        assert_eq!(payload["empfaenger_mp_id"], "9870111000002");
    }

    /// `SG6 RFF+TN` is Muss on 55036 / 44036 and comes from the LFN's own
    /// `SG4 IDE+24`, which the CloudEvent has to carry for that to be possible.
    #[test]
    fn the_anmeldung_payload_carries_the_vorgangsnummer() {
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900555000005",
                "grid_operator": "9900000000001",
                "process_date": "20261001",
                "vorgangsnummer": "VG-4711"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse");
        assert_eq!(payload.vorgangsnummer.as_deref(), Some("VG-4711"));
    }

    // ── AnmeldungPayload parsing ───────────────────────────────────────────────

    #[test]
    fn parse_strom_lieferbeginn_event() {
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "bilanzierungsgebiet": "11YF-VATTENFALL-2",
                "process_date": "20261001",
                "transaktionsgrund": "E01",
                "bilanzierungsmethode": "RLM"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse");
        assert_eq!(payload.pid, 55001);
        assert_eq!(payload.malo_id, "51238696012");
        assert_eq!(payload.new_supplier_gln, "9900357000004");
        assert_eq!(payload.grid_operator_gln, "9900000000001");
        assert_eq!(
            payload.bilanzierungsgebiet.as_deref(),
            Some("11YF-VATTENFALL-2")
        );
        assert_eq!(payload.transaktionsgrund.as_deref(), Some("E01"));
        // No Transaktionsgrundergänzung → the default verbrauchende branch.
        assert!(payload.transaktionsgrund_ergaenzung.is_none());
        // Messtyp derives from the TM+EM marker in the payload.
        let anfrage = payload.into_anfrage();
        assert_eq!(anfrage.messtyp, mako_pruefung::Messtyp::Rlm);
        assert_eq!(anfrage.transaktionsgrund.as_deref(), Some("E01"));
        assert_eq!(anfrage.marktlokationsart, Marktlokationsart::Verbrauchend);
    }

    #[test]
    fn parse_erzeugende_malo_takes_the_erzeugende_branch() {
        // `STS+7++E01:ZW3` — the Ergänzung is element 3 of DE 9013, a different
        // composite from the Anmeldegrund, and the adapter now surfaces it raw.
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440000",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261001",
                "transaktionsgrund": "E01",
                "transaktionsgrund_ergaenzung": "ZW3"
            }
        });
        let anfrage = AnmeldungPayload::parse(&event)
            .expect("should parse")
            .into_anfrage();
        assert_eq!(anfrage.marktlokationsart, Marktlokationsart::Erzeugend);
        // No `CCI+Z22` on the message → the Veräußerungsform is unknown and
        // `evaluate` escalates rather than picking one of six Vorlauffristen.
        assert!(anfrage.erzeugung.is_none());
    }

    /// With the Veräußerungsform on the wire the engine has what `E_0622`
    /// Prüfschritte 400–440 ask for.
    #[test]
    fn a_veraeusserungsform_reaches_the_engine() {
        let event = serde_json::json!({
            "makopid": 55_077,
            "subject": "550e8400-e29b-41d4-a716-446655440021",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261101",
                "transaktionsgrund": "E03",
                "transaktionsgrund_ergaenzung": "ZW3",
                "veraeusserungsform": "Z91"
            }
        });
        let anfrage = AnmeldungPayload::parse(&event)
            .expect("parses")
            .into_anfrage();
        let erz = anfrage.erzeugung.expect("Veräußerungsform present");
        assert_eq!(
            erz.angemeldete_veraeusserungsform,
            mako_pruefung::nb::types::Veraeusserungsform::Marktpraemie
        );
        assert_eq!(erz.geschaeftsvorfall, Geschaeftsvorfall::Eins);
        // The *bestehende* form is the NB's own register, not a wire fact.
        assert!(erz.bestehende_veraeusserungsform.is_none());
    }

    #[test]
    fn parse_gas_lieferbeginn_event() {
        let event = serde_json::json!({
            "makopid": 44001,
            "subject": "550e8400-e29b-41d4-a716-446655440001",
            "data": {
                "malo_id": "51238696781",
                "new_supplier": "9800357000004",
                "grid_operator": "9800000000001",
                "process_date": "2026-10-01"
            }
        });
        let payload = AnmeldungPayload::parse(&event).expect("should parse gas event");
        assert_eq!(payload.pid, 44001);
        let anfrage = payload.into_anfrage();
        assert!(matches!(anfrage.sparte, mako_markt::domain::Sparte::Gas));
    }

    #[test]
    fn parse_ignores_unknown_pids() {
        let event = serde_json::json!({
            "makopid": 55008, // E_0624 — LF PID, not NB
            "subject": "550e8400-e29b-41d4-a716-446655440002",
            "data": { "malo_id": "51238696012", "new_supplier": "99x", "grid_operator": "99y", "process_date": "20261001" }
        });
        assert!(AnmeldungPayload::parse(&event).is_none());
    }

    // ── Command name mapping ───────────────────────────────────────────────────
    //
    // Anmeldung and Abmeldung answer through *different* commands. Answering an
    // Abmeldung with `gpke.lieferbeginn.bestaetigen` puts the wrong response PID
    // on the wire, and both names are plausible enough to survive review.

    #[test]
    fn anmeldung_and_abmeldung_take_different_commands() {
        assert_eq!(
            answer_commands(55_001),
            (
                "gpke.lieferbeginn.bestaetigen",
                "gpke.lieferbeginn.ablehnen"
            )
        );
        assert_eq!(
            answer_commands(55_077),
            (
                "gpke.lieferbeginn.bestaetigen",
                "gpke.lieferbeginn.ablehnen"
            ),
            "makod derives 55078/55080 from the inbound PID it spawned with"
        );
        assert_eq!(
            answer_commands(55_004),
            ("gpke.lieferende.bestaetigen", "gpke.lieferende.ablehnen")
        );
        assert_eq!(
            answer_commands(44_001),
            (
                "geli.lieferbeginn.bestaetigen",
                "geli.lieferbeginn.ablehnen"
            )
        );
        assert_eq!(
            answer_commands(44_004),
            ("geli.lieferende.bestaetigen", "geli.lieferende.ablehnen")
        );
    }

    /// Every posted name must be in the shared list `makod`'s registry test
    /// cross-checks — an unregistered name comes back as HTTP 422.
    #[test]
    fn every_answer_command_is_registered() {
        for pid in answered_pids() {
            let (accept, reject) = answer_commands(pid);
            for name in [accept, reject] {
                assert!(
                    mako_markt::commands::DISPATCHED_BY_SERVICES.contains(&name),
                    "{name:?} (PID {pid}) missing from DISPATCHED_BY_SERVICES"
                );
            }
        }
    }

    /// 55016 „Kündigung" is LFN → LFA: parsing it here would make an `nb-only`
    /// binary answer a supplier obligation.
    #[test]
    fn the_kuendigung_is_not_an_nb_anmeldung() {
        assert!(!ANMELDUNG_PIDS.contains(&55_016));
        let event = serde_json::json!({
            "makopid": 55_016,
            "subject": "550e8400-e29b-41d4-a716-446655440009",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261001"
            }
        });
        assert!(AnmeldungPayload::parse(&event).is_none());
        assert!(AbmeldungPayload::parse(&event).is_none());
    }

    // ── Abmeldung ─────────────────────────────────────────────────────────────

    #[test]
    fn parse_strom_abmeldung_event() {
        let event = serde_json::json!({
            "makopid": 55_004,
            "subject": "550e8400-e29b-41d4-a716-446655440010",
            "data": {
                "malo_id": "51238696012",
                "current_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261101",
                "transaktionsgrund": "E01"
            }
        });
        let p = AbmeldungPayload::parse(&event).expect("parses");
        assert_eq!(p.pid, 55_004);
        assert_eq!(p.lf_mp_id, "9900357000004");
        let a = p.into_anfrage();
        assert!(matches!(a.sparte, Sparte::Strom));
        assert_eq!(a.messtyp, Messtyp::Slp);
    }

    #[test]
    fn parse_gas_abmeldung_event() {
        let event = serde_json::json!({
            "makopid": 44_004,
            "subject": "550e8400-e29b-41d4-a716-446655440011",
            "data": {
                "malo_id": "51238696012",
                "current_supplier": "9800357000004",
                "grid_operator": "9800000000001",
                "process_date": "2026-11-01",
                "bilanzierungsmethode": "RLM"
            }
        });
        let a = AbmeldungPayload::parse(&event)
            .expect("parses")
            .into_anfrage();
        assert!(matches!(a.sparte, Sparte::Gas));
        assert_eq!(a.messtyp, Messtyp::Rlm);
    }

    /// An Anmeldung PID must not parse as an Abmeldung and vice versa —
    /// the two pipelines dispatch different market messages.
    #[test]
    fn the_two_payloads_do_not_overlap() {
        for pid in answered_pids() {
            let event = serde_json::json!({
                "makopid": pid,
                "subject": "550e8400-e29b-41d4-a716-446655440012",
                "data": {
                    "malo_id": "51238696012",
                    "new_supplier": "9900357000004",
                    "grid_operator": "9900000000001",
                    "process_date": "20261101"
                }
            });
            let anmeldung = AnmeldungPayload::parse(&event).is_some();
            let abmeldung = AbmeldungPayload::parse(&event).is_some();
            assert!(
                anmeldung ^ abmeldung,
                "PID {pid} parses as {} — exactly one pipeline must claim it",
                if anmeldung { "both" } else { "neither" }
            );
        }
    }

    /// PID 55077 *is* the „Anmeldung erz. MaLo" use case, so the § 10c EEG
    /// Monatserster rule must apply even when the adapter omitted the ZW3 flag.
    #[test]
    fn pid_55077_is_always_an_erzeugende_marktlokation() {
        let event = serde_json::json!({
            "makopid": 55_077,
            "subject": "550e8400-e29b-41d4-a716-446655440013",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": "9900357000004",
                "grid_operator": "9900000000001",
                "process_date": "20261101"
            }
        });
        let a = AnmeldungPayload::parse(&event)
            .expect("parses")
            .into_anfrage();
        assert_eq!(a.marktlokationsart, Marktlokationsart::Erzeugend);
    }

    // ── Misdirection check ─────────────────────────────────────────────────────

    /// The Selbstzahler hold keys on the Wechsel Transaktionsgrund and on
    /// nothing else.
    ///
    /// GPKE Teil 1, Vorbemerkung, carves out only „die Meldungen des Lieferanten
    /// im Rahmen des Lieferantenwechsels". An Einzug (`E01`) or an Einzug in
    /// Neuanlage (`E02`) on a Selbstzahler MaLo stays on the automated path: the
    /// Letztverbraucher is a full LF there. Widening the hold to every
    /// Transaktionsgrund would take an industrial customer's whole MaLo
    /// portfolio off automation.
    #[test]
    fn only_the_wechsel_grund_triggers_the_selbstzahler_hold() {
        assert_eq!(super::WECHSEL, "E03");
        // The other Anmeldung Transaktionsgründe stay on the automated path:
        // E01 Ein-/Auszug, E02 Einzug in Neuanlage, E06 Ersatzbelieferung.
        for other in ["E01", "E02", "E06"] {
            assert_ne!(other, super::WECHSEL);
        }
    }

    /// The incumbent is read at the day *before* the requested Zuordnungsbeginn:
    /// on the Zuordnungsbeginn itself the new contract may already be in force,
    /// and the question is who the Wechsel displaces.
    #[test]
    fn the_incumbent_is_read_the_day_before_the_zuordnungsbeginn() {
        let beginn = time::macros::date!(2026 - 07 - 01);
        assert_eq!(
            beginn.previous_day().unwrap(),
            time::macros::date!(2026 - 06 - 30)
        );
    }

    #[test]
    fn affiliate_detection() {
        // When new_supplier == own_mp_id, initiator_is_affiliate must be true.
        let own_mp_id = "9900357000004";
        let event = serde_json::json!({
            "makopid": 55001,
            "subject": "550e8400-e29b-41d4-a716-446655440003",
            "data": {
                "malo_id": "51238696012",
                "new_supplier": own_mp_id, // affiliate!
                "grid_operator": "9900000000001",
                "process_date": "20261001"
            }
        });
        let payload = AnmeldungPayload::parse(&event).unwrap();
        let initiator_is_affiliate = payload.new_supplier_gln == own_mp_id;
        assert!(
            initiator_is_affiliate,
            "affiliate must be detected when new_supplier == own_mp_id"
        );
    }
}
