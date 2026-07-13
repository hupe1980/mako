//! Regulatory scenario tests for `mako-markt` using in-memory repositories.
//!
//! Run: `cargo test -p mako-markt --test regulatory_scenarios --features testing`

#[cfg(feature = "testing")]
mod tests {
    use mako_markt::{
        domain::MaloId,
        repository::{
            LieferStatus, Lokationszuordnung, NbEnergiemixRepository as _, VersorgungsStatusRecord,
            VersorgungsStatusRepository as _,
        },
        testing::{InMemoryNbEnergiemixRepository, InMemoryVersorgungsStatusRepository},
    };
    use time::macros::date;

    fn malo_id(id: &str) -> MaloId {
        MaloId::try_from(id).expect("valid MaLo-ID")
    }

    #[allow(clippy::too_many_arguments)]
    fn vs_rec(
        malo: &str,
        tenant: &str,
        status: LieferStatus,
        lf_mp_id: Option<&str>,
        lf_mp_id_next: Option<&str>,
        lf_next_lieferbeginn: Option<time::Date>,
        lieferbeginn: Option<time::Date>,
        lieferende: Option<time::Date>,
        nb_mp_id: &str,
    ) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: malo_id(malo),
            tenant: tenant.to_owned(),
            lieferstatus: status,
            lf_mp_id: lf_mp_id.map(str::to_owned),
            lf_mp_id_next: lf_mp_id_next.map(str::to_owned),
            lf_next_lieferbeginn,
            lieferbeginn,
            lieferende,
            msb_mp_id: None,
            nb_mp_id: nb_mp_id.to_owned(),
            last_process_id: None,
            updated_at: time::OffsetDateTime::now_utc(),
            version: 0, // will be overwritten by repo
        }
    }

    // ═════════════════════════════════════════════════════════════════════
    // VersorgungsStatus transitions (GPKE BK6-22-024)
    // ═════════════════════════════════════════════════════════════════════

    /// New MaLo has no VersorgungsStatus.
    #[tokio::test]
    async fn versorgungsstatus_initially_absent() {
        let repo = InMemoryVersorgungsStatusRepository::default();
        let result = repo.find(&malo_id("51238696780"), "t1").await.unwrap();
        assert!(
            result.is_none(),
            "new MaLo should have no VersorgungsStatus"
        );
    }

    /// GPKE §3: 55001 Anmeldung sets lf_mp_id_next + lf_next_lieferbeginn.
    /// Status stays Unbeliefert until NB confirms.
    #[tokio::test]
    async fn anmeldung_sets_lf_next() {
        let repo = InMemoryVersorgungsStatusRepository::default();
        repo.upsert(
            vs_rec(
                "51238696780",
                "t1",
                LieferStatus::Unbeliefert,
                None,
                Some("9910000000001"),
                Some(date!(2026 - 10 - 01)),
                None,
                None,
                "9904234560001",
            ),
            None,
        )
        .await
        .unwrap();

        let r = repo
            .find(&malo_id("51238696780"), "t1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.lieferstatus, LieferStatus::Unbeliefert);
        assert_eq!(
            r.lf_mp_id_next.as_deref(),
            Some("9910000000001"),
            "announced LF must be set"
        );
        assert_eq!(r.lf_next_lieferbeginn, Some(date!(2026 - 10 - 01)));
        assert!(
            r.lf_mp_id.is_none(),
            "active LF must not be set until NB confirms"
        );
    }

    /// GPKE §3: 55003 Bestätigung → Beliefert, lf_mp_id set, lf_mp_id_next cleared.
    #[tokio::test]
    async fn bestaetigung_transitions_to_beliefert() {
        let repo = InMemoryVersorgungsStatusRepository::default();
        // 55001
        repo.upsert(
            vs_rec(
                "51238696780",
                "t1",
                LieferStatus::Unbeliefert,
                None,
                Some("9910000000001"),
                Some(date!(2026 - 10 - 01)),
                None,
                None,
                "9904234560001",
            ),
            None,
        )
        .await
        .unwrap();
        // 55003 NB confirmation
        repo.upsert(
            vs_rec(
                "51238696780",
                "t1",
                LieferStatus::Beliefert,
                Some("9910000000001"),
                None,
                None,
                Some(date!(2026 - 10 - 01)),
                None,
                "9904234560001",
            ),
            Some(1),
        )
        .await
        .unwrap();

        let r = repo
            .find(&malo_id("51238696780"), "t1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.lieferstatus, LieferStatus::Beliefert);
        assert_eq!(r.lf_mp_id.as_deref(), Some("9910000000001"));
        assert!(
            r.lf_mp_id_next.is_none(),
            "announced LF must be cleared after confirmation"
        );
        assert_eq!(r.lieferbeginn, Some(date!(2026 - 10 - 01)));
        assert_eq!(r.version, 2);
    }

    /// GPKE §3: 55009 Lieferende confirmed → Unbeliefert, lieferende set.
    #[tokio::test]
    async fn lieferende_transitions_to_unbeliefert() {
        let repo = InMemoryVersorgungsStatusRepository::default();
        repo.upsert(
            vs_rec(
                "51238696780",
                "t1",
                LieferStatus::Beliefert,
                Some("9910000000001"),
                None,
                None,
                Some(date!(2026 - 01 - 01)),
                None,
                "9904234560001",
            ),
            None,
        )
        .await
        .unwrap();
        repo.upsert(
            vs_rec(
                "51238696780",
                "t1",
                LieferStatus::Unbeliefert,
                None,
                None,
                None,
                Some(date!(2026 - 01 - 01)),
                Some(date!(2026 - 12 - 31)),
                "9904234560001",
            ),
            Some(1),
        )
        .await
        .unwrap();

        let r = repo
            .find(&malo_id("51238696780"), "t1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.lieferstatus, LieferStatus::Unbeliefert);
        assert!(r.lf_mp_id.is_none(), "LF must be cleared after Lieferende");
        assert_eq!(r.lieferende, Some(date!(2026 - 12 - 31)));
    }

    /// Optimistic concurrency: wrong version is rejected.
    #[tokio::test]
    async fn version_conflict_rejected() {
        let repo = InMemoryVersorgungsStatusRepository::default();
        repo.upsert(
            vs_rec(
                "51238696780",
                "t1",
                LieferStatus::Beliefert,
                Some("9910000000001"),
                None,
                None,
                Some(date!(2026 - 01 - 01)),
                None,
                "9904234560001",
            ),
            None, // create
        )
        .await
        .unwrap();

        // Try to update expecting version 99 (wrong)
        let result = repo
            .upsert(
                vs_rec(
                    "51238696780",
                    "t1",
                    LieferStatus::Unbeliefert,
                    None,
                    None,
                    None,
                    None,
                    None,
                    "9904234560001",
                ),
                Some(99), // wrong version
            )
            .await;

        assert!(result.is_err(), "wrong version must be rejected");
    }

    /// Simultaneous supplier switch: GPKE rule A06 — only one pending Anmeldung allowed.
    /// Test documents the expected invariant (enforced at application layer, not repo).
    #[tokio::test]
    async fn only_one_pending_anmeldung_at_a_time() {
        // The VersorgungsStatus record only has ONE lf_mp_id_next slot.
        // A second 55001 UTILMD would overwrite the first — GPKE NB must reject it
        // with A06 before that happens.
        let rec = vs_rec(
            "51238696780",
            "t1",
            LieferStatus::Unbeliefert,
            None,
            Some("9910000000001"),
            Some(date!(2026 - 10 - 01)),
            None,
            None,
            "9904234560001",
        );
        assert!(rec.lf_mp_id_next.is_some());
        assert!(
            rec.lf_mp_id.is_none(),
            "at most one announced LF, no active LF during Anmeldung-phase"
        );
    }

    // ═════════════════════════════════════════════════════════════════════
    // §42 EnWG — NbEnergiemix
    // ═════════════════════════════════════════════════════════════════════

    /// §42 EnWG: NB publishes Energiemix; LF reads it for Reststrommix.
    #[tokio::test]
    async fn nb_energiemix_roundtrip() {
        let repo = InMemoryNbEnergiemixRepository::default();

        let energiemix = serde_json::json!({
            "energietraeger": [
                { "erzeugungsart": "SOLAR", "anteilProzent": 15.0 },
                { "erzeugungsart": "WIND_ONSHORE", "anteilProzent": 25.0 },
                { "erzeugungsart": "STEINKOHLE", "anteilProzent": 60.0 },
            ]
        });

        repo.upsert_energiemix(
            "t1",
            "9904234560001",
            2025,
            energiemix,
            Some(150_000_000),
            Some(500_000_000),
        )
        .await
        .unwrap();

        let r = repo
            .find_energiemix("t1", "9904234560001", Some(2025))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.nb_mp_id, "9904234560001");
        assert_eq!(r.gueltig_fuer, 2025);
        assert_eq!(r.eeg_einspeisung_kwh, Some(150_000_000));
    }

    /// §42 Abs. 5 EnWG: renewable share derivable from Energiemix.
    #[tokio::test]
    async fn energiemix_renewable_share_40_pct_solar_30_pct_wind() {
        let repo = InMemoryNbEnergiemixRepository::default();

        repo.upsert_energiemix(
            "t1",
            "9904234560001",
            2025,
            serde_json::json!({
                "energietraeger": [
                    { "erzeugungsart": "SOLAR",        "anteilProzent": 40.0 },
                    { "erzeugungsart": "WIND_ONSHORE",  "anteilProzent": 30.0 },
                    { "erzeugungsart": "ERDGAS",         "anteilProzent": 30.0 },
                ]
            }),
            None,
            None,
        )
        .await
        .unwrap();

        let r = repo
            .find_energiemix("t1", "9904234560001", Some(2025))
            .await
            .unwrap()
            .unwrap();
        let renewable_pct: f64 = r.energiemix["energietraeger"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| {
                matches!(
                    e["erzeugungsart"].as_str().unwrap_or(""),
                    "SOLAR" | "WIND_ONSHORE" | "WIND_OFFSHORE" | "WASSERKRAFT" | "BIOMASSE"
                )
            })
            .filter_map(|e| e["anteilProzent"].as_f64())
            .sum();
        assert!((renewable_pct - 70.0).abs() < 0.001, "70% renewable");
    }

    /// Most recent year returned when no year specified.
    #[tokio::test]
    async fn energiemix_most_recent_year_returned() {
        let repo = InMemoryNbEnergiemixRepository::default();
        for y in [2023i16, 2024, 2025] {
            repo.upsert_energiemix(
                "t1",
                "9904234560001",
                y,
                serde_json::json!({"year": y}),
                None,
                None,
            )
            .await
            .unwrap();
        }
        let r = repo
            .find_energiemix("t1", "9904234560001", None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r.gueltig_fuer, 2025, "must return most recent year");
    }

    /// Years returned in descending order.
    #[tokio::test]
    async fn energiemix_list_years_descending() {
        let repo = InMemoryNbEnergiemixRepository::default();
        for y in [2024i16, 2022, 2025, 2023] {
            repo.upsert_energiemix("t1", "9904234560001", y, serde_json::json!({}), None, None)
                .await
                .unwrap();
        }
        let years = repo
            .list_energiemix_years("t1", "9904234560001")
            .await
            .unwrap();
        assert_eq!(years, vec![2025, 2024, 2023, 2022]);
    }

    /// Different tenants are isolated.
    #[tokio::test]
    async fn energiemix_tenant_isolation() {
        let repo = InMemoryNbEnergiemixRepository::default();
        repo.upsert_energiemix(
            "tenant_a",
            "9904234560001",
            2025,
            serde_json::json!({}),
            None,
            None,
        )
        .await
        .unwrap();
        let result = repo
            .find_energiemix("tenant_b", "9904234560001", Some(2025))
            .await
            .unwrap();
        assert!(result.is_none(), "tenant_b must not see tenant_a data");
    }

    // ═════════════════════════════════════════════════════════════════════
    // Lokationszuordnung temporal semantics
    // ═════════════════════════════════════════════════════════════════════

    /// Open-ended assignment (valid_to = None) is valid today.
    #[test]
    fn lokationszuordnung_open_ended_is_valid_today() {
        let a = Lokationszuordnung {
            zuordnungstyp: "NB".to_owned(),
            rollencodenummer: "9904234560001".to_owned(),
            valid_from: date!(2020 - 01 - 01),
            valid_to: None,
        };
        let today = time::OffsetDateTime::now_utc().date();
        let valid = a.valid_from <= today && a.valid_to.is_none_or(|to| to >= today);
        assert!(valid, "open-ended assignment must be valid today");
    }

    /// Assignment with past valid_to is NOT valid today.
    #[test]
    fn lokationszuordnung_past_assignment_is_expired() {
        let a = Lokationszuordnung {
            zuordnungstyp: "LF".to_owned(),
            rollencodenummer: "9910000000001".to_owned(),
            valid_from: date!(2020 - 01 - 01),
            valid_to: Some(date!(2021 - 12 - 31)),
        };
        let today = time::OffsetDateTime::now_utc().date();
        let valid = a.valid_from <= today && a.valid_to.is_none_or(|to| to >= today);
        assert!(!valid, "past assignment must be expired");
    }

    /// Future assignment is not yet valid.
    #[test]
    fn lokationszuordnung_future_assignment_not_yet_valid() {
        let a = Lokationszuordnung {
            zuordnungstyp: "MSB".to_owned(),
            rollencodenummer: "9900001234567".to_owned(),
            valid_from: date!(2099 - 01 - 01), // far future
            valid_to: None,
        };
        let today = time::OffsetDateTime::now_utc().date();
        let valid = a.valid_from <= today && a.valid_to.is_none_or(|to| to >= today);
        assert!(!valid, "future assignment must not yet be valid");
    }
}

// ── GeLi Gas 3.0 VersorgungsStatus scenarios (BK7-24-01-009) ────────────────

#[cfg(feature = "testing")]
mod geli_gas_tests {
    use mako_markt::{
        domain::MaloId,
        repository::{LieferStatus, VersorgungsStatusRecord, VersorgungsStatusRepository as _},
        testing::InMemoryVersorgungsStatusRepository,
    };
    use time::macros::date;

    fn gas_malo() -> MaloId {
        // Valid MaLo-ID with correct BDEW alternating-weight checksum (digit = 0)
        MaloId::try_from("51238696780").expect("valid Gas MaLo-ID")
    }

    fn gas_vs_rec(
        status: LieferStatus,
        lf: Option<&str>,
        lf_next: Option<&str>,
        lieferbeginn: Option<time::Date>,
        lf_next_lieferbeginn: Option<time::Date>,
        lieferende: Option<time::Date>,
    ) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: gas_malo(),
            lieferstatus: status,
            lf_mp_id: lf.map(str::to_owned),
            lf_mp_id_next: lf_next.map(str::to_owned),
            lf_next_lieferbeginn,
            lieferbeginn,
            lieferende,
            msb_mp_id: Some("9880000000001".to_owned()), // DVGW GMSB
            nb_mp_id: "9870000000001".to_owned(),        // DVGW GNB
            last_process_id: None,
            updated_at: time::OffsetDateTime::now_utc(),
            tenant: "test".to_owned(),
            version: 0,
        }
    }

    /// GeLi Gas 3.0 PID 44001 (LFN-Anmeldung): before any LF assignment the
    /// initial supply state is Unbeliefert (§36 EnWG Grundversorgung is separate).
    #[tokio::test]
    async fn geli_gas_initial_state_is_unbeliefert() {
        let repo = InMemoryVersorgungsStatusRepository::default();
        let rec = gas_vs_rec(LieferStatus::Unbeliefert, None, None, None, None, None);
        repo.upsert(rec, None).await.unwrap();
        let result = repo.find(&gas_malo(), "test").await.unwrap().unwrap();
        assert_eq!(result.lieferstatus, LieferStatus::Unbeliefert);
        assert!(result.lf_mp_id.is_none());
    }

    /// GeLi Gas 3.0 PID 44003 (LFN-Anmeldung Bestätigung, GNB→LFN):
    /// After GNB confirms, status transitions to Beliefert with a future Lieferbeginn.
    /// BK7-24-01-009 §5: GNB has 10 Werktage to respond.
    #[tokio::test]
    async fn geli_gas_anmeldung_bestaetigt_transitions_to_beliefert() {
        let repo = InMemoryVersorgungsStatusRepository::default();

        // Step 1: Register initial Unbeliefert state.
        let rec = gas_vs_rec(LieferStatus::Unbeliefert, None, None, None, None, None);
        repo.upsert(rec, None).await.unwrap();

        // Step 2: GNB confirms Lieferbeginn → Beliefert.
        let confirmed = gas_vs_rec(
            LieferStatus::Beliefert,
            Some("9810000000001"), // LFG MP-ID (DVGW 98...)
            None,
            Some(date!(2025 - 10 - 01)),
            None,
            None,
        );
        repo.upsert(confirmed, None).await.unwrap();

        let result = repo.find(&gas_malo(), "test").await.unwrap().unwrap();
        assert_eq!(result.lieferstatus, LieferStatus::Beliefert);
        assert_eq!(result.lf_mp_id.as_deref(), Some("9810000000001"));
        assert_eq!(result.lieferbeginn, Some(date!(2025 - 10 - 01)));
    }

    /// GeLi Gas 3.0 PID 44013 (LFN-Abmeldung): Lieferantenwechsel sets a future
    /// Lieferende and populates lf_mp_id_next / lf_next_lieferbeginn atomically.
    /// BK7-24-01-009 §7: simultaneous Abmeldung + Anmeldung (Lieferantenwechsel).
    #[tokio::test]
    async fn geli_gas_lieferantenwechsel_sets_lf_next() {
        let repo = InMemoryVersorgungsStatusRepository::default();

        // Current supply active.
        let rec = gas_vs_rec(
            LieferStatus::Beliefert,
            Some("9810000000001"),
            None,
            Some(date!(2025 - 01 - 01)),
            None,
            None,
        );
        repo.upsert(rec, None).await.unwrap();

        // Lieferantenwechsel: next-LF fields set, Lieferende announced.
        let wechsel = gas_vs_rec(
            LieferStatus::Beliefert,
            Some("9810000000001"),
            Some("9820000000001"), // new LFG
            Some(date!(2025 - 01 - 01)),
            Some(date!(2025 - 10 - 01)),
            Some(date!(2025 - 10 - 01)),
        );
        repo.upsert(wechsel, None).await.unwrap();

        let result = repo.find(&gas_malo(), "test").await.unwrap().unwrap();
        assert_eq!(result.lf_mp_id_next.as_deref(), Some("9820000000001"));
        assert_eq!(result.lf_next_lieferbeginn, Some(date!(2025 - 10 - 01)));
        assert_eq!(result.lieferende, Some(date!(2025 - 10 - 01)));
    }

    /// GeLi Gas 3.0 Stornierung (PID 44022–44024): after a successful Stornierung
    /// the supply state reverts — lf_mp_id_next and lf_next_lieferbeginn are cleared.
    /// BK7-24-01-009 §9: Stornierung must nullify all pending Wechsel fields.
    #[tokio::test]
    async fn geli_gas_stornierung_clears_lf_next() {
        let repo = InMemoryVersorgungsStatusRepository::default();

        // Pre-stornierung: Lieferantenwechsel in progress.
        let before_storno = gas_vs_rec(
            LieferStatus::Beliefert,
            Some("9810000000001"),
            Some("9820000000001"),
            Some(date!(2025 - 01 - 01)),
            Some(date!(2025 - 10 - 01)),
            Some(date!(2025 - 10 - 01)),
        );
        repo.upsert(before_storno, None).await.unwrap();

        // Stornierung: next-LF fields cleared, Lieferende removed.
        let after_storno = gas_vs_rec(
            LieferStatus::Beliefert,
            Some("9810000000001"),
            None, // cleared
            Some(date!(2025 - 01 - 01)),
            None, // cleared
            None, // cleared
        );
        repo.upsert(after_storno, None).await.unwrap();

        let result = repo.find(&gas_malo(), "test").await.unwrap().unwrap();
        assert!(
            result.lf_mp_id_next.is_none(),
            "stornierung must clear lf_mp_id_next"
        );
        assert!(
            result.lf_next_lieferbeginn.is_none(),
            "stornierung must clear lf_next_lieferbeginn"
        );
        assert!(
            result.lieferende.is_none(),
            "stornierung must clear lieferende"
        );
    }
}
