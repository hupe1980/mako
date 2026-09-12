//! Which einsd MCP tools can move money, pinned.
//!
//! `einsd` computes a statutory payment, so „a model may explain a settlement
//! and never produce one" is an invariant about this service and not only about
//! the plane in front of it. Two mechanisms hold it today, and both live
//! elsewhere: `agentd` grants none of these three tools to any specialist, and
//! `xtask check-tool-grants` refuses a mutating grant on a `tool-calling` agent
//! at all.
//!
//! That leaves the surface itself unpinned. A fourth mutating tool would widen
//! what the grant policy has to keep refusing, and would do it in a diff that
//! reads like any other tool addition — `check-tool-grants` would still pass,
//! because it checks the grants against the hints rather than the size of the
//! mutating set.
//!
//! So the set is named here with the reason each entry writes. Adding a
//! mutating tool is allowed; adding one *silently* is not.

use std::collections::BTreeSet;

/// The tools that write. Each is an operator action with a consequence a model
/// must not reach on its own.
const MUTATING: &[(&str, &str)] = &[
    (
        "trigger_settle",
        "produces a settlement — the amount the Netzbetreiber owes for a period",
    ),
    (
        "import_marktwert",
        "writes the Monats-/Jahresmarktwert series; a substituted value misprices \
         every kWh of the period (`PriceMissing` is a refusal, not a zero)",
    ),
    (
        "import_epex_monthly_price",
        "writes the EPEX monthly aggregate the § 51 and Post-EEG paths price from",
    ),
];

/// Every `#[tool(...)]` in the MCP server, with whether it declares
/// `read_only_hint = true`.
fn declared_tools() -> Vec<(String, bool)> {
    let src = include_str!("../src/mcp_server.rs");
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("#[tool(") {
        let after = &rest[i + "#[tool(".len()..];
        // The attribute body ends at the `)]` that closes it.
        let Some(end) = after.find(")]") else { break };
        let attrs = &after[..end];
        let tail = &after[end..];
        // The next `fn <name>` is the tool this attribute decorates.
        let name = tail
            .find("fn ")
            .map(|f| {
                tail[f + 3..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .unwrap_or_default();
        if !name.is_empty() {
            out.push((name, attrs.contains("read_only_hint = true")));
        }
        rest = tail;
    }
    out
}

#[test]
fn the_mutating_tool_surface_is_the_declared_one() {
    let tools = declared_tools();
    assert!(
        tools.len() >= 15,
        "parsed only {} tools — the attribute shape changed and this guard is \
         reading nothing",
        tools.len()
    );

    let found: BTreeSet<&str> = tools
        .iter()
        .filter(|(_, read_only)| !read_only)
        .map(|(name, _)| name.as_str())
        .collect();
    let declared: BTreeSet<&str> = MUTATING.iter().map(|(name, _)| *name).collect();

    let added: Vec<&&str> = found.difference(&declared).collect();
    assert!(
        added.is_empty(),
        "einsd gained mutating MCP tool(s) {added:?} — a wider surface the grant \
         policy has to keep refusing. Add each to MUTATING with the reason it \
         writes, so widening it is a decision and not a diff.",
    );

    let removed: Vec<&&str> = declared.difference(&found).collect();
    assert!(
        removed.is_empty(),
        "MUTATING names {removed:?}, which no longer mutate (or no longer exist). \
         Drop them — a list that over-states the surface trains readers to ignore it.",
    );
}

/// The read surface is the larger half, and it is what a specialist is granted.
/// A tool that quietly stops declaring `read_only_hint` moves from that half to
/// the other one without changing its name.
#[test]
fn every_read_tool_declares_the_hint_that_makes_it_grantable() {
    let tools = declared_tools();
    let reads = tools.iter().filter(|(_, ro)| *ro).count();
    assert_eq!(
        reads + MUTATING.len(),
        tools.len(),
        "every tool is either read-only by declaration or named in MUTATING; \
         `xtask check-tool-grants` reads the same hint to decide what a \
         specialist may hold"
    );
}
