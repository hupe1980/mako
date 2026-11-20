//! Embeds the contract every service publishes — its committed OpenAPI
//! document, its MCP tool snapshot and its event schemas — so the kit checks a
//! replacement against exactly what the reference service commits, and a
//! document change and the kit move in one commit.

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let services = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../services");
    let services = services.canonicalize().expect("services/ exists");
    println!("cargo:rerun-if-changed={}", services.display());

    let mut openapi = String::new();
    let mut mcp = String::new();
    let mut schemas = String::new();
    let mut dirs: Vec<_> = std::fs::read_dir(&services)
        .expect("services/ is readable")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let doc = dir.join("openapi.json");
        if doc.is_file() {
            println!("cargo:rerun-if-changed={}", doc.display());
            writeln!(
                openapi,
                "    ({name:?}, include_str!({:?})),",
                doc.display().to_string()
            )
            .unwrap();
        }
        let snap = dir.join("mcp.snapshot.json");
        if snap.is_file() {
            println!("cargo:rerun-if-changed={}", snap.display());
            writeln!(
                mcp,
                "    ({name:?}, include_str!({:?})),",
                snap.display().to_string()
            )
            .unwrap();
        }
        let events = dir.join("events");
        if events.is_dir() {
            println!("cargo:rerun-if-changed={}", events.display());
            let mut files: Vec<_> = std::fs::read_dir(&events)
                .expect("events/ is readable")
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            files.sort();
            for f in files {
                let ty = f.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
                writeln!(
                    schemas,
                    "    ({ty:?}, include_str!({:?})),",
                    f.display().to_string()
                )
                .unwrap();
            }
        }
    }
    let out = format!(
        "/// Every service's committed OpenAPI document.\n\
         pub static OPENAPI: &[(&str, &str)] = &[\n{openapi}];\n\
         /// Every service's committed MCP tool snapshot.\n\
         pub static MCP_SNAPSHOTS: &[(&str, &str)] = &[\n{mcp}];\n\
         /// Every published event schema, by type.\n\
         pub static EVENT_SCHEMAS: &[(&str, &str)] = &[\n{schemas}];\n"
    );
    let dest = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("documents.rs");
    std::fs::write(dest, out).expect("OUT_DIR is writable");
}
