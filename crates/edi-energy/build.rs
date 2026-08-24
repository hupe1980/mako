//! Collapses "is any message type compiled in?" into a single `cfg`.
//!
//! Seventeen message types are individually feature-gated, and a dozen items
//! across the crate exist only when *at least one* of them does. Spelling that
//! inline costs a copy of the seventeen-feature list per item, and a stale copy
//! still compiles — it just drops its item from the build.
//!
//! Emitting one `cfg` here reduces each to `#[cfg(any_message)]`.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    // Rust 1.80+ checks unexpected `cfg` names; declare ours so `--cfg` stays warning-free.
    println!("cargo::rustc-check-cfg=cfg(any_message)");

    const MESSAGE_FEATURES: &[&str] = &[
        "utilmd", "mscons", "aperak", "contrl", "invoic", "remadv", "orders", "iftsta", "insrpt",
        "reqote", "partin", "ordchg", "ordrsp", "quotes", "comdis", "pricat", "utilts",
    ];

    let any_enabled = MESSAGE_FEATURES.iter().any(|feature| {
        std::env::var_os(format!(
            "CARGO_FEATURE_{}",
            feature.to_uppercase().replace('-', "_")
        ))
        .is_some()
    });
    if any_enabled {
        println!("cargo::rustc-cfg=any_message");
    }
}
