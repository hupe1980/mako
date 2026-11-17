//! The OpenAPI 3.1 document for `accountingd`, served at
//! `GET /api/v1/openapi.json` and committed as `services/accountingd/openapi.json`.
//!
//! It describes every `/api/v1` route, each mounted through [`api_router`]: a
//! route mounted there cannot exist without its document entry.

use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::handlers;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "accountingd — Customer Account Ledger",
        description = "REST API of the supplier sub-ledger: customer accounts, the Kontokorrent, open items, dunning, SEPA, EEG payouts and payment matching.",
    ),
    tags(
        (name = "accounts", description = "One customer account: balance and Kontokorrent"),
        (name = "books", description = "Tenant-wide books: open items, aging, trial balance, period seals and proofs"),
        (name = "payments", description = "Payment reconciliation"),
        (name = "eeg", description = "EEG payouts to plant operators"),
        (name = "dunning", description = "Dunning cases, Mahnsperren and Forderungseinwände"),
        (name = "sepa", description = "SEPA mandates, collections, reversals and recalls"),
    ),
)]
struct ApiDoc;

/// Every `/api/v1` route and the document describing them.
pub fn api_router() -> (Router, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(handlers::get_balance))
        .routes(routes!(handlers::get_ledger))
        .routes(routes!(handlers::get_offene_posten))
        .routes(routes!(handlers::get_dunning))
        .routes(routes!(handlers::match_payment))
        .routes(routes!(handlers::get_unmatched_payments))
        .routes(routes!(handlers::assign_payment))
        .routes(routes!(handlers::get_credit_payouts))
        .routes(routes!(handlers::post_credit_payout_paid))
        .routes(routes!(handlers::get_eeg_payouts))
        // ── Account endpoints ──────────────────────────────────────────────────
        .routes(routes!(handlers::get_account, handlers::put_account))
        .routes(routes!(handlers::put_account_verbraucher))
        .routes(routes!(handlers::get_kontoauszug))
        .routes(routes!(handlers::put_abschlag))
        // The advances a settling invoice may deduct (§ 14 Abs. 5 Satz 2
        // UStG), with the rate each was raised at. billingd's §40b sweep
        // reads it; without it an automated Jahresrechnung cannot itemise
        // the Abschläge § 40 Abs. 1 EnWG requires it to show.
        .routes(routes!(handlers::get_abschlaege))
        .routes(routes!(handlers::post_buchen))
        // Vorauszahlung — BO4E typed advance-payment schedule (L12 — §40 Abs. 1 EnWG)
        .routes(routes!(
            handlers::get_vorauszahlung,
            handlers::put_vorauszahlung
        ))
        // Zahlungsinformation typed BO4E REST (IBAN + BIC + SEPA, rubo4e::current::Zahlungsinformation)
        .routes(routes!(
            handlers::get_zahlungsinformation,
            handlers::put_zahlungsinformation
        ))
        // ── Payment import ─────────────────────────────────────────────────────
        .routes(routes!(handlers::import_payments))
        .routes(routes!(handlers::import_payments_camt054))
        // camt.053 end-of-day statement — same booking rules, plus the
        // bank's own closing balance for reconciliation.
        .routes(routes!(handlers::import_payments_camt053))
        // camt.052 intraday report — the door for a bank that reports
        // intraday as camt.052 rather than camt.054. Only booked entries
        // post; the provisional ones are reported.
        .routes(routes!(handlers::import_payments_camt052))
        // ── Offene Posten ──────────────────────────────────────────────────────
        .routes(routes!(handlers::put_account_business_partner))
        .routes(routes!(handlers::get_bp_accounts))
        .routes(routes!(handlers::get_bp_balance))
        // ── Dunning ────────────────────────────────────────────────────────────
        .routes(routes!(handlers::escalate_dunning))
        .routes(routes!(handlers::resolve_dunning))
        // §41g Abs. 1 S. 2 — the offer of an Abwendungsvereinbarung, recorded
        // separately from its acceptance: a supplier that never offered one
        // was otherwise indistinguishable from one that offered and was
        // refused, and only the first breaches the statute.
        .routes(routes!(handlers::abwendung_angebot))
        // ── Mahnsperren — every §§41f/41g halt, with a reason and an end ──
        // One endpoint replaces `.../abwendung` and `.../unverhaeltnismaessig`,
        // which set bare timestamps that nothing could lift.
        .routes(routes!(handlers::get_locks, handlers::place_lock))
        .routes(routes!(handlers::get_locks_due_review))
        .routes(routes!(handlers::lift_lock))
        // ── Forderungseinwände (§41f Abs. 3 S. 3–5) ──────────────────────
        // Amounts that stay out of the Verzug calculation. Not halts: they
        // reduce what the threshold is measured against.
        .routes(routes!(handlers::get_einwaende, handlers::place_einwand))
        .routes(routes!(handlers::close_einwand))
        // ── SEPA ───────────────────────────────────────────────────────────────
        .routes(routes!(handlers::post_mandate))
        .routes(routes!(handlers::get_mandate, handlers::delete_mandate))
        .routes(routes!(handlers::run_sepa))
        // EPC 36-month dormancy — mandates that are, or are about to become,
        // uncollectable. Tracked here because no bank tracks it for us.
        .routes(routes!(handlers::get_dormant_mandates))
        // What a collection run collected, and where each entry stands —
        // the list a reversal is chosen from.
        .routes(routes!(handlers::get_collection_entries))
        // pain.002 Payment Status Report — applies to whatever the bank's
        // references point at (pain.001 payouts, pain.008 collections),
        // including the Verification of Payee outcome.
        .routes(routes!(handlers::import_pain002))
        // pain.007 reversal — the creditor giving a settled collection back.
        .routes(routes!(handlers::post_sepa_reversal))
        .routes(routes!(handlers::post_sepa_recall))
        .routes(routes!(handlers::import_camt029))
        // ── §25 EEG 2023 — SEPA Credit Transfer payout pipeline ───────────────
        // EnFG Belastungsausgleich (§ 13 Abs. 1 EnFG): the payments made per
        // billing month, and the Aufrechnungen that count as made (Satz 2).
        .routes(routes!(handlers::get_enfg_zahlungen))
        .routes(routes!(handlers::post_eeg_aufrechnung))
        .routes(routes!(handlers::post_eeg_rueckzahlung))
        .routes(routes!(handlers::get_eeg_rueckforderungen))
        .routes(routes!(handlers::post_run_eeg_payouts))
        .routes(routes!(handlers::get_eeg_payout))
        .routes(routes!(handlers::put_eeg_payout_status))
        .routes(routes!(handlers::post_jahresabschluss))
        // ── Balance reconciliation ──────────────────────────────────────
        // Detects and optionally corrects balance_ct cache drift.
        .routes(routes!(handlers::post_reconcile))
        // ── Open-item management ─────────────────────────────────────────
        // FIFO-cleared list of unpaid/partially-paid invoice debits.
        .routes(routes!(handlers::get_open_items))
        // ── GDPR Art. 17 anonymization ──────────────────────────────────
        // Pseudonymizes PII while preserving ledger records (§238 HGB).
        .routes(routes!(handlers::post_anonymize))
        // ── Aging analysis: overdue receivables by age bucket ─────────────
        .routes(routes!(handlers::get_aging))
        // ── Festschreibung (period seals) + audit proofs — GoBD / § 146 AO ──
        .routes(routes!(handlers::post_seal_period))
        .routes(routes!(handlers::get_seals))
        // ── Parked bookings ─────────────────────────────────────────────
        .routes(routes!(handlers::post_rebook))
        .routes(routes!(handlers::post_discard))
        .routes(routes!(handlers::get_entry_proof))
        // Proves a customer's closing balance for a sealed period, not just
        // that a booking exists — the question a Betriebsprüfung asks.
        .routes(routes!(handlers::get_period_balance_proof))
        // Append-only evidence for an auditor holding an archived head.
        .routes(routes!(handlers::get_consistency_proof))
        // ── Summen- und Saldenliste + Zahlungszuordnung (open-item clearing) ──
        .routes(routes!(handlers::get_trial_balance))
        .routes(routes!(handlers::post_clear))
        .routes(routes!(handlers::post_reset_clearing))
        // ── Verzugszinsen §288 BGB ─────────────────────────────────────────
        .routes(routes!(
            handlers::get_interest_charges,
            handlers::post_interest_charge
        ))
        .routes(routes!(handlers::place_account_lock))
        // ── Payment plans (Zahlungsvereinbarung) ───────────────────────────
        .routes(routes!(
            handlers::get_payment_plans,
            handlers::post_payment_plan
        ))
        .routes(routes!(
            handlers::get_payment_plan,
            handlers::delete_payment_plan
        ))
        .split_for_parts()
}

/// The document as served.
#[must_use]
pub fn document() -> serde_json::Value {
    serde_json::to_value(api_router().1).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    /// The committed `openapi.json` is the document the service serves;
    /// `BLESS_MCP_SNAPSHOT=1` rewrites it.
    #[test]
    fn the_openapi_document_matches_the_committed_one() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
        mako_service::openapi::assert_committed(&super::api_router().1, &path);
    }
}
