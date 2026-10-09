//! Guard: every `demos/` config file `makod` loads is one `makod` accepts.
//!
//! A demo config is shipped surface in the same way a demo request body is: it
//! is what an operator copies when writing their own. `ConfigFile` denies
//! unknown fields, so a key it does not declare is a startup failure rather than
//! a silent drop — and a demo carrying one is a stack that does not come up.
//!
//! What this asserts is **no unknown key**. A *missing* one is fine: the demo
//! stack supplies the storage path and the secrets through the environment and
//! the command line, so a file that does not deserialise for want of a required
//! field still says nothing about the keys it does carry.

/// Every demo config this service is started with.
const DEMO_CONFIGS: &[&str] = &["demos/nb-stp/makod.toml"];

#[test]
fn the_demo_configs_name_no_key_the_service_ignores() {
    for rel in DEMO_CONFIGS {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(rel);
        let src = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
        if let Err(e) = toml::from_str::<makod::config::ConfigFile>(&src) {
            let msg = e.to_string();
            assert!(
                !msg.contains("unknown field"),
                "{rel} names a key `makod` does not have — fix the demo, or the config:\n{msg}"
            );
        }
    }
}
