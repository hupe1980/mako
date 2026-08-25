//! Jahresablesung campaign scheduler and compliance report (§ 40b Abs. 1 EnWG).

#[allow(unused_imports)]
use super::*;

// ── Jahresablesung campaign (N7 — § 40b Abs. 1 EnWG) ───────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct JahresablesungCampaignRequest {
    /// NB MP-ID (BDEW-Codenummer) — used to filter MaLos in the NB's grid area.
    pub nb_mp_id: String,
    /// Campaign year (defaults to current year).
    pub campaign_year: Option<i32>,
    /// Target reading date (YYYY-MM-DD).  Defaults to December 31 of campaign_year.
    pub geplant_am: Option<time::Date>,
    /// Latest acceptable reading date.  Defaults to January 31 of campaign_year+1.
    pub ausfuehrt_bis: Option<time::Date>,
    /// MSB MP-ID responsible for executing the reading.
    /// If absent, the grundzuständiger MSB per MaLo is used.
    pub ausfuehrender_msb: Option<String>,
    /// Maximum number of MaLos to process in one request (default 5000, max 50000).
    pub max_malos: Option<i64>,
}

/// `POST /api/v1/reading-orders/campaign`
///
/// **Jahresablesung campaign scheduler (§ 40b Abs. 1 EnWG).**
///
/// Creates bulk `JAHRESABLESUNG` reading orders for all SLP MaLos in the NB's
/// grid area that have not yet been scheduled for reading this campaign year.
///
/// ## Pipeline
///
/// 1. Query `marktd GET /api/v1/malos?bilanzierungsmethode=SLP&size=500` (paginated)
///    to enumerate SLP MaLos in the NB's grid area.
/// 2. For each MaLo: check `ablese_auftraege` — skip those already having an
///    OFFEN/BEAUFTRAGT/AUSGEFUEHRT `JAHRESABLESUNG` for this year.
/// 3. Insert `ablese_auftraege` rows with:
///    - `anlass = JAHRESABLESUNG`
///    - `auftraggeber_rolle = NB`
///    - `geplant_am = December 31 of campaign_year`
///    - `ausfuehrt_bis = January 31 of campaign_year+1`
/// 4. Return campaign summary.
///
/// ## § 40b Abs. 1 EnWG
///
/// NB is obligated to ensure annual SLP meter reading.  Unread SLP meters →
/// estimated settlement → potential Mehr-/Mindermengendisputes with the LF.
/// This endpoint enables a single-click annual reading campaign without ERP
/// integration.
///
/// ## Idempotency
///
/// Re-running for the same NB + year is safe — already-scheduled MaLos are
/// counted in `skipped` and not double-scheduled.
pub(crate) async fn jahresablesung_campaign(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<JahresablesungCampaignRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match run_jahresablesung_campaign(
        state.repo.pool(),
        &state.tenant,
        &state.marktd_url,
        &state.marktd_api_key,
        &req,
    )
    .await
    {
        Ok(outcome) => (StatusCode::CREATED, Json(outcome.into_json(&req))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// What a campaign run did.
pub struct CampaignOutcome {
    /// SLP MaLos found in the NB's grid area.
    pub total_malos: usize,
    /// Reading orders created.
    pub created: usize,
    /// MaLos that already had an order for this campaign year.
    pub skipped: usize,
    /// Campaign year the orders were dated in.
    pub year: i32,
    /// Planned reading date.
    pub geplant_am: time::Date,
    /// Latest acceptable reading date.
    pub ausfuehrt_bis: time::Date,
}

impl CampaignOutcome {
    fn into_json(self, req: &JahresablesungCampaignRequest) -> serde_json::Value {
        serde_json::json!({
            "nb_mp_id": req.nb_mp_id,
            "campaign_year": self.year,
            "geplant_am": self.geplant_am.to_string(),
            "ausfuehrt_bis": self.ausfuehrt_bis.to_string(),
            "total_slp_malos_enumerated": self.total_malos,
            "reading_orders_created": self.created,
            "already_scheduled_skipped": self.skipped,
            "legal_basis": "§ 40b Abs. 1 EnWG",
        })
    }
}

/// Why a campaign run could not complete.
pub enum CampaignError {
    /// `nb_mp_id` is not a 13-digit BDEW/DVGW Codenummer.
    InvalidNbMpId,
    /// `marktd` could not be reached or answered with an error.
    Marktd(String),
    /// The orders could not be written.
    ///
    /// Its own variant rather than a reuse of [`Self::Marktd`]: the two say
    /// different things to an operator — one is "the master-data service is
    /// down", the other "this database refused the write" — and they are fixed
    /// in different places.
    Store(String),
}

impl CampaignError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::InvalidNbMpId => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "nb_mp_id must be a 13-digit BDEW/DVGW Codenummer",
                })),
            )
                .into_response(),
            Self::Marktd(detail) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": detail })),
            )
                .into_response(),
            Self::Store(detail) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": detail })),
            )
                .into_response(),
        }
    }

    /// Human-readable reason, for callers that are not HTTP.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::InvalidNbMpId => "nb_mp_id must be a 13-digit BDEW/DVGW Codenummer".to_owned(),
            Self::Marktd(d) | Self::Store(d) => d.clone(),
        }
    }
}

/// Create the Jahresablesung reading orders for one NB's grid area.
///
/// Shared by the HTTP endpoint and the MCP tool so both raise identical orders.
/// A second implementation would be a second § 40b Abs. 1 EnWG obligation with its
/// own idempotency rules.
///
/// # Errors
///
/// [`CampaignError`] when `nb_mp_id` is malformed or `marktd` cannot be read.
/// Per-MaLo insert failures are logged and skipped: a campaign that aborts
/// half-way leaves an unrepeatable partial state, whereas re-running is
/// idempotent.
pub async fn run_jahresablesung_campaign(
    pool: &sqlx::PgPool,
    tenant: &str,
    marktd_url: &str,
    marktd_api_key: &secrecy::SecretString,
    req: &JahresablesungCampaignRequest,
) -> Result<CampaignOutcome, CampaignError> {
    let year = req
        .campaign_year
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().year());

    // Default dates: geplant_am = Dec 31, ausfuehrt_bis = Jan 31 next year.
    let geplant_am = req.geplant_am.unwrap_or_else(|| {
        time::Date::from_calendar_date(year, time::Month::December, 31)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date())
    });
    let ausfuehrt_bis = req.ausfuehrt_bis.unwrap_or_else(|| {
        time::Date::from_calendar_date(year + 1, time::Month::January, 31)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date())
    });

    let max_malos = req.max_malos.unwrap_or(5_000).min(50_000);
    let marktd = mako_service::http::Upstream::new(
        "marktd",
        marktd_url,
        Some(marktd_api_key.clone()),
        mako_service::http::default_client(),
    );

    // Enumerate the SLP MaLos in **this NB's** grid area (paginated, 500 per
    // page). `zuordnungstyp=NB` with `rollencodenummer` restricts the result to
    // MaLos whose Netzbetreiber role is held by `nb_mp_id`; without it the
    // campaign enumerates every SLP MaLo in the market and creates reading
    // orders for locations another NB is responsible for.
    // A BDEW/DVGW Codenummer is 13 digits. Validating rather than escaping keeps
    // the value out of the query string unless it is well formed.
    let nb_mp_id = req.nb_mp_id.trim();
    if nb_mp_id.len() != 13 || !nb_mp_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(CampaignError::InvalidNbMpId);
    }
    // The MaLo **and its Sparte**. A campaign enumerates SLP Marktlokationen,
    // and an SLP point is as often gas as electricity — the Sparte decides
    // whether the Zählerstand the order comes back with is kWh or m³, and a
    // reading filed in the wrong dimension is refused rather than stored.
    let mut malos: Vec<(String, crate::domain::Sparte)> = Vec::new();
    let mut page = 1i64;
    let page_size = 500i64;

    loop {
        let request = marktd.get("/api/v1/malos").query(&[
            ("bilanzierungsmethode", "SLP".to_owned()),
            ("zuordnungstyp", "NB".to_owned()),
            ("rollencodenummer", nb_mp_id.to_owned()),
            ("size", page_size.to_string()),
            ("page", page.to_string()),
        ]);
        let body: serde_json::Value = match marktd.json(request).await {
            Ok(Some(v)) => v,
            // marktd knows no MaLos for this grid area — an empty campaign, not
            // a failure.
            Ok(None) => break,
            Err(e) => {
                tracing::error!(error = %e, "edmd: campaign could not enumerate MaLos");
                return Err(CampaignError::Marktd(e.to_string()));
            }
        };

        let items = match body.get("items").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => break,
        };
        if items.is_empty() {
            break;
        }

        for item in &items {
            if let Some(mid) = item.get("malo_id").and_then(|v| v.as_str()) {
                let sparte = item
                    .get("sparte")
                    .and_then(|v| v.as_str())
                    .and_then(crate::domain::parse_sparte)
                    .unwrap_or(crate::domain::Sparte::Strom);
                malos.push((mid.to_owned(), sparte));
                if malos.len() as i64 >= max_malos {
                    break;
                }
            }
        }

        // Check pagination — stop when we've retrieved all or hit max.
        let total: i64 = body.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
        if malos.len() as i64 >= total || malos.len() as i64 >= max_malos {
            break;
        }
        page += 1;
    }

    let total_malos = malos.len();

    // One statement for the whole campaign, not a SELECT and an INSERT per MaLo.
    // At the 50 000-MaLo ceiling that was 100 000 round trips, and the pre-check
    // it saved was never the thing that made the run safe: `ON CONFLICT DO
    // NOTHING` against `ablese_scheduled_unique`
    // `(tenant, malo_id, anlass, geplant_am)` is, because only the constraint
    // survives two campaign runs racing. `rows_affected` is then exactly the
    // number of orders created, and everything else was already scheduled.
    let (malo_ids, spartes): (Vec<String>, Vec<String>) = malos
        .iter()
        .map(|(m, s)| (m.clone(), s.as_str().to_owned()))
        .unzip();
    let created = match sqlx::query(
        "INSERT INTO ablese_auftraege
             (malo_id,tenant,anlass,auftraggeber_rolle,ausfuehrender_msb,
              geplant_am,ausfuehrt_bis,sparte)
         SELECT m, $2, 'JAHRESABLESUNG', 'NB', $3, $4, $5, sp
           FROM unnest($1::text[], $6::text[]) AS t(m, sp)
         ON CONFLICT (tenant, malo_id, anlass, geplant_am)
             WHERE insrpt_process_id IS NULL
         DO NOTHING",
    )
    .bind(&malo_ids)
    .bind(tenant)
    .bind(&req.ausfuehrender_msb)
    .bind(geplant_am)
    .bind(ausfuehrt_bis)
    .bind(&spartes)
    .execute(pool)
    .await
    {
        Ok(r) => r.rows_affected(),
        Err(e) => {
            tracing::error!(error = %e, "edmd: Jahresablesung campaign insert failed");
            return Err(CampaignError::Store(format!("campaign insert failed: {e}")));
        }
    };
    let skipped = total_malos as u64 - created.min(total_malos as u64);

    tracing::info!(
        nb_mp_id = %req.nb_mp_id,
        campaign_year = year,
        total_malos,
        created,
        skipped,
        "edmd: Jahresablesung campaign complete"
    );

    Ok(CampaignOutcome {
        total_malos,
        created: usize::try_from(created).unwrap_or(usize::MAX),
        skipped: usize::try_from(skipped).unwrap_or(usize::MAX),
        year,
        geplant_am,
        ausfuehrt_bis,
    })
}

/// `GET /api/v1/compliance/jahresablesung/{year}`
///
/// § 40b Abs. 1 EnWG compliance report for a campaign year.
///
/// The obligation is to read each SLP Marktlokation annually. This reports
/// whether that happened, broken down by what actually became of each order —
/// which is the distinction that matters, because only `AUSGEFUEHRT` discharges
/// the obligation. `STORNIERT` withdraws it; `FEHLGESCHLAGEN` leaves it
/// outstanding with a documented Ablesehindernis; anything still `OFFEN` or
/// `BEAUFTRAGT` past its deadline is simply late.
///
/// **Cedar action**: `read-reading-order`
pub(crate) async fn jahresablesung_compliance(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(year): Path<i32>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let rows = sqlx::query(
        r"SELECT status,
                 count(*)                                        AS orders,
                 count(*) FILTER (WHERE ausfuehrt_bis < CURRENT_DATE
                                    AND status <> 'AUSGEFUEHRT') AS overdue
          FROM   ablese_auftraege
          WHERE  tenant = $1
            AND  anlass = 'JAHRESABLESUNG'
            AND  extract(year FROM geplant_am) = $2
          GROUP BY status",
    )
    .bind(&state.tenant)
    .bind(year)
    .fetch_all(state.repo.pool())
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    use sqlx::Row as _;
    let mut by_status = serde_json::Map::new();
    let (mut total, mut ausgefuehrt, mut overdue) = (0i64, 0i64, 0i64);
    for r in &rows {
        let status: String = r.try_get("status").unwrap_or_default();
        let orders: i64 = r.try_get("orders").unwrap_or(0);
        let od: i64 = r.try_get("overdue").unwrap_or(0);
        total += orders;
        overdue += od;
        if status == "AUSGEFUEHRT" {
            ausgefuehrt = orders;
        }
        by_status.insert(status, serde_json::json!(orders));
    }

    // Reasons the failed readings could not be taken. The Ablesehindernis
    // decides whether the NB may estimate under §40a EnWG or must re-dispatch.
    let grounds = sqlx::query(
        r"SELECT fehlschlag_grund, count(*) AS n
          FROM   ablese_auftraege
          WHERE  tenant = $1 AND anlass = 'JAHRESABLESUNG'
            AND  extract(year FROM geplant_am) = $2
            AND  status = 'FEHLGESCHLAGEN'
          GROUP BY fehlschlag_grund",
    )
    .bind(&state.tenant)
    .bind(year)
    .fetch_all(state.repo.pool())
    .await
    .unwrap_or_default();

    let mut by_grund = serde_json::Map::new();
    for r in &grounds {
        let g: Option<String> = r.try_get("fehlschlag_grund").ok().flatten();
        let n: i64 = r.try_get("n").unwrap_or(0);
        by_grund.insert(
            g.unwrap_or_else(|| "UNBEKANNT".to_owned()),
            serde_json::json!(n),
        );
    }

    // Rate against orders raised, not against the SLP population: this service
    // knows what was ordered, and `marktd` owns how many MaLos exist. A
    // population-based rate computed here would overstate coverage whenever a
    // MaLo was never scheduled at all.
    #[allow(clippy::cast_precision_loss)]
    let quote = if total > 0 {
        ausgefuehrt as f64 / total as f64
    } else {
        0.0
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "campaign_year":      year,
            "orders_total":       total,
            "ausgefuehrt":        ausgefuehrt,
            "ablesequote":        (quote * 10_000.0).round() / 10_000.0,
            "ueberfaellig":       overdue,
            "by_status":          by_status,
            "fehlschlag_gruende": by_grund,
            "legal_basis":        "§ 40b Abs. 1 EnWG i. V. m. GPKE (BK6-24-174) Teil 1 Turnusablesung; § 40a Abs. 2 EnWG (Schätzung)",
            "note": "`ablesequote` is over orders raised, not over the SLP population — \
                     a MaLo that was never scheduled has no order here. Cross-check the \
                     population with marktd.",
        })),
    )
        .into_response()
}
