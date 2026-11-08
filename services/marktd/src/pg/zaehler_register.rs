//! PostgreSQL storage: [`PgZaehlzeitRepository`] for `marktd`.

use mako_markt::{
    error::MdmError,
    repository::{
        MonatTag, ZaehlzeitRegisterRecord, ZaehlzeitRegisterUpsert, ZaehlzeitSaisonRecord,
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
                  (register_id, saison, wochentage, zeit_von, zeit_bis,
                   saison_von, saison_bis, updated_at)
              SELECT r.id, $3::text, $4::smallint[], $5::time, $6::time,
                     $7::text, $8::text, now()
                FROM zaehler_register r
               WHERE r.id = $1 AND r.tenant = $2
              ON CONFLICT (register_id, saison, zeit_von) DO UPDATE
              SET wochentage = EXCLUDED.wochentage,
                  zeit_bis   = EXCLUDED.zeit_bis,
                  saison_von = EXCLUDED.saison_von,
                  saison_bis = EXCLUDED.saison_bis,
                  updated_at = now()
              RETURNING id, register_id, saison, wochentage, zeit_von, zeit_bis,
                        saison_von, saison_bis, updated_at",
        )
        .bind(register_id)
        .bind(tenant)
        .bind(body.saison.as_str())
        .bind(&body.wochentage)
        .bind(body.zeit_von)
        .bind(body.zeit_bis)
        .bind(body.saison_von.map(|d| d.to_string()))
        .bind(body.saison_bis.map(|d| d.to_string()))
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
                     s.zeit_von, s.zeit_bis, s.saison_von, s.saison_bis, s.updated_at
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
    /// `local_datetime`, a German (Europe/Berlin) wall-clock time.
    ///
    /// The windows read are those of the registers valid on the instant's
    /// calendar day — validity is half-open `[valid_from, valid_to)`, so a
    /// register replaced since still answers for the time before its
    /// replacement — and [`classify`] decides which of them holds it: the
    /// weekday and the season of the day a window opens on, the wall clock
    /// within it. `None` when no window covers the instant; the endpoint
    /// refuses that rather than answering `EINZEL`.
    ///
    /// # Errors
    /// [`MdmError::Unprocessable`] when windows of different zones both hold
    /// the instant — master data with two answers, which no row order may
    /// settle.
    pub async fn resolve_tariff_zone(
        &self,
        zaehler_id: &str,
        tenant: &str,
        local_datetime: time::PrimitiveDateTime,
    ) -> Result<Option<String>, MdmError> {
        let rows = sqlx::query(
            r"SELECT r.zaehlerauspraegung, r.bezeichnung, r.valid_from,
                     s.saison, s.saison_von, s.saison_bis,
                     s.zeit_von, s.zeit_bis, s.wochentage
              FROM zaehler_register r
              JOIN zaehler_saisons  s ON s.register_id = r.id
              WHERE r.zaehler_id = $1 AND r.tenant = $2
                AND r.valid_from <= $3
                AND (r.valid_to IS NULL OR $3 < r.valid_to)
              ORDER BY r.valid_from, r.bezeichnung, s.saison, s.zeit_von, s.id",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .bind(local_datetime.date())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        let windows = rows
            .iter()
            .map(row_to_fenster)
            .collect::<Result<Vec<_>, _>>()?;
        classify(&windows, local_datetime)
    }
}

/// One Zählzeit window as [`classify`] reads it: a `zaehler_saisons` row with
/// the zone of its register.
#[derive(Debug, Clone)]
pub struct Zaehlzeitfenster {
    /// The register's `zaehlerauspraegung`: `HT`, `NT` or `EINZEL`.
    pub zone: String,
    /// The register's `bezeichnung`, to name the window in a refusal.
    pub register: String,
    /// `SOMMER`, `WINTER` or `GESAMT`.
    pub saison: String,
    /// The season's first day, inclusive — `None` for `GESAMT`.
    pub saison_von: Option<MonatTag>,
    /// The day after the season, exclusive — `None` for `GESAMT`.
    pub saison_bis: Option<MonatTag>,
    /// ISO weekdays, Monday = 1.
    pub wochentage: Vec<i16>,
    /// Window start, German wall clock, inclusive.
    pub zeit_von: time::Time,
    /// Window end, German wall clock, exclusive.
    pub zeit_bis: time::Time,
}

impl Zaehlzeitfenster {
    /// Whether the window holds the German wall-clock instant `at`: the wall
    /// clock is inside it, the day it opened on is one of its weekdays (see
    /// [`window_holds`]), and the instant's own calendar day is inside its
    /// season.
    ///
    /// The season is the instant's day, not the opening day's: each season's
    /// windows tile its own days, so on 1 April a SOMMER night 20:00–08:00
    /// that opened on 31 March takes over from WINTER's 22:00–06:00 at
    /// midnight. Dating the season by the opening day instead leaves 06:00–08:00
    /// on 1 April in no window and 06:00–08:00 on 1 October in two.
    #[must_use]
    pub fn holds(&self, at: time::PrimitiveDateTime) -> bool {
        let in_season = match (self.saison_von, self.saison_bis) {
            (Some(von), Some(bis)) => MonatTag::range_holds(von, bis, at.date()),
            _ => true,
        };
        in_season
            && opened_on(self.zeit_von, self.zeit_bis, at)
                .is_some_and(|opened| weekday_listed(&self.wochentage, opened))
    }
}

/// The tariff zone of the windows that hold `at`, independent of their order.
///
/// # Errors
/// [`MdmError::Unprocessable`] when windows of different zones hold it.
pub fn classify(
    windows: &[Zaehlzeitfenster],
    at: time::PrimitiveDateTime,
) -> Result<Option<String>, MdmError> {
    let holding: Vec<&Zaehlzeitfenster> = windows.iter().filter(|w| w.holds(at)).collect();
    let Some(first) = holding.first() else {
        return Ok(None);
    };
    if let Some(other) = holding.iter().find(|w| w.zone != first.zone) {
        let name = |w: &Zaehlzeitfenster| {
            format!("{} {} {}–{}", w.register, w.saison, w.zeit_von, w.zeit_bis)
        };
        return Err(MdmError::Unprocessable {
            reason: format!(
                "{at}: the windows {} ({}) and {} ({}) both hold it — the tariff zone is contradictory",
                name(first),
                first.zone,
                name(other),
                other.zone
            ),
        });
    }
    Ok(Some(first.zone.clone()))
}

fn row_to_fenster(row: &sqlx::postgres::PgRow) -> Result<Zaehlzeitfenster, MdmError> {
    let get_err = |e: sqlx::Error| MdmError::Internal(e.to_string());
    Ok(Zaehlzeitfenster {
        zone: row.try_get("zaehlerauspraegung").map_err(get_err)?,
        register: row.try_get("bezeichnung").map_err(get_err)?,
        saison: row.try_get("saison").map_err(get_err)?,
        saison_von: monat_tag(row, "saison_von")?,
        saison_bis: monat_tag(row, "saison_bis")?,
        wochentage: row.try_get("wochentage").map_err(get_err)?,
        zeit_von: row.try_get("zeit_von").map_err(get_err)?,
        zeit_bis: row.try_get("zeit_bis").map_err(get_err)?,
    })
}

/// A stored `'MM-DD'` season boundary. The column's CHECK admits `02-30`,
/// which no year has; reading it is an internal error, not a guess.
fn monat_tag(row: &sqlx::postgres::PgRow, column: &str) -> Result<Option<MonatTag>, MdmError> {
    row.try_get::<Option<String>, _>(column)
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .map(|raw| {
            raw.parse()
                .map_err(|e: MdmError| MdmError::Internal(format!("{column}: {e}")))
        })
        .transpose()
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
        saison_von: monat_tag(row, "saison_von")?,
        saison_bis: monat_tag(row, "saison_bis")?,
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
    opened_on(zeit_von, zeit_bis, at).is_some_and(|opened| weekday_listed(wochentage, opened))
}

/// The day the occurrence of `[zeit_von, zeit_bis)` holding `at` opened on —
/// the day before for the after-midnight part of a wrapping window — or
/// `None` when the wall clock of `at` is outside the window.
fn opened_on(
    zeit_von: time::Time,
    zeit_bis: time::Time,
    at: time::PrimitiveDateTime,
) -> Option<time::Date> {
    if !window_contains(zeit_von, zeit_bis, at.time()) {
        return None;
    }
    let after_midnight = zeit_von > zeit_bis && at.time() < zeit_bis;
    Some(if after_midnight {
        at.date().previous_day().unwrap_or(at.date())
    } else {
        at.date()
    })
}

fn weekday_listed(wochentage: &[i16], day: time::Date) -> bool {
    wochentage.contains(&i16::from(day.weekday().number_from_monday()))
}

#[cfg(test)]
mod tests {
    use super::{Zaehlzeitfenster, classify, window_contains, window_holds};
    use mako_markt::repository::MonatTag;
    use time::{
        Duration, OffsetDateTime, PrimitiveDateTime,
        macros::{datetime, time},
    };

    /// A daily window of `saison`, which an operator's Preisblatt dates
    /// 01.04.–30.09. (SOMMER) and 01.10.–31.03. (WINTER). These dates are the
    /// fixture's: no published rule fixes a Zählzeit season.
    fn fenster(zone: &str, saison: &str, von: time::Time, bis: time::Time) -> Zaehlzeitfenster {
        let (apr, okt) = (MonatTag::new(4, 1).unwrap(), MonatTag::new(10, 1).unwrap());
        let (saison_von, saison_bis) = match saison {
            "SOMMER" => (Some(apr), Some(okt)),
            "WINTER" => (Some(okt), Some(apr)),
            _ => (None, None),
        };
        Zaehlzeitfenster {
            zone: zone.to_owned(),
            register: zone.to_owned(),
            saison: saison.to_owned(),
            saison_von,
            saison_bis,
            wochentage: vec![1, 2, 3, 4, 5, 6, 7],
            zeit_von: von,
            zeit_bis: bis,
        }
    }

    /// SOMMER HT 08:00–20:00 / NT 20:00–08:00 over WINTER HT 06:00–22:00 /
    /// NT 22:00–06:00: at 07:00 and 21:00 a SOMMER NT window and a WINTER HT
    /// window overlap on the same weekday and hour.
    fn sommer_over_winter() -> Vec<Zaehlzeitfenster> {
        vec![
            fenster("HT", "SOMMER", time!(08:00), time!(20:00)),
            fenster("NT", "SOMMER", time!(20:00), time!(08:00)),
            fenster("HT", "WINTER", time!(06:00), time!(22:00)),
            fenster("NT", "WINTER", time!(22:00), time!(06:00)),
        ]
    }

    fn zone_at(windows: &[Zaehlzeitfenster], at: PrimitiveDateTime) -> Option<String> {
        classify(windows, at).expect("no contradiction")
    }

    #[test]
    fn overlapping_seasons_answer_the_instants_own_season_in_any_order() {
        let mut windows = sommer_over_winter();
        for _ in 0..2 {
            assert_eq!(
                zone_at(&windows, datetime!(2026-01-15 07:00)).as_deref(),
                Some("HT")
            );
            assert_eq!(
                zone_at(&windows, datetime!(2026-07-15 07:00)).as_deref(),
                Some("NT")
            );
            assert_eq!(
                zone_at(&windows, datetime!(2026-01-15 21:00)).as_deref(),
                Some("HT")
            );
            assert_eq!(
                zone_at(&windows, datetime!(2026-07-15 21:00)).as_deref(),
                Some("NT")
            );
            windows.reverse();
        }
    }

    /// 30.03.–03.04.2026 and 29.09.–03.10.2026 cross the season boundaries:
    /// every quarter hour is classified by the season of its own day, so no
    /// quarter hour falls in no window and none in two.
    #[test]
    fn a_period_crossing_the_season_boundary_resolves_each_instant_to_its_own_season() {
        let windows = sommer_over_winter();
        let sommer = |day: time::Date| {
            (time::macros::date!(2026 - 04 - 01)..time::macros::date!(2026 - 10 - 01))
                .contains(&day)
        };
        for (from, to) in [
            (datetime!(2026-03-30 00:00), datetime!(2026-04-03 00:00)),
            (datetime!(2026-09-29 00:00), datetime!(2026-10-03 00:00)),
        ] {
            let mut at = from;
            while at < to {
                let t = at.time();
                let ht = if sommer(at.date()) {
                    (time!(08:00)..time!(20:00)).contains(&t)
                } else {
                    (time!(06:00)..time!(22:00)).contains(&t)
                };
                let expected = if ht { "HT" } else { "NT" };
                assert_eq!(zone_at(&windows, at).as_deref(), Some(expected), "{at}");
                at += Duration::minutes(15);
            }
        }
        // 31 March 23:00 is WINTER's night; 1 April 03:00 and 07:00 are
        // SOMMER's, whose night opened on 31 March.
        assert_eq!(
            zone_at(&windows, datetime!(2026-03-31 23:00)).as_deref(),
            Some("NT")
        );
        assert_eq!(
            zone_at(&windows, datetime!(2026-04-01 07:00)).as_deref(),
            Some("NT")
        );
        assert_eq!(
            zone_at(&windows, datetime!(2026-03-31 07:00)).as_deref(),
            Some("HT")
        );
    }

    #[test]
    fn a_gesamt_window_holds_all_year() {
        let windows = [fenster("EINZEL", "GESAMT", time!(00:00), time!(00:00:01))];
        for at in [datetime!(2026-01-15 00:00), datetime!(2026-07-15 00:00)] {
            assert_eq!(zone_at(&windows, at).as_deref(), Some("EINZEL"));
        }
    }

    /// Two windows of different zones over the same instant are master data
    /// with two answers; the row order Postgres happens to return must not
    /// pick one.
    #[test]
    fn windows_of_different_zones_over_one_instant_are_refused() {
        let windows = [
            fenster("HT", "GESAMT", time!(06:00), time!(22:00)),
            fenster("NT", "WINTER", time!(20:00), time!(06:00)),
        ];
        assert!(classify(&windows, datetime!(2026-01-15 21:00)).is_err());
        assert_eq!(
            zone_at(&windows, datetime!(2026-07-15 21:00)).as_deref(),
            Some("HT")
        );
        // Windows agreeing on the zone are no contradiction.
        let agreeing = [
            fenster("NT", "GESAMT", time!(22:00), time!(06:00)),
            fenster("NT", "WINTER", time!(20:00), time!(06:00)),
        ];
        assert_eq!(
            zone_at(&agreeing, datetime!(2026-01-15 23:00)).as_deref(),
            Some("NT")
        );
    }

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
