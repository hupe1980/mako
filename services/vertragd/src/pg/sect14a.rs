//! The § 14a EnWG module state of a supply component, valid-time.
//!
//! BK8-22/010-A numbers three Netzentgelt modules for a steuerbare
//! Verbrauchseinrichtung: **Modul 1** (pauschale Reduzierung) and **Modul 2**
//! (prozentuale Reduzierung des Arbeitspreises) are the two forms the base
//! module takes, and **Modul 3** (zeitvariable Netzentgelte) is offered only in
//! combination with Modul 1. Which of them the Anschlussnutzer holds is a fact
//! about his supply, recorded here and read by `billingd`, which selects the
//! reductions it bills from it and never from the product alone.
//!
//! The state is stored as recorded, not normalised: a combination the
//! Festlegung does not offer (Modul 1 with Modul 2, Modul 2 with Modul 3) stays
//! representable so the billing engine's guards refuse it where it would be
//! billed, rather than the record silently dropping one module.
//!
//! Half-open `[gueltig_von, gueltig_bis)`, as `komponenten_produkte`: a change
//! of module ends the slice in force and opens the next on the same day.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::Date;
use uuid::Uuid;

/// One of the three § 14a Netzentgelt modules of BK8-22/010-A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Sect14aModul {
    /// Pauschale Reduzierung des Netzentgelts.
    #[serde(rename = "MODUL_1")]
    Modul1,
    /// Prozentuale Reduzierung des Arbeitspreises.
    #[serde(rename = "MODUL_2")]
    Modul2,
    /// Zeitvariable Netzentgelte, three Tarifstufen HT/ST/NT.
    #[serde(rename = "MODUL_3")]
    Modul3,
}

impl Sect14aModul {
    /// Every value, in the order of the CHECK list.
    pub const ALL: [Self; 3] = [Self::Modul1, Self::Modul2, Self::Modul3];

    /// The stored value.
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Modul1 => "MODUL_1",
            Self::Modul2 => "MODUL_2",
            Self::Modul3 => "MODUL_3",
        }
    }
}

/// One slice of a MaLo's § 14a module state, clipped to the asked period.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Sect14aSlice {
    pub malo_id: String,
    /// The modules held, sorted, each once.
    pub module: Vec<String>,
    pub gueltig_von: Date,
    /// Exclusive; `None` = open-ended.
    pub gueltig_bis: Option<Date>,
}

/// Body of `POST /api/v1/malo/{malo_id}/sect14a`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sect14aInput {
    /// The modules the Anschlussnutzer holds from `gueltig_ab`; at least one.
    pub module: Vec<Sect14aModul>,
    /// The first day the state applies.
    pub gueltig_ab: Date,
}

/// The module set a contract or an order states for one Strom component.
///
/// The customer chooses „gegenüber dem Lieferanten" at conclusion
/// (BK8-22/010-A, Begründung Rn. 148, S. 54), so the choice arrives with the
/// contract and is recorded from its Lieferbeginn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sect14aIntake {
    pub module: Vec<Sect14aModul>,
}

/// Why an intake module set is refused — new data is held to the Festlegung,
/// unlike a recorded state, which stays representable for billing to refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sect14aIntakeFehler {
    /// An empty set states nothing.
    Leer,
    /// § 14a EnWG covers steuerbare Verbrauchseinrichtungen in the
    /// electricity grid.
    NurStrom,
    /// Modul 2 is chosen „als Alternative zum Modul 1" (Tenor 2 a), and
    /// Modul 3 only „in Ergänzung zu Modul 1" (Tenor 3 a): Modul 1 with
    /// Modul 3 is the only pair, and Modul 3 never stands alone.
    Kombination,
}

impl Sect14aIntakeFehler {
    /// The stable `code` of the `422`.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Leer => "sect14a_leer",
            Self::NurStrom => "sect14a_nur_strom",
            Self::Kombination => "sect14a_kombination_unzulaessig",
        }
    }
}

impl Sect14aIntake {
    /// The module set, sorted and each once, when `sparte` may carry it.
    ///
    /// # Errors
    ///
    /// [`Sect14aIntakeFehler`] naming the rule that refuses it.
    pub fn pruefe(
        &self,
        sparte: &str,
    ) -> std::result::Result<Vec<Sect14aModul>, Sect14aIntakeFehler> {
        let mut module = self.module.clone();
        module.sort();
        module.dedup();
        if module.is_empty() {
            return Err(Sect14aIntakeFehler::Leer);
        }
        if sparte != "STROM" {
            return Err(Sect14aIntakeFehler::NurStrom);
        }
        let hat = |m| module.contains(&m);
        let (m1, m2, m3) = (
            hat(Sect14aModul::Modul1),
            hat(Sect14aModul::Modul2),
            hat(Sect14aModul::Modul3),
        );
        // The three sets the Festlegung offers: Modul 1, Modul 2, Modul 1 + 3.
        if !matches!(
            (m1, m2, m3),
            (true, false, false) | (false, true, false) | (true, false, true)
        ) {
            return Err(Sect14aIntakeFehler::Kombination);
        }
        Ok(module)
    }
}

/// Write the first module slice of a component created with one, on the
/// contract's transaction, from its Lieferbeginn — never earlier: „Ein
/// rückwirkender Modulwechsel ist ausgeschlossen" (BK8-22/010-A Rn. 145, S. 53).
///
/// # Errors
///
/// Propagates storage errors.
pub async fn open_initial(
    conn: &mut sqlx::PgConnection,
    tenant: &str,
    komp_id: Uuid,
    module: &[Sect14aModul],
    ab: Date,
) -> Result<()> {
    let module: Vec<&'static str> = module.iter().map(|m| m.as_db()).collect();
    sqlx::query(
        "INSERT INTO komponenten_sect14a (tenant, komp_id, module, gueltig_von)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant)
    .bind(komp_id)
    .bind(&module)
    .bind(ab)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Why a module state could not be recorded.
#[derive(Debug)]
pub enum Sect14aError {
    /// No billable supply component carries the MaLo.
    KeineKomponente(String),
    /// An empty module list states nothing.
    KeinModul,
    /// A later state is already recorded; history is not rewritten here.
    SpaetererStand(Date),
    Db(sqlx::Error),
}

impl std::fmt::Display for Sect14aError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeineKomponente(malo) => {
                write!(f, "keine Vertragskomponente beliefert MaLo {malo}")
            }
            Self::KeinModul => f.write_str("mindestens ein § 14a-Modul ist anzugeben"),
            Self::SpaetererStand(d) => {
                write!(f, "ein späterer § 14a-Modulstand beginnt bereits am {d}")
            }
            Self::Db(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Sect14aError {}

impl From<sqlx::Error> for Sect14aError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}

/// Record the module state of `malo_id`'s supply component from `input.gueltig_ab`.
///
/// Closes the slice in force at that day and opens the new one, in one
/// transaction. A state recorded for the same first day is replaced.
///
/// # Errors
///
/// [`Sect14aError`] — no component, no module, or a later state already
/// recorded.
pub async fn record_sect14a(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    input: &Sect14aInput,
) -> std::result::Result<Vec<String>, Sect14aError> {
    let mut module: Vec<Sect14aModul> = input.module.clone();
    module.sort();
    module.dedup();
    if module.is_empty() {
        return Err(Sect14aError::KeinModul);
    }
    let module: Vec<&'static str> = module.into_iter().map(Sect14aModul::as_db).collect();
    let ab = input.gueltig_ab;

    let mut tx = pool.begin().await?;
    let komp: Option<(Uuid,)> = sqlx::query_as(mako_service::sql::dynamic(format!(
        "SELECT k.id FROM vertragskomponenten k
           JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          WHERE k.malo_id = $1 AND k.tenant = $2
            AND k.status IN {komponente}
          ORDER BY v.vertragsbeginn DESC, k.created_at DESC LIMIT 1",
        komponente = crate::pg::vertraege::KOMPONENTE_BILLABLE,
    )))
    .bind(malo_id)
    .bind(tenant)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((komp_id,)) = komp else {
        return Err(Sect14aError::KeineKomponente(malo_id.to_owned()));
    };

    let spaeter: Option<(Date,)> = sqlx::query_as(
        "SELECT gueltig_von FROM komponenten_sect14a
          WHERE komp_id = $1 AND gueltig_von > $2
          ORDER BY gueltig_von LIMIT 1",
    )
    .bind(komp_id)
    .bind(ab)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((spaeter,)) = spaeter {
        return Err(Sect14aError::SpaetererStand(spaeter));
    }
    sqlx::query("DELETE FROM komponenten_sect14a WHERE komp_id = $1 AND gueltig_von = $2")
        .bind(komp_id)
        .bind(ab)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE komponenten_sect14a SET gueltig_bis = $2
          WHERE komp_id = $1 AND gueltig_von < $2
            AND (gueltig_bis IS NULL OR gueltig_bis > $2)",
    )
    .bind(komp_id)
    .bind(ab)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO komponenten_sect14a (tenant, komp_id, module, gueltig_von)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant)
    .bind(komp_id)
    .bind(&module)
    .bind(ab)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(module.into_iter().map(str::to_owned).collect())
}

/// The module-state slices of `malo_id` overlapping `[von, bis]` (inclusive),
/// clipped to it, in order.
///
/// # Errors
///
/// Propagates storage errors; refuses a reversed period.
pub async fn malo_sect14a(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    von: Date,
    bis: Date,
) -> Result<Vec<Sect14aSlice>> {
    anyhow::ensure!(von <= bis, "von ({von}) darf nicht nach bis ({bis}) liegen");
    let bis_exkl = bis.next_day().unwrap_or(bis);
    Ok(
        sqlx::query_as::<_, Sect14aSlice>(mako_service::sql::dynamic(format!(
            "SELECT k.malo_id, s.module,
                    GREATEST(s.gueltig_von, $3) AS gueltig_von,
                    CASE WHEN s.gueltig_bis IS NULL OR s.gueltig_bis > $4
                         THEN $4 ELSE s.gueltig_bis END AS gueltig_bis
               FROM komponenten_sect14a s
               JOIN vertragskomponenten k ON k.id = s.komp_id
              WHERE s.tenant = $1 AND k.malo_id = $2
                AND k.status IN {komponente}
                AND daterange(s.gueltig_von, s.gueltig_bis, '[)')
                    && daterange($3::DATE, $4::DATE, '[)')
              ORDER BY s.gueltig_von",
            komponente = crate::pg::vertraege::KOMPONENTE_BILLABLE,
        )))
        .bind(tenant)
        .bind(malo_id)
        .bind(von)
        .bind(bis_exkl)
        .fetch_all(pool)
        .await?,
    )
}

#[cfg(test)]
mod intake_tests {
    use super::{Sect14aIntake, Sect14aIntakeFehler, Sect14aModul::*};

    /// BK8-22/010-A offers Modul 1, Modul 2, and Modul 3 beside Modul 1 —
    /// every other set is refused, and none on another Sparte.
    #[test]
    fn only_the_three_offered_sets_pass() {
        let pruefe = |m: Vec<_>, sparte| Sect14aIntake { module: m }.pruefe(sparte);
        assert!(pruefe(vec![Modul1], "STROM").is_ok());
        assert!(pruefe(vec![Modul2], "STROM").is_ok());
        assert_eq!(
            pruefe(vec![Modul3, Modul1, Modul1], "STROM"),
            Ok(vec![Modul1, Modul3])
        );
        for set in [
            vec![Modul1, Modul2],
            vec![Modul2, Modul3],
            vec![Modul3],
            vec![Modul1, Modul2, Modul3],
        ] {
            assert_eq!(
                pruefe(set.clone(), "STROM"),
                Err(Sect14aIntakeFehler::Kombination),
                "{set:?}"
            );
        }
        assert_eq!(
            pruefe(vec![Modul1], "GAS"),
            Err(Sect14aIntakeFehler::NurStrom)
        );
        assert_eq!(pruefe(vec![], "STROM"), Err(Sect14aIntakeFehler::Leer));
    }
}
