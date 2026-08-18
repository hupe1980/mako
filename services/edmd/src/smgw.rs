//! §14a Fernsteuerbarkeit compliance — SMGW session registry and daily audit worker.
//!
//! ## Legal basis
//!
//! | Anchor | Obligation |
//! |---|---|
//! | **§ 25 MsbG** | The Smart-Meter-Gateway-Administrator is responsible for *installation, commissioning, configuration, administration, **monitoring and maintenance*** of the intelligent metering system, and must report security deficiencies to the BSI without delay. Every check in this module discharges part of that duty. |
//! | **§ 22 MsbG** | Mindestanforderungen an das Smart-Meter-Gateway *durch Schutzprofile und Technische Richtlinien* — what gives BSI TR-03109 its legal force. |
//! | **§ 24 MsbG** | Zertifizierung des Smart-Meter-Gateway. |
//! | **§ 28 MsbG** | Inhaber der Wurzelzertifikate — the SM-PKI trust root the gateway certificates chain to. |
//! | **BSI TR-03109-1** | SMGW architecture, incl. the CLS channel a control command travels over. |
//! | **BSI TR-03109-4** | SM-PKI: certificate runtimes are binding here; the renewal lead time and the overlap ("Zertifikatswechsel") window are fixed by the Root-CP. |
//! | **BK6-22-300** (27.11.2023, in force 01.01.2024) | §14a EnWG netzorientierte Steuerung — the Konfigurationsprodukt a CLS channel needs before a DSO may control the load. |
//! | **§ 60 Abs. 2 MsbG** | Plausibilisierung und Ersatzwertbildung — what a silent gateway leaves owing. |
//!
//! ### Citations this module must not carry
//!
//! Four earlier ones were wrong, and each pointed a reader somewhere real but
//! irrelevant, which is worse than pointing nowhere:
//!
//! - **§ 21c MsbG does not exist.** The MsbG runs § 21 → § 22.
//! - **§ 29 MsbG** is *Ausstattung von Messstellen mit intelligenten Messsystemen*
//!   — the rollout obligation and its 2032 deadlines. It says nothing about
//!   certificates, and there is no "Abs. 3" renewal rule in it.
//! - **BK6-24-174** is GPKE. The §14a Konfigurationsprodukt is BK6-22-300.
//! - **"BSI TR-03109-4 §6.3 requires renewal ≥ 30 days before expiry"** — the TR
//!   binds certificate *runtimes*; the lead times live in the Root-CP. The
//!   90/30/7-day ladder below is an **operational** warning schedule this service
//!   chooses, not a statutory deadline, and it is configurable for that reason.
//!
//! ## Architecture
//!
//! ```text
//! MSB ERP/GWA ──PUT /api/v1/smgw/{malo_id}──► edmd SmgwSession store (JSONB)
//!                                                │
//!                              ┌─────────────────┘
//!                              │   Daily worker (05:00 UTC, configurable)
//!                              ▼
//!                        run_cls_compliance_sweep()
//!                              │
//!                   ┌──────────┴───────────┐
//!                   ▼                      ▼
//!         check_session_compliance()    upsert cls_compliance_issues
//!                   │                      │
//!                   ▼                      ▼
//!       de.messwert.cls.compliance-issue   GET /api/v1/smgw/compliance
//!       CloudEvent (ERP webhook)       (on-demand status endpoint)
//! ```
//!
//! ## Compliance issue types
//!
//! | Type | Severity | Trigger | §14a impact |
//! |---|---|---|---|
//! | `CERT_EXPIRED` | CRITICAL | TLS cert past `valid_to` | SMGW unreachable, §14a lost |
//! | `CERT_EXPIRING` | WARNING | TLS cert expiry ≤ 30 days | Renewal required |
//! | `TLS_CERT_MISSING` | CRITICAL | No TLS cert in session | SMGW Admin Protocol broken |
//! | `CLS_NOT_COMPLIANT` | WARNING | Active channel, no Konfigurationsprodukt | DSO control impossible |
//! | `COMMUNICATION_FAULT` | CRITICAL | No contact > 2h | § 60 Abs. 2 MsbG substitution + Sonderablesung |
//! | `GATEWAY_REVOKED` | CRITICAL | `status = REVOKED` | Security incident — replace immediately |
//!
//! ## Certificate-expiry advance warning
//!
//! Alongside the compliance sweep, a second daily worker
//! ([`run_smgw_cert_expiry_sweep`]) emits a tiered
//! `de.messwert.smgw.cert.expiry-warning` at **90 / 30 / 7 days** before each
//! certificate's `valid_to` (`SMGW_CERT_ABLAUFDATUM`), once per tier per
//! certificate (dedup in `smgw_cert_expiry_alerts`; a renewed cert with a new
//! `valid_to` gets a fresh set). This is the *advance* warning — an
//! already-expired cert is the `CERT_EXPIRED` compliance issue above. BSI
//! The ladder is operational, not statutory — see the note above. An expired
//! cert silently ends §14a Fernsteuerbarkeit.

use std::sync::Arc;

use crate::smgw_model::{CertificateType, SmgwSession};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::handler::HandlerState;

// ── Domain types ─────────────────────────────────────────────────────────────

/// Type of detected compliance violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComplianceIssueType {
    /// TLS certificate has passed its `valid_to` date.
    CertExpired,
    /// TLS certificate will expire within the warning window (default: 30 days).
    CertExpiring,
    /// No TLS certificate registered for this gateway.
    TlsCertMissing,
    /// CLS channel is Active but has no §14a Konfigurationsprodukt assigned.
    ClsNotCompliant,
    /// Gateway has not been heard from in more than the fault threshold (default: 2h).
    CommunicationFault,
    /// Gateway status is `REVOKED` — security incident.
    GatewayRevoked,
}

impl ComplianceIssueType {
    fn severity(self) -> &'static str {
        match self {
            Self::CertExpired
            | Self::TlsCertMissing
            | Self::CommunicationFault
            | Self::GatewayRevoked => "CRITICAL",
            Self::CertExpiring | Self::ClsNotCompliant => "WARNING",
        }
    }

    fn db_str(self) -> &'static str {
        match self {
            Self::CertExpired => "CERT_EXPIRED",
            Self::CertExpiring => "CERT_EXPIRING",
            Self::TlsCertMissing => "TLS_CERT_MISSING",
            Self::ClsNotCompliant => "CLS_NOT_COMPLIANT",
            Self::CommunicationFault => "COMMUNICATION_FAULT",
            Self::GatewayRevoked => "GATEWAY_REVOKED",
        }
    }
}

/// A single detected compliance issue for a SMGW session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceIssue {
    pub malo_id: String,
    pub device_id: String,
    pub issue_type: ComplianceIssueType,
    pub severity: &'static str,
    /// Applicable certificate serial number (for `CERT_*` issues).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_serial: Option<String>,
    /// Certificate type (for `CERT_*` issues): `"TLS"`, `"SIG"`, `"ENC"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cert_type: Option<String>,
    /// Days until expiry — negative when already expired (for `CERT_*` issues).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub days_to_expiry: Option<i32>,
    /// CLS channel ID (for `CLS_NOT_COMPLIANT`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    /// Human-readable description.
    pub description: String,
}

/// Response for `GET /api/v1/smgw/compliance`.
#[derive(Debug, Serialize)]
pub struct ComplianceReport {
    /// Scan timestamp (UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub scanned_at: OffsetDateTime,
    /// Total number of SMGW sessions scanned.
    pub sessions_scanned: usize,
    /// Number of sessions with at least one issue.
    pub sessions_with_issues: usize,
    /// Issues that were newly opened (or reopened) by this sweep — the ones that
    /// emitted a CloudEvent. A standing fault counts once, when it starts.
    pub newly_opened: usize,
    /// Issues this sweep no longer found and therefore closed.
    pub resolved: usize,
    /// All detected issues (empty when fully compliant).
    pub issues: Vec<ComplianceIssue>,
    /// `true` when any CRITICAL issue was found.
    pub has_critical: bool,
    /// §14a compliance rate: `sessions_with_no_issues / sessions_scanned * 100`.
    pub compliance_pct: f64,
}

// ── Core compliance engine ────────────────────────────────────────────────────

/// Run compliance checks on a single `SmgwSession`.
///
/// Returns an ordered list of issues (CRITICAL first, then WARNING).
/// `cert_warning_days` is configurable (default: 30).
pub fn check_session_compliance(
    session: &SmgwSession,
    now: OffsetDateTime,
    cert_warning_days: i32,
    comm_fault_threshold_hours: i64,
) -> Vec<ComplianceIssue> {
    // One evaluation instant for the whole check, supplied by the caller. The
    // certificate rules work on the date, the communication-fault rule on the
    // instant, and they must agree: reading the clock again inside the function
    // — as the fault branch used to — made the result depend on when it was
    // called rather than on what it was asked about, and left that branch
    // untestable.
    let today = now.date();
    let mut issues = Vec::new();

    // ── 1. Gateway-level status checks ───────────────────────────────────────

    if matches!(session.status, crate::smgw_model::GatewayStatus::Revoked) {
        issues.push(ComplianceIssue {
            malo_id: session.malo_id.clone(),
            device_id: session.device_id.clone(),
            issue_type: ComplianceIssueType::GatewayRevoked,
            severity: ComplianceIssueType::GatewayRevoked.severity(),
            cert_serial: None,
            cert_type: None,
            days_to_expiry: None,
            channel_id: None,
            description: format!(
                "SMGW {} at MaLo {} status is REVOKED — security incident, replace immediately \
                 (§ 25 MsbG: the GWA must report security deficiencies to the BSI without delay)",
                session.device_id, session.malo_id
            ),
        });
    }

    // ── 2. Communication fault ────────────────────────────────────────────────

    if session.is_communication_fault(now, comm_fault_threshold_hours) {
        let hours = session.hours_since_last_contact(now);
        issues.push(ComplianceIssue {
            malo_id: session.malo_id.clone(),
            device_id: session.device_id.clone(),
            issue_type: ComplianceIssueType::CommunicationFault,
            severity: ComplianceIssueType::CommunicationFault.severity(),
            cert_serial: None,
            cert_type: None,
            days_to_expiry: None,
            channel_id: None,
            description: match hours {
                Some(h) => format!(
                    "SMGW {} no contact for {h}h (threshold: {comm_fault_threshold_hours}h) \
                     — § 60 Abs. 2 MsbG substitute values required",
                    session.device_id
                ),
                None => format!(
                    "SMGW {} has never been contacted — § 60 Abs. 2 MsbG substitute values required",
                    session.device_id
                ),
            },
        });
    }

    // ── 3. TLS certificate checks ─────────────────────────────────────────────

    let tls_certs: Vec<_> = session
        .certificates
        .iter()
        .filter(|c| matches!(c.cert_type, CertificateType::Tls))
        .collect();

    if tls_certs.is_empty() {
        issues.push(ComplianceIssue {
            malo_id: session.malo_id.clone(),
            device_id: session.device_id.clone(),
            issue_type: ComplianceIssueType::TlsCertMissing,
            severity: ComplianceIssueType::TlsCertMissing.severity(),
            cert_serial: None,
            cert_type: Some("TLS".to_owned()),
            days_to_expiry: None,
            channel_id: None,
            description: format!(
                "SMGW {} has no TLS certificate registered — SMGW Admin Protocol unreachable \
                         (BSI TR-03109-4; § 25 MsbG administration duty)",
                session.device_id
            ),
        });
    } else {
        // Check each TLS cert (there should normally be one active + possibly one pending renewal).
        for cert in &tls_certs {
            let days = cert.days_to_expiry(today);
            if !cert.is_valid(today) {
                // Expired or revoked.
                issues.push(ComplianceIssue {
                    malo_id: session.malo_id.clone(),
                    device_id: session.device_id.clone(),
                    issue_type: ComplianceIssueType::CertExpired,
                    severity: ComplianceIssueType::CertExpired.severity(),
                    cert_serial: Some(cert.serial_number.clone()),
                    cert_type: Some("TLS".to_owned()),
                    days_to_expiry: Some(days),
                    channel_id: None,
                    description: format!(
                        "SMGW {} TLS cert {} expired {} days ago — §14a eligibility lost \
                         (BSI TR-03109-4 SM-PKI; § 25 MsbG)",
                        session.device_id, cert.serial_number, -days
                    ),
                });
            } else if cert.is_expiring_soon(today, cert_warning_days) {
                issues.push(ComplianceIssue {
                    malo_id: session.malo_id.clone(),
                    device_id: session.device_id.clone(),
                    issue_type: ComplianceIssueType::CertExpiring,
                    severity: ComplianceIssueType::CertExpiring.severity(),
                    cert_serial: Some(cert.serial_number.clone()),
                    cert_type: Some("TLS".to_owned()),
                    days_to_expiry: Some(days),
                    channel_id: None,
                    description: format!(
                        "SMGW {} TLS cert {} expires in {days} days — renew before the \
                         Root-CP lead time elapses (BSI TR-03109-4 SM-PKI Zertifikatswechsel)",
                        session.device_id, cert.serial_number
                    ),
                });
            }
        }
    }

    // ── 4. Non-TLS certificate expiry warnings ────────────────────────────────

    for cert in session.expiring_certificates(today, cert_warning_days) {
        if matches!(cert.cert_type, CertificateType::Tls) {
            continue; // already handled above
        }
        let cert_type_str = match cert.cert_type {
            CertificateType::Sig => "SIG",
            CertificateType::Enc => "ENC",
            CertificateType::KeyAgreement => "KEY_AGREEMENT",
            CertificateType::Tls => unreachable!(),
        };
        let days = cert.days_to_expiry(today);
        let issue_type = if days <= 0 {
            ComplianceIssueType::CertExpired
        } else {
            ComplianceIssueType::CertExpiring
        };
        issues.push(ComplianceIssue {
            malo_id: session.malo_id.clone(),
            device_id: session.device_id.clone(),
            issue_type,
            severity: issue_type.severity(),
            cert_serial: Some(cert.serial_number.clone()),
            cert_type: Some(cert_type_str.to_owned()),
            days_to_expiry: Some(days),
            channel_id: None,
            description: format!(
                "SMGW {} {cert_type_str} cert {} {}",
                session.device_id,
                cert.serial_number,
                if days <= 0 {
                    format!("expired {} days ago", -days)
                } else {
                    format!("expires in {days} days")
                }
            ),
        });
    }

    // ── 5. CLS channel §14a Konfigurationsprodukt check ──────────────────────

    for channel in &session.cls_channels {
        if channel.is_active() && !channel.is_section_14a_compliant() {
            issues.push(ComplianceIssue {
                malo_id: session.malo_id.clone(),
                device_id: session.device_id.clone(),
                issue_type: ComplianceIssueType::ClsNotCompliant,
                severity: ComplianceIssueType::ClsNotCompliant.severity(),
                cert_serial: None,
                cert_type: None,
                days_to_expiry: None,
                channel_id: Some(channel.channel_id.clone()),
                description: format!(
                    "CLS channel {} on SMGW {} is Active but has no §14a Konfigurationsprodukt \
                     — DSO load control impossible (BK6-22-300)",
                    channel.channel_id, session.device_id
                ),
            });
        }
    }

    // Sort: CRITICAL first, then WARNING; stable within each severity group.
    issues.sort_by_key(|i| if i.severity == "CRITICAL" { 0u8 } else { 1u8 });

    issues
}

// ── Background worker ─────────────────────────────────────────────────────────

/// Run a full fleet compliance sweep: query all `smgw_sessions`, check each
/// session, reconcile them against `cls_compliance_issues`, and emit
/// `de.messwert.cls.compliance-issue` CloudEvents to the ERP webhook.
///
/// Called by the daily background worker and by
/// `POST /api/v1/smgw/compliance/scan` (on-demand).
///
/// Returns a `ComplianceReport` summarising the scan.
pub async fn run_cls_compliance_sweep(
    pool: &PgPool,
    tenant: &str,
    erp_webhook_url: Option<&str>,
    erp_webhook_secret: Option<&str>,
    cert_warning_days: i32,
    comm_fault_threshold_hours: i64,
) -> ComplianceReport {
    let scanned_at = OffsetDateTime::now_utc();
    let client = mako_service::http::default_client();

    // The watermark that decides which rows this sweep did *not* re-sight must
    // come from the **database** clock, because `last_seen_at` does: the upsert
    // writes `now()`, which is Postgres's transaction time. Comparing that
    // against an application `OffsetDateTime::now_utc()` makes the sweep depend
    // on the skew between two machines — and when the database is even slightly
    // behind, every row is closed in the same sweep that re-sighted it, so a
    // standing fault flaps resolved/reopened forever and re-emits both events
    // each time. Reading the clock we are going to compare against removes the
    // question.
    let sweep_start: OffsetDateTime = match sqlx::query_scalar("SELECT now()").fetch_one(pool).await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, tenant, "edmd: cls-compliance-sweep: cannot read the DB clock");
            return ComplianceReport {
                scanned_at,
                sessions_scanned: 0,
                sessions_with_issues: 0,
                newly_opened: 0,
                resolved: 0,
                issues: Vec::new(),
                has_critical: false,
                compliance_pct: 100.0,
            };
        }
    };

    // ── 1. Fetch the tenant's live gateways ───────────────────────────────────
    // `REPLACED` is excluded: the gateway is a historical record, physically
    // swapped out. Scanning it reported an expired certificate on a device that
    // no longer exists, every day, forever — and the promoted `gateway_status`
    // column exists precisely so this filter is an index lookup rather than a
    // JSONB extraction. The column was promoted for it and the filter was never
    // written.
    let rows = match sqlx::query(
        "SELECT malo_id, session FROM smgw_sessions
          WHERE tenant = $1 AND gateway_status <> 'REPLACED'",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, tenant, "edmd: cls-compliance-sweep: DB error fetching sessions");
            return ComplianceReport {
                scanned_at,
                sessions_scanned: 0,
                sessions_with_issues: 0,
                newly_opened: 0,
                resolved: 0,
                issues: Vec::new(),
                has_critical: false,
                compliance_pct: 100.0,
            };
        }
    };

    let session_count = rows.len();
    let mut all_issues: Vec<ComplianceIssue> = Vec::new();
    let mut sessions_with_issues = 0usize;
    let mut newly_opened = 0usize;

    for row in rows {
        let malo_id: String = row.get("malo_id");
        let session_val: serde_json::Value = row.get("session");
        let session: SmgwSession = match serde_json::from_value(session_val) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    malo_id = %malo_id,
                    "edmd: cls-compliance-sweep: cannot deserialise SmgwSession — skip"
                );
                continue;
            }
        };

        let issues = check_session_compliance(
            &session,
            scanned_at,
            cert_warning_days,
            comm_fault_threshold_hours,
        );
        if issues.is_empty() {
            continue;
        }
        sessions_with_issues += 1;

        for issue in &issues {
            let event_id = Uuid::new_v4().to_string();

            // ── 2. Open or re-sight the issue in the register ─────────────────
            // `first_detected_at = last_seen_at` is true exactly when this row
            // was inserted now or reopened now — the two transitions worth an
            // event. A plain re-sighting keeps its older `first_detected_at` and
            // stays quiet, which is what stops a standing fault from emitting
            // one CloudEvent per gateway per day for as long as it lasts.
            let details = serde_json::to_value(issue).ok();
            let transition: Option<bool> = match sqlx::query_scalar(
                r"INSERT INTO cls_compliance_issues
                      (tenant, device_id, issue_type, cert_serial, channel_id,
                       malo_id, severity, cert_type, days_to_expiry, details, cloud_event_id)
                  VALUES ($1,$2,$3,COALESCE($4,''),COALESCE($5,''),$6,$7,$8,$9,$10,$11)
                  ON CONFLICT (tenant, device_id, issue_type, cert_serial, channel_id)
                  DO UPDATE SET
                      last_seen_at   = now(),
                      malo_id        = EXCLUDED.malo_id,
                      severity       = EXCLUDED.severity,
                      cert_type      = EXCLUDED.cert_type,
                      days_to_expiry = EXCLUDED.days_to_expiry,
                      details        = EXCLUDED.details,
                      -- A recurrence restarts the clock, so the age of an
                      -- issue is the age of the current episode.
                      first_detected_at = CASE
                          WHEN cls_compliance_issues.resolved_at IS NOT NULL THEN now()
                          ELSE cls_compliance_issues.first_detected_at END,
                      cloud_event_id = CASE
                          WHEN cls_compliance_issues.resolved_at IS NOT NULL
                          THEN EXCLUDED.cloud_event_id
                          ELSE cls_compliance_issues.cloud_event_id END,
                      resolved_at    = NULL
                  RETURNING first_detected_at = last_seen_at",
            )
            .bind(tenant)
            .bind(&issue.device_id)
            .bind(issue.issue_type.db_str())
            .bind(issue.cert_serial.as_deref())
            .bind(issue.channel_id.as_deref())
            .bind(&issue.malo_id)
            .bind(issue.severity)
            .bind(issue.cert_type.as_deref())
            .bind(issue.days_to_expiry)
            .bind(&details)
            .bind(&event_id)
            .fetch_one(pool)
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: cls-compliance-sweep: failed to register issue");
                    continue;
                }
            };

            if transition != Some(true) {
                continue; // already open and already reported
            }
            newly_opened += 1;

            // ── 3. Emit de.messwert.cls.compliance-issue CloudEvent ───────────────
            if let Some(url) = erp_webhook_url {
                let ce = mako_service::CloudEvent::new(
                    mako_service::source("edmd", tenant),
                    mako_events::messwert::CLS_COMPLIANCE_ISSUE,
                    issue.malo_id.clone(),
                    serde_json::json!({
                        "malo_id":       issue.malo_id,
                        "device_id":     issue.device_id,
                        "issue_type":    issue.issue_type.db_str(),
                        "severity":      issue.severity,
                        "cert_serial":   issue.cert_serial,
                        "cert_type":     issue.cert_type,
                        "days_to_expiry": issue.days_to_expiry,
                        "channel_id":    issue.channel_id,
                        "description":   issue.description,
                    }),
                )
                .with_id(event_id)
                .extension("tenantid", tenant)
                // CLS_COMPLIANCE_ISSUE is also emitted by the SMGW upsert path;
                // `worker` disambiguates the emitting worker (type/subject alone
                // do not identify it).
                .extension("worker", "cls-compliance-worker");

                // A lost warning silently runs a gateway into an expired
                // certificate and out of §14a eligibility, so this retries like
                // every other edmd compliance event.
                if let Err(e) = mako_service::post_ce_with_retry(
                    &client,
                    url,
                    &ce,
                    erp_webhook_secret.map(str::as_bytes),
                )
                .await
                {
                    tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
                }
            }
        }

        all_issues.extend(issues);
    }

    // ── 4. Close what this sweep no longer found ──────────────────────────────
    // The sweep visited every live gateway of the tenant, so any row still open
    // but not re-sighted is fixed (or its gateway is gone). Without this the
    // register would only ever grow and "what is broken now" would stay
    // unanswerable — the question the whole table exists to answer.
    let resolved = sqlx::query(
        r"UPDATE cls_compliance_issues
             SET resolved_at = now()
           WHERE tenant = $1 AND resolved_at IS NULL AND last_seen_at < $2
       RETURNING device_id, issue_type, malo_id, cert_serial, channel_id",
    )
    .bind(tenant)
    .bind(sweep_start)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for row in &resolved {
        let malo_id: String = row.get("malo_id");
        let device_id: String = row.get("device_id");
        let issue_type: String = row.get("issue_type");
        tracing::info!(
            %malo_id, %device_id, %issue_type,
            "edmd: cls-compliance-sweep: issue resolved"
        );
        // A resolution is as actionable as an occurrence: it is what closes a
        // ticket the earlier event opened.
        if let Some(url) = erp_webhook_url {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("edmd", tenant),
                mako_events::messwert::CLS_COMPLIANCE_RESOLVED,
                malo_id.clone(),
                serde_json::json!({
                    "malo_id":     malo_id,
                    "device_id":   device_id,
                    "issue_type":  issue_type,
                    "cert_serial": row.get::<String, _>("cert_serial"),
                    "channel_id":  row.get::<String, _>("channel_id"),
                    "resolved_at": scanned_at.to_string(),
                }),
            )
            .extension("tenantid", tenant)
            .extension("worker", "cls-compliance-worker");
            if let Err(e) = mako_service::post_ce_with_retry(
                &client,
                url,
                &ce,
                erp_webhook_secret.map(str::as_bytes),
            )
            .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
    }

    let has_critical = all_issues.iter().any(|i| i.severity == "CRITICAL");
    let compliance_pct = if session_count == 0 {
        100.0
    } else {
        let compliant = session_count.saturating_sub(sessions_with_issues);
        (compliant as f64 / session_count as f64) * 100.0
    };

    if !all_issues.is_empty() {
        tracing::warn!(
            sessions_scanned = session_count,
            issues = all_issues.len(),
            newly_opened,
            resolved = resolved.len(),
            has_critical,
            compliance_pct = format!("{:.1}", compliance_pct),
            "edmd: cls-compliance-sweep: issues detected"
        );
    } else {
        tracing::info!(
            sessions_scanned = session_count,
            resolved = resolved.len(),
            "edmd: cls-compliance-sweep: all sessions compliant"
        );
    }

    ComplianceReport {
        scanned_at,
        sessions_scanned: session_count,
        sessions_with_issues,
        newly_opened,
        resolved: resolved.len(),
        issues: all_issues,
        has_critical,
        compliance_pct,
    }
}

/// Spawn the daily CLS compliance background worker.
///
/// Runs at startup and then every `interval_secs` seconds (default: 86400 for daily).
/// Gracefully stops on `shutdown_token` cancellation.
#[allow(clippy::too_many_arguments)]
pub fn spawn_cls_compliance_worker(
    pool: Arc<PgPool>,
    tenant: String,
    erp_webhook_url: Option<String>,
    erp_webhook_secret: Option<String>,
    cert_warning_days: i32,
    comm_fault_threshold_hours: i64,
    interval_secs: u64,
    shutdown_token: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        // Initial delay: wait 30s after startup so the DB pool is fully warmed.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_token.cancelled() => {
                    tracing::info!("edmd: cls-compliance-worker: shutdown requested");
                    break;
                }
            }

            tracing::info!(
                tenant = %tenant,
                cert_warning_days,
                "edmd: cls-compliance-worker: starting sweep"
            );

            run_cls_compliance_sweep(
                &pool,
                &tenant,
                erp_webhook_url.as_deref(),
                erp_webhook_secret.as_deref(),
                cert_warning_days,
                comm_fault_threshold_hours,
            )
            .await;
        }
    });
}

// ── SMGW certificate expiry alerting (BSI TR-03109-4 SM-PKI) ─────────────────

/// Advance-warning tiers (days before `valid_to`), most-urgent first.
///
/// These are an **operational** ladder, not a statutory one: BSI TR-03109-4 binds
/// certificate runtimes and the Root-CP fixes the renewal lead time and the
/// Zertifikatswechsel overlap window, neither of which is a flat 30 days. 90 days
/// is an early planning notice (INFO), 30 the point at which renewal should be
/// under way (WARNING), and 7 imminent §14a loss (CRITICAL).
pub const CERT_EXPIRY_TIERS: [i32; 3] = [7, 30, 90];

/// The most urgent expiry tier a certificate with `days` remaining has reached,
/// or `None` when it is further out than the widest tier (90 days). Returns the
/// smallest threshold `≥ days`, so a cert aging past each tier alerts at that tier.
fn most_urgent_cert_tier(days: i32) -> Option<i32> {
    CERT_EXPIRY_TIERS.iter().copied().find(|&t| days <= t)
}

/// Severity for a given tier — see [`CERT_EXPIRY_TIERS`].
fn cert_tier_severity(threshold_days: i32) -> &'static str {
    match threshold_days {
        7 => "CRITICAL",
        30 => "WARNING",
        _ => "INFO",
    }
}

/// Canonical string for a certificate type (matches the `cert_type` values stored).
fn cert_type_str(t: CertificateType) -> &'static str {
    match t {
        CertificateType::Tls => "TLS",
        CertificateType::Sig => "SIG",
        CertificateType::Enc => "ENC",
        CertificateType::KeyAgreement => "KEY_AGREEMENT",
    }
}

/// Summary of one certificate-expiry sweep.
#[derive(Debug, Clone)]
pub struct CertExpirySweepReport {
    /// When the sweep ran.
    pub scanned_at: OffsetDateTime,
    /// SMGW sessions examined.
    pub sessions_scanned: usize,
    /// Non-revoked certificates examined.
    pub certs_scanned: usize,
    /// `de.messwert.smgw.cert.expiry-warning` events emitted this sweep.
    pub warnings_emitted: usize,
}

/// Sweep every SMGW certificate and emit a tiered
/// `de.messwert.smgw.cert.expiry-warning` at the 90 / 30 / 7-day marks
/// (`SMGW_CERT_ABLAUFDATUM` = `GatewayCertificate::valid_to`).
///
/// Idempotent: `smgw_cert_expiry_alerts` records one row per
/// (cert, `valid_to`, tier), so each tier fires **exactly once** as a certificate
/// ages. A renewed certificate (new `valid_to`) gets a fresh set of alerts.
/// Already-expired certificates are left to the CLS compliance sweep
/// (`CERT_EXPIRED`) — this worker is the *advance* warning.
pub async fn run_smgw_cert_expiry_sweep(
    pool: &PgPool,
    tenant: &str,
    erp_webhook_url: Option<&str>,
    erp_webhook_secret: Option<&str>,
) -> CertExpirySweepReport {
    let scanned_at = OffsetDateTime::now_utc();
    let today = scanned_at.date();
    let client = mako_service::http::default_client();

    let rows = match sqlx::query("SELECT malo_id, session FROM smgw_sessions WHERE tenant = $1")
        .bind(tenant)
        .fetch_all(pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, tenant, "edmd: cert-expiry-sweep: DB error fetching sessions");
            return CertExpirySweepReport {
                scanned_at,
                sessions_scanned: 0,
                certs_scanned: 0,
                warnings_emitted: 0,
            };
        }
    };

    let sessions_scanned = rows.len();
    let mut certs_scanned = 0usize;
    let mut warnings_emitted = 0usize;

    for row in rows {
        let malo_id: String = row.get("malo_id");
        let session_val: serde_json::Value = row.get("session");
        let session: SmgwSession = match serde_json::from_value(session_val) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, malo_id = %malo_id, "edmd: cert-expiry-sweep: cannot deserialise SmgwSession — skip");
                continue;
            }
        };

        for cert in &session.certificates {
            if cert.is_revoked {
                continue;
            }
            certs_scanned += 1;
            let days = cert.days_to_expiry(today);
            // Advance warning only; an expired cert is a CERT_EXPIRED compliance
            // issue handled by the CLS sweep.
            if days <= 0 {
                continue;
            }
            // Most urgent tier the cert has reached (smallest threshold ≥ days).
            let Some(tier) = most_urgent_cert_tier(days) else {
                continue;
            };

            let severity = cert_tier_severity(tier);
            let cert_type = cert_type_str(cert.cert_type);
            let event_id = Uuid::new_v4().to_string();

            // Claim this tier. The INSERT succeeds only the first time the cert
            // reaches `tier`; a conflict means we already alerted this tier.
            let claimed = sqlx::query(
                r"INSERT INTO smgw_cert_expiry_alerts
                      (tenant, device_id, cert_serial, cert_type, valid_to, threshold_days,
                       days_to_expiry, severity, emitted, malo_id, cloud_event_id)
                  VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                  ON CONFLICT (tenant, device_id, cert_serial, valid_to, threshold_days) DO NOTHING",
            )
            .bind(tenant)
            .bind(&session.device_id)
            .bind(&cert.serial_number)
            .bind(cert_type)
            .bind(cert.valid_to)
            .bind(tier as i16)
            .bind(days)
            .bind(severity)
            .bind(erp_webhook_url.is_some())
            .bind(&malo_id)
            .bind(&event_id)
            .execute(pool)
            .await;

            match claimed {
                Ok(res) if res.rows_affected() == 0 => continue, // already alerted this tier
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, device_id = %session.device_id, "edmd: cert-expiry-sweep: dedup insert failed");
                    continue;
                }
            }

            tracing::warn!(
                malo_id = %malo_id,
                device_id = %session.device_id,
                cert_serial = %cert.serial_number,
                cert_type,
                days_to_expiry = days,
                threshold_days = tier,
                severity,
                "edmd: SMGW certificate expiry warning",
            );

            if let Some(url) = erp_webhook_url {
                let ce = mako_service::CloudEvent::new(
                    mako_service::source("edmd", tenant),
                    mako_events::messwert::SMGW_CERT_EXPIRY_WARNING,
                    malo_id.clone(),
                    serde_json::json!({
                        "malo_id":        malo_id,
                        "device_id":      session.device_id,
                        "msb_mp_id":      session.msb_mp_id,
                        "cert_serial":    cert.serial_number,
                        "cert_type":      cert_type,
                        "valid_to":       cert.valid_to.to_string(),
                        "days_to_expiry": days,
                        "threshold_days": tier,
                        "severity":       severity,
                    }),
                )
                .with_id(event_id)
                .extension("tenantid", tenant)
                .extension("worker", "smgw-cert-expiry-worker");
                // A lost warning silently runs a gateway into an expired
                // certificate and out of §14a eligibility, so retry like every
                // other edmd compliance event.
                if let Err(e) = mako_service::post_ce_with_retry(
                    &client,
                    url,
                    &ce,
                    erp_webhook_secret.map(str::as_bytes),
                )
                .await
                {
                    tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
                }
            }
            warnings_emitted += 1;
        }
    }

    if warnings_emitted > 0 {
        tracing::warn!(
            sessions_scanned,
            certs_scanned,
            warnings_emitted,
            "edmd: cert-expiry-sweep: certificate expiry warnings emitted"
        );
    } else {
        tracing::info!(
            sessions_scanned,
            certs_scanned,
            "edmd: cert-expiry-sweep: no certificates near expiry"
        );
    }

    CertExpirySweepReport {
        scanned_at,
        sessions_scanned,
        certs_scanned,
        warnings_emitted,
    }
}

/// Spawn the daily SMGW certificate-expiry background worker.
///
/// Runs shortly after startup, then every `interval_secs` (default daily).
/// Stops on `shutdown_token` cancellation.
pub fn spawn_smgw_cert_expiry_worker(
    pool: Arc<PgPool>,
    tenant: String,
    erp_webhook_url: Option<String>,
    erp_webhook_secret: Option<String>,
    interval_secs: u64,
    shutdown_token: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        // Slight offset from the CLS sweep so the two daily sweeps don't contend.
        tokio::time::sleep(std::time::Duration::from_secs(45)).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_token.cancelled() => {
                    tracing::info!("edmd: smgw-cert-expiry-worker: shutdown requested");
                    break;
                }
            }
            run_smgw_cert_expiry_sweep(
                &pool,
                &tenant,
                erp_webhook_url.as_deref(),
                erp_webhook_secret.as_deref(),
            )
            .await;
        }
    });
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// `PUT /api/v1/smgw/{malo_id}`
///
/// Register or update a `SmgwSession` for a MaLo.
///
/// The full `metering::SmgwSession` is stored as JSONB.  Callers are typically:
/// - MSB GWA (Gateway-Administrator) system after a BSI TR-03109-4 Admin session
/// - `marktd` `de.markt.geraet.konfiguration.updated` webhook handler (automated sync)
///
/// Triggers a synchronous compliance check.  Detected issues are logged to
/// `cls_compliance_issues` immediately and CloudEvents emitted.  This makes the first
/// compliance check available within seconds of gateway registration, without waiting
/// for the daily sweep.
///
/// ## `gateway_status` extraction
///
/// The promoted `gateway_status` column is extracted from `session.status` so the
/// compliance sweep can pre-filter `WHERE gateway_status != 'REPLACED'` efficiently.
pub async fn put_smgw_session(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<serde_json::Value>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-meter-reads", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Parse and validate the session payload.
    let session: SmgwSession = match serde_json::from_value(req.clone()) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid SmgwSession: {e}") })),
            )
                .into_response();
        }
    };

    let gateway_status = match session.status {
        crate::smgw_model::GatewayStatus::Provisioned => "PROVISIONED",
        crate::smgw_model::GatewayStatus::Commissioned => "COMMISSIONED",
        crate::smgw_model::GatewayStatus::Operational => "OPERATIONAL",
        crate::smgw_model::GatewayStatus::Revoked => "REVOKED",
        crate::smgw_model::GatewayStatus::Replaced => "REPLACED",
        crate::smgw_model::GatewayStatus::CommunicationFault => "COMMUNICATION_FAULT",
    };

    let last_contact = session.last_contact_at;
    let device_id = session.device_id.clone();
    let msb_mp_id = session.msb_mp_id.clone();

    let session_json = match serde_json::to_value(&session) {
        Ok(v) => v,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    if let Err(e) = sqlx::query(
        r"INSERT INTO smgw_sessions
              (malo_id, tenant, device_id, msb_mp_id, gateway_status, session, last_contact_at, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, now())
          ON CONFLICT (malo_id, tenant) DO UPDATE
          SET device_id       = EXCLUDED.device_id,
              msb_mp_id       = EXCLUDED.msb_mp_id,
              gateway_status  = EXCLUDED.gateway_status,
              session         = EXCLUDED.session,
              last_contact_at = EXCLUDED.last_contact_at,
              updated_at      = now()",
    )
    .bind(&malo_id)
    .bind(tenant)
    .bind(&device_id)
    .bind(&msb_mp_id)
    .bind(gateway_status)
    .bind(&session_json)
    .bind(last_contact)
    .execute(pool.as_ref())
    .await
    {
        tracing::warn!(error = %e, malo_id, "edmd: put_smgw_session: DB error");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Immediate compliance check on upsert, so a newly registered gateway does
    // not wait a day for its first verdict. It goes through the same register as
    // the sweep and emits on the same transition rule — a GWA that re-syncs its
    // fleet hourly would otherwise re-announce every standing fault hourly.
    let issues = check_session_compliance(
        &session,
        OffsetDateTime::now_utc(),
        state.smgw.cert_warning_days,
        state.smgw.comm_fault_threshold_hours,
    );

    for issue in &issues {
        let event_id = Uuid::new_v4().to_string();
        let details = serde_json::to_value(issue).ok();
        let transition: Option<bool> = sqlx::query_scalar(
            r"INSERT INTO cls_compliance_issues
                  (tenant, device_id, issue_type, cert_serial, channel_id,
                   malo_id, severity, cert_type, days_to_expiry, details, cloud_event_id)
              VALUES ($1,$2,$3,COALESCE($4,''),COALESCE($5,''),$6,$7,$8,$9,$10,$11)
              ON CONFLICT (tenant, device_id, issue_type, cert_serial, channel_id)
              DO UPDATE SET
                  last_seen_at   = now(),
                  malo_id        = EXCLUDED.malo_id,
                  severity       = EXCLUDED.severity,
                  cert_type      = EXCLUDED.cert_type,
                  days_to_expiry = EXCLUDED.days_to_expiry,
                  details        = EXCLUDED.details,
                  first_detected_at = CASE
                      WHEN cls_compliance_issues.resolved_at IS NOT NULL THEN now()
                      ELSE cls_compliance_issues.first_detected_at END,
                  resolved_at    = NULL
              RETURNING first_detected_at = last_seen_at",
        )
        .bind(tenant)
        .bind(&issue.device_id)
        .bind(issue.issue_type.db_str())
        .bind(issue.cert_serial.as_deref())
        .bind(issue.channel_id.as_deref())
        .bind(&issue.malo_id)
        .bind(issue.severity)
        .bind(issue.cert_type.as_deref())
        .bind(issue.days_to_expiry)
        .bind(&details)
        .bind(&event_id)
        .fetch_one(pool.as_ref())
        .await
        .unwrap_or(Some(false));

        if transition != Some(true) {
            continue;
        }

        if let Some(url) = &state.erp_webhook_url {
            let ce = mako_service::CloudEvent::new(
                mako_service::source("edmd", tenant),
                mako_events::messwert::CLS_COMPLIANCE_ISSUE,
                issue.malo_id.clone(),
                serde_json::json!({
                    "malo_id":        issue.malo_id,
                    "device_id":      issue.device_id,
                    "issue_type":     issue.issue_type.db_str(),
                    "severity":       issue.severity,
                    "cert_serial":    issue.cert_serial,
                    "cert_type":      issue.cert_type,
                    "days_to_expiry": issue.days_to_expiry,
                    "channel_id":     issue.channel_id,
                    "description":    issue.description,
                }),
            )
            .with_id(event_id)
            .extension("tenantid", tenant)
            // CLS_COMPLIANCE_ISSUE is also emitted by the compliance sweep worker;
            // `worker` disambiguates the emitting path (type/subject alone do not).
            .extension("worker", "smgw-upsert");
            let client = mako_service::http::default_client();
            if let Err(e) =
                mako_service::post_ce_with_retry(&client, url, &ce, state.webhook_secret_bytes())
                    .await
            {
                tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
            }
        }
    }

    if issues.is_empty() {
        StatusCode::NO_CONTENT.into_response()
    } else {
        // Return 200 with the detected issues so callers know immediately.
        Json(serde_json::json!({
            "status": "accepted_with_compliance_issues",
            "issues": issues,
        }))
        .into_response()
    }
}

/// `GET /api/v1/smgw/{malo_id}`
///
/// Returns the stored `SmgwSession` for a MaLo, plus its **open** compliance
/// issues from `cls_compliance_issues` — what is wrong now, oldest first.
pub async fn get_smgw_session(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let row = match sqlx::query(
        "SELECT malo_id, device_id, gateway_status, session, last_contact_at, updated_at \
         FROM smgw_sessions WHERE malo_id = $1 AND tenant = $2",
    )
    .bind(&malo_id)
    .bind(tenant)
    .fetch_optional(pool.as_ref())
    .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let Some(row) = row else {
        return (
            StatusCode::NOT_FOUND,
            format!("SmgwSession for MaLo {malo_id} not found"),
        )
            .into_response();
    };

    let device_id: String = row.get("device_id");
    let gateway_status: String = row.get("gateway_status");
    let session: serde_json::Value = row.get("session");
    let last_contact_at: Option<OffsetDateTime> = row.try_get("last_contact_at").unwrap_or(None);
    let updated_at: OffsetDateTime = row.get("updated_at");

    // What is wrong with this gateway *now*, oldest episode first. This used to
    // return "the ten most recent log rows", which — with one row written per
    // issue per daily sweep — was ten copies of yesterday's single problem.
    let recent_issues = sqlx::query(
        r"SELECT issue_type, severity, cert_serial, days_to_expiry, channel_id,
                 first_detected_at, last_seen_at
          FROM cls_compliance_issues
          WHERE malo_id = $1 AND tenant = $2 AND resolved_at IS NULL
          ORDER BY first_detected_at ASC
          LIMIT 50",
    )
    .bind(&malo_id)
    .bind(tenant)
    .fetch_all(pool.as_ref())
    .await
    .unwrap_or_default();

    let issues: Vec<serde_json::Value> = recent_issues
        .into_iter()
        .map(|r| {
            let issue_type: String = r.get("issue_type");
            let severity: String = r.get("severity");
            let cert_serial: Option<String> = r.try_get("cert_serial").unwrap_or(None);
            let days_to_expiry: Option<i32> = r.try_get("days_to_expiry").unwrap_or(None);
            let channel_id: Option<String> = r.try_get("channel_id").unwrap_or(None);
            let first_detected_at: OffsetDateTime = r.get("first_detected_at");
            let last_seen_at: OffsetDateTime = r.get("last_seen_at");
            serde_json::json!({
                "issue_type":        issue_type,
                "severity":          severity,
                "cert_serial":       cert_serial,
                "days_to_expiry":    days_to_expiry,
                "channel_id":        channel_id,
                "first_detected_at": first_detected_at.to_string(),
                "last_seen_at":      last_seen_at.to_string(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "malo_id":         malo_id,
        "device_id":       device_id,
        "gateway_status":  gateway_status,
        "session":         session,
        "last_contact_at": last_contact_at.map(|t| t.to_string()),
        "updated_at":      updated_at.to_string(),
        "open_issues":     issues,
    }))
    .into_response()
}

/// Query parameters for `GET /api/v1/smgw`.
#[derive(Debug, Deserialize)]
pub struct ListSmgwQuery {
    /// Filter by gateway status.  Defaults to all statuses.
    pub status: Option<String>,
    /// When `true`, return only sessions with open compliance issues.
    pub with_issues_only: Option<bool>,
    /// Max sessions to return (default 500, hard cap 5000).
    pub limit: Option<i64>,
}

/// `GET /api/v1/smgw`
///
/// List all SMGW sessions for the tenant with their current compliance status.
/// Returns sessions with the most recently updated first.
pub async fn list_smgw_sessions(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
    Query(q): Query<ListSmgwQuery>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Filters are pushed into SQL and the result is bounded. They used to be
    // applied with `Vec::retain` *after* fetching every session the tenant owns —
    // a fleet-sized scan and materialisation on every dashboard poll, with
    // `?status=` narrowing only the JSON that came back, not the work done.
    let limit = q.limit.unwrap_or(500).clamp(1, 5_000);
    let rows = sqlx::query(
        r"SELECT s.malo_id, s.device_id, s.gateway_status, s.last_contact_at, s.updated_at,
                 COUNT(c.*) FILTER (WHERE c.severity = 'CRITICAL') AS critical_count,
                 COUNT(c.*) FILTER (WHERE c.severity = 'WARNING')  AS warning_count
          FROM smgw_sessions s
          LEFT JOIN cls_compliance_issues c
                 ON c.device_id = s.device_id AND c.tenant = s.tenant
                AND c.resolved_at IS NULL
          WHERE s.tenant = $1
            AND ($2::text IS NULL OR s.gateway_status = $2)
          GROUP BY s.malo_id, s.device_id, s.gateway_status, s.last_contact_at, s.updated_at
          HAVING NOT $3::bool OR COUNT(c.*) > 0
          ORDER BY s.updated_at DESC
          LIMIT $4",
    )
    .bind(tenant)
    .bind(q.status.as_ref().map(|s| s.to_uppercase()))
    .bind(q.with_issues_only.unwrap_or(false))
    .bind(limit)
    .fetch_all(pool.as_ref())
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|r| {
                    let malo_id: String = r.get("malo_id");
                    let device_id: String = r.get("device_id");
                    let gateway_status: String = r.get("gateway_status");
                    let last_contact_at: Option<OffsetDateTime> =
                        r.try_get("last_contact_at").unwrap_or(None);
                    let updated_at: OffsetDateTime = r.get("updated_at");
                    let critical: i64 = r.try_get::<i64, _>("critical_count").unwrap_or(0);
                    let warning: i64 = r.try_get::<i64, _>("warning_count").unwrap_or(0);
                    serde_json::json!({
                        "malo_id":         malo_id,
                        "device_id":       device_id,
                        "gateway_status":  gateway_status,
                        "last_contact_at": last_contact_at.map(|t| t.to_string()),
                        "updated_at":      updated_at.to_string(),
                        // Issues **open right now**. This was "rows logged in the
                        // last 24 h", which — with the log written once per sweep
                        // — measured the sweep cadence, not the fleet.
                        "open_critical_issues": critical,
                        "open_warning_issues":  warning,
                    })
                })
                .collect();

            Json(serde_json::json!({
                "sessions":  items,
                "count":     items.len(),
                "truncated": i64::try_from(items.len()).unwrap_or(i64::MAX) >= limit,
                "limit":     limit,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/smgw/compliance`
///
/// Run an on-demand compliance scan across all SMGW sessions.
///
/// This is equivalent to triggering the background worker's sweep logic
/// synchronously.  The response is a `ComplianceReport` with all detected issues.
///
/// **Does not write to `cls_compliance_issues`** and **does not emit CloudEvents** —
/// it is a read-only audit endpoint for dashboards and the `smgw-diagnostics-agent`.
/// Use `POST /api/v1/smgw/compliance/scan` for a side-effecting full sweep.
pub async fn get_smgw_compliance(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let scanned_at = OffsetDateTime::now_utc();

    let rows = match sqlx::query(
        "SELECT malo_id, session FROM smgw_sessions
          WHERE tenant = $1 AND gateway_status <> 'REPLACED'",
    )
    .bind(tenant)
    .fetch_all(pool.as_ref())
    .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let session_count = rows.len();
    let mut all_issues = Vec::new();
    let mut sessions_with_issues = 0;

    for row in rows {
        if let Ok(session) = serde_json::from_value::<SmgwSession>(row.get("session")) {
            let issues = check_session_compliance(
                &session,
                scanned_at,
                state.smgw.cert_warning_days,
                state.smgw.comm_fault_threshold_hours,
            );
            if !issues.is_empty() {
                sessions_with_issues += 1;
                all_issues.extend(issues);
            }
        }
    }

    let has_critical = all_issues.iter().any(|i| i.severity == "CRITICAL");
    let compliance_pct = if session_count == 0 {
        100.0
    } else {
        let compliant = session_count.saturating_sub(sessions_with_issues);
        (compliant as f64 / session_count as f64) * 100.0
    };

    Json(ComplianceReport {
        scanned_at,
        sessions_scanned: session_count,
        sessions_with_issues,
        // A read-only audit opens and closes nothing.
        newly_opened: 0,
        resolved: 0,
        issues: all_issues,
        has_critical,
        compliance_pct,
    })
    .into_response()
}

/// `POST /api/v1/smgw/compliance/scan`
///
/// Trigger an immediate, side-effecting compliance sweep:
/// - Runs `run_cls_compliance_sweep()` synchronously
/// - Reconciles all found issues against `cls_compliance_issues`
/// - Emits `de.messwert.cls.compliance-issue` CloudEvents for each issue
///
/// Use this endpoint for manual compliance audits or integration tests.
/// The daily background worker calls the same logic automatically.
pub async fn post_smgw_compliance_scan(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-meter-reads", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let report = run_cls_compliance_sweep(
        pool.as_ref(),
        tenant,
        state.erp_webhook_url.as_deref(),
        state.erp_webhook_secret.as_ref().map(|s| {
            use secrecy::ExposeSecret;
            s.expose_secret()
        }),
        state.smgw.cert_warning_days,
        state.smgw.comm_fault_threshold_hours,
    )
    .await;

    Json(report).into_response()
}

#[cfg(test)]
mod cert_expiry_tests {
    use super::{cert_tier_severity, cert_type_str, most_urgent_cert_tier};
    use crate::smgw_model::CertificateType;

    #[test]
    fn tier_is_the_smallest_threshold_reached() {
        assert_eq!(most_urgent_cert_tier(120), None, "beyond the widest tier");
        assert_eq!(most_urgent_cert_tier(91), None);
        assert_eq!(most_urgent_cert_tier(90), Some(90));
        assert_eq!(most_urgent_cert_tier(45), Some(90));
        assert_eq!(most_urgent_cert_tier(31), Some(90));
        assert_eq!(most_urgent_cert_tier(30), Some(30));
        assert_eq!(most_urgent_cert_tier(8), Some(30));
        assert_eq!(most_urgent_cert_tier(7), Some(7));
        assert_eq!(
            most_urgent_cert_tier(1),
            Some(7),
            "imminent expiry → tightest tier"
        );
    }

    #[test]
    fn severity_escalates_with_urgency() {
        assert_eq!(cert_tier_severity(90), "INFO");
        assert_eq!(cert_tier_severity(30), "WARNING");
        assert_eq!(cert_tier_severity(7), "CRITICAL");
    }

    #[test]
    fn cert_type_strings_match_db_check_list() {
        assert_eq!(cert_type_str(CertificateType::Tls), "TLS");
        assert_eq!(cert_type_str(CertificateType::Sig), "SIG");
        assert_eq!(cert_type_str(CertificateType::Enc), "ENC");
        assert_eq!(
            cert_type_str(CertificateType::KeyAgreement),
            "KEY_AGREEMENT"
        );
    }
}
