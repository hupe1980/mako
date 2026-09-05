//! Rahmenverträge, Versorgungsverträge and their supply components.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::Date;
use uuid::Uuid;

use super::{RahmenvertragRow, VersorgungsvertragRow, VertragskomponenteRow};
use crate::{domain, outbound};

// ── Input types ───────────────────────────────────────────────────────────────

/// Create a B2B framework contract.
#[derive(Debug, Deserialize)]
pub struct CreateRahmenvertragInput {
    pub gueltig_von: Date,
    pub gueltig_bis: Option<Date>,
    pub kuendigungsfrist_monate: Option<i32>,
    pub auto_renewal: Option<bool>,
    pub renewal_monate: Option<i32>,
    pub preisanpassungsformel: Option<String>,
    pub portfolio_rabatt_prozent: Option<rust_decimal::Decimal>,
    pub rechnungsstellung: Option<String>, // EINZEL | SAMMEL | POSITIONEN
    pub sammelrechnung_intervall: Option<String>,
    pub erp_rahmenvertrag_id: Option<String>,
    /// Traceability link to the B2B Angebot that led here (CPQ pipeline).
    pub angebot_id: Option<Uuid>,
    pub notizen: Option<String>,
}

/// Create a supply contract with its commodity components.
#[derive(Debug, Deserialize)]
pub struct CreateVersorgungsvertragInput {
    pub rahmenvertrag_id: Option<Uuid>, // B2B: usually set; B2C: None
    pub kundentyp: String,
    /// GRUNDVERSORGUNG | ERSATZVERSORGUNG | SONDERVERTRAG. Defaults to
    /// SONDERVERTRAG — the regime with the least statutory privilege, so an
    /// omission cannot silently claim Grundversorgungs-Fristen.
    pub vertragsart: Option<String>,
    pub bundle_code: Option<String>,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub kuendigungsfrist_monate: Option<i32>,
    pub preisgarantie_bis: Option<Date>,
    pub abrechnungszyklus: Option<String>,
    pub auto_renewal: Option<bool>,
    /// `0` (the default) extends into an unbefristeten Vertrag — the only
    /// lawful tacit extension of a consumer contract (§ 309 Nr. 9 lit. b BGB).
    pub renewal_monate: Option<i32>,
    pub standort_bezeichnung: Option<String>,
    /// BO4E `Adresse` of the supply location.
    pub standort_adresse: Option<serde_json::Value>,
    pub zahlungsziel_tage: Option<i32>,
    pub erp_contract_id: Option<String>,
    pub notizen: Option<String>,
    pub komponenten: Vec<CreateKomponenteInput>,
}

/// One commodity position of a supply contract.
#[derive(Debug, Deserialize)]
pub struct CreateKomponenteInput {
    pub sparte: String,
    pub malo_id: Option<String>,
    /// Messlokation. Mandatory for GAS: `start-supply-gas` carries it as the
    /// Zählpunktbezeichnung (RFF+Z13).
    pub melo_id: Option<String>,
    pub nb_mp_id: Option<String>,
    pub product_code: String,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
    pub fulfillment_data: Option<serde_json::Value>,
}

/// Terminate a contract.
#[derive(Debug, Deserialize)]
pub struct KuendigungInput {
    /// The day supply ends.
    pub lieferende: Date,
    /// Why — it decides the notice period. See [`domain::Kuendigungsgrund`].
    #[serde(default = "default_grund")]
    pub grund: domain::Kuendigungsgrund,
    /// For a § 41 Abs. 5 Satz 4 Sonderkündigung: the day the announced price
    /// change takes effect. The termination ends the contract exactly then.
    #[serde(default)]
    pub preisanpassung_wirksam_zum: Option<Date>,
    /// When the customer's notice arrived. Defaults to today; an operator
    /// entering a letter that arrived last week must be able to say so,
    /// because the notice period runs from receipt, not from data entry.
    #[serde(default)]
    pub eingang: Option<Date>,
    pub bemerkung: Option<String>,
}

const fn default_grund() -> domain::Kuendigungsgrund {
    domain::Kuendigungsgrund::Ordentlich
}

/// Change the product of one component.
#[derive(Debug, Deserialize)]
pub struct TarifwechselInput {
    /// UUID of the Vertragskomponente to be re-tariffed.
    pub komp_id: Uuid,
    /// New product code in `productd`.
    pub new_product_code: String,
    /// When the new tariff takes effect.
    pub wirksamkeit: Date,
    /// Who is changing the tariff — `"LIEFERANT"` or `"KUNDE"`.
    ///
    /// Required, with no default: the two are legally different acts. A
    /// supplier-initiated change is an exercise of a reserved change right and
    /// owes the § 41 Abs. 5 Satz 1 EnWG notice with the Satz 4
    /// Sonderkündigungsrecht; a switch the customer asked for is an agreed
    /// change and is confirmed instead. Guessing either way misstates the
    /// customer's rights, so the caller says which it is.
    pub initiator: crate::pg::produkte::Initiator,
    pub grund: Option<String>,
    /// Operator override: bypass the `preisgarantie_bis` contract lock.
    /// Only for operators with a documented price-lock waiver; every use is
    /// logged to `preisgarantie_override_log`.
    #[serde(default)]
    pub override_preisgarantie: bool,
    /// § 41 Abs. 5 Satz 3 EnWG — the **Umfang** of the change, line by line, as
    /// the notice will state it.
    ///
    /// Supplied here because the caller chose the new tariff and therefore
    /// holds both price sheets, and because what the notice said is a fact
    /// about the notice: a catalogue lookup years later answers what the price
    /// *is*, not what the customer was *told*. `vertragd` never asks `productd`
    /// (BILLING.md § 3), and this is why it does not have to.
    ///
    /// **Mandatory for a supplier-initiated future change where this deployment
    /// renders the Preisänderungsanzeige itself** (`outputd_url` configured):
    /// the lines are the document's content, so the change is refused without
    /// them. Where the CloudEvent is the notice, the ERP composing the letter
    /// states the Umfang from its own price sheets and these lines are
    /// optional — the event carries them when they are given and marks them
    /// absent when they are not. A retroactive correction and a switch the
    /// customer asked for announce nothing and need none.
    #[serde(default)]
    pub preise: Vec<AngekuendigterPreis>,
}

/// One price line as the § 41 Abs. 5 notice states it changing.
///
/// Amounts are decimal strings — the same convention the document views use, so
/// the value travels from the API to the page without ever becoming a float.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AngekuendigterPreis {
    /// What is priced, as the customer's tariff names it — `"Arbeitspreis"`,
    /// `"Grundpreis"`, `"Arbeitspreis HT"`.
    pub bezeichnung: String,
    /// The unit both amounts are in — `"ct/kWh"`, `"EUR/Jahr"`.
    pub einheit: String,
    /// What it costs today.
    pub bisher: String,
    /// What it will cost from `wirksamkeit`.
    pub neu: String,
}

// ── Insert results ────────────────────────────────────────────────────────────

/// A component row as it exists after insert — carries the real primary key the
/// MaKo dispatch needs, not a request-body echo.
#[derive(Debug, Clone)]
pub struct InsertedKomponente {
    pub id: Uuid,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub melo_id: Option<String>,
    pub nb_mp_id: Option<String>,
    pub lieferbeginn: Date,
}

/// Result of [`insert_versorgungsvertrag`].
#[derive(Debug, Clone)]
pub struct InsertedVertrag {
    pub id: Uuid,
    pub vertrags_nr: String,
    pub is_new: bool,
    pub komponenten: Vec<InsertedKomponente>,
    /// How many Lieferbeginn tasks the insert enqueued. Zero on an idempotent
    /// replay, which is what stops a re-POST firing a second UTILMD.
    pub dispatched: usize,
}

// ── Rahmenvertrag ─────────────────────────────────────────────────────────────

/// Create a framework contract, or return the one already carrying
/// `erp_rahmenvertrag_id`.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn insert_rahmenvertrag(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    input: &CreateRahmenvertragInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO rahmenvertraege
         (id,kunden_id,tenant,gueltig_von,gueltig_bis,
          kuendigungsfrist_monate,auto_renewal,renewal_monate,
          preisanpassungsformel,portfolio_rabatt_prozent,
          rechnungsstellung,sammelrechnung_intervall,erp_rahmenvertrag_id,angebot_id,notizen)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
         ON CONFLICT (tenant,erp_rahmenvertrag_id) WHERE erp_rahmenvertrag_id IS NOT NULL
           DO UPDATE SET updated_at=now()
         RETURNING id",
    )
    .bind(id)
    .bind(kunden_id)
    .bind(tenant)
    .bind(input.gueltig_von)
    .bind(input.gueltig_bis)
    .bind(input.kuendigungsfrist_monate.unwrap_or(3))
    .bind(input.auto_renewal.unwrap_or(true))
    .bind(input.renewal_monate.unwrap_or(12))
    .bind(&input.preisanpassungsformel)
    .bind(input.portfolio_rabatt_prozent.as_ref())
    .bind(input.rechnungsstellung.as_deref().unwrap_or("EINZEL"))
    .bind(&input.sammelrechnung_intervall)
    .bind(&input.erp_rahmenvertrag_id)
    .bind(input.angebot_id)
    .bind(&input.notizen)
    .fetch_one(pool)
    .await?;
    Ok(row.try_get("id")?)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_rahmenvertrag(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> Result<Option<RahmenvertragRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM rahmenvertraege WHERE id=$1 AND tenant=$2")
            .bind(id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?,
    )
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_all_rahmenvertraege(
    pool: &PgPool,
    tenant: &str,
    status: Option<&str>,
    limit: i64,
) -> Result<Vec<RahmenvertragRow>> {
    Ok(sqlx::query_as(
        r"SELECT * FROM rahmenvertraege
          WHERE tenant = $1
            AND ($2::TEXT IS NULL OR status = $2)
          ORDER BY gueltig_von DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(status)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_rahmenvertraege_by_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<RahmenvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM rahmenvertraege WHERE kunden_id=$1 AND tenant=$2 ORDER BY gueltig_von DESC",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

/// One active supply site of a Rahmenvertrag — what a Sammelrechnung enumerates.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RahmenvertragMaloRow {
    pub malo_id: String,
    pub sparte: String,
    /// The product this site is on **today**, or `None` when its supply has
    /// not started yet.
    ///
    /// A site whose Lieferbeginn is still ahead must still appear — a bundle
    /// that silently omits it under-bills — so this is an outer lookup, and
    /// billingd resolves the per-period product through
    /// `GET /api/v1/malo/{malo_id}/produkte` anyway.
    pub product_code: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub kundentyp: String,
}

/// List active MaLos of a Rahmenvertrag with the product each is on.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_rahmenvertrag_malos(
    pool: &PgPool,
    rahmenvertrag_id: Uuid,
    tenant: &str,
) -> Result<Vec<RahmenvertragMaloRow>> {
    Ok(sqlx::query_as::<_, RahmenvertragMaloRow>(
        r"SELECT k.malo_id, k.sparte, p.product_code,
                 vv.standort_bezeichnung, ku.kundentyp
          FROM vertragskomponenten k
          JOIN versorgungsvertraege vv ON vv.id = k.vertrag_id
          JOIN kunden ku               ON ku.id = vv.kunden_id
          LEFT JOIN LATERAL (
              SELECT product_code FROM komponenten_produkte
               WHERE komp_id = k.id AND gueltig_von <= heute()
                 AND (gueltig_bis IS NULL OR gueltig_bis > heute())
               LIMIT 1
          ) p ON TRUE
          WHERE vv.rahmenvertrag_id = $1
            AND vv.tenant           = $2
            AND k.status IN ('AKTIV', 'BESTAETIGT')
            AND k.malo_id IS NOT NULL
          ORDER BY k.malo_id",
    )
    .bind(rahmenvertrag_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_versorgungsvertraege_by_rahmenvertrag(
    pool: &PgPool,
    rahmenvertrag_id: Uuid,
    tenant: &str,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM versorgungsvertraege
         WHERE rahmenvertrag_id = $1 AND tenant = $2
           AND status IN ('AKTIV', 'TEILERFUELLUNG', 'GEKÜNDIGT')
         ORDER BY vertragsbeginn",
    )
    .bind(rahmenvertrag_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

// ── Versorgungsvertrag ────────────────────────────────────────────────────────

/// Create a supply contract with its components and their Lieferbeginn tasks —
/// all in one transaction.
///
/// The contract, its components and the intent to register them at the NB
/// either all exist or none do. Inserting the components outside a transaction
/// left contracts with half their commodities on any failure, and dispatching
/// from a detached task lost the registration on any restart.
///
/// Idempotent on `erp_contract_id`: a replay returns the existing contract with
/// `is_new = false`, no components and nothing enqueued.
///
/// # Errors
///
/// Propagates storage errors, and refuses a gas component with no Messlokation
/// (see [`outbound::lieferbeginn`]).
pub async fn insert_versorgungsvertrag(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
    lf_mp_id: &str,
    input: &CreateVersorgungsvertragInput,
) -> Result<InsertedVertrag> {
    let mut tx = pool.begin().await?;
    let id = Uuid::new_v4();
    let row = sqlx::query(
        "INSERT INTO versorgungsvertraege
         (id,kunden_id,rahmenvertrag_id,tenant,kundentyp,vertragsart,bundle_code,
          vertragsbeginn,vertragsende,kuendigungsfrist_monate,
          preisgarantie_bis,abrechnungszyklus,auto_renewal,renewal_monate,
          standort_bezeichnung,standort_adresse,zahlungsziel_tage,erp_contract_id,notizen)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19)
         ON CONFLICT (tenant,erp_contract_id) WHERE erp_contract_id IS NOT NULL
           DO UPDATE SET updated_at=now()
         RETURNING id, vertrags_nr, id = $1 AS is_new_insert",
    )
    .bind(id)
    .bind(kunden_id)
    .bind(input.rahmenvertrag_id)
    .bind(tenant)
    .bind(&input.kundentyp)
    .bind(input.vertragsart.as_deref().unwrap_or("SONDERVERTRAG"))
    .bind(&input.bundle_code)
    .bind(input.vertragsbeginn)
    .bind(input.vertragsende)
    .bind(input.kuendigungsfrist_monate.unwrap_or(1))
    .bind(input.preisgarantie_bis)
    .bind(input.abrechnungszyklus.as_deref().unwrap_or("JAEHRLICH"))
    .bind(input.auto_renewal.unwrap_or(false))
    .bind(input.renewal_monate.unwrap_or(0))
    .bind(&input.standort_bezeichnung)
    .bind(&input.standort_adresse)
    .bind(input.zahlungsziel_tage)
    .bind(&input.erp_contract_id)
    .bind(&input.notizen)
    .fetch_one(&mut *tx)
    .await?;

    let actual_id: Uuid = row.try_get("id")?;
    let vertrags_nr: String = row.try_get("vertrags_nr")?;
    // `id = $1` is true only for a genuine insert; false on conflict.
    let is_new: bool = row.try_get("is_new_insert").unwrap_or(true);

    let mut komponenten = Vec::new();
    let mut dispatched = 0usize;
    if is_new {
        for komp in &input.komponenten {
            let komp_id = insert_komponente(&mut tx, actual_id, tenant, lf_mp_id, komp).await?;
            // Persist-before-dispatch: the registration intent commits with the
            // contract, so it survives a restart and can be retried.
            if VertragskomponenteRow::requires_mako_workflow(&komp.sparte)
                && let (Some(malo_id), Some(nb_mp_id)) = (&komp.malo_id, &komp.nb_mp_id)
            {
                let task = outbound::lieferbeginn(
                    komp_id,
                    &komp.sparte,
                    malo_id,
                    komp.melo_id.as_deref(),
                    nb_mp_id,
                    lf_mp_id,
                    komp.lieferbeginn,
                )?;
                if outbound::enqueue(&mut *tx, tenant, &task).await? {
                    dispatched += 1;
                }
            }
            komponenten.push(InsertedKomponente {
                id: komp_id,
                sparte: komp.sparte.clone(),
                malo_id: komp.malo_id.clone(),
                melo_id: komp.melo_id.clone(),
                nb_mp_id: komp.nb_mp_id.clone(),
                lieferbeginn: komp.lieferbeginn,
            });
        }
        if dispatched > 0 {
            sqlx::query(
                "UPDATE versorgungsvertraege SET status='IN_BEARBEITUNG', updated_at=now()
                 WHERE id=$1 AND tenant=$2 AND status='ANGELEGT'",
            )
            .bind(actual_id)
            .bind(tenant)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(InsertedVertrag {
        id: actual_id,
        vertrags_nr,
        is_new,
        komponenten,
        dispatched,
    })
}

async fn insert_komponente(
    conn: &mut sqlx::PgConnection,
    vertrag_id: Uuid,
    tenant: &str,
    lf_mp_id: &str,
    k: &CreateKomponenteInput,
) -> Result<Uuid> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO vertragskomponenten
         (id,vertrag_id,tenant,sparte,malo_id,melo_id,lf_mp_id,nb_mp_id,
          lieferbeginn,lieferende,fulfillment_data)
         SELECT $1,$2,v.tenant,$3,$4,$5,$6,$7,$8,$9,$10
         FROM versorgungsvertraege v WHERE v.id=$2",
    )
    .bind(id)
    .bind(vertrag_id)
    .bind(&k.sparte)
    .bind(&k.malo_id)
    .bind(&k.melo_id)
    .bind(lf_mp_id)
    .bind(&k.nb_mp_id)
    .bind(k.lieferbeginn)
    .bind(k.lieferende)
    .bind(&k.fulfillment_data)
    .execute(&mut *conn)
    .await?;
    // The product a component starts on is its first valid-time slice. Written
    // in the same transaction, so a component never exists without one.
    crate::pg::produkte::open_initial(&mut *conn, tenant, id, &k.product_code, k.lieferbeginn)
        .await?;
    Ok(id)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_vertrag(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> Result<Option<VersorgungsvertragRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM versorgungsvertraege WHERE id=$1 AND tenant=$2")
            .bind(id)
            .bind(tenant)
            .fetch_optional(pool)
            .await?,
    )
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_komponenten(
    pool: &PgPool,
    vertrag_id: Uuid,
) -> Result<Vec<VertragskomponenteRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE vertrag_id=$1 ORDER BY created_at")
            .bind(vertrag_id)
            .fetch_all(pool)
            .await?,
    )
}

/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_komponente(pool: &PgPool, id: Uuid) -> Result<Option<VertragskomponenteRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE id=$1")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_offene_vertraege(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM versorgungsvertraege
         WHERE tenant=$1 AND status IN ('ANGELEGT','IN_BEARBEITUNG','TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
         ORDER BY created_at LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_vertraege_by_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        "SELECT * FROM versorgungsvertraege WHERE kunden_id=$1 AND tenant=$2
         ORDER BY vertragsbeginn DESC",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

/// Contracts with a Kündigung whose Lieferende is still ahead.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_pending_kuendigungen(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
) -> Result<Vec<VersorgungsvertragRow>> {
    Ok(sqlx::query_as(
        r"SELECT * FROM versorgungsvertraege
          WHERE tenant = $1
            AND status = 'GEKÜNDIGT'
            AND (kuendigung_zum IS NULL OR kuendigung_zum >= heute())
          ORDER BY kuendigung_zum ASC NULLS LAST
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// All MaLos a customer currently has supply at.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_aktive_malo_ids(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT k.malo_id FROM vertragskomponenten k
         JOIN versorgungsvertraege v ON v.id = k.vertrag_id
         WHERE v.kunden_id=$1 AND v.tenant=$2
           AND k.status IN ('AKTIV','BESTAETIGT')
           AND k.malo_id IS NOT NULL",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(m,)| m).collect())
}

/// The active Versorgungsvertrag delivering to a MaLo, with its component.
///
/// The lookup `billingd` uses to put § 40 Abs. 1 EnWG contract facts on the
/// invoice. Newest active contract wins when a MaLo re-contracted within the
/// tenant.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_vertrag_by_malo(
    pool: &PgPool,
    malo_id: &str,
    tenant: &str,
) -> Result<Option<(VersorgungsvertragRow, VertragskomponenteRow)>> {
    let vertrag: Option<VersorgungsvertragRow> = sqlx::query_as(
        "SELECT v.* FROM versorgungsvertraege v
         JOIN vertragskomponenten k ON k.vertrag_id = v.id
         WHERE k.malo_id=$1 AND v.tenant=$2
           AND v.status IN ('TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
           AND k.status IN ('AKTIV','BESTAETIGT')
         ORDER BY v.vertragsbeginn DESC LIMIT 1",
    )
    .bind(malo_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    let Some(vertrag) = vertrag else {
        return Ok(None);
    };
    // The same status filter as the contract lookup. Without it a later
    // ABGELEHNT/STORNIERT retry row for the same MaLo won the ORDER BY and fed
    // billingd the § 40 facts of a component that never went into supply.
    let komponente: Option<VertragskomponenteRow> = sqlx::query_as(
        "SELECT * FROM vertragskomponenten
         WHERE vertrag_id=$1 AND malo_id=$2
           AND status IN ('AKTIV','BESTAETIGT')
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(vertrag.id)
    .bind(malo_id)
    .fetch_optional(pool)
    .await?;
    Ok(komponente.map(|k| (vertrag, k)))
}

/// One active supply component with its contract's § 40b billing cadence —
/// the unit of work for billingd's billing-run worker.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BillingCandidateRow {
    pub malo_id: String,
    pub lf_mp_id: String,
    pub nb_mp_id: Option<String>,
    pub sparte: String,
    /// § 40b EnWG cadence chosen on the contract.
    pub abrechnungszyklus: String,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
}

/// All active supply components eligible for scheduled billing (§ 40b EnWG).
///
/// A component is in supply from the moment the MaKo Lieferbeginn is confirmed.
/// `BESTAETIGT` is that state — nothing ever promotes it to `AKTIV` — so
/// requiring `AKTIV` alone left the § 40b billing feed permanently empty. Every
/// other reader already accepts both.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_billing_candidates(
    pool: &PgPool,
    tenant: &str,
) -> Result<Vec<BillingCandidateRow>> {
    Ok(sqlx::query_as::<_, BillingCandidateRow>(
        "SELECT k.malo_id, k.lf_mp_id, k.nb_mp_id, k.sparte,
                v.abrechnungszyklus, v.vertragsbeginn, v.vertragsende,
                k.lieferbeginn, k.lieferende
         FROM versorgungsvertraege v
         JOIN vertragskomponenten k ON k.vertrag_id = v.id
         WHERE v.tenant = $1
           AND v.status IN ('TEILERFUELLUNG','AKTIV','GEKÜNDIGT')
           AND k.status IN ('AKTIV','BESTAETIGT')
           AND k.malo_id IS NOT NULL
         ORDER BY k.malo_id",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

/// Per-MaLo summary for a B2B portfolio overview.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PortfolioItemRow {
    pub vertrag_id: Uuid,
    pub vertrags_nr: String,
    pub standort_bezeichnung: Option<String>,
    pub sparte: String,
    pub malo_id: Option<String>,
    /// `None` for a component whose supply has not started yet.
    pub product_code: Option<String>,
    pub lieferbeginn: Date,
    pub lieferende: Option<Date>,
    pub status: String,
    pub vertrag_status: String,
}

/// # Errors
///
/// Propagates storage errors.
pub async fn list_portfolio_by_kunde(
    pool: &PgPool,
    kunden_id: Uuid,
    tenant: &str,
) -> Result<Vec<PortfolioItemRow>> {
    Ok(sqlx::query_as::<_, PortfolioItemRow>(
        r"SELECT k.vertrag_id, v.vertrags_nr, v.standort_bezeichnung,
                 k.sparte, k.malo_id, p.product_code,
                 k.lieferbeginn, k.lieferende, k.status,
                 v.status AS vertrag_status
          FROM vertragskomponenten k
          JOIN versorgungsvertraege v ON v.id = k.vertrag_id
          LEFT JOIN LATERAL (
              SELECT product_code FROM komponenten_produkte
               WHERE komp_id = k.id AND gueltig_von <= heute()
                 AND (gueltig_bis IS NULL OR gueltig_bis > heute())
               LIMIT 1
          ) p ON TRUE
          WHERE v.kunden_id = $1 AND v.tenant = $2
            AND k.status IN ('AKTIV','BESTAETIGT','ANGEMELDET')
          ORDER BY v.standort_bezeichnung, k.sparte",
    )
    .bind(kunden_id)
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}

// ── Status transitions ────────────────────────────────────────────────────────

/// Statuses a Versorgungsvertrag never leaves.
///
/// A late or replayed MaKo outcome re-derives the contract status from its
/// components, and that derivation knows nothing about a Kündigung or a
/// Stornierung — it happily returned AKTIV for a contract that had already
/// ended. Terminal states are therefore not overwritten.
const VERTRAG_TERMINAL: &str = "('GEKÜNDIGT','ABGELAUFEN','STORNIERT')";

/// Statuses a Vertragskomponente never leaves.
///
/// A replayed rejection flipped an already-confirmed or already-ended component
/// to ABGELEHNT, which took it out of the billing feed retroactively.
const KOMPONENTE_TERMINAL: &str = "('BEENDET','ABGELEHNT','STORNIERT')";

/// # Errors
///
/// Propagates storage errors.
pub async fn update_vertrag_status(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
    status: &str,
) -> Result<()> {
    // A terminal contract may still progress between terminal states
    // (GEKÜNDIGT → ABGELAUFEN once every component has ended), but it never
    // returns to a live one.
    sqlx::query(&format!(
        "UPDATE versorgungsvertraege SET status=$1, updated_at=now(),
         completed_at = CASE WHEN $1 IN ('ABGELAUFEN','STORNIERT') THEN now() ELSE completed_at END
         WHERE id=$2 AND tenant=$3
           AND (status NOT IN {VERTRAG_TERMINAL} OR $1 IN {VERTRAG_TERMINAL})"
    ))
    .bind(status)
    .bind(id)
    .bind(tenant)
    .execute(executor)
    .await?;
    Ok(())
}

/// # Errors
///
/// Propagates storage errors.
pub async fn update_komponente_status(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    status: &str,
    mako_process_id: Option<&str>,
    malo_id: Option<&str>,
    erc: Option<&str>,
    reason: Option<&str>,
) -> Result<()> {
    // The rejection detail is written only by a rejection. Overwriting it with
    // NULL on every later update erased the ERC code the customer was told.
    sqlx::query(&format!(
        "UPDATE vertragskomponenten SET status=$1, updated_at=now(),
         mako_process_id=COALESCE($2,mako_process_id),
         malo_id=COALESCE($3,malo_id),
         abgelehnt_erc=COALESCE($4,abgelehnt_erc),
         abgelehnt_reason=COALESCE($5,abgelehnt_reason)
         WHERE id=$6 AND status NOT IN {KOMPONENTE_TERMINAL}"
    ))
    .bind(status)
    .bind(mako_process_id)
    .bind(malo_id)
    .bind(erc)
    .bind(reason)
    .bind(id)
    .execute(executor)
    .await?;
    Ok(())
}

/// Record the day a component's supply ends.
///
/// The status only follows once that day has passed. A Kündigung filed three
/// months ahead does not end supply today: the customer is still being supplied
/// and still has to be invoiced for it, and marking the component `BEENDET` at
/// once took it out of the § 40b billing feed and out of the § 40 Abs. 1 invoice
/// facts — so the remaining months, and the Schlussrechnung itself, had no
/// billable component behind them. [`close_due_supply`] makes the transition on
/// the date.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn schedule_lieferende(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    lieferende: Date,
) -> Result<()> {
    sqlx::query(&format!(
        "UPDATE vertragskomponenten
         SET lieferende = $2,
             status = CASE WHEN $2 < heute() THEN 'BEENDET' ELSE status END,
             updated_at = now()
         WHERE id=$1 AND status NOT IN {KOMPONENTE_TERMINAL}"
    ))
    .bind(id)
    .bind(lieferende)
    .execute(executor)
    .await?;
    Ok(())
}

/// Move supply that has run out into its terminal state, and close the
/// contracts behind it.
///
/// Returns the contracts that reached `ABGELAUFEN`. Nothing else performed this
/// transition: a Kündigung set the end date, and the contract then sat in
/// `GEKÜNDIGT` for ever with components that were still nominally in supply.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn close_due_supply(pool: &PgPool, tenant: &str) -> Result<Vec<Uuid>> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE vertragskomponenten
            SET status = 'BEENDET', updated_at = now()
          WHERE tenant = $1
            AND status IN ('AKTIV','BESTAETIGT')
            AND lieferende IS NOT NULL
            AND lieferende < heute()",
    )
    .bind(tenant)
    .execute(&mut *tx)
    .await?;

    // A contract is over when nothing under it is in supply or on its way
    // there. `completed_at` is what the § 147 AO retention clock counts from.
    let closed: Vec<(Uuid,)> = sqlx::query_as(
        "UPDATE versorgungsvertraege v
            SET status = 'ABGELAUFEN', completed_at = now(), updated_at = now()
          WHERE v.tenant = $1
            AND v.status IN ('AKTIV','TEILERFUELLUNG','GEKÜNDIGT')
            AND EXISTS (SELECT 1 FROM vertragskomponenten k WHERE k.vertrag_id = v.id)
            AND NOT EXISTS (
                SELECT 1 FROM vertragskomponenten k
                 WHERE k.vertrag_id = v.id
                   AND k.status IN ('ANGELEGT','ANGEMELDET','BESTAETIGT','AKTIV')
            )
          RETURNING v.id",
    )
    .bind(tenant)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(closed.into_iter().map(|(id,)| id).collect())
}

/// Recompute a contract's status from its components' statuses.
///
/// The order of the arms is the semantics: a contract with any supply running
/// is live, whatever else happened around it; one where every commodity was
/// rejected never came into being; one where every commodity has ended is over.
#[must_use]
pub fn derive_vertrag_status(komponenten: &[VertragskomponenteRow]) -> &'static str {
    if komponenten.is_empty() {
        return "ANGELEGT";
    }
    let mut aktiv = 0;
    let mut offen = 0;
    let mut abgelehnt = 0;
    let mut beendet = 0;
    let mut storniert = 0;
    for k in komponenten {
        match k.status.as_str() {
            "AKTIV" | "BESTAETIGT" => aktiv += 1,
            "ANGELEGT" | "ANGEMELDET" => offen += 1,
            "ABGELEHNT" => abgelehnt += 1,
            "BEENDET" => beendet += 1,
            _ => storniert += 1,
        }
    }
    match () {
        // Supply is running somewhere and something is still pending.
        () if aktiv > 0 && offen > 0 => "TEILERFUELLUNG",
        () if aktiv > 0 => "AKTIV",
        // Nothing runs and nothing is pending: how it ended decides which
        // terminal state it is.
        () if offen == 0 && beendet > 0 => "ABGELAUFEN",
        () if offen == 0 && abgelehnt > 0 => "ABGELEHNT",
        () if offen == 0 && storniert > 0 => "STORNIERT",
        // Something is still pending and nothing runs yet.
        () if komponenten.iter().any(|k| k.status == "ANGEMELDET") => "IN_BEARBEITUNG",
        () => "ANGELEGT",
    }
}

// ── Kündigung ─────────────────────────────────────────────────────────────────

/// What [`kuendige_vertrag`] did.
#[derive(Debug, Clone)]
pub struct KuendigungResult {
    /// Components whose Lieferende UTILMD was enqueued.
    pub dispatched: Vec<Uuid>,
    /// Components ended directly (no MaKo workflow).
    pub direkt_beendet: Vec<Uuid>,
}

/// Terminate a contract: record the Kündigung, end or de-register every live
/// component, and enqueue the Lieferende UTILMDs — atomically.
///
/// The whole termination is one transaction so a contract can never be
/// GEKÜNDIGT with its Lieferende un-dispatched, or vice versa. The contract's
/// `vertragsende` is set to the Kündigungstermin: it is what the § 40 Abs. 1
/// EnWG invoice facts, the expiry monitor and the portal all read, and leaving
/// it untouched meant a terminated contract still advertised its old term.
///
/// # Errors
///
/// Propagates storage errors. The caller has already validated the notice
/// period against [`domain::kuendigungsfrist`].
pub async fn kuendige_vertrag(
    tx: &mut sqlx::PgConnection,
    vertrag: &VersorgungsvertragRow,
    input: &KuendigungInput,
    eingang: Date,
    lf_mp_id: &str,
) -> Result<KuendigungResult> {
    let komponenten: Vec<VertragskomponenteRow> =
        sqlx::query_as("SELECT * FROM vertragskomponenten WHERE vertrag_id=$1")
            .bind(vertrag.id)
            .fetch_all(&mut *tx)
            .await?;

    let mut dispatched = Vec::new();
    let mut direkt_beendet = Vec::new();
    for k in komponenten
        .iter()
        .filter(|k| matches!(k.status.as_str(), "AKTIV" | "BESTAETIGT"))
    {
        match (k.is_mako(), k.malo_id.as_deref(), k.nb_mp_id.as_deref()) {
            (true, Some(malo_id), Some(nb_mp_id)) => {
                // The Schlussablesung is the LF's own obligation and does not
                // depend on the Lieferende reaching the NB, so it is its own
                // task — a processd outage must not cost the customer the
                // reading their Schlussrechnung is built from.
                let ablesung = outbound::ablesung(k.id, malo_id, true, input.lieferende);
                // Superseding, not plain enqueue: a Kündigung widerrufen and
                // re-issued to a different date must replace the reading order
                // it scheduled, not sit behind it.
                outbound::enqueue_superseding(&mut *tx, &vertrag.tenant, &ablesung).await?;
                let task = outbound::lieferende(
                    k.id,
                    &k.sparte,
                    malo_id,
                    nb_mp_id,
                    lf_mp_id,
                    input.lieferende,
                );
                outbound::enqueue(&mut *tx, &vertrag.tenant, &task).await?;
                // The component leaves supply on the agreed date either way;
                // the UTILMD tells the NB, it does not decide the contract.
                schedule_lieferende(&mut *tx, k.id, input.lieferende).await?;
                dispatched.push(k.id);
            }
            _ => {
                schedule_lieferende(&mut *tx, k.id, input.lieferende).await?;
                direkt_beendet.push(k.id);
            }
        }
    }

    sqlx::query(
        "UPDATE versorgungsvertraege
            SET status = 'GEKÜNDIGT',
                vertragsende = $3,
                kuendigung_zum = $3,
                kuendigung_grund = $4,
                kuendigung_eingang = $5,
                -- A terminated contract must not silently renew itself while
                -- its notice period runs out.
                auto_renewal = false,
                notizen = COALESCE($6, notizen),
                updated_at = now()
          WHERE id = $1 AND tenant = $2",
    )
    .bind(vertrag.id)
    .bind(&vertrag.tenant)
    .bind(input.lieferende)
    .bind(input.grund.as_db())
    .bind(eingang)
    .bind(&input.bemerkung)
    .execute(&mut *tx)
    .await?;

    Ok(KuendigungResult {
        dispatched,
        direkt_beendet,
    })
}

/// Record that the § 41 Abs. 8 Nr. 2 EnWG Textform confirmation went out.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn mark_kuendigung_bestaetigt(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    tenant: &str,
) -> Result<()> {
    sqlx::query(
        "UPDATE versorgungsvertraege
            SET kuendigungsbestaetigung_am = now(), updated_at = now()
          WHERE id = $1 AND tenant = $2 AND kuendigungsbestaetigung_am IS NULL",
    )
    .bind(id)
    .bind(tenant)
    .execute(executor)
    .await?;
    Ok(())
}

/// Revert a GEKÜNDIGT contract to AKTIV (Widerruf der Kündigung).
///
/// Valid only while the Lieferende is still ahead. Reverts exactly the
/// components this Kündigung ended — reverting every BEENDET component put one
/// that had ended for its own reasons, possibly years earlier, back into supply
/// and nulled the lieferende that said when it left.
///
/// # Errors
///
/// Refuses a contract that is not GEKÜNDIGT, or whose supply has already ended.
pub async fn widerruf_kuendigung(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    tenant: &str,
) -> Result<()> {
    let vertrag: Option<(String, Option<Date>)> = sqlx::query_as(
        "SELECT status, kuendigung_zum FROM versorgungsvertraege WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(&mut *conn)
    .await?;

    let (status, zum) = vertrag.ok_or_else(|| anyhow::anyhow!("Vertrag {id} not found"))?;
    if status != "GEKÜNDIGT" {
        anyhow::bail!(
            "Kündigung Widerruf only allowed for GEKÜNDIGT contracts, current status: {status}"
        );
    }
    let heute = mako_fristen::heute();
    if let Some(zum) = zum
        && zum <= heute
    {
        anyhow::bail!(
            "Kündigung Widerruf only allowed before the Lieferende — supply ended on {zum}"
        );
    }

    // Revert exactly what this Kündigung scheduled — components whose supply
    // has not run out yet. A component that ended for its own reasons, possibly
    // years earlier, must not be put back into supply with its lieferende
    // nulled.
    sqlx::query(
        "UPDATE vertragskomponenten
         SET status = 'AKTIV', lieferende = NULL, updated_at = now()
         WHERE vertrag_id = $1
           AND status IN ('AKTIV','BESTAETIGT','BEENDET')
           AND lieferende IS NOT NULL AND lieferende > heute()",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;

    sqlx::query(
        "UPDATE versorgungsvertraege
         SET status = 'AKTIV', vertragsende = NULL,
             kuendigung_zum = NULL, kuendigung_grund = NULL,
             kuendigung_eingang = NULL, kuendigungsbestaetigung_am = NULL,
             updated_at = now()
         WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Cancel a contract that has not yet gone into supply.
///
/// The guard lives in SQL, not only in the handler, so no other caller can
/// bypass it: a contract in AKTIV/TEILERFUELLUNG carries a confirmed MaKo
/// commitment, and cancelling it here would contradict the NB-confirmed
/// Lieferbeginn — that path is Kündigung, not Stornierung.
///
/// # Errors
///
/// Refuses a contract already in supply.
pub async fn storniere_vertrag(pool: &PgPool, id: Uuid, tenant: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE versorgungsvertraege SET status = 'STORNIERT', completed_at = now(), updated_at = now()
         WHERE id = $1 AND tenant = $2
           AND status IN ('ANGELEGT','IN_BEARBEITUNG')",
    )
    .bind(id)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        anyhow::bail!(
            "Stornierung refused: contract is not in ANGELEGT/IN_BEARBEITUNG \
             (active supply must be terminated via Kündigung)"
        );
    }
    sqlx::query(
        r"UPDATE vertragskomponenten
          SET status = 'STORNIERT', updated_at = now()
          WHERE vertrag_id = $1
            AND tenant = $2
            AND status NOT IN ('AKTIV','BEENDET','BESTAETIGT','STORNIERT')",
    )
    .bind(id)
    .bind(tenant)
    .execute(&mut *tx)
    .await?;
    // A registration that has not left the queue yet must not leave it now:
    // the customer cancelled before supply began, so there is nothing to
    // register. One that already reached processd is the operator's to cancel
    // there — which is exactly what the response says.
    sqlx::query(
        r"UPDATE outbound_tasks
          SET dead_lettered_at = now(),
              last_error = 'Vertrag storniert vor Lieferbeginn'
          WHERE kind = 'LIEFERBEGINN'
            AND completed_at IS NULL AND dead_lettered_at IS NULL
            AND komp_id IN (SELECT id FROM vertragskomponenten WHERE vertrag_id = $1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── Tarifwechsel / Preisgarantie ─────────────────────────────────────────────

/// Store or replace the BO4E `Preisgarantie` for a contract.
///
/// Also updates `preisgarantie_bis`, which the Tarifwechsel guard reads without
/// loading the JSONB.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn upsert_preisgarantie(
    executor: impl sqlx::PgExecutor<'_>,
    vertrag_id: Uuid,
    tenant: &str,
    preisgarantie: &serde_json::Value,
    bis: Option<Date>,
) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE versorgungsvertraege
         SET preisgarantie = $3, preisgarantie_bis = $4, updated_at = now()
         WHERE id = $1 AND tenant = $2",
    )
    .bind(vertrag_id)
    .bind(tenant)
    .bind(preisgarantie)
    .bind(bis)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_preisgarantie(
    pool: &PgPool,
    vertrag_id: Uuid,
    tenant: &str,
) -> Result<Option<serde_json::Value>> {
    Ok(sqlx::query_scalar::<_, Option<serde_json::Value>>(
        "SELECT preisgarantie FROM versorgungsvertraege WHERE id=$1 AND tenant=$2",
    )
    .bind(vertrag_id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?
    .flatten())
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn komp(status: &str) -> VertragskomponenteRow {
        VertragskomponenteRow {
            id: Uuid::new_v4(),
            vertrag_id: Uuid::nil(),
            tenant: "t".into(),
            sparte: "STROM".into(),
            malo_id: None,
            melo_id: None,
            lf_mp_id: "LF".into(),
            nb_mp_id: None,
            lieferbeginn: date!(2026 - 01 - 01),
            lieferende: None,
            status: status.into(),
            mako_process_id: None,
            abgelehnt_erc: None,
            abgelehnt_reason: None,
            ablese_auftrag_id: None,
            fulfillment_data: None,
        }
    }

    #[test]
    fn no_components_means_the_contract_is_only_angelegt() {
        assert_eq!(derive_vertrag_status(&[]), "ANGELEGT");
    }

    #[test]
    fn running_supply_makes_the_contract_aktiv() {
        assert_eq!(
            derive_vertrag_status(&[komp("BESTAETIGT"), komp("AKTIV")]),
            "AKTIV"
        );
    }

    #[test]
    fn one_commodity_still_pending_is_teilerfuellung() {
        assert_eq!(
            derive_vertrag_status(&[komp("BESTAETIGT"), komp("ANGEMELDET")]),
            "TEILERFUELLUNG"
        );
    }

    #[test]
    fn a_rejection_beside_running_supply_does_not_end_the_contract() {
        assert_eq!(
            derive_vertrag_status(&[komp("BESTAETIGT"), komp("ABGELEHNT")]),
            "AKTIV"
        );
    }

    #[test]
    fn every_commodity_rejected_is_abgelehnt_not_storniert() {
        // A registration the NB refused is not a cancellation by the customer,
        // and the two are answered differently.
        assert_eq!(derive_vertrag_status(&[komp("ABGELEHNT")]), "ABGELEHNT");
    }

    #[test]
    fn every_commodity_ended_is_abgelaufen() {
        assert_eq!(
            derive_vertrag_status(&[komp("BEENDET"), komp("BEENDET")]),
            "ABGELAUFEN"
        );
    }

    #[test]
    fn an_ended_commodity_outweighs_a_rejected_one() {
        // Supply ran and then ended; that the second commodity was never
        // accepted does not turn the contract into a rejection.
        assert_eq!(
            derive_vertrag_status(&[komp("BEENDET"), komp("ABGELEHNT")]),
            "ABGELAUFEN"
        );
    }

    #[test]
    fn every_commodity_cancelled_is_storniert() {
        assert_eq!(derive_vertrag_status(&[komp("STORNIERT")]), "STORNIERT");
    }

    #[test]
    fn a_dispatched_registration_is_in_bearbeitung() {
        assert_eq!(
            derive_vertrag_status(&[komp("ANGEMELDET")]),
            "IN_BEARBEITUNG"
        );
    }
}
