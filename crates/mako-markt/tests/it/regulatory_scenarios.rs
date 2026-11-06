//! `Rollenzuordnung` validity: an assignment is valid on a day inside its
//! closed `[valid_from, valid_to]` window, and open-ended when `valid_to` is
//! `None`.

mod tests {
    use mako_markt::repository::Rollenzuordnung;
    use time::macros::date;

    // ═════════════════════════════════════════════════════════════════════
    // Rollenzuordnung temporal semantics
    // ═════════════════════════════════════════════════════════════════════

    /// Open-ended assignment (valid_to = None) is valid today.
    #[test]
    fn rollenzuordnung_open_ended_is_valid_today() {
        let a = Rollenzuordnung {
            zuordnungstyp: "NB".to_owned(),
            rollencodenummer: "9904234560001".to_owned(),
            valid_from: date!(2020 - 01 - 01),
            valid_to: None,
        };
        let today = mako_fristen::heute();
        let valid = a.valid_from <= today && a.valid_to.is_none_or(|to| to >= today);
        assert!(valid, "open-ended assignment must be valid today");
    }

    /// Assignment with past valid_to is NOT valid today.
    #[test]
    fn rollenzuordnung_past_assignment_is_expired() {
        let a = Rollenzuordnung {
            zuordnungstyp: "LF".to_owned(),
            rollencodenummer: "9910000000001".to_owned(),
            valid_from: date!(2020 - 01 - 01),
            valid_to: Some(date!(2021 - 12 - 31)),
        };
        let today = mako_fristen::heute();
        let valid = a.valid_from <= today && a.valid_to.is_none_or(|to| to >= today);
        assert!(!valid, "past assignment must be expired");
    }

    /// Future assignment is not yet valid.
    #[test]
    fn rollenzuordnung_future_assignment_not_yet_valid() {
        let a = Rollenzuordnung {
            zuordnungstyp: "MSB".to_owned(),
            rollencodenummer: "9900001234567".to_owned(),
            valid_from: date!(2099 - 01 - 01), // far future
            valid_to: None,
        };
        let today = mako_fristen::heute();
        let valid = a.valid_from <= today && a.valid_to.is_none_or(|to| to >= today);
        assert!(!valid, "future assignment must not yet be valid");
    }
}
