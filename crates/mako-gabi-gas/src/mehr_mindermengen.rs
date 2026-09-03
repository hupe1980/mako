//! GaBi Gas Mehr-/Mindermengenmeldung — SSQNOT (NB → MGV).
//!
//! The Netzbetreiber reports, per Netzkonto and Abrechnungszeitraum, the
//! Mehrmenge and the Mindermenge that the Mehr-/Mindermengenabrechnung settles
//! against the Marktgebietsverantwortlichen. SSQNOT 5.7 publishes two
//! Anwendungsfälle, both `NB an MGV`, and no answer message.
//!
//! ```text
//! NB ──(SSQNOT 70095 SLP / 70096 RLM)──→ MGV
//! ```
//!
//! The workflow hosts both ends: the MGV records what a Netzbetreiber reports
//! ([`MehrMindermengenCommand::ReceiveSsqnot`]), and a Netzbetreiber tenant
//! reports its own figures ([`MehrMindermengenCommand::Melden`]), which
//! enqueues the `SSQNOT` the outbox renders.
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ Recorded ──(a later report for the same Netzkonto and Zeitraum)──→ Recorded
//! ```
//!
//! SSQNOT carries no version or correction code, so a later report for the
//! same Netzkonto and Zeitraum simply stands: the state holds the latest, the
//! event stream keeps every one. No response is due and no Frist binds either
//! side, so the process registers no deadline.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK7-24-01-008** — GaBi Gas 2.1, Mehr-/Mindermengenabrechnung
//! - **Kooperationsvereinbarung Gas (KoV)** — Netzkontoführung durch den MGV
//! - **DVGW SSQNOT 5.7** — message format; §4 Hinweise \[500\]/\[501\] retire the
//!   RLM Anwendungsfall for Zeiträume from 1.10.2015

use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use rust_decimal::Decimal;
use time::Date;

// ── Prüfidentifikator set ─────────────────────────────────────────────────────

/// Every DVGW Prüfidentifikator that routes to this workflow.
///
/// SSQNOT 5.7 §4: 70095 Mehr-/Mindermengenmeldung SLP, 70096 RLM. A test in
/// this module pins the list to `dvgw_edi::catalogue_for`.
pub const MEHR_MINDERMENGEN_PIDS: &[u32] = &[70095, 70096];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-mehr-mindermengen";

// ── Domain data ───────────────────────────────────────────────────────────────

/// How the Netzbetreiber determined the quantities — `STS` `A1G` / `A2G`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MmmVerfahren {
    /// Standardlastprofil.
    Slp,
    /// Registrierende Leistungsmessung — Zeiträume before 1.10.2015 only.
    Rlm,
}

impl From<dvgw_edi::ssqnot::Verfahren> for MmmVerfahren {
    fn from(v: dvgw_edi::ssqnot::Verfahren) -> Self {
        match v {
            dvgw_edi::ssqnot::Verfahren::Slp => Self::Slp,
            dvgw_edi::ssqnot::Verfahren::Rlm => Self::Rlm,
        }
    }
}

/// One Mehr-/Mindermengenmeldung as recorded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MehrMindermengenData {
    /// The Prüfidentifikator (70095 SLP / 70096 RLM).
    pub pruefidentifikator: u32,
    /// `NAD+MS` — the reporting Netzbetreiber.
    pub netzbetreiber: String,
    /// `NAD+MR` — the Marktgebietsverantwortliche.
    pub marktgebietsverantwortlicher: String,
    /// `SG39 NAD+ZSH` — the Netzkonto the quantities are booked against.
    pub netzkonto: String,
    /// First gas day of the Abrechnungszeitraum (`DTM+Z01`).
    pub zeitraum_von: Date,
    /// Exclusive end of the Abrechnungszeitraum.
    pub zeitraum_bis: Date,
    /// SLP or RLM.
    pub verfahren: MmmVerfahren,
    /// `QTY+ZY0` in kWh.
    pub mehrmenge_kwh: Decimal,
    /// `QTY+ZY2` in kWh.
    pub mindermenge_kwh: Decimal,
    /// `UNH` DE 0062 of the SSQNOT.
    pub message_ref: MessageRef,
}

impl MehrMindermengenData {
    /// Mehrmenge minus Mindermenge, in kWh.
    #[must_use]
    pub fn saldo_kwh(&self) -> Decimal {
        self.mehrmenge_kwh - self.mindermenge_kwh
    }

    /// The outbox payload `makod` renders as the SSQNOT (its `dvgw` renderer):
    /// one `LIN` position per Menge, each under `LOC+Z99` with the
    /// Abrechnungszeitraum as its period, the Verfahren in `STS`, and the
    /// Netzkonto as the position's `NAD+ZSH`.
    #[must_use]
    pub fn ssqnot_payload(&self) -> serde_json::Value {
        let verfahren = match self.verfahren {
            MmmVerfahren::Slp => "A1G",
            MmmVerfahren::Rlm => "A2G",
        };
        let period = serde_json::json!({
            "start": format!("{}T05:00:00Z", self.zeitraum_von),
            "end": format!("{}T05:00:00Z", self.zeitraum_bis),
        });
        let position = |qualifier: &str, kwh: Decimal| {
            serde_json::json!({
                "location": { "qualifier": "Z99" },
                "quantities": [{
                    "qualifier": qualifier,
                    "value": kwh.normalize().to_string(),
                    "period": period,
                    "status": [verfahren],
                }],
                "parties": [{ "role": "ZSH", "code": self.netzkonto }],
            })
        };
        serde_json::json!({
            "pid": self.pruefidentifikator,
            "sender": self.netzbetreiber,
            "receiver": self.marktgebietsverantwortlicher,
            "document_number": format!("SSQNOT{}", self.message_ref.as_str()),
            "message_ref": self.message_ref.as_str(),
            "validity_period": period,
            "positions": [position("ZY0", self.mehrmenge_kwh), position("ZY2", self.mindermenge_kwh)],
        })
    }
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the Mehr-/Mindermengen workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum MehrMindermengenEvent {
    /// A SSQNOT was received and recorded.
    MehrMindermengenGemeldet(MehrMindermengenData),
    /// This tenant reported its Mehr-/Mindermenge; the SSQNOT is in the outbox.
    MehrMindermengenGesendet(MehrMindermengenData),
}

impl EventPayload for MehrMindermengenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::MehrMindermengenGemeldet(_) => "GaBiGasMehrMindermengenGemeldet",
            Self::MehrMindermengenGesendet(_) => "GaBiGasMehrMindermengenGesendet",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Current state of a Mehr-/Mindermengen process stream — one Netzkonto and
/// one Abrechnungszeitraum.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum MehrMindermengenState {
    /// No SSQNOT received yet.
    #[default]
    New,
    /// The latest report on file.
    Recorded(Box<MehrMindermengenData>),
}

impl MehrMindermengenState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Recorded(_) => "Recorded",
        }
    }

    /// The latest report on file, if any.
    #[must_use]
    pub fn latest(&self) -> Option<&MehrMindermengenData> {
        match self {
            Self::New => None,
            Self::Recorded(d) => Some(d),
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the Mehr-/Mindermengen workflow.
#[derive(Debug, Clone)]
pub enum MehrMindermengenCommand {
    /// An inbound SSQNOT — constructed by the DVGW adapter in `makod`.
    ReceiveSsqnot(MehrMindermengenData),
    /// Report this tenant's Mehr-/Mindermenge to the MGV: the SSQNOT goes to
    /// the outbox as [`MehrMindermengenData::ssqnot_payload`].
    Melden(MehrMindermengenData),
}

impl CommandPayload for MehrMindermengenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas Mehr-/Mindermengen workflow: receive-and-record.
pub struct GaBiGasMehrMindermengenWorkflow;

impl Workflow for GaBiGasMehrMindermengenWorkflow {
    type State = MehrMindermengenState;
    type Event = MehrMindermengenEvent;
    type Command = MehrMindermengenCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        let _ = state;
        match event {
            MehrMindermengenEvent::MehrMindermengenGemeldet(data)
            | MehrMindermengenEvent::MehrMindermengenGesendet(data) => {
                MehrMindermengenState::Recorded(Box::new(data.clone()))
            }
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        let (data, outbound) = match command {
            MehrMindermengenCommand::ReceiveSsqnot(data) => (data, false),
            MehrMindermengenCommand::Melden(data) => (data, true),
        };
        if !MEHR_MINDERMENGEN_PIDS.contains(&data.pruefidentifikator) {
            return Err(WorkflowError::rejected(format!(
                "PID {} is not a SSQNOT Prüfidentifikator (70095/70096)",
                data.pruefidentifikator
            )));
        }
        // SSQNOT 5.7 §4 Hinweise [500]/[501]: the RLM Anwendungsfall exists for
        // Zeiträume before 1.10.2015 only. A later one is a message the MGV
        // must not book against the Netzkonto.
        if data.verfahren == MmmVerfahren::Rlm && data.zeitraum_von >= dvgw_edi::SSQNOT_RLM_CUTOFF {
            return Err(WorkflowError::rejected(format!(
                "a RLM Mehr-/Mindermengenmeldung is admitted only for Zeiträume before {} \
                 (SSQNOT 5.7 Hinweis [500]); this one starts {}",
                dvgw_edi::SSQNOT_RLM_CUTOFF,
                data.zeitraum_von
            )));
        }
        if data.zeitraum_bis <= data.zeitraum_von {
            return Err(WorkflowError::rejected(format!(
                "the Abrechnungszeitraum does not run forwards: {}..{}",
                data.zeitraum_von, data.zeitraum_bis
            )));
        }
        // A later report for the same Netzkonto and Zeitraum stands; the format
        // carries no correction marker and the event stream keeps the history.
        if let Some(previous) = state.latest()
            && previous.netzkonto != data.netzkonto
        {
            return Err(WorkflowError::rejected(format!(
                "the process holds Netzkonto {} but the report names {}",
                previous.netzkonto, data.netzkonto
            )));
        }
        if outbound {
            let outbox = PendingOutbox::new(
                "SSQNOT",
                data.marktgebietsverantwortlicher.as_str(),
                data.ssqnot_payload(),
            );
            return Ok(WorkflowOutput {
                events: vec![MehrMindermengenEvent::MehrMindermengenGesendet(data)],
                outbox: vec![outbox],
                deadlines: Vec::new(),
            });
        }
        Ok(vec![MehrMindermengenEvent::MehrMindermengenGemeldet(data)].into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn report(
        pid: u32,
        verfahren: MmmVerfahren,
        von: Date,
        mehr: i64,
        minder: i64,
    ) -> MehrMindermengenData {
        MehrMindermengenData {
            pruefidentifikator: pid,
            netzbetreiber: "9870012345678".into(),
            marktgebietsverantwortlicher: "9800505300009".into(),
            netzkonto: "THE0NKH712345678".into(),
            zeitraum_von: von,
            zeitraum_bis: von.replace_month(von.month().next()).unwrap_or(von),
            verfahren,
            mehrmenge_kwh: Decimal::from(mehr),
            mindermenge_kwh: Decimal::from(minder),
            message_ref: MessageRef::new("1"),
        }
    }

    /// Pinned to the DVGW catalogue: a drifted copy stops routing a published
    /// Anwendungsfall in silence.
    #[test]
    fn the_pid_list_matches_the_dvgw_catalogue() {
        let published: Vec<u32> = dvgw_edi::catalogue_for(dvgw_edi::DvgwMessageType::Ssqnot)
            .map(|info| info.pid)
            .collect();
        assert_eq!(published, MEHR_MINDERMENGEN_PIDS);
        for info in dvgw_edi::catalogue_for(dvgw_edi::DvgwMessageType::Ssqnot) {
            assert_eq!(info.direction, "NB an MGV", "{}", info.pid);
        }
    }

    #[test]
    fn a_report_is_recorded_and_a_later_one_stands() {
        let first = report(70095, MmmVerfahren::Slp, date!(2026 - 03 - 01), 120, 6782);
        let out = GaBiGasMehrMindermengenWorkflow::handle(
            &MehrMindermengenState::New,
            MehrMindermengenCommand::ReceiveSsqnot(first.clone()),
        )
        .expect("recorded");
        let state = out.events.iter().fold(
            MehrMindermengenState::New,
            GaBiGasMehrMindermengenWorkflow::apply,
        );
        assert_eq!(state.latest(), Some(&first));
        assert_eq!(state.latest().unwrap().saldo_kwh(), Decimal::from(-6662));

        let later = report(70095, MmmVerfahren::Slp, date!(2026 - 03 - 01), 130, 6782);
        let out = GaBiGasMehrMindermengenWorkflow::handle(
            &state,
            MehrMindermengenCommand::ReceiveSsqnot(later.clone()),
        )
        .expect("a later report for the same period stands");
        let state = out
            .events
            .iter()
            .fold(state, GaBiGasMehrMindermengenWorkflow::apply);
        assert_eq!(state.latest(), Some(&later));
        assert_eq!(state.label(), "Recorded");
    }

    #[test]
    fn reporting_enqueues_the_ssqnot() {
        let data = report(70095, MmmVerfahren::Slp, date!(2026 - 03 - 01), 120, 6782);
        let out = GaBiGasMehrMindermengenWorkflow::handle(
            &MehrMindermengenState::New,
            MehrMindermengenCommand::Melden(data.clone()),
        )
        .expect("reported");
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "SSQNOT");
        assert_eq!(out.outbox[0].recipient.as_ref(), "9800505300009");
        let payload = &out.outbox[0].payload;
        assert_eq!(payload["pid"], 70095);
        assert_eq!(payload["positions"].as_array().map(Vec::len), Some(2));
        assert_eq!(payload["positions"][1]["quantities"][0]["qualifier"], "ZY2");
        assert_eq!(payload["positions"][1]["quantities"][0]["value"], "6782");
        assert_eq!(
            payload["positions"][0]["parties"][0]["code"],
            "THE0NKH712345678"
        );
        let state = out.events.iter().fold(
            MehrMindermengenState::New,
            GaBiGasMehrMindermengenWorkflow::apply,
        );
        assert_eq!(state.latest(), Some(&data));
    }

    #[test]
    fn a_rlm_report_after_the_cutoff_is_refused() {
        let rlm = report(70096, MmmVerfahren::Rlm, date!(2026 - 03 - 01), 1, 1);
        let err = GaBiGasMehrMindermengenWorkflow::handle(
            &MehrMindermengenState::New,
            MehrMindermengenCommand::ReceiveSsqnot(rlm),
        )
        .expect_err("RLM ended 1.10.2015");
        assert!(err.to_string().contains("2015-10-01"), "{err}");

        let old = report(70096, MmmVerfahren::Rlm, date!(2015 - 03 - 01), 1, 1);
        GaBiGasMehrMindermengenWorkflow::handle(
            &MehrMindermengenState::New,
            MehrMindermengenCommand::ReceiveSsqnot(old),
        )
        .expect("a 2015 RLM report is admitted");
    }

    #[test]
    fn another_netzkonto_is_another_process() {
        let first = report(70095, MmmVerfahren::Slp, date!(2026 - 03 - 01), 1, 1);
        let state = MehrMindermengenState::Recorded(Box::new(first));
        let mut other = report(70095, MmmVerfahren::Slp, date!(2026 - 03 - 01), 1, 1);
        other.netzkonto = "THE0NKH000000000".into();
        assert!(
            GaBiGasMehrMindermengenWorkflow::handle(
                &state,
                MehrMindermengenCommand::ReceiveSsqnot(other)
            )
            .is_err()
        );
    }
}
