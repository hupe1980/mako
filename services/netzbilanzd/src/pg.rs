//! PostgreSQL persistence for `netzbilanzd`.

use anyhow::Context as _;
use invoic_checker::check::CheckOutcome;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use time::Date;
use uuid::Uuid;

// ── upsert_draft ─────────────────────────────────────────────────────────────

/// A summary row for billing history queries (lighter than DraftRow).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DraftSummaryRow {
    pub id: String,
    pub malo_id: String,
    pub pid: i32,
    pub rechnungsart: String,
    pub status: String,
    pub gross_eur_units: i64,
    pub period_from: Date,
    pub period_to: Date,
    pub dispatch_ref: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// Billing history for a single MaLo — lightweight, no Rechnung JSONB.
pub async fn billing_history_for_malo(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    limit: i64,
) -> anyhow::Result<Vec<DraftSummaryRow>> {
    sqlx::query_as::<_, DraftSummaryRow>(
        r"SELECT id::TEXT, malo_id, pid, rechnungsart, status,
                 gross_eur_units, period_from, period_to, dispatch_ref, created_at
          FROM invoice_drafts
          WHERE tenant = $1 AND malo_id = $2
          ORDER BY created_at DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("billing_history_for_malo")
}

/// Dispatch multiple drafts in a single batch operation.
///
/// Returns `(succeeded, failed)` counts. Each draft is dispatched independently
/// so partial failures don't block the rest.
pub async fn dispatch_batch(
    pool: &PgPool,
    makod: &Arc<mako_markt::makod_client::MakodClient>,
    ids: &[Uuid],
) -> anyhow::Result<(usize, Vec<(Uuid, String)>)> {
    let mut succeeded = 0usize;
    let mut failures: Vec<(Uuid, String)> = Vec::new();
    for &id in ids {
        match approve_and_dispatch(pool, makod, id).await {
            Ok(_) => succeeded += 1,
            Err(e) => failures.push((id, e.to_string())),
        }
    }
    Ok((succeeded, failures))
}

/// Monthly billing summary grouped by PID and status.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BillingSummaryRow {
    pub pid: i32,
    pub status: String,
    pub rechnungsart: String,
    pub count: i64,
    pub total_gross_eur_units: i64,
}

/// Monthly billing totals for the MCP `get_billing_summary` tool.
pub async fn billing_summary(
    pool: &PgPool,
    tenant: &str,
    year: i32,
    month: u8,
) -> anyhow::Result<Vec<BillingSummaryRow>> {
    sqlx::query_as::<_, BillingSummaryRow>(
        r"SELECT pid, status, rechnungsart,
                 COUNT(*) AS count,
                 SUM(gross_eur_units) AS total_gross_eur_units
          FROM invoice_drafts
          WHERE tenant = $1
            AND date_trunc('month', period_from) = make_date($2, $3, 1)
          GROUP BY pid, status, rechnungsart
          ORDER BY pid, status",
    )
    .bind(tenant)
    .bind(year)
    .bind(month as i32)
    .fetch_all(pool)
    .await
    .context("billing_summary")
}

/// List drafts that are still in 'draft' status older than `stale_hours` hours.
/// These are undispatched invoices that may be approaching Zahlungsziel.
pub async fn list_undispatched_stale(
    pool: &PgPool,
    tenant: &str,
    stale_hours: i64,
    limit: i64,
) -> anyhow::Result<Vec<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid, rechnungsart,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, original_draft_id, created_at, updated_at
          FROM invoice_drafts
          WHERE tenant = $1
            AND status = 'draft'
            AND created_at < now() - ($2 * INTERVAL '1 hour')
            AND check_outcome IN ('Ok', 'Warn')
          ORDER BY created_at ASC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(stale_hours)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_undispatched_stale")
}

/// Insert a new invoice draft.  Returns the generated UUID.
///
/// Idempotent on `(tenant, malo_id, period_from, period_to, pid)` for RECHNUNG
/// drafts — re-submitting the same billing run returns the existing draft UUID
/// without creating a duplicate (enforced by partial unique index
/// `id_no_double_billing` in migration 0003).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_draft(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    nb_mp_id: &str,
    lf_mp_id: &str,
    pid: i32,
    period_from: Date,
    period_to: Date,
    rechnung: serde_json::Value,
    total_eur: Decimal,
    check_outcome: CheckOutcome,
) -> anyhow::Result<Uuid> {
    let outcome_str = match check_outcome {
        CheckOutcome::Ok => "Ok",
        CheckOutcome::Warn => "Warn",
        CheckOutcome::Dispute => "Dispute",
    };

    // Store total as integer in units of 10⁻⁵ EUR for lossless round-trip.
    let total_i64: i64 = (total_eur * Decimal::from(100_000_i64))
        .round()
        .to_string()
        .parse()
        .context("total_eur to i64")?;

    // ON CONFLICT on the partial unique index returns the existing draft id
    // so billing runs are idempotent (operator double-click safety).
    let row = sqlx::query(
        r"INSERT INTO invoice_drafts
              (tenant, malo_id, nb_mp_id, lf_mp_id, pid, period_from, period_to,
               rechnung, gross_eur_units, check_outcome, status, rechnungsart)
          VALUES ($10, $1, $2, $3, $4, $5, $6, $7, $8, $9, 'draft', 'RECHNUNG')
          ON CONFLICT (tenant, malo_id, period_from, period_to, pid)
          WHERE rechnungsart = 'RECHNUNG' AND status != 'rejected'
          DO UPDATE SET updated_at = now()
          RETURNING id::TEXT",
    )
    .bind(malo_id)
    .bind(nb_mp_id)
    .bind(lf_mp_id)
    .bind(pid)
    .bind(period_from)
    .bind(period_to)
    .bind(&rechnung)
    .bind(total_i64)
    .bind(outcome_str)
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("insert invoice_draft")?;

    let id_str: String = row.try_get("id")?;
    id_str.parse::<Uuid>().context("parse UUID")
}

// ── list_drafts_pg ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DraftRow {
    pub id: String,
    pub malo_id: String,
    pub nb_mp_id: String,
    pub lf_mp_id: String,
    pub pid: i32,
    pub rechnungsart: String,
    pub period_from: Date,
    pub period_to: Date,
    pub rechnung: serde_json::Value,
    pub gross_eur_units: i64,
    pub check_outcome: Option<String>,
    pub status: String,
    pub dispatch_ref: Option<String>,
    pub reject_reason: Option<String>,
    pub original_draft_id: Option<Uuid>,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

/// List drafts with optional filters.
pub async fn list_drafts_pg(
    pool: &PgPool,
    status: Option<&str>,
    malo_id: Option<&str>,
    nb_mp_id: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid, rechnungsart,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, original_draft_id, created_at, updated_at
          FROM invoice_drafts
          WHERE ($1::TEXT IS NULL OR status = $1)
            AND ($2::TEXT IS NULL OR malo_id = $2)
            AND ($3::TEXT IS NULL OR nb_mp_id = $3)
          ORDER BY created_at DESC
          LIMIT $4",
    )
    .bind(status)
    .bind(malo_id)
    .bind(nb_mp_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_drafts_pg")
}

// ── fetch_draft ───────────────────────────────────────────────────────────────

/// Fetch a single draft by UUID.
pub async fn fetch_draft(pool: &PgPool, id: Uuid) -> anyhow::Result<Option<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid, rechnungsart,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, original_draft_id, created_at, updated_at
          FROM invoice_drafts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch_draft")
}

// ── approve_and_dispatch ─────────────────────────────────────────────────────

/// Validate the draft via `invoic-checker` and dispatch it to `makod` as
/// INVOIC 31001/31002/31005/31009.  Updates status to `dispatched`.
///
/// Blocked when `check_outcome == 'Dispute'` — an NB must never send an INVOIC
/// that fails its own plausibility checks.
pub async fn approve_and_dispatch(
    pool: &PgPool,
    makod: &Arc<mako_markt::makod_client::MakodClient>,
    id: Uuid,
) -> anyhow::Result<String> {
    // Verify draft exists and is in draft state.
    let row = sqlx::query(
        "SELECT status, check_outcome, malo_id, nb_mp_id, lf_mp_id, pid, rechnung
         FROM invoice_drafts WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .context("fetch for dispatch")?;

    let Some(row) = row else {
        anyhow::bail!("draft not found: {id}");
    };

    let status: String = row.try_get("status")?;
    let outcome: Option<String> = row.try_get("check_outcome")?;
    let malo_id: String = row.try_get("malo_id")?;
    let nb_mp_id: String = row.try_get("nb_mp_id")?;
    let lf_mp_id: String = row.try_get("lf_mp_id")?;
    let pid: i32 = row.try_get("pid")?;
    let rechnung: serde_json::Value = row.try_get("rechnung")?;

    if status != "draft" {
        anyhow::bail!("draft {id} is already {status}");
    }
    if outcome.as_deref() == Some("Dispute") {
        anyhow::bail!(
            "cannot dispatch draft {id}: invoic-checker reported Dispute — fix the invoice before dispatching"
        );
    }

    // Map PID to makod command name.
    let command = match pid {
        31001 => "gpke.nne.rechnung.stellen",
        31002 => "gpke.mmm.rechnung.stellen",
        31005 => "gpke.nne-gas.rechnung.stellen",
        31009 => "wim.msb-rechnung.stellen",
        // PID 31011: Rechnung sonstige Leistung (GeLi Gas AWH Sperrprozesse, GNB → LFG).
        // Regulatory basis: BK7-24-01-009 §5.4 — GNB bills LFG for billable actions
        // (Abrechnungswürdige Handlungen) during Sperrprozess.
        31011 => "geli.awh-rechnung.stellen",
        _ => anyhow::bail!("unknown billing PID {pid}"),
    };

    // Dispatch to makod — the engine serialises this as EDIFACT INVOIC.
    let idempotency_key = format!("netzbilanzd-invoic-{id}");
    let cmd = mako_markt::makod_client::ForwardCommand {
        command: command.to_owned(),
        marktrolle: Some("NB".to_owned()),
        malo_id: Some(malo_id.clone()),
        melo_id: None,
        payload: serde_json::json!({
            "lf_mp_id":  lf_mp_id,
            "nb_mp_id":  nb_mp_id,
            "pid":       pid,
            "rechnung":  rechnung,
        }),
    };

    let accepted = makod
        .post_command(&idempotency_key, &cmd)
        .await
        .map_err(|e| anyhow::anyhow!("dispatch to makod failed: {e}"))?;

    let dispatch_ref = accepted.process_id.to_string();

    sqlx::query(
        "UPDATE invoice_drafts SET status = 'dispatched', dispatch_ref = $1, updated_at = now() WHERE id = $2",
    )
    .bind(&dispatch_ref)
    .bind(id)
    .execute(pool)
    .await
    .context("update dispatch")?;

    Ok(dispatch_ref)
}

// ── reject_draft_pg ───────────────────────────────────────────────────────────

/// Reject a draft with an operator-supplied reason.
pub async fn reject_draft_pg(pool: &PgPool, id: Uuid, reason: &str) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        "UPDATE invoice_drafts SET status = 'rejected', reject_reason = $1, updated_at = now() WHERE id = $2 AND status = 'draft'",
    )
    .bind(reason)
    .bind(id)
    .execute(pool)
    .await
    .context("reject_draft")?
    .rows_affected();
    Ok(rows > 0)
}

// ── Kostenblatt (Redispatch 2.0, N4) ─────────────────────────────────────────

/// Stored Kostenblatt record row (migration 0002 + 0004).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct KostenblattRow {
    pub id: Uuid,
    pub tenant: String,
    pub activation_id: String,
    pub tr_id: String,
    pub malo_id: Option<String>,
    pub period_year: i16,
    pub period_month: i16,
    pub uenb_mp_id: String,
    pub vnb_mp_id: String,
    pub dispatch_kwh: Decimal,
    pub arbeitspreis_eur_per_kwh: Decimal,
    pub einsatzkosten_eur: Option<Decimal>,
    pub kosten_json: Option<serde_json::Value>,
    pub status: String,
    pub submitted_at: Option<time::OffsetDateTime>,
    pub dispatch_ref: Option<String>,
    /// UTC start of the activation window (migration 0004).  `None` for records
    /// inserted before 0004 or without an explicit compute request.
    pub activation_start_utc: Option<time::OffsetDateTime>,
    /// UTC end of the activation window (migration 0004).
    pub activation_end_utc: Option<time::OffsetDateTime>,
    /// Data provenance: `"lastgang_sum"` | `"billing_period"` | `"manual_override"`.
    /// `None` for legacy records inserted before migration 0004.
    pub dispatch_source: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

/// Request body for `PUT /api/v1/redispatch/kostenblatt/{activation_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct UpsertKostenblattRequest {
    pub tr_id: String,
    pub malo_id: Option<String>,
    pub period_year: i16,
    pub period_month: i16,
    pub uenb_mp_id: String,
    pub vnb_mp_id: String,
    /// Energy dispatched in kWh (mandatory unless auto-fetched from edmd).
    pub dispatch_kwh: Decimal,
    /// Contract rate EUR/kWh (from Redispatch contract or TechnischeRessource).
    pub arbeitspreis_eur_per_kwh: Decimal,
    /// Full `rubo4e::current::Kosten` JSON for CIM export (optional).
    pub kosten_json: Option<serde_json::Value>,
    /// UTC activation start — stored for re-computation and audit trail.
    pub activation_start_utc: Option<time::OffsetDateTime>,
    /// UTC activation end.
    pub activation_end_utc: Option<time::OffsetDateTime>,
    /// Data provenance: `"lastgang_sum"` | `"billing_period"` | `"manual_override"`.
    pub dispatch_source: Option<String>,
}

pub async fn upsert_kostenblatt(
    pool: &PgPool,
    tenant: &str,
    activation_id: &str,
    req: &UpsertKostenblattRequest,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO kostenblatt_records
              (tenant, activation_id, tr_id, malo_id,
               period_year, period_month, uenb_mp_id, vnb_mp_id,
               dispatch_kwh, arbeitspreis_eur_per_kwh, kosten_json,
               activation_start_utc, activation_end_utc, dispatch_source)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
          ON CONFLICT (tenant, activation_id, tr_id) DO UPDATE
          SET dispatch_kwh             = EXCLUDED.dispatch_kwh,
              arbeitspreis_eur_per_kwh = EXCLUDED.arbeitspreis_eur_per_kwh,
              kosten_json              = COALESCE(EXCLUDED.kosten_json, kostenblatt_records.kosten_json),
              activation_start_utc     = COALESCE(EXCLUDED.activation_start_utc, kostenblatt_records.activation_start_utc),
              activation_end_utc       = COALESCE(EXCLUDED.activation_end_utc, kostenblatt_records.activation_end_utc),
              dispatch_source          = COALESCE(EXCLUDED.dispatch_source, kostenblatt_records.dispatch_source),
              updated_at               = now()
          RETURNING id",
    )
    .bind(tenant)
    .bind(activation_id)
    .bind(&req.tr_id)
    .bind(&req.malo_id)
    .bind(req.period_year)
    .bind(req.period_month)
    .bind(&req.uenb_mp_id)
    .bind(&req.vnb_mp_id)
    .bind(req.dispatch_kwh)
    .bind(req.arbeitspreis_eur_per_kwh)
    .bind(&req.kosten_json)
    .bind(req.activation_start_utc)
    .bind(req.activation_end_utc)
    .bind(&req.dispatch_source)
    .fetch_one(pool)
    .await
    .context("upsert_kostenblatt")?;

    Ok(row.try_get("id")?)
}

pub async fn fetch_kostenblatt(
    pool: &PgPool,
    activation_id: &str,
    tenant: &str,
) -> anyhow::Result<Option<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        "SELECT * FROM kostenblatt_records WHERE activation_id = $1 AND tenant = $2 LIMIT 1",
    )
    .bind(activation_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_kostenblatt")
}

pub async fn list_kostenblatt(
    pool: &PgPool,
    tenant: &str,
    period_year: i16,
    period_month: i16,
    status_filter: Option<&str>,
) -> anyhow::Result<Vec<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        r"SELECT * FROM kostenblatt_records
          WHERE tenant = $1 AND period_year = $2 AND period_month = $3
            AND ($4::text IS NULL OR status = $4)
          ORDER BY created_at DESC",
    )
    .bind(tenant)
    .bind(period_year)
    .bind(period_month)
    .bind(status_filter)
    .fetch_all(pool)
    .await
    .context("list_kostenblatt")
}

pub async fn mark_kostenblatt_submitted(
    pool: &PgPool,
    id: Uuid,
    dispatch_ref: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE kostenblatt_records
         SET status = 'submitted', submitted_at = now(), dispatch_ref = $2, updated_at = now()
         WHERE id = $1",
    )
    .bind(id)
    .bind(dispatch_ref)
    .execute(pool)
    .await
    .context("mark_kostenblatt_submitted")?;
    Ok(())
}

/// Return Kostenblatt records for a month that have no energy data yet.
///
/// A "gap" is a record where `dispatch_kwh = 0` AND `dispatch_source IS NULL` —
/// meaning the energy quantity was never computed from any source (neither
/// Lastgang nor billing-period nor operator override).
///
/// Operators should call
/// `POST /api/v1/redispatch/kostenblatt/{activation_id}/compute`
/// for each gap before the 15th-of-month submission deadline.
pub async fn list_kostenblatt_gaps(
    pool: &PgPool,
    tenant: &str,
    period_year: i16,
    period_month: i16,
) -> anyhow::Result<Vec<KostenblattRow>> {
    sqlx::query_as::<_, KostenblattRow>(
        r"SELECT * FROM kostenblatt_records
          WHERE tenant = $1
            AND period_year = $2
            AND period_month = $3
            AND dispatch_kwh = 0
            AND dispatch_source IS NULL
            AND status = 'pending'
          ORDER BY created_at DESC",
    )
    .bind(tenant)
    .bind(period_year)
    .bind(period_month)
    .fetch_all(pool)
    .await
    .context("list_kostenblatt_gaps")
}

// ── Fremdkosten (§22 MessZV external cost pass-through, BO4E typed) ───────────

/// Stored Fremdkosten record row.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct FremdkostenRow {
    pub id: Uuid,
    pub tenant: String,
    pub draft_id: Uuid,
    pub fremdkosten_json: serde_json::Value,
    pub bezeichnung: Option<String>,
    pub total_eur: Decimal,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Request body for `PUT /api/v1/billing/fremdkosten/{draft_id}`.
#[derive(Debug, serde::Deserialize)]
pub struct UpsertFremdkostenRequest {
    /// Optional human-readable description.
    pub bezeichnung: Option<String>,
    /// Full `rubo4e::current::Fremdkosten` JSON.
    ///
    /// Structure:
    /// ```json
    /// {
    ///   "_typ": "FREMDKOSTEN",
    ///   "summe": [{
    ///     "_typ": "FREMDKOSTENBLOCK",
    ///     "kostenblocksbezeichnung": "ÜNB Ausgleichsenergie",
    ///     "kostenpositionen": [{
    ///       "_typ": "FREMDKOSTENPOSITION",
    ///       "positionsbezeichnung": "Regelenergie Strom",
    ///       "menge": { "_typ": "MENGE", "wert": "100", "einheit": "KWH" },
    ///       "einzelpreis": { "_typ": "PREIS", "wert": "0.05", "einheit": "EUR" },
    ///       "betrag": { "_typ": "BETRAG", "wert": "5.00", "waehrung": "EUR" }
    ///     }]
    ///   }]
    /// }
    /// ```
    pub fremdkosten_json: serde_json::Value,
    /// Pre-computed total EUR (sum of all FremdkostenPosition.betrag.wert).
    /// Must match the positions; used for invoic-checker validation.
    pub total_eur: Decimal,
}

/// Upsert a Fremdkosten record for a draft.  Replaces existing record on conflict.
pub async fn upsert_fremdkosten(
    pool: &PgPool,
    tenant: &str,
    draft_id: Uuid,
    req: &UpsertFremdkostenRequest,
) -> anyhow::Result<Uuid> {
    // Auto-inject _typ: "FREMDKOSTEN" when absent.
    let mut json = req.fremdkosten_json.clone();
    if let Some(obj) = json.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::json!("FREMDKOSTEN"));
    }

    let row = sqlx::query(
        r"INSERT INTO fremdkosten_records
              (tenant, draft_id, fremdkosten_json, bezeichnung, total_eur)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (tenant, draft_id) DO UPDATE
          SET fremdkosten_json = EXCLUDED.fremdkosten_json,
              bezeichnung      = COALESCE(EXCLUDED.bezeichnung, fremdkosten_records.bezeichnung),
              total_eur        = EXCLUDED.total_eur,
              updated_at       = now()
          RETURNING id",
    )
    .bind(tenant)
    .bind(draft_id)
    .bind(&json)
    .bind(&req.bezeichnung)
    .bind(req.total_eur)
    .fetch_one(pool)
    .await
    .context("upsert_fremdkosten")?;

    Ok(row.try_get("id")?)
}

/// Fetch Fremdkosten for a draft.
pub async fn fetch_fremdkosten(
    pool: &PgPool,
    draft_id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<FremdkostenRow>> {
    sqlx::query_as::<_, FremdkostenRow>(
        "SELECT * FROM fremdkosten_records WHERE draft_id = $1 AND tenant = $2 LIMIT 1",
    )
    .bind(draft_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_fremdkosten")
}

/// Delete Fremdkosten for a draft (when a draft is rejected/deleted).
#[allow(dead_code)]
pub async fn delete_fremdkosten(pool: &PgPool, draft_id: Uuid, tenant: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM fremdkosten_records WHERE draft_id = $1 AND tenant = $2")
        .bind(draft_id)
        .bind(tenant)
        .execute(pool)
        .await
        .context("delete_fremdkosten")?;
    Ok(())
}

// ── MCP helper queries ────────────────────────────────────────────────────────

/// Flexible invoice draft listing for MCP tools.
///
/// Filters: `tenant` (mandatory), `malo_id`, `lf_mp_id`, `outcome`, `limit`.
/// Used by `list_nne_drafts` and `list_disputed` MCP tools.
#[allow(dead_code)]
pub async fn list_billing_records(
    pool: &PgPool,
    tenant: &str,
    malo_id: Option<&str>,
    lf_mp_id: Option<&str>,
    outcome: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid, rechnungsart,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, original_draft_id, created_at, updated_at
          FROM invoice_drafts
          WHERE tenant = $5
            AND ($1::TEXT IS NULL OR malo_id = $1)
            AND ($2::TEXT IS NULL OR lf_mp_id = $2)
            AND ($3::TEXT IS NULL OR check_outcome = $3)
          ORDER BY created_at DESC
          LIMIT $4",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(outcome)
    .bind(limit)
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_billing_records")
}

/// Fetch a single invoice draft by UUID with tenant guard.
///
/// Returns `None` when the draft exists but belongs to a different tenant
/// (prevents cross-tenant information leakage).
/// Used by `get_nne_draft` MCP tool.
#[allow(dead_code)]
pub async fn fetch_billing_record(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<DraftRow>> {
    sqlx::query_as::<_, DraftRow>(
        r"SELECT id::TEXT, malo_id, nb_mp_id, lf_mp_id, pid, rechnungsart,
                 period_from, period_to, rechnung,
                 gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, original_draft_id, created_at, updated_at
          FROM invoice_drafts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_billing_record")
}

// ── Korrekturrechnung (MSB 31009 correction, §22 MessZV) ─────────────────────

/// Create a Korrekturrechnung (correction invoice) linked to an original draft.
///
/// The correction carries:
/// - `rechnungsart = "KORREKTURRECHNUNG"` in the Rechnung JSONB
/// - `zusatzAttribute: originalRechnungsnummer` pointing to the original
/// - A fresh UUID and status `'draft'`
///
/// The original draft is **never modified** — corrections always produce new
/// records.  Both original and correction are kept in `invoice_drafts`.
///
/// Used by `POST /api/v1/billing/drafts/{id}/correction` (netzbilanzd NB role).
#[allow(dead_code)]
pub async fn insert_correction_draft(
    pool: &PgPool,
    original_id: Uuid,
    reason: &str,
    amended_rechnung: Option<serde_json::Value>,
) -> anyhow::Result<Uuid> {
    // Load the original draft.
    let row = sqlx::query(
        r"SELECT malo_id, nb_mp_id, lf_mp_id, pid, period_from, period_to,
                 rechnung, gross_eur_units
          FROM invoice_drafts WHERE id = $1",
    )
    .bind(original_id)
    .fetch_optional(pool)
    .await
    .context("load original draft for correction")?
    .ok_or_else(|| anyhow::anyhow!("original draft not found: {original_id}"))?;

    let malo_id: String = row.try_get("malo_id")?;
    let nb_mp_id: String = row.try_get("nb_mp_id")?;
    let lf_mp_id: String = row.try_get("lf_mp_id")?;
    let pid: i32 = row.try_get("pid")?;
    let period_from: Date = row.try_get("period_from")?;
    let period_to: Date = row.try_get("period_to")?;
    let original_rechnung: serde_json::Value = row.try_get("rechnung")?;
    let gross_eur_units: i64 = row.try_get("gross_eur_units")?;

    // Build the correction Rechnung — use amended data or clone original with changed rechnungsart.
    let is_storno = amended_rechnung.is_none();
    let mut correction_rechnung = amended_rechnung.unwrap_or_else(|| original_rechnung.clone());
    // Mark as STORNORECHNUNG when no amendment supplied, KORREKTURRECHNUNG otherwise.
    let rechnungsart = if is_storno {
        "STORNORECHNUNG"
    } else {
        "KORREKTURRECHNUNG"
    };
    if let Some(obj) = correction_rechnung.as_object_mut() {
        obj.insert(
            "rechnungsart".to_owned(),
            serde_json::Value::String(rechnungsart.to_owned()),
        );
        // §22 MessZV: carry originalRechnungsnummer as ZusatzAttribut.
        let original_nr = original_rechnung
            .get("rechnungsnummer")
            .and_then(|v| v.as_str())
            .unwrap_or(&original_id.to_string())
            .to_owned();
        let zusatz = serde_json::json!([{
            "_typ": "ZUSATZ_ATTRIBUT",
            "name": "originalRechnungsnummer",
            "wert": original_nr,
        }, {
            "_typ": "ZUSATZ_ATTRIBUT",
            "name": "korrekturGrund",
            "wert": reason,
        }]);
        obj.insert("zusatzAttribute".to_owned(), zusatz);
    }

    // Negate gross for Storno; keep original for Korrektur.
    let correction_gross = if is_storno {
        -gross_eur_units
    } else {
        gross_eur_units
    };

    let new_row = sqlx::query(
        r"INSERT INTO invoice_drafts
              (tenant, malo_id, nb_mp_id, lf_mp_id, pid, period_from, period_to,
               rechnung, gross_eur_units, check_outcome, status,
               rechnungsart, original_draft_id)
          VALUES ('default', $1, $2, $3, $4, $5, $6, $7, $8, 'Ok', 'draft', $9, $10)
          RETURNING id::TEXT",
    )
    .bind(&malo_id)
    .bind(&nb_mp_id)
    .bind(&lf_mp_id)
    .bind(pid)
    .bind(period_from)
    .bind(period_to)
    .bind(&correction_rechnung)
    .bind(correction_gross)
    .bind(rechnungsart)
    .bind(original_id)
    .fetch_one(pool)
    .await
    .context("insert correction_draft")?;

    let id_str: String = new_row.try_get("id")?;
    id_str.parse::<Uuid>().context("parse correction UUID")
}

// ── REMADV payment lifecycle ──────────────────────────────────────────────────

/// Mark a dispatched invoice draft as paid (REMADV 33001/33003/33004).
///
/// `remadv_ref` is the EDIFACT reference from the REMADV message
/// (stored for BNetzA §22 MessZV 3-year audit trail).
///
/// Returns `Ok(true)` when the update succeeded (draft was in `dispatched` status).
/// Returns `Ok(false)` when the draft does not exist or is not in `dispatched` status.
pub async fn mark_draft_paid(pool: &PgPool, id: Uuid, remadv_ref: &str) -> anyhow::Result<bool> {
    let rows = sqlx::query(
        "UPDATE invoice_drafts
         SET status = 'paid',
             dispatch_ref = COALESCE($2, dispatch_ref),
             updated_at = now()
         WHERE id = $1 AND status = 'dispatched'",
    )
    .bind(id)
    .bind(remadv_ref)
    .execute(pool)
    .await
    .context("mark_draft_paid")?
    .rows_affected();
    Ok(rows > 0)
}

/// Mark a dispatched invoice draft as disputed (REMADV 33002).
///
/// `erc_code` is the EDIFACT ERC reason code from the REMADV (e.g. "Z32", "Z34", "Z35").
/// `reason` is the free-text explanation from the LF.
///
/// Returns `Ok(true)` on success, `Ok(false)` if draft not found or wrong status.
pub async fn mark_draft_disputed(
    pool: &PgPool,
    id: Uuid,
    erc_code: &str,
    reason: &str,
) -> anyhow::Result<bool> {
    let combined_reason = format!("ERC {erc_code}: {reason}");
    let rows = sqlx::query(
        "UPDATE invoice_drafts
         SET status = 'dispatched',
             check_outcome = 'Dispute',
             reject_reason = $2,
             updated_at = now()
         WHERE id = $1 AND status = 'dispatched'",
    )
    .bind(id)
    .bind(&combined_reason)
    .execute(pool)
    .await
    .context("mark_draft_disputed")?
    .rows_affected();
    Ok(rows > 0)
}

// ── BNetzA §22 MessZV audit export ────────────────────────────────────────────

/// Audit export query params.
pub struct AuditQuery {
    pub tenant: String,
    pub from: Option<time::Date>,
    pub to: Option<time::Date>,
    pub pid: Option<i32>,
    pub status: Option<String>,
    pub limit: i64,
}

/// Full audit export row (lightweight — no Rechnung JSONB).
///
/// Satisfies BNetzA §22 MessZV 3-year retention requirement.
/// The full Rechnung JSONB can be fetched separately via `fetch_draft(id)`.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AuditRow {
    pub id: String,
    pub tenant: String,
    pub malo_id: String,
    pub nb_mp_id: String,
    pub lf_mp_id: String,
    pub pid: i32,
    pub rechnungsart: String,
    pub period_from: Date,
    pub period_to: Date,
    pub gross_eur_units: i64,
    pub check_outcome: Option<String>,
    pub status: String,
    pub dispatch_ref: Option<String>,
    pub reject_reason: Option<String>,
    pub bo4e_version: Option<String>,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

/// Export invoice records for BNetzA audit (§22 MessZV 3-year retention).
///
/// Filters by date range, PID, and status.  Does not return `rechnung` JSONB
/// to keep response payload manageable for large portfolios.
pub async fn list_audit(pool: &PgPool, q: AuditQuery) -> anyhow::Result<Vec<AuditRow>> {
    sqlx::query_as::<_, AuditRow>(
        r"SELECT id::TEXT, tenant, malo_id, nb_mp_id, lf_mp_id, pid, rechnungsart,
                 period_from, period_to, gross_eur_units, check_outcome, status,
                 dispatch_ref, reject_reason, bo4e_version, created_at, updated_at
          FROM invoice_drafts
          WHERE tenant = $1
            AND ($2::DATE IS NULL OR period_from >= $2)
            AND ($3::DATE IS NULL OR period_to   <= $3)
            AND ($4::INT  IS NULL OR pid = $4)
            AND ($5::TEXT IS NULL OR status = $5)
          ORDER BY period_from DESC, created_at DESC
          LIMIT $6",
    )
    .bind(&q.tenant)
    .bind(q.from)
    .bind(q.to)
    .bind(q.pid)
    .bind(q.status.as_deref())
    .bind(q.limit)
    .fetch_all(pool)
    .await
    .context("list_audit")
}

/// Payment statistics for ERP reconciliation: totals grouped by status × PID.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PaymentStatsRow {
    pub pid: i32,
    pub status: String,
    pub count: i64,
    pub total_gross_eur_units: i64,
}

/// Payment stats grouped by PID and status — used by `get_payment_stats` MCP tool.
pub async fn payment_stats(
    pool: &PgPool,
    tenant: &str,
    year: i32,
    month: u8,
) -> anyhow::Result<Vec<PaymentStatsRow>> {
    sqlx::query_as::<_, PaymentStatsRow>(
        r"SELECT pid, status,
                 COUNT(*) AS count,
                 COALESCE(SUM(gross_eur_units), 0) AS total_gross_eur_units
          FROM invoice_drafts
          WHERE tenant = $1
            AND date_trunc('month', period_from) = make_date($2, $3, 1)
          GROUP BY pid, status
          ORDER BY pid, status",
    )
    .bind(tenant)
    .bind(year)
    .bind(month as i32)
    .fetch_all(pool)
    .await
    .context("payment_stats")
}
