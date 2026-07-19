//! §14a Fernsteuerbarkeit compliance — SMGW session registry and daily audit worker.
//!
//! ## Legal basis
//!
//! | Law | Obligation |
//! |---|---|
//! | **MsbG §21c** | MSB must ensure continuous CLS reachability for §14a devices |
//! | **MsbG §29 Abs. 3** | TLS certificate renewal ≥ 30 days before expiry |
//! | **BK6-22-300 §4** | §14a Konfigurationsprodukt must be assigned before load control |
//! | **BSI TR-03109-1 §5.3** | CLS channel must be Active before control commands |
//! | **BSI TR-03109-4 §6.3** | MSB must monitor certificate expiry across the fleet |
//! | **§17 MessZV §2** | Communication fault triggers substitute-value obligation |
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
//!         check_session_compliance()    append cls_compliance_log
//!                   │                      │
//!                   ▼                      ▼
//!       de.edmd.cls.compliance_issue   GET /api/v1/smgw/compliance
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
//! | `COMMUNICATION_FAULT` | CRITICAL | No contact > 2h | §17 MessZV substitution + Sonderablesung |
//! | `GATEWAY_REVOKED` | CRITICAL | `status = REVOKED` | Security incident — replace immediately |

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use metering::{CertificateType, SmgwSession};
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
    /// All detected issues (empty when fully compliant).
    pub issues: Vec<ComplianceIssue>,
    /// `true` when any CRITICAL issue was found.
    pub has_critical: bool,
    /// §14a compliance rate: `sessions_with_no_issues / sessions_scanned * 100`.
    pub compliance_pct: f64,
}

/// Request body for `PUT /api/v1/smgw/{malo_id}`.
#[derive(Debug, Deserialize)]
pub struct PutSmgwSessionRequest {
    /// Full `metering::SmgwSession` serialized as JSON.
    pub session: serde_json::Value,
}

// ── Core compliance engine ────────────────────────────────────────────────────

/// Run compliance checks on a single `SmgwSession`.
///
/// Returns an ordered list of issues (CRITICAL first, then WARNING).
/// `cert_warning_days` is configurable (default: 30).
pub fn check_session_compliance(
    session: &SmgwSession,
    today: time::Date,
    cert_warning_days: i32,
    comm_fault_threshold_hours: i64,
) -> Vec<ComplianceIssue> {
    let mut issues = Vec::new();

    // ── 1. Gateway-level status checks ───────────────────────────────────────

    if matches!(session.status, metering::GatewayStatus::Revoked) {
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
                "SMGW {} at MaLo {} status is REVOKED — security incident, replace immediately (MsbG §29)",
                session.device_id, session.malo_id
            ),
        });
    }

    // ── 2. Communication fault ────────────────────────────────────────────────

    if session.is_communication_fault(comm_fault_threshold_hours) {
        let hours = session.hours_since_last_contact();
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
                     — §17 MessZV substitute values required",
                    session.device_id
                ),
                None => format!(
                    "SMGW {} has never been contacted — §17 MessZV substitute values required",
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
                "SMGW {} has no TLS certificate registered — SMGW Admin Protocol unreachable (BSI TR-03109-4)",
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
                         (BSI TR-03109-4 §6.3, MsbG §29)",
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
                        "SMGW {} TLS cert {} expires in {days} days — renew now (BSI TR-03109-4 §6.3 requires \
                         renewal ≥ 30 days before expiry)",
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
                    format!("expired {}", -days)
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
                     — DSO load control impossible (BK6-24-174 §4.3, BK6-22-300)",
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
/// session, log issues to `cls_compliance_log`, and emit
/// `de.edmd.cls.compliance_issue` CloudEvents to the ERP webhook.
///
/// Called by the daily background worker and by
/// `POST /api/v1/smgw/compliance/scan` (on-demand).
///
/// Returns a `ComplianceReport` summarising the scan.
pub async fn run_cls_compliance_sweep(
    pool: &PgPool,
    tenant: &str,
    erp_webhook_url: Option<&str>,
    cert_warning_days: i32,
    comm_fault_threshold_hours: i64,
) -> ComplianceReport {
    let scanned_at = OffsetDateTime::now_utc();
    let today = scanned_at.date();
    let client = mako_service::http::default_client();

    // ── 1. Fetch all sessions for this tenant ─────────────────────────────────
    let rows = match sqlx::query("SELECT malo_id, session FROM smgw_sessions WHERE tenant = $1")
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
                issues: Vec::new(),
                has_critical: false,
                compliance_pct: 100.0,
            };
        }
    };

    let session_count = rows.len();
    let mut all_issues: Vec<ComplianceIssue> = Vec::new();
    let mut sessions_with_issues = 0usize;

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
            today,
            cert_warning_days,
            comm_fault_threshold_hours,
        );
        if issues.is_empty() {
            continue;
        }
        sessions_with_issues += 1;

        for issue in &issues {
            let event_id = Uuid::new_v4().to_string();

            // ── 2. Log to cls_compliance_log ──────────────────────────────────
            let details = serde_json::to_value(issue).ok();
            if let Err(e) = sqlx::query(
                r"INSERT INTO cls_compliance_log
                      (malo_id, device_id, issue_type, severity, cert_serial, cert_type,
                       days_to_expiry, channel_id, details, cloud_event_id, tenant)
                  VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(&issue.malo_id)
            .bind(&issue.device_id)
            .bind(issue.issue_type.db_str())
            .bind(issue.severity)
            .bind(issue.cert_serial.as_deref())
            .bind(issue.cert_type.as_deref())
            .bind(issue.days_to_expiry)
            .bind(issue.channel_id.as_deref())
            .bind(&details)
            .bind(&event_id)
            .bind(tenant)
            .execute(pool)
            .await
            {
                tracing::warn!(error = %e, "edmd: cls-compliance-sweep: failed to log issue");
            }

            // ── 3. Emit de.edmd.cls.compliance_issue CloudEvent ───────────────
            if let Some(url) = erp_webhook_url {
                let ce = serde_json::json!({
                    "specversion": "1.0",
                    "id": event_id,
                    "type": "de.edmd.cls.compliance_issue",
                    "source": format!("urn:edmd:tenant:{}:cls-compliance-worker", tenant),
                    "subject": issue.malo_id,
                    "time": scanned_at.to_string(),
                    "datacontenttype": "application/json",
                    "tenant": tenant,
                    "data": {
                        "malo_id":       issue.malo_id,
                        "device_id":     issue.device_id,
                        "issue_type":    issue.issue_type.db_str(),
                        "severity":      issue.severity,
                        "cert_serial":   issue.cert_serial,
                        "cert_type":     issue.cert_type,
                        "days_to_expiry": issue.days_to_expiry,
                        "channel_id":    issue.channel_id,
                        "description":   issue.description,
                    }
                });

                if let Err(e) = client
                    .post(url)
                    .header("Content-Type", "application/cloudevents+json")
                    .json(&ce)
                    .send()
                    .await
                {
                    tracing::warn!(
                        error = %e,
                        issue_type = issue.issue_type.db_str(),
                        malo_id = %issue.malo_id,
                        "edmd: cls-compliance-sweep: webhook delivery failed"
                    );
                }
            }
        }

        all_issues.extend(issues);
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
            has_critical,
            compliance_pct = format!("{:.1}", compliance_pct),
            "edmd: cls-compliance-sweep: issues detected"
        );
    } else {
        tracing::info!(
            sessions_scanned = session_count,
            "edmd: cls-compliance-sweep: all sessions compliant"
        );
    }

    ComplianceReport {
        scanned_at,
        sessions_scanned: session_count,
        sessions_with_issues,
        issues: all_issues,
        has_critical,
        compliance_pct,
    }
}

/// Spawn the daily CLS compliance background worker.
///
/// Runs at startup and then every `interval_secs` seconds (default: 86400 for daily).
/// Gracefully stops on `shutdown_token` cancellation.
pub fn spawn_cls_compliance_worker(
    pool: Arc<PgPool>,
    tenant: String,
    erp_webhook_url: Option<String>,
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
                cert_warning_days,
                comm_fault_threshold_hours,
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
/// `cls_compliance_log` immediately and CloudEvents emitted.  This makes the first
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
        metering::GatewayStatus::Provisioned => "PROVISIONED",
        metering::GatewayStatus::Commissioned => "COMMISSIONED",
        metering::GatewayStatus::Operational => "OPERATIONAL",
        metering::GatewayStatus::Revoked => "REVOKED",
        metering::GatewayStatus::Replaced => "REPLACED",
        metering::GatewayStatus::CommunicationFault => "COMMUNICATION_FAULT",
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

    // Immediate compliance check on upsert — issues surfaced without delay.
    let today = OffsetDateTime::now_utc().date();
    let issues = check_session_compliance(&session, today, 30, 2);

    for issue in &issues {
        let event_id = Uuid::new_v4().to_string();
        let details = serde_json::to_value(issue).ok();
        let _ = sqlx::query(
            r"INSERT INTO cls_compliance_log
                      (malo_id, device_id, issue_type, severity, cert_serial, cert_type,
                       days_to_expiry, channel_id, details, cloud_event_id, tenant)
                  VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
        )
        .bind(&issue.malo_id)
        .bind(&issue.device_id)
        .bind(issue.issue_type.db_str())
        .bind(issue.severity)
        .bind(issue.cert_serial.as_deref())
        .bind(issue.cert_type.as_deref())
        .bind(issue.days_to_expiry)
        .bind(issue.channel_id.as_deref())
        .bind(&details)
        .bind(&event_id)
        .bind(tenant)
        .execute(pool.as_ref())
        .await;

        if let Some(url) = &state.erp_webhook_url {
            let ce = serde_json::json!({
                "specversion": "1.0",
                "id": event_id,
                "type": "de.edmd.cls.compliance_issue",
                "source": format!("urn:edmd:tenant:{}:smgw-upsert", tenant),
                "subject": issue.malo_id,
                "time": OffsetDateTime::now_utc().to_string(),
                "datacontenttype": "application/json",
                "tenant": tenant,
                "data": {
                    "malo_id":        issue.malo_id,
                    "device_id":      issue.device_id,
                    "issue_type":     issue.issue_type.db_str(),
                    "severity":       issue.severity,
                    "cert_serial":    issue.cert_serial,
                    "cert_type":      issue.cert_type,
                    "days_to_expiry": issue.days_to_expiry,
                    "channel_id":     issue.channel_id,
                    "description":    issue.description,
                }
            });
            let client = mako_service::http::default_client();
            let _ = client
                .post(url)
                .header("Content-Type", "application/cloudevents+json")
                .json(&ce)
                .send()
                .await;
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
/// Returns the stored `SmgwSession` for a MaLo, plus the most recent compliance
/// issues from `cls_compliance_log`.
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

    // Fetch the 10 most recent compliance issues.
    let recent_issues = sqlx::query(
        r"SELECT issue_type, severity, cert_serial, days_to_expiry, channel_id, detected_at
          FROM cls_compliance_log
          WHERE malo_id = $1 AND tenant = $2
          ORDER BY detected_at DESC
          LIMIT 10",
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
            let detected_at: OffsetDateTime = r.get("detected_at");
            serde_json::json!({
                "issue_type":    issue_type,
                "severity":      severity,
                "cert_serial":   cert_serial,
                "days_to_expiry": days_to_expiry,
                "channel_id":    channel_id,
                "detected_at":   detected_at.to_string(),
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
        "recent_issues":   issues,
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

    let rows = sqlx::query(
        r"SELECT s.malo_id, s.device_id, s.gateway_status, s.last_contact_at, s.updated_at,
                 COUNT(c.id) FILTER (WHERE c.severity = 'CRITICAL') AS critical_count,
                 COUNT(c.id) FILTER (WHERE c.severity = 'WARNING')  AS warning_count
          FROM smgw_sessions s
          LEFT JOIN cls_compliance_log c
                 ON c.malo_id = s.malo_id AND c.tenant = s.tenant
                AND c.detected_at >= now() - INTERVAL '24 hours'
          WHERE s.tenant = $1
          GROUP BY s.malo_id, s.device_id, s.gateway_status, s.last_contact_at, s.updated_at
          ORDER BY s.updated_at DESC",
    )
    .bind(tenant)
    .fetch_all(pool.as_ref())
    .await;

    match rows {
        Ok(rows) => {
            let mut items: Vec<serde_json::Value> = rows
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
                        "critical_issues_24h": critical,
                        "warning_issues_24h":  warning,
                    })
                })
                .collect();

            // Apply filters.
            if let Some(ref status) = q.status {
                let s = status.to_uppercase();
                items.retain(|r| r["gateway_status"].as_str() == Some(&s));
            }
            if q.with_issues_only.unwrap_or(false) {
                items.retain(|r| {
                    r["critical_issues_24h"].as_i64().unwrap_or(0) > 0
                        || r["warning_issues_24h"].as_i64().unwrap_or(0) > 0
                });
            }

            Json(serde_json::json!({
                "sessions": items,
                "count": items.len(),
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
/// **Does not write to `cls_compliance_log`** and **does not emit CloudEvents** —
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
    let today = scanned_at.date();

    let rows = match sqlx::query("SELECT malo_id, session FROM smgw_sessions WHERE tenant = $1")
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
            let issues = check_session_compliance(&session, today, 30, 2);
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
/// - Logs all found issues to `cls_compliance_log`
/// - Emits `de.edmd.cls.compliance_issue` CloudEvents for each issue
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
        30,
        2,
    )
    .await;

    Json(report).into_response()
}
