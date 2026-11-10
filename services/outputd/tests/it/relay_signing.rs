//! A relay with no credential is refused at startup; the signing of the push
//! itself is held by `store_integration`'s relay-outage test.
//!
//! The body of one of these POSTs is the customer's document itself — the
//! invoice or Mahnung base64-encoded — with their name, e-mail address, MaLo
//! and Kundennummer beside it. Unsigned, the relay cannot tell a mako push from
//! any other POST that reached its URL, and a captured one replays forever.

use outputd::config::DeliveryConfig;

/// A relay URL with no credential is named at startup, not discovered when the
/// first invoice has already been POSTed to it.
#[test]
fn a_relay_url_without_a_credential_is_reported() {
    let mut cfg = DeliveryConfig::default();
    assert!(
        cfg.unsigned_relays().is_empty(),
        "a deployment that configures no relay has nothing to report"
    );

    cfg.email_relay_url = Some("https://mail.example.test/send".to_owned());
    assert_eq!(cfg.unsigned_relays(), vec!["email_relay_api_key"]);

    cfg.email_relay_api_key = Some(secrecy::SecretString::from("k"));
    assert!(cfg.unsigned_relays().is_empty());

    cfg.postal_relay_url = Some("https://print.example.test/spool".to_owned());
    cfg.erp_webhook_url = Some("https://erp.example.test/hook".to_owned());
    assert_eq!(
        cfg.unsigned_relays(),
        vec!["postal_relay_api_key", "erp_api_key"]
    );

    // An empty URL is not a configured relay — the same emptiness rule the
    // worker applies before it claims a row.
    let empty = DeliveryConfig {
        erp_webhook_url: Some(String::new()),
        ..Default::default()
    };
    assert!(empty.unsigned_relays().is_empty());
}
