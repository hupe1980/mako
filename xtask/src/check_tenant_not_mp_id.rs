//! Guard: the tenant never stands in for a Marktpartner-ID, and no Marktrolle's
//! code stands in for another's.
//!
//! `tenant` is a data-isolation key — any stable string — and each Marktrolle
//! has its own Codenummer: „Marktteilnehmer benötigen für jede Marktrolle
//! Marktfunktion im Strom- bzw. Gasmarkt eine gesonderte Codenummer"
//! (Allgemeine Festlegungen 6.1d Kap. 2.13). A fallback from an MP-ID, a sender
//! or a seller name to the tenant reads correctly only while the two happen to
//! be equal, and then prints a slug where a Codenummer or a company name
//! belongs. A fallback to makod's primary `[[party]]` sends a message under
//! another Marktrolle's code.
//!
//! Two shapes are refused, per statement, outside test code:
//!
//! 1. a fallback (`unwrap_or`, `unwrap_or_else`, `map_or_else`, `or`,
//!    `or_else`) whose argument names `tenant`, in a statement about an MP-ID,
//!    a GLN, a sender, a receiver, a seller or a Lieferant;
//! 2. `primary_mp_id()` as a fallback or as the source of a `TenantId`.

use std::path::{Path, PathBuf};

/// Statements that match a shape and are not an identity on the wire, each
/// with the reason.
const EXEMPT: &[(&str, &str, &str)] = &[(
    "services/makod/src/core/cedar_authz.rs",
    "res.mp_id.unwrap_or(res.tenant)",
    "a Cedar entity id: the partner collection without an MP-ID is named after \
     the tenant it belongs to, and the name never leaves the authorizer",
)];

/// The lowest number of files a healthy scan reads. Measured; a scan below it
/// has lost the layout, not found a cleaner tree.
const MIN_FILES: usize = 900;

const FALLBACKS: [&str; 5] = [
    ".unwrap_or(",
    ".unwrap_or_else(",
    ".map_or_else(",
    ".or(",
    ".or_else(",
];

const IDENTITIES: [&str; 7] = [
    "mp_id",
    "gln",
    "seller",
    "sender",
    "receiver",
    "lieferant",
    "netzbetreiber",
];

/// Whether `stmt` names the tenant itself — `tenant`, not `tenant_id`.
fn names_tenant(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(i) = text[from..].find("tenant") {
        let at = from + i;
        let end = at + "tenant".len();
        let before = at.checked_sub(1).map(|j| bytes[j]);
        let after = bytes.get(end).copied();
        let word = |b: Option<u8>| b.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !word(before) && !word(after) {
            return true;
        }
        from = end;
    }
    false
}

/// The text of the call whose argument list opens at `open` (just past the
/// `(`), up to its matching `)`.
fn argument(text: &str, open: usize) -> &str {
    let mut depth = 1usize;
    for (k, ch) in text[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open..open + k];
                }
            }
            _ => {}
        }
    }
    &text[open..]
}

/// The reason `stmt` is refused, if it is.
fn finding(stmt: &str) -> Option<&'static str> {
    let lower = stmt.to_ascii_lowercase();
    for f in FALLBACKS {
        let mut from = 0;
        while let Some(i) = stmt[from..].find(f) {
            let open = from + i + f.len();
            let arg = argument(stmt, open);
            if names_tenant(arg) && IDENTITIES.iter().any(|id| lower[..open].contains(id)) {
                return Some("falls back to the tenant where an identity belongs");
            }
            if arg.contains("primary_mp_id()") {
                return Some("falls back to the primary [[party]] MP-ID");
            }
            from = open;
        }
    }
    let mut from = 0;
    while let Some(i) = stmt[from..].find("from_tenant(") {
        let open = from + i + "from_tenant(".len();
        if argument(stmt, open).contains("primary_mp_id()") {
            return Some("derives the tenant from the primary [[party]] MP-ID");
        }
        from = open;
    }
    None
}

/// `src` without its comments and without what follows a top-level
/// `#[cfg(test)]`.
fn production_code(src: &str) -> String {
    let mut out = String::new();
    for line in src.lines() {
        if line.starts_with("#[cfg(test)]") {
            break;
        }
        let code = line.split("//").next().unwrap_or_default();
        out.push_str(code);
        out.push('\n');
    }
    out
}

fn rust_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name == "tests" || name == "target" || name == "node_modules" {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            out.push(p);
        }
    }
}

/// Run the guard over the workspace.
pub fn run(workspace_root: &Path) -> bool {
    scan(workspace_root, MIN_FILES)
}

fn scan(workspace_root: &Path, floor: usize) -> bool {
    let mut files = Vec::new();
    for top in ["services", "crates", "tools"] {
        rust_files(&workspace_root.join(top), &mut files);
    }
    files.sort();
    let mut findings = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let code = production_code(&src);
        let mut line = 1usize;
        for stmt in code.split([';', '{', '}']) {
            let flat: String = stmt.split_whitespace().collect::<Vec<_>>().join(" ");
            if let Some(why) = finding(&flat) {
                let rel = path.strip_prefix(workspace_root).unwrap_or(path);
                if EXEMPT
                    .iter()
                    .any(|(file, text, _)| rel == Path::new(file) && flat.contains(text))
                {
                    line += stmt.matches('\n').count();
                    continue;
                }
                findings.push(format!("{}:{line}: {why}: {flat}", rel.display()));
            }
            line += stmt.matches('\n').count();
        }
    }
    if files.len() < floor {
        println!(
            "check-tenant-not-mp-id: FAIL — read {} files, fewer than the floor of {floor}",
            files.len()
        );
        return false;
    }
    if findings.is_empty() {
        println!(
            "check-tenant-not-mp-id: ok — {} files, no tenant or primary MP-ID stands in for an identity",
            files.len()
        );
        true
    } else {
        println!("check-tenant-not-mp-id: FAIL");
        for f in &findings {
            println!("  {f}");
        }
        println!(
            "  The tenant is a data-isolation key, and each Marktrolle has its own MP-ID \
             (Allgemeine Festlegungen 6.1d Kap. 2.13): name the identity explicitly, or refuse."
        );
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tenant_fallback_for_an_mp_id_is_refused() {
        assert!(finding("let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant)").is_some());
        assert!(finding("cfg.seller_name.clone().unwrap_or_else(|| cfg.tenant.clone())").is_some());
        assert!(
            finding("let nb_mp_id = data.get(\"nb_mp_id\").map_or_else(|| tenant.to_owned(), str::to_owned)")
                .is_some()
        );
    }

    #[test]
    fn a_primary_fallback_is_refused() {
        assert!(
            finding("let sender = text(&p.sender).unwrap_or_else(|| registry.primary_mp_id())")
                .is_some()
        );
        assert!(finding("let t = TenantId::from_tenant(registry.primary_mp_id())").is_some());
    }

    #[test]
    fn the_tenant_as_a_tenant_passes() {
        assert!(finding("let tenant = cfg.tenant.clone()").is_none());
        assert!(finding("let id = TenantId::from_tenant(&tenant)").is_none());
        assert!(finding("let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.lf_mp_id)").is_none());
        assert!(finding("let x = req.tenant_id.unwrap_or(state.tenant_id)").is_none());
    }

    #[test]
    fn test_code_is_not_scanned() {
        let src = "fn a() {}\n#[cfg(test)]\nmod tests { fn b() { x.mp_id.unwrap_or(tenant); } }\n";
        assert!(!production_code(src).contains("unwrap_or"));
    }
}
