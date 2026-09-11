//! Guard: every `demos/` config file `productd` loads is one `productd` accepts.
//!
//! A demo config is shipped surface in the same way a demo request body is: it
//! is what an operator copies when writing their own. `figment` merges the TOML
//! with `PRODUCTD_*` environment variables, so a key the struct does not declare is
//! dropped in silence unless the config denies unknown fields — which is how a
//! setting can appear configured and do nothing.
//!
//! What this asserts is **no unknown key**. A *missing* one is fine: the demo
//! stacks supply `database.url` and the secrets through the environment, so a
//! file that does not deserialise for want of a required field still says
//! nothing about the keys it does carry.

/// Every demo config this service is started with.
const DEMO_CONFIGS: &[&str] = &["demos/o2c/productd.toml"];

#[test]
fn the_demo_configs_name_no_key_the_service_ignores() {
    for rel in DEMO_CONFIGS {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
        if let Err(e) = toml::from_str::<productd::config::ProductdConfig>(&src) {
            let msg = e.to_string();
            assert!(
                !msg.contains("unknown field"),
                "{rel} names a key `productd` does not have — fix the demo, or the config:\n{msg}"
            );
        }
    }
}
