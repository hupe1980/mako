//! PostgreSQL storage: [`PgUtilmdStammdatenRepository`] — the NB's
//! `malo_netznutzungsdaten` and the read that assembles a Marktlokation's
//! UTILMD Stammdaten on a Stichtag from everything marktd holds.
//!
//! The record is read, never derived: each fact comes from the one table or
//! BO4E object that holds it, and a fact no row holds stays `None`. Which of
//! them a message needs is the message builder's decision, and it refuses a
//! command for the ones that are missing.

use mako_markt::{
    domain::Sparte,
    error::MdmError,
    utilmd_stammdaten::{
        Abrechnungsvariante, Anschrift, Betriebszustand, BilanzierungStammdaten, Kunde,
        MaloUtilmdStammdaten, MaloZaehlwerk, MesslokationStammdaten, NetznutzungZahler,
        Netznutzungsdaten, Netznutzungsvertrag, ZaehlerStammdaten,
    },
};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use time::Date;

use super::PgMeloMsbRepository;

/// `malo_netznutzungsdaten` and the UTILMD Stammdaten read.
#[derive(Clone, Debug)]
pub struct PgUtilmdStammdatenRepository {
    pool: PgPool,
}

fn internal(e: impl std::fmt::Display) -> MdmError {
    MdmError::Internal(e.to_string())
}

impl PgUtilmdStammdatenRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store one row of the NB's Netznutzungsdaten. A row with the same
    /// `valid_from` is replaced; one overlapping another row is refused by
    /// `malo_netznutzungsdaten_no_overlap`.
    ///
    /// # Errors
    ///
    /// [`MdmError::Unprocessable`] for an overlap, a `CHECK` violation or an
    /// unknown Marktlokation; [`MdmError::Internal`] otherwise.
    pub async fn upsert_netznutzung(
        &self,
        tenant: &str,
        malo_id: &str,
        row: &Netznutzungsdaten,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO malo_netznutzungsdaten
                  (tenant, malo_id, valid_from, valid_to, haushaltskunde, betriebszustand,
                   abrechnungstermin, naechste_abrechnung_jahr, abrechnungsintervall_monate,
                   abrechnungsvariante, gemeinderabatt_prozent, turnusablesung,
                   netznutzungsvertrag, netznutzung_zahler)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
              ON CONFLICT (tenant, malo_id, valid_from) DO UPDATE
              SET valid_to = EXCLUDED.valid_to,
                  haushaltskunde = EXCLUDED.haushaltskunde,
                  betriebszustand = EXCLUDED.betriebszustand,
                  abrechnungstermin = EXCLUDED.abrechnungstermin,
                  naechste_abrechnung_jahr = EXCLUDED.naechste_abrechnung_jahr,
                  abrechnungsintervall_monate = EXCLUDED.abrechnungsintervall_monate,
                  abrechnungsvariante = EXCLUDED.abrechnungsvariante,
                  gemeinderabatt_prozent = EXCLUDED.gemeinderabatt_prozent,
                  turnusablesung = EXCLUDED.turnusablesung,
                  netznutzungsvertrag = EXCLUDED.netznutzungsvertrag,
                  netznutzung_zahler = EXCLUDED.netznutzung_zahler,
                  updated_at = now()",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(row.valid_from)
        .bind(row.valid_to)
        .bind(row.haushaltskunde)
        .bind(row.betriebszustand.map(Betriebszustand::as_str))
        .bind(row.abrechnungstermin.as_deref())
        .bind(row.naechste_abrechnung_jahr)
        .bind(row.abrechnungsintervall_monate)
        .bind(row.abrechnungsvariante.map(Abrechnungsvariante::as_str))
        .bind(row.gemeinderabatt_prozent)
        .bind(row.turnusablesung.as_deref())
        .bind(row.netznutzungsvertrag.map(Netznutzungsvertrag::as_str))
        .bind(row.netznutzung_zahler.map(NetznutzungZahler::as_str))
        .execute(&self.pool)
        .await
        .map_err(super::write_error)?;
        Ok(())
    }

    /// The Netznutzungsdaten row covering `stichtag`.
    async fn netznutzung_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Option<Netznutzungsdaten>, MdmError> {
        let row = sqlx::query(
            r"SELECT valid_from, valid_to, haushaltskunde, betriebszustand, abrechnungstermin,
                     naechste_abrechnung_jahr, abrechnungsintervall_monate, abrechnungsvariante,
                     gemeinderabatt_prozent, turnusablesung, netznutzungsvertrag,
                     netznutzung_zahler
              FROM malo_netznutzungsdaten
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
        let betriebszustand: Option<String> = row.try_get("betriebszustand").map_err(internal)?;
        let variante: Option<String> = row.try_get("abrechnungsvariante").map_err(internal)?;
        let vertrag: Option<String> = row.try_get("netznutzungsvertrag").map_err(internal)?;
        let zahler: Option<String> = row.try_get("netznutzung_zahler").map_err(internal)?;
        Ok(Some(Netznutzungsdaten {
            valid_from: row.try_get("valid_from").map_err(internal)?,
            valid_to: row.try_get("valid_to").map_err(internal)?,
            haushaltskunde: row.try_get("haushaltskunde").map_err(internal)?,
            betriebszustand: betriebszustand
                .as_deref()
                .and_then(Betriebszustand::from_wire),
            abrechnungstermin: row.try_get("abrechnungstermin").map_err(internal)?,
            naechste_abrechnung_jahr: row.try_get("naechste_abrechnung_jahr").map_err(internal)?,
            abrechnungsintervall_monate: row
                .try_get("abrechnungsintervall_monate")
                .map_err(internal)?,
            abrechnungsvariante: variante.as_deref().and_then(Abrechnungsvariante::from_wire),
            gemeinderabatt_prozent: row
                .try_get::<Option<Decimal>, _>("gemeinderabatt_prozent")
                .map_err(internal)?,
            turnusablesung: row.try_get("turnusablesung").map_err(internal)?,
            netznutzungsvertrag: vertrag.as_deref().and_then(Netznutzungsvertrag::from_wire),
            netznutzung_zahler: zahler.as_deref().and_then(NetznutzungZahler::from_wire),
        }))
    }

    /// The UTILMD Stammdaten of `malo_id` on `stichtag`; `None` when marktd
    /// holds no such Marktlokation.
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on a SQL failure or a stored BO4E document that
    /// no longer reads as its type.
    pub async fn find_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Option<MaloUtilmdStammdaten>, MdmError> {
        let malo_row = sqlx::query(r"SELECT sparte, data FROM malo WHERE malo_id = $1")
            .bind(malo_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?;
        let Some(malo_row) = malo_row else {
            return Ok(None);
        };
        let sparte: String = malo_row.try_get("sparte").map_err(internal)?;
        let sparte = match sparte.as_str() {
            "GAS" => Sparte::Gas,
            _ => Sparte::Strom,
        };
        let data: serde_json::Value = malo_row.try_get("data").map_err(internal)?;
        let malo: rubo4e::current::Marktlokation = serde_json::from_value(data)
            .map_err(|e| internal(format!("stored Marktlokation {malo_id}: {e}")))?;

        let bilanzierung = self.bilanzierung_at(tenant, malo_id, stichtag).await?;
        let netznutzung = self.netznutzung_at(tenant, malo_id, stichtag).await?;
        let messlokationen = self.messlokationen_at(tenant, malo_id, stichtag).await?;

        Ok(Some(MaloUtilmdStammdaten {
            malo_id: malo_id.to_owned(),
            sparte,
            stichtag,
            netzebene: malo.netzebene,
            gasqualitaet: malo.gasqualitaet,
            marktgebiet: malo.marktgebiet.as_ref().map(ToString::to_string),
            anschrift: malo.lokationsadresse.as_ref().map(anschrift),
            kunde: malo.endkunde.as_deref().map(|gp| Kunde {
                nachname: gp.nachname.clone(),
                vorname: gp.vorname.clone(),
                organisationsname: gp.organisationsname.clone(),
                anschrift: gp.adresse.as_ref().map(anschrift),
            }),
            zaehlwerke: malo
                .zaehlwerke
                .iter()
                .flatten()
                .filter_map(|zw| {
                    let obis = zw.obis_kennzahl.as_ref()?.to_string();
                    let ka = zw.konzessionsabgabe.as_ref();
                    Some(MaloZaehlwerk {
                        obis_kennzahl: obis,
                        konzessionsabgabe_satz: ka.and_then(|k| k.satz),
                        konzessionsabgabe_eur_kwh: ka.and_then(|k| k.kosten),
                    })
                })
                .collect(),
            bilanzierung,
            netznutzung,
            messlokationen,
        }))
    }

    /// The BO4E `Bilanzierung` effective at the start of `stichtag`
    /// (Europe/Berlin), reduced to what a UTILMD record states.
    async fn bilanzierung_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Option<BilanzierungStammdaten>, MdmError> {
        let at = mako_fristen::berlin_midnight(stichtag);
        let data: Option<(serde_json::Value,)> = sqlx::query_as(
            r"SELECT data FROM bilanzierungen
              WHERE tenant = $1 AND malo_id = $2
                AND bilanzierungsbeginn <= $3
                AND (bilanzierungsende IS NULL OR bilanzierungsende > $3)
              ORDER BY bilanzierungsbeginn DESC
              LIMIT 1",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(at)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        let Some((data,)) = data else {
            return Ok(None);
        };
        let b: rubo4e::current::Bilanzierung = serde_json::from_value(data)
            .map_err(|e| internal(format!("stored Bilanzierung of {malo_id}: {e}")))?;
        let jahresmenge = match b.jahresverbrauchsprognose.as_ref() {
            Some(menge) => menge_in_kwh(menge),
            None => None,
        };
        Ok(Some(BilanzierungStammdaten {
            prognosegrundlage: b.prognosegrundlage,
            profiltypen: b.details_prognosegrundlage.unwrap_or_default(),
            jahresverbrauchsprognose_kwh: jahresmenge,
            fallgruppe: b.fallgruppenzuordnung,
        }))
    }

    /// Every Messlokation the Lokationszuordnung graph assigns to `malo_id`
    /// on `stichtag`, with its Messstellenbetrieb and its Zähler.
    async fn messlokationen_at(
        &self,
        tenant: &str,
        malo_id: &str,
        stichtag: Date,
    ) -> Result<Vec<MesslokationStammdaten>, MdmError> {
        let melos: Vec<(String,)> = sqlx::query_as(
            r"SELECT DISTINCT von_id FROM lokationszuordnungen
              WHERE tenant = $1 AND von_typ = 'MELO' AND nach_typ = 'MALO' AND nach_id = $2
                AND (valid_from IS NULL OR valid_from <= $3)
                AND (valid_to IS NULL OR valid_to > $3)
              ORDER BY von_id",
        )
        .bind(tenant)
        .bind(malo_id)
        .bind(stichtag)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;

        let msb_repo = PgMeloMsbRepository::new(self.pool.clone());
        let mut out = Vec::with_capacity(melos.len());
        for (melo_id,) in melos {
            let messstellenbetrieb = msb_repo
                .find_messstellenbetrieb_at(tenant, &melo_id, stichtag)
                .await?;
            let zaehler = self.zaehler_of(tenant, &melo_id, stichtag).await?;
            out.push(MesslokationStammdaten {
                melo_id,
                messstellenbetrieb,
                zaehler,
            });
        }
        Ok(out)
    }

    /// The Zähler of `melo_id`, each with the OBIS-Kennzahlen of its registers
    /// active on `stichtag`.
    async fn zaehler_of(
        &self,
        tenant: &str,
        melo_id: &str,
        stichtag: Date,
    ) -> Result<Vec<ZaehlerStammdaten>, MdmError> {
        let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
            r"SELECT zaehler_id, data FROM zaehler
              WHERE tenant = $1 AND melo_id = $2
              ORDER BY zaehler_id",
        )
        .bind(tenant)
        .bind(melo_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut out = Vec::with_capacity(rows.len());
        for (zaehler_id, data) in rows {
            let z: rubo4e::current::Zaehler = serde_json::from_value(data)
                .map_err(|e| internal(format!("stored Zaehler {zaehler_id}: {e}")))?;
            let obis: Vec<(String,)> = sqlx::query_as(
                r"SELECT obis_kennzahl FROM zaehler_register
                  WHERE tenant = $1 AND zaehler_id = $2 AND obis_kennzahl IS NOT NULL
                    AND valid_from <= $3 AND (valid_to IS NULL OR valid_to > $3)
                  ORDER BY bezeichnung",
            )
            .bind(tenant)
            .bind(&zaehler_id)
            .bind(stichtag)
            .fetch_all(&self.pool)
            .await
            .map_err(internal)?;
            out.push(ZaehlerStammdaten {
                zaehlernummer: z.zaehlernummer.clone(),
                zaehlertyp: z.zaehlertyp,
                zaehlergroesse: z.zaehlergroesse,
                fernauslesbar: z.ist_fernauslesbar,
                obis_kennzahlen: obis.into_iter().map(|(o,)| o).collect(),
                zaehler_id,
            });
        }
        Ok(out)
    }
}

/// A BO4E `Adresse` as a UTILMD `NAD` carries it.
fn anschrift(a: &rubo4e::current::Adresse) -> Anschrift {
    Anschrift {
        strasse: a.strasse.clone(),
        hausnummer: a.hausnummer.clone(),
        postleitzahl: a.postleitzahl.clone(),
        ort: a.ort.clone(),
        ortsteil: a.ortsteil.clone(),
        landescode: a
            .landescode
            .as_ref()
            .and_then(|l| serde_json::to_value(l).ok())
            .and_then(|v| v.as_str().map(str::to_owned)),
    }
}

/// A BO4E `Menge` in kWh — `None` for any other unit, which is not
/// converted: the UTILMD Jahresmenge is stated in kWh.
fn menge_in_kwh(menge: &rubo4e::current::Menge) -> Option<Decimal> {
    let einheit = serde_json::to_value(menge.einheit.as_ref()?).ok()?;
    (einheit.as_str()? == "KWH").then_some(menge.wert?)
}
