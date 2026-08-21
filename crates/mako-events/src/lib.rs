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
//! - Segments are separated by `.`, and a multi-word segment joins its words
//!   with `-` — never `_`. German domain nouns keep their established spelling
//!   (participles like `beliefert`, compound nouns like `nb-contract`).
//!   `segments_use_hyphen_not_underscore` enforces this.
//! - A namespace names its facts in one language. `de.vertrag.*` is German
//!   throughout (`gekuendigt`, `preisgarantie-hinterlegt`); `de.mako.*`,
//!   `de.gabi.*` and `de.accounting.*` are technical or English-domain and stay
//!   English. `german_namespaces_use_german_participles` enforces the German
//!   set. This is about the *participle*, not the noun — and it does not mean
//!   `.updated` and `.geaendert` are interchangeable: in `de.markt.*` they name
//!   different facts (any master-data write vs. a regulated GPKE
//!   Stammdatenänderung carrying its patch).
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
    /// Outbound EDIFACT interchange handed to the webhook EDIFACT sender.
    ///
    /// Emitted **only** by `WebhookEdifactSender`, the development / ERP
    /// integration transport used when there is no AS4 infrastructure; the
    /// production `BdewAs4Sender` path does not emit it. The CloudEvent is the
    /// delivery envelope carrying the interchange, not a notification about it.
    pub const EDIFACT_OUTBOUND: &str = "de.mako.edifact.outbound";
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
}

/// Billing events (`de.billing.*`), emitted by `billingd`.
pub mod billing {
    /// Invoice (Rechnung) created. A credit note / Stornorechnung is the same
    /// event with `data.is_correction = true` and a negated amount — there is
    /// deliberately no separate `gutschrift` type (one signed document stream is
    /// cleaner for the double-entry ledger, and avoids a double-booking hazard).
    pub const RECHNUNG_ERSTELLT: &str = "de.billing.rechnung.erstellt";
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
    /// Monthly auto-settle batch trigger. Emitted by einsd's auto-settle worker
    /// (`emit_batch_due_ce`); subscribed by agentd's `einsd-batch-agent`.
    pub const SETTLEMENT_BATCH_DUE: &str = "de.eeg.settlement.batch-due";
    /// EEG-Anlage Förderung ends within the warning window.
    pub const ANLAGE_FOERDERUNG_AUSLAUFEND: &str = "de.eeg.anlage.foerderung-auslaufend";
    /// EEG-Anlage MaStR registration confirmed.
    pub const ANLAGE_MASTR_REGISTRIERT: &str = "de.eeg.anlage.mastr-registriert";
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
    /// §41f Abs. 1 EnWG — Sperrandrohung: disconnection threatened after an
    /// unresolved Mahnstufe-3 dunning case, opening the 4-Wochen-Frist.
    pub const SPERRANDROHUNG: &str = "de.accounting.sperrandrohung";
    /// §41f Abs. 5 EnWG — Sperrankündigung: the concrete disconnection date is
    /// announced 8 Werktage in advance (after the 4-Wochen Androhung Frist).
    pub const SPERRANKUENDIGUNG: &str = "de.accounting.sperrankuendigung";
    /// §41f Abs. 1 EnWG — Sperrauftrag: **ORDERS 17115** dispatched to the
    /// Netzbetreiber once the Androhung (4 Wochen) and Ankündigung (8 Werktage)
    /// Fristen have elapsed, the Abs. 3 arrears gates still hold, and no
    /// Abwendungsvereinbarung / Unverhältnismäßigkeit halted the sequence. The
    /// event announces the market message; it does not carry it.
    pub const SPERRAUFTRAG: &str = "de.accounting.sperrauftrag";
    /// §41f Abs. 7 EnWG — Entsperrauftrag: **ORDERS 17117** dispatched once the
    /// grounds for the interruption are gone (the arrears were settled). The
    /// statute makes the restoration *unverzüglich* and unconditional on being
    /// asked, so this follows automatically from the case settling.
    pub const ENTSPERRAUFTRAG: &str = "de.accounting.entsperrauftrag";
    /// §41g Abs. 1 S. 2 EnWG — the Grundversorger's offer of an
    /// Abwendungsvereinbarung: due within one week of the customer demanding it
    /// after the Androhung, and at the latest together with the Ankündigung.
    /// Carries the interest-free instalment terms (§41g Abs. 1 S. 7–9).
    pub const ABWENDUNG_ANGEBOTEN: &str = "de.accounting.abwendung.angeboten";
    /// §41g Abs. 1 S. 11 EnWG — an accepted Abwendungsvereinbarung was broken.
    /// The supplier may resume the interruption, but must re-observe §41f
    /// Abs. 1 S. 2 and issue a **fresh** Ankündigung (§41f Abs. 5).
    pub const ABWENDUNG_GEBROCHEN: &str = "de.accounting.abwendung.gebrochen";
    /// SEPA direct-debit return (Bankrücklastschrift) — emitted by
    /// `accountingd` when a camt.053/054 booking carries a return reason code
    /// or debits the account, and subscribed by agentd's `payment-agent`.
    pub const BANKRUECKLAST: &str = "de.accounting.bankruecklast";
    /// A pain.002 rejected a submitted direct-debit collection: the money will
    /// never arrive, so the receivable stays open and the mandate needs
    /// attention (`AC01` wrong IBAN, `MD01` no mandate, `AM04` no funds).
    ///
    /// Distinct from [`BANKRUECKLAST`], which is a collection that *settled*
    /// and was then returned — a different reconciliation, and a different
    /// R-transaction fee.
    pub const SEPA_COLLECTION_REJECTED: &str = "de.accounting.sepa.collection-rejected";
    /// The creditor gave a settled collection back via pain.007.
    pub const SEPA_REVERSAL_ISSUED: &str = "de.accounting.sepa.reversal-issued";
    /// Verification of Payee reported something other than a match for an
    /// outgoing credit transfer. Mandatory for euro credit transfers since
    /// 9 October 2025: executing after a `RVNM` no-match shifts liability to
    /// the payer, so this is a decision an operator has to make.
    pub const PAYEE_VERIFICATION_MISMATCH: &str = "de.accounting.payee.verification-mismatch";
}

/// MaBiS/Netzbilanzierung INVOIC events (`de.netzbilanz.*`), emitted by
/// `netzbilanzd`.
pub mod netzbilanz {
    /// A Netzbetreiber invoice was settled and stored as a draft.
    pub const INVOIC_DRAFTED: &str = "de.netzbilanz.invoic.drafted";
    /// The invoice was handed to `makod` for EDIFACT dispatch.
    pub const INVOIC_DISPATCHED: &str = "de.netzbilanz.invoic.dispatched";
    /// A draft is still undispatched past its window.
    pub const INVOIC_DISPATCH_OVERDUE: &str = "de.netzbilanz.invoic.dispatch-overdue";
    /// The counterparty confirmed payment (REMADV 33001, the only Bestätigung).
    pub const INVOIC_PAID: &str = "de.netzbilanz.invoic.paid";
    /// The counterparty rejected the invoice (REMADV 33002/33003/33004).
    pub const INVOIC_DISPUTED: &str = "de.netzbilanz.invoic.disputed";
    /// Kostenblatt computed.
    pub const KOSTENBLATT_COMPUTED: &str = "de.netzbilanz.kostenblatt.computed";
    /// Kostenblatt submission deadline approaching.
    pub const KOSTENBLATT_DEADLINE_APPROACHING: &str =
        "de.netzbilanz.kostenblatt.deadline-approaching";
}

/// Meter-reading / energy-data events (`de.messwert.*`), emitted by `edmd`.
///
/// Renamed from the legacy `de.edmd.*` prefix — the context is the
/// Messwert (meter value), not the daemon that happens to store it.
pub mod messwert {
    /// Hampel grade C/F, or any V-rule finding, on newly ingested readings.
    pub const READING_QUALITY_WARNING: &str = "de.messwert.reading.quality.warning";
    /// Direct iMSys/SMGW push stored.
    pub const READING_DIRECT_STORED: &str = "de.messwert.reading.direct.stored";
    /// Ablesesteuerung reading order failed.
    pub const READING_ORDER_FAILED: &str = "de.messwert.reading.order.failed";
    /// Expected reading confirmation overdue.
    pub const READING_CONFIRMATION_OVERDUE: &str = "de.messwert.reading.confirmation.overdue";
    /// A measuring point has stopped delivering, or is delivering too little of
    /// the settlement window to bill.
    ///
    /// The counterpart to [`READING_QUALITY_WARNING`], which can only fire on
    /// data that *arrived*. Silence produces no ingest and therefore no
    /// validation, so without this a head-end that simply stops is invisible
    /// until a settlement run comes up short — by which point the window in
    /// which the values could still have been re-read has closed
    /// (§ 60 Abs. 2 MsbG).
    pub const READING_DELIVERY_OVERDUE: &str = "de.messwert.reading.delivery.overdue";
    /// A measuring point that was overdue is delivering again.
    pub const READING_DELIVERY_RESUMED: &str = "de.messwert.reading.delivery.resumed";
    /// §14a SMGW/CLS compliance issue **opened** (§ 25 MsbG monitoring duty).
    ///
    /// Fires on the transition into a fault, not on every sweep that still sees
    /// it — see `cls_compliance_issues`.
    pub const CLS_COMPLIANCE_ISSUE: &str = "de.messwert.cls.compliance-issue";
    /// A §14a SMGW/CLS compliance issue a later sweep no longer finds.
    pub const CLS_COMPLIANCE_RESOLVED: &str = "de.messwert.cls.compliance-resolved";
    /// SMGW certificate approaching expiry — tiered advance warning at 90 / 30 /
    /// 7 days before `valid_to`, once per tier per certificate.
    ///
    /// The ladder is operational, not statutory: BSI TR-03109-4 binds
    /// certificate runtimes while the Root-CP fixes the renewal lead time and
    /// the Zertifikatswechsel overlap. An expired certificate silently ends §14a
    /// Fernsteuerbarkeit.
    pub const SMGW_CERT_EXPIRY_WARNING: &str = "de.messwert.smgw.cert.expiry-warning";
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
    /// Kündigung accepted; the Lieferende and the Schlussablesung are
    /// enqueued. Carries the § 41 Abs. 8 Nr. 2 EnWG Textform confirmation the
    /// supplier owes the customer — the document is produced downstream, the
    /// instruction to produce it commits with the termination.
    ///
    /// A cascade Kündigung over a Rahmenvertrag emits [`GEKUENDIGT`] per child
    /// instead, because the framework contract is what was terminated.
    pub const KUENDIGUNG: &str = "de.vertrag.kuendigung";
    /// Kündigung withdrawn before Lieferende.
    pub const KUENDIGUNG_WIDERRUFEN: &str = "de.vertrag.kuendigung-widerrufen";
    /// Product change applied immediately.
    pub const TARIFWECHSEL: &str = "de.vertrag.tarifwechsel";
    /// Future-dated product change stored.
    pub const TARIFWECHSEL_GEPLANT: &str = "de.vertrag.tarifwechsel-geplant";
    /// Price guarantee stored/replaced.
    pub const PREISGARANTIE_HINTERLEGT: &str = "de.vertrag.preisgarantie-hinterlegt";
    /// § 41 Abs. 5 EnWG price-change notice. Sent as soon as the change is
    /// scheduled — Satz 2 is a floor, not a ceiling — and carrying the regime
    /// that applied plus the Satz 4 Sonderkündigungsrecht.
    pub const PREISAENDERUNG_ANKUENDIGUNG: &str = "de.vertrag.preisaenderung.ankuendigung";
    /// 30 days before auto-renewal.
    pub const AUTOERNEUERUNG_ANKUENDIGUNG: &str = "de.vertrag.autoerneuerung.ankuendigung";
    /// 30 days before vertragsende / preisgarantie_bis.
    pub const ABLAUF_ANKUENDIGUNG: &str = "de.vertrag.ablauf.ankuendigung";
    /// Supply has actually ended: every commodity has passed its Lieferende and
    /// the contract is `ABGELAUFEN`. Distinct from [`GEKUENDIGT`], which is the
    /// day the termination was *accepted* — months earlier for a notice period
    /// that long, and with supply and invoicing running throughout. This is the
    /// event a Schlussrechnung and the § 147 AO retention clock hang off.
    pub const ABGESCHLOSSEN: &str = "de.vertrag.abgeschlossen";
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
    /// An agent run reached a terminal state and produced a decision.
    ///
    /// Carries the run's outcome — `completed`, `failed`, `suspended`,
    /// `exhausted`, `quarantined`, `replanning`, `cancelled` or
    /// `not-admitted` — so a subscriber sees a run awaiting human approval as
    /// readily as a successful one. Beside it: `run_id` (the journal key, which
    /// `GET /api/v1/oversight/runs/{run_id}` takes), `waiting_for` (present only
    /// when suspended — an approval, a message, an instant) and `tokens`.
    ///
    /// There is no separate dead-letter event: a run that fails is resumable
    /// from its journal rather than a message that has nowhere left to go.
    pub const DECISION_MADE: &str = "de.agent.decision.made";
}

/// GaBi Gas balancing events (`de.gabi.*`), defined in `mako-gabi-gas`.
///
/// [`gabi::ALOCAT_MISSING`] is emitted by `makod`: the `gabi-gas-allocation` workflow
/// enqueues a `GabiFinalAllocationOverdue` outbox entry when the KoV §6.4
/// final-allocation window closes unsettled, and `OutboxErpWorker` delivers it
/// as a CloudEvent like every other ERP notification.
///
/// ⚠ The remaining eleven are phantom: subscribed by agentd (`gabi-gas-agent`
/// globs `de.gabi.imbalance.*`, `de.gabi.nomination.*`), but no service emits
/// them yet — the ingest arms for IMBNOT/SCHEDL/TRANOT/DELORD still return
/// `Skipped`, so there is no domain fact to raise an event about (ROADMAP).
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
    /// The KoV §6.4 final-allocation window closed with no binding final
    /// ALOCAT on file, so the gas day's imbalance cannot be settled.
    ///
    /// Emitted by `makod` from the `gabi-gas-allocation` deadline via
    /// `ErpEventType::GabiFinalAllocationOverdue`. The `data` payload carries
    /// `gas_day`, `deadline_label`, `sender_eic`, `receiver_eic` and
    /// `synthetic_pid`. The operator's action is to open a Clearingfall with
    /// the FNB/MGV.
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

/// Sperr/Entsperr execution events (`de.sperr.*`), emitted by `sperrd`.
///
/// These are the **NB side**: the grid operator's record of a Sperr- or
/// Entsperrauftrag it received (ORDERS 17115/17117) and physically carried out.
/// They are not the LF's §41f notices — those are `de.accounting.sperr*`.
///
/// Consumed by agentd's `sperrd-agent`, which watches the execution SLA and the
/// IFTSTA 21039 dispatch.
pub mod sperr {
    /// A Sperr-/Entsperrauftrag entered the field-service queue — either from an
    /// inbound ORDERS 17115/17117 or from an operator creating one directly.
    pub const AUFTRAG_EINGEGANGEN: &str = "de.sperr.auftrag.eingegangen";
    /// The field team carried the order out. The IFTSTA 21039 reporting
    /// `STS+Z37/Z38 → Z14 erfolgreich` has been handed to `makod`.
    pub const AUSGEFUEHRT: &str = "de.sperr.ausgefuehrt";
    /// The order could not be carried out (`Z13 gescheitert`) — meter access
    /// denied, safety block, address not found. Carries the EBD Prüfschritt code
    /// so the LF learns *why* instead of waiting out its ORDRSP deadline.
    pub const FEHLGESCHLAGEN: &str = "de.sperr.fehlgeschlagen";
    /// A Sperrversuch did not succeed but the order stays in the queue: GPKE
    /// Teil 2 § 3.5.1.2 Nr. 5 gives the NB **two** Sperrversuche within one
    /// Sperrauftrag, and only the second turns into `FEHLGESCHLAGEN`.
    pub const VERSUCH_GESCHEITERT: &str = "de.sperr.versuch.gescheitert";
    /// A pending order was withdrawn before execution (operator action, or an
    /// inbound ORDCHG 39000 Stornierung). No IFTSTA is dispatched.
    pub const STORNIERT: &str = "de.sperr.storniert";
    /// A pending order is past the window GPKE Teil 2 § 3.5.1.2 Nr. 1 gives the
    /// NB for the physical act — 6 Werktage after the frühestmöglicher
    /// Sperrtermin. Announced once per order.
    pub const AUSFUEHRUNG_UEBERFAELLIG: &str = "de.sperr.ausfuehrung.ueberfaellig";
    /// An order is terminal in `sperrd` but its IFTSTA 21039 has still not
    /// reached `makod` after the retry budget. Until it does, the LF's
    /// `gpke-sperrung-lf` process cannot close — this is the one state in the
    /// service that needs a human.
    pub const IFTSTA_AUSSTEHEND: &str = "de.sperr.iftsta.ausstehend";
}

/// MaBiS Summenzeitreihe submission events (`de.mabis.*`), emitted by
/// `mabis-syncd`.
///
/// Both are failure signals: a healthy submission cycle is silent, because the
/// scheduled Erstaufschlag run filing on time is the normal case and an event
/// per success would be noise nobody subscribes to.
pub mod mabis {
    /// A Summenzeitreihe aggregation or BIKO submission failed
    /// (BK6-24-174 Anlage 3 §3.10). Carries the run id, the
    /// Bilanzierungsgebiet, the period, the phase and `attempt_count` — after
    /// three attempts the scheduler stops retrying and a human has to look.
    pub const SUBMISSION_FAILED: &str = "de.mabis.submission.failed";
    /// A negative Prüfmitteilung opened a Korrekturbedarf (§9.8.1): the BIKO
    /// or a BKV objected to a filed Summenzeitreihe, and a corrected version
    /// must be submitted within the Clearing window.
    pub const KORREKTURBEDARF_OPENED: &str = "de.mabis.korrekturbedarf.opened";
}

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
        // de.billing.*
        billing::RECHNUNG_ERSTELLT,
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
        accounting::SPERRANDROHUNG,
        accounting::SPERRANKUENDIGUNG,
        accounting::SPERRAUFTRAG,
        accounting::ENTSPERRAUFTRAG,
        accounting::ABWENDUNG_ANGEBOTEN,
        accounting::ABWENDUNG_GEBROCHEN,
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
        messwert::READING_DELIVERY_OVERDUE,
        messwert::READING_DELIVERY_RESUMED,
        messwert::CLS_COMPLIANCE_ISSUE,
        messwert::CLS_COMPLIANCE_RESOLVED,
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
        vertrag::PREISGARANTIE_HINTERLEGT,
        vertrag::PREISAENDERUNG_ANKUENDIGUNG,
        vertrag::AUTOERNEUERUNG_ANKUENDIGUNG,
        vertrag::ABLAUF_ANKUENDIGUNG,
        // de.vpp.*
        vpp::DISPATCH_CONFIRMED,
        vpp::SETTLEMENT_BERECHNET,
        // de.agent.*
        agent::DECISION_MADE,
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
        // de.sperr.*
        sperr::AUFTRAG_EINGEGANGEN,
        sperr::VERSUCH_GESCHEITERT,
        sperr::AUSFUEHRUNG_UEBERFAELLIG,
        sperr::AUSGEFUEHRT,
        sperr::FEHLGESCHLAGEN,
        sperr::STORNIERT,
        sperr::IFTSTA_AUSSTEHEND,
        // de.mabis.*
        mabis::SUBMISSION_FAILED,
        mabis::KORREKTURBEDARF_OPENED,
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

    /// Segments are dot-separated; multi-word segments join with `-`, never `_`.
    ///
    /// The catalog is a published contract, so a drifting separator means two
    /// spellings of the same concept reach subscribers. Ten types used `_`
    /// before this rule was enforced; hyphen is now the single convention.
    /// Namespaces whose domain vocabulary is German keep German participles.
    ///
    /// `de.vertrag.*` is the clearest case: every event is a contract-lifecycle
    /// fact named in the language the contract itself uses — `gekuendigt`,
    /// `kuendigung-widerrufen`, `tarifwechsel-geplant`, `abgelaufen`. One event
    /// used an English participle on a German noun
    /// (`de.vertrag.preisgarantie-updated`), so a subscriber reading the
    /// namespace had to know which of two languages each fact was named in.
    ///
    /// This is deliberately narrow. It is **not** a rule that every event must
    /// be German: `de.mako.*` (EDIFACT transport), `de.gabi.*` and
    /// `de.accounting.*` are technical or English-domain namespaces and stay
    /// English. Nor does it merge `.updated` into `.geaendert` elsewhere —
    /// in `de.markt.*` those are different facts (`malo.updated` is any
    /// master-data write; `malo.stammdaten-geaendert` is a regulated GPKE
    /// Stammdatenänderung carrying the applied patch for audit), and collapsing
    /// them would lose a distinction the ERP relies on.
    /// A constant nothing references must say so.
    ///
    /// `⚠ phantom:` marks a type the catalog declares but no service emits. The
    /// marker only helps if it is true, and prose rots: the roadmap entry that
    /// tracked this drifted to "six constants, none has a subscriber" when the
    /// real figures were eleven unused and a subscriber that does exist
    /// (`gabi-gas-agent` globs `de.gabi.imbalance.*` and `de.gabi.nomination.*`).
    ///
    /// So the annotation is checked rather than trusted: any constant not named
    /// anywhere outside this crate must carry the marker. The reverse does not
    /// hold — `ALOCAT_MISSING` is referenced by a *subscriber* and is still
    /// phantom, because subscribing is not emitting.
    #[test]
    fn unreferenced_constants_are_marked_phantom() {
        let src = include_str!("lib.rs");
        let catalog = src.split("#[cfg(test)]").next().expect("catalog section");

        // Every `pub const NAME: &str = "value";` with the doc block above it.
        let lines: Vec<&str> = catalog.lines().collect();
        let mut entries: Vec<(String, bool)> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let Some(rest) = line.trim().strip_prefix("pub const ") else {
                continue;
            };
            let Some(name) = rest.split(':').next() else {
                continue;
            };
            // Walk back over *this* constant's own doc block only. A fixed
            // lookback window would span into the previous entry's comment —
            // and in `gabi` every neighbour carries the marker, so the check
            // passed for a constant whose marker had been deleted.
            let mut phantom = false;
            for prev in lines[..i].iter().rev() {
                let t = prev.trim();
                if t.starts_with("///") {
                    if t.contains("⚠ phantom:") {
                        phantom = true;
                    }
                } else if !t.is_empty() {
                    break;
                }
            }
            entries.push((name.trim().to_owned(), phantom));
        }
        assert!(
            entries.len() > 80,
            "parsed only {} constants — the parser broke, not the catalog",
            entries.len()
        );

        // Concatenate every other Rust source in the workspace.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        let mut blob = String::new();
        for dir in ["crates", "services"] {
            collect_rs(&root.join(dir), &mut blob);
        }

        let unmarked: Vec<&str> = entries
            .iter()
            .filter(|(name, phantom)| !*phantom && !blob.contains(name.as_str()))
            .map(|(name, _)| name.as_str())
            .collect();

        assert!(
            unmarked.is_empty(),
            "these constants are referenced nowhere outside `mako-events` but carry no \
             `⚠ phantom:` marker:\n  {unmarked:?}\n\
             Either wire an emitter, or document the gap with `⚠ phantom:` so the \
             catalog does not imply the event exists."
        );
    }

    /// Append every `.rs` file under `dir` (skipping this crate) to `out`.
    fn collect_rs(dir: &std::path::Path, out: &mut String) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "mako-events") {
                    continue;
                }
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(s) = std::fs::read_to_string(&path)
            {
                out.push_str(&s);
            }
        }
    }

    #[test]
    fn german_namespaces_use_german_participles() {
        /// Namespaces whose events are named in German.
        const GERMAN_NAMESPACES: &[&str] = &["vertrag"];
        /// English participles that have an established German form already in
        /// use elsewhere in the catalog.
        const ENGLISH_PARTICIPLES: &[&str] = &[
            "updated",
            "changed",
            "created",
            "deleted",
            "stored",
            "replaced",
            "cancelled",
            "canceled",
            "renewed",
            "expired",
            "planned",
        ];

        for ty in all() {
            let Some(ns) = ty.split('.').nth(1) else {
                continue;
            };
            if !GERMAN_NAMESPACES.contains(&ns) {
                continue;
            }
            for bad in ENGLISH_PARTICIPLES {
                assert!(
                    !ty.ends_with(&format!("-{bad}")) && !ty.ends_with(&format!(".{bad}")),
                    "{ty}: `de.{ns}.*` names its facts in German — use the German \
                     participle instead of {bad:?} (e.g. `preisgarantie-hinterlegt`, \
                     not `preisgarantie-updated`)"
                );
            }
        }
    }

    #[test]
    fn segments_use_hyphen_not_underscore() {
        for ty in all() {
            assert!(
                !ty.contains('_'),
                "{ty} must join multi-word segments with `-`, not `_`"
            );
            for segment in ty.split('.') {
                assert!(
                    !segment.is_empty(),
                    "{ty} must not contain an empty segment"
                );
                assert!(
                    !segment.starts_with('-') && !segment.ends_with('-'),
                    "{ty}: segment {segment:?} must not start or end with `-`"
                );
                assert!(
                    segment
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                    "{ty}: segment {segment:?} must match [a-z0-9-]+"
                );
            }
        }
    }

    /// No two catalog entries may differ only by separator or case.
    #[test]
    fn no_two_types_normalise_to_the_same_name() {
        let mut seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
        for ty in all() {
            let key = ty.replace(['-', '_'], "").to_lowercase();
            if let Some(prev) = seen.insert(key, ty) {
                assert_eq!(prev, *ty, "{prev} and {ty} collide after normalisation");
            }
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
