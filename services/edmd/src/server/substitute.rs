//! § 60 Abs. 1 MsbG auto-substitute (Ersatzwertbildung).
//!
//! The authority is **§ 60 Abs. 1 MsbG**: the Messstellenbetreiber owes the
//! berechtigten Stellen the *aufbereiteten* Messwerte at the times they set, and
//! Plausibilisierung und Ersatzwertbildung is that Aufbereitung — a slot with no
//! usable measurement still has to be delivered as something.
//!
//! **Not § 60 Abs. 2 MsbG.** That is a *Soll*-rule about *where* the Aufbereitung
//! runs — „im Smart-Meter-Gateway", for intelligente Messsysteme, once the BSI
//! rates it technically possible and the BNetzA has made the § 75 Satz 1 Nr. 4
//! Festlegung. It obliges nobody to form an Ersatzwert. Its Satz 2 is what lets
//! this endpoint exist at all: until that Festlegung the Aufbereitung may run
//! outside the gateway at the Messstellenbetreiber, and for gas permanently, at
//! the berechtigten Stellen nach § 49 Abs. 2.
//!
//! On the billing side the same gap is **§ 40a Abs. 2 EnWG**: consumption that
//! cannot be determined may be billed on a Schätzung „unter angemessener
//! Berücksichtigung der tatsächlichen Verhältnisse", and Satz 3 requires the
//! estimate, its grounds and its factors to be stated on the bill „unter
//! ausdrücklichem und optisch besonders hervorgehobenem Hinweis" — which is why
//! every value written here keeps the `SUBSTITUTED` quality flag.

#[allow(
    unused_imports,
    reason = "used only in some feature or target combinations"
)]
use super::*;

// ── § 60 Abs. 1 MsbG Auto-Substitute (post_substitute_values) ───────────────────────

/// Request body for `POST /api/v1/meter-reads/{malo_id}/substitute`.
///
/// There is no method to choose. `metering::vee::substitute` applies the
/// MeteringCode 2006 rule: a gap of up to two hours between validated measured
/// values is interpolated (`Z92`), a longer gap — or one at the edge of the
/// data — takes the mean of the same slot on the previous days of the same day
/// class (`ZJ2` Strom, `Z95` Gas), and a slot no rule can fill is refused,
/// never invented.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubstituteRequest {
    /// Gap start (UTC, RFC3339).
    pub gap_from: String,
    /// Gap end (UTC, RFC3339). Not in the future: a substitute stands in for a
    /// value that should already have arrived.
    pub gap_to: String,
    /// How far back to read reference data, in days (default: 7).
    ///
    /// The Vergleichswert averages the same slot on the previous **three** days
    /// of the gap's day class (Werktag, Samstag, Sonn-/Feiertag), so the window
    /// has to reach back far enough to hold three of them — a week holds three
    /// Werktage, but only one Saturday. It also supplies the measured values an
    /// interpolation is bracketed by.
    pub prior_days: Option<u32>,
    /// The Bundesland whose statutory holidays decide the day class
    /// (`BE`, `NW`, `DE-BY`, …) — **required**.
    ///
    /// A holiday is compared with Sundays, and holidays differ by Land.
    /// edmd holds no address for a Marktlokation (`marktd` does), and guessing
    /// one would compare a Bavarian Fronleichnam with ordinary Thursdays.
    pub bundesland: String,
    /// Fill gaps longer than two hours with the last measured value (`Z93`)
    /// instead of the Vergleichswert. Gas only — the MIG codes `Z93` for gas
    /// alone — and refused for any other Sparte.
    #[serde(default)]
    pub hold: bool,
    /// The whole gap window is a confirmed supply interruption, filled with
    /// zeros (MeteringCode 2006 A7.1.2.2). Only for an outage that was
    /// established, never for a gap nobody explained.
    #[serde(default)]
    pub confirmed_outage: bool,
    /// Operator ID for audit trail.
    pub operator_id: Option<String>,
    /// `STROM` (default) · `GAS` · `WAERME` · `WASSER`. Determines the `unit`
    /// the substitute is stored in — a substituted water gap is m³, not kWh.
    pub sparte: Option<String>,
    /// Why a substitute is required — **required**.
    ///
    /// One of the 28 `STS+Z40` Gründe der Ersatzwertbildung (MSCONS MIG 2.5),
    /// by name (`NO_ACCESS`) or by market code (`Z74`). The list is closed and
    /// carries no catch-all, so an unstated reason cannot be defaulted: it would
    /// put an invented one into a § 147 Abs. 1 AO audit trail and onto the
    /// `STS+Z40` of the MSCONS that later carries the value.
    pub reason: String,
    /// OBIS register the gap belongs to.
    ///
    /// Omit it and the measuring point's **dominant energy register** — the one
    /// carrying the most intervals in the window — is substituted, and the
    /// Ersatzwert is filed under that register. It is worth naming explicitly on
    /// a multi-register point: the reference data, the occupied-slot guard and
    /// the interpolation brackets are all read from the one register the value
    /// is written to, and "dominant" is a guess about which one the caller meant.
    pub obis_code: Option<String>,
}

/// `POST /api/v1/meter-reads/{malo_id}/substitute`
///
/// Generate and store § 60 Abs. 1 MsbG substitute values for a gap interval.
///
/// This endpoint:
/// 1. Validates the requested gap window.
/// 2. Reads the register the gap belongs to, with reference data either side.
/// 3. Validates it over the gap window and fills every slot an error finding
///    covers with `metering::vee::substitute::substitute`.
/// 4. Stores the generated intervals as `AUTO_SUBSTITUTE` source.
/// 5. Records each substitution in `substitute_value_log` for § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit.
/// 6. Returns the generated intervals with their methods, codes and references,
///    and every span that was refused.
///
/// **Cedar action**: `write-meter-reads`
pub async fn post_substitute_values(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<SubstituteRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    run_substitute_values(&state.repo, &state.tenant, &malo_id, &req).await
}

/// Core of the § 60 Abs. 1 MsbG substitute flow.
///
/// Shared by the HTTP handler (above) and the `trigger_substitution` MCP tool,
/// so both write substitutes under identical guards: never over a billable
/// reading, always with a `substitute_value_log` audit row, atomically.
///
/// Public so the § 60 Abs. 1 MsbG obligation can be pinned against a real
/// database rather than only through an authenticated HTTP round trip — the
/// numbers it produces are the regulated artefact, and they are what the
/// integration suite asserts.
pub async fn run_substitute_values(
    repo: &crate::store::MeterStoreTimeSeriesRepository,
    tenant: &str,
    malo_id: &str,
    req: &SubstituteRequest,
) -> axum::response::Response {
    use crate::domain::repository::TimeSeriesRepository as _;
    use crate::domain::{IngestionSource, MeterRead, TimeSeriesQuery};
    use metering::QualityFlag;
    use metering::vee::substitute::{Policy, SubstitutionReason};
    use time::format_description::well_known::Rfc3339;

    let unprocessable = |error: String| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": error })),
        )
            .into_response()
    };

    let gap_from = match OffsetDateTime::parse(&req.gap_from, &Rfc3339) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid gap_from: {e}") })),
            )
                .into_response();
        }
    };
    let gap_to = match OffsetDateTime::parse(&req.gap_to, &Rfc3339) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid gap_to: {e}") })),
            )
                .into_response();
        }
    };

    if gap_from >= gap_to {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "gap_from must be before gap_to" })),
        )
            .into_response();
    }
    let now = OffsetDateTime::now_utc();
    if gap_to > now {
        return unprocessable(format!(
            "gap_to {gap_to} lies in the future: an Ersatzwert stands in for a value \
             that should already have arrived"
        ));
    }

    let prior_days = i64::from(req.prior_days.unwrap_or(7));
    let operator_id = req.operator_id.as_deref().unwrap_or("AUTO");

    // The `STS+Z40` Grund. The list is closed: an unrecognised token is refused
    // rather than logged verbatim, because the value it labels is what a later
    // MSCONS states as its Ersatzwertbildungsgrund.
    let reason: SubstitutionReason = {
        let raw = req.reason.trim();
        match raw
            .parse()
            .ok()
            .or_else(|| SubstitutionReason::from_code(raw))
        {
            Some(r) => r,
            None => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!("unknown Ersatzwertbildungsgrund `{raw}`"),
                        "supported": SubstitutionReason::ALL
                            .iter()
                            .map(|r| serde_json::json!({
                                "name": r.as_str(),
                                "code": r.code(),
                                "beschreibung": r.description(),
                            }))
                            .collect::<Vec<_>>(),
                    })),
                )
                    .into_response();
            }
        }
    };
    let bundesland: metering::time::holiday::Bundesland = match req.bundesland.trim().parse() {
        Ok(land) => land,
        Err(e) => return unprocessable(format!("bundesland: {e}")),
    };

    // Read the register across the reference + gap + trailing window through
    // the repository. `query` is version-resolved and tenant-scoped and does
    // NOT filter quality: a Faulty/Unknown value is a `NON_BILLABLE` finding,
    // which the substitution replaces and keeps as the original — and which
    // never serves as reference data or brackets an interpolation.
    let prior_from = gap_from - time::Duration::days(prior_days);
    let resolved = match repo
        .query(&TimeSeriesQuery {
            malo_id: malo_id.to_string(),
            from: prior_from,
            to: gap_to + time::Duration::days(prior_days),
            sparte: None,
            tenant: tenant.to_string(),
        })
        .await
    {
        Ok(reads) => reads,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: substitute reference read failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    // Scope the reference data to the **one register** the Ersatzwert is written
    // to. `query` spans every channel the measuring point reported, and a series
    // is one channel: a prosumer's Einspeisung reading at 12:00 must neither
    // mark the 12:00 slot of its *consumption* register occupied nor bracket an
    // interpolation in it.
    //
    // The target register is the request's when it names one, otherwise the
    // measuring point's dominant energy series.
    let groups = crate::domain::register_groups(&resolved);
    let target: Option<String> = match req.obis_code.as_deref() {
        Some(code) => Some(
            code.parse::<metering::ObisCode>()
                .map_or_else(|_| code.to_owned(), |c| c.to_string()),
        ),
        None => groups
            .iter()
            .filter(|g| {
                g.obis_code
                    .is_none_or(crate::domain::register::is_energy_register)
            })
            .max_by_key(|g| g.intervals.len())
            .and_then(|g| g.obis_code.map(|c| c.to_string())),
    };
    let in_target = |r: &MeterRead| -> bool {
        let code = r
            .obis_code
            .as_deref()
            .and_then(|s| s.parse::<metering::ObisCode>().ok())
            .map(|c| c.to_string());
        code == target
    };
    let resolved: Vec<MeterRead> = resolved.into_iter().filter(in_target).collect();

    // The Sparte is authoritative on the stored series the gap belongs to; the
    // request value is only a fallback for a MaLo with no resolved reads.
    let sparte = match resolved.first().map(|r| r.sparte) {
        Some(sparte) => sparte,
        None => match req.sparte.as_deref().map(|s| s.trim().parse()) {
            Some(Ok(sparte)) => sparte,
            Some(Err(e)) => return unprocessable(format!("sparte: {e}")),
            None => metering::Sparte::Strom,
        },
    };

    // Slots inside the gap window that already carry a billable reading: a
    // substitute must never overwrite a real measurement (§ 60 Abs. 1 MsbG).
    let billable_slots: std::collections::HashSet<OffsetDateTime> = resolved
        .iter()
        .filter(|r| r.quality.is_billable() && r.dtm_from >= gap_from && r.dtm_from < gap_to)
        .map(|r| r.dtm_from)
        .collect();

    // The reporting operator keys the meterstore version scope, and versions
    // only supersede each other *within* a scope. A substitute filed under the
    // tenant would therefore never displace the faulty row it replaces: both
    // would stand and the slot would be counted twice. So it inherits the
    // operator of the reading it stands in for — preferring one from inside the
    // window, since that is the row being displaced.
    let sender_mp_id = resolved
        .iter()
        .filter(|r| r.dtm_from >= gap_from && r.dtm_from < gap_to)
        .find_map(|r| r.sender_mp_id.clone())
        .or_else(|| resolved.iter().find_map(|r| r.sender_mp_id.clone()));

    // The register as one series on its observed grid. The whole queried window
    // goes in, so an interpolation finds the measured values bracketing the gap
    // and the Vergleichswert finds the comparable days before it.
    let intervals: Vec<metering::MeterInterval> =
        resolved.iter().filter_map(|r| r.interval().ok()).collect();
    let series = match crate::domain::vee::series_of(sparte, intervals) {
        Ok(series) => series,
        Err(defect) => {
            return unprocessable(format!(
                "the register's readings are not one series ({}): {defect}",
                defect.code()
            ));
        }
    };

    let mut policy = Policy::new(sparte, bundesland, reason);
    if req.hold {
        policy = match policy.hold() {
            Ok(p) => p,
            Err(e) => return unprocessable(e.to_string()),
        };
    }
    if req.confirmed_outage {
        policy = policy.outage(gap_from, gap_to);
    }

    // Validate over the calendar periods the gap window falls in, keep only
    // the findings inside the window, and fill what an error finding covers.
    // The report still describes the whole series, so the substitution reads
    // its brackets and comparable days from all of it but fills only the
    // window.
    let boundary = series.boundary();
    let Some(periods) = crate::domain::vee::covering_periods(boundary, gap_from, gap_to) else {
        return unprocessable(format!(
            "{gap_from} – {gap_to} lies outside the supported calendar"
        ));
    };
    let mut substitutes = Vec::new();
    let mut refusals = Vec::new();
    for period in periods {
        let mut report = metering::vee::validation::validate(
            &series,
            &metering::vee::validation::Rules::new(sparte, period, now, None),
        );
        crate::domain::vee::clip_report(
            &mut report,
            boundary,
            series.resolution(),
            gap_from,
            gap_to,
        );
        match metering::vee::substitute::substitute(&series, &report, &policy) {
            Ok(filled) => {
                substitutes.extend(filled.substitutes);
                refusals.extend(filled.refusals);
            }
            Err(e) => return unprocessable(e.to_string()),
        }
    }
    let refused: Vec<serde_json::Value> = refusals
        .iter()
        .map(|r| {
            serde_json::json!({
                "from": r.from,
                "to": r.to,
                "reason": format!("{:?}", r.reason),
            })
        })
        .collect();

    if substitutes.is_empty() && !refused.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "No substitute values could be formed for this gap window",
                "refused": refused,
            })),
        )
            .into_response();
    }

    let reason_str = reason.as_str().to_owned();
    let mut stored = 0usize;
    let mut log_entries: Vec<serde_json::Value> = Vec::new();
    // Intervals left alone because they already carry a billable reading.
    let mut skipped: Vec<String> = Vec::new();
    // The substitute readings to append, and their per-interval audit tuples
    // (from, to, method, substitute_kwh, original_kwh).
    let mut substitute_reads: Vec<MeterRead> = Vec::new();
    let mut audit: Vec<(
        OffsetDateTime,
        OffsetDateTime,
        &'static str,
        rust_decimal::Decimal,
        Option<rust_decimal::Decimal>,
    )> = Vec::new();

    // No second validation pass here: `ValidatedReads::validate` below runs the
    // same one every ingest door runs — split per register, the Sparte's own
    // preset, the grid observed.
    for entry in &substitutes {
        // A § 60 Abs. 1 MsbG Ersatzwert fills a gap or replaces a rejected
        // value; it never overwrites a billable measurement. A slot already
        // carrying one is left untouched and reported as skipped.
        if billable_slots.contains(&entry.from) {
            skipped.push(entry.from.format(&Rfc3339).unwrap_or_default());
            continue;
        }
        // The method that *actually ran*, chosen by the gap's length and the
        // data available — the audit trail records what happened.
        let method_str = entry.method.as_str();
        substitute_reads.push(MeterRead {
            malo_id: malo_id.to_string(),
            melo_id: None,
            dtm_from: entry.from,
            dtm_to: entry.to,
            quantity_kwh: entry.value,
            quality: QualityFlag::Substituted,
            pid: 0,
            sparte,
            // The register the Ersatzwert was **derived from**, not whatever the
            // request happened to name. `target` is the request's `obis_code`
            // when it gave one and the point's dominant energy series otherwise.
            // An unlabelled reading *is* the canonical total register
            // (`domain::register`), so filing an unrequested substitute there
            // would make a dual-tariff series look like a decomposition of it.
            obis_code: target.clone(),
            tenant: tenant.to_string(),
            source: IngestionSource::AutoSubstitute,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
            // edmd's own output, not a delivery — arrival order is the order.
            mscons_version: None,
        });
        audit.push((
            entry.from,
            entry.to,
            method_str,
            entry.value,
            entry.original,
        ));
        stored += 1;
        log_entries.push(serde_json::json!({
            "from": entry.from,
            "to": entry.to,
            "value": entry.value.to_string(),
            "method": method_str,
            "code": entry.code,
            "reason": entry.reason.as_str(),
            "reason_code": entry.reason.code(),
            "reference_count": entry.references.len(),
            "original": entry.original.map(|v| v.to_string()),
        }));
    }

    if substitute_reads.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id": malo_id,
                "generated_count": 0,
                "skipped_measured": skipped,
                "message": "every interval in the window already carries a billable reading",
            })),
        )
            .into_response();
    }

    // Persist through the repository. meterstore's `append` routes each interval to
    // the tier that owns it, opens the § 60 Abs. 1 MsbG confirmation obligation
    // (so an operator is later nudged to replace the estimate with a real value),
    // and writes the § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) displacement audit — none of which the old
    // raw upsert did.
    // Ersatzwerte are edmd's own output, but they are billed like any other
    // reading, so they run the same validation — a generator that emits a
    // span off the grid or a duplicate slot must fail here, not at settlement.
    let (validated, _) = crate::domain::ValidatedReads::validate(
        substitute_reads,
        crate::domain::IngestContext::new("SUBSTITUTE_VALIDATION", malo_id),
    );

    if let Err(e) = repo.store_reads(validated).await {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute store failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // The per-interval method/reason audit row. `original_kwh` is the rejected
    // value a substitute replaced, NULL where the slot was a gap.
    let mut tx = match repo.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute audit tx failed to begin");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    for (from, to, method_str, kwh, original) in &audit {
        if let Err(e) = sqlx::query(
            r"INSERT INTO substitute_value_log
                (malo_id, dtm_from, dtm_to, method, reason, substitute_kwh,
                 original_kwh, created_by, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(method_str)
        .bind(&reason_str)
        .bind(kwh)
        .bind(original)
        .bind(operator_id)
        .bind(tenant)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute audit log failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute commit failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string(), "generated_count": 0 })),
        )
            .into_response();
    }

    tracing::info!(
        malo_id, stored, operator_id,
        gap_from = %gap_from, gap_to = %gap_to,
        "edmd: § 60 Abs. 1 MsbG substitute values generated"
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "malo_id": malo_id,
            "gap_from": gap_from,
            "gap_to": gap_to,
            "generated_count": stored,
            "resolution": series.resolution().to_string(),
            "bundesland": bundesland.as_str(),
            "obis_code": target,
            // What each interval was actually produced by: the method follows
            // the gap's length and the data available, so the § 147 Abs. 1 AO
            // record names what ran.
            "methods_applied": log_entries
                .iter()
                .filter_map(|e| e["method"].as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            // Intervals already covered by a billable reading. The § 60 Abs. 1
            // MsbG Aufbereitungspflicht is discharged by the measurement where
            // one exists; a substitute stands only in its absence.
            "skipped_measured": skipped,
            // Spans no rule could fill — never invented.
            "refused": refused,
            "legal_basis": "§ 60 Abs. 1 MsbG Ersatzwertbildung · § 40a Abs. 2 EnWG (Verbrauchsschätzung, Satz 3 Hinweispflicht)",
            "intervals": log_entries,
        })),
    )
        .into_response()
}
