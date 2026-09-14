//! Guard: every `demos/` config file `processd` loads is one `processd` accepts.
//!
//! A demo config is shipped surface in the same way a demo request body is: it
//! is what an operator copies when writing their own. `figment` merges the TOML
//! with `PROCESSD_*` environment variables, so a key the struct does not declare is
//! dropped in silence unless the config denies unknown fields — which is how a
//! setting can appear configured and do nothing.
//!
//! What this asserts is **no unknown key**. A *missing* one is fine: the demo
//! stacks supply `database.url` and the secrets through the environment, so a
//! file that does not deserialise for want of a required field still says
//! nothing about the keys it does carry.

/// Every demo config this service is started with.
const DEMO_CONFIGS: &[&str] = &["demos/nb-stp/processd.toml"];

#[test]
fn the_demo_configs_name_no_key_the_service_ignores() {
    for rel in DEMO_CONFIGS {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
        if let Err(e) = toml::from_str::<processd::config::Config>(&src) {
            let msg = e.to_string();
            assert!(
                !msg.contains("unknown field"),
                "{rel} names a key `processd` does not have — fix the demo, or the config:\n{msg}"
            );
        }
    }
}

/// A demo config the service refuses to start with is a broken demo.
///
/// The sibling test asks only whether every key is one the struct declares. A
/// config can pass that and still be refused at startup: the auth posture is a
/// second gate, and `allow_insecure_no_auth` is how a demo stack opts out of it
/// deliberately. Asserting the parse without the posture leaves the demo's
/// `docker compose up` as the only thing that notices.
#[test]
fn the_demo_configs_start() {
    for rel in DEMO_CONFIGS {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
        // A config that needs an environment-supplied field cannot be built
        // here; that is the sibling test's business, not this one's.
        let Ok(cfg) = toml::from_str::<processd::config::Config>(&src) else {
            continue;
        };
        if let Err(e) = cfg.check_auth_posture() {
            panic!(
                "{rel} parses but `processd` refuses to start with it. Set \
                 `allow_insecure_no_auth = true` in the demo, or configure the door:\n{e}"
            );
        }
    }
}
