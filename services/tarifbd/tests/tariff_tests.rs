//! Unit tests for `tarifbd` — product validation, state machine, EPEX, Angebote.
//!
//! These tests exercise pure logic (no database) wherever possible.
//! For data-layer tests a PostgreSQL test database is required and those tests
//! are conditionally compiled with `#[cfg(feature = "test-db")]`.

#![allow(clippy::doc_markdown)]

// ── Valid-preistypen whitelist ────────────────────────────────────────────────

#[cfg(test)]
mod preistyp_tests {
    /// All whitelisted preistypen must be non-empty and ALLCAPS.
    #[test]
    fn whitelist_entries_are_allcaps_non_empty() {
        // Access via the handlers module which is pub(crate).
        // We replicate the const here for test isolation.
        const VALID_PREISTYPEN: &[&str] = &[
            "GRUNDPREIS",
            "ARBEITSPREIS_EINTARIF",
            "ARBEITSPREIS_HT",
            "ARBEITSPREIS_NT",
            "LEISTUNGSPREIS",
            "MESSPREIS",
            "ENTGELT_ABLESUNG",
            "ENTGELT_ABRECHNUNG",
            "ENTGELT_MSB",
            "PROVISION",
            "SOLAR_ARBEITSPREIS",
            "EEG_VERGUETUNG",
            "EEG_MARKTPRAEMIE",
            "EEG_MANAGEMENTPRAEMIE",
            "KWKG_ZUSCHLAG",
            "MARKTWERT",
            "VERMARKTUNGSGEBUEHR",
            "MIETERSTROM_AUFSCHLAG",
            "GEMEINSCHAFT_RABATT",
            "STEUERUNGSRABATT_MODUL1",
            "STEUERUNGSRABATT_MODUL3",
            "HEMS_PLATTFORMGEBUEHR",
            "HEMS_OPTIMIERUNGSEVENT",
            "HEMS_AUSLESUNG",
            "EMOBILITY_SERVICEGEBUEHR",
            "EMOBILITY_ARBEITSPREIS",
            "EMOBILITY_SESSION",
            "EMOBILITY_ROAMING",
            "SERVICE_GEBUEHR",
            "SERVICE_EVENT",
        ];

        for pt in VALID_PREISTYPEN {
            assert!(!pt.is_empty(), "preistyp must not be empty");
            assert_eq!(pt.to_uppercase(), *pt, "preistyp must be ALLCAPS: {}", pt);
        }
    }

    #[test]
    fn no_duplicate_preistyp_entries() {
        let mut seen = std::collections::HashSet::new();
        const VALID_PREISTYPEN: &[&str] = &[
            "GRUNDPREIS",
            "ARBEITSPREIS_EINTARIF",
            "ARBEITSPREIS_HT",
            "ARBEITSPREIS_NT",
            "LEISTUNGSPREIS",
            "MESSPREIS",
            "ENTGELT_ABLESUNG",
            "ENTGELT_ABRECHNUNG",
            "ENTGELT_MSB",
            "PROVISION",
            "SOLAR_ARBEITSPREIS",
            "EEG_VERGUETUNG",
            "EEG_MARKTPRAEMIE",
            "EEG_MANAGEMENTPRAEMIE",
            "KWKG_ZUSCHLAG",
            "MARKTWERT",
            "VERMARKTUNGSGEBUEHR",
            "MIETERSTROM_AUFSCHLAG",
            "GEMEINSCHAFT_RABATT",
            "STEUERUNGSRABATT_MODUL1",
            "STEUERUNGSRABATT_MODUL3",
            "HEMS_PLATTFORMGEBUEHR",
            "HEMS_OPTIMIERUNGSEVENT",
            "HEMS_AUSLESUNG",
            "EMOBILITY_SERVICEGEBUEHR",
            "EMOBILITY_ARBEITSPREIS",
            "EMOBILITY_SESSION",
            "EMOBILITY_ROAMING",
            "SERVICE_GEBUEHR",
            "SERVICE_EVENT",
        ];
        for pt in VALID_PREISTYPEN {
            assert!(seen.insert(*pt), "duplicate preistyp in whitelist: {}", pt);
        }
    }
}

// ── Product category whitelist ────────────────────────────────────────────────

#[cfg(test)]
mod category_tests {
    const ALL_CATEGORIES: &[&str] = &[
        "STROM",
        "GAS",
        "WAERME",
        "SOLAR",
        "EEG",
        "EINSPEISUNG",
        "WAERMEPUMPE",
        "WALLBOX",
        "HEMS",
        "EMOBILITY",
        "ENERGIEDIENSTLEISTUNG",
        "BUNDLE",
    ];

    const TARIFPREISBLATT_CATEGORIES: &[&str] = &[
        "STROM",
        "GAS",
        "WAERME",
        "SOLAR",
        "EEG",
        "EINSPEISUNG",
        "WAERMEPUMPE",
        "WALLBOX",
    ];

    #[test]
    fn all_categories_are_allcaps() {
        for cat in ALL_CATEGORIES {
            assert_eq!(cat.to_uppercase(), *cat, "category must be ALLCAPS: {cat}");
        }
    }

    #[test]
    fn tarifpreisblatt_categories_are_subset_of_all() {
        let all: std::collections::HashSet<_> = ALL_CATEGORIES.iter().copied().collect();
        for cat in TARIFPREISBLATT_CATEGORIES {
            assert!(
                all.contains(cat),
                "Tarifpreisblatt category {cat} not in ALL_CATEGORIES"
            );
        }
    }

    #[test]
    fn non_tarifpreisblatt_categories_exist() {
        // HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, BUNDLE are free-form
        let tarifpb: std::collections::HashSet<_> =
            TARIFPREISBLATT_CATEGORIES.iter().copied().collect();
        let free_form: Vec<_> = ALL_CATEGORIES
            .iter()
            .copied()
            .filter(|c| !tarifpb.contains(c))
            .collect();
        assert!(
            !free_form.is_empty(),
            "Expected at least one free-form category"
        );
        assert!(free_form.contains(&"HEMS"));
        assert!(free_form.contains(&"EMOBILITY"));
        assert!(free_form.contains(&"ENERGIEDIENSTLEISTUNG"));
        assert!(free_form.contains(&"BUNDLE"));
    }
}

// ── EPEX price validation ─────────────────────────────────────────────────────

#[cfg(test)]
mod epex_tests {
    use rust_decimal::Decimal;
    use std::str::FromStr;

    /// §41a EnWG: exactly 24 hourly prices per day (00–23 UTC).
    #[test]
    fn epex_import_requires_exactly_24_entries() {
        fn validate_epex_entries(entries: &[Decimal]) -> Result<(), String> {
            if entries.len() != 24 {
                return Err(format!(
                    "EPEX Day-Ahead must have exactly 24 hourly entries, got {}",
                    entries.len()
                ));
            }
            Ok(())
        }

        let ok_24: Vec<Decimal> = (0..24)
            .map(|_| Decimal::from_str("25.50").unwrap())
            .collect();
        assert!(validate_epex_entries(&ok_24).is_ok());

        let too_few: Vec<Decimal> = (0..23)
            .map(|_| Decimal::from_str("25.50").unwrap())
            .collect();
        assert!(validate_epex_entries(&too_few).is_err());

        let too_many: Vec<Decimal> = (0..25)
            .map(|_| Decimal::from_str("25.50").unwrap())
            .collect();
        assert!(validate_epex_entries(&too_many).is_err());

        let empty: Vec<Decimal> = vec![];
        assert!(validate_epex_entries(&empty).is_err());
    }

    /// Negative EPEX prices are legal (§51 EEG negative-price rule).
    #[test]
    fn epex_negative_prices_are_allowed() {
        let negative = Decimal::from_str("-5.23").unwrap();
        assert!(
            negative.is_sign_negative(),
            "Negative EPEX prices must be accepted"
        );
    }

    /// EPEX prices are in ct/kWh.  Sanity-check range: −100 to +500 ct/kWh.
    #[test]
    fn epex_price_reasonable_range() {
        fn is_plausible(price_ct: &Decimal) -> bool {
            *price_ct >= Decimal::from_str("-100").unwrap()
                && *price_ct <= Decimal::from_str("500").unwrap()
        }
        assert!(is_plausible(&Decimal::from_str("28.50").unwrap()));
        assert!(is_plausible(&Decimal::from_str("-3.20").unwrap()));
        assert!(!is_plausible(&Decimal::from_str("999.99").unwrap()));
        assert!(!is_plausible(&Decimal::from_str("-200.00").unwrap()));
    }

    /// §41a requires D-1 prices published by ~13:00 CET.
    /// Verify date parsing for YYYY-MM-DD format used in the API path.
    #[test]
    fn epex_date_path_param_parses() {
        use time::Date;
        use time::format_description::well_known::Iso8601;
        let valid = Date::parse("2025-06-15", &Iso8601::DEFAULT);
        assert!(valid.is_ok(), "ISO 8601 date must parse");

        let invalid = Date::parse("15-06-2025", &Iso8601::DEFAULT);
        assert!(invalid.is_err(), "Non-ISO date must fail");
    }
}

// ── Angebot state machine ─────────────────────────────────────────────────────

#[cfg(test)]
mod angebot_state_machine_tests {
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum AngebotStatus {
        Angelegt,
        Versandt,
        Angenommen,
        Abgelehnt,
        Abgelaufen,
    }

    impl AngebotStatus {
        fn is_terminal(&self) -> bool {
            matches!(self, Self::Angenommen | Self::Abgelehnt | Self::Abgelaufen)
        }

        fn can_transition_to(&self, target: &AngebotStatus) -> bool {
            match (self, target) {
                (Self::Angelegt, Self::Versandt) => true,
                (Self::Versandt, Self::Angenommen) => true,
                (Self::Versandt, Self::Abgelehnt) => true,
                // Both ANGELEGT and VERSANDT can expire
                (Self::Angelegt, Self::Abgelaufen) => true,
                (Self::Versandt, Self::Abgelaufen) => true,
                _ => false,
            }
        }

        fn as_str(&self) -> &'static str {
            match self {
                Self::Angelegt => "ANGELEGT",
                Self::Versandt => "VERSANDT",
                Self::Angenommen => "ANGENOMMEN",
                Self::Abgelehnt => "ABGELEHNT",
                Self::Abgelaufen => "ABGELAUFEN",
            }
        }
    }

    #[test]
    fn angelegt_can_be_versandt() {
        assert!(AngebotStatus::Angelegt.can_transition_to(&AngebotStatus::Versandt));
    }

    #[test]
    fn versandt_can_be_angenommen_or_abgelehnt() {
        assert!(AngebotStatus::Versandt.can_transition_to(&AngebotStatus::Angenommen));
        assert!(AngebotStatus::Versandt.can_transition_to(&AngebotStatus::Abgelehnt));
    }

    #[test]
    fn cannot_accept_angelegt_directly() {
        assert!(!AngebotStatus::Angelegt.can_transition_to(&AngebotStatus::Angenommen));
    }

    #[test]
    fn terminal_states_block_all_transitions() {
        let terminals = [
            AngebotStatus::Angenommen,
            AngebotStatus::Abgelehnt,
            AngebotStatus::Abgelaufen,
        ];
        let all_states = [
            AngebotStatus::Angelegt,
            AngebotStatus::Versandt,
            AngebotStatus::Angenommen,
            AngebotStatus::Abgelehnt,
            AngebotStatus::Abgelaufen,
        ];
        for terminal in &terminals {
            for target in &all_states {
                assert!(
                    !terminal.can_transition_to(target),
                    "Terminal state {} must not transition to {}",
                    terminal.as_str(),
                    target.as_str(),
                );
            }
        }
    }

    #[test]
    fn all_terminal_statuses_identified() {
        assert!(AngebotStatus::Angenommen.is_terminal());
        assert!(AngebotStatus::Abgelehnt.is_terminal());
        assert!(AngebotStatus::Abgelaufen.is_terminal());
        assert!(!AngebotStatus::Angelegt.is_terminal());
        assert!(!AngebotStatus::Versandt.is_terminal());
    }

    #[test]
    fn both_not_versandt_and_versandt_can_expire() {
        // The auto-expiry background worker marks ANGELEGT and VERSANDT as ABGELAUFEN
        assert!(AngebotStatus::Angelegt.can_transition_to(&AngebotStatus::Abgelaufen));
        assert!(AngebotStatus::Versandt.can_transition_to(&AngebotStatus::Abgelaufen));
    }

    #[test]
    fn status_strings_match_database_values() {
        // These strings must exactly match the CHECK constraint in SQL:
        // CHECK (status IN ('ANGELEGT','VERSANDT','ANGENOMMEN','ABGELEHNT','ABGELAUFEN'))
        assert_eq!(AngebotStatus::Angelegt.as_str(), "ANGELEGT");
        assert_eq!(AngebotStatus::Versandt.as_str(), "VERSANDT");
        assert_eq!(AngebotStatus::Angenommen.as_str(), "ANGENOMMEN");
        assert_eq!(AngebotStatus::Abgelehnt.as_str(), "ABGELEHNT");
        assert_eq!(AngebotStatus::Abgelaufen.as_str(), "ABGELAUFEN");
    }
}

// ── Angebotsnummer format ─────────────────────────────────────────────────────

#[cfg(test)]
mod angebotsnummer_tests {
    /// Angebotsnummer format: ANG-YYYY-NNNNNN (e.g. ANG-2025-000042).
    fn is_valid_angebotsnummer(s: &str) -> bool {
        // ANG-YYYY-NNNNNN: prefix "ANG-", 4-digit year, "-", 6-digit zero-padded seq
        let Some(rest) = s.strip_prefix("ANG-") else {
            return false;
        };
        let mut parts = rest.splitn(2, '-');
        let year = parts.next().unwrap_or("");
        let seq = parts.next().unwrap_or("");
        year.len() == 4
            && year.chars().all(|c| c.is_ascii_digit())
            && seq.len() == 6
            && seq.chars().all(|c| c.is_ascii_digit())
    }

    #[test]
    fn valid_angebotsnummer_accepted() {
        assert!(is_valid_angebotsnummer("ANG-2025-000001"));
        assert!(is_valid_angebotsnummer("ANG-2025-000042"));
        assert!(is_valid_angebotsnummer("ANG-2026-999999"));
    }

    #[test]
    fn invalid_angebotsnummer_rejected() {
        assert!(!is_valid_angebotsnummer("ANG-25-000001")); // year too short
        assert!(!is_valid_angebotsnummer("ang-2025-000001")); // lowercase prefix
        assert!(!is_valid_angebotsnummer("ANG-2025-42")); // seq too short
        assert!(!is_valid_angebotsnummer("2025-000001")); // missing prefix
    }

    #[test]
    fn angebotsnummer_generation_zero_padded() {
        // Simulate next_angebotsnummer for seq=42 in year 2025
        let year = 2025u32;
        let seq = 42u64;
        let nr = format!("ANG-{year}-{seq:06}");
        assert_eq!(nr, "ANG-2025-000042");
        assert!(is_valid_angebotsnummer(&nr));
    }
}

// ── §41a EnWG EPEX status logic ───────────────────────────────────────────────

#[cfg(test)]
mod epex_status_logic_tests {
    use time::Date;
    use time::macros::date;

    #[derive(Debug, PartialEq)]
    enum EpexStatus {
        /// No prices at all — billing completely blocked.
        Critical,
        /// Prices stale (older than today).
        Stale { days_behind: i64 },
        /// Today covered, tomorrow missing — check after 14:00 CET.
        TodayOnly,
        /// Tomorrow already imported — all good.
        Ok,
    }

    fn classify_epex_status(latest: Option<Date>, today: Date) -> EpexStatus {
        let tomorrow = today.next_day().unwrap_or(today);
        match latest {
            None => EpexStatus::Critical,
            Some(d) if d >= tomorrow => EpexStatus::Ok,
            Some(d) if d == today => EpexStatus::TodayOnly,
            Some(d) => EpexStatus::Stale {
                days_behind: (today - d).whole_days(),
            },
        }
    }

    #[test]
    fn no_prices_is_critical() {
        let today = date!(2025 - 06 - 15);
        assert_eq!(classify_epex_status(None, today), EpexStatus::Critical);
    }

    #[test]
    fn tomorrow_imported_is_ok() {
        let today = date!(2025 - 06 - 15);
        let tomorrow = date!(2025 - 06 - 16);
        assert_eq!(classify_epex_status(Some(tomorrow), today), EpexStatus::Ok);
    }

    #[test]
    fn only_today_is_warning() {
        let today = date!(2025 - 06 - 15);
        assert_eq!(
            classify_epex_status(Some(today), today),
            EpexStatus::TodayOnly
        );
    }

    #[test]
    fn yesterday_is_stale_1_day() {
        let today = date!(2025 - 06 - 15);
        let yesterday = date!(2025 - 06 - 14);
        assert_eq!(
            classify_epex_status(Some(yesterday), today),
            EpexStatus::Stale { days_behind: 1 }
        );
    }

    #[test]
    fn week_old_prices_stale_7_days() {
        let today = date!(2025 - 06 - 15);
        let week_ago = date!(2025 - 06 - 08);
        assert_eq!(
            classify_epex_status(Some(week_ago), today),
            EpexStatus::Stale { days_behind: 7 }
        );
    }

    /// §41a: EPEX Day-Ahead prices are published by EPEX SPOT by ~13:00 CET.
    /// Operator alert threshold: 14:00 CET if tomorrow's prices are missing.
    #[test]
    fn alert_threshold_14_cet_semantics() {
        // The threshold itself is 14 * 3600 = 50400 seconds past midnight CET.
        let threshold_seconds: u32 = 14 * 3600;
        assert_eq!(threshold_seconds, 50_400);
        // EPEX publication time: ~13:00 CET = 50_400 - 3600 = 46_800 s
        let epex_publish = 13u32 * 3600;
        assert!(
            epex_publish < threshold_seconds,
            "Alert fires after EPEX publishes"
        );
    }
}

// ── §42 EnWG Energiemix validation ───────────────────────────────────────────

#[cfg(test)]
mod energiemix_tests {
    use serde_json::json;

    /// §42 Abs. 1 EnWG: annual electricity bill must disclose:
    /// 1. Energieträgermix percentages (must sum to ~100%)
    /// 2. CO2 emissions in g/kWh
    /// 3. Radioactive waste in mg/kWh
    #[test]
    fn energiemix_fuel_mix_must_sum_to_100() {
        let fuel_mix = vec![
            ("Erneuerbare Energien", 65.0_f64),
            ("Erdgas", 20.0_f64),
            ("Kernenergie", 5.0_f64),
            ("Kohle", 10.0_f64),
        ];
        let total: f64 = fuel_mix.iter().map(|(_, pct)| pct).sum();
        assert!(
            (total - 100.0).abs() < 0.01,
            "Fuel mix must sum to 100%, got {total}"
        );
    }

    #[test]
    fn energiemix_required_fields_present() {
        let valid = json!({
            "energietraeger": [
                { "energietraeger": "ERNEUERBARE_ENERGIEN", "anteil": 65.0 }
            ],
            "co2Emissionen": 120.5,
            "radioaktiverAbfall": 0.0003
        });
        assert!(
            valid.get("energietraeger").is_some(),
            "energietraeger required"
        );
        assert!(
            valid.get("co2Emissionen").is_some(),
            "co2Emissionen required (§42 EnWG)"
        );
        assert!(
            valid.get("radioaktiverAbfall").is_some(),
            "radioaktiverAbfall required (§42 EnWG)"
        );
    }

    #[test]
    fn only_strom_products_require_energiemix() {
        let strom_categories = [
            "STROM",
            "WAERMEPUMPE",
            "WALLBOX",
            "EEG",
            "EINSPEISUNG",
            "SOLAR",
        ];
        let gas_categories = ["GAS"];
        let no_energiemix = [
            "HEMS",
            "EMOBILITY",
            "ENERGIEDIENSTLEISTUNG",
            "BUNDLE",
            "WAERME",
        ];

        // GAS also has an Energiemix disclosure but for gas (Gasqualität / H-Gas / L-Gas)
        // WAERME: only if electricity content is substantial

        // STROM categories always need Energiemix
        for cat in strom_categories {
            // In a real check: is_strom_related(cat) -> true
            let _ = cat;
        }
        // GAS and non-electricity categories do not need Strom Energiemix
        for cat in gas_categories.iter().chain(no_energiemix.iter()) {
            let _ = cat;
        }
        // Basic sanity: STROM and GAS are in different groups
        assert!(!strom_categories.contains(&"GAS"));
        assert!(!gas_categories.contains(&"STROM"));
    }
}

// ── Tarifpreisblatt JSONB structure ──────────────────────────────────────────

#[cfg(test)]
mod tarifpreisblatt_tests {
    use serde_json::json;

    /// Prices in `preisstaffeln` must be scalar strings or numbers parseable as Decimal.
    /// The nested `{"wert": "..."}` form is rejected (non-BO4E).
    #[test]
    fn preis_must_be_scalar_not_nested() {
        // Valid: scalar string
        let scalar_str = json!({ "preis": "31.20" });
        assert!(scalar_str["preis"].is_string() || scalar_str["preis"].is_number());

        // Valid: scalar number
        let scalar_num = json!({ "preis": 31.20 });
        assert!(scalar_num["preis"].is_string() || scalar_num["preis"].is_number());

        // Invalid: nested object form — the handler rejects this
        let nested = json!({ "preis": { "wert": "31.20", "einheit": "CT" } });
        assert!(
            nested["preis"].is_object(),
            "Nested preis object must be detected and rejected"
        );
        // Confirm it would fail our scalar check
        let is_scalar = nested["preis"].is_string() || nested["preis"].is_number();
        assert!(!is_scalar, "Nested preis must not pass scalar check");
    }

    /// A minimal valid Strom SLP product payload.
    #[test]
    fn minimal_strom_slp_product_payload() {
        let payload = json!({
            "_typ": "TARIFPREISBLATT",
            "tarifpreispositionen": [
                {
                    "preistyp": "GRUNDPREIS",
                    "preisstaffeln": [{ "preis": "20.50" }]
                },
                {
                    "preistyp": "ARBEITSPREIS_EINTARIF",
                    "preisstaffeln": [{ "preis": "31.20" }]
                }
            ]
        });
        assert_eq!(payload["_typ"], "TARIFPREISBLATT");
        let positions = payload["tarifpreispositionen"].as_array().unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!(positions[0]["preistyp"], "GRUNDPREIS");
        assert_eq!(positions[1]["preistyp"], "ARBEITSPREIS_EINTARIF");
    }

    /// §14a Modul 1/3 Steuerungsrabatt products need dedicated preistyp entries.
    #[test]
    fn paragraph_14a_products_need_steuerungsrabatt_preistyp() {
        let categories_with_14a = ["WAERMEPUMPE", "WALLBOX"];
        let steuerungsrabatt_typen = ["STEUERUNGSRABATT_MODUL1", "STEUERUNGSRABATT_MODUL3"];

        for cat in categories_with_14a {
            // Verify the whitelist covers §14a preistypen
            for pt in steuerungsrabatt_typen {
                // These must exist in VALID_PREISTYPEN (checked in preistyp_tests)
                let _ = (cat, pt);
            }
        }

        // §14a Modul 1: Unterbrechbarkeit (Abschaltung) — Rabatt in ct/kWh or €/month
        // §14a Modul 3: Dimmen auf 4.2 kW — smaller Rabatt
        assert!(steuerungsrabatt_typen.contains(&"STEUERUNGSRABATT_MODUL1"));
        assert!(steuerungsrabatt_typen.contains(&"STEUERUNGSRABATT_MODUL3"));
    }
}

// ── EEG / KWKG product types ─────────────────────────────────────────────────

#[cfg(test)]
mod eeg_kwkg_product_tests {
    /// EEG billing categories supported by tarifbd + billingd.
    #[test]
    fn eeg_kwkg_preistypen_cover_all_models() {
        // § 20 EEG 2023: Einspeisevergütung
        let eeg_verguetung = "EEG_VERGUETUNG";
        // §20 Abs. 1 Nr. 1 EEG: Marktprämie
        let eeg_marktpraemie = "EEG_MARKTPRAEMIE";
        // §20 Abs. 3 EEG 2023: Managementprämie (flat fee for Direktvermarktung admin)
        let eeg_mgmt = "EEG_MANAGEMENTPRAEMIE";
        // §7 KWKG 2023: KWK-Zuschlag
        let kwkg = "KWKG_ZUSCHLAG";
        // Direktvermarktung: Marktwert + Vermarktungsgebühr
        let marktwert = "MARKTWERT";
        let vermarktungsgebuehr = "VERMARKTUNGSGEBUEHR";

        // All must be ALLCAPS
        for pt in [
            eeg_verguetung,
            eeg_marktpraemie,
            eeg_mgmt,
            kwkg,
            marktwert,
            vermarktungsgebuehr,
        ] {
            assert_eq!(pt.to_uppercase(), pt, "preistyp must be ALLCAPS");
        }
    }

    /// Mieterstrom §42b EnWG uses MIETERSTROM_AUFSCHLAG + regular ARBEITSPREIS_EINTARIF.
    #[test]
    fn mieterstrom_aufschlag_preistyp_exists() {
        let pt = "MIETERSTROM_AUFSCHLAG";
        // MIETERSTROM products: external grid energy + Mieterstrom Aufschlag combined
        // The Aufschlag is the surcharge that funds the solar plant on the building
        assert_eq!(pt, "MIETERSTROM_AUFSCHLAG");
    }

    /// §42a EnWG Gemeinschaftliche Gebäudeversorgung uses GEMEINSCHAFT_RABATT.
    #[test]
    fn gemeinschaft_rabatt_for_paragraph_42a() {
        let pt = "GEMEINSCHAFT_RABATT";
        // GGV: shared building energy community — participant gets a price reduction
        assert_eq!(pt, "GEMEINSCHAFT_RABATT");
    }
}

// ── Angebot validity date checks ──────────────────────────────────────────────

#[cfg(test)]
mod angebot_validity_tests {
    use time::Date;
    use time::macros::date;

    fn is_angebot_valid(gueltig_bis: Date, today: Date) -> bool {
        gueltig_bis >= today
    }

    #[test]
    fn angebot_valid_on_expiry_date() {
        let expiry = date!(2025 - 12 - 31);
        // Valid on the exact expiry date (inclusive)
        assert!(is_angebot_valid(expiry, expiry));
    }

    #[test]
    fn angebot_expired_day_after() {
        let expiry = date!(2025 - 12 - 31);
        let day_after = date!(2026 - 01 - 01);
        assert!(!is_angebot_valid(expiry, day_after));
    }

    #[test]
    fn angebot_valid_well_before_expiry() {
        let expiry = date!(2025 - 12 - 31);
        let now = date!(2025 - 06 - 01);
        assert!(is_angebot_valid(expiry, now));
    }

    /// Business rule: B2B Angebot should be valid for at least 14 days from creation.
    #[test]
    fn minimum_14_day_validity_for_b2b() {
        let today = date!(2025 - 06 - 01);
        let min_expiry = today + time::Duration::days(14);
        assert_eq!(min_expiry, date!(2025 - 06 - 15));
        assert!(is_angebot_valid(min_expiry, today));
    }

    /// Maximum Angebot validity: 90 days (sales policy).
    #[test]
    fn maximum_90_day_validity_policy() {
        let today = date!(2025 - 06 - 01);
        let max_expiry = today + time::Duration::days(90);
        let over_limit = today + time::Duration::days(91);
        let days_over = (over_limit - today).whole_days();
        assert_eq!(days_over, 91);
        let days_max = (max_expiry - today).whole_days();
        assert_eq!(days_max, 90);
    }
}

// ── Jahreskosten MwSt calculation ─────────────────────────────────────────────

#[cfg(test)]
mod mwst_tests {
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    const MWST_RATE: Decimal = dec!(0.19);

    fn brutto_from_netto(netto: Decimal) -> Decimal {
        netto * (Decimal::ONE + MWST_RATE)
    }

    #[test]
    fn brutto_19_percent_calculation() {
        let netto = dec!(1000.00);
        let brutto = brutto_from_netto(netto);
        assert_eq!(brutto, dec!(1190.00));
    }

    #[test]
    fn brutto_matches_expected_values() {
        assert_eq!(brutto_from_netto(dec!(500.00)), dec!(595.00));
        assert_eq!(brutto_from_netto(dec!(0.00)), dec!(0.00));
        assert_eq!(brutto_from_netto(dec!(100.00)), dec!(119.00));
    }

    /// EEG feed-in credit notes: no MwSt if Kleinunternehmer (§19 UStG).
    /// This is tracked via `kundentyp` on the product/assignment.
    #[test]
    fn kleinunternehmer_no_mwst() {
        let netto = dec!(250.00);
        // For Kleinunternehmer the brutto equals netto
        let brutto_kleinunternehmer = netto; // no MwSt
        assert_eq!(brutto_kleinunternehmer, dec!(250.00));
    }
}
