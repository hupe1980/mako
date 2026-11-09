//! PostgreSQL storage: [`PgLokationsbuendelRepository`] — the read that
//! assembles a Marktlokation's Lokationsbündel on a Stichtag, and the NB's
//! `malo_lokationsbuendel_angaben`.
//!
//! The bündel is read, never derived: its objects are what the
//! `lokationszuordnungen` graph joins to the Marktlokation on the Stichtag,
//! each object's position is the `lokationsbuendelObjektcode` its own row
//! holds, and the structure is the `lokationsbuendelcode` its edges carry. A
//! fact no row holds stays `None`.

use std::collections::BTreeMap;

use mako_markt::{
    domain::Lokationstyp,
    error::MdmError,
    lokationsbuendel::{
        BuendelAngaben, BuendelObjekt, BuendelZaehler, BuendelZuordnung, MaloLokationsbuendel,
        Profilverfahren,
    },
};
use sqlx::{PgPool, Row};
use time::Date;

/// The Lokationsbündel read and `malo_lokationsbuendel_angaben`.
#[derive(Clone, Debug)]
pub struct PgLokationsbuendelRepository {
    pool: PgPool,
}

fn internal(e: impl std::fmt::Display) -> MdmError {
    MdmError::Internal(e.to_string())
}

fn typ(raw: &str) -> Result<Lokationstyp, MdmError> {
    raw.parse()
        .map_err(|e| internal(format!("stored Lokationstyp {raw:?}: {e}")))
}

impl PgLokationsbuendelRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store one row of the NB's Angaben. A row with the same `valid_from` is
    /// replaced; one overlapping another row is refused by
    /// `malo_lokationsbuendel_angaben_no_overlap`.
    ///
    /// # Errors
    ///
    /// [`MdmError::Unprocessable`] for an overlap, a `CHECK` violation or an
    /// unknown Marktlokation; [`MdmError::Internal`] otherwise.
    pub async fn upsert_angaben(
        &self,
        tenant: &str,
        malo_id: &str,
        row: &BuendelAngaben,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO malo_lokationsbuendel_angaben
                  (tenant, malo_id, valid_from, valid_to, uenb_mp_id, technische_einrichtungen,
                   standardlastprofil, profilverfahren, nna_positionen, obis, register)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
              ON CONFLICT (tenant, malo_id, valid_from) DO UPDATE
              SET valid_to = EXCLUDED.valid_to,
                  uenb_mp_id = EXCLUDED.uenb_mp_id,
                  technische_einrichtungen = EXCLUDED.technische_einrichtungen,
                  standardlastprofil = EXCLUDED.standardlastprofil,
                  profilverfahren = EXCLUDED.profilverfahren,
                  nna_positionen = EXCLUDED.nna_positionen,
                  obis = EXCLUDED.obis,
                  register = EXCLUDED.register,
                  updated_at = now()",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(row.valid_from)
        .bind(row.valid_to)
        .bind(row.uenb_mp_id.as_deref())
        .bind(row.technische_einrichtungen)
        .bind(row.standardlastprofil.as_deref())
        .bind(row.profilverfahren.map(Profilverfahren::as_str))
        .bind(to_json(&row.nna_positionen)?)
        .bind(to_json(&row.obis)?)
        .bind(to_json(&row.register)?)
        .execute(&self.pool)
        .await
        .map_err(super::write_error)?;
        Ok(())
    }

    /// The Lokationsbündel of `malo_id` on `stichtag`; `None` when marktd
    /// holds no such Marktlokation.
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on a SQL failure or a stored value that no
    /// longer reads as its type.
    pub async fn find_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Option<MaloLokationsbuendel>, MdmError> {
        let malo: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            r"SELECT energierichtung, lokationsbuendel_objektcode FROM malo WHERE malo_id = $1",
        )
        .bind(malo_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        let Some((energierichtung, malo_objektcode)) = malo else {
            return Ok(None);
        };
        let energierichtung = energierichtung
            .as_deref()
            .map(|e| {
                rubo4e::current::Energierichtung::from_wire(e)
                    .map_err(|err| internal(format!("stored energierichtung {e:?}: {err}")))
            })
            .transpose()?;

        let zuordnungen = self.zuordnungen_at(tenant, malo_id, stichtag).await?;
        let mut lokationsbuendelcodes: Vec<String> = Vec::new();
        // Every object the edges reach, by type then ID, the Marktlokation
        // itself first.
        let mut ids: BTreeMap<(Lokationstyp, String), ()> = BTreeMap::new();
        for (z, code) in &zuordnungen {
            ids.insert((z.von_typ, z.von_id.clone()), ());
            ids.insert((z.nach_typ, z.nach_id.clone()), ());
            if let Some(code) = code
                && !lokationsbuendelcodes.contains(code)
            {
                lokationsbuendelcodes.push(code.clone());
            }
        }
        ids.remove(&(Lokationstyp::Malo, malo_id.to_owned()));
        let of = |t: Lokationstyp| -> Vec<String> {
            ids.keys()
                .filter(|(k, _)| *k == t)
                .map(|(_, id)| id.clone())
                .collect()
        };
        let mut codes = self
            .objektcodes(
                tenant,
                &of(Lokationstyp::Malo),
                &of(Lokationstyp::Melo),
                &of(Lokationstyp::Nelo),
                &of(Lokationstyp::Tr),
                &of(Lokationstyp::Sr),
            )
            .await?;
        let mut objekte = vec![BuendelObjekt {
            typ: Lokationstyp::Malo,
            id: malo_id.to_owned(),
            objektcode: malo_objektcode,
        }];
        objekte.extend(ids.into_keys().map(|(t, id)| BuendelObjekt {
            objektcode: codes.remove(&(t, id.clone())).flatten(),
            typ: t,
            id,
        }));
        let melos: Vec<String> = objekte
            .iter()
            .filter(|o| o.typ == Lokationstyp::Melo)
            .map(|o| o.id.clone())
            .collect();
        let zaehler = self.zaehler_of(tenant, &melos).await?;
        let angaben = self.angaben_at(tenant, malo_id, stichtag).await?;

        Ok(Some(MaloLokationsbuendel {
            malo_id: malo_id.to_owned(),
            stichtag,
            energierichtung,
            lokationsbuendelcodes,
            objekte,
            zuordnungen: zuordnungen.into_iter().map(|(z, _)| z).collect(),
            angaben,
            zaehler,
        }))
    }

    /// Every edge valid on `stichtag` (half-open `[valid_from, valid_to)`)
    /// that joins an object reachable from `malo_id`, followed in both
    /// directions: the MeLo PUT writes `MELO → MALO`, the graph API either
    /// way, and both are the same bündel. Depth is capped at 8.
    async fn zuordnungen_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Vec<(BuendelZuordnung, Option<String>)>, MdmError> {
        let rows = sqlx::query(
            r"WITH RECURSIVE kanten AS (
                  SELECT von_id, von_typ, nach_id, nach_typ, valid_from, valid_to,
                         lokationsbuendelcode
                  FROM lokationszuordnungen
                  WHERE tenant = $1
                    AND (valid_from IS NULL OR valid_from <= $3)
                    AND (valid_to IS NULL OR valid_to > $3)
              ),
              knoten(id, tiefe) AS (
                  SELECT $2::TEXT, 0
                  UNION
                  SELECT CASE WHEN k.von_id = n.id THEN k.nach_id ELSE k.von_id END, n.tiefe + 1
                  FROM kanten k JOIN knoten n ON n.id IN (k.von_id, k.nach_id)
                  WHERE n.tiefe < 8
              )
              SELECT DISTINCT von_id, von_typ, nach_id, nach_typ, valid_from, valid_to,
                     lokationsbuendelcode
              FROM kanten
              WHERE von_id IN (SELECT id FROM knoten) AND nach_id IN (SELECT id FROM knoten)
              ORDER BY von_typ, von_id, nach_typ, nach_id",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(stichtag)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|r| {
                let von_typ: String = r.try_get("von_typ").map_err(internal)?;
                let nach_typ: String = r.try_get("nach_typ").map_err(internal)?;
                Ok((
                    BuendelZuordnung {
                        von_id: r.try_get("von_id").map_err(internal)?,
                        von_typ: typ(&von_typ)?,
                        nach_id: r.try_get("nach_id").map_err(internal)?,
                        nach_typ: typ(&nach_typ)?,
                        valid_from: r.try_get("valid_from").map_err(internal)?,
                        valid_to: r.try_get("valid_to").map_err(internal)?,
                    },
                    r.try_get("lokationsbuendelcode").map_err(internal)?,
                ))
            })
            .collect()
    }

    /// Each object's `lokationsbuendelObjektcode`: the typed column of a
    /// Markt- or Messlokation, the BO4E document of the others.
    async fn objektcodes(
        &self,
        tenant: &str,
        malos: &[String],
        melos: &[String],
        nelos: &[String],
        trs: &[String],
        srs: &[String],
    ) -> Result<BTreeMap<(Lokationstyp, String), Option<String>>, MdmError> {
        let mut out = BTreeMap::new();
        let malo: Vec<(String, Option<String>)> = sqlx::query_as(
            r"SELECT malo_id, lokationsbuendel_objektcode FROM malo WHERE malo_id = ANY($1)",
        )
        .bind(malos)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        out.extend(
            malo.into_iter()
                .map(|(id, c)| ((Lokationstyp::Malo, id), c)),
        );
        let melo: Vec<(String, Option<String>)> = sqlx::query_as(
            r"SELECT melo_id, lokationsbuendel_objektcode FROM melo WHERE melo_id = ANY($1)",
        )
        .bind(melos)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        out.extend(
            melo.into_iter()
                .map(|(id, c)| ((Lokationstyp::Melo, id), c)),
        );
        let nelo: Vec<(String, Option<String>)> = sqlx::query_as(
            r"SELECT nelo_id, data->>'lokationsbuendelObjektcode' FROM nelo
              WHERE tenant = $1 AND nelo_id = ANY($2)",
        )
        .bind(tenant)
        .bind(nelos)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        out.extend(
            nelo.into_iter()
                .map(|(id, c)| ((Lokationstyp::Nelo, id), c)),
        );
        let tr: Vec<(String, Option<String>)> = sqlx::query_as(
            r"SELECT tr_id, data->>'lokationsbuendelObjektcode' FROM technische_ressourcen
              WHERE tenant = $1 AND tr_id = ANY($2)",
        )
        .bind(tenant)
        .bind(trs)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        out.extend(tr.into_iter().map(|(id, c)| ((Lokationstyp::Tr, id), c)));
        let sr: Vec<(String, Option<String>)> = sqlx::query_as(
            r"SELECT sr_id, data->>'lokationsbuendelObjektcode' FROM steuerbare_ressourcen
              WHERE tenant = $1 AND sr_id = ANY($2)",
        )
        .bind(tenant)
        .bind(srs)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        out.extend(sr.into_iter().map(|(id, c)| ((Lokationstyp::Sr, id), c)));
        Ok(out)
    }

    /// The Zähler of `melos` with their BO4E `zaehlertypSpezifikation`.
    async fn zaehler_of(
        &self,
        tenant: &str,
        melos: &[String],
    ) -> Result<Vec<BuendelZaehler>, MdmError> {
        let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
            r"SELECT melo_id, zaehler_id, data->>'zaehlertypSpezifikation' FROM zaehler
              WHERE tenant = $1 AND melo_id = ANY($2)
              ORDER BY melo_id, zaehler_id",
        )
        .bind(tenant)
        .bind(melos)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.into_iter()
            .map(|(melo_id, zaehler_id, spez)| {
                let zaehlertyp_spezifikation = spez
                    .map(|s| {
                        serde_json::from_value(serde_json::Value::String(s.clone())).map_err(|e| {
                            internal(format!(
                                "stored zaehlertypSpezifikation {s:?} of Zähler {zaehler_id}: {e}"
                            ))
                        })
                    })
                    .transpose()?;
                Ok(BuendelZaehler {
                    melo_id,
                    zaehler_id,
                    zaehlertyp_spezifikation,
                })
            })
            .collect()
    }

    /// The Angaben row covering `stichtag`.
    async fn angaben_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Option<BuendelAngaben>, MdmError> {
        let row = sqlx::query(
            r"SELECT valid_from, valid_to, uenb_mp_id, technische_einrichtungen,
                     standardlastprofil, profilverfahren, nna_positionen, obis, register
              FROM malo_lokationsbuendel_angaben
              WHERE tenant = $1 AND malo_id = $2
                AND valid_from <= $3 AND (valid_to IS NULL OR valid_to > $3)",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(stichtag)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let verfahren: Option<String> = row.try_get("profilverfahren").map_err(internal)?;
        Ok(Some(BuendelAngaben {
            valid_from: row.try_get("valid_from").map_err(internal)?,
            valid_to: row.try_get("valid_to").map_err(internal)?,
            uenb_mp_id: row.try_get("uenb_mp_id").map_err(internal)?,
            technische_einrichtungen: row.try_get("technische_einrichtungen").map_err(internal)?,
            standardlastprofil: row.try_get("standardlastprofil").map_err(internal)?,
            profilverfahren: verfahren.as_deref().and_then(Profilverfahren::from_wire),
            nna_positionen: jsonb_list(&row, "nna_positionen", malo_id)?,
            obis: jsonb_list(&row, "obis", malo_id)?,
            register: jsonb_list(&row, "register", malo_id)?,
        }))
    }
}

/// One of the Angaben's JSONB lists, read back as its type.
fn jsonb_list<T: serde::de::DeserializeOwned>(
    row: &sqlx::postgres::PgRow,
    col: &str,
    malo_id: &str,
) -> Result<T, MdmError> {
    let v: serde_json::Value = row.try_get(col).map_err(internal)?;
    serde_json::from_value(v).map_err(|e| internal(format!("stored {col} of {malo_id}: {e}")))
}

/// A JSONB bind of a list of the Angaben.
fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value, MdmError> {
    serde_json::to_value(v).map_err(internal)
}
