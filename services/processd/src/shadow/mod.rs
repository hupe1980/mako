//! Private review of one locally reconciled cohort. This composition has no dispatch workers.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::ensure;
use axum::{
    Extension, Router,
    extract::{Path, State},
    middleware,
};
use mako_service::{
    ApiError, ApiResult, Befund, Json, ServiceContext,
    oidc::{Claims, ExpectedTenant, OidcConfig},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row as _};
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::config::Config;
mod mcp;
mod packet;
use packet::{Assessment, Cohort};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub file: PathBuf,
    pub sha256: String,
    pub scope: String,
    pub generation: i64,
    pub required_werktage: u32,
    pub readers: BTreeSet<String>,
    pub resolvers: BTreeSet<String>,
}

fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

impl Settings {
    pub(crate) fn validate(&self, cfg: &Config) -> Vec<Befund> {
        let mut out = Vec::new();
        for (valid, path, message) in [
            (
                cfg.oidc.as_ref().is_some_and(|o| o.service_keys.is_empty())
                    && !cfg.allow_insecure_no_auth,
                "oidc",
                "Configure token verification for the review installation.",
            ),
            (
                cfg.identity.nb_mp_ids.len() == 1 && !cfg.identity.tenant.trim().is_empty(),
                "identity",
                "Configure one Strom NB and a tenant for the review.",
            ),
            (
                self.generation > 0,
                "shadow_review.generation",
                "A review generation must be positive.",
            ),
            (
                self.required_werktage > 0,
                "shadow_review.required_werktage",
                "At least one complete Werktag is required.",
            ),
            (
                digest(&self.sha256) && digest(&self.scope),
                "shadow_review.sha256",
                "Pin the file SHA-256 and cohort scope.",
            ),
            (
                !self.readers.is_empty()
                    && self.resolvers.is_subset(&self.readers)
                    && self.readers.iter().all(|s| !s.trim().is_empty()),
                "shadow_review.readers",
                "Configure named readers and grant resolution only to a named reader.",
            ),
        ] {
            if !valid {
                out.push(Befund::blockiert(path, message));
            }
        }
        out
    }
}

#[derive(Clone)]
pub(crate) struct Review {
    pool: PgPool,
    cedar: Arc<mako_service::cedar::CedarEnforcer>,
    tenant: String,
    cohort: Arc<Cohort>,
    revision: String,
    generation: i64,
    required_werktage: u32,
    corrections: Arc<BTreeSet<String>>,
    readers: BTreeSet<String>,
    resolvers: BTreeSet<String>,
}

impl Review {
    fn authorize(&self, c: &Claims, write: bool) -> ApiResult<()> {
        let subjects = if write {
            &self.resolvers
        } else {
            &self.readers
        };
        let action = if write {
            "resolve-shadow-review"
        } else {
            "read-shadow-review"
        };
        if subjects.contains(c.sub())
            && self
                .cedar
                .check(&c.principal(), action, &self.tenant)
                .is_ok()
        {
            Ok(())
        } else {
            Err(ApiError::forbidden("No access to this cohort").with_code("shadow_forbidden"))
        }
    }

    async fn current(&self, tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> ApiResult<()> {
        let generation: Option<i64> = sqlx::query_scalar(
            "SELECT generation FROM shadow_review_head
             WHERE tenant = $1 AND nb = $2 AND scope = $3 FOR SHARE",
        )
        .bind(&self.tenant)
        .bind(&self.cohort.nb)
        .bind(&self.cohort.scope)
        .fetch_optional(&mut **tx)
        .await?;
        if generation != Some(self.generation) {
            return Err(stale());
        }
        Ok(())
    }
}

fn stale() -> ApiError {
    ApiError::precondition_failed("Review evidence changed. Reload the cohort.")
        .with_code("shadow_revision_changed")
}

#[derive(Serialize, ToSchema)]
pub struct Resolution {
    pub occurrence: String,
    pub generation: i64,
    pub disposition: String,
    pub actor: String,
    pub evidence: String,
    pub reason: String,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    pub recorded_at: time::OffsetDateTime,
}

#[derive(Serialize, ToSchema)]
pub struct ReviewResult {
    #[schema(value_type = Cohort)]
    pub cohort: Arc<Cohort>,
    pub generation: i64,
    pub revision: String,
    pub resolutions: Vec<Resolution>,
    pub eligible: usize,
    pub assessed: usize,
    pub unassessed: usize,
    pub different: usize,
    pub open_differences: usize,
    pub timing_unassessed: usize,
    pub required_werktage: u32,
    pub ready: bool,
    pub can_resolve: bool,
    pub corrections: BTreeSet<String>,
}

impl Review {
    async fn read(&self, can_resolve: bool) -> ApiResult<ReviewResult> {
        let mut tx = self.pool.begin().await?;
        self.current(&mut tx).await?;
        let history = sqlx::query(
            "SELECT DISTINCT ON (occurrence)
                    occurrence, generation, disposition, actor, evidence, reason, recorded_at
             FROM shadow_review_resolution
             WHERE tenant = $1 AND nb = $2 AND scope = $3 AND occurrence = ANY($4)
             ORDER BY occurrence, generation DESC",
        )
        .bind(&self.tenant)
        .bind(&self.cohort.nb)
        .bind(&self.cohort.scope)
        .bind(
            self.cohort
                .rows
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let mut resolved = BTreeSet::new();
        let mut resolutions = Vec::new();
        for r in history {
            let occurrence: String = r.try_get("occurrence")?;
            let generation = r.try_get("generation")?;
            if generation == self.generation {
                resolved.insert(occurrence.clone());
            }
            let recorded_at: time::OffsetDateTime = r.try_get("recorded_at")?;
            resolutions.push(Resolution {
                occurrence,
                generation,
                disposition: r.try_get("disposition")?,
                actor: r.try_get("actor")?,
                evidence: r.try_get("evidence")?,
                reason: r.try_get("reason")?,
                recorded_at,
            });
        }
        let eligible = self.cohort.rows.len();
        let mut assessed = 0;
        let mut timed = 0;
        let mut different = 0;
        let mut open_differences = 0;
        for row in &self.cohort.rows {
            if let Some(a) = &row.assessment {
                assessed += 1;
                timed += usize::from(a.timing != "unassessed");
                if a.differs() {
                    different += 1;
                    open_differences += usize::from(!resolved.contains(&row.id));
                }
            }
        }
        open_differences += self.corrections.difference(&resolved).count();
        let timing_unassessed = eligible - timed;
        let ready = eligible > 0
            && assessed == eligible
            && open_differences == 0
            && timing_unassessed == 0
            && self.cohort.complete_werktage >= self.required_werktage
            && self.cohort.directions == ["inbound", "outbound"];
        Ok(ReviewResult {
            cohort: self.cohort.clone(),
            revision: self.revision.clone(),
            generation: self.generation,
            resolutions,
            eligible,
            assessed,
            unassessed: eligible - assessed,
            different,
            open_differences,
            timing_unassessed,
            required_werktage: self.required_werktage,
            ready,
            can_resolve,
            corrections: self.corrections.as_ref().clone(),
        })
    }
}

#[utoipa::path(get, path="/api/v1/shadow/review", tag="shadow", responses((status=200, body=ReviewResult)))]
async fn get_review(
    State(s): State<Review>,
    claims: Claims,
) -> ApiResult<axum::Json<ReviewResult>> {
    s.authorize(&claims, false)?;
    Ok(axum::Json(
        s.read(s.resolvers.contains(claims.sub())).await?,
    ))
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Resolve {
    pub generation: i64,
    pub revision: String,
    pub evidence: String,
    pub reason: String,
}

/// Record that the incumbent answer differs from the reviewed evidence (alt_falsch).
#[utoipa::path(post, path="/api/v1/shadow/review/{id}/resolve", tag="shadow", params(("id" = String, Path)), request_body=Resolve, responses((status=204),(status=412)))]
async fn resolve(
    State(s): State<Review>,
    claims: Claims,
    Path(id): Path<String>,
    Json(body): Json<Resolve>,
) -> ApiResult<axum::http::StatusCode> {
    s.authorize(&claims, true)?;
    if body.generation != s.generation || body.revision != s.revision {
        return Err(stale());
    }
    let a = s
        .cohort
        .rows
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::not_found("Unknown occurrence"))?
        .assessment
        .as_ref();
    if !s.corrections.contains(&id) && !a.is_some_and(Assessment::differs) {
        return Err(
            ApiError::conflict("Only an assessed difference can be resolved")
                .with_code("shadow_no_difference"),
        );
    }
    let reason = body.reason.trim();
    let evidence = body.evidence.trim();
    if [reason, evidence]
        .iter()
        .any(|v| v.is_empty() || v.len() > 1000 || v.chars().any(char::is_control))
    {
        return Err(ApiError::unprocessable(
            "Supply a reason and evidence reference of at most 1000 bytes",
        )
        .with_code("shadow_reason_required"));
    }
    let mut tx = s.pool.begin().await?;
    s.current(&mut tx).await?;
    let disposition = if s.corrections.contains(&id) {
        "mako_korrigiert"
    } else {
        "alt_falsch"
    };
    let stored = sqlx::query(
        "INSERT INTO shadow_review_resolution
            (tenant, nb, scope, generation, occurrence, actor, evidence, reason, disposition)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) ON CONFLICT DO NOTHING",
    )
    .bind(&s.tenant)
    .bind(&s.cohort.nb)
    .bind(&s.cohort.scope)
    .bind(s.generation)
    .bind(&id)
    .bind(claims.sub())
    .bind(evidence)
    .bind(reason)
    .bind(disposition)
    .execute(&mut *tx)
    .await?;
    if stored.rows_affected() == 0 {
        let same: bool = sqlx::query_scalar(
            "SELECT actor = $6 AND evidence = $7 AND reason = $8
             FROM shadow_review_resolution
             WHERE tenant = $1 AND nb = $2 AND scope = $3 AND generation = $4 AND occurrence = $5",
        )
        .bind(&s.tenant)
        .bind(&s.cohort.nb)
        .bind(&s.cohort.scope)
        .bind(s.generation)
        .bind(&id)
        .bind(claims.sub())
        .bind(evidence)
        .bind(reason)
        .fetch_one(&mut *tx)
        .await?;
        if !same {
            return Err(ApiError::conflict("A resolution is already recorded")
                .with_code("shadow_already_resolved"));
        }
    }
    tx.commit().await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub(crate) fn api() -> OpenApiRouter<Review> {
    OpenApiRouter::new()
        .routes(routes!(get_review))
        .routes(routes!(resolve))
}

/// Demo OIDC admits an anonymous synthetic persona on live routes. A recorded review
/// always requires an explicit JWT, including when rehearsed with the demo issuer.
async fn require_token(
    request: axum::extract::Request,
    next: middleware::Next,
) -> ApiResult<axum::response::Response> {
    let bearer = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if !bearer.is_some_and(mako_service::oidc::OidcVerifier::looks_like_jwt) {
        return Err(
            ApiError::unauthorized("Supply an OIDC token").with_code("shadow_token_required")
        );
    }
    Ok(next.run(request).await)
}

pub(crate) async fn install(
    settings: &Settings,
    cfg: &Config,
    ctx: ServiceContext,
) -> anyhow::Result<Router> {
    ensure!(
        settings.validate(cfg).is_empty(),
        "invalid isolated review settings"
    );
    let cohort = Cohort::load(settings, &cfg.identity.nb_mp_ids[0])?;
    let oidc = OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &ctx.http,
        &cfg.identity.tenant,
        ctx.shutdown.clone(),
    )
    .await?;
    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../../policies/shadow.cedar"
        ))
        .map_err(|e| anyhow::anyhow!("review policy: {e}"))?,
    );
    let pool = ctx.pool().clone();
    let mut tx = pool.begin().await?;
    let installed = sqlx::query(
        "INSERT INTO shadow_review_head AS head (tenant, nb, scope, generation, digest)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (tenant, nb, scope) DO UPDATE
             SET generation = EXCLUDED.generation, digest = EXCLUDED.digest
         WHERE (head.generation < EXCLUDED.generation AND head.digest <> EXCLUDED.digest)
            OR (head.generation = EXCLUDED.generation AND head.digest = EXCLUDED.digest)",
    )
    .bind(&cfg.identity.tenant)
    .bind(&cohort.nb)
    .bind(&cohort.scope)
    .bind(settings.generation)
    .bind(&settings.sha256)
    .execute(&mut *tx)
    .await?;
    ensure!(
        installed.rows_affected() == 1,
        "stale or conflicting review generation"
    );
    // The comparison is retained with its resolution even after the local file is replaced.
    let content_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&cohort)?));
    let previous: Option<(sqlx::types::Json<Cohort>, Vec<String>)> = sqlx::query_as(
        "SELECT packet, corrections FROM shadow_review_generation
         WHERE tenant = $1 AND nb = $2 AND scope = $3 AND generation < $4
         ORDER BY generation DESC LIMIT 1",
    )
    .bind(&cfg.identity.tenant)
    .bind(&cohort.nb)
    .bind(&cohort.scope)
    .bind(settings.generation)
    .fetch_optional(&mut *tx)
    .await?;
    let mut prior = BTreeMap::new();
    let mut rechecked = BTreeSet::new();
    if let Some((p, corrections)) = previous {
        ensure!(
            p.start == cohort.start && p.end == cohort.end && p.directions == cohort.directions,
            "cohort interval or directions changed under the same scope"
        );
        prior.extend(p.0.rows.into_iter().map(|r| (r.id.clone(), r)));
        rechecked.extend(corrections);
    }
    // Changing an expectation alone cannot establish that mako was corrected.
    let corrections: BTreeSet<String> = cohort
        .rows
        .iter()
        .filter(|row| {
            let Some(old) = prior.get(&row.id) else {
                return false;
            };
            let Some((before, after)) = old.assessment.as_ref().zip(row.assessment.as_ref()) else {
                return false;
            };
            old.request_sha256 == row.request_sha256
                && before.expected_fingerprint == after.expected_fingerprint
                && before.expected == after.expected
                && (before.actual != before.expected || rechecked.contains(&row.id))
                && !after.differs()
        })
        .map(|r| r.id.clone())
        .collect();
    sqlx::query(
        "INSERT INTO shadow_review_generation
        (tenant, nb, scope, generation, content_digest, packet, corrections)
        VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (tenant,nb,scope,generation) DO NOTHING",
    )
    .bind(&cfg.identity.tenant)
    .bind(&cohort.nb)
    .bind(&cohort.scope)
    .bind(settings.generation)
    .bind(content_digest)
    .bind(sqlx::types::Json(&cohort))
    .bind(corrections.iter().collect::<Vec<_>>())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    let s = Review {
        pool,
        cedar,
        corrections: Arc::new(corrections),
        tenant: cfg.identity.tenant.clone(),
        cohort: Arc::new(cohort),
        revision: settings.sha256.clone(),
        generation: settings.generation,
        required_werktage: settings.required_werktage,
        readers: settings.readers.clone(),
        resolvers: settings.resolvers.clone(),
    };
    let (router, doc) = api().split_for_parts();
    Ok(router
        .with_state(s.clone())
        .merge(mcp::router(s))
        .layer(middleware::from_fn(require_token))
        .merge(mako_service::openapi::serve(doc))
        .layer(Extension(ExpectedTenant(cfg.identity.tenant.clone())))
        .layer(Extension(oidc)))
}
