//! The Bundesland of each Marktlokation, cached from `marktd`.
//!
//! An Ersatzwert runs on the gap's day class, and a holiday is one only in
//! the Land that keeps it. edmd holds no address, so `marktd` derives the Land
//! from the PLZ of the MaLo's address and edmd keeps the answer per MaLo: the
//! substitution reads this table and never calls `marktd`. The row is
//! refreshed whenever `marktd` announces the MaLo (`de.markt.malo.updated`,
//! raised on every write of its master data, the address among it).

use sqlx::PgPool;

/// The cached Land of `malo_id`, or `None` when none is cached.
///
/// # Errors
///
/// The database error.
pub async fn lookup(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT bundesland FROM malo_bundesland WHERE tenant = $1 AND malo_id = $2")
        .bind(tenant)
        .bind(malo_id)
        .fetch_optional(pool)
        .await
}

/// Re-read the Land of `malo_id` at `marktd` and cache what it answers.
///
/// A Land replaces the row; a refusal (no PLZ, a PLZ across a Land border, no
/// such MaLo) removes it, so a moved MaLo never keeps the old Land.
///
/// # Errors
///
/// `marktd` unreachable or the database write failed — both worth a
/// redelivery.
pub async fn refresh(
    pool: &PgPool,
    marktd: &mako_markt::marktd_client::MarktdClient,
    tenant: &str,
    malo_id: &str,
) -> anyhow::Result<Option<String>> {
    match marktd.get_malo_bundesland(malo_id).await? {
        Ok(land) => {
            sqlx::query(
                "INSERT INTO malo_bundesland (tenant, malo_id, bundesland, postleitzahl, quelle)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (tenant, malo_id) DO UPDATE SET
                     bundesland   = EXCLUDED.bundesland,
                     postleitzahl = EXCLUDED.postleitzahl,
                     quelle       = EXCLUDED.quelle,
                     abgerufen_am = now()",
            )
            .bind(tenant)
            .bind(malo_id)
            .bind(&land.bundesland)
            .bind(&land.postleitzahl)
            .bind(&land.quelle)
            .execute(pool)
            .await?;
            Ok(Some(land.bundesland))
        }
        Err(refusal) => {
            tracing::warn!(%malo_id, %refusal, "edmd: marktd names no Bundesland for the MaLo");
            sqlx::query("DELETE FROM malo_bundesland WHERE tenant = $1 AND malo_id = $2")
                .bind(tenant)
                .bind(malo_id)
                .execute(pool)
                .await?;
            Ok(None)
        }
    }
}
