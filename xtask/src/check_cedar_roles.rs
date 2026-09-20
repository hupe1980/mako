//! Guard: a Cedar policy names a role the platform actually issues.
//!
//! Authorization here is `context.principal_roles.contains("…")` against the
//! `mako_roles` JWT claim, and Cedar's `contains` is an exact string match. A
//! role literal the platform never mints is therefore not a loose end — it is a
//! disjunct that is false for every caller forever, and it fails **silently**:
//! the policy parses, every test that fabricates the same spelling passes, and
//! the only observable is a `403` from an account that was provisioned to have
//! the access.
//!
//! The spelling that got here was `"admin"` against a claim minted as `"ADMIN"`,
//! in five clauses of one service. Nothing caught it: the policy is valid Cedar,
//! and the service's own policy test built its principal with the same lowercase
//! string — so the test exercised the dead disjunct against itself and went
//! green. That is the shape this guard exists for. A test that constructs the
//! principal cannot discover that the principal is unissuable; only a comparison
//! against the minting side can.
//!
//! ## The set
//!
//! [`KNOWN_ROLES`] is the uppercase BDEW role codes `Marktrolle::parse` accepts
//! in `services/makod/src/main.rs` — which states the convention itself
//! („Accepts uppercase BDEW role codes") — plus `ADMIN`, the operator role
//! `OidcVerifier::disabled_claims` mints alongside the market roles and which is
//! not a Marktrolle at all.
//!
//! Sub-qualifiers (`GNB`, `LFG`, `GMSB`, `ANB`, `VNB`) are in the set because
//! `parse` accepts them, not because they are Rollenmodell V2.2 roles — the root
//! `AGENTS.md` is explicit that they are EDIFACT-AHB sub-qualifiers. A policy
//! **should** prefer the role they normalise to, but a policy that names one is
//! wrong about modelling rather than dead, and this guard reports only the dead.
//!
//! ## What it does not check
//!
//! Whether the role is the *right* one for the action. That is a judgement about
//! who may do what, and it lives in each service's own policy test, where the
//! reasoning is next to the endpoint. This guard answers the one question those
//! tests structurally cannot: whether the string can ever match.

use std::path::Path;

/// Role strings that reach `mako_roles` and can therefore match.
///
/// Kept as a literal rather than parsed out of `main.rs`: the parse arm is a
/// `match` over aliases (`"UENB" | "ÜNB" | "UNB" | "FNB"`), so deriving the set
/// from it means reimplementing Rust pattern syntax in a guard. The floor below
/// is what keeps the literal honest — if this list and the platform disagree,
/// the disagreement surfaces as a refused policy, which is the loud direction.
const KNOWN_ROLES: &[&str] = &[
    // Marktrollen — `Marktrolle::parse`, uppercase forms.
    "NB", "LF", "MSB", "NMSB", "AMSB", "BKV", "UENB", "ÜNB", "UNB", "FNB", "BIKO", "ESA", "MGV",
    // Sub-qualifiers `parse` normalises onto a Marktrolle.
    "GNB", "ANB", "VNB", "LFG", "GMSB",
    // Platform operator role — not a Marktrolle; minted by `disabled_claims`
    // and expected from the IdP for operator-level capabilities.
    "ADMIN",
];

/// The lowest number of policy files a healthy scan may find.
///
/// A scanner that silently stops matching certifies nothing, and this one walks
/// a glob-shaped layout (`services/*/policies/*.cedar`) that a reorganisation
/// would quietly empty.
const MIN_POLICIES: usize = 5;

/// Every `principal_roles.contains("…")` literal in `src`, with its line number.
#[must_use]
pub fn role_literals(src: &str) -> Vec<(String, usize)> {
    const CALL: &str = "principal_roles.contains(\"";
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let mut rest = line;
        while let Some(idx) = rest.find(CALL) {
            rest = &rest[idx + CALL.len()..];
            let Some(close) = rest.find('"') else { break };
            out.push((rest[..close].to_owned(), i + 1));
            rest = &rest[close..];
        }
    }
    out
}

/// Scan every service policy for a role the platform cannot issue.
///
/// Returns `true` when every role literal is one `mako_roles` can carry.
#[must_use]
pub fn run(workspace_root: &Path) -> bool {
    let services = workspace_root.join("services");
    let Ok(entries) = std::fs::read_dir(&services) else {
        println!("check-cedar-roles: FAIL — services/ is not readable");
        return false;
    };

    let mut dirs: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();

    let mut scanned = 0usize;
    let mut findings: Vec<String> = Vec::new();

    for dir in dirs {
        let policies = dir.join("policies");
        let Ok(files) = std::fs::read_dir(&policies) else {
            continue;
        };
        let mut paths: Vec<_> = files.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|s| s.to_str()) != Some("cedar") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            scanned += 1;
            for (role, line) in role_literals(&text) {
                if KNOWN_ROLES.contains(&role.as_str()) {
                    continue;
                }
                let rel = path.strip_prefix(workspace_root).unwrap_or(&path);
                let hint = KNOWN_ROLES
                    .iter()
                    .find(|k| k.eq_ignore_ascii_case(&role))
                    .map_or_else(
                        || "no role of that name is issued".to_owned(),
                        |k| format!("the issued spelling is `{k}`"),
                    );
                findings.push(format!("{}:{line}: `{role}` — {hint}", rel.display()));
            }
        }
    }

    if scanned < MIN_POLICIES {
        println!(
            "check-cedar-roles: FAIL — found {scanned} policy file(s), at least {MIN_POLICIES} \
             are expected.\n\
             The layout moved and this guard has to be taught it; it has not stopped being needed."
        );
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-cedar-roles: every role named in {scanned} policy file(s) is one the platform \
             issues"
        );
        return true;
    }

    println!(
        "check-cedar-roles: FAIL — {} role literal(s) no token can carry:\n",
        findings.len()
    );
    for f in &findings {
        println!("  {f}");
    }
    println!(
        "\nCedar's `contains` is an exact match against the `mako_roles` claim, so a literal the \
         platform never mints is a disjunct that is false for every caller. The account \
         provisioned for that access gets 403 and the policy still parses.\n\
         Use the spelling `Marktrolle::parse` accepts (uppercase BDEW role codes), or `ADMIN` for \
         the operator role."
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literals are read out of the clause shape the policies use.
    #[test]
    fn it_reads_every_literal_on_a_line() {
        let src = "    (context.principal_roles.contains(\"MSB\") ||\n     context.principal_roles.contains(\"ADMIN\"))\n";
        let found = role_literals(src);
        assert_eq!(
            found,
            vec![("MSB".to_owned(), 1), ("ADMIN".to_owned(), 2)],
            "both disjuncts and their lines"
        );
    }

    /// The defect this guard exists for: a case-mismatched operator role.
    ///
    /// A guard that cannot fail certifies nothing, so this is the input that
    /// makes it fail — and it asserts the message names the issued spelling,
    /// because „unknown role" without it sends the reader back to the same
    /// guess that produced the defect.
    #[test]
    fn a_case_mismatched_role_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-cedar-roles-pos");
        let _ = std::fs::remove_dir_all(&dir);
        for s in ["a", "b", "c", "d", "e"] {
            let p = dir.join("services").join(s).join("policies");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(
                p.join("x.cedar"),
                "when { context.principal_roles.contains(\"NB\") };\n",
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("services/a/policies/x.cedar"),
            "when { context.principal_roles.contains(\"admin\") };\n",
        )
        .unwrap();
        assert!(!run(&dir), "a role no token carries must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every spelling the platform issues passes, including the Gas aliases.
    #[test]
    fn the_issued_spellings_pass() {
        let dir = std::env::temp_dir().join("mako-check-cedar-roles-neg");
        let _ = std::fs::remove_dir_all(&dir);
        for (s, role) in [
            ("a", "NB"),
            ("b", "UENB"),
            ("c", "ESA"),
            ("d", "ADMIN"),
            ("e", "FNB"),
        ] {
            let p = dir.join("services").join(s).join("policies");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(
                p.join("x.cedar"),
                format!("when {{ context.principal_roles.contains(\"{role}\") }};\n"),
            )
            .unwrap();
        }
        assert!(run(&dir), "the issued spellings must pass");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The floor binds: an emptied layout is a moved layout, not a clean tree.
    #[test]
    fn an_empty_tree_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-cedar-roles-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("services")).unwrap();
        assert!(!run(&dir), "a tree with no policy must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
