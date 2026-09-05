//! Guards on `accountingd`'s authentication and authorization surface.
//!
//! Source-level, because all three defects they pin are silent at compile time
//! and invisible until a request arrives:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware — `Claims` is a `FromRequestParts` extractor, so a
//!    handler that does not name it is served to anyone.
//! 2. **`Claims` alone is authentication, not authorization.** Without a Cedar
//!    check any valid token from any tenant is accepted.
//! 3. **A Cedar action checked in code but named in no policy is a permanent
//!    403,** because Cedar is default-deny. The reverse is a dead grant, and
//!    usually means an endpoint lost its guard.
//! 4. **An exemption whose justification is only a comment.** The MCP surface
//!    used to carry no Cedar action at all, exempted in `accountingd.cedar` as
//!    "read-only by construction". Five of its thirteen tools write: they post
//!    a Buchung, raise the month's Abschlagsforderungen, book a CAMT.054
//!    payment, rewrite a customer's advance, or emit a bank-submittable
//!    pain.008. The last three tests below make the replacement claim —
//!    "every tool is held to the action its REST twin enforces" — a thing that
//!    is checked rather than asserted.

use std::collections::BTreeSet;

const POLICY: &str = include_str!("../policies/accountingd.cedar");
const HANDLERS: &str = include_str!("../src/handlers.rs");
/// The router, so the guards can tell a **routed** handler from a function that
/// merely lives in `handlers.rs`.
///
/// Without it, every `pub async fn` there was treated as reachable over HTTP —
/// which was true until the Jahresabschluss settlement was extracted so the
/// annual worker and the operator's POST could share one implementation. That
/// function takes a `&PgPool`, is reachable from no route, and being told it
/// "is served to any caller without a token" is the guard misreading its own
/// input. Cross-checking the router is also strictly stronger: it is the thing
/// that actually decides what is exposed.
const MAIN: &str = include_str!("../src/main.rs");
/// The MCP surface, whose tools are authorized by `mcp_server::tool_action`
/// rather than by a `cedar.check` in `handlers.rs`.
const MCP: &str = include_str!("../src/mcp_server.rs");

/// Every `"…"` literal passed as the action argument of `cedar.check(..)`.
fn actions_used_in_code() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = HANDLERS;
    while let Some(idx) = rest.find(".check(") {
        rest = &rest[idx + ".check(".len()..];
        let Some(open) = rest.find('"') else { break };
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        found.insert(after[..close].to_owned());
        rest = &after[close..];
    }
    found
}

/// Every `Action::"…"` named in the policy.
fn actions_permitted_in_policy() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = POLICY;
    while let Some(idx) = rest.find("Action::\"") {
        rest = &rest[idx + "Action::\"".len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_owned());
        rest = &rest[close..];
    }
    found
}

#[test]
fn the_policy_parses() {
    mako_service::cedar::CedarEnforcer::from_policy_str(POLICY)
        .expect("accountingd.cedar must parse — the service refuses to start otherwise");
}

#[test]
fn the_policy_permits_every_action_the_code_checks() {
    let used = actions_used_in_code();
    assert!(
        !used.is_empty(),
        "the extractor found no actions — it has drifted from the call shape"
    );
    let permitted = actions_permitted_in_policy();
    let missing: Vec<_> = used.difference(&permitted).cloned().collect();
    assert!(
        missing.is_empty(),
        "these Cedar actions are checked in code but appear in no policy, so Cedar's \
         default-deny makes those endpoints return 403 for every caller: {missing:?}"
    );
}

#[test]
fn the_policy_grants_no_action_the_code_never_checks() {
    let mut used = actions_used_in_code();
    // `use-mcp` is the blanket gate the shared MCP middleware applies
    // (`mako_service::mcp_auth::McpAuth::authenticate`), one crate over, and the
    // per-tool actions are checked by `mcp_server::mcp_auth_middleware` rather
    // than by a `cedar.check` in `handlers.rs`.
    used.insert("use-mcp".to_owned());
    used.extend(mcp_tools().iter().filter_map(|t| {
        accountingd::mcp_server::tool_action(t).map(std::borrow::ToOwned::to_owned)
    }));
    let permitted = actions_permitted_in_policy();
    let dead: Vec<_> = permitted.difference(&used).cloned().collect();
    assert!(
        dead.is_empty(),
        "these Cedar actions are granted by policy but checked nowhere — either the \
         endpoint lost its check (and is now unauthorized) or the grant is dead: {dead:?}"
    );
}

/// Handlers that legitimately carry no `Claims` extractor, each with its reason.
fn unauthenticated_by_design(handler: &str) -> Option<&'static str> {
    match handler {
        "ingest_webhook" => {
            Some("authenticated by the inbound Standard Webhooks signature, not by a bearer token")
        }
        "metrics" => Some("Prometheus scrape target; aggregates only, no per-customer data"),
        _ => None,
    }
}

/// Every **routed** `pub async fn` in `handlers.rs`, paired with its parameter
/// list.
///
/// Routed means `main.rs` passes it to a method filter as
/// `get(handlers::<name>)` / `post(handlers::<name>)` — the only definition of
/// "reachable over HTTP" that cannot drift, since it reads the router itself.
/// Matched with the closing parenthesis on purpose: a worker *calling* a
/// function (`handlers::settle_jahresabschluss(pool, …)`) is not routing it,
/// and a bare-name match cannot tell the two apart.
fn handler_signatures() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = HANDLERS;
    while let Some(idx) = rest.find("\npub async fn ") {
        rest = &rest[idx + "\npub async fn ".len()..];
        let Some(paren) = rest.find('(') else { break };
        let name = rest[..paren].trim().to_owned();
        let Some(end) = rest.find(") ->") else { break };
        let sig = rest[paren..end].to_owned();
        rest = &rest[end..];
        if MAIN.contains(&format!("(handlers::{name})")) {
            out.push((name, sig));
        }
    }
    out
}

#[test]
fn every_handler_authenticates() {
    let offenders: Vec<String> = handler_signatures()
        .into_iter()
        .filter(|(name, sig)| !sig.contains("Claims") && unauthenticated_by_design(name).is_none())
        .map(|(name, _)| name)
        .collect();
    assert!(
        offenders.is_empty(),
        "these handlers name no `Claims` extractor, so they are served to any caller \
         without a token: {offenders:?}. Add `claims: Claims` and a Cedar check, or \
         list the handler in `unauthenticated_by_design` with its reason."
    );
}

#[test]
fn every_authenticated_handler_authorizes() {
    // A handler that takes `Claims` but never reaches Cedar has decided *who* is
    // calling and then ignored the answer.
    let mut offenders = Vec::new();
    for (name, sig) in handler_signatures() {
        if !sig.contains("Claims") {
            continue;
        }
        let Some(start) = HANDLERS.find(&format!("\npub async fn {name}(")) else {
            continue;
        };
        // The body runs to the next top-level `pub async fn`, or to EOF.
        let tail = &HANDLERS[start + 1..];
        let end = tail.find("\npub async fn ").map_or(tail.len(), |i| i + 1);
        if !tail[..end].contains("cedar.check(") {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "these handlers authenticate but never authorize — any valid token from any \
         tenant is accepted: {offenders:?}"
    );
}

#[test]
fn the_handler_count_has_not_silently_shrunk() {
    // A guard that finds no handlers passes vacuously. accountingd had 55 when
    // this suite was written; the exact number matters less than the parser
    // still finding roughly that many — and, since it now cross-checks the
    // router, that the router is still being read at all.
    let n = handler_signatures().len();
    assert!(
        n >= 50,
        "the signature parser found only {n} handlers — it has drifted from the \
         source shape and the guards above are now vacuous"
    );
}

// ── The MCP surface ───────────────────────────────────────────────────────────

/// Every tool the MCP handler declares, read out of the `#[tool(…)]` attributes
/// rather than from a hand-kept list — a tool added without an entry in
/// `tool_action` must show up here, which is the whole point.
fn mcp_tools() -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = MCP;
    while let Some(idx) = rest.find("    #[tool(") {
        rest = &rest[idx..];
        // The tool's function follows its attribute; take the first
        // `async fn <name>(` after it.
        let Some(f) = rest.find("async fn ") else {
            break;
        };
        let after = &rest["    #[tool(".len()..];
        let name: String = rest[f + "async fn ".len()..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.push(name);
        }
        rest = after;
    }
    out.sort();
    out.dedup();
    out
}

/// The pg/ledger calls that change stored state, and the pain.008 builder.
///
/// A tool whose body names one of these is not a read, whatever its
/// `read_only_hint` annotation says — `run_sepa_collection` is annotated
/// `read_only_hint = false` and `trigger_jahresabschluss` `read_only_hint =
/// true`, and neither annotation is enforced by anything.
const MUTATING_CALLS: [&str; 5] = [
    "post_entry(",
    "update_account(",
    "upsert_account(",
    "raise_abschlagsforderung(",
    // Persists nothing, but produces the bank-submittable collection file and
    // reads every mandate's IBAN to do it.
    "build_pain_008(",
];

/// The actions that permit a change — everything the policy holds to a market
/// role and grants under "writes that move money", dunning, closing and erasure.
const WRITE_ACTIONS: [&str; 8] = [
    "write-account",
    "post-entry",
    "manage-sepa",
    "import-payments",
    "run-payout",
    "close-period",
    "manage-dunning",
    "erase-pii",
];

/// One MCP tool's body, from its `async fn` to the next `#[tool(` attribute.
fn mcp_tool_body(name: &str) -> &'static str {
    let start = MCP
        .find(&format!("async fn {name}("))
        .unwrap_or_else(|| panic!("{name} is defined in mcp_server.rs"));
    let rest = &MCP[start..];
    match rest.find("\n    #[tool(") {
        Some(end) => &rest[..end],
        None => rest,
    }
}

#[test]
fn the_tool_extractor_still_finds_the_surface() {
    let tools = mcp_tools();
    assert!(
        tools.len() >= 13,
        "only {} MCP tools found — the extractor has drifted from the `#[tool(…)]` \
         attribute shape, and the two guards below are now vacuous: {tools:?}",
        tools.len()
    );
}

/// Every tool is authorized. This is the replacement for the exemption that
/// said the surface was "read-only by construction".
#[test]
fn every_mcp_tool_carries_a_cedar_action() {
    let unmapped: Vec<String> = mcp_tools()
        .into_iter()
        .filter(|t| accountingd::mcp_server::tool_action(t).is_none())
        .collect();
    assert!(
        unmapped.is_empty(),
        "these MCP tools have no entry in `mcp_server::tool_action`, so nothing decides what \
         they may do: {unmapped:?}. The middleware refuses them at runtime, which is the \
         right failure — but the fix is to map each to the Cedar action its REST twin \
         enforces."
    );
}

/// The exemption's old justification, made checkable in the only direction that
/// matters: a tool that changes stored state must be held to an action that
/// permits a change, not to one of the three reads.
///
/// This is what would have caught the original defect. `post_manual_booking`,
/// `run_abschlag_cycle`, `import_payments`, `update_abschlag` and
/// `run_sepa_collection` all name a mutating call, so under the old blanket
/// exemption — action `None` — this test fails for all five.
#[test]
fn no_mutating_mcp_tool_is_held_to_a_read_action() {
    let mut offenders = Vec::new();
    for tool in mcp_tools() {
        let body = mcp_tool_body(&tool);
        let Some(call) = MUTATING_CALLS.iter().find(|c| body.contains(**c)) else {
            continue;
        };
        match accountingd::mcp_server::tool_action(&tool) {
            None => offenders.push(format!(
                "{tool}: calls `{call}` and carries no Cedar action at all"
            )),
            Some(action) if !WRITE_ACTIONS.contains(&action) => offenders.push(format!(
                "{tool}: calls `{call}` but is authorized as {action:?}, which the policy \
                 grants to a read"
            )),
            Some(_) => {}
        }
    }
    assert!(
        offenders.is_empty(),
        "MCP tools that change stored state without a write action:\n  {}",
        offenders.join("\n  ")
    );
}

/// The five that must never be reachable on a read grant, named one by one — so
/// a refactor that renames a tool or moves it onto `read-account` fails here
/// rather than silently widening the surface.
#[test]
fn the_money_moving_tools_are_named_and_held_to_write_actions() {
    for (tool, expected) in [
        ("post_manual_booking", "post-entry"),
        ("run_abschlag_cycle", "post-entry"),
        ("import_payments", "import-payments"),
        ("update_abschlag", "write-account"),
        ("run_sepa_collection", "manage-sepa"),
    ] {
        assert!(
            mcp_tools().contains(&tool.to_owned()),
            "{tool} is no longer a declared MCP tool — if it was renamed, rename it here too"
        );
        assert_eq!(
            accountingd::mcp_server::tool_action(tool),
            Some(expected),
            "{tool} is not authorized as {expected:?}"
        );
    }
}

/// A `tools/call` naming a tool with no mapping is refused rather than served.
#[test]
fn an_unmapped_tool_name_is_refused() {
    assert!(
        accountingd::mcp_server::tool_action("delete_the_ledger").is_none(),
        "`tool_action` answers for a tool that does not exist, so its match arms have a \
         catch-all that admits anything"
    );
}

/// The policy is only enforced on MCP if the middleware was given a verifier
/// and the policy to enforce. `from_auth_config` carries neither.
#[test]
fn the_mcp_surface_is_built_with_the_verifier_and_the_policy() {
    let compact: String = MAIN.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        compact.contains("McpAuth::from_auth_config_oidc(&cfg.mcp,oidc,Some(Arc::clone(&cedar)),"),
        "the MCP surface is not built with the OIDC verifier and the Cedar enforcer, so \
         neither `use-mcp` nor any per-tool action is ever checked and the five mutating \
         tools are open to whatever the API-key path admits"
    );
    let mcp_compact: String = MCP.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        mcp_compact.contains("state.auth.authorize(&parts.headers,action)"),
        "mcp_auth_middleware no longer authorizes the per-tool action before dispatching \
         the frame — `use-mcp` alone is then the whole check"
    );
}
