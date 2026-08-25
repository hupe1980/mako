//! WiM Rechnung — the INVOIC billing processes of WiM Strom and WiM Gas.
//!
//! The process lives in [`mako_invoic`]; this module declares the WiM family
//! and the one thing that is genuinely WiM's own — which
//! Ablehnungs-Entscheidungsbaum a Gas refusal is answered from.
//!
//! The workflow hosts **both** sides of each exchange, and the deployment's
//! Marktrolle selects which commands it issues:
//!
//! - **MSB (invoicer):** `SendInvoic` records the outbound invoice, then awaits
//!   the payer's REMADV (33001–33004) and may refuse it with a COMDIS.
//! - **Payer:** `ReceiveInvoic` ingests it, then `SettleInvoice` /
//!   `DisputeInvoice` returns the REMADV.
//!
//! **31009 belongs exclusively to the WiM domain** and must not be registered
//! by `mako-gpke` — see `GPKE_INVOIC_PIDS` there for the explicit exclusion.
//!
//! # Answer windows
//!
//! Every WiM invoice is answered against the **Zahlungsziel it carries**
//! (`SG8 DTM+265`), never a flat Werktage count from arrival. Where the answer
//! sits relative to that date depends on who pays:
//!
//! | Rechnung | Zahler | Spätester ÜT der Antwort | Fundstelle |
//! |---|---|---|---|
//! | MSB-Rechnung 31009 | NB | **4. WT vor** dem Zahlungsziel | WiM Teil 1 Kap. 6.2 Nr. 2 |
//! | MSB-Rechnung 31009 | LF · ESA | zum Zahlungsziel | Kap. 3.6.3.8.2 Nr. 2/4 |
//! | WiM-Rechnung 31003 | NB · MSBN | zum Zahlungsziel | Kap. 3.7.2 Nr. 2/4 |
//!
//! The MSB's Mitteilung that a refused invoice was correct after all (COMDIS
//! 29001) is due by the **2. WT vor** dem Zahlungsziel (Kap. 6.2 Nr. 3), and
//! the Zahlungsziel itself may not fall short of 10 Werktage after receipt.
//! [`mako_fristen::vorlauf`] holds all four as one table; `makod` registers the
//! process deadline from it.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-22-024 Anlage 2a** — WiM Strom Teil 1, Kap. 3.6.3.8 / 3.7 / 6
//! - **AWH WiM Gas 2.0** — Kap. 4.7 (Abrechnung von Dienstleistungen)
//! - **INVOIC AHB 1.0b** — EDI@Energy invoice message format

use mako_engine::types::Pruefidentifikator;
use mako_invoic::{InvoicFamily, InvoicWorkflow};

// ── PID set ───────────────────────────────────────────────────────────────────

/// WiM billing Prüfidentifikatoren (INVOIC AHB 1.0b), in **both Sparten**.
///
/// | PID | Name | Empfänger | Sparte | Fundstelle |
/// |---|---|---|---|---|
/// | 31009 | MSB-Rechnung | NB · LF · ESA | Strom | GPKE Teil 3, WiM Strom Teil 1/2, AWH Änd. Technik |
/// | 31003 | WiM-Rechnung (Abrechnung von Dienstleistungen im Messwesen) | NB · MSBN | **beide** | WiM Strom Teil 1 Kap. 3.7, AWH WiM Gas 2.0 Kap. 4.7 |
/// | 31004 | Stornorechnung | wie die Ursprungsrechnung | **neutral** | INVOIC AHB §3.1.2 |
///
/// **31003 is not the Gas twin of 31009.** They are different Abrechnungen:
/// 31009 bills the *Messstellenbetrieb* to the NB, LF or ESA and exists only in
/// Strom; 31003 bills the *Dienstleistungen* between the abgebender and the
/// aufnehmender MSB — the temporäre Fortführung, the Geräteübernahme and a
/// Zwischen- oder Kontrollablesung — and exists in both Sparten.
///
/// The Gas Ablehnung splits by **who refuses whose invoice**, not by PID
/// (EBD 4.3 Kap. 14.7) — [`gas_ablehnungs_ebd`] resolves it.
pub const WIM_INVOIC_PIDS: &[u32] = &[31009, 31003, 31004];

/// REMADV PIDs answering a WiM invoice.
///
/// The shared [`mako_invoic::REMADV_PIDS`] set. Settlement is „ganz oder gar
/// nicht" (no Teilzahlung), so 33002/33003/33004 are all Abweisungen and only
/// 33001 confirms.
///
/// 33003/33004 are **Strom-only**: the Gas WiM-Rechnung 31003 is rejected with
/// 33002 alone (REMADV AHB 1.0a; PID-Übersicht 4.0 rows 39780–39910). Inbound
/// REMADV routing is by correlation (RFF+Z13 → the original message reference),
/// so this set governs which PIDs the workflow *accepts*, not routing.
pub const WIM_REMADV_PIDS: &[u32] = mako_invoic::REMADV_PIDS;

/// COMDIS PID for inbound Ablehnung REMADV in WiM (payer role).
pub const WIM_COMDIS_ABLEHNUNG_PID: Pruefidentifikator = mako_invoic::COMDIS_ABLEHNUNG_PID;

/// Workflow key for WiM billing processes.
pub const WORKFLOW_NAME: &str = "wim-invoic";

/// Deadline label for the INVOIC settlement response window.
///
/// The window itself is
/// [`mako_fristen::vorlauf::rechnung_antwort_spaetester_uet`] — it is anchored
/// on the Zahlungsziel the invoice carries and on the payer's Marktrolle, so
/// the workflow labels the deadline and `makod` dates it.
pub const SETTLEMENT_WINDOW_LABEL: &str = "wim-invoic-settlement-deadline";

// ── The family ────────────────────────────────────────────────────────────────

/// The WiM billing family.
pub struct WimInvoic;

impl InvoicFamily for WimInvoic {
    const WORKFLOW_NAME: &'static str = WORKFLOW_NAME;
    const DEADLINE_LABEL: &'static str = SETTLEMENT_WINDOW_LABEL;
    const INVOIC_PIDS: &'static [u32] = WIM_INVOIC_PIDS;
    const SENDS_INVOIC: bool = true;
    const ANSWERS_COMDIS: bool = true;
}

/// The WiM billing workflow (PIDs 31009, 31003, 31004).
pub type WimInvoicWorkflow = InvoicWorkflow<WimInvoic>;

// ── Gas Ablehnung ─────────────────────────────────────────────────────────────

/// Who refused the Gas invoice, and what it invoiced — the pair that picks the
/// Ablehnungs-Entscheidungsbaum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GasAblehnung {
    /// The **NB** refuses a Rechnung that names a Marktlokation — `E_2014`.
    NbRechnung,
    /// The **MSBN** refuses a Rechnung — `E_2015`.
    MsbnRechnung,
    /// The **NB** refuses a Rechnung that names only a Messlokation — `E_2016`.
    NbMesslokationsRechnung,
    /// The **NB** refuses a Stornorechnung — `E_2018`.
    NbStorno,
    /// The **MSBN** refuses a Stornorechnung — `E_2019`.
    MsbnStorno,
}

/// The Gas Ablehnungs-Entscheidungsbaum for a refusal.
///
/// EBD 4.3 Kap. 14.7 splits one INVOIC family across five trees, and the PID is
/// not what tells them apart: `E_2014`/`E_2016` are the NB's, `E_2015` the
/// MSBN's, and the two Storno trees repeat that split. `E_2017`
/// („Nichtzahlungsavis prüfen") has no tree, „da keine Antwort gegeben wird",
/// so a Zahlungsavis carries no `AJT`.
#[must_use]
pub const fn gas_ablehnungs_ebd(ablehnung: GasAblehnung) -> &'static str {
    match ablehnung {
        GasAblehnung::NbRechnung => mako_pruefung::codes::EBD_WIM_RECHNUNG_NB_GAS,
        GasAblehnung::MsbnRechnung => mako_pruefung::codes::EBD_WIM_RECHNUNG_MSBN_GAS,
        GasAblehnung::NbMesslokationsRechnung => mako_pruefung::codes::EBD_WIM_RECHNUNG_MELO_GAS,
        GasAblehnung::NbStorno => mako_pruefung::codes::EBD_WIM_STORNO_GAS,
        GasAblehnung::MsbnStorno => mako_pruefung::codes::EBD_WIM_STORNO_MSBN_GAS,
    }
}

#[cfg(test)]
mod gas_ablehnung_tests {
    use super::*;

    /// Each of the five Gas refusal trees is reachable and publishes codes.
    #[test]
    fn every_gas_ablehnungsbaum_resolves_to_a_published_tree() {
        for a in [
            GasAblehnung::NbRechnung,
            GasAblehnung::MsbnRechnung,
            GasAblehnung::NbMesslokationsRechnung,
            GasAblehnung::NbStorno,
            GasAblehnung::MsbnStorno,
        ] {
            let ebd = gas_ablehnungs_ebd(a);
            let codes = mako_pruefung::codes::CODELISTEN
                .iter()
                .find(|(id, _)| *id == ebd)
                .map(|(_, codes)| *codes)
                .unwrap_or_else(|| panic!("{ebd} is registered in CODELISTEN"));
            assert!(!codes.is_empty(), "{ebd} publishes no codes");
            assert!(
                codes
                    .iter()
                    .all(|c| c.cluster == mako_pruefung::Cluster::Ablehnung),
                "{ebd} must publish Ablehnungscodes only — the Gas Zahlungsavis carries no AJT"
            );
            assert!(
                mako_pruefung::codes::wire_codeliste(ebd, mako_pruefung::Cluster::Ablehnung)
                    .is_some_and(|c| c.starts_with("G_")),
                "{ebd} must name a Gas Codeliste in DE 1082"
            );
        }
    }

    /// The NB's and the MSBN's trees are different trees, even though they
    /// spell the same alphabet.
    #[test]
    fn the_nb_and_the_msbn_refuse_from_different_trees() {
        assert_ne!(
            gas_ablehnungs_ebd(GasAblehnung::NbRechnung),
            gas_ablehnungs_ebd(GasAblehnung::MsbnRechnung)
        );
        assert_ne!(
            gas_ablehnungs_ebd(GasAblehnung::NbStorno),
            gas_ablehnungs_ebd(GasAblehnung::MsbnStorno)
        );
    }

    /// Only the Messlokations-Abrechnung names a Messlokation alone in code 14.
    #[test]
    fn code_14_names_the_marktlokation_except_on_the_melo_abrechnung() {
        let name_of = |a| {
            mako_pruefung::codes::lookup(gas_ablehnungs_ebd(a), "14")
                .expect("code 14 is published")
                .bedeutung
        };
        assert!(name_of(GasAblehnung::NbRechnung).contains("Marktlokation"));
        assert!(!name_of(GasAblehnung::NbMesslokationsRechnung).contains("Marktlokation"));
    }
}
