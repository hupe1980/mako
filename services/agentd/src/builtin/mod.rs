//! Which CloudEvent types reach which specialist.
//!
//! This module is the **subscription table**, and nothing more. Each specialist's
//! procedure, model pair, tool grants, ceilings and result schema live in its
//! manifest at `agents/<name>.yaml`, run by `agentplane`. Routing stays in Rust
//! because agentplane has no notion of an event bus — it runs a capability, and
//! deciding which capability a `de.*` event calls for is mako's job.
//!
//! ## Why the prompt is not here
//!
//! A manifest is digest-covered: editing a procedure changes the digest, which
//! is a version bump a reviewer sees in a diff. A Rust constant carries no such
//! record, and keeping both would let them disagree about what the agent is —
//! with the manifest being the copy the model actually reads.
//!
//! ## Activation in agentd.toml
//!
//! ```toml
//! [bundled_agents]
//! enable_all = true                     # every specialist in this build
//! # enable = ["mako-agent", "eeg-agent"]  # or name them
//! ```
//!
//! There are no per-agent overrides. Changing a specialist's model is an edit to
//! its manifest, which is the reviewable path by design.

/// A compiled-in specialist's subscription.
///
/// This is deliberately **not** the agent's definition. The procedure, the model
/// pair, the tool grants and the ceilings live in `agents/<name>.yaml`, where
/// they are covered by the manifest digest a reviewer approves. What stays in
/// Rust is the one thing agentplane has no notion of: which CloudEvent types
/// reach this specialist.
///
/// Keeping a second copy of the prompt here would let the two disagree about
/// what the agent is, with the manifest the one the model actually reads.
#[derive(Debug, Clone)]
pub struct Specialist {
    /// Unique identifier, matching the manifest's `metadata.name` and the
    /// name used in `[bundled_agents] enable`.
    pub name: &'static str,
    /// One-line description for the catalogue endpoint.
    pub specialty: &'static str,
    /// CloudEvent type glob patterns that route an event to this specialist.
    pub trigger_patterns: &'static [&'static str],
}

// ── Catalogue ─────────────────────────────────────────────────────────────────

/// All built-in specialist agents.
///
/// Returned by [`all()`] and looked up by [`find(name)`].
/// Every built-in specialist compiled into this binary.
///
/// # Role scoping (§ 9 EnWG informatorisches Unbundling)
///
/// Entries are gated by the same `role-*` features every other daemon uses. An
/// `role-lf` build contains no NB or MSB specialist at all — not a specialist
/// that policy declines to run, but code that is not in the binary.
///
/// This matters because `agentd` is the one service that reaches all the others.
/// In a combined-role deployment, Cedar can refuse an NB principal access to LF
/// process state, but that is a runtime control over a process holding both sets
/// of credentials. Structural separation comes first; Cedar is defence in depth.
///
/// A specialist with no gate is cross-cutting — the protocol, deadline,
/// compliance and process surfaces exist in every deployment.
static BUILTIN_AGENTS: &[Specialist] = &[
    MAKO_AGENT,
    DEADLINE_ALERT_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    BILLING_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    NETZBILANZ_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    INVOICE_RECONCILIATION_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    BILLING_ANOMALY_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    BILLING_REGULATORY_GUARD_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    JAHRESABRECHNUNG_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    EEG_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    EEG_COMPLIANCE_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    PAYMENT_RECONCILIATION_AGENT,
    COMPLIANCE_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-msb",
    ))]
    MSB_HISTORY_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-msb",
    ))]
    METER_DATA_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    GRID_ANOMALY_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    TARIFF_OPTIMIZATION_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    VERTRAGD_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    TARIFBD_AGENT,
    PROCESSD_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    SPERRD_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    PORTALD_AGENT,
    REGULATORY_REPORTING_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    REPLACEMENT_VALUE_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    MABIS_SYNCD_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-msb",
    ))]
    SMGW_DIAGNOSTICS_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-lf",
    ))]
    VPP_BILLING_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    GABI_GAS_AGENT,
    #[cfg(any(
        not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
        feature = "role-nb",
    ))]
    EINSD_BATCH_AGENT,
];

const MAKO_AGENT: Specialist = Specialist {
    name: "mako-agent",
    specialty: "GPKE/WiM/GeLi Gas process lifecycle expert. Diagnoses Anmeldung rejections, APERAK errors, stuck processes, and format-version mismatches. Queries makod MCP and obsd KPI reports.",
    trigger_patterns: &[
        mako_events::mako::PROCESS_FAILED,
        mako_events::mako::APERAK_TIMEOUT,
        "de.mako.aperak.*",
    ],
};

const DEADLINE_ALERT_AGENT: Specialist = Specialist {
    name: "deadline-alert-agent",
    specialty: "APERAK deadline monitor. Detects processes approaching BDEW MaKo response windows (45 min Strom UTILMD, 3 WT Gas APERAK). Triggers operator alerts when SLAs are at risk.",
    trigger_patterns: &[
        mako_events::mako::PROCESS_FAILED,
        mako_events::mako::APERAK_TIMEOUT,
        mako_events::obs::DEADLINE_APPROACHING,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const BILLING_AGENT: Specialist = Specialist {
    name: "billing-agent",
    specialty: "INVOIC dispute resolution and O2C lifecycle. Handles receipt.disputed events, checks REMADV positions against Rechnung, and coordinates with netzbilanzd for settlement corrections.",
    trigger_patterns: &[
        mako_events::invoic::RECEIPT_DISPUTED,
        mako_events::accounting::MAHNUNG_ISSUED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const NETZBILANZ_AGENT: Specialist = Specialist {
    name: "netzbilanz-agent",
    specialty: "NNE/KA/MMM billing draft lifecycle (NB role). Monitors invoice_drafts, detects overdue dispatch alerts, verifies CalculationTrace completeness, and handles REMADV payment confirmations.",
    trigger_patterns: &[
        mako_events::netzbilanz::INVOIC_DRAFTED,
        mako_events::netzbilanz::INVOIC_DISPATCHED,
        mako_events::netzbilanz::INVOIC_DISPATCH_OVERDUE,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const INVOICE_RECONCILIATION_AGENT: Specialist = Specialist {
    name: "invoice-reconciliation-agent",
    specialty: "REMADV payment reconciliation and overdue invoice resolution. Matches incoming REMADV 33001/33002 against open invoices, identifies partial payments, and triggers dunning when appropriate.",
    trigger_patterns: &[mako_events::invoic::PAYMENT_OVERDUE, "de.invoic.receipt.*"],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const BILLING_ANOMALY_AGENT: Specialist = Specialist {
    name: "billing-anomaly-agent",
    specialty: "Retail invoice anomaly detection. Rolling 3-month statistical deviation analysis (threshold: 20%). Investigates root causes: meter exchange, tariff change, quality substitution, or data error.",
    trigger_patterns: &[mako_events::billing::RECHNUNG_ERSTELLT],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const BILLING_REGULATORY_GUARD_AGENT: Specialist = Specialist {
    name: "billing-regulatory-guard-agent",
    specialty: "Post-billing §40/§41/§41a Abs. 1/§42 EnWG compliance guard. Validates every dispatched invoice for mandatory fields, §41a Abs. 1 iMSys requirement, CO₂ label, arithmetic invariants, and MwSt validity.",
    trigger_patterns: &[mako_events::billing::RECHNUNG_ERSTELLT],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const JAHRESABRECHNUNG_AGENT: Specialist = Specialist {
    name: "jahresabrechnung-agent",
    specialty: "Annual Schlussabrechnung orchestrator (LF role). Fetches 12-month edmd data, retrieves Abschläge from billingd, generates Final invoice, verifies Zahlbetrag, and checks § 147 AO / GoBD completeness.",
    trigger_patterns: &[],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const EEG_AGENT: Specialist = Specialist {
    name: "eeg-agent",
    specialty: "EEG/KWKG plant lifecycle and settlement. Handles Förderung expiry, post-EEG transition planning, settlement history queries, and iMSys rollout detection from edmd direct pushes.",
    trigger_patterns: &[
        mako_events::eeg::ANLAGE_FOERDERUNG_AUSLAUFEND,
        mako_events::messwert::READING_DIRECT_STORED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const EEG_COMPLIANCE_AGENT: Specialist = Specialist {
    name: "eeg-compliance-agent",
    specialty: "EEG/KWKG regulatory compliance monitor. Checks §52 Pflichtzahlungen, §44b Biogas 45% cap, §20 Direktvermarktungspflicht (>100 kW), §42a Holzbiomasse, and §43 Substratdeckel violations.",
    trigger_patterns: &[
        "de.eeg.anlage.*",
        "de.eeg.verguetung.*",
        "de.eeg.marktpraemie.*",
        "de.eeg.compliance.*",
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const PAYMENT_RECONCILIATION_AGENT: Specialist = Specialist {
    name: "payment-reconciliation-agent",
    specialty: "SEPA payment reconciliation and Mahnwesen escalation. Matches CAMT.054 bank returns against accountingd ledger, triggers Mahnstufe escalation (1→2→3), and generates Sperrauftrag when needed.",
    trigger_patterns: &[
        mako_events::accounting::PAYMENT_DUE,
        mako_events::accounting::BANKRUECKLAST,
    ],
};

const COMPLIANCE_AGENT: Specialist = Specialist {
    name: "compliance-agent",
    specialty: "§20 EnWG Diskriminierungsfreiheit parity monitor and BNetzA KPI reporting. Tracks STP rates across market roles, generates BNetzA Diskriminierungsbericht, and flags parity violations.",
    trigger_patterns: &[mako_events::obs::STP_PARITY_ALERT],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-msb",
))]
const MSB_HISTORY_AGENT: Specialist = Specialist {
    name: "msb-history-agent",
    specialty: "WiM Strom MSB-change history and INSRPT reading-order lifecycle. Detects stuck reading orders, quality issues after MSB transitions, and iMSys rollout progress per §31 MsbG.",
    trigger_patterns: &[
        mako_events::messwert::READING_QUALITY_WARNING,
        mako_events::messwert::READING_DIRECT_STORED,
        mako_events::mako::PROCESS_COMPLETED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-msb",
))]
const METER_DATA_AGENT: Specialist = Specialist {
    name: "meter-data-agent",
    specialty: "MSCONS meter data quality and § 60 Abs. 2 MsbG substitute value analysis. Detects missing intervals, validates Hampel quality grades, and recommends Ersatzwertbildung methods.",
    trigger_patterns: &[
        mako_events::messwert::READING_QUALITY_WARNING,
        mako_events::mako::PROCESS_COMPLETED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const GRID_ANOMALY_AGENT: Specialist = Specialist {
    name: "grid-anomaly-agent",
    specialty: "NB grid-assignment and contract gap detection. Flags MaLos whose marktd malo_grid record is missing or lacks a valid NB contract (Vertrag), which would block NB STP auto-decisions (processd check 5).",
    trigger_patterns: &[
        mako_events::markt::NB_CONTRACT_UPDATED,
        mako_events::markt::MALO_UPDATED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const TARIFF_OPTIMIZATION_AGENT: Specialist = Specialist {
    name: "tariff-optimization-agent",
    specialty: "§41a dynamic tariff upgrade advisor. Identifies iMSys customers not yet on dynamic tariffs, estimates annual savings vs fixed tariff, and recommends product changes in tarifbd.",
    trigger_patterns: &[
        mako_events::billing::RECHNUNG_ERSTELLT,
        mako_events::mako::PROCESS_COMPLETED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const VERTRAGD_AGENT: Specialist = Specialist {
    name: "vertragd-agent",
    specialty: "Contract & customer lifecycle specialist. Preisgarantie Tarifwechsel guards, § 41 Abs. 5 EnWG price-change notices, expiring contract alerts, stuck MaKo workflows (§20 EnWG parity), and B2B Rahmenvertrag.",
    trigger_patterns: &[
        "de.vertrag.*",
        mako_events::mako::APERAK_REJECTED,
        mako_events::mako::PROCESS_FAILED,
        mako_events::vertrag::ABLAUF_ANKUENDIGUNG,
        mako_events::vertrag::PREISAENDERUNG_ANKUENDIGUNG,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const TARIFBD_AGENT: Specialist = Specialist {
    name: "tarifbd-agent",
    specialty: "Product catalog hygiene, §41a EPEX price availability monitor, and §42 Energiemix \
completeness guard. Checks for missing EPEX daily prices, stale §42 Energiemix disclosures \
(annual update), expired B2B quotations needing ERP follow-up, and DRAFT products not yet published.",
    trigger_patterns: &[
        mako_events::tarif::PRODUCT_UPDATED,
        mako_events::tarif::ANGEBOT_ABGELAUFEN,
        mako_events::tarif::EPEX_MISSING,
    ],
};

const PROCESSD_AGENT: Specialist = Specialist {
    name: "processd-agent",
    specialty: "NB STP decision trace and LF E_0624 auto-response monitor. Explains why processd rejected an Anmeldung (which of 6 netz-checker checks failed), and tracks approval_queue items.",
    trigger_patterns: &[
        mako_events::mako::PROCESS_INITIATED,
        mako_events::mako::APERAK_REJECTED,
        mako_events::mako::PROCESS_FAILED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const SPERRD_AGENT: Specialist = Specialist {
    name: "sperrd-agent",
    specialty: "Sperrung execution SLA monitor. Tracks sperr_orders lifecycle (pending→executed/failed/cancelled), flags overdue orders that risk BK6-22-024 compliance, and diagnoses IFTSTA 21039 dispatch failures.",
    trigger_patterns: &[
        mako_events::accounting::SPERRAUFTRAG,
        "de.sperr.*",
        mako_events::mako::PROCESS_COMPLETED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const PORTALD_AGENT: Specialist = Specialist {
    name: "portald-agent",
    specialty: "Customer portal proactive notification orchestrator. Triggers invoice-ready notifications, EEG Förderung expiry alerts, Mahnung reminders, and Tarifwechsel confirmation messages.",
    trigger_patterns: &[
        mako_events::billing::RECHNUNG_ERSTELLT,
        mako_events::eeg::ANLAGE_FOERDERUNG_AUSLAUFEND,
        mako_events::accounting::MAHNUNG_ISSUED,
        "de.vertrag.*",
    ],
};

const REGULATORY_REPORTING_AGENT: Specialist = Specialist {
    name: "regulatory-reporting-agent",
    specialty: "BNetzA §20 EnWG annual Diskriminierungsbericht generator and quarterly KPI reporter. Aggregates process STP rates, APERAK response times, and parity metrics for regulatory submissions.",
    trigger_patterns: &[],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const REPLACEMENT_VALUE_AGENT: Specialist = Specialist {
    name: "replacement-value-agent",
    specialty: "§ 60 Abs. 2 MsbG Ersatzwertbildung orchestrator. Selects and applies the correct substitute-value method (linear/prior-period/profile) for missing meter intervals, with full audit trail.",
    trigger_patterns: &[
        mako_events::messwert::READING_QUALITY_WARNING,
        mako_events::mako::PROCESS_COMPLETED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const MABIS_SYNCD_AGENT: Specialist = Specialist {
    name: "mabis-syncd-agent",
    specialty: "MaBiS Summenzeitreihe submission monitor. Triages failed submissions and open Korrekturbedarf against the Werktag-based Erstaufschlag/Clearing windows (BK6-24-174 Anlage 3 §3.10), and checks whether a quality warning threatens Summenzeitreihe accuracy.",
    trigger_patterns: &[
        mako_events::mabis::SUBMISSION_FAILED,
        mako_events::mabis::KORREKTURBEDARF_OPENED,
        mako_events::messwert::READING_QUALITY_WARNING,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-msb",
))]
const SMGW_DIAGNOSTICS_AGENT: Specialist = Specialist {
    name: "smgw-diagnostics-agent",
    specialty: "BSI TR-03109 Smart Meter Gateway lifecycle diagnostics. Detects certificate expiry (TLS/SIG/ENC/KEY_AGREEMENT), CLS channel §14a compliance gaps, communication faults, and stalled iMSys rollout. Uses edmd SMGW registry API.",
    trigger_patterns: &[
        mako_events::messwert::CLS_COMPLIANCE_ISSUE,
        mako_events::messwert::SMGW_CERT_EXPIRY_WARNING,
        mako_events::messwert::READING_QUALITY_WARNING,
        mako_events::messwert::READING_DIRECT_STORED,
        mako_events::mako::PROCESS_INITIATED,
        mako_events::markt::GERAET_KONFIGURATION_UPDATED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-lf",
))]
const VPP_BILLING_AGENT: Specialist = Specialist {
    name: "vpp-billing-agent",
    specialty: "VPP settlement anomaly monitor. Verifies that every `de.vpp.dispatch.confirmed` \
                event produced a matching `de.vpp.settlement.berechnet` within 5 minutes, \
                alerts on missing settlements, and performs Art. 17 RL (EU) 2019/944 audit checks.",
    trigger_patterns: &[
        mako_events::vpp::DISPATCH_CONFIRMED,
        mako_events::vpp::SETTLEMENT_BERECHNET,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const GABI_GAS_AGENT: Specialist = Specialist {
    name: "gabi-gas-agent",
    specialty: "GaBi Gas 2.1 (BK7-24-01-008) balancing and allocation monitor. Tracks ALOCAT/IMBNOT \
                gas imbalance saldos, monitors daily nomination/allocation cycle completeness, \
                flags Mehr-/Mindermengensaldo deviations, and diagnoses MSCONS 13013 dispatch \
                failures (Allokationsliste Gas, MMMA).",
    trigger_patterns: &[
        "de.gabi.imbalance.*",
        mako_events::gabi::ALOCAT_MISSING,
        "de.gabi.nomination.*",
        mako_events::netzbilanz::INVOIC_DRAFTED,
    ],
};

#[cfg(any(
    not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")),
    feature = "role-nb",
))]
const EINSD_BATCH_AGENT: Specialist = Specialist {
    name: "einsd-batch-agent",
    specialty: "EEG/KWKG monthly auto-settlement orchestrator and §52 violation sweep. Triggers \
                batch settlement for all active plants, detects §52 Pflichtzahlungen accrual, \
                checks §44b biogas cap utilisation, and ensures post-EEG plants are on correct \
                remuneration scheme.",
    trigger_patterns: &[
        mako_events::eeg::SETTLEMENT_BATCH_DUE,
        "de.eeg.compliance.*",
        mako_events::eeg::ANLAGE_FOERDERUNG_AUSLAUFEND,
    ],
};

/// Every specialist compiled into this binary, in declaration order.
///
/// A role-scoped build yields only its own role's specialists (§ 9 EnWG).
pub fn all() -> impl Iterator<Item = &'static Specialist> {
    BUILTIN_AGENTS.iter()
}

/// Look up a specialist by name.
#[must_use]
pub fn find(name: &str) -> Option<&'static Specialist> {
    BUILTIN_AGENTS.iter().find(|d| d.name == name)
}

#[cfg(test)]
mod trigger_contract_tests {
    use super::{Specialist, all};

    /// Every subscription has a manifest to run.
    ///
    /// The two halves of a specialist now live in different files. This is the
    /// seam: a name in the subscription table with no `agents/<name>.yaml` is an
    /// event type that routes to nothing.
    #[test]
    fn every_subscription_has_a_manifest() {
        let missing: Vec<&str> = all()
            .map(|d| d.name)
            .filter(|n| crate::plane::find_manifest(n).is_none())
            .collect();
        assert!(
            missing.is_empty(),
            "these specialists subscribe to events but have no manifest: {missing:?}"
        );
    }

    /// § 9 EnWG: a role-scoped build contains no other arm's specialists.
    ///
    /// `agentd` is the one service that reaches all the others, so in a
    /// combined-role deployment it is the component that cannot be split by
    /// policy alone. This asserts the structural half: a `role-lf` binary does
    /// not *contain* the NB specialists, so no misconfiguration can enable them.
    #[test]
    fn role_scoped_builds_exclude_the_other_arms_specialists() {
        let names: Vec<&str> = all().map(|a| a.name).collect();
        let has = |n: &str| names.contains(&n);

        // Cross-cutting specialists are in every build — the protocol, deadline
        // and compliance surfaces exist for every Marktrolle.
        assert!(
            has("mako-agent"),
            "the protocol specialist is cross-cutting"
        );
        assert!(
            has("deadline-alert-agent"),
            "deadline monitoring is cross-cutting"
        );

        #[cfg(all(feature = "role-lf", not(feature = "role-nb")))]
        {
            assert!(
                has("billing-agent"),
                "an LF build keeps its billing specialist"
            );
            assert!(
                !has("netzbilanz-agent"),
                "an LF build must not contain the NB grid-billing specialist"
            );
            assert!(
                !has("sperrd-agent"),
                "an LF build must not contain the NB Sperrung specialist"
            );
            // The wiring an LF deployment must provide follows the *compiled*
            // specialists, not the embedded manifest set — `manifests![]` is
            // not role-gated, so without the filter in `servers_named_in_grants`
            // an LF deployment could not boot without an MCP endpoint for
            // `sperrd`, a server only the NB Sperrung specialist grants.
            assert!(
                !crate::plane::tools::servers_named_in_grants().contains("sperrd"),
                "an LF deployment must not be required to wire the NB-only sperrd endpoint"
            );
        }

        #[cfg(all(feature = "role-nb", not(feature = "role-lf")))]
        {
            assert!(has("netzbilanz-agent"), "an NB build keeps grid billing");
            assert!(
                !has("billing-agent"),
                "an NB build must not contain the LF retail-billing specialist"
            );
            assert!(
                !has("vertragd-agent"),
                "an NB build must not contain the LF contract specialist"
            );
        }

        #[cfg(not(any(feature = "role-lf", feature = "role-nb", feature = "role-msb")))]
        assert_eq!(
            names.len(),
            28,
            "the default build carries every specialist; update this count \
             deliberately when one is added or removed"
        );
    }

    /// Specialists with no event subscription, on purpose.
    ///
    /// An empty trigger array means the specialist can only be started by hand
    /// (`POST /api/v1/run`) or by an external scheduler — a batch shape, not a
    /// reactive one. That is a legitimate design for a monthly report, and a
    /// silent bug for anything else: an unsubscribed specialist looks exactly
    /// like one that ran and found nothing.
    const MANUAL_ONLY: &[(&str, &str)] = &[
        (
            "jahresabrechnung-agent",
            "annual Schlussabrechnung is a yearly batch an operator or scheduler starts; \
             no CloudEvent marks 'twelve months have passed for this MaLo'",
        ),
        (
            "regulatory-reporting-agent",
            "the BNetzA Diskriminierungsbericht is an annual/quarterly report started \
             by an operator; obsd emits no reporting-period event",
        ),
    ];

    /// A specialist is subscribed, or its batch shape is declared here.
    ///
    /// A specialist cannot go silently unsubscribed: an empty trigger array is
    /// refused unless the specialist is on [`MANUAL_ONLY`] with the reason —
    /// and a listed specialist that *gains* a subscription must leave the list,
    /// so the allowlist cannot rot into covering reactive agents.
    #[test]
    fn a_specialist_is_subscribed_or_declared_manual_only() {
        for def in all() {
            let listed = MANUAL_ONLY.iter().any(|(n, _)| *n == def.name);
            if def.trigger_patterns.is_empty() {
                assert!(
                    listed,
                    "{}: subscribes to nothing and is not declared manual-only — it can \
                     never be triggered by an event, which reads as an agent that ran and \
                     found nothing. Add trigger patterns, or add it to MANUAL_ONLY with \
                     the reason it is a batch job.",
                    def.name
                );
            } else {
                assert!(
                    !listed,
                    "{}: is declared manual-only but subscribes to events — remove it \
                     from MANUAL_ONLY so the list stays honest",
                    def.name
                );
            }
        }
    }

    /// Trigger patterns that deliberately match nothing in the catalog yet.
    ///
    /// Each entry is a subscription placed ahead of the emitter, so the glob is
    /// live but nothing fires it. Removing an entry here is the last step of
    /// wiring the emitter, not a chore.
    const UNEMITTED_PATTERNS: &[(&str, &str)] = &[
        (
            "de.sperr.*",
            "sperrd does not emit a concrete de.sperr.* type yet; \
             mako-events documents the gap and the ROADMAP tracks it",
        ),
        (
            "de.eeg.compliance.*",
            "einsd does not emit a concrete de.eeg.compliance.* type yet; \
             mako-events documents the gap on the `eeg` module",
        ),
    ];

    /// The concrete event types named under `## TRIGGERED BY` in the
    /// specialist's **manifest** prompt.
    ///
    /// The prompt moved to `agents/<name>.yaml` during the agentplane cutover,
    /// so this reads the manifest rather than a Rust constant — otherwise the
    /// guard would check a copy the model never sees.
    fn prompt_triggers(def: &Specialist) -> Vec<String> {
        let Some(embedded) = crate::plane::find_manifest(def.name) else {
            return Vec::new();
        };
        let manifest = embedded;
        let Some(identity) = manifest.spec.identity.as_ref() else {
            return Vec::new();
        };
        let prompt = agentplane::manifest::Identity::system_prompt(identity);

        let Some(start) = prompt.find("## TRIGGERED BY") else {
            return Vec::new();
        };
        let body = &prompt[start + "## TRIGGERED BY".len()..];
        // The block runs to the next `##` heading.
        let body = body.split("\n##").next().unwrap_or(body);

        let mut out = Vec::new();
        for line in body.lines() {
            if !line.trim_start().starts_with("- ") {
                continue;
            }
            // A line may name more than one type:
            // ``- `de.eeg.verguetung.*` / `de.eeg.marktpraemie.*` — settlement``
            let mut rest = line;
            while let Some(open) = rest.find('`') {
                rest = &rest[open + 1..];
                let Some(close) = rest.find('`') else { break };
                let token = &rest[..close];
                if token.starts_with("de.") {
                    out.push(token.to_owned());
                }
                rest = &rest[close + 1..];
            }
        }
        out
    }

    /// A subscription that matches no catalog event can never fire.
    #[test]
    fn builtin_trigger_patterns_match_a_catalog_event() {
        let catalog = mako_events::all();
        let mut dead = Vec::new();

        for def in all() {
            for pattern in def.trigger_patterns {
                if UNEMITTED_PATTERNS.iter().any(|(p, _)| p == pattern) {
                    continue;
                }
                if !catalog.iter().any(|ev| mako_events::matches(pattern, ev)) {
                    dead.push(format!("{}: {pattern}", def.name));
                }
            }
        }

        assert!(
            dead.is_empty(),
            "these specialists subscribe to event types that exist nowhere in \
             the mako-events catalog, so the agent can never be triggered: \
             {dead:#?}\n\n\
             Either correct the pattern to a real type, or — if the emitter is \
             genuinely not built yet — add it to UNEMITTED_PATTERNS with the \
             reason."
        );
    }

    /// The prompt tells the model what wakes it; the subscription decides.
    ///
    /// When they disagree the model reasons about a trigger it never receives
    /// (or stays silent about one it does), and nothing fails loudly.
    #[test]
    fn builtin_prompt_triggers_match_subscriptions() {
        let mut drift = Vec::new();

        for def in all() {
            let promised = prompt_triggers(def);
            if promised.is_empty() {
                continue;
            }
            let subscribed = def.trigger_patterns;

            for ev in &promised {
                // A prompt entry may be a glob itself, so accept an exact
                // pattern match as well as a glob that covers it.
                let covered = subscribed
                    .iter()
                    .any(|p| *p == ev.as_str() || mako_events::matches(p, ev));
                if !covered {
                    drift.push(format!(
                        "{}: prompt promises `{ev}` but no trigger pattern covers it",
                        def.name
                    ));
                }
            }

            for p in subscribed {
                // A pattern nothing emits yet need not be promised to the
                // model — it would describe a wake-up that cannot happen.
                if UNEMITTED_PATTERNS.iter().any(|(u, _)| u == p) {
                    continue;
                }
                let mentioned = promised
                    .iter()
                    .any(|ev| ev == p || mako_events::matches(p, ev));
                if !mentioned {
                    drift.push(format!(
                        "{}: subscribes to `{p}` but the prompt never mentions it",
                        def.name
                    ));
                }
            }
        }

        assert!(
            drift.is_empty(),
            "the `## TRIGGERED BY` prompt block and `trigger_patterns` \
             disagree for these specialists: {drift:#?}"
        );
    }
}
