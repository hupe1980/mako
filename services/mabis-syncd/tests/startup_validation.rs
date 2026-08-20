//! The configuration checks run at start-up, where they can still refuse.
//!
//! # Why this is a test
//!
//! A configuration check that is never reached is worse than none, because the
//! doc comment claims a guarantee. Both checks below refuse a deployment that
//! could otherwise start cleanly and fail at 05:00 on the
//! Erstaufschlag-Werktag — after a month of metering data has been aggregated
//! and the run's version number consumed.

use mabis_syncd::config::Config;

/// A valid Bilanzierungsgebiet: a 16-character EIC of ENTSO-E object type `Y`.
const VALID_GEBIET: &str = "11YMAKO-TEST-01U";

fn config_with(extra: &str) -> Config {
    // Top-level keys must precede the tables, or TOML reads them as belonging
    // to the last one opened.
    let toml_src = format!(
        r#"
{extra}

[http]
addr = "0.0.0.0:8880"

[database]
url = "postgres://localhost/mabis"

[identity]
tenant         = "9900357000004"
sender_mp_id   = "9900357000004"
receiver_mp_id = "9900077000006"
bilanzierungsgebiet_id = "{VALID_GEBIET}"

[edmd]
url     = "http://edmd:8380"
api_key = "k"

[marktd]
url     = "http://marktd:8180"
api_key = "k"

[makod]
url     = "http://makod:8080"
api_key = "k"
"#
    );
    toml::from_str(&toml_src).expect("config parses")
}

/// The default deployment validates.
#[test]
fn the_documented_configuration_is_accepted() {
    config_with("").validate().expect("the default is valid");
}

/// The Hub target refuses at start-up, and says why.
///
/// BK6-24-210 has no Beschluss: no wire format, endpoint or payload shape has
/// been published, so an implementation would be invention. An invented format
/// that reaches a real Hub is indistinguishable, at the point of failure, from
/// a correct one that was rejected.
#[test]
fn an_unimplemented_submission_target_refuses_at_startup() {
    let err = config_with(r#"submission_target = "mabis-hub""#)
        .validate()
        .expect_err("mabis-hub has no implementation");
    let msg = err.to_string();
    assert!(
        msg.contains("BK6-24-210"),
        "the refusal must cite why: {msg}"
    );
    assert!(
        msg.contains("biko-bilateral"),
        "and name the way forward: {msg}"
    );
}

/// A Bilanzkreis in the Bilanzierungsgebiet field refuses at start-up.
///
/// Both are 16-character EICs, so only the object type separates them — `Y`
/// (Area) from `X` (Party) — and `LOC+107` carries the value as free text. The
/// BIKO would accept either, which is why this has to be caught here.
#[test]
fn a_bilanzkreis_is_not_a_bilanzierungsgebiet() {
    let err = config_with("")
        .validate_with_gebiet("11XSUEDWESTSTRO8")
        .expect_err("an X-type EIC is a Bilanzkreis, not a territory");
    let msg = err.to_string();
    assert!(msg.contains("bilanzierungsgebiet_id"), "{msg}");
    assert!(
        msg.contains('Y') && msg.contains('X'),
        "the message must name both object types: {msg}"
    );
}

/// A value that is not an EIC at all is refused too.
#[test]
fn a_free_text_territory_is_refused() {
    assert!(
        config_with("")
            .validate_with_gebiet("Regelzone Nord")
            .is_err()
    );
    assert!(config_with("").validate_with_gebiet("").is_err());
}

/// Test-only helper: swap the territory and re-validate.
trait ValidateWithGebiet {
    fn validate_with_gebiet(self, gebiet: &str) -> anyhow::Result<()>;
}

impl ValidateWithGebiet for Config {
    fn validate_with_gebiet(mut self, gebiet: &str) -> anyhow::Result<()> {
        self.identity.bilanzierungsgebiet_id = gebiet.to_owned();
        self.validate()
    }
}
