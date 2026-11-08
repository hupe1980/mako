//! The EoG command processd posts to makod, read through the request type
//! makod's door reads it as ([`mako_service::json_from_value`] refuses an
//! undeclared key exactly as makod does).
//!
//! makod reads the Marktlokation's Stammdaten from marktd, so the command
//! carries none: a body with a key makod does not declare is refused on every
//! gap, and a missing field is the same. Asserting that keys are present would
//! pass both.

use mako_markt::commands::{
    GELI_EOG_ANMELDEN, GPKE_EOG_ANMELDEN, GeliEogAnmelden, GpkeEogAnmelden,
};
use mako_markt::domain::Sparte;
use processd::eog_module::{eog_anmelden_command, eog_pid};
use time::macros::date;

const MALO: &str = "51238696012";
const GV: &str = "9900001000002";

#[test]
fn the_strom_eog_command_is_the_body_makod_reads() {
    let cmd = eog_anmelden_command(Sparte::Strom, MALO, GV, date!(2026 - 12 - 01), "ZT6");
    assert_eq!(cmd.command, GPKE_EOG_ANMELDEN);
    assert_eq!(eog_pid(Sparte::Strom), 55013);
    let body: GpkeEogAnmelden =
        mako_service::json_from_value(cmd.payload).expect("gpke.eog.anmelden body");
    assert_eq!(body.malo_id, MALO);
    assert_eq!(body.gv_mp_id, GV);
    assert_eq!(body.process_date, "2026-12-01");
    assert_eq!(body.transaktionsgrund, "ZT6");
}

/// The Gas door makes the Bilanzierungsbeginn Muss (`SG4 DTM+158`, UTILMD AHB
/// Gas 1.2 Kap. 5.9 Bedingung `[18]`); processd states it as the
/// Zuordnungsbeginn, which GeLi Gas 3.0 Kap. 3.3.1 lets an EoG Zuordnung set
/// for Bilanzierung as for Netznutzung.
#[test]
fn the_gas_eog_command_is_the_body_makod_reads() {
    let cmd = eog_anmelden_command(Sparte::Gas, MALO, GV, date!(2026 - 12 - 01), "ZT6");
    assert_eq!(cmd.command, GELI_EOG_ANMELDEN);
    assert_eq!(eog_pid(Sparte::Gas), 44013);
    let body: GeliEogAnmelden =
        mako_service::json_from_value(cmd.payload).expect("geli.eog.anmelden body");
    assert_eq!(body.process_date, "2026-12-01");
    assert_eq!(body.bilanzierungsbeginn, body.process_date);
    assert_eq!(body.transaktionsgrund, "ZT6");
}

/// The check is makod's: a key its type does not declare refuses the body —
/// the Stammdaten a caller used to send among them.
#[test]
fn a_body_with_a_key_makod_does_not_declare_is_refused() {
    let mut payload =
        eog_anmelden_command(Sparte::Gas, MALO, GV, date!(2026 - 12 - 01), "ZT6").payload;
    payload["stammdaten"] = serde_json::json!({ "lokationen": [] });
    let err = mako_service::json_from_value::<GeliEogAnmelden>(payload)
        .expect_err("an undeclared key is refused");
    assert!(err.to_string().contains("stammdaten"), "{err}");
}
