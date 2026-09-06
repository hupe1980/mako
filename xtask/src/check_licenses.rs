//! Guard: the licence governance page states the licences `deny.toml` allows.
//!
//! `site/content/docs/compliance/licenses.md` is what a procurement or
//! compliance reader consults to learn what this workspace may depend on, and
//! the page itself instructs a maintainer to "commit both files together so
//! `deny.toml` and this document are always in sync" — a promise nothing
//! enforced. It had already failed: the page carried a retired `MPL-2.0`
//! section stating the licence was allowed with governance review, while
//! `deny.toml` had dropped it, so the two documents gave opposite answers to
//! the only question the page exists to answer.
//!
//! Both directions matter. A licence in `deny.toml` and not on the page is a
//! dependency class nobody reviewed; a licence on the page and not in
//! `deny.toml` reads as permission that `cargo deny` will refuse.
//!
//! The page names each identifier in a table cell or a heading — either is a
//! statement that it is allowed — so both are read.

use std::path::Path;

/// Where the governance page lives.
const DOC: &str = "site/content/docs/compliance/licenses.md";

pub fn run(workspace_root: &Path) -> bool {
    let Ok(doc) = std::fs::read_to_string(workspace_root.join(DOC)) else {
        eprintln!("check-licenses: cannot read {DOC}");
        return false;
    };
    let Ok(deny) = std::fs::read_to_string(workspace_root.join("deny.toml")) else {
        eprintln!("check-licenses: cannot read deny.toml");
        return false;
    };

    let allowed = allowed_licences(&deny);
    if allowed.is_empty() {
        eprintln!("check-licenses: deny.toml declares no [licenses] allow list — has it moved?");
        return false;
    }
    let documented = documented_licences(&doc);

    let undocumented: Vec<_> = allowed
        .iter()
        .filter(|id| !documented.contains(*id))
        .cloned()
        .collect();
    let unallowed: Vec<_> = documented
        .iter()
        .filter(|id| !allowed.contains(*id))
        .cloned()
        .collect();

    if undocumented.is_empty() && unallowed.is_empty() {
        println!(
            "check-licenses: {} allowed licence(s) in deny.toml, every one documented in {DOC}",
            allowed.len()
        );
        return true;
    }

    eprintln!("check-licenses: {DOC} and deny.toml disagree:");
    for id in &undocumented {
        eprintln!(
            "  {id}: allowed by deny.toml, absent from the page — a dependency class \
             nobody recorded a review for"
        );
    }
    for id in &unallowed {
        eprintln!(
            "  {id}: stated as allowed on the page, absent from deny.toml's allow list — \
             `cargo deny` refuses it, so the page reads as permission that does not exist"
        );
    }
    false
}

/// The SPDX identifiers in `deny.toml`'s `[licenses] allow = [...]`.
///
/// Comment lines are skipped: the list carries a rationale comment above most
/// entries, and one of them names another identifier in prose.
pub fn allowed_licences(deny: &str) -> Vec<String> {
    let Some(rest) = deny.split_once("[licenses]").map(|(_, r)| r) else {
        return Vec::new();
    };
    let Some(rest) = rest.split_once("allow").map(|(_, r)| r) else {
        return Vec::new();
    };
    let Some(list) = rest.split_once('[').and_then(|(_, r)| r.split_once(']')) else {
        return Vec::new();
    };
    list.0
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('"').and_then(|(_, r)| r.split_once('"')))
        .map(|(id, _)| id.to_owned())
        .collect()
}

/// The SPDX identifiers the page states as allowed.
///
/// A backticked identifier in a table row or a `###` heading. Prose mentions are
/// deliberately not read: the page discusses licences it refuses by name, and
/// counting those would make every refusal look like a permission.
pub fn documented_licences(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in doc.lines() {
        let line = line.trim();
        let is_row = line.starts_with('|');
        let is_heading = line.starts_with("###");
        if !is_row && !is_heading {
            continue;
        }
        let field = if is_row {
            line.trim_start_matches('|')
                .split('|')
                .next()
                .unwrap_or_default()
        } else {
            line
        };
        if let Some((_, r)) = field.split_once('`')
            && let Some((id, _)) = r.split_once('`')
            && !id.is_empty()
            && !out.contains(&id.to_owned())
        {
            out.push(id.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Rationale comments sit inside the array and one of them names a licence.
    #[test]
    fn comments_in_the_allow_list_are_not_licences() {
        let deny = "[licenses]\nallow = [\n    \"MIT\",\n    # 0BSD: see the governance page\n    \"Apache-2.0\",\n]\nconfidence-threshold = 0.9\n";
        assert_eq!(
            super::allowed_licences(deny),
            vec!["MIT".to_owned(), "Apache-2.0".to_owned()]
        );
    }

    /// A table row and a governance heading both state a permission; a licence
    /// named only in the refusal prose does not.
    #[test]
    fn rows_and_headings_are_read_and_prose_is_not() {
        let doc = "| SPDX Identifier | Notes |\n\
                   |---|---|\n\
                   | `MIT` | Permissive. |\n\
                   ### `0BSD` — Zero-Clause BSD\n\
                   Licences that are never acceptable: `AGPL-3.0-only`.\n";
        assert_eq!(
            super::documented_licences(doc),
            vec!["MIT".to_owned(), "0BSD".to_owned()]
        );
    }
}
