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

/// One `#[tool(...)]` as the source declares it.
struct DeclaredTool {
    /// The Rust function the attribute decorates.
    fn_name: String,
    /// The name a `tools/call` frame carries: the attribute's `name = "…"`
    /// where it gives one, else the function name. The auth middleware matches
    /// on this, so a guard that only knows `fn_name` can miss a renamed tool.
    wire_name: String,
    /// Whether it declares `read_only_hint = true`.
    read_only: bool,
}

/// Every `#[tool(...)]` in the MCP server.
fn declared_tools() -> Vec<DeclaredTool> {
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
        let fn_name = tail
            .find("fn ")
            .map(|f| {
                tail[f + 3..]
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect::<String>()
            })
            .unwrap_or_default();
        if !fn_name.is_empty() {
            let wire_name = attrs
                .find("name = \"")
                .map(|n| {
                    attrs[n + "name = \"".len()..]
                        .chars()
                        .take_while(|c| *c != '"')
                        .collect::<String>()
                })
                .unwrap_or_else(|| fn_name.clone());
            out.push(DeclaredTool {
                fn_name,
                wire_name,
                read_only: attrs.contains("read_only_hint = true"),
            });
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
        .filter(|t| !t.read_only)
        .map(|t| t.fn_name.as_str())
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
    let reads = tools.iter().filter(|t| t.read_only).count();
    assert_eq!(
        reads + MUTATING.len(),
        tools.len(),
        "every tool is either read-only by declaration or named in MUTATING; \
         `xtask check-tool-grants` reads the same hint to decide what a \
         specialist may hold"
    );
}

/// Every mutating tool is gated by a Cedar action, not by the blanket
/// `use-mcp` grant.
///
/// `use-mcp` sits in the **read** permit of `einsd.cedar`, whose only condition
/// is `principal_tenant == resource_tenant`. A principal with no market role —
/// an `[mcp] api_key` whose `api_key_roles` is unset is one — clears it. So a
/// mutating tool that reaches the middleware without its own action is served
/// to a caller the REST twin refuses with a 403.
///
/// The middleware fails closed on a mutating tool it cannot map, which is what
/// keeps a fourth from shipping ungated. This asserts the mapping is complete
/// so that closure never has to fire in production.
#[test]
fn every_mutating_tool_maps_to_a_cedar_action() {
    for tool in declared_tools().iter().filter(|t| !t.read_only) {
        let action = einsd::mcp_server::mutating_tool_action(&tool.wire_name);
        assert!(
            action.is_some(),
            "MCP tool {:?} mutates but `mcp_server::mutating_tool_action` maps it to no \
             Cedar action — over /mcp it would be gated by `use-mcp` alone, which any \
             role-less principal of the tenant holds. Map it to the action its REST twin \
             enforces.",
            tool.wire_name,
        );
    }
}

/// The middleware's fail-closed list is the declared mutating surface.
///
/// `mutating_tool_action` returning `None` means "read tool, `use-mcp` is the
/// whole check" *unless* the name is in `MUTATING_TOOLS`. A mutating tool
/// missing from both lists is therefore served, not refused — the one shape the
/// fail-closed branch exists to prevent.
#[test]
fn the_middlewares_mutating_list_is_the_declared_surface() {
    let declared: BTreeSet<String> = declared_tools()
        .iter()
        .filter(|t| !t.read_only)
        .map(|t| t.wire_name.clone())
        .collect();
    let guarded: BTreeSet<String> = einsd::mcp_server::MUTATING_TOOLS
        .iter()
        .map(|t| (*t).to_owned())
        .collect();
    assert_eq!(
        declared, guarded,
        "`mcp_server::MUTATING_TOOLS` must list exactly the tools without \
         `read_only_hint` — a mutating tool absent from it falls through the \
         middleware's read branch and is gated by `use-mcp` alone",
    );
}

/// Each mapped action is one the policy actually writes.
///
/// A typo maps the tool to an action no `permit` names; Cedar is default-deny,
/// so that fails closed rather than open — but it fails for every caller, which
/// reads as "MCP is broken" rather than "the action is misspelt".
#[test]
fn every_mapped_action_exists_in_the_policy() {
    let policy = include_str!("../policies/einsd.cedar");
    for tool in declared_tools().iter().filter(|t| !t.read_only) {
        let action =
            einsd::mcp_server::mutating_tool_action(&tool.wire_name).expect("mapping is complete");
        assert!(
            policy.contains(&format!("Action::\"{action}\"")),
            "{} maps to Cedar action {action:?}, which einsd.cedar does not name — \
             default-deny would refuse every caller",
            tool.wire_name,
        );
    }
}
