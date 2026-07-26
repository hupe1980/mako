//! Compile-time catalog of every CloudEvents `type` used across the mako
//! workspace.
//!
//! One `pub const` per event type, organized in bounded-context modules.
//! Emitters and subscribers reference these constants instead of inline
//! string literals, so a rename is a one-line change and drift between
//! producer and consumer is a compile error rather than a silent mismatch.
//!
//! # Conventions
//!
//! - Every type starts with `de.` and is entirely lowercase
//!   (CloudEvents §3.1 recommends lowercase reverse-DNS types).
//! - Segments are separated by `.`; German domain nouns keep their
//!   established spelling (participles like `beliefert`, hyphenated nouns
//!   like `nb-contract` stay as-is).
//! - Statuses worth grepping for are flagged in doc comments:
//!   - `⚠ phantom:` — subscribed (usually by `agentd`), but no emitter
//!     exists yet; the emitter is tracked in the roadmap.
//!   - `orphan emit:` — emitted, but the workspace audit found no
//!     subscriber.
//!
//! Glob subscription patterns (e.g. `de.mako.*` in `agentd` trigger
//! configs) are not part of this catalog — only concrete types are. The
//! canonical pattern matcher every subscription mechanism uses is
//! [`matches()`](matches()).
//!
//! # Scope (why `mako-events`, not `mako-common`)
//!
//! This crate deliberately stays a single-purpose leaf: constants and logic
//! *about CloudEvents types* (the catalog, the naming convention, the
//! pattern matcher) and nothing else. A general `mako-common`/`mako-core`
//! grab-bag would become a dependency magnet — every crate depends on it,
//! every change rebuilds the workspace, and unrelated helpers accrete
//! without an owner. Shared logic already has purpose-named homes:
//! domain runtime in `mako-engine`, service framework in `mako-service`,
//! master-data types in `mako-markt`. New shared code goes to the crate
//! whose purpose it serves; a new purpose gets a new purpose-named crate.

/// Core MaKo process lifecycle + EDIFACT transport events (`de.mako.*`).
///
/// Emitted by `makod` toward the ERP adapter / event bus; consumed by
/// `obsd`, `processd`, `edmd`, `invoicd`, `marktd`, `vertragd` and `agentd`.
pub mod mako {
    /// A MaKo market process was started (first outbound message built).
    pub const PROCESS_INITIATED: &str = "de.mako.process.initiated";
    /// Happy path finished — the process reached its terminal success state.
    pub const PROCESS_COMPLETED: &str = "de.mako.process.completed";
    /// Unrecoverable failure — the process was cancelled/failed.
    pub const PROCESS_FAILED: &str = "de.mako.process.failed";
    /// APERAK acknowledged the outbound message.
    pub const APERAK_ACCEPTED: &str = "de.mako.aperak.accepted";
    /// APERAK rejected the outbound message (carries BDEW ERC code).
    pub const APERAK_REJECTED: &str = "de.mako.aperak.rejected";
    /// APERAK deadline missed.
    pub const APERAK_TIMEOUT: &str = "de.mako.aperak.timeout";
    /// CONTRL received for an outbound interchange.
    pub const CONTRL_RECEIVED: &str = "de.mako.contrl.received";
    /// MaLo identified during Lieferantenwechsel (GPKE identification step).
    pub const MALO_IDENTIFIED: &str = "de.mako.malo.identified";
    /// Outbound EDIFACT interchange handed to the AS4/AS2 sender.
    pub const EDIFACT_OUTBOUND: &str = "de.mako.edifact.outbound";
    /// Inbound EDIFACT interchange received (edmd ingest allow-list).
    pub const EDIFACT_INBOUND: &str = "de.mako.edifact.inbound";
    // Design note: there are deliberately NO per-process outcome types
    // (`de.mako.gpke.lieferbeginn.bestaetigt` and friends). Process outcomes
    // are the generic `PROCESS_*`/`APERAK_*` family — the process family
    // rides in `data.workflow`, and `vertragd` matches outcome *suffixes*
    // (`.bestaetigt`/`.abgelehnt`/`.completed`/…) so a future fine-grained
    // emitter would be consumed without code changes. Minting per-process
    // constants here without an emitter would institutionalize fiction (an
    // earlier draft carried four such test-fixture types; they were removed).
    /// §20 NZV Netzzugang: aggregated Übermittlungsbedarf toward the NB.
    pub const NETZZUGANG_UEBERMITTLUNGSBEDARF: &str = "de.mako.netzzugang.uebermittlungsbedarf";
}

/// Market master-data events (`de.markt.*`), emitted by `marktd`.
pub mod markt {
    /// Marktlokation stammdaten changed.
    pub const MALO_UPDATED: &str = "de.markt.malo.updated";
    /// A UTILMD Stammdatenänderung (GPKE Teil 4 / GeLi Gas) was applied to a
    /// MaLo's typed columns — carries the applied `patch` for ERP audit.
    pub const MALO_STAMMDATEN_GEAENDERT: &str = "de.markt.malo.stammdaten-geaendert";
    /// Object-generic Stammdatenänderung applied to a non-MaLo master-data object
    /// (MeLo/NeLo/Tranche). The subject is the object's location id; the payload
    /// carries the `objekt` marker.
    pub const STAMMDATEN_GEAENDERT: &str = "de.markt.stammdaten.geaendert";
    /// Messlokation stammdaten changed.
    pub const MELO_UPDATED: &str = "de.markt.melo.updated";
    /// Marktpartner record changed.
    pub const PARTNER_UPDATED: &str = "de.markt.partner.updated";
    /// Netzbetreiber contract (Lieferanten-Rahmenvertrag) changed.
    pub const NB_CONTRACT_UPDATED: &str = "de.markt.nb-contract.updated";
    /// MSB-Rahmenvertrag Gas changed.
    pub const MSB_RAHMENVERTRAG_GAS_UPDATED: &str = "de.markt.msb-rahmenvertrag-gas.updated";
    /// Device (Gerät) configuration changed.
    pub const GERAET_KONFIGURATION_UPDATED: &str = "de.markt.geraet.konfiguration.updated";
    /// Steuerbare-Ressource Konfigurationsprodukt changed (§14a).
    pub const SR_KONFIGURATIONSPRODUKT_UPDATED: &str = "de.markt.sr.konfigurationsprodukt.updated";
    /// §20 NZV Netzzugang application state changed.
    pub const NETZZUGANG_ANTRAG_UPDATED: &str = "de.markt.netzzugang.antrag.updated";
    /// Einwilligung (consent) granted.
    pub const EINWILLIGUNG_ERTEILT: &str = "de.markt.einwilligung.erteilt";
    /// Einwilligung (consent) revoked.
    pub const EINWILLIGUNG_WIDERRUFEN: &str = "de.markt.einwilligung.widerrufen";
    /// Versorgung state changed (generic transition).
    pub const VERSORGUNG_CHANGED: &str = "de.markt.versorgung.changed";
    /// Versorgung entered `BELIEFERT` (supply active).
    pub const VERSORGUNG_BELIEFERT: &str = "de.markt.versorgung.beliefert";
    /// Supply gap detected: a MaLo became `Unbeliefert` with no announced
    /// successor — the NB must activate Ersatz-/Grundversorgung (§38 EnWG).
    /// Consumed by the `processd` EoG gap-closure automation.
    pub const VERSORGUNG_GAP_DETECTED: &str = "de.markt.versorgung.gap-detected";
    /// Statutory fallback supply began (Ersatz- or Grundversorgung);
    /// `data.eog_art` carries the regime, `data.eog_seit` the start date.
    pub const VERSORGUNG_EOG_BEGONNEN: &str = "de.markt.versorgung.eog-begonnen";
    /// §38 Abs. 2 EnWG: a running Ersatzversorgung approaches its 3-month
    /// maximum. Emitted by the `processd` EoG timer.
    pub const VERSORGUNG_ERSATZ_AUSLAUFEND: &str = "de.markt.versorgung.ersatz-auslaufend";
    /// PRICAT price catalog published.
    pub const PRICAT_PUBLISHED: &str = "de.markt.pricat.published";
    /// MMMA import batch succeeded.
    pub const MMMA_IMPORT_SUCCESS: &str = "de.markt.mmma.import.success";
    /// MMMA import batch failed.
    pub const MMMA_IMPORT_FAILED: &str = "de.markt.mmma.import.failed";
    /// Subscription self-test event (`POST /subscriptions/{id}/test`).
    pub const SUBSCRIPTION_TEST: &str = "de.markt.subscription.test";
    /// Grid topology drift between NIS/GIS and marktd detected.
    ///
    /// Kept under `de.markt` even though `nis-syncd` (not `marktd`) emits
    /// it — the subject is marktd master data drifting from the NIS.
    pub const GRID_DRIFT_DETECTED: &str = "de.markt.grid.drift.detected";
}

/// Billing events (`de.billing.*`), emitted by `billingd`.
pub mod billing {
    /// Invoice (Rechnung) created.
    pub const RECHNUNG_ERSTELLT: &str = "de.billing.rechnung.erstellt";
    /// Credit note (Gutschrift) created.
    pub const GUTSCHRIFT_ERSTELLT: &str = "de.billing.gutschrift.erstellt";
    /// Monthly Abrechnungsinformation (§40 EnWG) generated.
    pub const ABRECHNUNGSINFORMATION_MONATLICH: &str =
        "de.billing.abrechnungsinformation.monatlich";
    /// XRechnung for a B2G recipient is ready for dispatch.
    pub const XRECHNUNG_B2G_READY: &str = "de.billing.xrechnung.b2g.ready";
}

/// INVOIC receipt/payment events (`de.invoic.*`), emitted by `invoicd`.
pub mod invoic {
    /// Inbound INVOIC disputed (REMADV Ablehnung path).
    pub const RECEIPT_DISPUTED: &str = "de.invoic.receipt.disputed";
    /// Inbound INVOIC dispatched to the ERP.
    pub const RECEIPT_DISPATCHED: &str = "de.invoic.receipt.dispatched";
    /// Inbound INVOIC settled (REMADV Zahlungsavis).
    pub const RECEIPT_SETTLED: &str = "de.invoic.receipt.settled";
    /// Outbound INVOIC payment overdue (no REMADV in time).
    pub const PAYMENT_OVERDUE: &str = "de.invoic.payment.overdue";
}

/// EEG/KWKG settlement events (`de.eeg.*`), emitted by `einsd`.
///
/// `agentd` additionally subscribes to the globs `de.eeg.*`,
/// `de.eeg.anlage.*`, `de.eeg.verguetung.*`, `de.eeg.marktpraemie.*` and
/// `de.eeg.compliance.*` (no concrete `de.eeg.compliance.*` type exists
/// yet).
pub mod eeg {
    /// Einspeisevergütung settlement computed (feed-in tariff schemes).
    pub const VERGUETUNG_BERECHNET: &str = "de.eeg.verguetung.berechnet";
    /// Marktprämie settlement computed (Direktvermarktung schemes).
    pub const MARKTPRAEMIE_BERECHNET: &str = "de.eeg.marktpraemie.berechnet";
    /// Generic settlement computed (MCP `trigger_settle`).
    pub const SETTLEMENT_BERECHNET: &str = "de.eeg.settlement.berechnet";
    /// ⚠ phantom: subscribed by agentd (`einsd-batch-agent`), no emitter
    /// yet (tracked in ROADMAP). Monthly auto-settle batch trigger.
    pub const SETTLEMENT_BATCH_DUE: &str = "de.eeg.settlement.batch_due";
    /// EEG-Anlage Förderung ends within the warning window.
    pub const ANLAGE_FOERDERUNG_AUSLAUFEND: &str = "de.eeg.anlage.foerderung_auslaufend";
    /// EEG-Anlage MaStR registration confirmed.
    pub const ANLAGE_MASTR_REGISTRIERT: &str = "de.eeg.anlage.mastr_registriert";
    /// §21b Veräußerungsform switched.
    pub const VERAEUSSERUNGSFORM_GEWECHSELT: &str = "de.eeg.veraeusserungsform.gewechselt";
}

/// FI-CA subledger events (`de.accounting.*`), emitted by `accountingd`.
pub mod accounting {
    /// Dunning notice (Mahnung) issued.
    pub const MAHNUNG_ISSUED: &str = "de.accounting.mahnung.issued";
    /// Abschlag (installment) posted.
    pub const ABSCHLAG_POSTED: &str = "de.accounting.abschlag.posted";
    /// Payment due notification.
    pub const PAYMENT_DUE: &str = "de.accounting.payment.due";
    /// Bank statement payment imported and matched.
    pub const PAYMENT_IMPORTED: &str = "de.accounting.payment.imported";
    /// Refund (Erstattung) due to the customer.
    pub const ERSTATTUNG_FAELLIG: &str = "de.accounting.erstattung.faellig";
    /// Late-payment interest (§288 BGB) charged.
    pub const INTEREST_CHARGED: &str = "de.accounting.interest.charged";
    /// EEG payout rejected (e.g. missing bank data).
    pub const EEG_PAYOUT_REJECTED: &str = "de.accounting.eeg.payout.rejected";
    /// ⚠ phantom: subscribed by agentd (`sperr-agent`), no emitter yet
    /// (tracked in ROADMAP). Disconnection order after exhausted dunning.
    pub const SPERRAUFTRAG: &str = "de.accounting.sperrauftrag";
    /// ⚠ phantom: subscribed by agentd (`payment-agent`), no emitter yet
    /// (tracked in ROADMAP). SEPA direct-debit return (Bankrücklastschrift).
    pub const BANKRUECKLAST: &str = "de.accounting.bankruecklast";
}

/// MaBiS/Netzbilanzierung INVOIC events (`de.netzbilanz.*`), emitted by
/// `netzbilanzd`.
pub mod netzbilanz {
    /// Bilanzkreis INVOIC drafted.
    pub const INVOIC_DRAFTED: &str = "de.netzbilanz.invoic.drafted";
    /// Bilanzkreis INVOIC dispatched.
    pub const INVOIC_DISPATCHED: &str = "de.netzbilanz.invoic.dispatched";
    /// Bilanzkreis INVOIC not dispatched before its deadline.
    pub const INVOIC_DISPATCH_OVERDUE: &str = "de.netzbilanz.invoic.dispatch_overdue";
    /// Bilanzkreis INVOIC paid (REMADV settled).
    pub const INVOIC_PAID: &str = "de.netzbilanz.invoic.paid";
    /// Bilanzkreis INVOIC disputed.
    pub const INVOIC_DISPUTED: &str = "de.netzbilanz.invoic.disputed";
    /// Kostenblatt computed.
    pub const KOSTENBLATT_COMPUTED: &str = "de.netzbilanz.kostenblatt.computed";
    /// Kostenblatt submission deadline approaching.
    pub const KOSTENBLATT_DEADLINE_APPROACHING: &str =
        "de.netzbilanz.kostenblatt.deadline_approaching";
}

/// Meter-reading / energy-data events (`de.messwert.*`), emitted by `edmd`.
///
/// Renamed from the legacy `de.edmd.*` prefix — the context is the
/// Messwert (meter value), not the daemon that happens to store it.
pub mod messwert {
    /// Hampel/V01–V10 quality flag on new meter readings (grade C/F).
    pub const READING_QUALITY_WARNING: &str = "de.messwert.reading.quality.warning";
    /// Direct iMSys/SMGW push stored.
    pub const READING_DIRECT_STORED: &str = "de.messwert.reading.direct.stored";
    /// Ablesesteuerung reading order failed.
    pub const READING_ORDER_FAILED: &str = "de.messwert.reading.order.failed";
    /// Expected reading confirmation overdue.
    pub const READING_CONFIRMATION_OVERDUE: &str = "de.messwert.reading.confirmation.overdue";
    /// §14a SMGW/CLS compliance issue detected (MsbG §21c sweep).
    pub const CLS_COMPLIANCE_ISSUE: &str = "de.messwert.cls.compliance_issue";
    /// SMGW certificate approaching expiry — tiered advance warning at 90 / 30 / 7
    /// days (BSI TR-03109-4 §6.3). An expired cert silently ends §14a
    /// Fernsteuerbarkeit and the MsbG §29 remote-readout obligation, so each tier
    /// fires once per certificate as it ages.
    pub const SMGW_CERT_EXPIRY_WARNING: &str = "de.messwert.smgw.cert.expiry_warning";
}

/// Product & tariff catalog events (`de.tarif.*`), emitted by `tarifbd`.
///
/// Renamed from the legacy `de.tarifbd.*` prefix (and the stray top-level
/// `de.angebot.angenommen`).
pub mod tarif {
    /// Product created/updated in the catalog.
    pub const PRODUCT_UPDATED: &str = "de.tarif.product.updated";
    /// B2B Angebot accepted — vertragd auto-creates the Rahmenvertrag.
    pub const ANGEBOT_ANGENOMMEN: &str = "de.tarif.angebot.angenommen";
    /// ⚠ phantom: subscribed by agentd (`tarifbd-agent`), no emitter yet
    /// (tracked in ROADMAP). B2B quote expired.
    pub const ANGEBOT_ABGELAUFEN: &str = "de.tarif.angebot.abgelaufen";
    /// ⚠ phantom: subscribed by agentd (`tarifbd-agent`), no emitter yet
    /// (tracked in ROADMAP). EPEX D-1 prices not imported by 18:00 CET.
    pub const EPEX_MISSING: &str = "de.tarif.epex.missing";
}

/// Contract lifecycle events (`de.vertrag.*`), emitted by `vertragd`.
///
/// `agentd` additionally subscribes to the glob `de.vertrag.*`.
pub mod vertrag {
    /// All components NB-confirmed — billing may start.
    pub const AKTIV: &str = "de.vertrag.aktiv";
    /// Lieferende dispatched (Rahmenvertrag cascade, per child).
    pub const GEKUENDIGT: &str = "de.vertrag.gekuendigt";
    /// Kündigung accepted, Lieferende dispatched.
    ///
    /// Documented in the vertragd event table; no code emitter exists
    /// today (the cancel path emits [`GEKUENDIGT`]).
    pub const KUENDIGUNG: &str = "de.vertrag.kuendigung";
    /// Kündigung withdrawn before Lieferende.
    pub const KUENDIGUNG_WIDERRUFEN: &str = "de.vertrag.kuendigung_widerrufen";
    /// Product change applied immediately.
    pub const TARIFWECHSEL: &str = "de.vertrag.tarifwechsel";
    /// Future-dated product change stored.
    pub const TARIFWECHSEL_GEPLANT: &str = "de.vertrag.tarifwechsel_geplant";
    /// Price guarantee stored/replaced.
    pub const PREISGARANTIE_UPDATED: &str = "de.vertrag.preisgarantie_updated";
    /// §41 Abs. 5 EnWG price-change notice (≤ 42 days before Wirksamkeit).
    pub const PREISAENDERUNG_ANKUENDIGUNG: &str = "de.vertrag.preisaenderung.ankuendigung";
    /// 30 days before auto-renewal.
    pub const AUTOERNEUERUNG_ANKUENDIGUNG: &str = "de.vertrag.autoerneuerung.ankuendigung";
    /// 30 days before vertragsende / preisgarantie_bis.
    pub const ABLAUF_ANKUENDIGUNG: &str = "de.vertrag.ablauf.ankuendigung";
}

/// Virtual-power-plant events (`de.vpp.*`).
pub mod vpp {
    /// VPP dispatch confirmed (ERP event type, emitted via mako-engine).
    pub const DISPATCH_CONFIRMED: &str = "de.vpp.dispatch.confirmed";
    /// VPP settlement computed (emitted by `billingd`).
    pub const SETTLEMENT_BERECHNET: &str = "de.vpp.settlement.berechnet";
}

/// Agent runtime events (`de.agent.*`), emitted by `agentd`.
pub mod agent {
    /// An agent session finished and produced a decision.
    pub const DECISION_MADE: &str = "de.agent.decision.made";
    /// A DLQ entry exhausted its redelivery attempts.
    pub const SESSION_DLQ_EXHAUSTED: &str = "de.agent.session.dlq.exhausted";
}

/// GaBi Gas balancing events (`de.gabi.*`), defined in `mako-gabi-gas`.
///
/// ⚠ phantom (all 12): subscribed by agentd (`gabi-gas-agent` globs
/// `de.gabi.imbalance.*`, `de.gabi.nomination.*` and exact types), but no
/// service emits them yet (tracked in ROADMAP).
pub mod gabi {
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const MEASUREMENT_RECEIVED: &str = "de.gabi.measurement.received";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const ALLOCATION_COMPLETED: &str = "de.gabi.allocation.completed";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const NOMINATION_CREATED: &str = "de.gabi.nomination.created";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const NOMINATION_CONFIRMED: &str = "de.gabi.nomination.confirmed";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const IMBALANCE_CALCULATED: &str = "de.gabi.imbalance.calculated";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const CORRECTION_CREATED: &str = "de.gabi.correction.created";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const INVOIC_MMM_RECEIVED: &str = "de.gabi.invoic.mmm.received";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const INVOIC_KAPAZITAET_RECEIVED: &str = "de.gabi.invoic.kapazitaet.received";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const ALOCAT_MISSING: &str = "de.gabi.alocat.missing";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const IMBNOT_RECEIVED: &str = "de.gabi.imbnot.received";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const GAS_QUALITY_VIOLATION: &str = "de.gabi.quality.violation";
    /// ⚠ phantom: no emitter yet (tracked in ROADMAP).
    pub const FINAL_ALOCAT_DEADLINE: &str = "de.gabi.alocat.final.deadline";
}

/// Process-observability events (`de.obs.*`).
///
/// Produced by `obsd`'s background sweep workers (`services/obsd/src/worker.rs`)
/// and consumed by `agentd` (`compliance-agent`, `deadline-alert-agent`).
pub mod obs {
    /// §20 EnWG STP parity-gap alert: the completion-rate gap between affiliate-
    /// and non-affiliate-initiated Anmeldungen exceeds the configured threshold.
    /// Emitted by obsd's parity sweep; consumed by agentd (`compliance-agent`).
    pub const STP_PARITY_ALERT: &str = "de.obs.stp.parity.alert";
    /// A tracked process is approaching its regulatory response deadline (within
    /// the warn window). Emitted per process by obsd's deadline sweep; consumed
    /// by agentd (`deadline-alert-agent`).
    pub const DEADLINE_APPROACHING: &str = "de.obs.deadline.approaching";
}

/// Sperr/Entsperr (disconnection) events (`de.sperr.*`).
///
/// ⚠ phantom: agentd's `sperr-agent` subscribes to the glob `de.sperr.*`,
/// but no concrete `de.sperr.*` type is defined or emitted anywhere yet
/// (tracked in ROADMAP). Add constants here when `sperrd` starts emitting.
pub mod sperr {}

/// Every concrete CloudEvents type in the catalog.
#[must_use]
pub fn all() -> &'static [&'static str] {
    &[
        // de.mako.*
        mako::PROCESS_INITIATED,
        mako::PROCESS_COMPLETED,
        mako::PROCESS_FAILED,
        mako::APERAK_ACCEPTED,
        mako::APERAK_REJECTED,
        mako::APERAK_TIMEOUT,
        mako::CONTRL_RECEIVED,
        mako::MALO_IDENTIFIED,
        mako::EDIFACT_OUTBOUND,
        mako::EDIFACT_INBOUND,
        mako::NETZZUGANG_UEBERMITTLUNGSBEDARF,
        // de.markt.*
        markt::MALO_UPDATED,
        markt::MALO_STAMMDATEN_GEAENDERT,
        markt::MELO_UPDATED,
        markt::PARTNER_UPDATED,
        markt::NB_CONTRACT_UPDATED,
        markt::MSB_RAHMENVERTRAG_GAS_UPDATED,
        markt::GERAET_KONFIGURATION_UPDATED,
        markt::SR_KONFIGURATIONSPRODUKT_UPDATED,
        markt::NETZZUGANG_ANTRAG_UPDATED,
        markt::EINWILLIGUNG_ERTEILT,
        markt::EINWILLIGUNG_WIDERRUFEN,
        markt::VERSORGUNG_CHANGED,
        markt::VERSORGUNG_BELIEFERT,
        markt::VERSORGUNG_GAP_DETECTED,
        markt::VERSORGUNG_EOG_BEGONNEN,
        markt::VERSORGUNG_ERSATZ_AUSLAUFEND,
        markt::PRICAT_PUBLISHED,
        markt::MMMA_IMPORT_SUCCESS,
        markt::MMMA_IMPORT_FAILED,
        markt::SUBSCRIPTION_TEST,
        markt::GRID_DRIFT_DETECTED,
        // de.billing.*
        billing::RECHNUNG_ERSTELLT,
        billing::GUTSCHRIFT_ERSTELLT,
        billing::ABRECHNUNGSINFORMATION_MONATLICH,
        billing::XRECHNUNG_B2G_READY,
        // de.invoic.*
        invoic::RECEIPT_DISPUTED,
        invoic::RECEIPT_DISPATCHED,
        invoic::RECEIPT_SETTLED,
        invoic::PAYMENT_OVERDUE,
        // de.eeg.*
        eeg::VERGUETUNG_BERECHNET,
        eeg::MARKTPRAEMIE_BERECHNET,
        eeg::SETTLEMENT_BERECHNET,
        eeg::SETTLEMENT_BATCH_DUE,
        eeg::ANLAGE_FOERDERUNG_AUSLAUFEND,
        eeg::ANLAGE_MASTR_REGISTRIERT,
        eeg::VERAEUSSERUNGSFORM_GEWECHSELT,
        // de.accounting.*
        accounting::MAHNUNG_ISSUED,
        accounting::ABSCHLAG_POSTED,
        accounting::PAYMENT_DUE,
        accounting::PAYMENT_IMPORTED,
        accounting::ERSTATTUNG_FAELLIG,
        accounting::INTEREST_CHARGED,
        accounting::EEG_PAYOUT_REJECTED,
        accounting::SPERRAUFTRAG,
        accounting::BANKRUECKLAST,
        // de.netzbilanz.*
        netzbilanz::INVOIC_DRAFTED,
        netzbilanz::INVOIC_DISPATCHED,
        netzbilanz::INVOIC_DISPATCH_OVERDUE,
        netzbilanz::INVOIC_PAID,
        netzbilanz::INVOIC_DISPUTED,
        netzbilanz::KOSTENBLATT_COMPUTED,
        netzbilanz::KOSTENBLATT_DEADLINE_APPROACHING,
        // de.messwert.*
        messwert::READING_QUALITY_WARNING,
        messwert::READING_DIRECT_STORED,
        messwert::READING_ORDER_FAILED,
        messwert::READING_CONFIRMATION_OVERDUE,
        messwert::CLS_COMPLIANCE_ISSUE,
        messwert::SMGW_CERT_EXPIRY_WARNING,
        // de.tarif.*
        tarif::PRODUCT_UPDATED,
        tarif::ANGEBOT_ANGENOMMEN,
        tarif::ANGEBOT_ABGELAUFEN,
        tarif::EPEX_MISSING,
        // de.vertrag.*
        vertrag::AKTIV,
        vertrag::GEKUENDIGT,
        vertrag::KUENDIGUNG,
        vertrag::KUENDIGUNG_WIDERRUFEN,
        vertrag::TARIFWECHSEL,
        vertrag::TARIFWECHSEL_GEPLANT,
        vertrag::PREISGARANTIE_UPDATED,
        vertrag::PREISAENDERUNG_ANKUENDIGUNG,
        vertrag::AUTOERNEUERUNG_ANKUENDIGUNG,
        vertrag::ABLAUF_ANKUENDIGUNG,
        // de.vpp.*
        vpp::DISPATCH_CONFIRMED,
        vpp::SETTLEMENT_BERECHNET,
        // de.agent.*
        agent::DECISION_MADE,
        agent::SESSION_DLQ_EXHAUSTED,
        // de.gabi.*
        gabi::MEASUREMENT_RECEIVED,
        gabi::ALLOCATION_COMPLETED,
        gabi::NOMINATION_CREATED,
        gabi::NOMINATION_CONFIRMED,
        gabi::IMBALANCE_CALCULATED,
        gabi::CORRECTION_CREATED,
        gabi::INVOIC_MMM_RECEIVED,
        gabi::INVOIC_KAPAZITAET_RECEIVED,
        gabi::ALOCAT_MISSING,
        gabi::IMBNOT_RECEIVED,
        gabi::GAS_QUALITY_VIOLATION,
        gabi::FINAL_ALOCAT_DEADLINE,
        // de.obs.*
        obs::STP_PARITY_ALERT,
        obs::DEADLINE_APPROACHING,
    ]
}

/// The canonical event-type pattern matcher, shared by every subscription
/// mechanism in the workspace (marktd webhook subscriptions, agentd trigger
/// patterns).
///
/// Semantics: `*` matches any (possibly empty) sequence, `?` matches exactly
/// one character, everything else is literal. A bare `*` matches everything.
/// Trailing-`*` prefix patterns (`de.mako.*`) therefore behave exactly like
/// the historical marktd prefix matcher, and mid-pattern globs
/// (`de.*.rechnung.*`) work too.
///
/// There is deliberately ONE implementation: before 2026-07 marktd and agentd
/// each carried their own with silently different semantics (exact+prefix vs
/// full glob).
#[must_use]
pub fn matches(pattern: &str, event_type: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = event_type.chars().collect();
    let mut pi = 0usize;
    let mut vi = 0usize;
    let mut star_pi: Option<usize> = None;
    let mut star_vi = 0usize;

    while vi < v.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star_pi = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod matcher_tests {
    use super::matches;

    #[test]
    fn exact_and_bare_star() {
        assert!(matches(
            super::mako::PROCESS_INITIATED,
            super::mako::PROCESS_INITIATED
        ));
        assert!(!matches(
            super::mako::PROCESS_INITIATED,
            super::mako::PROCESS_COMPLETED
        ));
        assert!(matches("*", "de.anything.at.all"));
    }

    #[test]
    fn trailing_star_is_prefix_match() {
        // The historical marktd subscription semantics.
        assert!(matches("de.mako.*", "de.mako.process.initiated"));
        assert!(matches("de.markt.*", "de.markt.malo.updated"));
        assert!(!matches("de.mako.*", "de.markt.malo.updated"));
        // Prefix boundary is character-wise, exactly like the old
        // `starts_with(trim_end_matches('*'))`.
        assert!(matches("de.mako.process.*", "de.mako.process.failed"));
    }

    #[test]
    fn mid_glob_and_question_mark() {
        assert!(matches(
            "de.*.rechnung.erstellt",
            "de.billing.rechnung.erstellt"
        ));
        assert!(matches(
            "de.e?g.verguetung.berechnet",
            "de.eeg.verguetung.berechnet"
        ));
        assert!(!matches(
            "de.e?g.verguetung.berechnet",
            "de.eeeg.verguetung.berechnet"
        ));
    }

    #[test]
    fn empty_pattern_matches_only_empty() {
        assert!(matches("", ""));
        assert!(!matches("", "de.x"));
    }
}

#[cfg(test)]
mod tests {
    use super::all;

    #[test]
    fn every_type_starts_with_de_prefix() {
        for ty in all() {
            assert!(ty.starts_with("de."), "{ty} must start with `de.`");
        }
    }

    #[test]
    fn every_type_is_lowercase() {
        for ty in all() {
            assert_eq!(
                *ty,
                ty.to_lowercase(),
                "{ty} must be entirely lowercase (CloudEvents type convention)"
            );
        }
    }

    #[test]
    fn every_type_has_valid_segments() {
        for ty in all() {
            assert!(
                !ty.contains(' ') && !ty.contains('*'),
                "{ty} must be a concrete type — no whitespace, no globs"
            );
            for segment in ty.split('.') {
                assert!(!segment.is_empty(), "{ty} has an empty `.` segment");
                assert!(
                    segment
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_".contains(c)),
                    "{ty}: segment `{segment}` has characters outside [a-z0-9_-]"
                );
            }
        }
    }

    #[test]
    fn no_duplicates() {
        let types = all();
        let mut seen = std::collections::BTreeSet::new();
        for ty in types {
            assert!(seen.insert(*ty), "duplicate catalog entry: {ty}");
        }
    }

    #[test]
    fn legacy_prefixes_are_gone() {
        for ty in all() {
            assert!(
                !ty.starts_with("de.edmd.") && !ty.starts_with("de.tarifbd."),
                "{ty} uses a retired service-name prefix (use de.messwert / de.tarif)"
            );
            assert_ne!(
                *ty, "de.angebot.angenommen",
                "moved to de.tarif.angebot.angenommen"
            );
        }
    }
}
