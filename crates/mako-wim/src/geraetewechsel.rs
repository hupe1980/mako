//! WiM Messstellenbetrieb — MSB change workflow (PIDs 55039, 55042, 55051, 55168).
//!
//! Covers the four MSB-Wechsel use cases of BK6-22-024 WiM Strom Teil 1: the
//! Kündigung between outgoing and incoming Messstellenbetreiber (Kap. 2.2), the
//! Anmeldung of the incoming MSB at the Netzbetreiber (Kap. 2.3), the Abmeldung
//! (Kap. 2.4), and the Verpflichtungsanfrage the NB puts to the grundzuständiger
//! MSB when a Zuordnungslücke looms (Kap. 2.5).
//!
//! # Two clocks, never one
//!
//! An inbound order starts two independent timers, and treating them as one is
//! the classic error here:
//!
//! | Clock | Window | Basis |
//! |---|---|---|
//! | **APERAK** — technical acknowledgement | 45 minutes (Strom UTILMD) | APERAK AHB §2.4.1 |
//! | **Antwort** — business Bestätigung/Ablehnung | **1 / 3 / 5 / 7 WT, per PID** | WiM Teil 1 Kap. 2.2.2 / 2.3.2 / 2.4.2 / 2.5.2 |
//!
//! The business window is never flat: see [`antwort_frist_werktage`]. Sizing all
//! four at 5 WT escalates the Abmeldung (7 WT) two days early against a
//! counterparty still inside its window, and lets a missed Verpflichtungsanfrage
//! (1 WT) run four days undetected.
//!
//! # Regulatory basis
//!
//! - **MsbG** — Messstellenbetriebsgesetz (Smart-Meter-Rollout)
//! - **BNetzA BK6-22-024**, Anlage 2a — WiM Strom Teil 1 (Lesefassung)
//! - **UTILMD S2.x** — EDI@Energy message format for metering processes
//! - **APERAK 2.x** — application error acknowledgement

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Sparte},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{
    APERAK_STROM_WINDOW_LABEL, HolidayCalendar, aperak_strom_due_at, deadline_at_werktage,
};
use time::OffsetDateTime;

/// Stable workflow name used as the `WorkflowId.name` and in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "wim-device-change";

/// Deadline label for **our** obligation to answer an inbound MSB-Wechsel order.
///
/// Despite the process name, this is *not* the APERAK window. Two different
/// clocks run on an inbound order and conflating them is the mistake this
/// naming exists to prevent:
///
/// | Clock | Window | Label |
/// |---|---|---|
/// | Technical acknowledgement (APERAK) | **45 minutes**, Strom UTILMD (APERAK AHB §2.4.1) | `mako_fristen::APERAK_STROM_WINDOW_LABEL` |
/// | Business answer (Bestätigung/Ablehnung) | **1 / 3 / 5 / 7 WT**, per PID | this label |
///
/// Size it with [`antwort_frist_werktage`] — never a flat window:
///
/// ```rust,ignore
/// let wt = antwort_frist_werktage(pid).expect("a WiM MSB-Wechsel request PID");
/// let due = mako_fristen::deadline_at_werktage(
///     received_at, wt, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., ANTWORT_FRIST_WINDOW_LABEL, due);
/// ```
pub const ANTWORT_FRIST_WINDOW_LABEL: &str = "wim-device-change-antwort-frist";

/// Prüfidentifikatoren that carry a WiM MSB-Wechsel UTILMD, **in both Sparten**.
///
/// Directions are per *Anwendungsübersicht der Prüfidentifikatoren* 4.0, the
/// BK6-22-024 WiM Strom Teil 1 Lesefassung and the BDEW *AWH Wechselprozesse im
/// Messwesen Gas 2.0* (gültig ab 01.10.2026). Note that they are **not**
/// uniformly „MSB → NB" — the Kündigung never reaches the NB at all, and the
/// Verpflichtungsanfrage addresses the gMSB:
///
/// | Strom | Gas   | Process                              | Von  | An   |
/// |-------|-------|--------------------------------------|------|------|
/// | 55039 | 44039 | Kündigung MSB                        | MSBN | MSBA |
/// | 55042 | 44042 | Anmeldung MSB (Beginn Messstellenbetrieb) | MSBN | NB |
/// | 55051 | 44051 | Ende MSB (Abmeldung)                 | MSBA | NB   |
/// | 55168 | 44168 | Verpflichtungsanfrage / Aufforderung | NB   | gMSB |
///
/// **One process family, two Sparten.** AWH WiM Gas 2.0 restates WiM Strom
/// Teil 1 use-case for use-case, Frist for Frist and Prüfschritt for
/// Prüfschritt; only the UTILMD PID namespace, the Antwort-Codeliste
/// (`G_00xx` against `S_00xx`), the APERAK regime and the Zuordnungszeitpunkt
/// (06:00 Uhr Gastag against 00:00) differ. They are therefore one workflow
/// parameterised by [`Sparte`], not two.
///
/// The Kündigung is on the **contract layer** between the two MSB and is
/// explicitly *non-constitutive*: WiM Strom Teil 1 Kap. 2.1.3 and AWH WiM Gas
/// 2.0 Kap. 3.1.2 c both state that a switch is effected solely by the
/// successful Anmeldung MSBN → NB. Never gate 55042/44042 on a 55040/44040
/// Bestätigung — they are independent channels.
///
/// Used both to validate inbound UTILMD and to constrain the outbound
/// [`DeviceChangeCommand::InitiateDeviceChange`] order.
pub const DEVICE_CHANGE_PIDS: &[u32] = &[
    55_039, 55_042, 55_051, 55_168, // WiM Strom Teil 1
    44_039, 44_042, 44_051, 44_168, // AWH WiM Gas 2.0
];

/// The Sparte a WiM MSB-Wechsel Prüfidentifikator belongs to.
///
/// The UTILMD legs split by namespace — 55xxx Strom, 44xxx Gas — and that is
/// the only place the Sparte is legible from the PID alone. Every other leg of
/// the same Use-Cases (ORDERS 17001/17002/17009, ORDRSP 19001–19004/19015/19016,
/// REQOTE 35001, QUOTES 15001, IFTSTA 21007–21013/21036, INSRPT 23001–23008)
/// runs on a Sparte-neutral AHB and the same PID in both, so there the Sparte
/// comes from the interchange recipient's MP-ID and travels in the command —
/// see [`crate::geraeteubernahme::GeraeteubernahmeData::sparte`].
///
/// Returns `None` for a PID outside the family — including the answer PIDs,
/// which [`antwort_pid_meaning`] resolves to their request first.
#[must_use]
pub fn wim_sparte(pid: u32) -> Option<Sparte> {
    match pid {
        55_039 | 55_042 | 55_051 | 55_168 => Some(Sparte::Strom),
        44_039 | 44_042 | 44_051 | 44_168 => Some(Sparte::Gas),
        _ => None,
    }
}

/// The Transaktionsgründe an inbound WiM MSB-Wechsel message may state, per
/// Prüfidentifikator.
///
/// `SG4 STS+7` DE 9013 is **Muss on every one of the twelve PIDs** — the
/// Anfrage *and* both answers, in both Sparten (UTILMD AHB Strom 2.2 Kap. 10,
/// Gas 1.2 Kap. 6). The answer echoes the Grund the request stated; a WiM
/// answer that omits the segment is rejected before any Antwortcode is read.
///
/// | Anwendungsfall | Strom | Gas |
/// |---|---|---|
/// | Kündigung | `E03` `ZR9` | `E03` `ZR9` |
/// | Anmeldung | `E01` `E02` `E03` `ZJ4` | `E01` `E02` `E03` |
/// | Ende MSB | `E01` `E03` `Z33` `ZZB` | `E01` `E03` `Z33` |
/// | Verpflichtungsanfrage | `E01` `E02` `E03` | `E01` `E02` `E03` |
///
/// Two Strom-only codes: `ZJ4` „Übernahme aufgrund nicht erfolgtem
/// iMS-Einbau" and `ZZB` „Stilllegung inkl. Ausbau" — both name a Sachverhalt
/// the Gas rollout has no equivalent of.
///
/// **The WiM `STS+7` carries no Ergänzung.** GPKE's `STS+7++<Grund>+<ZW4…>`
/// third element is absent from every WiM Anwendungsübersicht; emitting a
/// default `ZW4` („verbrauchende Marktlokation") on a Messlokations-Vorgang
/// states something the AHB has no element for.
#[must_use]
pub fn transaktionsgruende(pid: u32) -> &'static [&'static str] {
    // Resolve an answer PID to the request whose Grund it echoes.
    let request = if wim_sparte(pid).is_some() {
        pid
    } else {
        match antwort_pid_meaning(pid) {
            Some((r, _)) => r,
            None => return &[],
        }
    };
    match request {
        55_039 => &["E03", "ZR9"],
        44_039 => &["E03", "ZR9"],
        55_042 => &["E01", "E02", "E03", "ZJ4"],
        44_042 => &["E01", "E02", "E03"],
        55_051 => &["E01", "E03", "Z33", "ZZB"],
        44_051 => &["E01", "E03", "Z33"],
        55_168 | 44_168 => &["E01", "E02", "E03"],
        _ => &[],
    }
}

/// The Transaktionsgrund a WiM message defaults to when the caller states none.
///
/// `E03` „Wechsel" is published by all four Anwendungsfälle in both Sparten and
/// is the case every one of them describes — a Kündigung, an Anmeldung, an
/// Abmeldung and a Verpflichtungsanfrage all arise from a Wechsel des
/// Messstellenbetreibers unless the caller says otherwise. It is the only code
/// that can be defaulted without asserting a Sachverhalt (`E02` Neuanlage,
/// `Z33` Auszug, `ZR9` Vertrag mit Anschlussnehmer) the process did not report.
pub const TRANSAKTIONSGRUND_WECHSEL: &str = "E03";

/// UTILMD 44183 — „Ende MSB von NB", the Gas NB informing the MSB that the
/// Messlokation is being stilllegt (AWH WiM Gas 2.0 Kap. 3.7).
///
/// An **information without an answer**: UTILMD AHB Gas 1.2 Kap. 6.4 gives it a
/// `STS+7` with the single Transaktionsgrund `Z33` („Auszug wegen
/// Stilllegung"), `DTM+93`, a Meldepunkt and a Prüfidentifikator — and no
/// `STS+E01`, no `RFF+TN` and no answer Prüfidentifikator. Handled by
/// [`DeviceChangeCommand::ReceiveInformation`], which records it and leaves the
/// state alone.
///
/// It has no Strom twin: the equivalent notice there is an IFTSTA.
pub const ENDE_MSB_VOM_NB_PID: u32 = 44_183;

/// The hour of day at which a WiM Zuordnung takes effect.
///
/// Strom assigns at **00:00 Uhr** (WiM Strom Teil 1 Kap. 2.1.1), Gas at
/// **06:00 Uhr** — the Gastag boundary (AWH WiM Gas 2.0 Kap. 3.1.1: „… mit dem
/// Zeitpunkt 06:00 Uhr zu. Die Zuordnung des MSBA endet entsprechend zu diesem
/// Zeitpunkt"). Assigning a Gas Messlokation from midnight hands the MSBN six
/// hours of a Gastag that still belongs to the MSBA, and every value in that
/// window is then attributed to the wrong party.
#[must_use]
pub const fn zuordnungs_stunde(sparte: Sparte) -> u8 {
    match sparte {
        Sparte::Strom => 0,
        Sparte::Gas => 6,
    }
}

/// Antwortfrist in Werktagen for the counterparty's business response.
///
/// **These differ per process** — a single flat window would fire early for the
/// Kündigung and late for the Abmeldung. From BK6-22-024 WiM Teil 1
/// ("Unverzüglich, jedoch spätester ÜT ist der *n*. WT nach dem ÜT von Nr. 1"):
///
/// | Request | Antwort | Frist | Fundstelle |
/// |---------|---------|-------|------------|
/// | 55039   | 55040/55041 | **3 WT** | Kap. 2.2.2 Nr. 2 |
/// | 55042   | 55043/55044 | **5 WT** | Kap. 2.3.2 Nr. 2 |
/// | 55051   | 55052/55053 | **7 WT** | Kap. 2.4.2 Nr. 2 |
/// | 55168   | 55169/55170 | **1 WT** | Kap. 2.5.2 Nr. 4 |
///
/// A view on [`mako_fristen::antwort::WIM`], which is the same table
/// `makod` registers the deadline from, `processd` sizes its operator queue by
/// and `obsd` raises the breach alert against.
///
/// Distinct from the APERAK window, which is **45 minutes** for UTILMD in Strom
/// (APERAK AHB §2.4.1) — see [`ANTWORT_FRIST_WINDOW_LABEL`].
///
/// Returns `None` when `request_pid` is not a WiM MSB-Wechsel request.
#[must_use]
pub fn antwort_frist_werktage(request_pid: u32) -> Option<u32> {
    use mako_fristen::antwort::FristShape;
    if !DEVICE_CHANGE_PIDS.contains(&request_pid) {
        return None;
    }
    match mako_fristen::antwort::antwort_obligation(request_pid)?.frist {
        FristShape::WerktageAtCutoff(n) => Some(n),
        _ => None,
    }
}

/// Deadline label for the counterparty's response window on an **outbound**
/// MSB-Wechsel order (WiM BK6-22-024).
///
/// Sized per PID via [`antwort_frist_werktage`] — 3 / 5 / 7 / 1 WT, never flat.
///
/// Registered by the caller alongside [`DeviceChangeCommand::InitiateDeviceChange`].
/// Distinct from [`ANTWORT_FRIST_WINDOW_LABEL`], which tracks *our* obligation to
/// acknowledge an inbound message; this one tracks *their* obligation to answer ours.
pub const AUFTRAG_ANTWORT_WINDOW_LABEL: &str = "wim-device-change-auftrag-antwort";

/// Response Prüfidentifikatoren for the WiM MSB-Wechsel, as
/// `(antwort_pid, request_pid, is_confirmed)`.
///
/// | Request | Bestätigung | Ablehnung |
/// |---------|-------------|-----------|
/// | 55039   | 55040       | 55041     |
/// | 55042   | 55043       | 55044     |
/// | 55051   | 55052       | 55053     |
/// | 55168   | 55169       | 55170     |
///
/// These close an order opened with [`DeviceChangeCommand::InitiateDeviceChange`].
///
/// The UTILMD AHB defines every one of these as a full Anwendungsfall — Strom
/// 2.2 Kap. 10.1–10.4, Gas 1.2 Kap. 6.1–6.5 — each as one table with a column
/// per Prüfidentifikator. Every answer carries `SG4 STS+E01` (Status der
/// Antwort) with the Prüfschritt code in DE 9013.
///
/// **DE 1131 names a Codeliste, not the Entscheidungsbaum.** The AHB column
/// reads „Codeliste Strom Nr. `S_0090`", not „EBD Nr. `E_0200`", and the
/// **cluster** picks which of the pair: `S_0090` on the Bestätigung, `S_0054`
/// on the Ablehnung. Ask [`mako_pruefung::codes::AntwortCode::wire_codeliste`]
/// for the wire value — `wim_ebd` returns the tree a code is *resolved* against,
/// which is a different thing and never goes on the wire for this family.
///
/// | Request | Bestätigung DE 1131 | Ablehnung DE 1131 |
/// |---|---|---|
/// | 55039 / 44039 | `S_0090` / `G_0052` | `S_0054` / `G_0051` |
/// | 55042 / 44042 | `S_0055` / `G_0054` | `S_0056` / `G_0053` |
/// | 55051 / 44051 | `S_0059` / `G_0058` | `S_0060` / `G_0057` |
/// | 55168 / 44168 | `S_0063` / `G_0070` | `S_0064` / `G_0071` |
pub const DEVICE_CHANGE_ANTWORT_PIDS: &[(u32, u32, bool)] = &[
    // WiM Strom Teil 1 — UTILMD AHB Strom 2.2 Kap. 10.1–10.4.
    (55_040, 55_039, true),
    (55_041, 55_039, false),
    (55_043, 55_042, true),
    (55_044, 55_042, false),
    (55_052, 55_051, true),
    (55_053, 55_051, false),
    (55_169, 55_168, true),
    (55_170, 55_168, false),
    // AWH WiM Gas 2.0 — UTILMD AHB Gas 1.2 Kap. 6.1–6.5.
    (44_040, 44_039, true),
    (44_041, 44_039, false),
    (44_043, 44_042, true),
    (44_044, 44_042, false),
    (44_052, 44_051, true),
    (44_053, 44_051, false),
    (44_169, 44_168, true),
    // **44170 does not exist.** The Gas Verpflichtungsanfrage publishes a
    // Bestätigung and no Ablehnungs-PID (PID-Übersicht 4.0 rows 39240/39250);
    // the 44170 of PID 3.3 was withdrawn with FV2026-10-01. `E_2006` still
    // publishes the Ablehnungs-Codeliste `G_0071`, so the codes exist with no
    // carrier — [`antwort_pid_for`] returns `None` and the caller escalates
    // rather than inventing a Prüfidentifikator the market does not accept.
];

/// Resolve a response PID to `(request_pid, is_confirmed)`.
///
/// Returns `None` when `pid` is not a WiM MSB-Wechsel response.
#[must_use]
pub fn antwort_pid_meaning(pid: u32) -> Option<(u32, bool)> {
    DEVICE_CHANGE_ANTWORT_PIDS
        .iter()
        .find(|(antwort, _, _)| *antwort == pid)
        .map(|(_, request, confirmed)| (*request, *confirmed))
}

/// The Entscheidungsbaum that decides the answer to an MSB-Wechsel request.
///
/// The alphabets are disjoint and none of them is a GPKE one: a rejection is
/// `ZC9` / `Z29` / `Z34` / `E11` / `E17` / `Z09` here, never `A02` or `A05`.
/// Resolve a code with [`mako_pruefung::codes::lookup`] against the tree this
/// returns — a code alone identifies no meaning.
#[must_use]
pub fn wim_ebd(request_pid: u32) -> Option<&'static str> {
    use mako_pruefung::codes as c;
    match request_pid {
        // WiM Strom (EBD 4.3 Kap. 8) — codes ride the `S_00xx` Codelisten.
        55_039 => Some(c::EBD_KUENDIGUNG_MSB),
        55_042 => Some(c::EBD_ANMELDUNG_MSB),
        55_051 => Some(c::EBD_ABMELDUNG_MSB),
        55_168 => Some(c::EBD_VERPFLICHTUNGSANFRAGE),
        // WiM Gas (EBD 4.3 Kap. 14) — codes ride the `G_00xx` Codelisten and
        // share nothing with the Strom trees beyond the spelling of `E15`.
        44_039 => Some(c::EBD_KUENDIGUNG_MSB_GAS),
        44_042 => Some(c::EBD_ANMELDUNG_MSB_GAS),
        44_051 => Some(c::EBD_ABMELDUNG_MSB_GAS),
        44_168 => Some(c::EBD_VERPFLICHTUNGSANFRAGE_GAS),
        _ => None,
    }
}

/// The outbound answer PID for an inbound MSB-Wechsel request.
///
/// The inverse of [`antwort_pid_meaning`]: this one is used when *we* answer,
/// that one when the counterparty does.
///
/// Returns `None` when `request_pid` is not a WiM MSB-Wechsel request.
#[must_use]
pub fn antwort_pid_for(request_pid: u32, bestaetigt: bool) -> Option<u32> {
    DEVICE_CHANGE_ANTWORT_PIDS
        .iter()
        .find(|(_, request, confirmed)| *request == request_pid && *confirmed == bestaetigt)
        .map(|(antwort, _, _)| *antwort)
}

/// WiM Strom IFTSTA Prüfidentifikatoren (PIDs 21007, 21009–21015, 21018, 21029–21032).
///
/// These status messages are part of the WiM MSB-Wechsel (WiM Strom Teil 1)
/// process. All are routed to `"wim-device-change"` for correlation.
///
/// Per IFTSTA AHB these PIDs are "WiM / Statusmeldung MSB-Wechsel nach MsbG".
///
/// | PID   | Beschreibung | Richtung |
/// |-------|---|---|
/// | 21007 | Statusmeldung NB→LF / NB→MSBA | WiM Strom Teil 1 / WiM Gas |
/// | 21009 | Statusmeldung MSB-Wechsel nach MsbG an LF | NB → LF |
/// | 21010 | Statusmeldung MSB-Wechsel nach MsbG an NB | MSB alt → NB |
/// | 21011 | Statusmeldung MSB-Wechsel nach MsbG an NB | MSB neu → NB |
/// | 21012 | Statusmeldung MSB-Wechsel nach MsbG an BKV | NB → BKV |
/// | 21013 | Statusmeldung MSB-Wechsel nach MsbG an ÜNB | NB → ÜNB |
/// | 21015 | Statusmeldung Einbau iMS | wMSB → gMSB |
/// | 21018 | Statusmeldung Anforderung Datenzugang | MSB → LF |
/// | 21029 | Vorabinformation | wMSB → NB |
/// | 21030 | iMS-Ersteinbauzustand | wMSB → gMSB |
/// | 21031 | Bestandssituation / Eigenausbau iMS | wMSB → gMSB |
/// | 21032 | Antwort auf das Angebot | LF → MSB |
/// | 21036 | Zeitpunkt des Geräteausbaus | MSBN → MSBA |
pub const IFTSTA_PIDS: &[u32] = &[
    21_007, 21_009, 21_010, 21_011, 21_012, 21_013, 21_015, 21_018, 21_029, 21_030, 21_031, 21_032,
    21_036,
];

/// The five IFTSTA PIDs of the **Mitteilung über Gesamtvorgang** — the leg that
/// makes a Zuordnung constitutive (WiM Teil 1 Kap. 2.3.2 Nr. 7/8, 14–18).
///
/// The Anmeldebestätigung 55043 is *vorläufig*. Kap. 2.1.1: the NB assigns the
/// MSBN „zu dem Tag des vom MSBN mitgeteilten Termins des erfolgreichen
/// Abschlusses des Gesamtvorgangs … mit dem Zeitpunkt 00:00 Uhr", and the
/// MSBA's Zuordnung ends at the same instant. Until that report arrives the
/// MSBA stays assigned, and if the Gesamtvorgang fails it stays assigned for
/// good.
///
/// | PID | Meaning | Von → An |
/// |---|---|---|
/// | 21009 | Statusmeldung (**gescheitert**) | MSBN → NB |
/// | 21010 | Statusmeldung (**erfolgreich**) | MSBN → NB |
/// | 21011 | Statusmeldung (MSB-Scheitermeldung) | NB → MSBN / MSBA / LF |
/// | 21012 | Statusmeldung (**erfolgreich**) | NB → MSBN |
/// | 21013 | Statusmeldung (gescheitert) | NB → MSBN / MSBA / LF |
///
/// Source: IFTSTA AHB 2.1 § 6.2. Note that 21009 is the *failure* report and
/// 21010 the success — the numeric order is the reverse of the reading order.
pub const GESAMTVORGANG_PIDS: &[u32] = &[21_009, 21_010, 21_011, 21_012, 21_013];

/// IFTSTA 21010 — „Statusmeldung (erfolgreich) vom MSBN an NB".
///
/// Carries the Zuordnungsbeginn in `SG15 DTM+2380` under Bedingung `[521]`
/// („Zeitpunkt, ab dem der MSBN tatsächlich den Messstellenbetrieb übernimmt").
pub const GESAMTVORGANG_ERFOLG_PID: u32 = 21_010;

/// IFTSTA 21009 — „Statusmeldung (gescheitert) vom MSBN an NB".
pub const GESAMTVORGANG_SCHEITERN_PID: u32 = 21_009;

/// IFTSTA 21012 — the NB's positive answer; the Zuordnung has been made.
pub const ZUORDNUNG_ERFOLG_PID: u32 = 21_012;

/// IFTSTA 21011 — the NB's answer recording an MSB-Scheitermeldung.
pub const ZUORDNUNG_SCHEITERN_PID: u32 = 21_011;

/// IFTSTA 21013 — the NB reporting that no Gesamtvorgang report arrived at all
/// (Kap. 2.3.2 Nr. 16, spätester ÜT der 11. WT nach dem bestätigten
/// Zuordnungsbeginn).
pub const GESAMTVORGANG_AUSGEBLIEBEN_PID: u32 = 21_013;

/// Our obligation as MSBN to report the Gesamtvorgang — 10 Werktage after the
/// Zuordnungsbeginn the NB confirmed (Kap. 2.3.2 Nr. 7).
pub const GESAMTVORGANG_MELDUNG_WINDOW_LABEL: &str = "wim-gesamtvorgang-meldung";

/// Our obligation as NB to report that no Gesamtvorgang arrived — 11 Werktage
/// after the confirmed Zuordnungsbeginn (Kap. 2.3.2 Nr. 16).
pub const GESAMTVORGANG_AUSBLEIBEN_WINDOW_LABEL: &str = "wim-gesamtvorgang-ausbleiben";

/// Our obligation as NB to answer a Gesamtvorgang report — 1 Werktag
/// (Kap. 2.3.2 Nr. 8).
pub const ZUORDNUNG_ANTWORT_WINDOW_LABEL: &str = "wim-gesamtvorgang-antwort";

/// Werktage the MSBN has to report the Gesamtvorgang (Kap. 2.3.2 Nr. 7).
pub const GESAMTVORGANG_MELDUNG_WT: u32 = 10;

/// Werktage after which the NB reports an absent Gesamtvorgang
/// (Kap. 2.3.2 Nr. 16).
pub const GESAMTVORGANG_AUSBLEIBEN_WT: u32 = 11;

/// `YYYYMMDD` → a calendar date, for the dates that ride the wire as strings.
///
/// Returns `None` on anything else — a caller that cannot parse the date it was
/// given must not silently substitute today.
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

/// The APERAK *sending* deadline for an inbound WiM order, per Sparte.
///
/// Two different regimes, and the numbers do not overlap:
///
/// | Sparte | Window | Fundstelle |
/// |---|---|---|
/// | Strom | **45 Minuten** on a Werktag for UTILMD/ORDERS; Saturday → Sonntag 12:00 | APERAK AHB 1.1 §2.4.1 |
/// | Gas | nächster Werktag **12:00** (Folgeprozess) / **3 Werktage** (Initialprozess) | APERAK AHB 1.1 §2.3.1 |
///
/// The Gas branch picks its window from the Prüfidentifikator, because that is
/// what the BDEW made the discriminator — see
/// [`mako_fristen::GAS_INITIALPROZESS_PIDS`].
fn aperak_deadline(sparte: Sparte, pid: u32, received_at: OffsetDateTime) -> PendingDeadline {
    match sparte {
        Sparte::Strom => {
            PendingDeadline::new(APERAK_STROM_WINDOW_LABEL, aperak_strom_due_at(received_at))
        }
        Sparte::Gas => {
            let (label, due) = mako_fristen::aperak_gas_due_at(pid, received_at);
            PendingDeadline::new(label, due)
        }
    }
}

/// A calendar date as the 17:00 Europe/Berlin MaKo cut-off instant on it.
fn berlin_cutoff(date: time::Date) -> OffsetDateTime {
    mako_fristen::berlin_at(
        date,
        time::Time::from_hms(17, 0, 0).expect("17:00 is valid"),
    )
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gerätewechsel workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DeviceChangeEvent {
    /// Process initiated by a valid UTILMD Anmeldung Messstellenbetrieb.
    Initiated {
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// GLN of the incoming Messstellenbetreiber.
        incoming_msb: MarktpartnerCode,
        /// GLN of the grid operator (Netzbetreiber).
        grid_operator: MarktpartnerCode,
        /// Physical device identifier.
        device_id: DeviceId,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference (UNH/BGM).
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// `IDE+24` DE 7402 — the Vorgangsnummer the answer must echo.
        #[serde(default)]
        vorgangsnummer: Option<String>,
        /// `SG4 DTM` — the date the order asks for (YYYYMMDD).
        #[serde(default)]
        process_date: Option<String>,
        /// `SG4 STS+7` DE 9013 — the Transaktionsgrund the order stated.
        ///
        /// Muss on the Anfrage and on both answers, so the answer echoes it —
        /// see [`transaktionsgruende`].
        #[serde(default)]
        transaktionsgrund: Option<String>,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// A positive or negative APERAK was dispatched.
    ///
    /// The **technical** acknowledgement only (APERAK AHB 1.0 §2.1.1,
    /// BGM+312 / BGM+313), due 45 minutes after receipt for Strom UTILMD. It
    /// says the message could be processed — it decides nothing. The business
    /// decision is [`Self::AntwortGesendet`].
    AperakDispatched {
        /// `true` for positive (accepted), `false` for negative (rejected).
        positive: bool,
        /// Rejection reason (only set when `positive = false`).
        reason: Option<String>,
    },
    /// The **business** Bestätigung or Ablehnung was dispatched as a UTILMD on
    /// the answer Prüfidentifikator.
    ///
    /// This, not [`Self::AperakDispatched`], is what discharges
    /// [`ANTWORT_FRIST_WINDOW_LABEL`] and what the counterparty's EBD engine
    /// reads.
    AntwortGesendet {
        /// The answer PID — 55040/55041, 55043/55044, 55052/55053, 55169/55170.
        pruefidentifikator: Pruefidentifikator,
        /// `true` for a Bestätigung, `false` for an Ablehnung.
        bestaetigt: bool,
        /// `SG4 STS+E01` DE 9013 — the code from the EBD named below.
        antwort_code: String,
        /// `SG4 STS+E01` DE 1131 — the Entscheidungsbaum the code comes from.
        antwort_ebd: String,
        /// `FTX+ACB` Bemerkung, where the code or the operator supplied one.
        bemerkung: Option<String>,
        /// The date the answer confirms, when it differs from the requested
        /// one (`Z01` „Zustimmung mit Terminänderung", `Z12` nächstmöglicher
        /// Kündigungstermin).
        abweichender_termin: Option<String>,
    },
    /// The MSBN reported the outcome of the Gesamtvorgang (IFTSTA 21009/21010).
    ///
    /// „Erfolgreicher Abschluss des Gesamtvorgangs" is the situation that MSBA
    /// and MSBN have agreed on every technical installation the MSBN needs —
    /// through a Geräteübernahme, a Gerätewechsel, or both (Kap. 2.3.2 Nr. 5).
    /// The date it carries becomes the Zuordnungsbeginn.
    GesamtvorgangGemeldet {
        /// `true` for 21010 (erfolgreich), `false` for 21009 (gescheitert).
        erfolgreich: bool,
        /// `SG15 DTM+2380` — the day the MSBN actually takes over, 00:00 Uhr.
        /// `None` on a Scheitermeldung, which reports no takeover date.
        zuordnungsbeginn: Option<String>,
        /// Whether this party sent the report (MSBN side) or received it (NB).
        outbound: bool,
        /// EDIFACT message reference of the IFTSTA.
        message_ref: MessageRef,
    },
    /// The NB answered the Gesamtvorgang report (IFTSTA 21011/21012/21013).
    ///
    /// On 21012 the Zuordnung is made: the MSBN is assigned from
    /// `zuordnungsbeginn` 00:00 and the MSBA's assignment ends at the same
    /// instant (Kap. 2.1.1). On 21011/21013 the MSBA stays assigned.
    ZuordnungEntschieden {
        /// The answering PID — 21012, 21011 or 21013.
        pruefidentifikator: Pruefidentifikator,
        /// `true` only for 21012.
        zugeordnet: bool,
        /// The Zuordnungsbeginn the NB confirmed, on 21012.
        zuordnungsbeginn: Option<String>,
        /// Whether this party sent the answer (NB side) or received it (MSBN).
        outbound: bool,
    },
    /// Meter device physically changed; new MSB is active.
    Completed {
        /// Physical device identifier confirmed at completion.
        device_id: DeviceId,
    },
    /// Process was rejected and closed.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline expired before the process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received a WiM message that informs without asking for anything.
    ///
    /// Two kinds, and neither drives a state transition:
    ///
    /// * the **IFTSTA Statusmeldungen** (21007, 21013, 21018, 21025–21036) that
    ///   notify the parties of process status and Vollzugsmeldungen;
    /// * **UTILMD 44183** „Ende MSB von NB" — the Gas NB informing the MSB that
    ///   the Messlokation is being stilllegt (AWH WiM Gas 2.0 Kap. 3.7). It
    ///   carries a Transaktionsgrund of `Z33` and no answer at all: the AHB Gas
    ///   1.2 column has no `STS+E01` and no `RFF+TN`.
    ///
    /// Both are recorded in the event log for audit purposes.
    InformationEmpfangen {
        /// Prüfidentifikator of the informational message.
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// An **outbound** MSB-Wechsel order was dispatched by this party.
    ///
    /// Distinct from [`Self::Initiated`], which records an inbound UTILMD we
    /// *received*. Conflating the two would make the event log claim we were
    /// sent a message we in fact sent — the audit trail must record direction.
    AuftragGesendet {
        /// Messlokation the order applies to.
        melo_id: MeLo,
        /// GLN of this party (the order sender).
        sender: MarktpartnerCode,
        /// GLN of the counterparty (NB or nMSB, depending on PID).
        receiver: MarktpartnerCode,
        /// Requested execution date (YYYYMMDD, German local time).
        process_date: String,
        /// EDIFACT message reference of the outbound UTILMD.
        message_ref: MessageRef,
        /// Prüfidentifikator (55039, 55042, 55051, or 55168).
        pruefidentifikator: Pruefidentifikator,
    },
    /// The counterparty answered our outbound order (Bestätigung or Ablehnung).
    ///
    /// Closes the loop opened by [`Self::AuftragGesendet`] and absorbs the
    /// 5-Werktage response deadline.
    AntwortEmpfangen {
        /// Response Prüfidentifikator — see [`DEVICE_CHANGE_ANTWORT_PIDS`].
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the answering counterparty.
        sender: MarktpartnerCode,
        /// EDIFACT message reference of the inbound response.
        message_ref: MessageRef,
        /// `true` for a Bestätigung, `false` for an Ablehnung.
        is_confirmed: bool,
        /// Rejection reason, when the counterparty supplied one.
        reason: Option<String>,
        /// The date the answer states when it moves ours — `Z01` on a
        /// Bestätigung, `Z12` on a Kündigungsablehnung. It replaces the
        /// requested date for everything downstream.
        #[serde(default)]
        bestaetigter_termin: Option<String>,
    },
}

impl EventPayload for DeviceChangeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragGesendet { .. } => "WimDeviceChangeAuftragGesendet",
            Self::AntwortEmpfangen { .. } => "WimDeviceChangeAntwortEmpfangen",
            Self::Initiated { .. } => "WimDeviceChangeInitiated",
            Self::ValidationPassed { .. } => "WimDeviceChangeValidationPassed",
            Self::AperakDispatched { .. } => "WimDeviceChangeAperakDispatched",
            Self::AntwortGesendet { .. } => "WimDeviceChangeAntwortGesendet",
            Self::GesamtvorgangGemeldet { .. } => "WimDeviceChangeGesamtvorgangGemeldet",
            Self::ZuordnungEntschieden { .. } => "WimDeviceChangeZuordnungEntschieden",
            Self::Completed { .. } => "WimDeviceChangeCompleted",
            Self::Rejected { .. } => "WimDeviceChangeRejected",
            Self::DeadlineExpired { .. } => "WimDeviceChangeDeadlineExpired",
            Self::InformationEmpfangen { .. } => "WimDeviceChangeInformationEmpfangen",
        }
    }
    // schema_version defaults to 1; increment and add an upcast arm on next
    // backward-incompatible payload layout change.
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set at `Initiated` time and carried through every later state.
///
/// All fields are structurally guaranteed to be present once the process moves
/// past `New` — no `unwrap()` required downstream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceChangeData {
    /// EIC/MeLo code for the metering location.
    pub melo_id: MeLo,
    /// Market partner code (GLN) of the incoming MSB.
    pub incoming_msb: MarktpartnerCode,
    /// Market partner code (GLN) of the grid operator.
    pub grid_operator: MarktpartnerCode,
    /// Device identifier.
    pub device_id: DeviceId,
    /// EDIFACT document date string from the UTILMD.
    pub document_date: String,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: Pruefidentifikator,
    /// Original UTILMD message reference, preserved for APERAK construction.
    /// `None` only for processes initiated before this field was added (old snapshots).
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
    /// `IDE+24` DE 7402 of the inbound order.
    ///
    /// The answer must echo it — that is how the counterparty correlates a
    /// Bestätigung to the Vorgang it sent, and the message reference is not a
    /// substitute: one interchange can carry several Vorgänge.
    #[serde(default)]
    pub vorgangsnummer: Option<String>,
    /// The date the order asks for (`SG4 DTM+76`, YYYYMMDD).
    ///
    /// Carried because the answer states a date too, and because it is the
    /// anchor every Vorlauffrist check in
    /// [`mako_fristen::vorlauf`] measures against.
    #[serde(default)]
    pub process_date: Option<String>,
    /// The Zuordnungsbeginn the NB **confirmed** (YYYYMMDD).
    ///
    /// Distinct from [`Self::process_date`], which is what the order *asked*
    /// for: a Bestätigung may carry `Z01` and move it. Everything downstream —
    /// the Realisierungskorridor, the 10- and 11-Werktage Gesamtvorgang
    /// windows, and the date `marktd` records — measures against this one.
    #[serde(default)]
    pub bestaetigter_zuordnungsbeginn: Option<String>,
    /// `SG4 STS+7` DE 9013 — the Transaktionsgrund the order stated.
    ///
    /// Muss on the Anfrage **and** on both answers, so the answer has to echo
    /// what arrived rather than restate a default. `None` on a stream opened
    /// before the field existed, and on the REST channel, which carries no
    /// EDIFACT Transaktionsgrund; both fall back to
    /// [`TRANSAKTIONSGRUND_WECHSEL`] at render time.
    #[serde(default)]
    pub transaktionsgrund: Option<String>,
}

/// Current state of a WiM Gerätewechsel process stream.
///
/// Modelled as an enum-per-variant to eliminate all `Option`-unwraps:
/// each variant carries exactly the data that is structurally available at
/// that stage. Invalid states are unrepresentable.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Completed
///                                    ↘ Rejected
///     ↘ Rejected (failed validation at Initiated step)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum DeviceChangeState {
    /// No events yet; stream exists but process has not started.
    #[default]
    New,
    /// Outbound MSB-Wechsel order dispatched; awaiting the counterparty's answer.
    AuftragGesendet(DeviceChangeData),
    /// Counterparty confirmed our outbound order; awaiting the physical device swap.
    AuftragBestaetigt(DeviceChangeData),
    /// UTILMD received and `Initiated` event applied.
    Initiated(DeviceChangeData),
    /// EDIFACT validation passed; APERAK not yet dispatched.
    ValidationPassed(DeviceChangeData),
    /// Positive APERAK dispatched; the business answer is still owed.
    AperakSent(DeviceChangeData),
    /// The business Bestätigung/Ablehnung went out; awaiting the physical
    /// device swap (or, for an Ablehnung, nothing further).
    AntwortGesendet(DeviceChangeData),
    /// The Gesamtvorgang outcome has been reported and the Zuordnung is not
    /// decided yet — the NB owes its answer within one Werktag.
    GesamtvorgangGemeldet(DeviceChangeData),
    /// Device physically changed; new MSB is active.
    Completed(DeviceChangeData),
    /// Process rejected (validation failure or negative APERAK).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for DeviceChangeState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Outbound order awaiting an answer, or an inbound order being
            // worked through validation, APERAK and the physical swap.
            Self::AuftragGesendet(_)
            | Self::AuftragBestaetigt(_)
            | Self::Initiated(_)
            | Self::ValidationPassed(_)
            | Self::AperakSent(_)
            | Self::AntwortGesendet(_)
            | Self::GesamtvorgangGemeldet(_) => true,
            // Terminal. A meter can be changed more than once, so a completed
            // Gerätewechsel must not retire the MeLo.
            Self::New | Self::Completed(_) | Self::Rejected { .. } => false,
        }
    }
}

impl DeviceChangeState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragGesendet(_) => "AuftragGesendet",
            Self::AuftragBestaetigt(_) => "AuftragBestaetigt",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AperakSent(_) => "AperakSent",
            Self::AntwortGesendet(_) => "AntwortGesendet",
            Self::GesamtvorgangGemeldet(_) => "GesamtvorgangGemeldet",
            Self::Completed(_) => "Completed",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Gerätewechsel workflow.
///
/// **All domain values must be pre-extracted by the transport layer** before
/// constructing a command. `Workflow::handle()` is pure — no I/O, no EDIFACT
/// parsing, no external calls. See the crate-level doc for a construction
/// example.
#[derive(Clone)]
pub enum DeviceChangeCommand {
    /// ERP instructs this party to **send** a WiM MSB-Wechsel order.
    ///
    /// Emits [`DeviceChangeEvent::AuftragGesendet`] plus a `UTILMD` outbox entry.
    /// The caller registers the counterparty-response deadline
    /// ([`AUFTRAG_ANTWORT_WINDOW_LABEL`]) alongside it, sized per process via
    /// [`antwort_frist_werktage`] — 3 / 5 / 7 / 1 WT, **not** one flat window.
    ///
    /// Direction is per-PID and not a uniform "MSB → NB" split — see
    /// [`DEVICE_CHANGE_PIDS`] for the table. In particular 55039 never reaches
    /// the NB (MSBN → MSBA) and 55168 addresses the gMSB.
    ///
    /// The command itself is role-agnostic; `makod`'s command API enforces which
    /// Marktrolle may issue which PID.
    InitiateDeviceChange {
        /// Prüfidentifikator; must be one of [`DEVICE_CHANGE_PIDS`].
        pid: Pruefidentifikator,
        /// GLN of this party (order sender).
        sender: MarktpartnerCode,
        /// GLN of the counterparty (order receiver).
        receiver: MarktpartnerCode,
        /// Messlokation the order applies to.
        melo_id: MeLo,
        /// Requested execution date (YYYYMMDD, German local time).
        process_date: String,
        /// EDIFACT message reference of the outbound UTILMD.
        message_ref: MessageRef,
    },
    /// Inbound Bestätigung / Ablehnung answering our outbound order.
    ///
    /// Emits [`DeviceChangeEvent::AntwortEmpfangen`] and closes the
    /// [`AUFTRAG_ANTWORT_WINDOW_LABEL`] deadline by leaving `AuftragGesendet`.
    ReceiveAntwort {
        /// Response Prüfidentifikator — see [`DEVICE_CHANGE_ANTWORT_PIDS`].
        pid: Pruefidentifikator,
        /// GLN of the answering counterparty.
        sender: MarktpartnerCode,
        /// EDIFACT message reference of the inbound response.
        message_ref: MessageRef,
        /// Rejection reason, when the counterparty supplied one.
        reason: Option<String>,
        /// `SG4 DTM` on an answer that moves the date (`Z01`, `Z12`).
        bestaetigter_termin: Option<String>,
    },
    /// Inbound UTILMD accepted from the AS4 layer. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator.
        pid: Pruefidentifikator,
        /// GLN of the message sender (nMSB).
        sender: MarktpartnerCode,
        /// GLN of the message receiver (NB).
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Physical device identifier.
        device_id: DeviceId,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `IDE+24` DE 7402 — the Vorgangsnummer the answer must echo.
        ///
        /// Not the message reference: one interchange can carry several
        /// Vorgänge, so echoing `UNH` would correlate the answer to the wrong
        /// one whenever a counterparty batches.
        vorgangsnummer: Option<String>,
        /// `SG4 DTM` — the date the order asks for (YYYYMMDD).
        ///
        /// Which qualifier carries it depends on the Anwendungsfall: `DTM+93`
        /// (Datum Vertragsende, XOR `DTM+471`) on a Kündigung and an Ende MSB,
        /// `DTM+76` (Lieferdatum/-zeit, geplant) on an Anmeldung and a
        /// Verpflichtungsanfrage.
        process_date: Option<String>,
        /// `SG4 STS+7` DE 9013 — the Transaktionsgrund the order stated.
        ///
        /// Muss on every WiM MSB-Wechsel Prüfidentifikator, and the answer
        /// echoes it. `None` from a source that has no such field (the REST
        /// channel), where the render-time default applies.
        transaktionsgrund: Option<String>,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
        /// UTC wall-clock time when the inbound UTILMD was received.
        ///
        /// Sizes both clocks the arrival starts: the APERAK sending window
        /// (45 min in Strom, the Initial-/Folgeprozess split in Gas) and the
        /// business Antwortfrist (3 / 5 / 7 / 1 Werktage per PID).
        received_at: OffsetDateTime,
    },
    /// Inbound iMS Universalbestellprozess order received via REST
    /// (BDEW API-Webdienste Strom, valid 2026-01-29+, PIDs 11021–11023).
    ///
    /// Used when the Netzbetreiber orders an iMS installation from the MSB
    /// through the REST channel rather than via EDIFACT/AS4. The caller is
    /// responsible for validating the request before constructing this command.
    ReceiveRestOrder {
        /// REST transaction UUID (idempotency key; carried through to events).
        tx_id: String,
        /// 13-digit GLN of the Netzbetreiber (order sender).
        sender_mp_id: MarktpartnerCode,
        /// EIC of the Messlokation at which the device should be installed.
        melo_id: MeLo,
        /// Requested device category (e.g. `"iMSys"`, `"mME"`, `"mME+KME"`).
        device_category: String,
        /// Requested installation / process date (ISO 8601 date string).
        process_date: String,
    },
    /// Dispatch the APERAK — the technical acknowledgement, on its own clock.
    ///
    /// Strom: **45 Minuten** for a UTILMD received on a Werktag, both
    /// polarities. Gas: **only** the Verarbeitbarkeitsfehlermeldung, by the
    /// next Werktag 12:00 or, on an Initialprozess, within 3 Werktagen
    /// (APERAK AHB 1.1 §2.3.1/§2.4.1). A `positive: true` in Gas is refused.
    DispatchAperak {
        /// `true` for positive APERAK, `false` for negative.
        positive: bool,
        /// Rejection reason (required when `positive = false`).
        reason: Option<String>,
    },
    /// Dispatch the **business** Bestätigung or Ablehnung as a UTILMD on the
    /// answer Prüfidentifikator.
    ///
    /// This is the message the Festlegung means by „Antwort": UTILMD 55040 /
    /// 55043 / 55052 / 55169 (Bestätigung) or 55041 / 55044 / 55053 / 55170
    /// (Ablehnung), carrying `SG4 STS+E01` with a code from the process's
    /// Entscheidungsbaum.
    ///
    /// **It is not the APERAK.** That is a transport-level statement that the
    /// message could be processed, due in 45 minutes; this is the decision, due
    /// in 3 / 5 / 7 / 1 Werktagen.
    ///
    /// `antwort_code` is resolved against the process's own Entscheidungsbaum
    /// ([`mako_pruefung::codes`]) before anything is enqueued, so a code from
    /// the wrong tree is refused here rather than becoming an unparseable
    /// answer on the wire.
    DispatchAntwort {
        /// `true` for a Bestätigung, `false` for an Ablehnung.
        bestaetigt: bool,
        /// `SG4 STS+E01` DE 9013. Must be published in the request PID's EBD
        /// and in the matching cluster.
        antwort_code: String,
        /// `FTX+ACB` free text. Required by the AHB alongside a catch-all
        /// rejection and useful on every other.
        bemerkung: Option<String>,
        /// The date the answer confirms when it differs from the requested one
        /// — `Z01` „Zustimmung mit Terminänderung" and `Z12` (nächstmöglicher
        /// Kündigungstermin) both require it.
        abweichender_termin: Option<String>,
    },
    /// Report the outcome of the Gesamtvorgang as the **MSBN**
    /// (IFTSTA 21010 erfolgreich / 21009 gescheitert, Kap. 2.3.2 Nr. 7).
    ///
    /// The date is what makes the Zuordnung: the NB assigns the MSBN from it,
    /// 00:00 Uhr, and ends the MSBA's assignment at the same instant
    /// (Kap. 2.1.1). It must lie inside the ±9-Werktage Realisierungskorridor
    /// around the Zuordnungsbeginn the NB confirmed, which this command checks.
    MeldeGesamtvorgang {
        /// `true` for the erfolgreicher Abschluss, `false` for the Scheitern.
        erfolgreich: bool,
        /// `SG15 DTM+2380` (YYYYMMDD) — required when `erfolgreich`.
        zuordnungsbeginn: Option<String>,
    },
    /// Inbound Gesamtvorgang report from the MSBN, as the **NB**.
    ReceiveGesamtvorgang {
        /// 21010 (erfolgreich) or 21009 (gescheitert).
        pid: Pruefidentifikator,
        /// The takeover date the report carries.
        zuordnungsbeginn: Option<String>,
        /// EDIFACT message reference of the IFTSTA.
        message_ref: MessageRef,
    },
    /// Decide the Zuordnung as the **NB** (IFTSTA 21012 / 21011, Nr. 8).
    ///
    /// `zugeordnet = false` records the MSB-Scheitermeldung: the MSBA stays
    /// assigned and continues the Messstellenbetrieb or starts an Ende MSB of
    /// its own (Nr. 14).
    DispatchZuordnung {
        /// `true` for 21012, `false` for 21011.
        zugeordnet: bool,
    },
    /// Inbound Zuordnungsantwort from the NB, as the **MSBN**
    /// (IFTSTA 21012 / 21011 / 21013).
    ReceiveZuordnungsantwort {
        /// 21012, 21011 or 21013.
        pid: Pruefidentifikator,
        /// The Zuordnungsbeginn the NB confirmed, on 21012.
        zuordnungsbeginn: Option<String>,
    },
    /// Mark the device change as completed once the physical swap is confirmed.
    Complete {
        /// Physical device identifier confirmed at completion.
        device_id: DeviceId,
    },
    /// A registered deadline fired and was dispatched by the scheduler.
    ///
    /// Transitions the process to `Rejected` unless it has already reached
    /// a terminal state (`Completed` or `Rejected`), in which case this is a no-op.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
    /// Received a WiM message that informs without asking for anything.
    ///
    /// Covers the IFTSTA Statusmeldungen and the Gas UTILMD
    /// [`ENDE_MSB_VOM_NB_PID`]. Constructed by the `makod` adapters on an
    /// inbound AS4 message, or via the `"wim.iftsta.empfangen"` REST command.
    ReceiveInformation {
        /// Prüfidentifikator of the informational message.
        pid: Pruefidentifikator,
        /// Sender party code (GLN).
        sender: MarktpartnerCode,
        /// Receiver party code (GLN).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Whether the message passed AHB validation.
        validation_passed: bool,
        /// Validation errors collected by the AHB validator.
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for DeviceChangeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Messstellenbetrieb (PIDs 55039, 55042, 55051, 55168) workflow.
///
/// Covers the four Use-Cases of WiM Strom Teil 1 Kap. 2 and the IFTSTA
/// Gesamtvorgang leg that closes them. An inbound order is acknowledged with an
/// APERAK within 45 minutes and answered with a UTILMD within 3 / 5 / 7 / 1
/// Werktagen; on a 55042 the Zuordnung then follows the Gesamtvorgang report.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<WimDeviceChangeWorkflow>(
///     tenant_id,
///     WorkflowId::new("wim-device-change", "FV2025-10-01"),
/// );
/// ```
pub struct WimDeviceChangeWorkflow;

impl Workflow for WimDeviceChangeWorkflow {
    type State = DeviceChangeState;
    type Event = DeviceChangeEvent;
    type Command = DeviceChangeCommand;

    /// Deadline compensation for the windows this workflow arms.
    ///
    /// | Label | State guard | Frist |
    /// |---|---|---|
    /// | [`ANTWORT_FRIST_WINDOW_LABEL`] | `Initiated` / `ValidationPassed` | 3 / 5 / 7 / 1 WT per PID |
    /// | [`AUFTRAG_ANTWORT_WINDOW_LABEL`] | `AuftragGesendet` | the counterparty's, same numbers |
    /// | [`GESAMTVORGANG_MELDUNG_WINDOW_LABEL`] | `AuftragBestaetigt` | 10 WT nach dem bestätigten Zuordnungsbeginn |
    /// | [`GESAMTVORGANG_AUSBLEIBEN_WINDOW_LABEL`] | `AntwortGesendet` | 11 WT |
    /// | [`ZUORDNUNG_ANTWORT_WINDOW_LABEL`] | `GesamtvorgangGemeldet` | 1 WT |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ANTWORT_FRIST_WINDOW_LABEL,
                DeviceChangeState::Initiated(_) | DeviceChangeState::ValidationPassed(_),
            ) => Some(DeviceChangeCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            // Counterparty missed its answer window on our outbound order.
            (AUFTRAG_ANTWORT_WINDOW_LABEL, DeviceChangeState::AuftragGesendet(_)) => {
                Some(DeviceChangeCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            // We are the MSBN and let the 10-Werktage Gesamtvorgang window
            // lapse. The NB will report the Scheitern on the 11. WT and the
            // MSBA stays assigned (Kap. 2.3.2 Nr. 16), so the process is over.
            (GESAMTVORGANG_MELDUNG_WINDOW_LABEL, DeviceChangeState::AuftragBestaetigt(_)) => {
                Some(DeviceChangeCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            // We are the NB and no Gesamtvorgang report arrived. „Es liegt nach
            // maximaler Frist des Gesamtvorgangs zu Geräteübernahme /
            // Gerätewechsel keine Meldung des MSBN beim NB vor. Der MSBA bleibt
            // der einzelnen Messlokation zugeordnet."
            (GESAMTVORGANG_AUSBLEIBEN_WINDOW_LABEL, DeviceChangeState::AntwortGesendet(_)) => {
                Some(DeviceChangeCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            // We owe the Zuordnungsantwort and did not send it.
            (ZUORDNUNG_ANTWORT_WINDOW_LABEL, DeviceChangeState::GesamtvorgangGemeldet(_)) => {
                Some(DeviceChangeCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            DeviceChangeEvent::AuftragGesendet {
                melo_id,
                sender,
                receiver,
                process_date,
                message_ref,
                pruefidentifikator,
            } => DeviceChangeState::AuftragGesendet(DeviceChangeData {
                // An outbound order states its own Grund; the workflow carries
                // the default until the caller supplies one.
                transaktionsgrund: None,
                melo_id: melo_id.clone(),
                // On an outbound order this party is the sender; `incoming_msb`
                // and `grid_operator` are populated by PID direction so the
                // projection stays meaningful either way.
                incoming_msb: sender.clone(),
                grid_operator: receiver.clone(),
                device_id: DeviceId::new(""),
                document_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
                vorgangsnummer: None,
                process_date: Some(process_date.clone()),
                bestaetigter_zuordnungsbeginn: None,
            }),
            DeviceChangeEvent::AntwortEmpfangen {
                is_confirmed,
                reason,
                bestaetigter_termin,
                ..
            } => match state {
                DeviceChangeState::AuftragGesendet(mut data) => {
                    if *is_confirmed {
                        // A `Z01` Bestätigung moves the date; everything
                        // downstream measures against what the NB confirmed,
                        // not against what we asked for.
                        data.bestaetigter_zuordnungsbeginn = bestaetigter_termin
                            .clone()
                            .or_else(|| data.process_date.clone());
                        DeviceChangeState::AuftragBestaetigt(data)
                    } else {
                        DeviceChangeState::Rejected {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "Auftrag vom Marktpartner abgelehnt".to_owned()),
                        }
                    }
                }
                other => other,
            },
            DeviceChangeEvent::Initiated {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                document_date,
                message_ref,
                pruefidentifikator,
                vorgangsnummer,
                process_date,
                transaktionsgrund,
            } => DeviceChangeState::Initiated(DeviceChangeData {
                transaktionsgrund: transaktionsgrund.clone(),
                melo_id: melo_id.clone(),
                incoming_msb: incoming_msb.clone(),
                grid_operator: grid_operator.clone(),
                device_id: device_id.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
                vorgangsnummer: vorgangsnummer.clone(),
                process_date: process_date.clone(),
                bestaetigter_zuordnungsbeginn: None,
            }),
            DeviceChangeEvent::ValidationPassed { .. } => {
                if let DeviceChangeState::Initiated(data) = state {
                    DeviceChangeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            DeviceChangeEvent::AperakDispatched { positive, .. } => match state {
                DeviceChangeState::ValidationPassed(data) => {
                    if *positive {
                        DeviceChangeState::AperakSent(data)
                    } else {
                        DeviceChangeState::Rejected {
                            reason: "negative APERAK".to_owned(),
                        }
                    }
                }
                _ => state,
            },
            DeviceChangeEvent::AntwortGesendet {
                bestaetigt,
                abweichender_termin,
                ..
            } => match state {
                DeviceChangeState::ValidationPassed(mut data)
                | DeviceChangeState::AperakSent(mut data) => {
                    if *bestaetigt {
                        data.bestaetigter_zuordnungsbeginn = abweichender_termin
                            .clone()
                            .or_else(|| data.process_date.clone());
                        DeviceChangeState::AntwortGesendet(data)
                    } else {
                        // An Ablehnung is terminal for us: the counterparty
                        // must start a new Vorgang, and nothing further is
                        // owed on this one.
                        DeviceChangeState::Rejected {
                            reason: "Ablehnung versendet".to_owned(),
                        }
                    }
                }
                other => other,
            },
            DeviceChangeEvent::GesamtvorgangGemeldet {
                erfolgreich,
                zuordnungsbeginn,
                ..
            } => match state {
                DeviceChangeState::AntwortGesendet(mut data)
                | DeviceChangeState::AuftragBestaetigt(mut data) => {
                    if *erfolgreich {
                        if let Some(d) = zuordnungsbeginn {
                            data.bestaetigter_zuordnungsbeginn = Some(d.clone());
                        }
                        DeviceChangeState::GesamtvorgangGemeldet(data)
                    } else {
                        // „Bei Mitteilung des Scheiterns des Gesamtvorgangs
                        // bleibt der MSBA der einzelnen Messlokation zugeordnet."
                        DeviceChangeState::Rejected {
                            reason: "Gesamtvorgang gescheitert — der MSBA bleibt zugeordnet"
                                .to_owned(),
                        }
                    }
                }
                other => other,
            },
            DeviceChangeEvent::ZuordnungEntschieden {
                zugeordnet,
                zuordnungsbeginn,
                ..
            } => match state {
                DeviceChangeState::GesamtvorgangGemeldet(mut data) => {
                    if *zugeordnet {
                        if let Some(d) = zuordnungsbeginn {
                            data.bestaetigter_zuordnungsbeginn = Some(d.clone());
                        }
                        DeviceChangeState::Completed(data)
                    } else {
                        DeviceChangeState::Rejected {
                            reason: "Zuordnung nicht erfolgt — der MSBA bleibt zugeordnet"
                                .to_owned(),
                        }
                    }
                }
                other => other,
            },
            DeviceChangeEvent::Completed { device_id } => match state {
                DeviceChangeState::AperakSent(mut data)
                | DeviceChangeState::AntwortGesendet(mut data)
                | DeviceChangeState::GesamtvorgangGemeldet(mut data)
                | DeviceChangeState::AuftragBestaetigt(mut data) => {
                    data.device_id = device_id.clone();
                    DeviceChangeState::Completed(data)
                }
                other => other,
            },
            DeviceChangeEvent::Rejected { reason } => DeviceChangeState::Rejected {
                reason: reason.clone(),
            },
            DeviceChangeEvent::DeadlineExpired { label, .. } => match state {
                DeviceChangeState::Completed(_) | DeviceChangeState::Rejected { .. } => state,
                _ => DeviceChangeState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },

            // Informational WiM IFTSTA status messages do not change state.
            DeviceChangeEvent::InformationEmpfangen { .. } => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            DeviceChangeCommand::InitiateDeviceChange {
                pid,
                sender,
                receiver,
                melo_id,
                process_date,
                message_ref,
            } => {
                if !matches!(state, DeviceChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !DEVICE_CHANGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a WiM MSB-Wechsel PID (55039, 55042, 55051, 55168), got {pid}",
                    )));
                }

                // Key set required by `edifact_renderer::render_utilmd` for a WiM
                // UTILMD: pid, sender, receiver, melo, process_date.
                let outbox = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":    "outbound",
                        "pid":          pid.as_u32(),
                        "sender":       sender.as_str(),
                        "receiver":     receiver.as_str(),
                        "melo":         melo_id.as_str(),
                        "process_date": process_date,
                        "message_ref":  message_ref.as_str(),
                    }),
                )
                .caused_by(0);

                let event = DeviceChangeEvent::AuftragGesendet {
                    melo_id,
                    sender,
                    receiver,
                    process_date,
                    message_ref,
                    pruefidentifikator: pid,
                };
                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            DeviceChangeCommand::ReceiveAntwort {
                pid,
                sender,
                message_ref,
                reason,
                bestaetigter_termin,
            } => {
                let DeviceChangeState::AuftragGesendet(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "AuftragGesendet",
                        state.status_str(),
                    ));
                };

                let Some((request_pid, is_confirmed)) = antwort_pid_meaning(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a WiM MSB-Wechsel Antwort (expected one of \
                         55040, 55041, 55043, 55044, 55052, 55053, 55169, 55170)",
                    )));
                };

                // The answer must belong to the order we actually sent. Without this
                // check a 55043 (Anmeldung confirmed) could silently close a 55039
                // (Kündigung) order and the audit trail would claim the wrong process
                // completed.
                if request_pid != data.pruefidentifikator.as_u32() {
                    return Err(WorkflowError::rejected(format!(
                        "Antwort PID {pid} answers request {request_pid}, but this process \
                         sent {}",
                        data.pruefidentifikator,
                    )));
                }

                // A Bestätigung on an Anmeldung opens the Gesamtvorgang leg:
                // Kap. 2.3.2 Nr. 7 gives us 10 Werktage from the confirmed
                // Zuordnungsbeginn to report its outcome, and until we do the
                // MSBA stays assigned.
                let events = vec![DeviceChangeEvent::AntwortEmpfangen {
                    pruefidentifikator: pid,
                    sender,
                    message_ref,
                    is_confirmed,
                    reason,
                    bestaetigter_termin: bestaetigter_termin.clone(),
                }];
                let beginn = bestaetigter_termin.or_else(|| data.process_date.clone());
                match (
                    is_confirmed,
                    request_pid,
                    beginn.as_deref().and_then(parse_yyyymmdd),
                ) {
                    (true, 55_042, Some(d)) => Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![],
                        vec![PendingDeadline::new(
                            GESAMTVORGANG_MELDUNG_WINDOW_LABEL,
                            berlin_cutoff(mako_fristen::add_werktage(
                                d,
                                GESAMTVORGANG_MELDUNG_WT,
                                HolidayCalendar::BdewMaKo,
                            )),
                        )],
                    )),
                    _ => Ok(events.into()),
                }
            }

            DeviceChangeCommand::ReceiveRestOrder {
                tx_id,
                sender_mp_id,
                melo_id,
                device_category,
                process_date,
            } => {
                if !matches!(state, DeviceChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                // PID 55042 (WiM MSB Anmeldung Strom) is the canonical EDIFACT process
                // identifier for iMSys Universalbestellprozess Anmeldung regardless of
                // transport channel.  REST (API-Webdienste Strom) and EDIFACT (UTILMD)
                // both initiate the same underlying MaKo process; 55042 keeps the audit
                // trail consistent and avoids phantom PIDs in the event store.
                let pid = Pruefidentifikator::new(55_042).map_err(|e| {
                    WorkflowError::rejected(format!(
                        "constant PID 55042 (WiM Anmeldung MSB) invalid: {e}"
                    ))
                })?;
                // REST orders carry no EDIFACT device ID; use the tx_id as a
                // provisional placeholder until the MSB assigns a device EIC.
                let device_id = DeviceId::new(&*tx_id);
                let message_ref = MessageRef::new(&*tx_id);
                Ok(vec![
                    DeviceChangeEvent::Initiated {
                        melo_id,
                        incoming_msb: sender_mp_id,
                        // REST orders target the MSB (self); grid_operator is
                        // not known at this point — carry device_category in
                        // document_date for now (process_date holds the date).
                        grid_operator: MarktpartnerCode::new(""),
                        device_id,
                        document_date: format!("{process_date}|category={device_category}"),
                        message_ref: message_ref.clone(),
                        pruefidentifikator: pid,
                        // Nor a Transaktionsgrund: the API-Webdienste payload
                        // has no field for `SG4 STS+7`, so the render-time
                        // default applies.
                        transaktionsgrund: None,
                        // The REST channel carries no EDIFACT Vorgangsnummer;
                        // the transaction id is what the counterparty echoes.
                        vorgangsnummer: Some(tx_id.clone()),
                        process_date: Some(process_date.clone()),
                    },
                    // REST-sourced orders are structurally valid by definition
                    // (the HTTP layer validated the JSON payload); emit
                    // ValidationPassed immediately.
                    DeviceChangeEvent::ValidationPassed { message_ref },
                ]
                .into())
            }

            DeviceChangeCommand::ReceiveUtilmd {
                pid,
                transaktionsgrund,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                message_ref,
                vorgangsnummer,
                process_date,
                validation_passed,
                validation_errors,
                received_at,
            } => {
                if !matches!(state, DeviceChangeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                // PID guard: reject any PID not in the WiM MSB-Wechsel family.
                // `WimModule` registers exactly `DEVICE_CHANGE_PIDS`; this guard
                // is defence-in-depth for direct callers.
                let Some(sparte) = wim_sparte(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not a WiM Messstellenbetrieb PID (expected one of {DEVICE_CHANGE_PIDS:?})",
                        pid.as_u32()
                    )));
                };
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![DeviceChangeEvent::Initiated {
                    melo_id,
                    incoming_msb: sender,
                    grid_operator: receiver,
                    device_id,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                    vorgangsnummer,
                    process_date,
                    transaktionsgrund,
                }];
                if validation_passed {
                    events.push(DeviceChangeEvent::ValidationPassed { message_ref });
                    // The APERAK is dispatched by the ERP, never auto-emitted here:
                    // DispatchAperak is the single decision point for both the
                    // positive (BGM+312) and the negative (BGM+313) one.
                    //
                    // Register TWO deadlines atomically with the events:
                    //   1. APERAK Strom *sending* deadline (APERAK AHB 1.0 \u00a72.4.1):
                    //      weekday = 45 min; Saturday = Sunday noon.
                    //   2. The *business answer* deadline \u2014 Best\u00e4tigung or Ablehnung.
                    //      Sized per PID (3 / 5 / 7 / 1 WT), never flat: see
                    //      `antwort_frist_werktage`. The PID guard above already
                    //      rejected anything outside the family, so the lookup
                    //      cannot fail here.
                    let aperak_send_dl = aperak_deadline(sparte, pid.as_u32(), received_at);
                    let frist_wt = antwort_frist_werktage(pid.as_u32())
                        .expect("PID guard above restricts this to the MSB-Wechsel family");
                    let process_dl = PendingDeadline::new(
                        ANTWORT_FRIST_WINDOW_LABEL,
                        deadline_at_werktage(received_at, frist_wt, HolidayCalendar::BdewMaKo),
                    );
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![],
                        vec![aperak_send_dl, process_dl],
                    ))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(DeviceChangeEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 \u2014 mandatory per APERAK AHB 1.0 \u00a72.1.1.
                    // Validation failed \u2192 APERAK sent immediately: register the 45-min
                    // *sending* deadline so the OutboxWorker is monitored (APERAK AHB 1.0 \u00a72.4.1).
                    let aperak_send_dl = aperak_deadline(sparte, pid.as_u32(), received_at);
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_mp_id.as_str(),
                                "pid":        29001_u32,
                                "positive":   false,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        outbox,
                        vec![aperak_send_dl],
                    ))
                }
            }

            DeviceChangeCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    DeviceChangeState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                // The APERAK is the **technical** acknowledgement and decides
                // nothing about the business case; the Bestätigung/Ablehnung is
                // `DispatchAntwort`, on its own 3/5/7/1-Werktage clock.
                //
                //   positive = true  → BGM+312 Anerkennungsmeldung
                //   positive = false → BGM+313 Verarbeitbarkeitsfehlermeldung
                //
                // **Only Strom has both.** In Gas the APERAK reports
                // „ausschließlich" errors (APERAK AHB 1.1 §2.3): a processable
                // message is acknowledged by the Frist lapsing in silence. The
                // decision is still recorded — the ERP made one — but the entry
                // carries `suppress_wire` so nothing reaches the wire.
                let sparte = wim_sparte(data.pruefidentifikator.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {} is not a WiM Messstellenbetrieb PID",
                        data.pruefidentifikator
                    ))
                })?;
                let suppress_wire = positive
                    && !mako_fristen::aperak_hat_anerkennungsmeldung(sparte == Sparte::Gas);
                // `sender` = grid_operator: the answering party sends the APERAK
                // back to the party that sent the order.
                let mut aperak_payload = serde_json::json!({
                    "sender":   data.grid_operator.as_str(),
                    "pid":      data.pruefidentifikator.as_u32(),
                    "melo":     data.melo_id.as_str(),
                    "positive": positive,
                });
                if suppress_wire {
                    aperak_payload["suppress_wire"] = serde_json::Value::Bool(true);
                }
                if let Some(ref mr) = data.message_ref {
                    aperak_payload["orig_message_ref"] =
                        serde_json::Value::String(mr.as_str().to_owned());
                }
                if let Some(ref r) = reason {
                    aperak_payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox_entry =
                    PendingOutbox::new("APERAK", data.incoming_msb.as_str(), aperak_payload)
                        .caused_by(0);
                Ok(WorkflowOutput::with_outbox(
                    vec![DeviceChangeEvent::AperakDispatched { positive, reason }],
                    vec![outbox_entry],
                ))
            }

            DeviceChangeCommand::DispatchAntwort {
                bestaetigt,
                antwort_code,
                bemerkung,
                abweichender_termin,
            } => {
                let data = match state {
                    DeviceChangeState::ValidationPassed(d) | DeviceChangeState::AperakSent(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed or AperakSent",
                            state.status_str(),
                        ));
                    }
                };
                let request_pid = data.pruefidentifikator.as_u32();

                // The code decides the PID, not the other way round: an EBD
                // publishes each code in exactly one cluster, and a Zustimmung
                // code on an Ablehnung PID is refused here rather than being
                // rendered into an answer nobody can read.
                let ebd = wim_ebd(request_pid).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {request_pid} is not answered by a WiM Entscheidungsbaum"
                    ))
                })?;
                let code = mako_pruefung::codes::lookup(ebd, &antwort_code).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "Antwortcode {antwort_code:?} is not published in {ebd}"
                    ))
                })?;
                if code.ist_zustimmung() != Some(bestaetigt) {
                    return Err(WorkflowError::rejected(format!(
                        "{ebd} publishes {} in the {} cluster",
                        code.code,
                        code.cluster.label()
                    )));
                }
                let antwort_pid = antwort_pid_for(request_pid, bestaetigt).ok_or_else(|| {
                    WorkflowError::rejected(format!("PID {request_pid} has no answer PID"))
                })?;

                // `Z01`, `Z12` and `Z14` each mean „to a different date than
                // you asked for". An answer that asserts one without naming the
                // date is incomplete — `Z12`'s own Anmerkung says the
                // nächstmöglicher Kündigungszeitpunkt must be carried in `DTM`.
                if abweichender_termin.is_none() && matches!(code.code, "Z01" | "Z12" | "Z14") {
                    return Err(WorkflowError::rejected(format!(
                        "{ebd} {} ({}) requires `abweichender_termin` — the answer states a \
                         date change and must name the date",
                        code.code, code.bedeutung
                    )));
                }

                // The answer date: the deviating one where the code declares
                // one, otherwise the date the order asked for.
                let process_date = abweichender_termin
                    .clone()
                    .or_else(|| data.process_date.clone())
                    .unwrap_or_else(|| data.document_date.clone());

                // `SG4 STS+E01`: DE 9013 the Prüfschritt code, DE 1131 the
                // **Codeliste** it comes from — `S_0090`/`G_0052` and friends,
                // not the EBD number. The tree stays in the event for the audit
                // trail; only the Codeliste goes on the wire.
                let codeliste = code.wire_codeliste().ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "{ebd} {} names no Codeliste for DE 1131",
                        code.code
                    ))
                })?;
                // `SG4 STS+7` is Muss on the answer too, and it echoes the
                // Grund the request stated — the AHB marks the same code list
                // on all three Prüfidentifikatoren of every Anwendungsfall.
                // Where the source carried none (the REST channel), `E03`
                // „Wechsel" is the only code all four Use-Cases publish and
                // the only one that asserts nothing extra.
                let grund = data
                    .transaktionsgrund
                    .clone()
                    .unwrap_or_else(|| TRANSAKTIONSGRUND_WECHSEL.to_owned());
                if !transaktionsgruende(request_pid).contains(&grund.as_str()) {
                    return Err(WorkflowError::rejected(format!(
                        "Transaktionsgrund {grund:?} is not published for PID {request_pid} \
                         (expected one of {:?})",
                        transaktionsgruende(request_pid)
                    )));
                }
                let mut payload = serde_json::json!({
                    "pid":               antwort_pid,
                    // We answer, so the roles invert: the party that received
                    // the order is the sender of its answer.
                    "sender":            data.grid_operator.as_str(),
                    "receiver":          data.incoming_msb.as_str(),
                    "melo":              data.melo_id.as_str(),
                    "process_date":      process_date,
                    "transaktionsgrund": grund,
                    "antwort_code":      code.code,
                    "antwort_codeliste": codeliste,
                    "antwort_ebd":       ebd,
                });
                if let Some(ref vn) = data.vorgangsnummer {
                    payload["vorgangsnummer"] = serde_json::Value::String(vn.clone());
                }
                if let Some(ref text) = bemerkung {
                    payload["bemerkung"] = serde_json::Value::String(text.clone());
                }

                let outbox =
                    PendingOutbox::new("UTILMD", data.incoming_msb.as_str(), payload).caused_by(0);

                // A confirmed Anmeldung is *vorläufig*: the Zuordnung follows the
                // MSBN's Gesamtvorgang report, and if none arrives by the 11. WT
                // after the confirmed Zuordnungsbeginn the NB reports the
                // Scheitern itself (Kap. 2.3.2 Nr. 16).
                let deadlines = if bestaetigt && matches!(request_pid, 55_042 | 44_042) {
                    parse_yyyymmdd(&process_date)
                        .map(|d| {
                            vec![PendingDeadline::new(
                                GESAMTVORGANG_AUSBLEIBEN_WINDOW_LABEL,
                                berlin_cutoff(mako_fristen::add_werktage(
                                    d,
                                    GESAMTVORGANG_AUSBLEIBEN_WT,
                                    HolidayCalendar::BdewMaKo,
                                )),
                            )]
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    vec![DeviceChangeEvent::AntwortGesendet {
                        pruefidentifikator: Pruefidentifikator::new(antwort_pid)
                            .map_err(WorkflowError::rejected)?,
                        bestaetigt,
                        antwort_code: code.code.to_owned(),
                        antwort_ebd: ebd.to_owned(),
                        bemerkung,
                        abweichender_termin,
                    }],
                    vec![outbox],
                    deadlines,
                ))
            }

            // ── Mitteilung über Gesamtvorgang (Kap. 2.3.2 Nr. 7/8) ──────────
            DeviceChangeCommand::MeldeGesamtvorgang {
                erfolgreich,
                zuordnungsbeginn,
            } => {
                let DeviceChangeState::AuftragBestaetigt(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "AuftragBestaetigt",
                        state.status_str(),
                    ));
                };
                if !matches!(data.pruefidentifikator.as_u32(), 55_042 | 44_042) {
                    return Err(WorkflowError::rejected(format!(
                        "the Gesamtvorgang belongs to the Beginn Messstellenbetrieb \
                         (55042 Strom / 44042 Gas); this process is {}",
                        data.pruefidentifikator
                    )));
                }

                let mut payload = serde_json::json!({
                    "pid": if erfolgreich {
                        GESAMTVORGANG_ERFOLG_PID
                    } else {
                        GESAMTVORGANG_SCHEITERN_PID
                    },
                    "sender":   data.incoming_msb.as_str(),
                    "receiver": data.grid_operator.as_str(),
                    "melo":     data.melo_id.as_str(),
                });

                if erfolgreich {
                    let Some(ref beginn) = zuordnungsbeginn else {
                        return Err(WorkflowError::rejected(
                            "an erfolgreicher Gesamtvorgang must name the Zuordnungsbeginn \
                             (SG15 DTM+2380) — it is the date the NB assigns from"
                                .to_owned(),
                        ));
                    };
                    // The Realisierungskorridor: „ein Zeitraum vom 9. WT vor bis
                    // zum 9. WT nach dem vom NB bestätigten Zuordnungsbeginn"
                    // (Kap. 2.3.2 Nr. 5/6). A date outside it is one the NB
                    // cannot assign from.
                    if let (Some(datum), Some(bestaetigt)) = (
                        parse_yyyymmdd(beginn),
                        data.bestaetigter_zuordnungsbeginn
                            .as_deref()
                            .and_then(parse_yyyymmdd),
                    ) && !mako_fristen::vorlauf::VorlaufShape::Korridor(
                        mako_fristen::vorlauf::REALISIERUNGSKORRIDOR_WT,
                    )
                    .check(datum, bestaetigt, HolidayCalendar::BdewMaKo)
                    .is_ok()
                    {
                        let korridor = mako_fristen::vorlauf::realisierungskorridor(
                            bestaetigt,
                            HolidayCalendar::BdewMaKo,
                        );
                        return Err(WorkflowError::rejected(format!(
                            "Übernahmezeitpunkt {datum} liegt außerhalb des \
                             Realisierungskorridors {}..={} um den bestätigten \
                             Zuordnungsbeginn {bestaetigt} (WiM Teil 1 Kap. 2.3.2 Nr. 5/6)",
                            korridor.start(),
                            korridor.end(),
                        )));
                    }
                    payload["zuordnungsbeginn"] = serde_json::Value::String(beginn.clone());
                }

                let message_ref = data
                    .message_ref
                    .clone()
                    .unwrap_or_else(|| MessageRef::new(data.melo_id.as_str()));
                Ok(WorkflowOutput::with_outbox(
                    vec![DeviceChangeEvent::GesamtvorgangGemeldet {
                        erfolgreich,
                        zuordnungsbeginn,
                        outbound: true,
                        message_ref,
                    }],
                    vec![
                        PendingOutbox::new("IFTSTA", data.grid_operator.as_str(), payload)
                            .caused_by(0),
                    ],
                ))
            }

            DeviceChangeCommand::ReceiveGesamtvorgang {
                pid,
                zuordnungsbeginn,
                message_ref,
            } => {
                if !matches!(state, DeviceChangeState::AntwortGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.status_str(),
                    ));
                }
                let erfolgreich = match pid.as_u32() {
                    GESAMTVORGANG_ERFOLG_PID => true,
                    GESAMTVORGANG_SCHEITERN_PID => false,
                    other => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {other} is not a Gesamtvorgang report (expected \
                             {GESAMTVORGANG_ERFOLG_PID} erfolgreich or \
                             {GESAMTVORGANG_SCHEITERN_PID} gescheitert)"
                        )));
                    }
                };
                let events = vec![DeviceChangeEvent::GesamtvorgangGemeldet {
                    erfolgreich,
                    zuordnungsbeginn,
                    outbound: false,
                    message_ref,
                }];
                if erfolgreich {
                    // Kap. 2.3.2 Nr. 8 — „Unverzüglich, jedoch spätester ÜT ist
                    // der 1. WT nach dem ÜT von Nr. 7."
                    Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![],
                        vec![PendingDeadline::new(
                            ZUORDNUNG_ANTWORT_WINDOW_LABEL,
                            deadline_at_werktage(
                                OffsetDateTime::now_utc(),
                                1,
                                HolidayCalendar::BdewMaKo,
                            ),
                        )],
                    ))
                } else {
                    Ok(events.into())
                }
            }

            DeviceChangeCommand::DispatchZuordnung { zugeordnet } => {
                let DeviceChangeState::GesamtvorgangGemeldet(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "GesamtvorgangGemeldet",
                        state.status_str(),
                    ));
                };
                let sparte = wim_sparte(data.pruefidentifikator.as_u32()).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "PID {} is not a WiM Messstellenbetrieb PID",
                        data.pruefidentifikator
                    ))
                })?;
                let pid = if zugeordnet {
                    ZUORDNUNG_ERFOLG_PID
                } else {
                    ZUORDNUNG_SCHEITERN_PID
                };
                let mut payload = serde_json::json!({
                    "pid":      pid,
                    "sender":   data.grid_operator.as_str(),
                    "receiver": data.incoming_msb.as_str(),
                    "melo":     data.melo_id.as_str(),
                });
                if zugeordnet {
                    let Some(ref beginn) = data.bestaetigter_zuordnungsbeginn else {
                        return Err(WorkflowError::rejected(
                            "the Zuordnung needs the Zuordnungsbeginn the MSBN reported — \
                             without it there is no date to assign from"
                                .to_owned(),
                        ));
                    };
                    // The NB commits here, so it checks the corridor itself
                    // rather than trusting the MSBN's arithmetic: assigning
                    // from a date outside it puts the registry and the market
                    // on different days.
                    if let (Some(datum), Some(vorlaeufig)) = (
                        parse_yyyymmdd(beginn),
                        data.process_date.as_deref().and_then(parse_yyyymmdd),
                    ) && !mako_fristen::vorlauf::VorlaufShape::Korridor(
                        mako_fristen::vorlauf::REALISIERUNGSKORRIDOR_WT,
                    )
                    .check(datum, vorlaeufig, HolidayCalendar::BdewMaKo)
                    .is_ok()
                    {
                        let korridor = mako_fristen::vorlauf::realisierungskorridor(
                            vorlaeufig,
                            HolidayCalendar::BdewMaKo,
                        );
                        return Err(WorkflowError::rejected(format!(
                            "der gemeldete Übernahmezeitpunkt {datum} liegt außerhalb des \
                             Realisierungskorridors {}..={} — die Zuordnung ist abzulehnen \
                             (IFTSTA {ZUORDNUNG_SCHEITERN_PID})",
                            korridor.start(),
                            korridor.end(),
                        )));
                    }
                    payload["zuordnungsbeginn"] = serde_json::Value::String(beginn.clone());
                }
                let mut outbox = vec![
                    PendingOutbox::new("IFTSTA", data.incoming_msb.as_str(), payload).caused_by(0),
                ];
                // A confirmed Zuordnung is a market-data fact, not just a
                // message: `marktd` derives the per-Messlokation MSB timeline
                // from this event (`derive_msb_zuordnung`), which is why it
                // carries the MeLo, the MSB and the date rather than only the
                // process UUID.
                if zugeordnet {
                    outbox.push(
                        PendingOutbox::new(
                            "ProcessCompleted",
                            "",
                            serde_json::json!({
                                "pid":              pid,
                                "melo_id":          data.melo_id.as_str(),
                                "msb_mp_id":        data.incoming_msb.as_str(),
                                "zuordnungsbeginn": data.bestaetigter_zuordnungsbeginn,
                                // 00:00 in Strom, 06:00 in Gas — the Gastag
                                // boundary. `marktd` keys the timeline on the
                                // date; the hour is what tells a consumer which
                                // instant that date starts at.
                                "zuordnung_stunde": zuordnungs_stunde(sparte),
                                "sparte":           sparte,
                                "outcome":          "zugeordnet",
                            }),
                        )
                        .caused_by(0),
                    );
                }

                Ok(WorkflowOutput::with_outbox(
                    vec![DeviceChangeEvent::ZuordnungEntschieden {
                        pruefidentifikator: Pruefidentifikator::new(pid)
                            .map_err(WorkflowError::rejected)?,
                        zugeordnet,
                        zuordnungsbeginn: data.bestaetigter_zuordnungsbeginn.clone(),
                        outbound: true,
                    }],
                    outbox,
                ))
            }

            DeviceChangeCommand::ReceiveZuordnungsantwort {
                pid,
                zuordnungsbeginn,
            } => {
                if !matches!(state, DeviceChangeState::GesamtvorgangGemeldet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "GesamtvorgangGemeldet",
                        state.status_str(),
                    ));
                }
                let zugeordnet = match pid.as_u32() {
                    ZUORDNUNG_ERFOLG_PID => true,
                    ZUORDNUNG_SCHEITERN_PID | GESAMTVORGANG_AUSGEBLIEBEN_PID => false,
                    other => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {other} is not a Zuordnungsantwort (expected \
                             {ZUORDNUNG_ERFOLG_PID}, {ZUORDNUNG_SCHEITERN_PID} or \
                             {GESAMTVORGANG_AUSGEBLIEBEN_PID})"
                        )));
                    }
                };
                Ok(vec![DeviceChangeEvent::ZuordnungEntschieden {
                    pruefidentifikator: pid,
                    zugeordnet,
                    zuordnungsbeginn,
                    outbound: false,
                }]
                .into())
            }

            DeviceChangeCommand::Complete { device_id } => {
                // Reachable from both directions: `AntwortGesendet` closes an
                // inbound order we answered, `AuftragBestaetigt` an outbound
                // order the counterparty confirmed. `AperakSent` is *not* a
                // completion state — an acknowledged order whose business
                // answer never went out is exactly the failure this split
                // exists to make visible.
                if !matches!(
                    state,
                    DeviceChangeState::AntwortGesendet(_) | DeviceChangeState::AuftragBestaetigt(_)
                ) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet or AuftragBestaetigt",
                        state.status_str(),
                    ));
                }
                Ok(vec![DeviceChangeEvent::Completed { device_id }].into())
            }

            DeviceChangeCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    DeviceChangeState::Completed(_) | DeviceChangeState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![DeviceChangeEvent::DeadlineExpired { deadline_id, label }].into())
            }

            DeviceChangeCommand::ReceiveInformation {
                pid,
                sender,
                receiver,
                message_ref,
                ..
            } => {
                // Informational messages are accepted in any state — the
                // process may already be completed when a late Vollzugsmeldung
                // or a Stilllegungsinformation arrives — and recorded for
                // audit purposes without a transition.
                Ok(vec![DeviceChangeEvent::InformationEmpfangen {
                    pid,
                    sender,
                    receiver,
                    message_ref,
                }]
                .into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Gerätewechsel process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum DeviceChangeRecord {
    /// No `Initiated` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `Initiated` event applied; process fields now available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// GLN of the incoming Messstellenbetreiber.
        incoming_msb: MarktpartnerCode,
        /// GLN of the grid operator.
        grid_operator: MarktpartnerCode,
        /// Physical device identifier (updated on `Completed`).
        device_id: DeviceId,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl DeviceChangeRecord {
    /// Current lifecycle status label, suitable for logging and serialisation.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied to this stream.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }

    /// Domain data for this record if it has been initiated, or `None` if `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<DeviceChangeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pruefidentifikator,
                ..
            } => Some(DeviceChangeRecordData {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pruefidentifikator,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` `DeviceChangeRecord`.
#[derive(Debug, Clone, Copy)]
pub struct DeviceChangeRecordData<'a> {
    /// Messlokation EIC code.
    pub melo_id: &'a MeLo,
    /// GLN of the incoming Messstellenbetreiber.
    pub incoming_msb: &'a MarktpartnerCode,
    /// GLN of the grid operator.
    pub grid_operator: &'a MarktpartnerCode,
    /// Physical device identifier.
    pub device_id: &'a DeviceId,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

impl Default for DeviceChangeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model that tracks status across all WiM Gerätewechsel
/// streams. Feed via [`mako_engine::projection::ProjectionRunner`].
#[derive(Debug, Default)]
pub struct DeviceChangeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, DeviceChangeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for DeviceChangeProjection {
    fn name(&self) -> &'static str {
        "DeviceChangeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);

        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<DeviceChangeEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            DeviceChangeRecord::New { event_count } => *event_count += 1,
            DeviceChangeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            DeviceChangeEvent::AuftragGesendet {
                melo_id,
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = DeviceChangeRecord::Active {
                    status: "AuftragGesendet",
                    melo_id,
                    incoming_msb: sender,
                    grid_operator: receiver,
                    device_id: DeviceId::new(""),
                    pruefidentifikator,
                    event_count: count,
                };
            }
            DeviceChangeEvent::Initiated {
                melo_id,
                incoming_msb,
                grid_operator,
                device_id,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = DeviceChangeRecord::Active {
                    status: "Initiated",
                    melo_id,
                    incoming_msb,
                    grid_operator,
                    device_id,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            DeviceChangeEvent::ValidationPassed { .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            DeviceChangeEvent::AntwortEmpfangen { is_confirmed, .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = if is_confirmed {
                        "AuftragBestaetigt"
                    } else {
                        "Rejected"
                    };
                }
            }
            DeviceChangeEvent::AperakDispatched { positive, .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = if positive { "AperakSent" } else { "Rejected" };
                }
            }
            DeviceChangeEvent::AntwortGesendet { bestaetigt, .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = if bestaetigt {
                        "AntwortGesendet"
                    } else {
                        "Rejected"
                    };
                }
            }
            DeviceChangeEvent::GesamtvorgangGemeldet { erfolgreich, .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = if erfolgreich {
                        "GesamtvorgangGemeldet"
                    } else {
                        "Rejected"
                    };
                }
            }
            DeviceChangeEvent::ZuordnungEntschieden { zugeordnet, .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = if zugeordnet { "Completed" } else { "Rejected" };
                }
            }
            DeviceChangeEvent::Completed { device_id } => {
                if let DeviceChangeRecord::Active {
                    status,
                    device_id: d,
                    ..
                } = record
                {
                    *status = "Completed";
                    *d = device_id;
                }
            }
            DeviceChangeEvent::Rejected { .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            DeviceChangeEvent::DeadlineExpired { .. } => {
                if let DeviceChangeRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            DeviceChangeEvent::InformationEmpfangen { .. } => {
                // Informational — does not change the status label.
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_receive_cmd(pid: u32, validation_passed: bool) -> DeviceChangeCommand {
        DeviceChangeCommand::ReceiveUtilmd {
            transaktionsgrund: Some("E03".to_owned()),
            pid: Pruefidentifikator::new(pid).expect("test pid must be in range"),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            melo_id: MeLo::new("DE0000000001234567890000000000001"),
            device_id: DeviceId::new("ZHR-12345678"),
            document_date: "20250115".to_owned(),
            message_ref: MessageRef::new("MSG-WIM-001"),
            vorgangsnummer: Some("VG-WIM-001".to_owned()),
            process_date: Some("20250201".to_owned()),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["AHB rule violation".to_owned()]
            },
            received_at: time::OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn happy_path_new_to_completed() {
        let state = DeviceChangeState::default();

        let events = WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55042, true))
            .expect("should accept valid PID 55042");
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[0], DeviceChangeEvent::Initiated { pruefidentifikator, .. } if pruefidentifikator.as_u32() == 55042)
        );
        assert!(matches!(
            &events[1],
            DeviceChangeEvent::ValidationPassed { .. }
        ));

        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::ValidationPassed(_)),
            "expected ValidationPassed, got {}",
            state.status_str()
        );

        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect("dispatch APERAK");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::AperakSent(_)),
            "expected AperakSent"
        );

        // The APERAK acknowledges; it does not answer. The business
        // Bestätigung is a UTILMD 55043 carrying `E15` from `E_0201`.
        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: true,
                antwort_code: "E15".to_owned(),
                bemerkung: None,
                abweichender_termin: None,
            },
        )
        .expect("dispatch Antwort");
        let wire = &out.outbox[0];
        assert_eq!(&*wire.message_type, "UTILMD");
        assert_eq!(wire.payload["pid"], 55_043);
        assert_eq!(wire.payload["antwort_code"], "E15");
        assert_eq!(wire.payload["antwort_ebd"], "E_0201");
        assert_eq!(wire.payload["vorgangsnummer"], "VG-WIM-001");
        // The answer inverts the roles of the order it answers.
        assert_eq!(wire.payload["sender"], "9900357000004");
        assert_eq!(wire.payload["receiver"], "4012345000023");

        let events = out.events.clone();
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::AntwortGesendet(_)),
            "expected AntwortGesendet, got {}",
            state.status_str()
        );

        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::Complete {
                device_id: DeviceId::new("ZHR-99999999"),
            },
        )
        .expect("complete");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::Completed(d) if d.device_id == DeviceId::new("ZHR-99999999")),
            "expected Completed with new device_id",
        );
    }

    /// An acknowledged order whose business answer never went out is not a
    /// completed process: only the Antwort discharges the Antwortfrist.
    #[test]
    fn an_aperak_alone_does_not_complete_the_process() {
        let state = DeviceChangeState::default();
        let events =
            WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55_042, true)).expect("valid");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect("aperak");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::Complete {
                device_id: DeviceId::new("ZHR-1"),
            },
        )
        .expect_err("an unanswered order cannot complete");
        assert!(err.to_string().contains("AntwortGesendet"), "{err}");
    }

    /// A code from another Entscheidungsbaum never reaches the wire. `A02` is
    /// the GPKE „Marktlokation existiert nicht"; `E_0201` does not publish it.
    #[test]
    fn a_foreign_antwortcode_is_refused_before_the_wire() {
        let state = answered_state(55_042);
        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: false,
                antwort_code: "A02".to_owned(),
                bemerkung: None,
                abweichender_termin: None,
            },
        )
        .expect_err("A02 is not in E_0201");
        assert!(err.to_string().contains("E_0201"), "{err}");
    }

    /// `Z01`/`Z12` assert a date change, so the answer must name the date.
    #[test]
    fn a_terminaenderung_must_name_its_date() {
        let state = answered_state(55_039);
        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: false,
                antwort_code: "Z12".to_owned(),
                bemerkung: None,
                abweichender_termin: None,
            },
        )
        .expect_err("Z12 without a date");
        assert!(err.to_string().contains("abweichender_termin"), "{err}");

        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: false,
                antwort_code: "Z12".to_owned(),
                bemerkung: Some("Vertragsbindung bis 30.06.".to_owned()),
                abweichender_termin: Some("20260630".to_owned()),
            },
        )
        .expect("Z12 with a date");
        assert_eq!(out.outbox[0].payload["pid"], 55_041);
        assert_eq!(out.outbox[0].payload["process_date"], "20260630");
    }

    // ── Mitteilung über Gesamtvorgang (Kap. 2.3.2 Nr. 7/8) ───────────────

    /// Drive an outbound 55042 to `AuftragBestaetigt`, carrying the
    /// Zuordnungsbeginn the counterparty confirmed.
    fn auftrag_bestaetigt(bestaetigter_termin: Option<&str>) -> DeviceChangeState {
        let state = DeviceChangeState::default();
        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::InitiateDeviceChange {
                pid: Pruefidentifikator::new(55_042).expect("valid"),
                sender: MarktpartnerCode::new("4012345000023"),
                receiver: MarktpartnerCode::new("9900357000004"),
                melo_id: MeLo::new("DE0000000001234567890000000000001"),
                process_date: "20260601".to_owned(),
                message_ref: MessageRef::new("MSG-OUT-1"),
            },
        )
        .expect("initiate");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::ReceiveAntwort {
                pid: Pruefidentifikator::new(55_043).expect("valid"),
                sender: MarktpartnerCode::new("9900357000004"),
                message_ref: MessageRef::new("MSG-ANT-1"),
                reason: None,
                bestaetigter_termin: bestaetigter_termin.map(str::to_owned),
            },
        )
        .expect("antwort");
        out.events
            .iter()
            .fold(state, WimDeviceChangeWorkflow::apply)
    }

    /// The Anmeldebestätigung is vorläufig, so confirming one opens the
    /// 10-Werktage window in which the MSBN owes its Gesamtvorgang report.
    #[test]
    fn a_confirmed_anmeldung_opens_the_gesamtvorgang_window() {
        let state = DeviceChangeState::default();
        let events = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::InitiateDeviceChange {
                pid: Pruefidentifikator::new(55_042).expect("valid"),
                sender: MarktpartnerCode::new("4012345000023"),
                receiver: MarktpartnerCode::new("9900357000004"),
                melo_id: MeLo::new("DE0000000001234567890000000000001"),
                process_date: "20260601".to_owned(),
                message_ref: MessageRef::new("MSG-OUT-1"),
            },
        )
        .expect("initiate");
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::ReceiveAntwort {
                pid: Pruefidentifikator::new(55_043).expect("valid"),
                sender: MarktpartnerCode::new("9900357000004"),
                message_ref: MessageRef::new("MSG-ANT-1"),
                reason: None,
                bestaetigter_termin: None,
            },
        )
        .expect("antwort");
        assert!(
            out.deadlines
                .iter()
                .any(|d| d.label == GESAMTVORGANG_MELDUNG_WINDOW_LABEL),
            "a confirmed 55042 must open the 10-Werktage Gesamtvorgang window"
        );
    }

    /// `Z01` moves the date, and everything downstream measures against the
    /// moved one — the Realisierungskorridor included.
    #[test]
    fn a_terminaenderung_moves_the_confirmed_zuordnungsbeginn() {
        let DeviceChangeState::AuftragBestaetigt(data) = auftrag_bestaetigt(Some("20260701"))
        else {
            panic!("expected AuftragBestaetigt");
        };
        assert_eq!(
            data.bestaetigter_zuordnungsbeginn.as_deref(),
            Some("20260701")
        );
        assert_eq!(data.process_date.as_deref(), Some("20260601"));
    }

    /// The date the MSBN reports becomes the Zuordnungsbeginn, so it may not
    /// fall outside the ±9-Werktage Realisierungskorridor.
    #[test]
    fn the_gesamtvorgang_date_must_lie_in_the_realisierungskorridor() {
        let state = auftrag_bestaetigt(None);
        let inside = mako_fristen::add_werktage(
            time::Date::from_calendar_date(2026, time::Month::June, 1).expect("valid"),
            9,
            HolidayCalendar::BdewMaKo,
        );
        let outside = mako_fristen::add_werktage(inside, 1, HolidayCalendar::BdewMaKo);
        let fmt = |d: time::Date| format!("{:04}{:02}{:02}", d.year(), d.month() as u8, d.day());

        assert!(
            WimDeviceChangeWorkflow::handle(
                &state,
                DeviceChangeCommand::MeldeGesamtvorgang {
                    erfolgreich: true,
                    zuordnungsbeginn: Some(fmt(inside)),
                },
            )
            .is_ok()
        );
        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::MeldeGesamtvorgang {
                erfolgreich: true,
                zuordnungsbeginn: Some(fmt(outside)),
            },
        )
        .expect_err("one Werktag past the corridor");
        assert!(err.to_string().contains("Realisierungskorridor"), "{err}");
    }

    /// An erfolgreicher Gesamtvorgang without a date names no day to assign
    /// from, so it cannot go out.
    #[test]
    fn an_erfolgreicher_gesamtvorgang_must_name_its_date() {
        let err = WimDeviceChangeWorkflow::handle(
            &auftrag_bestaetigt(None),
            DeviceChangeCommand::MeldeGesamtvorgang {
                erfolgreich: true,
                zuordnungsbeginn: None,
            },
        )
        .expect_err("no date");
        assert!(err.to_string().contains("DTM+2380"), "{err}");
    }

    /// „Bei Mitteilung des Scheiterns des Gesamtvorgangs bleibt der MSBA der
    /// einzelnen Messlokation zugeordnet." The process ends without a Zuordnung.
    #[test]
    fn a_gescheiterter_gesamtvorgang_leaves_the_msba_assigned() {
        let state = auftrag_bestaetigt(None);
        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::MeldeGesamtvorgang {
                erfolgreich: false,
                zuordnungsbeginn: None,
            },
        )
        .expect("Scheitern");
        assert_eq!(out.outbox[0].payload["pid"], GESAMTVORGANG_SCHEITERN_PID);
        let state = out
            .events
            .iter()
            .fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::Rejected { reason } if reason.contains("MSBA")),
            "got {}",
            state.status_str()
        );
    }

    /// The NB side: a Gesamtvorgang report opens the 1-Werktag answer window,
    /// and the Zuordnung carries the date the MSBN named.
    #[test]
    fn the_nb_answers_the_gesamtvorgang_and_assigns_from_the_reported_date() {
        // Confirm an inbound 55042 first.
        let state = answered_state(55_042);
        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: true,
                antwort_code: "E15".to_owned(),
                bemerkung: None,
                abweichender_termin: None,
            },
        )
        .expect("Bestätigung");
        assert!(
            out.deadlines
                .iter()
                .any(|d| d.label == GESAMTVORGANG_AUSBLEIBEN_WINDOW_LABEL),
            "confirming an Anmeldung must arm the 11-Werktage Ausbleiben window"
        );
        let state = out
            .events
            .iter()
            .fold(state, WimDeviceChangeWorkflow::apply);

        // Inside the corridor around the vorläufig confirmed 2025-02-01.
        let gemeldet = mako_fristen::add_werktage(
            time::Date::from_calendar_date(2025, time::Month::February, 1).expect("valid"),
            5,
            HolidayCalendar::BdewMaKo,
        );
        let gemeldet_str = format!(
            "{:04}{:02}{:02}",
            gemeldet.year(),
            gemeldet.month() as u8,
            gemeldet.day()
        );
        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::ReceiveGesamtvorgang {
                pid: Pruefidentifikator::new(GESAMTVORGANG_ERFOLG_PID).expect("valid"),
                zuordnungsbeginn: Some(gemeldet_str.clone()),
                message_ref: MessageRef::new("MSG-IFT-1"),
            },
        )
        .expect("report");
        assert!(
            out.deadlines
                .iter()
                .any(|d| d.label == ZUORDNUNG_ANTWORT_WINDOW_LABEL)
        );
        let state = out
            .events
            .iter()
            .fold(state, WimDeviceChangeWorkflow::apply);
        assert!(matches!(
            &state,
            DeviceChangeState::GesamtvorgangGemeldet(_)
        ));

        let out = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchZuordnung { zugeordnet: true },
        )
        .expect("Zuordnung");
        assert_eq!(&*out.outbox[0].message_type, "IFTSTA");
        assert_eq!(out.outbox[0].payload["pid"], ZUORDNUNG_ERFOLG_PID);
        assert_eq!(out.outbox[0].payload["zuordnungsbeginn"], gemeldet_str);
        // `marktd` derives the per-Messlokation MSB timeline from this second
        // entry, so it must carry the MeLo, the MSB and the date.
        let derived = &out.outbox[1];
        assert_eq!(&*derived.message_type, "ProcessCompleted");
        assert_eq!(derived.payload["pid"], ZUORDNUNG_ERFOLG_PID);
        assert_eq!(
            derived.payload["melo_id"],
            "DE0000000001234567890000000000001"
        );
        assert_eq!(derived.payload["msb_mp_id"], "4012345000023");
        assert_eq!(derived.payload["zuordnungsbeginn"], gemeldet_str);
        let state = out
            .events
            .iter()
            .fold(state, WimDeviceChangeWorkflow::apply);
        assert!(matches!(&state, DeviceChangeState::Completed(d)
                if d.bestaetigter_zuordnungsbeginn.as_deref() == Some(gemeldet_str.as_str())));
    }

    /// The NB commits the Zuordnung, so it checks the corridor itself rather
    /// than trusting the MSBN's arithmetic — assigning from a date outside it
    /// puts the registry and the market on different days.
    #[test]
    fn the_nb_refuses_to_assign_from_a_date_outside_the_korridor() {
        let state = answered_state(55_042);
        let state = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAntwort {
                bestaetigt: true,
                antwort_code: "E15".to_owned(),
                bemerkung: None,
                abweichender_termin: None,
            },
        )
        .expect("Bestätigung")
        .events
        .iter()
        .fold(state, WimDeviceChangeWorkflow::apply);

        let state = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::ReceiveGesamtvorgang {
                pid: Pruefidentifikator::new(GESAMTVORGANG_ERFOLG_PID).expect("valid"),
                // A year past the vorläufig confirmed 2025-02-01.
                zuordnungsbeginn: Some("20260210".to_owned()),
                message_ref: MessageRef::new("MSG-IFT-2"),
            },
        )
        .expect("the report is recorded even when its date is wrong")
        .events
        .iter()
        .fold(state, WimDeviceChangeWorkflow::apply);

        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchZuordnung { zugeordnet: true },
        )
        .expect_err("out of corridor");
        assert!(err.to_string().contains("Realisierungskorridor"), "{err}");

        // …and refusing the Zuordnung outright is always available.
        assert!(
            WimDeviceChangeWorkflow::handle(
                &state,
                DeviceChangeCommand::DispatchZuordnung { zugeordnet: false },
            )
            .is_ok()
        );
    }

    /// The numeric order of the IFTSTA PIDs is the reverse of the reading
    /// order: 21009 is the failure and 21010 the success (IFTSTA AHB 2.1 § 6.2).
    #[test]
    fn the_gesamtvorgang_pids_are_not_in_reading_order() {
        assert_eq!(GESAMTVORGANG_SCHEITERN_PID, 21_009);
        assert_eq!(GESAMTVORGANG_ERFOLG_PID, 21_010);
        assert_eq!(ZUORDNUNG_SCHEITERN_PID, 21_011);
        assert_eq!(ZUORDNUNG_ERFOLG_PID, 21_012);
        assert_eq!(GESAMTVORGANG_AUSGEBLIEBEN_PID, 21_013);
        for pid in GESAMTVORGANG_PIDS {
            assert!(
                IFTSTA_PIDS.contains(pid),
                "{pid} must route to this workflow"
            );
        }
    }

    /// Drive a process to the point where it owes an answer.
    fn answered_state(pid: u32) -> DeviceChangeState {
        let state = DeviceChangeState::default();
        let events =
            WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(pid, true)).expect("valid");
        events.iter().fold(state, WimDeviceChangeWorkflow::apply)
    }

    #[test]
    fn wrong_pid_is_rejected() {
        let state = DeviceChangeState::default();
        let err = WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55001, true))
            .expect_err("should reject wrong PID");
        let msg = err.to_string();
        assert!(
            msg.contains("55001"),
            "error should mention the supplied PID: {msg}"
        );
    }

    #[test]
    fn validation_failure_rejects_process() {
        let state = DeviceChangeState::default();
        let events = WimDeviceChangeWorkflow::handle(&state, make_receive_cmd(55042, false))
            .expect("should still produce events");
        assert!(matches!(&events[1], DeviceChangeEvent::Rejected { .. }));
        let state = events.iter().fold(state, WimDeviceChangeWorkflow::apply);
        assert!(
            matches!(&state, DeviceChangeState::Rejected { .. }),
            "expected Rejected"
        );
    }

    #[test]
    fn dispatch_aperak_in_wrong_state_is_rejected() {
        // Status is New (not ValidationPassed)
        let state = DeviceChangeState::default();
        let err = WimDeviceChangeWorkflow::handle(
            &state,
            DeviceChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .expect_err("should reject dispatch in wrong state");
        assert!(err.to_string().contains("ValidationPassed"), "{err}");
    }
}
