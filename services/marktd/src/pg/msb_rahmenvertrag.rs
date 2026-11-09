//! PostgreSQL repository for the MSB-Rahmenvertrag registry
//! (`msb_rahmenvertraege`), both Sparten.
//!
//! The NB's check of an Anmeldung MSB asks for the contract with the MSBN:
//! „Vorliegen eines Vertrages nach §9 Abs.1 Nr. 3 MsbG mit dem MSBN" (WiM
//! Teil 1, BK6-24-174 Anlage 2a, Kap. 2.3.2 Nr. 2 Ziff. 3, S. 24) and, in Gas,
//! „Vorliegen eines Vertrages zwischen dem MSBN und dem NB" (AWH WiM Gas 2.0
//! Kap. 3.5.2 Nr. 2 Ziff. 3, S. 20). [`PgMsbRahmenvertragRepository::pruefe`]
//! answers that question for one (Sparte, NB, MSB, day).
//!
//! Two columns are Gas-only. GeLi Gas 3.0 (BK7-24-01-009, S. 6–7) revokes
//! BK7-17-026 with effect from 01.10.2026 (Tenor 13) and obliges the parties to
//! conclude the Messstellenbetreiberrahmenvertrag „in der jeweils gültigen
//! Fassung" and to adapt contracts already concluded (Tenor 15). That Fassung
//! is KoV XV Anlage 8 (KoV Gas XV Haupttext, Stand 27.03.2026, § 2 Ziff. 2
//! lit. h), the Gas default of `fassung`; `anpassung_erforderlich` is the
//! Tenor 15 Satz 2 contract concluded before and still to be adapted.

use mako_markt::bo4e::Bo4e;
use mako_markt::domain::{MarktpartnerId, Sparte};
use rubo4e::current::Vertrag;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction, postgres::PgRow};
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

use mako_markt::error::MdmError;

/// The Gas Fassung in force from 01.10.2026: KoV Gas XV Haupttext (Stand
/// 27.03.2026) § 2 Ziff. 2 lit. h — „Messstellenbetreiberrahmenvertrag Gas
/// zwischen Netzbetreiber und Messstellenbetreiber nach § 9 Abs. 1 Nr. 3
/// Messstellenbetriebsgesetz (MsbG) (Anlage 8)".
pub const FASSUNG_GAS: &str = "KoV XV Anlage 8";

/// The Gas Fassung GeLi Gas 3.0 Tenor 13 revokes.
pub const FASSUNG_GAS_BK7_17_026: &str = "BK7-17-026";

/// The day BK7-17-026 stops carrying a Gas contract (BK7-24-01-009 Tenor 13:
/// „mit Wirkung zum 01.10.2026 widerrufen").
pub const BK7_17_026_WIDERRUFEN_AB: Date = time::macros::date!(2026 - 10 - 01);

/// Lifecycle state of an MSB framework contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MsbRvStatus {
    /// Published/offered by the NB; not yet concluded.
    Angeboten,
    /// Concluded between NB and MSB.
    Abgeschlossen,
    /// Gas only: a contract concluded before GeLi Gas 3.0 that Tenor 15 Satz 2
    /// requires to be adapted to the current Fassung.
    AnpassungErforderlich,
    /// Ended.
    Beendet,
}

impl MsbRvStatus {
    /// Every variant, in the order the SQL `CHECK` lists them.
    pub const VARIANTS: [Self; 4] = [
        Self::Angeboten,
        Self::Abgeschlossen,
        Self::AnpassungErforderlich,
        Self::Beendet,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Angeboten => "angeboten",
            Self::Abgeschlossen => "abgeschlossen",
            Self::AnpassungErforderlich => "anpassung_erforderlich",
            Self::Beendet => "beendet",
        }
    }
}

fn status_from_str(s: &str) -> Result<MsbRvStatus, sqlx::Error> {
    MsbRvStatus::VARIANTS
        .into_iter()
        .find(|v| v.as_str() == s)
        .ok_or_else(|| sqlx::Error::Decode(format!("unknown status {s}").into()))
}

fn sparte_from_str(s: &str) -> Result<Sparte, sqlx::Error> {
    s.parse::<Sparte>()
        .map_err(|e| sqlx::Error::Decode(e.into()))
}

/// An MSB framework-contract record **as stored**.
///
/// The natural key is `(tenant, sparte, nb_mp_id, msb_mp_id, valid_from)` —
/// upserts are idempotent on it and keep the `id` stable.
///
/// `vertrag` reads back as opaque JSON, the way every stored BO4E document in
/// mako does: a row may predate the current schema series. What goes *in* is
/// [`MsbRvUpsertRequest`], gated.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MsbRahmenvertrag {
    pub id: Uuid,
    pub tenant: String,
    #[schema(value_type = String, example = "GAS")]
    pub sparte: Sparte,
    #[schema(value_type = String)]
    pub nb_mp_id: MarktpartnerId,
    #[schema(value_type = String)]
    pub msb_mp_id: MarktpartnerId,
    /// Contract text edition. Gas: `KoV XV Anlage 8` unless named otherwise
    /// (legacy: `BK7-17-026`). Strom: as the parties name it, or absent.
    pub fassung: Option<String>,
    pub status: MsbRvStatus,
    #[serde(with = "time::serde::rfc3339::option")]
    pub signed_at: Option<time::OffsetDateTime>,
    /// First day of validity, inclusive.
    pub valid_from: Date,
    /// First day **no longer** valid (half-open `[valid_from, valid_to)`).
    pub valid_to: Option<Date>,
    /// The stored BO4E `Vertrag` (vertragsart `RAHMENVERTRAG`), as stored.
    pub vertrag: serde_json::Value,
    /// Optimistic-locking version, incremented on every successful write.
    pub version: i64,
}

/// The `PUT /api/v1/msb-rahmenvertraege` body.
///
/// Separate from [`MsbRahmenvertrag`] because the two directions are not
/// symmetric: what is **written** must cross the BO4E gate, and what is
/// **read** is whatever was stored.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MsbRvUpsertRequest {
    /// Optional: an id to keep stable. Absent → allocated on insert.
    #[serde(default)]
    pub id: Uuid,
    /// `STROM` or `GAS` — required: the NB holds one MP-ID per Sparte, and a
    /// contract of one Sparte never answers the other's lookup.
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    #[schema(value_type = String)]
    pub nb_mp_id: MarktpartnerId,
    #[schema(value_type = String)]
    pub msb_mp_id: MarktpartnerId,
    /// Contract text edition. Absent on a Gas row stores [`FASSUNG_GAS`];
    /// absent on a Strom row stores nothing.
    #[serde(default)]
    pub fassung: Option<String>,
    #[serde(default = "default_status")]
    pub status: MsbRvStatus,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub signed_at: Option<time::OffsetDateTime>,
    pub valid_from: Date,
    #[serde(default)]
    pub valid_to: Option<Date>,
    /// The BO4E `Vertrag` (vertragsart `RAHMENVERTRAG`), crossing
    /// [the gate](mako_markt::bo4e::decode) as it deserialises.
    ///
    /// Absent stores the empty object the column defaults to, not `null`.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub vertrag: Option<Bo4e<Vertrag>>,
    /// Optimistic-locking version; `0`/absent skips the check.
    #[serde(default)]
    pub version: i64,
}

impl MsbRvUpsertRequest {
    /// The Fassung the row stores: the caller's, else the Gas default.
    #[must_use]
    pub fn effective_fassung(&self) -> Option<String> {
        match (&self.fassung, self.sparte) {
            (Some(f), _) => Some(f.clone()),
            (None, Sparte::Gas) => Some(FASSUNG_GAS.to_owned()),
            (None, Sparte::Strom) => None,
        }
    }

    /// The refusals no column constraint states in the caller's terms.
    ///
    /// # Errors
    ///
    /// [`MdmError::Unprocessable`] naming the field.
    pub fn validate(&self) -> Result<(), MdmError> {
        if let Some(valid_to) = self.valid_to
            && valid_to <= self.valid_from
        {
            return Err(MdmError::Unprocessable {
                reason: format!(
                    "valid_to {valid_to} is not after valid_from {} — the window is \
                     [valid_from, valid_to)",
                    self.valid_from
                ),
            });
        }
        if self.status == MsbRvStatus::AnpassungErforderlich && self.sparte != Sparte::Gas {
            return Err(MdmError::Unprocessable {
                reason: "status anpassung_erforderlich is the GeLi Gas 3.0 Tenor 15 Satz 2 \
                         adaptation duty and exists only for sparte GAS"
                    .to_owned(),
            });
        }
        if self.fassung.as_deref().is_some_and(|f| f.trim().is_empty()) {
            return Err(MdmError::Unprocessable {
                reason: "fassung is empty — omit it instead".to_owned(),
            });
        }
        Ok(())
    }
}

const fn default_status() -> MsbRvStatus {
    MsbRvStatus::Angeboten
}

fn mp_id(row: &PgRow, column: &str) -> Result<MarktpartnerId, sqlx::Error> {
    let raw: String = row.try_get(column)?;
    MarktpartnerId::new(&raw).map_err(|e| sqlx::Error::Decode(e.to_string().into()))
}

fn map_row(row: &PgRow) -> Result<MsbRahmenvertrag, sqlx::Error> {
    Ok(MsbRahmenvertrag {
        id: row.try_get("id")?,
        tenant: row.try_get("tenant")?,
        sparte: sparte_from_str(row.try_get::<String, _>("sparte")?.as_str())?,
        nb_mp_id: mp_id(row, "nb_mp_id")?,
        msb_mp_id: mp_id(row, "msb_mp_id")?,
        fassung: row.try_get("fassung")?,
        status: status_from_str(row.try_get::<String, _>("status")?.as_str())?,
        signed_at: row.try_get("signed_at")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to")?,
        vertrag: row.try_get("vertrag")?,
        version: row.try_get("version")?,
    })
}

const SELECT_COLS: &str = "id, tenant, sparte, nb_mp_id, msb_mp_id, fassung, status, signed_at, \
                           valid_from, valid_to, vertrag, version";

// ── The lookup ────────────────────────────────────────────────────────────────

/// What the registry says about one (Sparte, NB, MSB, day).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MsbRvBefund {
    /// A contract with status `abgeschlossen` covers the day — the one answer
    /// that satisfies the check.
    Abgeschlossen,
    /// No contract of this Sparte between these parties covers the day —
    /// none was recorded, or the recorded one ended before it.
    KeinVertrag,
    /// The covering contract is only offered.
    Angeboten,
    /// Gas: the covering contract must be adapted to the current Fassung
    /// (GeLi Gas 3.0 Tenor 15 Satz 2) — its status says so, or it is still in
    /// the BK7-17-026 Fassung on a day from 01.10.2026 (Tenor 13).
    AnpassungErforderlich,
    /// The covering contract is marked ended.
    Beendet,
}

impl MsbRvBefund {
    /// Whether the Befund satisfies „Vorliegen eines Vertrages".
    #[must_use]
    pub const fn abgeschlossen(self) -> bool {
        matches!(self, Self::Abgeschlossen)
    }
}

/// The answer of `GET /api/v1/msb-rahmenvertraege/check`.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MsbRvPruefung {
    /// `true` only when a contract with status `abgeschlossen` is valid on
    /// `datum`.
    pub abgeschlossen: bool,
    pub befund: MsbRvBefund,
    /// The contract covering `datum`, when one does.
    pub vertrag: Option<MsbRahmenvertrag>,
}

/// Decide the Befund from the one row whose window covers the day.
///
/// Pure, so the rule is tested without a database. `covering` must already be
/// the row whose `[valid_from, valid_to)` contains `datum` — the exclusion
/// constraint `msb_rv_no_overlap` guarantees there is at most one.
#[must_use]
pub fn befund(covering: Option<&MsbRahmenvertrag>, datum: Date) -> MsbRvBefund {
    let Some(rv) = covering else {
        return MsbRvBefund::KeinVertrag;
    };
    match rv.status {
        MsbRvStatus::Angeboten => MsbRvBefund::Angeboten,
        MsbRvStatus::Beendet => MsbRvBefund::Beendet,
        MsbRvStatus::AnpassungErforderlich => MsbRvBefund::AnpassungErforderlich,
        MsbRvStatus::Abgeschlossen
            if rv.sparte == Sparte::Gas
                && rv.fassung.as_deref() == Some(FASSUNG_GAS_BK7_17_026)
                && datum >= BK7_17_026_WIDERRUFEN_AB =>
        {
            MsbRvBefund::AnpassungErforderlich
        }
        MsbRvStatus::Abgeschlossen => MsbRvBefund::Abgeschlossen,
    }
}

/// PostgreSQL-backed MSB framework-contract registry.
#[derive(Clone, Debug)]
pub struct PgMsbRahmenvertragRepository {
    pool: PgPool,
}

impl PgMsbRahmenvertragRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// The pool, for a caller that opens the write transaction itself.
    #[must_use]
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Insert or update a record by its natural key
    /// `(tenant, sparte, nb_mp_id, msb_mp_id, valid_from)` inside the caller's
    /// transaction — the handler enqueues the CloudEvent in the same one, so
    /// a stored contract and its `de.markt.msb-rahmenvertrag.updated` commit
    /// together or not at all.
    ///
    /// Optimistic locking: a non-zero `version` must match the stored one or
    /// the write fails with [`MdmError::VersionConflict`]. Returns the stable
    /// id and the new version.
    ///
    /// # Errors
    ///
    /// [`MdmError::VersionConflict`] on a stale version; a window overlapping
    /// another contract of the same parties and Sparte, or a broken `CHECK`,
    /// is [`MdmError::Unprocessable`]; anything else is
    /// [`MdmError::Internal`].
    pub async fn upsert_tx(
        tx: &mut Transaction<'_, Postgres>,
        tenant: &str,
        rec: &MsbRvUpsertRequest,
    ) -> Result<(Uuid, i64), MdmError> {
        rec.validate()?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM msb_rahmenvertraege \
             WHERE tenant = $1 AND sparte = $2 AND nb_mp_id = $3 AND msb_mp_id = $4 \
               AND valid_from = $5 \
             FOR UPDATE",
        )
        .bind(tenant)
        .bind(rec.sparte.to_string())
        .bind(rec.nb_mp_id.as_ref())
        .bind(rec.msb_mp_id.as_ref())
        .bind(rec.valid_from)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        let expected = (rec.version > 0).then_some(rec.version);
        let new_version = match (current, expected) {
            (Some(actual), Some(expected)) if actual != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
            (Some(actual), _) => actual + 1,
            (None, _) => 1,
        };

        let insert_id = if rec.id == Uuid::nil() {
            Uuid::new_v4()
        } else {
            rec.id
        };
        let vertrag = rec
            .vertrag
            .as_ref()
            .map(Bo4e::canonical_json)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))?
            // The column is `JSONB NOT NULL DEFAULT '{}'`, and JSON `null`
            // satisfies it. An absent contract is the empty object.
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
        let row = sqlx::query(
            "INSERT INTO msb_rahmenvertraege \
               (id, tenant, sparte, nb_mp_id, msb_mp_id, fassung, status, signed_at, \
                valid_from, valid_to, vertrag, version) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (tenant, sparte, nb_mp_id, msb_mp_id, valid_from) DO UPDATE SET \
               fassung    = EXCLUDED.fassung, \
               status     = EXCLUDED.status, \
               signed_at  = COALESCE(EXCLUDED.signed_at, msb_rahmenvertraege.signed_at), \
               valid_to   = EXCLUDED.valid_to, \
               vertrag    = EXCLUDED.vertrag, \
               version    = EXCLUDED.version, \
               updated_at = now() \
             RETURNING id, version",
        )
        .bind(insert_id)
        .bind(tenant)
        .bind(rec.sparte.to_string())
        .bind(rec.nb_mp_id.as_ref())
        .bind(rec.msb_mp_id.as_ref())
        .bind(rec.effective_fassung())
        .bind(rec.status.as_str())
        .bind(rec.signed_at)
        .bind(rec.valid_from)
        .bind(rec.valid_to)
        .bind(vertrag)
        .bind(new_version)
        .fetch_one(&mut **tx)
        .await
        .map_err(super::write_error)?;
        let id: Uuid = row
            .try_get("id")
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        let version: i64 = row
            .try_get("version")
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok((id, version))
    }

    /// [`Self::upsert_tx`] in a transaction of its own, for a caller with no
    /// event to commit beside it.
    ///
    /// # Errors
    ///
    /// As [`Self::upsert_tx`], plus [`MdmError::Internal`] when the
    /// transaction cannot be opened or committed.
    pub async fn upsert(
        &self,
        tenant: &str,
        rec: &MsbRvUpsertRequest,
    ) -> Result<(Uuid, i64), MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        let out = Self::upsert_tx(&mut tx, tenant, rec).await?;
        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(out)
    }

    /// Fetch by id (tenant-scoped).
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on a database failure.
    pub async fn get(&self, tenant: &str, id: Uuid) -> Result<Option<MsbRahmenvertrag>, MdmError> {
        let row = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM msb_rahmenvertraege WHERE tenant = $1 AND id = $2"
        )))
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        row.map(|r| map_row(&r))
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// List records, optionally filtered by Sparte, NB, MSB and status.
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on a database failure.
    pub async fn list(
        &self,
        tenant: &str,
        filter: &MsbRvFilter<'_>,
    ) -> Result<Vec<MsbRahmenvertrag>, MdmError> {
        let rows = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM msb_rahmenvertraege \
             WHERE tenant = $1 \
               AND ($2::text IS NULL OR sparte = $2) \
               AND ($3::text IS NULL OR nb_mp_id = $3) \
               AND ($4::text IS NULL OR msb_mp_id = $4) \
               AND ($5::text IS NULL OR status = $5) \
             ORDER BY valid_from DESC \
             LIMIT 500"
        )))
        .bind(tenant)
        .bind(filter.sparte.map(|s| s.to_string()))
        .bind(filter.nb_mp_id)
        .bind(filter.msb_mp_id)
        .bind(filter.status.map(MsbRvStatus::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        rows.iter()
            .map(|r| map_row(r).map_err(|e| MdmError::Internal(e.to_string())))
            .collect()
    }

    /// Whether a contract of `sparte` between `nb_mp_id` and `msb_mp_id` is
    /// `abgeschlossen` and valid on `datum`.
    ///
    /// Reads only the row whose half-open window covers `datum`; a row of the
    /// other Sparte, of another NB or one that ended before `datum` is not
    /// consulted, so it can never satisfy the check.
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on a database failure — never a `false`: an
    /// outage is not evidence that no contract exists.
    pub async fn pruefe(
        &self,
        tenant: &str,
        sparte: Sparte,
        nb_mp_id: &MarktpartnerId,
        msb_mp_id: &MarktpartnerId,
        datum: Date,
    ) -> Result<MsbRvPruefung, MdmError> {
        let row = sqlx::query(mako_service::sql::dynamic(format!(
            "SELECT {SELECT_COLS} FROM msb_rahmenvertraege \
             WHERE tenant = $1 AND sparte = $2 AND nb_mp_id = $3 AND msb_mp_id = $4 \
               AND valid_from <= $5 AND (valid_to IS NULL OR $5 < valid_to)"
        )))
        .bind(tenant)
        .bind(sparte.to_string())
        .bind(nb_mp_id.as_ref())
        .bind(msb_mp_id.as_ref())
        .bind(datum)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        let vertrag = row
            .map(|r| map_row(&r))
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        let befund = befund(vertrag.as_ref(), datum);
        Ok(MsbRvPruefung {
            abgeschlossen: befund.abgeschlossen(),
            befund,
            vertrag,
        })
    }
}

/// The optional filters of [`PgMsbRahmenvertragRepository::list`].
#[derive(Debug, Clone, Copy, Default)]
pub struct MsbRvFilter<'a> {
    pub sparte: Option<Sparte>,
    pub nb_mp_id: Option<&'a str>,
    pub msb_mp_id: Option<&'a str>,
    pub status: Option<MsbRvStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn rv(sparte: Sparte, status: MsbRvStatus, fassung: Option<&str>) -> MsbRahmenvertrag {
        MsbRahmenvertrag {
            id: Uuid::nil(),
            tenant: "t".to_owned(),
            sparte,
            nb_mp_id: MarktpartnerId::new("9900987654321").unwrap(),
            msb_mp_id: MarktpartnerId::new("9900357000004").unwrap(),
            fassung: fassung.map(ToOwned::to_owned),
            status,
            signed_at: None,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            vertrag: serde_json::json!({}),
            version: 1,
        }
    }

    /// Only `abgeschlossen` satisfies the check; every other status, and no
    /// row at all, is a Befund the NB escalates on.
    #[test]
    fn only_a_concluded_contract_satisfies_the_check() {
        let day = date!(2026 - 11 - 09);
        assert_eq!(befund(None, day), MsbRvBefund::KeinVertrag);
        for (status, expected) in [
            (MsbRvStatus::Abgeschlossen, MsbRvBefund::Abgeschlossen),
            (MsbRvStatus::Angeboten, MsbRvBefund::Angeboten),
            (MsbRvStatus::Beendet, MsbRvBefund::Beendet),
        ] {
            let b = befund(Some(&rv(Sparte::Strom, status, None)), day);
            assert_eq!(b, expected);
            assert_eq!(b.abgeschlossen(), status == MsbRvStatus::Abgeschlossen);
        }
        let gas = rv(
            Sparte::Gas,
            MsbRvStatus::AnpassungErforderlich,
            Some(FASSUNG_GAS),
        );
        assert!(!befund(Some(&gas), day).abgeschlossen());
    }

    /// A Gas contract still in the BK7-17-026 Fassung is concluded on a day
    /// before 01.10.2026 and to be adapted on any day from it (GeLi Gas 3.0
    /// Tenor 13/15) — it does not confirm an Anmeldung for a Zuordnungsbeginn
    /// under the new Fassung.
    #[test]
    fn a_bk7_17_026_gas_contract_needs_adapting_from_the_revocation() {
        let legacy = rv(
            Sparte::Gas,
            MsbRvStatus::Abgeschlossen,
            Some(FASSUNG_GAS_BK7_17_026),
        );
        assert_eq!(
            befund(Some(&legacy), date!(2026 - 09 - 30)),
            MsbRvBefund::Abgeschlossen
        );
        assert_eq!(
            befund(Some(&legacy), BK7_17_026_WIDERRUFEN_AB),
            MsbRvBefund::AnpassungErforderlich
        );
        let current = rv(Sparte::Gas, MsbRvStatus::Abgeschlossen, Some(FASSUNG_GAS));
        assert_eq!(
            befund(Some(&current), BK7_17_026_WIDERRUFEN_AB),
            MsbRvBefund::Abgeschlossen
        );
    }

    fn req(sparte: Sparte) -> MsbRvUpsertRequest {
        serde_json::from_value(serde_json::json!({
            "sparte": sparte,
            "nb_mp_id": "9900987654321",
            "msb_mp_id": "9900357000004",
            "valid_from": "2026-10-01",
        }))
        .expect("body decodes")
    }

    /// The Gas default is the Fassung Tenor 15 requires from 01.10.2026; a
    /// Strom row gets no Fassung it never named.
    #[test]
    fn the_fassung_default_is_gas_only() {
        assert_eq!(
            req(Sparte::Gas).effective_fassung().as_deref(),
            Some(FASSUNG_GAS)
        );
        assert_eq!(req(Sparte::Strom).effective_fassung(), None);
    }

    /// `anpassung_erforderlich` is the Gas Tenor 15 Satz 2 duty; a Strom row
    /// carrying it is refused before the database sees it.
    #[test]
    fn anpassung_erforderlich_is_refused_on_strom() {
        let mut strom = req(Sparte::Strom);
        strom.status = MsbRvStatus::AnpassungErforderlich;
        assert!(matches!(
            strom.validate(),
            Err(MdmError::Unprocessable { .. })
        ));
        let mut gas = req(Sparte::Gas);
        gas.status = MsbRvStatus::AnpassungErforderlich;
        assert!(gas.validate().is_ok());
    }

    /// The Sparte is required: a body without it cannot say which of the NB's
    /// two MP-IDs the contract is with.
    #[test]
    fn a_body_without_sparte_is_refused() {
        let err = serde_json::from_value::<MsbRvUpsertRequest>(serde_json::json!({
            "nb_mp_id": "9900987654321",
            "msb_mp_id": "9900357000004",
            "valid_from": "2026-10-01",
        }))
        .unwrap_err();
        assert!(err.to_string().contains("sparte"), "{err}");
    }
}
