//! MSB process decision module — the WiM Messstellenbetrieb answers.
//!
//! Both Sparten, on the same Prüfschritte: AWH WiM Gas 2.0 restates WiM Strom
//! Teil 1 use-case for use-case with the same Fristen, and only the alphabet
//! differs.
//!
//! | Strom | Gas | Process | Direction | Answered by | Frist | EBD Strom / Gas |
//! |---|---|---|---|---|---|---|
//! | **55042** | **44042** | Anmeldung MSB | MSBN → NB | the **NB** | 5 WT | `E_0201` / `E_2002` |
//! | **55051** | **44051** | Ende MSB | MSBA → NB | the **NB** | 7 WT | `E_0202` / `E_2005` |
//! | **55039** | **44039** | Kündigung MSB | MSBN → **MSBA** | the **MSB** | 3 WT | `E_0200` / `E_2000` |
//! | **55168** | **44168** | Verpflichtungsanfrage | NB → **gMSB** | the **MSB** | 1 WT | `E_0240` / `E_2006` |
//! | **35001/35002/35005** | — | REQOTE | → MSB | the **MSB** | 4/5/10 WT | — |
//!
//! The directions are not uniform, which is why the PID sets are two constants
//! gated by separate Cargo features: a Kündigung MSB never reaches the NB at
//! all, so an NB-role handler that answered it would be answering a message the
//! NB cannot receive.
//!
//! ```text
//! Event arrives → parse MsbWechselPayload
//!   → GET /api/v1/melos/{melo_id}                 ← marktd (does the MeLo exist?)
//!   → GET /api/v1/partners/{msbn_mp_id}           ← marktd (Rahmenvertrag § 9 MsbG?)
//!   → mako_pruefung::msb::pruefe_{anmeldung,kuendigung,abmeldung}
//!       Accept   → wim.geraetewechsel.bestaetigen [if auto_accept]
//!                  else approval_queue with the WiM Frist
//!       Reject   → wim.geraetewechsel.ablehnen with the EBD Antwortcode
//!       Escalate → approval_queue with the WiM Frist
//! ```
//!
//! The Prüfschritte live in [`mako_pruefung::msb`]; this module is the
//! plumbing. Three rules it keeps:
//!
//! - **A transport error is not evidence of absence.** Every `marktd` lookup
//!   failure propagates so the caller answers 5xx and the fan-out redelivers;
//!   only a genuine 404 may become a `ZC9`.
//! - **The Frist runs from the Übertragungstag**, which the CloudEvent carries
//!   in `time`. Parsing time would restart every window on a redelivery.
//! - **The Antwortcode comes from the process's own Entscheidungsbaum.** No WiM
//!   tree publishes the GPKE codes `A02` or `A05`; a code from the wrong tree
//!   is unparseable at the counterparty, not a softer rejection.
//!
//! # Regulatory basis
//!
//! - **BK6-22-024 Anlage 2a** (WiM Strom Teil 1) — Kap. 2.2–2.4
//! - **AWH WiM Gas 2.0** (gültig ab 01.10.2026) — Kap. 3.3, 3.5, 3.6
//! - **Entscheidungsbaum-Diagramme und Codelisten 4.3** — Kap. 8
//! - **§ 5 MsbG** — freie Wahl des Messstellenbetreibers
//! - **§ 9 Abs. 1 Nr. 3 MsbG** — the Rahmenvertrag the NB checks
//! - **§ 14 MsbG** — the Anschlussnutzer's right to switch

use tracing::{info, warn};

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the MSB module.
///
/// Carries no `marktd` connection details: the webhook path is handed an
/// already-connected client.
#[derive(Debug, Clone)]
pub struct MsbModuleConfig {
    pub own_mp_id: String,
    pub tenant: String,
    /// `[msb] auto_accept`. When `true`, an `Accept` verdict dispatches the
    /// Bestätigung itself; when `false`, it goes to the approval queue with its
    /// Frist attached.
    pub auto_accept: bool,
    /// When `true`, an inbound REQOTE is answered with a QUOTES built from the
    /// current `PreisblattMessung`. `[msb] auto_preisanfrage` in TOML.
    pub auto_preisanfrage: bool,
    /// Base URL of `vertragd`, which owns the Messstellenverträge.
    ///
    /// The Kündigung MSB is a contract-layer process (WiM Teil 1 Kap. 2.1.3),
    /// so `E_0200` is answered from contract state — the same split the LF
    /// module uses: supply state from `marktd`, contract state from `vertragd`.
    /// Unset means the Vertragslage stays unknown and every Kündigung
    /// escalates.
    pub vertragd_url: Option<String>,
    /// Bearer token for `vertragd`.
    pub vertragd_api_key: Option<secrecy::SecretString>,
}

// ── Decision types ────────────────────────────────────────────────────────────

/// The WiM MSB-Wechsel PIDs **this deployment's NB role** answers.
///
/// Per `mako_wim::geraetewechsel`, directions are not uniform: 55042
/// (Anmeldung) is MSBN → NB and 55051 (Ende MSB) is MSBA → NB, so the NB owes
/// both answers. 55039 and 55168 never reach the NB.
pub const NB_ANSWERED_PIDS: &[u32] = &[55_042, 55_051, 44_042, 44_051];

/// The WiM MSB-Wechsel PIDs **this deployment's MSB role** answers.
///
/// 55039/44039 (Kündigung MSB) is MSBN → MSBA — it never reaches the NB at all,
/// so routing it into an NB-role handler answers a message the NB cannot
/// receive. 55168/44168 (Verpflichtungsanfrage) is NB → gMSB.
pub const MSB_ANSWERED_PIDS: &[u32] = &[55_039, 55_168, 44_039, 44_168];

/// The Sparte a WiM MSB-Wechsel Prüfidentifikator belongs to.
///
/// The Prüfschritte are identical in both — AWH WiM Gas 2.0 restates WiM Strom
/// Teil 1 — but the alphabets are not, so the tree the answer resolves against
/// follows the PID: `E_0200`/`E_0201`/`E_0202` in Strom, `E_2000`/`E_2002`/
/// `E_2005` in Gas.
fn sparte_of(pid: u32) -> mako_pruefung::msb::types::Sparte {
    use mako_pruefung::msb::types::Sparte;
    if (44_000..45_000).contains(&pid) {
        Sparte::Gas
    } else {
        Sparte::Strom
    }
}

/// Fields extracted from `de.mako.process.initiated` for a WiM MSB-Wechsel PID.
#[derive(Debug, Clone)]
pub struct MsbWechselPayload {
    pub process_id: uuid::Uuid,
    pub pid: u32,
    pub malo_id: String,
    pub melo_id: String,
    /// MP-ID of the MSB that sent the order — the MSBN on 55039/55042, the
    /// MSBA on 55051.
    pub msb_mp_id: String,
    pub nb_mp_id: String,
    /// `SG4 DTM+76` — the Zuordnungsbeginn/-ende the order asks for.
    ///
    /// `None` when the message carried none, which is itself a finding: every
    /// Vorlauffrist in WiM Teil 1 is measured against this date, so without it
    /// no date check can run and the decision escalates.
    pub prozessdatum: Option<time::Date>,
    /// `SG4 STS+7` Transaktionsgrund, where the message carried one.
    pub transaktionsgrund: Option<String>,
    /// Whether the order carries the Versicherung über die Beauftragung
    /// (Kap. 2.3.2 Nr. 1 Ziff. 2).
    pub versicherung: bool,
    /// `SG4 DTM+471` — the Kündigung asks for the „nächstmöglicher Termin"
    /// rather than a fixed date. The two are answered differently
    /// (Kap. 2.2.1), so they cannot collapse into one optional date.
    pub naechstmoeglicher_termin: bool,
    /// The **Übertragungstag**, from the CloudEvent's `time`.
    ///
    /// Not the parse instant: a redelivery after an outage would otherwise
    /// restart every Frist, and a Vorlauffrist measured from the wrong day
    /// rejects or confirms the wrong messages.
    pub received_at: time::OffsetDateTime,
}

impl MsbWechselPayload {
    /// Parse from a `de.mako.process.initiated` CloudEvent for any WiM
    /// MSB-Wechsel PID this deployment could be asked to answer.
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !NB_ANSWERED_PIDS.contains(&pid) && !MSB_ANSWERED_PIDS.contains(&pid) {
            return None;
        }
        let subject = event["subject"].as_str()?;
        let process_id: uuid::Uuid = subject.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();
        let melo_id = data
            .get("melo_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let msb_mp_id = data
            .get("new_msb")
            .or_else(|| data.get("msb_mp_id"))
            .or_else(|| data.get("nmsb_mp_id"))
            .or_else(|| data.get("sender"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let nb_mp_id = data
            .get("grid_operator")
            .or_else(|| data.get("nb_mp_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let prozessdatum = data
            .get("process_date")
            .or_else(|| data.get("prozessdatum"))
            .and_then(|v| v.as_str())
            .and_then(parse_yyyymmdd);
        let transaktionsgrund = data
            .get("transaktionsgrund")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let versicherung = data
            .get("versicherung")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let naechstmoeglicher_termin = data
            .get("naechstmoeglicher_termin")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        Some(Self {
            process_id,
            pid,
            malo_id,
            melo_id,
            msb_mp_id,
            nb_mp_id,
            prozessdatum,
            transaktionsgrund,
            versicherung,
            naechstmoeglicher_termin,
            received_at: event["time"]
                .as_str()
                .and_then(|t| {
                    time::OffsetDateTime::parse(t, &time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_else(time::OffsetDateTime::now_utc),
        })
    }

    /// The Übertragungstag as a German-local calendar date — the anchor every
    /// WiM Vorlauffrist is measured from.
    fn uebertragungstag(&self) -> time::Date {
        mako_fristen::berlin_date(self.received_at)
    }
}

/// `YYYYMMDD` or `YYYY-MM-DD`, whichever the producer sent.
fn parse_yyyymmdd(raw: &str) -> Option<time::Date> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 8 {
        return None;
    }
    let year: i32 = digits[0..4].parse().ok()?;
    let month = time::Month::try_from(digits[4..6].parse::<u8>().ok()?).ok()?;
    let day: u8 = digits[6..8].parse().ok()?;
    time::Date::from_calendar_date(year, month, day).ok()
}

impl MsbModuleConfig {
    /// The config the webhook path needs, from the shared handler state.
    #[must_use]
    pub fn for_state(
        own_mp_id: &str,
        tenant: &str,
        auto_accept: bool,
        auto_preisanfrage: bool,
        vertragd_url: Option<String>,
        vertragd_api_key: Option<secrecy::SecretString>,
    ) -> Self {
        Self {
            own_mp_id: own_mp_id.to_owned(),
            tenant: tenant.to_owned(),
            auto_accept,
            auto_preisanfrage,
            vertragd_url,
            vertragd_api_key,
        }
    }
}

/// Outcome of an MSB-Wechsel evaluation, in the shape the dispatcher needs.
///
/// A thin projection of [`mako_pruefung::msb::MsbEntscheidung`]: the
/// Antwortcode and the EBD it came from, plus the Terminänderung the code may
/// assert. Everything that decides *which* code lives in `mako-pruefung`.
#[derive(Debug, Clone)]
pub enum MsbDecisionOutcome {
    /// Confirm, with the Zustimmungscode the tree publishes.
    Accept {
        /// `SG4 STS+E01` DE 9013.
        antwortcode: String,
        /// The date the Bestätigung states, when the code asserts one.
        abweichender_termin: Option<time::Date>,
    },
    /// Refuse, with the Ablehnungscode the tree publishes.
    Reject {
        /// `SG4 STS+E01` DE 9013 — from the process's own Entscheidungsbaum.
        antwortcode: String,
        /// The BDEW wording plus the concrete finding, for `FTX+ACB` and the
        /// audit log.
        reason: String,
        /// The date the Ablehnung points the sender at (`Z12`, `Z34`, `E17`).
        abweichender_termin: Option<time::Date>,
    },
    /// Requires a human.
    Escalate {
        /// What the operator has to establish.
        reason: String,
    },
}

impl From<mako_pruefung::msb::MsbEntscheidung> for MsbDecisionOutcome {
    fn from(e: mako_pruefung::msb::MsbEntscheidung) -> Self {
        use mako_pruefung::msb::MsbEntscheidung as E;
        match e {
            E::Accept(a) => Self::Accept {
                antwortcode: a.antwortcode,
                abweichender_termin: a.abweichender_termin,
            },
            E::Reject(r) => Self::Reject {
                antwortcode: r.antwort.antwortcode.clone(),
                reason: format!("{}: {}", r.antwort.bedeutung, r.detail),
                abweichender_termin: r.antwort.abweichender_termin,
            },
            E::Escalate { reason } => Self::Escalate { reason },
        }
    }
}

// ── Command name mapping ──────────────────────────────────────────────────────

/// Registered `makod` command + Marktrolle for answering an inbound
/// MSB-Wechsel order (PID 55039/55042).
///
/// Both PIDs resolve to the same command pair — `makod` routes the answer into
/// the `wim-geraetewechsel` process it spawned for the inbound order, which
/// already knows whether it is an Anmeldung or a Kündigung. The Marktrolle
/// differs: PID 55042 (Anmeldung, MSBN → NB) is answered by the NB, PID 55039
/// (Kündigung, MSBN → MSBA) by the incumbent MSB.
fn geraetewechsel_answer_command(pid: u32, accept: bool) -> (&'static str, &'static str) {
    let command = if accept {
        mako_markt::commands::WIM_GERAETEWECHSEL_BESTAETIGEN
    } else {
        mako_markt::commands::WIM_GERAETEWECHSEL_ABLEHNEN
    };
    let marktrolle = if NB_ANSWERED_PIDS.contains(&pid) {
        "NB"
    } else {
        "MSB"
    };
    (command, marktrolle)
}

// ── STP handler ───────────────────────────────────────────────────────────────

/// Process an inbound `de.mako.process.initiated` event for a WiM MSB-Wechsel PID.
///
/// Queries `marktd` for the two or three facts the published Prüfschritte need,
/// runs them through [`mako_pruefung::msb`], and dispatches the verdict to
/// `makod`.
///
/// | Outcome | Effect |
/// |---|---|
/// | Accept, `auto_accept` on | `wim.geraetewechsel.bestaetigen` with the Zustimmungscode |
/// | Accept, `auto_accept` off | approval-queue row carrying the code and the Frist |
/// | Reject | `wim.geraetewechsel.ablehnen` with the tree's Ablehnungscode |
/// | Escalate | approval-queue row with the Frist |
///
/// # Errors
///
/// Every `marktd` lookup failure is propagated so the caller answers 5xx and
/// `marktd`'s durable fan-out redelivers. A transport error is **not** evidence
/// of absence: treating it as one dispatches a wrongful `ZC9` rejection into
/// the market against a valid § 5 MsbG registration.
pub async fn handle_msb_wechsel(
    cfg: &MsbModuleConfig,
    payload: MsbWechselPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<()> {
    let outcome = evaluate(cfg, &payload, marktd).await?;
    dispatch(cfg, &payload, &outcome, makod, queue).await
}

/// Ask `marktd` what the tree needs and run it.
async fn evaluate(
    cfg: &MsbModuleConfig,
    payload: &MsbWechselPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
) -> anyhow::Result<MsbDecisionOutcome> {
    use mako_pruefung::msb;

    // 55168 has no executable Prüfschritt: WiM Teil 1 Kap. 2.4.2 Nr. 4 leaves
    // the answer to the gMSB's own commercial judgement („nach eigenem
    // Ermessen"), so inventing a rule here would put an unfounded Zustimmung
    // or Ablehnung on the market. It goes to the operator with its 1-Werktag
    // window attached.
    if matches!(payload.pid, 55_168 | 44_168) {
        return Ok(MsbDecisionOutcome::Escalate {
            reason: format!(
                "Verpflichtungsanfrage zur Messlokation {}: der gMSB entscheidet nach eigenem \
                 Ermessen (WiM Teil 1 Kap. 2.4.2 Nr. 4), ob er selbst übernimmt oder eine \
                 Weiterverpflichtung des MSBA wünscht",
                payload.melo_id
            ),
        });
    }

    // Every Vorlauffrist in WiM Teil 1 is measured against the date the message
    // carries. Without it the date checks cannot run, and running the rest
    // without them would confirm a Zuordnungsbeginn nobody checked.
    let Some(prozessdatum) = payload.prozessdatum else {
        return Ok(MsbDecisionOutcome::Escalate {
            reason: format!(
                "Die Nachricht zur Messlokation {} trägt kein SG4 DTM+76 — ohne \
                 Zuordnungsbeginn/-ende ist die Mindestvorlaufzeit nicht prüfbar",
                payload.melo_id
            ),
        });
    };

    // A MeLo-less order names no object the tree can be run against.
    if payload.melo_id.is_empty() {
        return Ok(MsbDecisionOutcome::Escalate {
            reason: format!(
                "Die Nachricht zur Marktlokation {} trägt keine Messlokation (SG5 LOC+Z17) — \
                 der Messstellenbetrieb wird je Messlokation zugeordnet",
                payload.malo_id
            ),
        });
    }

    let uet = payload.uebertragungstag();
    let cal = mako_fristen::HolidayCalendar::BdewMaKo;

    match payload.pid {
        // ── Anmeldung MSB (55042) — the NB answers, E_0201 ────────────────
        55_042 | 44_042 => {
            // `?` propagates a *transport* failure so the caller answers 5xx
            // and the fan-out redelivers; only a genuine 404 becomes `false`.
            let melo_bekannt = marktd.melo_known(&payload.melo_id).await?;
            // Kap. 2.3.2 Nr. 2 Ziff. 3 asks for a Vertrag nach § 9 Abs. 1
            // Nr. 3 MsbG with the MSBN. `marktd`'s partner directory stands in
            // for it: this deployment records a Marktpartner because it does
            // business with one, so an entry is taken as evidence of the
            // Rahmenvertrag and its absence as evidence against.
            //
            // The stand-in is stated rather than hidden because it is not the
            // same question — a Marktpartner can sit in the Verzeichnisdienst
            // with no Rahmenvertrag. `marktd` holds the precise record for Gas
            // (`msb_rahmenvertraege_gas`, GNB ↔ MSB) and has no Strom twin yet.
            // Both directions escalate rather than rejecting, because `E_0201`
            // publishes no code for a missing Rahmenvertrag.
            let rahmenvertrag = marktd.partner_known(&payload.msb_mp_id).await?;
            let anfrage = msb::AnmeldungMsb {
                sparte: sparte_of(payload.pid),
                melo_id: payload.melo_id.clone(),
                msbn_mp_id: payload.msb_mp_id.clone(),
                gewuenschter_zuordnungsbeginn: prozessdatum,
                einrichtungsart: einrichtungsart(payload.transaktionsgrund.as_deref()),
                versicherung_liegt_vor: payload.versicherung,
                melo_bekannt: Some(melo_bekannt),
                msb_rahmenvertrag: Some(rahmenvertrag),
            };
            Ok(msb::pruefe_anmeldung(&anfrage, uet, cal).into())
        }

        // ── Ende MSB (55051) — the NB answers, E_0202 ─────────────────────
        55_051 | 44_051 => {
            // „Die Messlokation war dem MSB nicht zugeordnet" is the Fehlerfall
            // of Kap. 2.4.1, so the question is which MSB holds the
            // Messlokation on the requested Zuordnungsende — not merely whether
            // the Messlokation exists. `Ok(None)` is the 404 (no assignment
            // covers the date); a transport failure still propagates.
            let zugeordneter_msb = marktd
                .get_melo_msb_at(&payload.melo_id, prozessdatum)
                .await?;
            let zuordnung = zugeordneter_msb.map(|msb| msb == payload.msb_mp_id);
            let anfrage = msb::AbmeldungMsb {
                sparte: sparte_of(payload.pid),
                melo_id: payload.melo_id.clone(),
                msba_mp_id: payload.msb_mp_id.clone(),
                gewuenschtes_zuordnungsende: prozessdatum,
                grund: abmeldegrund(payload.transaktionsgrund.as_deref()),
                zuordnung_besteht: zuordnung,
            };
            Ok(msb::pruefe_abmeldung(&anfrage, uet, cal).into())
        }

        // ── Kündigung MSB (55039) — the **MSBA** answers, E_0200 ──────────
        //
        // No grid registry is consulted: the Kündigung runs on the contract
        // layer between the two MSB (Kap. 2.1.3) and every Prüfschritt is a
        // question about this MSB's own Messstellenbetriebsvertrag.
        55_039 | 44_039 => {
            let vertrag = fetch_messstellenvertrag(cfg, &payload.melo_id).await;
            let anfrage = msb::KuendigungMsb {
                sparte: sparte_of(payload.pid),
                melo_id: payload.melo_id.clone(),
                msbn_mp_id: payload.msb_mp_id.clone(),
                kuendigungstermin: kuendigungstermin(payload, prozessdatum),
                vertragslage: vertragslage(vertrag.as_ref()),
            };
            Ok(msb::pruefe_kuendigung(&anfrage).into())
        }

        pid => Ok(MsbDecisionOutcome::Escalate {
            reason: format!(
                "PID {pid} ({}) hat keine automatisierbare Entscheidungsregel — die Antwort ist \
                 innerhalb von {} Werktagen fällig",
                msb_wechsel_process_name(pid),
                mako_wim::antwort_frist_werktage(pid).unwrap_or(0),
            ),
        }),
    }
}

/// `SG4 DTM+93` („Ende zum") versus `DTM+471` („Ende zum nächstmöglichen
/// Termin") — the two shapes a Kündigungstermin has.
///
/// WiM Teil 1 Kap. 2.2.1 answers them differently: a fixed date the contract
/// cannot honour is **refused** with the nächstmöglicher Termin named in the
/// Ablehnung, while „nächstmöglich" is **confirmed** with that same date
/// stated. Same contract, same day, opposite cluster.
fn kuendigungstermin(
    payload: &MsbWechselPayload,
    prozessdatum: time::Date,
) -> mako_pruefung::msb::Kuendigungstermin {
    use mako_pruefung::msb::Kuendigungstermin as K;
    if payload.naechstmoeglicher_termin {
        K::Naechstmoeglich
    } else {
        K::Fix(prozessdatum)
    }
}

/// The Messstellenbetriebsvertrag from `vertragd`, or `None` when it is not
/// configured, has no contract on file, or could not be reached.
///
/// A transport failure is deliberately **not** an error, matching the LF
/// module: the decision then reaches an unknown fact and escalates, which is
/// the right outcome for „we could not find out". Returning `Err` would make
/// the fan-out retry an event whose answer window is three Werktage wide.
///
/// The two are told apart by [`vertragslage`] — a 404 means *no contract*
/// (`ZC9`), an unreachable `vertragd` means *unknown* (escalate).
async fn fetch_messstellenvertrag(
    cfg: &MsbModuleConfig,
    melo_id: &str,
) -> Option<serde_json::Value> {
    use secrecy::ExposeSecret;

    let base = cfg.vertragd_url.as_ref()?;
    let url = format!(
        "{}/api/v1/messstellenvertraege/{melo_id}/{}",
        base.trim_end_matches('/'),
        cfg.own_mp_id
    );
    let mut req = reqwest::Client::new().get(&url);
    if let Some(key) = &cfg.vertragd_api_key {
        req = req.bearer_auth(key.expose_secret());
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
            Some(serde_json::json!({ "kein_vertrag": true }))
        }
        Ok(resp) => {
            warn!(
                status = %resp.status(), melo_id,
                "processd MSB: vertragd lookup failed — the Vertragslage stays unknown"
            );
            None
        }
        Err(e) => {
            warn!(%e, melo_id, "processd MSB: vertragd unreachable — the decision escalates");
            None
        }
    }
}

/// Project `vertragd`'s Messstellenvertrag view onto the Vertragslage `E_0200`
/// branches on.
///
/// Three distinct inputs, three distinct answers:
///
/// | Input | Vertragslage | `E_0200` |
/// |---|---|---|
/// | `vertragd` unreachable or unconfigured (`None`) | `Unbekannt` | escalate |
/// | 404 — no contract on file | `KeineZuordnung` | `ZC9` |
/// | a contract | `Laufend` / `BereitsGekuendigt` / `Beendet` | decided |
///
/// Collapsing the first two would answer `ZC9` because a lookup failed, which
/// refuses a lawful Kündigung and keeps the customer bound to an MSB they have
/// left (§ 14 MsbG).
fn vertragslage(vertrag: Option<&serde_json::Value>) -> mako_pruefung::msb::Vertragslage {
    use mako_pruefung::msb::Vertragslage as V;

    let Some(v) = vertrag else {
        return V::Unbekannt;
    };
    if v.get("kein_vertrag").is_some() {
        return V::KeineZuordnung;
    }
    let date = |k: &str| {
        v.get(k).and_then(|d| d.as_str()).and_then(|d| {
            time::Date::parse(d, &time::format_description::well_known::Iso8601::DEFAULT).ok()
        })
    };
    if date("beendet_am").is_some() {
        return V::Beendet;
    }
    if let Some(vertragsende) = date("kuendigung_zum") {
        return V::BereitsGekuendigt {
            vertragsende,
            frueher_moeglich: date("frueher_moeglich"),
        };
    }
    // `vertragd` computes `naechstmoeglich` from the contract's own notice
    // period, capped by § 309 Nr. 9 lit. c BGB. Its absence on a live contract
    // means the contract system could not state one.
    match date("naechstmoeglich") {
        Some(naechstmoeglich) => V::Laufend { naechstmoeglich },
        None => V::Unbekannt,
    }
}

/// `SG4 STS+7` DE 9013 → the Einrichtungsart the Vorlauffrist depends on.
///
/// `E02` „Einzug in eine Neuanlage" is the erstmalige Einrichtung des
/// Messstellenbetriebs and takes 7 Werktage instead of 15 (Kap. 2.3.2 Nr. 1).
/// Anything else is the ordinary Wechsel; the short window on a Wechsel
/// confirms a date the Realisierungskorridor cannot fit around.
fn einrichtungsart(transaktionsgrund: Option<&str>) -> mako_pruefung::msb::Einrichtungsart {
    use mako_pruefung::msb::Einrichtungsart as E;
    match transaktionsgrund {
        Some("E02") => E::ErstmaligeEinrichtung,
        Some("E03") => E::Wiederinbetriebnahme,
        _ => E::BestehenderMessstellenbetrieb,
    }
}

/// `SG4 STS+7` DE 9013 → the Abmeldegrund, which decides both whether the
/// 20-Werktage lead time applies and how long a Weiterverpflichtung may run.
fn abmeldegrund(transaktionsgrund: Option<&str>) -> mako_pruefung::msb::Abmeldegrund {
    use mako_pruefung::msb::Abmeldegrund as A;
    match transaktionsgrund {
        // Stilllegung / Außerbetriebnahme der Messlokation — reported after the
        // fact, so it has no lead time at all.
        Some("ZG9" | "ZH1" | "ZH2") => A::Ausserbetriebnahme,
        Some("E01") => A::AnschlussnutzerWechsel,
        _ => A::VertragsEnde,
    }
}

/// Turn a verdict into a `makod` command or an approval-queue row.
async fn dispatch(
    cfg: &MsbModuleConfig,
    payload: &MsbWechselPayload,
    outcome: &MsbDecisionOutcome,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<()> {
    match outcome {
        MsbDecisionOutcome::Accept {
            antwortcode,
            abweichender_termin,
        } => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                antwortcode,
                melo_id = %payload.melo_id,
                "processd MSB: Bestätigung"
            );
            if cfg.auto_accept {
                let (command_name, marktrolle) = geraetewechsel_answer_command(payload.pid, true);
                post_answer(
                    makod,
                    payload,
                    command_name,
                    marktrolle,
                    antwortcode,
                    None,
                    *abweichender_termin,
                    "accept",
                )
                .await
            } else {
                // `auto_accept` off means „an operator decides", not „nobody
                // answers": without a queue row the order goes unanswered and
                // unseen past its WiM Antwortfrist.
                enqueue(
                    cfg,
                    payload,
                    queue,
                    format!(
                        "auto_accept ist aus — die Prüfung ergibt Bestätigung {antwortcode} für {}",
                        msb_wechsel_process_name(payload.pid)
                    ),
                )
                .await
            }
        }

        MsbDecisionOutcome::Reject {
            antwortcode,
            reason,
            abweichender_termin,
        } => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                antwortcode,
                reason,
                "processd MSB: Ablehnung"
            );
            let (command_name, marktrolle) = geraetewechsel_answer_command(payload.pid, false);
            post_answer(
                makod,
                payload,
                command_name,
                marktrolle,
                antwortcode,
                Some(reason),
                *abweichender_termin,
                "reject",
            )
            .await
        }

        MsbDecisionOutcome::Escalate { reason } => {
            warn!(
                process_id = %payload.process_id,
                pid = payload.pid,
                reason,
                "processd MSB: Eskalation — in die Freigabewarteschlange gestellt"
            );
            enqueue(cfg, payload, queue, reason.clone()).await
        }
    }
}

/// Post a Bestätigung or Ablehnung to `makod`.
#[allow(clippy::too_many_arguments)]
async fn post_answer(
    makod: &mako_markt::makod_client::MakodClient,
    payload: &MsbWechselPayload,
    command_name: &str,
    marktrolle: &str,
    antwortcode: &str,
    bemerkung: Option<&String>,
    abweichender_termin: Option<time::Date>,
    idem_kind: &str,
) -> anyhow::Result<()> {
    let mut body = serde_json::json!({
        "process_id": payload.process_id,
        // The Antwortcode rides `SG4 STS+E01` DE 9013; makod resolves the EBD
        // (DE 1131) from the process's own PID, so it cannot drift from the
        // tree the code was drawn from.
        "antwortcode": antwortcode,
        "auto_stp": true,
    });
    if let Some(text) = bemerkung {
        body["bemerkung"] = serde_json::Value::String(text.clone());
    }
    if let Some(t) = abweichender_termin {
        body["abweichender_termin"] = serde_json::Value::String(format!(
            "{:04}{:02}{:02}",
            t.year(),
            t.month() as u8,
            t.day()
        ));
    }
    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: Some(marktrolle.to_owned()),
        command: command_name.to_owned(),
        malo_id: Some(payload.malo_id.clone()),
        melo_id: Some(payload.melo_id.clone()),
        payload: body,
    };
    let idem = format!("msb-wechsel-{idem_kind}-{}", payload.process_id);
    makod.post_command(&idem, &cmd).await.inspect_err(|e| {
        warn!(
            process_id = %payload.process_id,
            error = %e,
            command = command_name,
            "processd MSB: Antwort-Dispatch fehlgeschlagen"
        );
    })?;
    info!(
        process_id = %payload.process_id,
        command = command_name,
        antwortcode,
        "processd MSB: Antwort dispatched"
    );
    Ok(())
}

/// Put the decision in front of an operator, with its WiM Frist attached.
async fn enqueue(
    cfg: &MsbModuleConfig,
    payload: &MsbWechselPayload,
    queue: &PgApprovalQueue,
    reason: String,
) -> anyhow::Result<()> {
    let (approve, reject) = (
        geraetewechsel_answer_command(payload.pid, true),
        geraetewechsel_answer_command(payload.pid, false),
    );
    let window = mako_fristen::antwort::operator_window(payload.pid, payload.received_at);
    let entry = ApprovalQueueEntry::pending(
        payload.process_id,
        payload.pid as i32,
        Some(payload.malo_id.clone()),
        format!(
            "{reason} (Antwortfrist {}: {})",
            window.deadline, window.source
        ),
        window.expires_at,
        cfg.tenant.clone(),
    )
    .with_commands(approve.0, reject.0, Some(approve.1));
    queue.enqueue(&entry).await?;
    Ok(())
}

/// Human-readable name for a WiM MSB-Wechsel PID, for operator-facing reasons.
fn msb_wechsel_process_name(pid: u32) -> &'static str {
    match pid {
        55_039 | 44_039 => "Kündigung MSB",
        55_042 | 44_042 => "Anmeldung MSB",
        55_051 | 44_051 => "Ende MSB",
        55_168 | 44_168 => "Verpflichtungsanfrage",
        _ => "unknown MSB-Wechsel process",
    }
}

// ── M3: Preisanfrage REQOTE auto-response ──────────────────────────────────────

/// PIDs for which the MSB must auto-respond with a QUOTES message.
///
/// Single-sourced from `mako-wim`. A local copy here also listed 35003, which
/// is the ESA Werteanfrage (answered by 15003 in `esa-wertebestellung`) — so a
/// request for measurement values was answered with a PreisblattMessung quote.
use mako_wim::preisanfrage::REQOTE_PIDS;

/// Answer an inbound REQOTE Preisanfrage (PIDs 35001/35002/35004/35005,
/// nMSB → aMSB) with a QUOTES built from the current `PreisblattMessung`.
///
/// ## Every branch ends somewhere
///
/// Answering HTTP 200 on a path that automated nothing lets the fan-out mark
/// the event delivered while the five-Werktage window runs out with no queue
/// row and no operator surface. Each outcome therefore has a distinct ending:
///
/// | Situation | Ending |
/// |---|---|
/// | `auto_preisanfrage = false` | approval-queue entry with the WiM Frist |
/// | No active `PreisblattMessung` | approval-queue entry — an operator must quote |
/// | `marktd` unreachable | `Err` → the caller answers 5xx and the fan-out redelivers |
/// | `makod` dispatch failed | `Err` → same; the QUOTES has not gone out |
/// | Quote dispatched | `Ok(true)` |
///
/// ## Returns
///
/// `Ok(true)` when the event was handled, `Ok(false)` when the PID is not a
/// REQOTE Preisanfrage. `Err` means *retry*, never *decided*.
///
/// ## Regulatory basis
///
/// The answer window is per PID — 35001 → 4 WT, 35002 → 5 WT, 35005 → 10 WT —
/// from [`mako_wim::preisanfrage::antwort_frist_werktage`].
pub async fn handle_preisanfrage_reqote(
    event: &serde_json::Value,
    cfg: &MsbModuleConfig,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<bool> {
    let pid = event["makopid"]
        .as_u64()
        .or_else(|| event["data"]["pid"].as_u64())
        .unwrap_or(0) as u32;

    if !REQOTE_PIDS.contains(&pid) {
        return Ok(false);
    }

    // The subject is the makod process UUID. A non-UUID is a broken producer
    // contract: acking it would drop the answer obligation silently.
    let Ok(process_id) = event["subject"]
        .as_str()
        .unwrap_or("")
        .parse::<uuid::Uuid>()
    else {
        anyhow::bail!(
            "REQOTE (PID {pid}) CloudEvent subject {:?} is not a process UUID",
            event["subject"]
        );
    };

    let data = &event["data"];
    let melo_id = data["melo_id"]
        .as_str()
        .or_else(|| data["location_id"].as_str())
        .unwrap_or("")
        .to_owned();
    let nmsb_mp_id = data["sender"]
        .as_str()
        .or_else(|| data["nmsb_mp_id"].as_str())
        .unwrap_or("")
        .to_owned();

    let received_at = event["time"]
        .as_str()
        .and_then(|s| {
            time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
        })
        .unwrap_or_else(time::OffsetDateTime::now_utc);

    // Escalation is an approval-queue row carrying the WiM answer Frist and the
    // command that sends the quote, so an operator can dispatch it from the
    // queue rather than reconstructing it in the ERP.
    let escalate = async |reason: String| -> anyhow::Result<bool> {
        let window = mako_fristen::antwort::operator_window(pid, received_at);
        let entry = ApprovalQueueEntry::pending(
            process_id,
            pid as i32,
            None,
            format!(
                "{reason} (Antwortfrist {}: {})",
                window.deadline, window.source
            ),
            window.expires_at,
            cfg.tenant.clone(),
        )
        .with_approve_command(
            mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN,
            Some("MSB"),
        );
        queue.enqueue(&entry).await?;
        warn!(
            %process_id, pid, %melo_id, %nmsb_mp_id,
            deadline = %window.deadline,
            "processd MSB: REQOTE escalated to the approval queue"
        );
        Ok(true)
    };

    if !cfg.auto_preisanfrage {
        return escalate(
            "auto_preisanfrage disabled — the QUOTES is dispatched on operator approval".to_owned(),
        )
        .await;
    }

    // A marktd outage is not a business finding: only a genuine *absence* of a
    // PreisblattMessung may escalate. A transport error propagates so the
    // fan-out redelivers.
    let today = mako_fristen::heute();
    let preisblatt = marktd
        .get_preisblatt_messung(&cfg.own_mp_id, today)
        .await
        .map_err(|e| {
            warn!(error = %e, own_mp_id = %cfg.own_mp_id, %process_id,
                  "processd MSB: PreisblattMessung lookup failed — fan-out will redeliver");
            anyhow::anyhow!("marktd PreisblattMessung lookup failed: {e}")
        })?;

    let Some(preisblatt) = preisblatt else {
        return escalate(format!(
            "no PreisblattMessung is in force for aMSB {} on {today} — a QUOTES with no \
             prices must not go out automatically",
            cfg.own_mp_id
        ))
        .await;
    };

    let cmd = mako_markt::makod_client::ForwardCommand {
        command: mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN.to_owned(),
        marktrolle: Some("MSB".to_owned()),
        malo_id: None,
        melo_id: (!melo_id.is_empty()).then(|| melo_id.clone()),
        payload: serde_json::json!({
            "process_id": process_id,
            "auto_response": true,
            "source_pid": pid,
            // Forward the Gueltigkeit so makod can build the QUOTES.
            "preisblatt_gueltigkeit": preisblatt
                .gueltigkeit
                .as_ref()
                .map(|g| serde_json::to_value(g).unwrap_or_default()),
        }),
    };
    let resp = makod
        .post_command(&format!("preisanfrage-angebot-{process_id}"), &cmd)
        .await
        .map_err(|e| {
            warn!(error = %e, %process_id, pid,
                  "processd MSB: QUOTES dispatch failed — fan-out will redeliver");
            anyhow::anyhow!("makod QUOTES dispatch failed: {e}")
        })?;

    info!(
        %process_id, pid, %melo_id, %nmsb_mp_id,
        response_process_id = %resp.process_id,
        "processd MSB: auto-dispatched QUOTES (wim.preisanfrage.angebot-senden)"
    );
    Ok(true)
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;
    use uuid::Uuid;

    fn ev(pid: u32, data: serde_json::Value, time: &str) -> serde_json::Value {
        serde_json::json!({
            "makopid": pid,
            "subject": Uuid::new_v4().to_string(),
            "time": time,
            "data": data,
        })
    }

    // ── Payload parsing ───────────────────────────────────────────────────

    /// The Frist runs from the Übertragungstag the CloudEvent carries. Reading
    /// the parse instant instead restarts every window on a redelivery, and a
    /// Vorlauffrist measured from the wrong day decides the wrong way.
    #[test]
    fn the_uebertragungstag_comes_from_the_event_not_the_clock() {
        let p = MsbWechselPayload::parse(&ev(
            55_042,
            serde_json::json!({ "malo_id": "51238696012", "melo_id": "DE000…1" }),
            "2026-03-02T08:30:00Z",
        ))
        .expect("parses");
        assert_eq!(
            p.uebertragungstag(),
            time::Date::from_calendar_date(2026, Month::March, 2).unwrap()
        );
        assert!(p.received_at < time::OffsetDateTime::now_utc());
    }

    #[test]
    fn the_process_date_is_read_in_either_spelling() {
        for raw in ["20260601", "2026-06-01"] {
            let p = MsbWechselPayload::parse(&ev(
                55_042,
                serde_json::json!({ "malo_id": "51238696012", "process_date": raw }),
                "2026-03-02T08:30:00Z",
            ))
            .expect("parses");
            assert_eq!(
                p.prozessdatum,
                Some(time::Date::from_calendar_date(2026, Month::June, 1).unwrap()),
                "{raw}"
            );
        }
    }

    #[test]
    fn a_pid_outside_the_family_is_not_ours() {
        assert!(
            MsbWechselPayload::parse(&ev(
                55_001,
                serde_json::json!({ "malo_id": "51238696012" }),
                "2026-03-02T08:30:00Z"
            ))
            .is_none()
        );
    }

    // ── Transaktionsgrund mapping ─────────────────────────────────────────

    /// `E02` is the erstmalige Einrichtung and takes 7 Werktage instead of 15.
    /// Reading every Anmeldung as the ordinary case rejects a lawful Neuanlage
    /// eight Werktage early.
    #[test]
    fn e02_selects_the_short_vorlauffrist() {
        use mako_pruefung::msb::Einrichtungsart as E;
        assert_eq!(einrichtungsart(Some("E02")), E::ErstmaligeEinrichtung);
        assert_eq!(einrichtungsart(Some("E03")), E::Wiederinbetriebnahme);
        assert_eq!(einrichtungsart(None), E::BestehenderMessstellenbetrieb);
        assert!(einrichtungsart(Some("E02")).ist_erstmalig());
        assert!(!einrichtungsart(None).ist_erstmalig());
    }

    /// An Außerbetriebnahme is reported *after* the Geräteausbau, so its
    /// Zuordnungsende is in the past by construction. Measuring 20 Werktage
    /// against it would manufacture a finding on every Stilllegung.
    #[test]
    fn a_stilllegung_has_no_lead_time_and_a_shorter_weiterverpflichtung() {
        use mako_pruefung::msb::Abmeldegrund as A;
        for grund in ["ZG9", "ZH1", "ZH2"] {
            assert_eq!(abmeldegrund(Some(grund)), A::Ausserbetriebnahme, "{grund}");
            assert!(!abmeldegrund(Some(grund)).hat_mindestvorlauffrist());
        }
        assert_eq!(abmeldegrund(Some("E01")), A::AnschlussnutzerWechsel);
        assert_eq!(
            abmeldegrund(Some("E01")).max_weiterverpflichtung_monate(),
            3
        );
        assert_eq!(abmeldegrund(None).max_weiterverpflichtung_monate(), 1);
    }

    // ── Vertragslage → E_0200 ─────────────────────────────────────────────

    fn vertrag(
        naechstmoeglich: Option<&str>,
        kuendigung_zum: Option<&str>,
        frueher_moeglich: Option<&str>,
        beendet_am: Option<&str>,
    ) -> serde_json::Value {
        let mut v = serde_json::json!({
            "melo_id": "DE000…1",
            "msb_mp_id": "9900000000003",
            "vertragsbeginn": "2024-01-01",
            "kuendigungsfrist_monate": 1,
        });
        for (k, d) in [
            ("naechstmoeglich", naechstmoeglich),
            ("kuendigung_zum", kuendigung_zum),
            ("frueher_moeglich", frueher_moeglich),
            ("beendet_am", beendet_am),
        ] {
            if let Some(d) = d {
                v[k] = serde_json::Value::String(d.to_owned());
            }
        }
        v
    }

    /// Three distinct inputs, three distinct answers. Collapsing „could not
    /// ask" into „no contract" answers `ZC9` because a lookup failed, which
    /// refuses a lawful Kündigung and keeps the customer bound (§ 14 MsbG).
    #[test]
    fn unreachable_and_absent_are_different_answers() {
        use mako_pruefung::msb::Vertragslage as V;
        assert_eq!(vertragslage(None), V::Unbekannt);
        assert_eq!(
            vertragslage(Some(&serde_json::json!({ "kein_vertrag": true }))),
            V::KeineZuordnung
        );
    }

    /// The four Vertragslage branches `E_0200` distinguishes, in the order the
    /// contract's own fields decide them.
    #[test]
    fn the_contract_maps_onto_the_e0200_branches() {
        use mako_pruefung::msb::Vertragslage as V;
        let d = |y, m, day| time::Date::from_calendar_date(y, m, day).unwrap();

        assert_eq!(
            vertragslage(Some(&vertrag(Some("2026-07-01"), None, None, None))),
            V::Laufend {
                naechstmoeglich: d(2026, Month::July, 1)
            }
        );
        assert_eq!(
            vertragslage(Some(&vertrag(
                None,
                Some("2026-08-01"),
                Some("2026-06-01"),
                None
            ))),
            V::BereitsGekuendigt {
                vertragsende: d(2026, Month::August, 1),
                frueher_moeglich: Some(d(2026, Month::June, 1)),
            },
            "a Kündigung already in force outranks the notice period"
        );
        assert_eq!(
            vertragslage(Some(&vertrag(
                None,
                Some("2026-08-01"),
                None,
                Some("2026-05-01")
            ))),
            V::Beendet,
            "an ended contract outranks everything — Z29, not Z34"
        );
    }

    /// `vertragd` derives `naechstmoeglich` from the notice period; its absence
    /// on a live contract means the contract system could not state one, and
    /// „none recorded" is not „terminable at any time".
    #[test]
    fn a_live_contract_without_a_next_date_escalates() {
        use mako_pruefung::msb::Vertragslage as V;
        assert_eq!(
            vertragslage(Some(&vertrag(None, None, None, None))),
            V::Unbekannt
        );
    }

    /// `DTM+93` and `DTM+471` are answered differently (Kap. 2.2.1), so the
    /// flag cannot collapse into an optional date.
    #[test]
    fn the_two_kuendigungstermin_shapes_stay_distinct() {
        use mako_pruefung::msb::Kuendigungstermin as K;
        let datum = time::Date::from_calendar_date(2026, Month::July, 1).unwrap();
        let mut p = MsbWechselPayload::parse(&ev(
            55_039,
            serde_json::json!({ "malo_id": "51238696012", "melo_id": "DE000…1" }),
            "2026-03-02T08:30:00Z",
        ))
        .expect("parses");
        assert_eq!(kuendigungstermin(&p, datum), K::Fix(datum));
        p.naechstmoeglicher_termin = true;
        assert_eq!(kuendigungstermin(&p, datum), K::Naechstmoeglich);
    }

    /// The Kündigung is decided on the contract, so it answers instead of
    /// escalating — with a code `E_0200` publishes.
    #[test]
    fn a_kuendigung_inside_the_binding_is_z12_naming_the_next_date() {
        use mako_pruefung::msb;
        let anfrage = msb::KuendigungMsb {
            sparte: mako_pruefung::msb::types::Sparte::Strom,
            melo_id: "DE000…1".to_owned(),
            msbn_mp_id: "9900000000003".to_owned(),
            kuendigungstermin: msb::Kuendigungstermin::Fix(
                time::Date::from_calendar_date(2026, Month::May, 1).unwrap(),
            ),
            vertragslage: vertragslage(Some(&vertrag(Some("2026-07-01"), None, None, None))),
        };
        match msb::pruefe_kuendigung(&anfrage).into() {
            MsbDecisionOutcome::Reject {
                antwortcode,
                abweichender_termin,
                ..
            } => {
                assert_eq!(antwortcode, "Z12");
                assert_eq!(
                    abweichender_termin,
                    Some(time::Date::from_calendar_date(2026, Month::July, 1).unwrap())
                );
                assert!(
                    mako_pruefung::codes::lookup(
                        mako_pruefung::codes::EBD_KUENDIGUNG_MSB,
                        &antwortcode
                    )
                    .is_some()
                );
            }
            other => panic!("expected Z12, got {other:?}"),
        }
    }

    // ── Verdict projection ────────────────────────────────────────────────

    /// Every code this module can put on the wire must be published by the
    /// process's own Entscheidungsbaum. `A02` and `A05` — what it sent before —
    /// are `E_0622` codes and appear in none of them.
    #[test]
    fn every_reachable_code_is_published_by_its_tree() {
        use mako_pruefung::codes;
        use mako_pruefung::msb::{self, MsbEntscheidung};
        let cal = mako_fristen::HolidayCalendar::BdewMaKo;
        let beginn = time::Date::from_calendar_date(2026, Month::June, 1).unwrap();

        let mut seen = 0;
        for (uet, versicherung, bekannt) in [
            (
                mako_fristen::sub_werktage(beginn, 15, cal),
                true,
                Some(true),
            ),
            (mako_fristen::sub_werktage(beginn, 2, cal), true, Some(true)),
            (
                mako_fristen::sub_werktage(beginn, 15, cal),
                false,
                Some(true),
            ),
            (
                mako_fristen::sub_werktage(beginn, 15, cal),
                true,
                Some(false),
            ),
        ] {
            let anfrage = msb::AnmeldungMsb {
                sparte: mako_pruefung::msb::types::Sparte::Strom,
                melo_id: "DE000…1".to_owned(),
                msbn_mp_id: "9900000000003".to_owned(),
                gewuenschter_zuordnungsbeginn: beginn,
                einrichtungsart: msb::Einrichtungsart::BestehenderMessstellenbetrieb,
                versicherung_liegt_vor: versicherung,
                melo_bekannt: bekannt,
                msb_rahmenvertrag: Some(true),
            };
            let entscheidung = msb::pruefe_anmeldung(&anfrage, uet, cal);
            let outcome: MsbDecisionOutcome = entscheidung.clone().into();
            let code = match &outcome {
                MsbDecisionOutcome::Accept { antwortcode, .. }
                | MsbDecisionOutcome::Reject { antwortcode, .. } => antwortcode.clone(),
                MsbDecisionOutcome::Escalate { .. } => continue,
            };
            assert!(
                codes::lookup(codes::EBD_ANMELDUNG_MSB, &code).is_some(),
                "{code} is not published by E_0201"
            );
            assert!(matches!(
                entscheidung,
                MsbEntscheidung::Accept(_) | MsbEntscheidung::Reject(_)
            ));
            seen += 1;
        }
        assert_eq!(seen, 4, "each fixture must reach a coded answer");
    }

    /// A rejection that moves a date carries it, so the counterparty is told
    /// what it could have asked for.
    #[test]
    fn a_short_vorlauffrist_rejection_names_the_next_possible_date() {
        use mako_pruefung::msb;
        let cal = mako_fristen::HolidayCalendar::BdewMaKo;
        let beginn = time::Date::from_calendar_date(2026, Month::June, 1).unwrap();
        let uet = mako_fristen::sub_werktage(beginn, 3, cal);
        let outcome: MsbDecisionOutcome = msb::pruefe_anmeldung(
            &msb::AnmeldungMsb {
                sparte: mako_pruefung::msb::types::Sparte::Strom,
                melo_id: "DE000…1".to_owned(),
                msbn_mp_id: "9900000000003".to_owned(),
                gewuenschter_zuordnungsbeginn: beginn,
                einrichtungsart: msb::Einrichtungsart::BestehenderMessstellenbetrieb,
                versicherung_liegt_vor: true,
                melo_bekannt: Some(true),
                msb_rahmenvertrag: Some(true),
            },
            uet,
            cal,
        )
        .into();
        match outcome {
            MsbDecisionOutcome::Reject {
                antwortcode,
                abweichender_termin,
                ..
            } => {
                assert_eq!(antwortcode, "E17");
                assert_eq!(
                    abweichender_termin,
                    Some(mako_fristen::add_werktage(uet, 15, cal))
                );
            }
            other => panic!("expected E17, got {other:?}"),
        }
    }

    // ── Command name mapping ───────────────────────────────────────────────
    //
    // The posted names must come from the shared `mako_markt::commands` list;
    // makod's registry test asserts every name in that list is registered. The
    // pair of tests is what keeps processd from posting a name makod rejects
    // with 422.

    #[test]
    fn answer_command_anmeldung_is_nb() {
        assert_eq!(
            geraetewechsel_answer_command(55042, true),
            ("wim.geraetewechsel.bestaetigen", "NB")
        );
        assert_eq!(
            geraetewechsel_answer_command(55042, false),
            ("wim.geraetewechsel.ablehnen", "NB")
        );
    }

    #[test]
    fn answer_command_kuendigung_is_msb() {
        assert_eq!(
            geraetewechsel_answer_command(55039, true),
            ("wim.geraetewechsel.bestaetigen", "MSB")
        );
        assert_eq!(
            geraetewechsel_answer_command(55039, false),
            ("wim.geraetewechsel.ablehnen", "MSB")
        );
    }

    /// 35003 is the ESA Werteanfrage (answered by 15003 in `esa-wertebestellung`),
    /// not a Preisanfrage — it must never be answered with a PreisblattMessung
    /// quote.
    #[test]
    fn reqote_pids_are_the_canonical_preisanfrage_set() {
        assert_eq!(REQOTE_PIDS, mako_wim::preisanfrage::REQOTE_PIDS);
        assert!(!REQOTE_PIDS.contains(&35003));
    }

    #[test]
    fn posted_commands_are_in_shared_registry_list() {
        for name in [
            geraetewechsel_answer_command(55042, true).0,
            geraetewechsel_answer_command(55039, false).0,
            mako_markt::commands::WIM_PREISANFRAGE_ANGEBOT_SENDEN,
        ] {
            assert!(
                mako_markt::commands::DISPATCHED_BY_SERVICES.contains(&name),
                "{name:?} missing from mako_markt::commands::DISPATCHED_BY_SERVICES — \
                 makod's registry cross-check would not cover it"
            );
        }
    }
}
