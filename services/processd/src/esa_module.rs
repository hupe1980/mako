//! ESA process decision module — the WiM Teil 2 Kap. 4 answers an MSB owes an
//! Energieserviceanbieter.
//!
//! | Inbound PID | Process | Answered with | Frist | EBD |
//! |---|---|---|---|---|
//! | **35003** | Werteanfrage (REQOTE) | QUOTES 15003 | 5 WT | — |
//! | **17007** | Bestellung von Werten | ORDRSP 19011/19012 | 2 WT | `E_0256` |
//! | **39002** | Stornierung der Bestellung | ORDRSP 19013/19014 | 2 WT | `E_0257` |
//! | **17008** | Abbestellung von Werten | ORDRSP 19011/19012 | 2 WT | `E_0254` |
//!
//! Every row is answered by the **MSB**: §34 Abs. 2 S. 2 Nr. 10 MsbG makes
//! serving an ESA a mandatory, non-discriminatory Zusatzleistung, so this is
//! not an optional module for an MSB deployment that has ESA counterparties.
//!
//! ```text
//! Event arrives → parse EsaOrderPayload
//!   → GET /api/v1/esa/framework/{msb}/{esa}      ← marktd (Rahmenvertrag?)
//!   → GET /api/v1/esa/consent-check              ← marktd (Einwilligung gültig?)
//!   → GET /api/v1/melos/{melo}/msb?at=           ← marktd (zugeordnet?)
//!   → GET /api/v1/malos/{malo}/buendel?at=       ← marktd (ein MSB im Bündel?)
//!   → mako_pruefung::msb::esa::pruefe_{bestellung,stornierung,beendigung}
//!       Accept   → wim.wertebestellung.*-beantworten [if auto_accept]
//!                  else approval_queue with the WiM Frist
//!       Reject   → the same command with the tree's Ablehnungscode
//!       Escalate → approval_queue with the WiM Frist
//! ```
//!
//! `E_0253` „Angebot zur Anfrage prüfen" is published **without a tree**, so
//! the Werteanfrage always reaches an operator: the Angebot's Bindungsfrist,
//! earliest start and per-Artikel-ID prices are commercial terms the Festlegung
//! does not specify.
//!
//! # Regulatory basis
//!
//! - **BK6-22-024 Anlage 2b** (WiM Strom Teil 2) — Kap. 4.1–4.3
//! - **Entscheidungsbaum-Diagramme und Codelisten 4.3** — Kap. 8.25–8.26
//! - **§34 Abs. 2 S. 2 Nr. 10 MsbG** — the mandatory Zusatzleistung
//! - **§49 Abs. 2 Nr. 9 MsbG** — the ESA's consent-derived entitlement

use tracing::{info, warn};

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the ESA module.
#[derive(Debug, Clone)]
pub struct EsaModuleConfig {
    /// This deployment's MSB MP-ID.
    pub own_mp_id: String,
    pub tenant: String,
    /// `[esa] auto_accept`. When `true`, an `Accept` verdict dispatches the
    /// Bestätigung itself; when `false`, it goes to the approval queue with its
    /// Frist attached.
    pub auto_accept: bool,
    /// `[esa] auto_reject`. When `true`, a deterministic `Reject` verdict is
    /// dispatched without an operator.
    ///
    /// Separate from `auto_accept` because the two carry different risk: a
    /// wrong Bestätigung commits the MSB to a delivery, a wrong Ablehnung
    /// denies a §34-mandated Zusatzleistung. An operator may want one
    /// automated and not the other.
    pub auto_reject: bool,
    /// Whether the MSB honours a Bestellung that arrived after its own
    /// Bindungsfrist (`E_0256` Prüfschritt 2 — a commercial decision, not a
    /// rule). `[esa] accept_after_bindungsfrist`.
    pub accept_after_bindungsfrist: bool,
}

/// The WiM Teil 2 Kap. 4 PIDs an **MSB** deployment answers.
///
/// 35003 is included: it owes a QUOTES within 5 Werktage. It is answered by an
/// operator (see the module docs), but an unanswered Werteanfrage is a breach
/// either way, so it must reach the queue.
pub const ESA_ANSWERED_PIDS: &[u32] = &[
    mako_fristen::antwort::ESA_WERTEANFRAGE_PID,
    17_007,
    17_008,
    39_002,
];

/// Fields extracted from `de.mako.process.initiated` for an ESA order PID.
#[derive(Debug, Clone)]
pub struct EsaOrderPayload {
    pub process_id: uuid::Uuid,
    pub pid: u32,
    /// Meldepunkt the subscription is for (MaLo-ID, ZPB, NeLo- or Tranchen-ID).
    pub lokations_id: String,
    /// Messlokation, where the process knows one — the dated MSB assignment is
    /// held per MeLo.
    pub melo_id: String,
    /// MP-ID of the ordering ESA.
    pub esa_mp_id: String,
    /// Messprodukt-Code from *Codeliste der Konfigurationen* Kap. 4.6.
    ///
    /// Half of the subscription's business key, and what `E_0256`
    /// Prüfschritte 4/5 ask about.
    pub messprodukt: String,
    /// `IMD+7081` — whether the order is an Abo or a single transmission.
    ///
    /// `E_0254` and `E_0257` both branch on it, with different codes on each
    /// side, so it cannot be defaulted silently.
    pub abonnement: Option<mako_wim::esa::Abonnement>,
    /// `DTM+203` Ausführungsdatum — delivery start on a Bestellung, stop date
    /// on an Abbestellung.
    pub ausfuehrungsdatum: Option<time::Date>,
    /// End of the Bindungsfrist this MSB stated in its own Angebot.
    pub bindungsfrist: Option<time::OffsetDateTime>,
    /// Whether delivery under the order has begun (`E_0257` Prüfschritte 3/4).
    pub lieferung_begonnen: bool,
    /// The **Übertragungstag**, from the CloudEvent's `time`.
    pub received_at: time::OffsetDateTime,
}

impl EsaOrderPayload {
    /// Extract the fields from a `de.mako.process.initiated` event.
    ///
    /// `None` when the PID is not one this module answers or the event carries
    /// no process id — both mean „not for us" rather than „malformed".
    #[must_use]
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(|v| v.as_u64())
            .or_else(|| data.get("pid")?.as_u64())? as u32;
        if !ESA_ANSWERED_PIDS.contains(&pid) {
            return None;
        }
        let process_id: uuid::Uuid = event["subject"].as_str()?.parse().ok()?;
        let lokations_id = data
            .get("malo_id")
            .or_else(|| data.get("lokations_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        Some(Self {
            process_id,
            pid,
            lokations_id,
            melo_id: str_field(data, &["melo_id"]),
            esa_mp_id: str_field(data, &["esa_mp_id", "sender", "esa"]),
            messprodukt: mako_wim::esa::normalize_code(&str_field(data, &["messprodukt"])),
            abonnement: data
                .get("abonnement")
                .and_then(|v| v.as_str())
                .and_then(mako_wim::esa::Abonnement::from_imd_code),
            ausfuehrungsdatum: data
                .get("ausfuehrungsdatum")
                .or_else(|| data.get("beendigung_zum"))
                .and_then(|v| v.as_str())
                .and_then(parse_date),
            bindungsfrist: data
                .get("bindungsfrist")
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                        .ok()
                }),
            lieferung_begonnen: data
                .get("lieferung_begonnen")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            received_at: event
                .get("time")
                .and_then(|v| v.as_str())
                .and_then(|s| {
                    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
                        .ok()
                })
                .unwrap_or_else(time::OffsetDateTime::now_utc),
        })
    }

    /// The Übertragungstag the Frist is measured from.
    #[must_use]
    pub const fn uebertragungstag(&self) -> time::OffsetDateTime {
        self.received_at
    }
}

fn str_field(data: &serde_json::Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| data.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_owned()
}

/// Accept both `CCYYMMDD` and ISO `YYYY-MM-DD`; the wire uses the former and
/// the command API the latter.
fn parse_date(raw: &str) -> Option<time::Date> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).take(8).collect();
    if digits.len() != 8 {
        return None;
    }
    time::Date::from_calendar_date(
        digits[0..4].parse().ok()?,
        time::Month::try_from(digits[4..6].parse::<u8>().ok()?).ok()?,
        digits[6..8].parse().ok()?,
    )
    .ok()
}

// ── Decision types ────────────────────────────────────────────────────────────

/// What this module decided, flattened out of [`mako_pruefung::msb::MsbEntscheidung`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EsaDecisionOutcome {
    /// A Zustimmungscode from the process's own tree.
    Accept {
        antwortcode: String,
        ebd: &'static str,
    },
    /// An Ablehnungscode, with the Prüfschritt's own wording.
    Reject {
        antwortcode: String,
        ebd: &'static str,
        reason: String,
    },
    /// The decision needs a human.
    Escalate { reason: String },
}

impl EsaDecisionOutcome {
    fn from_entscheidung(d: mako_pruefung::msb::MsbEntscheidung, ebd: &'static str) -> Self {
        use mako_pruefung::msb::MsbEntscheidung as E;
        match d {
            E::Accept(a) => Self::Accept {
                antwortcode: a.antwortcode,
                ebd,
            },
            E::Reject(r) => Self::Reject {
                antwortcode: r.antwort.antwortcode,
                ebd,
                reason: r.detail,
            },
            E::Escalate { reason } => Self::Escalate { reason },
        }
    }
}

/// The makod command that answers each inbound PID, and the Marktrolle it runs
/// under.
const fn answer_command(pid: u32) -> Option<(&'static str, &'static str)> {
    match pid {
        17_007 => Some(("wim.wertebestellung.bestellung-beantworten", "MSB")),
        17_008 => Some(("wim.wertebestellung.abbestellung-beantworten", "MSB")),
        39_002 => Some(("wim.wertebestellung.stornierung-beantworten", "MSB")),
        // The Werteanfrage is answered with an Angebot or its Ablehnung, which
        // are two different commands — the queue entry names both.
        _ => None,
    }
}

fn process_name(pid: u32) -> &'static str {
    match pid {
        35_003 => "Anfrage von Werten (ESA)",
        17_007 => "Bestellung von Werten (ESA)",
        17_008 => "Abbestellung von Werten (ESA)",
        39_002 => "Stornierung der Bestellung (ESA)",
        _ => "unknown ESA process",
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Decide and dispatch the MSB's answer to an inbound ESA order.
///
/// # Errors
///
/// Propagates a `marktd` **transport** failure so the caller answers 5xx and
/// the fan-out redelivers. A genuine 404 is a fact the tree consumes, not an
/// error.
pub async fn handle_esa_order(
    cfg: &EsaModuleConfig,
    payload: EsaOrderPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<()> {
    let outcome = evaluate(cfg, &payload, marktd).await?;
    dispatch(cfg, &payload, &outcome, makod, queue).await
}

/// Ask `marktd` what the tree needs and run it.
async fn evaluate(
    cfg: &EsaModuleConfig,
    payload: &EsaOrderPayload,
    marktd: &mako_markt::marktd_client::MarktdClient,
) -> anyhow::Result<EsaDecisionOutcome> {
    use mako_pruefung::msb::esa;

    // The Werteanfrage has no tree: its answer is a priced Angebot whose terms
    // the Festlegung does not specify.
    if payload.pid == mako_fristen::antwort::ESA_WERTEANFRAGE_PID {
        return Ok(EsaDecisionOutcome::Escalate {
            reason: format!(
                "Werteanfrage des ESA {} zu {}{}: das Angebot nennt Bindungsfrist, frühesten \
                 Liefertermin und Preise je Artikel-ID — `E_0253` veröffentlicht dafür keinen \
                 Entscheidungsbaum, die kaufmännischen Konditionen entscheidet der MSB",
                payload.esa_mp_id,
                payload.lokations_id,
                messprodukt_hinweis(&payload.messprodukt),
            ),
        });
    }

    // `IMD+7081` is Muss on every order PID here, and both termination trees
    // branch on it with different codes per side. Defaulting it would answer a
    // one-shot as though it were an Abo.
    let Some(abonnement) = payload.abonnement else {
        return Ok(EsaDecisionOutcome::Escalate {
            reason: format!(
                "Die {} zu {} trägt kein IMD+7081 (Abonnement) — ohne Betriebsart ist weder \
                 `E_0254` noch `E_0257` durchlaufbar",
                process_name(payload.pid),
                payload.lokations_id
            ),
        });
    };
    let art = abonnement.bestellart();

    match payload.pid {
        // ── Bestellung (17007) — `E_0256` ────────────────────────────────
        17_007 => {
            let Some(bindungsfrist) = payload.bindungsfrist else {
                return Ok(EsaDecisionOutcome::Escalate {
                    reason: format!(
                        "Die Bestellung zu {} nennt keine Bindungsfrist des eigenen Angebots — \
                         `E_0256` Prüfschritt 1 ist damit nicht prüfbar",
                        payload.lokations_id
                    ),
                });
            };
            // Prüfschritte 4/5: does this MSB offer the product in that mode?
            // The Codeliste says which products exist; whether *this* MSB
            // serves them is a commercial fact mako does not hold — except for
            // the Pflichtprodukte, which BNetzA Mitteilung Nr. 3 makes
            // non-optional, so those are answered rather than escalated.
            let produkt = mako_wim::esa::messprodukt(&payload.messprodukt);
            let Some(produkt) = produkt else {
                // A code outside Kapitel 4.6 is not a product this role may
                // order at all. `E_0256` publishes no code for „unknown
                // product", so it goes to an operator.
                return Ok(EsaDecisionOutcome::Escalate {
                    reason: format!(
                        "Die Bestellung zu {} nennt das Messprodukt {:?}, das nicht in der \
                         Codeliste der Konfigurationen Kapitel 4.6 steht",
                        payload.lokations_id, payload.messprodukt
                    ),
                });
            };
            let pflicht = produkt.verbindlichkeit == mako_wim::esa::Verbindlichkeit::Pflicht;

            // Prüfschritt 6: is the ESA-Rahmenvertrag in force?
            let framework = marktd
                .esa_framework_established(&cfg.own_mp_id, &payload.esa_mp_id)
                .await?;
            // Prüfschritt 8: is the Einwilligung still valid? A *missing*
            // record is the ESA's self-assertion, which BNetzA Mitteilung Nr. 3
            // forbids rejecting on — so `None`, not `Some(false)`.
            let einwilligung = marktd
                .esa_consent_valid(&payload.esa_mp_id, &cfg.own_mp_id, &payload.lokations_id)
                .await?;
            // Prüfschritt 7: is this MSB assigned to the location for the
            // period? Resolved on the Ausführungsdatum, which is when the
            // delivery is to run.
            let zugeordnet = match (payload.melo_id.as_str(), payload.ausfuehrungsdatum) {
                ("", _) | (_, None) => None,
                (melo, Some(am)) => marktd
                    .get_melo_msb_at(melo, am)
                    .await?
                    .map(|msb| msb == cfg.own_mp_id),
            };
            let gebuendelt = matches!(
                produkt.ebene,
                mako_wim::esa::Lokationsebene::Marktlokation
                    | mako_wim::esa::Lokationsebene::Netzlokation
                    | mako_wim::esa::Lokationsebene::Tranche
            );
            let buendel_einheitlich = match (gebuendelt, payload.ausfuehrungsdatum) {
                (false, _) | (_, None) => None,
                (true, Some(am)) => {
                    marktd
                        .msb_serves_whole_buendel(&payload.lokations_id, &cfg.own_mp_id, am)
                        .await?
                }
            };
            let Some(zugeordnet) = zugeordnet else {
                return Ok(EsaDecisionOutcome::Escalate {
                    reason: format!(
                        "Die Zuordnung des Messstellenbetriebs zu {} im Leistungszeitraum ist \
                         nicht feststellbar — `E_0256` Prüfschritt 7 verlangt sie",
                        payload.lokations_id
                    ),
                });
            };

            let anfrage = esa::EsaBestellung {
                bindungsfrist,
                eingegangen_am: payload.uebertragungstag(),
                akzeptiert_nach_bindungsfrist: cfg.accept_after_bindungsfrist,
                art,
                // Only a Pflichtprodukt can be asserted deliverable without an
                // MSB product catalogue; an optional one escalates below.
                messprodukt_lieferbar: true,
                vertrag_gueltig: framework,
                zugeordnet,
                einwilligung_gueltig: einwilligung,
                // Whether the installed Gerätetechnik can produce the values is
                // a device fact mako does not hold, so it is not asserted
                // false — the walk would then reject on a guess.
                geraetetechnik_geeignet: true,
                gebuendelte_ebene: gebuendelt,
                // Prüfschritt 11: a MaLo/Tranche/NeLo order presupposes one MSB
                // across the whole Lokationsbündel (UC 4.1.1 Vorbedingung).
                // `None` — an empty bundle or a gap in a MeLo's timeline —
                // escalates rather than refusing.
                msb_aller_messlokationen: buendel_einheitlich,
            };
            let decision = esa::pruefe_bestellung(&anfrage);
            // An optional product may be declined outright, and only the
            // operator knows whether this MSB serves it.
            if !pflicht && matches!(decision, mako_pruefung::msb::MsbEntscheidung::Accept(_)) {
                return Ok(EsaDecisionOutcome::Escalate {
                    reason: format!(
                        "Messprodukt {} ({}) ist optional — ob dieser MSB es anbietet, ist eine \
                         kaufmännische Entscheidung; alle übrigen Prüfschritte von `E_0256` sind \
                         erfüllt",
                        produkt.code, produkt.bezeichnung
                    ),
                });
            }
            Ok(EsaDecisionOutcome::from_entscheidung(
                decision,
                mako_pruefung::codes::EBD_ESA_BESTELLUNG,
            ))
        }

        // ── Stornierung (39002) — `E_0257` ───────────────────────────────
        39_002 => {
            let anfrage = esa::EsaStornierung {
                // The Stornierung reached a running process at all, which is
                // what Prüfschritt 1 asks: makod resumes it only from
                // `BestellungBestaetigt`.
                bestellung_bestaetigt: true,
                art,
                uebermittlung_begonnen: payload.lieferung_begonnen,
            };
            Ok(EsaDecisionOutcome::from_entscheidung(
                esa::pruefe_stornierung(&anfrage),
                mako_pruefung::codes::EBD_ESA_STORNIERUNG,
            ))
        }

        // ── Abbestellung (17008) — `E_0254` ──────────────────────────────
        17_008 => {
            let Some(beendigung_zum) = payload.ausfuehrungsdatum else {
                return Ok(EsaDecisionOutcome::Escalate {
                    reason: format!(
                        "Die Abbestellung zu {} trägt kein DTM+203 Ausführungsdatum — ohne \
                         Beendigungszeitpunkt sind die Prüfschritte 2–4 von `E_0254` nicht \
                         durchlaufbar",
                        payload.lokations_id
                    ),
                });
            };
            // Prüfschritte 2–4 compare the requested end against the Abo start
            // and the values already delivered. Those live in the makod
            // process, not in marktd, and the event carries neither — so the
            // walk runs on what is known and escalates where it is not.
            let Some(abo_beginn) = payload
                .bindungsfrist
                .map(|b| b.date())
                .or(payload.ausfuehrungsdatum)
            else {
                return Ok(EsaDecisionOutcome::Escalate {
                    reason: format!(
                        "Der Beginn der turnusmäßigen Übermittlung zu {} ist unbekannt — \
                         `E_0254` Prüfschritt 2 verlangt ihn",
                        payload.lokations_id
                    ),
                });
            };
            let anfrage = esa::EsaBeendigung {
                art,
                beendigung_zum,
                abo_beginn,
                bereits_beendet_zum: None,
                juengste_lieferung: None,
            };
            Ok(EsaDecisionOutcome::from_entscheidung(
                esa::pruefe_beendigung(&anfrage),
                mako_pruefung::codes::EBD_ESA_BEENDIGUNG,
            ))
        }

        pid => Ok(EsaDecisionOutcome::Escalate {
            reason: format!("PID {pid} ist kein ESA-Bestellprozess dieses Moduls"),
        }),
    }
}

fn messprodukt_hinweis(code: &str) -> String {
    mako_wim::esa::messprodukt(code).map_or_else(
        || {
            if code.is_empty() {
                String::new()
            } else {
                format!(" (Messprodukt {code}, nicht in Kapitel 4.6)")
            }
        },
        |p| format!(" (Messprodukt {} — {})", p.code, p.bezeichnung),
    )
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

async fn dispatch(
    cfg: &EsaModuleConfig,
    payload: &EsaOrderPayload,
    outcome: &EsaDecisionOutcome,
    makod: &mako_markt::makod_client::MakodClient,
    queue: &PgApprovalQueue,
) -> anyhow::Result<()> {
    match outcome {
        EsaDecisionOutcome::Accept { antwortcode, ebd } => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                antwortcode,
                ebd,
                lokations_id = %payload.lokations_id,
                "processd ESA: Zustimmung"
            );
            if cfg.auto_accept {
                post_answer(makod, payload, antwortcode, None, "accept").await
            } else {
                enqueue(
                    cfg,
                    payload,
                    queue,
                    format!(
                        "auto_accept ist aus — {ebd} ergibt {antwortcode} für {}",
                        process_name(payload.pid)
                    ),
                )
                .await
            }
        }

        EsaDecisionOutcome::Reject {
            antwortcode,
            ebd,
            reason,
        } => {
            info!(
                process_id = %payload.process_id,
                pid = payload.pid,
                antwortcode,
                ebd,
                reason,
                "processd ESA: Ablehnung"
            );
            if cfg.auto_reject {
                post_answer(makod, payload, antwortcode, Some(reason), "reject").await
            } else {
                // Refusing a §34-mandated Zusatzleistung without an operator is
                // the risk `auto_reject` exists to gate.
                enqueue(
                    cfg,
                    payload,
                    queue,
                    format!("auto_reject ist aus — {ebd} ergibt {antwortcode}: {reason}"),
                )
                .await
            }
        }

        EsaDecisionOutcome::Escalate { reason } => {
            warn!(
                process_id = %payload.process_id,
                pid = payload.pid,
                reason,
                "processd ESA: Eskalation — in die Freigabewarteschlange gestellt"
            );
            enqueue(cfg, payload, queue, reason.clone()).await
        }
    }
}

/// Post the ORDRSP answer to `makod`.
///
/// The Antwortcode rides `SG2 AJT` DE 4465 and makod resolves DE 1082 from the
/// process's own Abo mode, so the code cannot travel under a tree that does not
/// publish it.
async fn post_answer(
    makod: &mako_markt::makod_client::MakodClient,
    payload: &EsaOrderPayload,
    antwortcode: &str,
    reason: Option<&String>,
    idem_kind: &str,
) -> anyhow::Result<()> {
    let Some((command, marktrolle)) = answer_command(payload.pid) else {
        anyhow::bail!("PID {} has no automatic answer command", payload.pid);
    };
    let mut body = serde_json::json!({
        "malo_id": payload.lokations_id,
        "antwort_code": antwortcode,
        "auto_stp": true,
    });
    // A subscription is the (Meldepunkt, Messprodukt) pair — without the
    // product the command cannot say which of several subscriptions at this
    // location it answers.
    if !payload.messprodukt.is_empty() {
        body["messprodukt"] = serde_json::Value::String(payload.messprodukt.clone());
    }
    if let Some(text) = reason {
        body["reason"] = serde_json::Value::String(text.clone());
    }
    let cmd = mako_markt::makod_client::ForwardCommand {
        marktrolle: Some(marktrolle.to_owned()),
        command: command.to_owned(),
        malo_id: Some(payload.lokations_id.clone()),
        melo_id: (!payload.melo_id.is_empty()).then(|| payload.melo_id.clone()),
        payload: body,
    };
    let idem = format!("esa-order-{idem_kind}-{}", payload.process_id);
    makod.post_command(&idem, &cmd).await.inspect_err(|e| {
        warn!(
            process_id = %payload.process_id,
            error = %e,
            command,
            "processd ESA: Antwort-Dispatch fehlgeschlagen"
        );
    })?;
    info!(
        process_id = %payload.process_id,
        command,
        antwortcode,
        "processd ESA: Antwort dispatched"
    );
    Ok(())
}

/// Put the decision in front of an operator, with its WiM Frist attached.
async fn enqueue(
    cfg: &EsaModuleConfig,
    payload: &EsaOrderPayload,
    queue: &PgApprovalQueue,
    reason: String,
) -> anyhow::Result<()> {
    let window = mako_fristen::antwort::operator_window(payload.pid, payload.received_at);
    let entry = ApprovalQueueEntry::pending(
        payload.process_id,
        payload.pid as i32,
        Some(payload.lokations_id.clone()),
        format!(
            "{reason} (Antwortfrist {}: {})",
            window.deadline, window.source
        ),
        window.expires_at,
        cfg.tenant.clone(),
    );
    // The Werteanfrage is answered with an Angebot or its Ablehnung — two
    // different commands, so both are named. The order PIDs are answered by one
    // command that carries either cluster's code, so approve and reject are the
    // same command name.
    let entry = if payload.pid == mako_fristen::antwort::ESA_WERTEANFRAGE_PID {
        entry.with_commands(
            "wim.wertebestellung.anbieten",
            "wim.wertebestellung.anfrage-ablehnen",
            Some("MSB"),
        )
    } else if let Some((command, marktrolle)) = answer_command(payload.pid) {
        entry.with_commands(command, command, Some(marktrolle))
    } else {
        entry
    };
    queue.enqueue(&entry).await?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn event(pid: u32, extra: serde_json::Value) -> serde_json::Value {
        let mut data = serde_json::json!({
            "malo_id": "51238696012",
            "melo_id": "DE0001234567890123456789012345678",
            "esa_mp_id": "9905550000005",
            "messprodukt": "9991000003056",
            "abonnement": "Z01",
        });
        if let Some(o) = extra.as_object() {
            for (k, v) in o {
                data[k] = v.clone();
            }
        }
        serde_json::json!({
            "subject": "0195f1a0-0000-7000-8000-000000000001",
            "makopid": pid,
            "time": "2026-03-02T08:00:00Z",
            "data": data,
        })
    }

    #[test]
    fn parse_takes_the_uebertragungstag_from_the_event_time() {
        let p = EsaOrderPayload::parse(&event(17_007, serde_json::json!({}))).expect("parsed");
        assert_eq!(p.pid, 17_007);
        assert_eq!(p.messprodukt, "9991000003056");
        assert_eq!(p.abonnement, Some(mako_wim::esa::Abonnement::StartAbo));
        assert_eq!(p.uebertragungstag(), datetime!(2026-03-02 08:00 UTC));
    }

    /// The spaced published form of a Messprodukt-Code is the same product.
    #[test]
    fn parse_normalises_the_messprodukt_code() {
        let p = EsaOrderPayload::parse(&event(
            17_007,
            serde_json::json!({ "messprodukt": "9991 00000 305 6" }),
        ))
        .expect("parsed");
        assert_eq!(p.messprodukt, "9991000003056");
    }

    #[test]
    fn parse_ignores_pids_this_module_does_not_answer() {
        assert!(EsaOrderPayload::parse(&event(55_042, serde_json::json!({}))).is_none());
    }

    /// Every PID this module claims has a published window, or the queue entry
    /// it creates carries no deadline.
    #[test]
    fn every_answered_pid_has_a_frist() {
        for pid in ESA_ANSWERED_PIDS {
            let w = mako_fristen::antwort::operator_window(*pid, datetime!(2026-03-02 8:00 UTC));
            assert!(w.is_regulatory, "PID {pid} has no published Antwortfrist");
        }
    }

    fn test_cfg() -> EsaModuleConfig {
        EsaModuleConfig {
            own_mp_id: "9900357000004".to_owned(),
            tenant: "t".to_owned(),
            auto_accept: true,
            auto_reject: true,
            accept_after_bindungsfrist: false,
        }
    }

    /// A client pointed at an address nothing listens on.
    ///
    /// Any test using it asserts a path that reaches **no** `marktd` call —
    /// if one were added, the request would fail and the test would notice.
    fn unreachable_marktd() -> mako_markt::marktd_client::MarktdClient {
        mako_markt::marktd_client::MarktdClient::new(
            "http://127.0.0.1:1",
            secrecy::SecretString::from("test"),
            reqwest::Client::new(),
        )
    }

    /// 35003 is answered by an operator: `E_0253` publishes no tree, and the
    /// Angebot's Bindungsfrist, earliest start and per-Artikel-ID prices are
    /// commercial terms the Festlegung does not specify.
    #[tokio::test]
    async fn the_werteanfrage_escalates_without_consulting_marktd() {
        let payload =
            EsaOrderPayload::parse(&event(35_003, serde_json::json!({}))).expect("parsed");
        let outcome = evaluate(&test_cfg(), &payload, &unreachable_marktd())
            .await
            .expect("the Werteanfrage path makes no marktd call");
        let EsaDecisionOutcome::Escalate { reason } = outcome else {
            panic!("the Werteanfrage must always reach an operator")
        };
        assert!(reason.contains("E_0253"), "{reason}");
    }

    /// `IMD+7081` is Muss on every order PID, and both termination trees branch
    /// on it with different codes per side — so a missing one escalates before
    /// any lookup rather than defaulting to Abo.
    #[tokio::test]
    async fn a_missing_abonnement_escalates_before_any_lookup() {
        for pid in [17_007_u32, 17_008, 39_002] {
            let payload = EsaOrderPayload::parse(&event(
                pid,
                serde_json::json!({ "abonnement": serde_json::Value::Null }),
            ))
            .expect("parsed");
            let outcome = evaluate(&test_cfg(), &payload, &unreachable_marktd())
                .await
                .expect("no marktd call precedes the IMD guard");
            let EsaDecisionOutcome::Escalate { reason } = outcome else {
                panic!("PID {pid} without IMD+7081 must escalate")
            };
            assert!(reason.contains("IMD+7081"), "{pid}: {reason}");
        }
    }

    /// The Stornierung is decided from the process state the event carries, so
    /// it reaches no `marktd` lookup at all.
    #[tokio::test]
    async fn the_stornierung_is_decided_without_marktd() {
        let payload = EsaOrderPayload::parse(&event(
            39_002,
            serde_json::json!({ "lieferung_begonnen": true }),
        ))
        .expect("parsed");
        let outcome = evaluate(&test_cfg(), &payload, &unreachable_marktd())
            .await
            .expect("the Stornierung path makes no marktd call");
        // `A02` — the Abo has already started delivering.
        assert_eq!(
            outcome,
            EsaDecisionOutcome::Reject {
                antwortcode: "A02".to_owned(),
                ebd: mako_pruefung::codes::EBD_ESA_STORNIERUNG,
                reason: "Mit der Übermittlung von Werten aus dem Abo wurde bereits begonnen"
                    .to_owned(),
            }
        );
    }

    /// An Abbestellung without `DTM+203` cannot run Prüfschritte 2–4.
    #[tokio::test]
    async fn an_abbestellung_without_an_ausfuehrungsdatum_escalates() {
        let payload =
            EsaOrderPayload::parse(&event(17_008, serde_json::json!({}))).expect("parsed");
        let outcome = evaluate(&test_cfg(), &payload, &unreachable_marktd())
            .await
            .expect("no marktd call precedes the date guard");
        let EsaDecisionOutcome::Escalate { reason } = outcome else {
            panic!("an Abbestellung without DTM+203 must escalate")
        };
        assert!(reason.contains("DTM+203"), "{reason}");
    }

    /// `E_0254` Prüfschritt 1: a one-shot order is *stornierbar*, not
    /// *abbestellbar* — and that is decided without any lookup.
    #[tokio::test]
    async fn a_one_shot_abbestellung_is_refused_with_a01() {
        let payload = EsaOrderPayload::parse(&event(
            17_008,
            serde_json::json!({ "abonnement": "Z03", "ausfuehrungsdatum": "2026-06-01" }),
        ))
        .expect("parsed");
        let outcome = evaluate(&test_cfg(), &payload, &unreachable_marktd())
            .await
            .expect("no marktd call on this path");
        let EsaDecisionOutcome::Reject {
            antwortcode, ebd, ..
        } = outcome
        else {
            panic!("a one-shot Abbestellung must be refused")
        };
        assert_eq!(antwortcode, "A01");
        assert_eq!(ebd, mako_pruefung::codes::EBD_ESA_BEENDIGUNG);
    }

    /// `E_0257` refuses a started delivery with a different code per Abo mode,
    /// so the module must not collapse the two.
    #[test]
    fn a_started_delivery_refuses_the_storno_per_betriebsart() {
        use mako_pruefung::msb::esa;
        for (abo, expected) in [
            (mako_wim::esa::Abonnement::StartAbo, "A02"),
            (mako_wim::esa::Abonnement::OhneAbo, "A03"),
        ] {
            let d = esa::pruefe_stornierung(&esa::EsaStornierung {
                bestellung_bestaetigt: true,
                art: abo.bestellart(),
                uebermittlung_begonnen: true,
            });
            let out =
                EsaDecisionOutcome::from_entscheidung(d, mako_pruefung::codes::EBD_ESA_STORNIERUNG);
            let EsaDecisionOutcome::Reject { antwortcode, .. } = out else {
                panic!("a started delivery must refuse the Stornierung")
            };
            assert_eq!(antwortcode, expected);
        }
    }

    /// An order without `IMD+7081` cannot be walked: both termination trees
    /// branch on it with different codes per side.
    #[test]
    fn a_missing_abonnement_is_visible_in_the_payload() {
        let p = EsaOrderPayload::parse(&event(
            17_008,
            serde_json::json!({ "abonnement": serde_json::Value::Null }),
        ))
        .expect("parsed");
        assert_eq!(p.abonnement, None);
    }

    #[test]
    fn answer_commands_cover_the_order_pids_but_not_the_anfrage() {
        assert!(answer_command(17_007).is_some());
        assert!(answer_command(17_008).is_some());
        assert!(answer_command(39_002).is_some());
        assert!(answer_command(35_003).is_none());
    }
}
