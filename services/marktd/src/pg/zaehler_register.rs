//! PostgreSQL storage: [`PgZaehlzeitRepository`] for `marktd`.

use mako_markt::{
    error::MdmError,
    repository::{
        ZaehlzeitRegisterRecord, ZaehlzeitRegisterUpsert, ZaehlzeitSaisonRecord,
        ZaehlzeitSaisonUpsert,
    },
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Clone)]
pub struct PgZaehlzeitRepository {
    pool: PgPool,
}

impl PgZaehlzeitRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PgZaehlzeitRepository {
    /// Create or update the register of `zaehler_id` the body identifies, and
    /// return it as stored.
    pub async fn upsert_register(
        &self,
        zaehler_id: &str,
        tenant: &str,
        body: &ZaehlzeitRegisterUpsert,
    ) -> Result<ZaehlzeitRegisterRecord, MdmError> {
        let row = sqlx::query(
            r"INSERT INTO zaehler_register
                  (zaehler_id, tenant, bezeichnung, zaehlerauspraegung,
                   obis_kennzahl, einheit, valid_from, valid_to, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
              ON CONFLICT (zaehler_id, tenant, bezeichnung, valid_from) DO UPDATE
              SET zaehlerauspraegung = EXCLUDED.zaehlerauspraegung,
                  obis_kennzahl      = EXCLUDED.obis_kennzahl,
                  einheit            = EXCLUDED.einheit,
                  valid_to           = EXCLUDED.valid_to,
                  updated_at         = now()
              RETURNING id, zaehler_id, tenant, bezeichnung, zaehlerauspraegung,
                        obis_kennzahl, einheit, valid_from, valid_to, updated_at",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .bind(&body.bezeichnung)
        .bind(body.zaehlerauspraegung.as_str())
        .bind(&body.obis_kennzahl)
        .bind(&body.einheit)
        .bind(body.valid_from)
        .bind(body.valid_to)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        row_to_register(&row)
    }

    /// Return all registers for a given `zaehler_id`.
    pub async fn list_registers_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlzeitRegisterRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT id, zaehler_id, tenant, bezeichnung, zaehlerauspraegung,
                     obis_kennzahl, einheit, valid_from, valid_to, updated_at
              FROM zaehler_register
              WHERE zaehler_id = $1 AND tenant = $2
              ORDER BY valid_from DESC",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter().map(row_to_register).collect()
    }

    /// Create or update a window of `register_id`, which must be one of
    /// `tenant`'s registers, and return it as stored.
    ///
    /// # Errors
    /// [`MdmError::NotFound`] when `tenant` holds no register `register_id`.
    ///
    /// Upsert one season window of a register.
    ///
    /// `zaehler_saisons` carries no tenant of its own — it is scoped through
    /// its `zaehler_register` parent, so the row is written only against a
    /// register of the caller's tenant: a register id from another tenant's
    /// path writes nothing and is refused as not found.
    pub async fn upsert_saison(
        &self,
        register_id: Uuid,
        tenant: &str,
        body: &ZaehlzeitSaisonUpsert,
    ) -> Result<ZaehlzeitSaisonRecord, MdmError> {
        let row = sqlx::query(
            r"INSERT INTO zaehler_saisons
                  (register_id, saison, wochentage, zeit_von, zeit_bis, updated_at)
              SELECT r.id, $3::text, $4::smallint[], $5::time, $6::time, now()
                FROM zaehler_register r
               WHERE r.id = $1 AND r.tenant = $2
              ON CONFLICT (register_id, saison, zeit_von) DO UPDATE
              SET wochentage = EXCLUDED.wochentage,
                  zeit_bis   = EXCLUDED.zeit_bis,
                  updated_at = now()
              RETURNING id, register_id, saison, wochentage, zeit_von, zeit_bis, updated_at",
        )
        .bind(register_id)
        .bind(tenant)
        .bind(body.saison.as_str())
        .bind(&body.wochentage)
        .bind(body.zeit_von)
        .bind(body.zeit_bis)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        // Nothing written means the tenant holds no such register — a refusal,
        // never a silent `Ok` reporting a window there is none of.
        let row = row.ok_or_else(|| MdmError::NotFound {
            resource_type: "zaehler_register",
            id: register_id.to_string(),
        })?;
        row_to_saison(&row)
    }

    /// Return all `ZaehlzeitSaison` entries for a register.
    ///
    /// The season windows of one register, scoped through the register's own
    /// tenant: `zaehler_saisons` carries none, and `register_id` reaches this
    /// from a request path.
    pub async fn list_saisons_by_register(
        &self,
        register_id: Uuid,
        tenant: &str,
    ) -> Result<Vec<ZaehlzeitSaisonRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT s.id, s.register_id, s.saison, s.wochentage,
                     s.zeit_von, s.zeit_bis, s.updated_at
              FROM zaehler_saisons  s
              JOIN zaehler_register r ON r.id = s.register_id
              WHERE s.register_id = $1 AND r.tenant = $2
              ORDER BY s.saison, s.zeit_von",
        )
        .bind(register_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter().map(row_to_saison).collect()
    }

    /// Resolve the applicable tariff zone (`HT`|`NT`|`EINZEL`) for a Zähler at
    /// `local_datetime`, a German (Europe/Berlin) wall-clock time. A window
    /// applies on the weekdays it lists, dated by the day it opens on — see
    /// [`window_holds`]. `None` when no window covers the instant; the endpoint
    /// refuses that rather than answering `EINZEL`.
    pub async fn resolve_tariff_zone(
        &self,
        zaehler_id: &str,
        tenant: &str,
        local_datetime: time::PrimitiveDateTime,
    ) -> Result<Option<String>, MdmError> {
        // Every window of the register; which one holds the instant is decided
        // by [`window_holds`], the one place a midnight-crossing window wraps
        // and is dated to the day it opened.
        let rows = sqlx::query(
            r"SELECT r.zaehlerauspraegung, s.zeit_von, s.zeit_bis, s.wochentage
              FROM zaehler_register r
              JOIN zaehler_saisons  s ON s.register_id = r.id
              WHERE r.zaehler_id = $1 AND r.tenant = $2
                AND r.valid_to IS NULL",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        for row in rows {
            let von: time::Time = row
                .try_get("zeit_von")
                .map_err(|e| MdmError::Internal(e.to_string()))?;
            let bis: time::Time = row
                .try_get("zeit_bis")
                .map_err(|e| MdmError::Internal(e.to_string()))?;
            let wochentage: Vec<i16> = row
                .try_get("wochentage")
                .map_err(|e| MdmError::Internal(e.to_string()))?;
            if window_holds(von, bis, &wochentage, local_datetime) {
                return row
                    .try_get::<String, _>("zaehlerauspraegung")
                    .map(Some)
                    .map_err(|e| MdmError::Internal(e.to_string()));
            }
        }
        Ok(None)
    }
}

fn row_to_register(row: &sqlx::postgres::PgRow) -> Result<ZaehlzeitRegisterRecord, MdmError> {
    Ok(ZaehlzeitRegisterRecord {
        id: row
            .try_get("id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zaehler_id: row
            .try_get("zaehler_id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        tenant: row
            .try_get("tenant")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        bezeichnung: row
            .try_get("bezeichnung")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zaehlerauspraegung: row
            .try_get("zaehlerauspraegung")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        obis_kennzahl: row
            .try_get("obis_kennzahl")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        einheit: row.try_get("einheit").unwrap_or_else(|_| "KWH".to_owned()),
        valid_from: row
            .try_get("valid_from")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        valid_to: row
            .try_get("valid_to")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
    })
}

fn row_to_saison(row: &sqlx::postgres::PgRow) -> Result<ZaehlzeitSaisonRecord, MdmError> {
    Ok(ZaehlzeitSaisonRecord {
        id: row
            .try_get("id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        register_id: row
            .try_get("register_id")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        saison: row
            .try_get("saison")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        wochentage: row
            .try_get("wochentage")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zeit_von: row
            .try_get("zeit_von")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        zeit_bis: row
            .try_get("zeit_bis")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
        updated_at: row
            .try_get("updated_at")
            .map_err(|e| MdmError::Internal(e.to_string()))?,
    })
}

/// Whether the German wall-clock time `at` lies inside the Zählzeit window
/// `[zeit_von, zeit_bis)`.
///
/// A window with `zeit_von > zeit_bis` crosses midnight — one row, as the
/// Preisblatt prints „NT 22:00–06:00" — and holds `at` when it is at or after
/// `zeit_von` or before `zeit_bis`. Reading it as an ordinary range holds no
/// minute at all, so the night's NT energy would go missing from the invoice.
/// `zeit_von == zeit_bis` is no window; the schema refuses it.
pub fn window_contains(zeit_von: time::Time, zeit_bis: time::Time, at: time::Time) -> bool {
    match zeit_von.cmp(&zeit_bis) {
        std::cmp::Ordering::Less => zeit_von <= at && at < zeit_bis,
        std::cmp::Ordering::Greater => at >= zeit_von || at < zeit_bis,
        std::cmp::Ordering::Equal => false,
    }
}

/// Whether a window applying on `wochentage` (ISO, Monday = 1) holds the
/// German wall-clock instant `at`.
///
/// A window belongs to the day it **opens** on: „Mo–Fr 22:00–06:00" holds
/// Friday 22:00 to Saturday 06:00 and not Monday 00:00–06:00, which is the
/// tail of Sunday's night. Dating the minutes after midnight by their own
/// weekday would bill Saturday's early hours at the day rate and Monday's at
/// the night rate — each against the window as printed.
pub fn window_holds(
    zeit_von: time::Time,
    zeit_bis: time::Time,
    wochentage: &[i16],
    at: time::PrimitiveDateTime,
) -> bool {
    if !window_contains(zeit_von, zeit_bis, at.time()) {
        return false;
    }
    let after_midnight = zeit_von > zeit_bis && at.time() < zeit_bis;
    let opened = if after_midnight {
        at.date().previous_day().unwrap_or(at.date())
    } else {
        at.date()
    };
    wochentage.contains(&i16::from(opened.weekday().number_from_monday()))
}

#[cfg(test)]
mod tests {
    use super::{window_contains, window_holds};
    use time::{Duration, OffsetDateTime, PrimitiveDateTime, macros::time};

    /// The German wall clock of a UTC instant.
    fn berlin_wall_clock(instant: OffsetDateTime) -> PrimitiveDateTime {
        let local = mako_fristen::in_berlin(instant);
        PrimitiveDateTime::new(local.date(), local.time())
    }

    /// HT 06:00–22:00 and NT 22:00–06:00, as the Preisblatt prints them.
    fn zone(at: time::Time) -> Option<&'static str> {
        [
            ("HT", time!(06:00), time!(22:00)),
            ("NT", time!(22:00), time!(06:00)),
        ]
        .into_iter()
        .find(|(_, von, bis)| window_contains(*von, *bis, at))
        .map(|(z, _, _)| z)
    }

    /// Every UTC minute from 22:00 Berlin on `evening` to 06:00 Berlin on the
    /// next day, classified on its German wall clock.
    fn assert_night_is_nt(evening: time::Date) {
        let start = mako_fristen::berlin_at(evening, time!(22:00));
        let end = mako_fristen::berlin_at(evening.next_day().unwrap(), time!(06:00));
        let mut t = start;
        let mut minutes = 0;
        while t < end {
            let local = berlin_wall_clock(t);
            assert_eq!(zone(local.time()), Some("NT"), "{t} = {local} Berlin");
            t += Duration::MINUTE;
            minutes += 1;
        }
        let night = (end - start).whole_minutes();
        assert_eq!(minutes, night);
        assert_eq!(
            zone(berlin_wall_clock(end).time()),
            Some("HT"),
            "06:00 Berlin has left the NT window"
        );
    }

    #[test]
    fn every_minute_of_an_ordinary_night_is_nt() {
        assert_night_is_nt(time::macros::date!(2026 - 01 - 07));
    }

    /// 29.03.2026, the last Sunday of March: 02:00 jumps to 03:00, so the night
    /// is seven hours long and no minute of it is unclassified.
    #[test]
    fn every_minute_of_the_spring_forward_night_is_nt() {
        let evening = time::macros::date!(2026 - 03 - 28);
        assert_night_is_nt(evening);
        let night = mako_fristen::berlin_at(evening.next_day().unwrap(), time!(06:00))
            - mako_fristen::berlin_at(evening, time!(22:00));
        assert_eq!(night, Duration::hours(7));
    }

    /// 25.10.2026, the last Sunday of October: 02:00–03:00 runs twice, so the
    /// night is nine hours long and both passes through 02:xx are NT.
    #[test]
    fn every_minute_of_the_fall_back_night_is_nt() {
        let evening = time::macros::date!(2026 - 10 - 24);
        assert_night_is_nt(evening);
        let night = mako_fristen::berlin_at(evening.next_day().unwrap(), time!(06:00))
            - mako_fristen::berlin_at(evening, time!(22:00));
        assert_eq!(night, Duration::hours(9));
    }

    /// „Mo–Fr 22:00–06:00": Friday's night runs into Saturday morning, and
    /// Monday's early hours are Sunday's night, which the row does not name.
    #[test]
    fn a_wrapping_window_belongs_to_the_day_it_opens() {
        use time::macros::datetime;
        let mo_fr = [1, 2, 3, 4, 5];
        let (von, bis) = (time!(22:00), time!(06:00));
        // Fri 09.01.2026 23:00 and Sat 10.01. 03:00: Friday's night.
        assert!(window_holds(von, bis, &mo_fr, datetime!(2026-01-09 23:00)));
        assert!(window_holds(von, bis, &mo_fr, datetime!(2026-01-10 03:00)));
        // Sat 22:00 and Mon 12.01. 03:00: Saturday's and Sunday's nights.
        assert!(!window_holds(von, bis, &mo_fr, datetime!(2026-01-10 22:00)));
        assert!(!window_holds(von, bis, &mo_fr, datetime!(2026-01-12 03:00)));
        // Tue 03:00 is Monday's night.
        assert!(window_holds(von, bis, &mo_fr, datetime!(2026-01-13 03:00)));
        // An ordinary window is dated by its own day.
        assert!(window_holds(
            time!(06:00),
            time!(22:00),
            &mo_fr,
            datetime!(2026-01-09 12:00)
        ));
        assert!(!window_holds(
            time!(06:00),
            time!(22:00),
            &mo_fr,
            datetime!(2026-01-10 12:00)
        ));
    }

    #[test]
    fn a_wrapping_window_is_half_open_at_both_ends() {
        let (von, bis) = (time!(22:00), time!(06:00));
        assert!(window_contains(von, bis, time!(22:00)));
        assert!(window_contains(von, bis, time!(00:00)));
        assert!(window_contains(von, bis, time!(05:59:59)));
        assert!(!window_contains(von, bis, time!(06:00)));
        assert!(!window_contains(von, bis, time!(21:59:59)));
        // An ordinary window still does not wrap.
        assert!(!window_contains(time!(06:00), time!(22:00), time!(23:00)));
        assert!(window_contains(time!(06:00), time!(22:00), time!(06:00)));
    }
}
