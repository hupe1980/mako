//! Guards on the authentication and authorization surface.
//!
//! `productd` decides what the supplier charges. Every price `billingd` puts on
//! an invoice is read from a row here and nothing downstream re-checks it, so
//! `PUT /api/v1/products/{lf_mp_id}/{product_code}` rewrites the Arbeitspreis
//! of a live tariff and the next run bills the new number; `PUT
//! /api/v1/epex-prices/{date}` is the same act for every § 41a dynamic tariff
//! at once.
//!
//! This service used to authenticate and not authorize: every route extracted
//! `Claims` and, at most, compared the path's `lf_mp_id` with the token's
//! tenant. That answers "is this my tenant's catalogue?", not "may this caller
//! change its prices" — so any token the verifier accepted for the tenant could
//! rewrite the whole price sheet.
//!
//! Nothing in the type system notices any of that: `Claims` is a
//! `FromRequestParts` extractor a signature can simply omit or bind to
//! `_claims`, and a `CedarEnforcer` can be injected into a router no handler
//! ever consults. So it is pinned here, in the three failure classes this repo
//! has met before:
//!
//! 1. **A handler with no `Claims` extractor is unauthenticated.** There is no
//!    global auth middleware, so a handler that does not name it is served to
//!    anyone who can open the port.
//! 2. **A handler that takes `Claims` and never authorizes** is reachable by
//!    every principal holding any token for this tenant — including the
//!    `_claims` extract-and-discard shape, which is what every Angebot and
//!    EPEX handler in this file looked like before.
//! 3. **Action-set mismatch, in both directions.** A Cedar action checked in
//!    code but named in no policy is a permanent 403, because Cedar is
//!    default-deny; a policy action nothing checks is a dead grant, which means
//!    either an endpoint lost its check or the grant describes a route that is
//!    not mounted.
//!
//! The remaining tests are the substance rather than the shape: the two § 41c
//! routes are exempt from authentication *and* bounded by their query, a
//! role-less token reads the catalogue and moves no price, and no token of
//! another tenant reaches anything.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use mako_service::cedar::{CedarEnforcer, CedarPrincipal};

const POLICY: &str = include_str!("../policies/productd.cedar");

const TENANT: &str = "9900357000004";

/// The module the router mounts REST handlers from.
const HANDLER_MODULE: &str = "handlers";

/// The blanket gate the shared MCP middleware applies
/// (`mako_service::mcp_auth::McpAuth::authenticate`), one crate over.
const MCP_GATE: &str = "use-mcp";

/// The two § 41c EnWG comparison-feed routes.
///
/// They are the only routed handlers that take no `Claims`, and that is
/// deliberate: § 41c obliges the supplier to make its tariff data available to
/// independent comparison instruments (Verivox, Check24, the BNetzA
/// Markttransparenzstelle), and an obligation that is only discharged behind a
/// bearer token is not discharged — requiring a credential would put the
/// operator in breach.
///
/// The exemption is bounded by the query rather than by the caller, which
/// [`the_public_feeds_serve_only_published_rows`] pins: unlike the HMAC-signed
/// webhooks the sibling services exempt, there is no second credential here to
/// check, so the only checkable claim is *what* leaves the house.
const PUBLIC_BY_STATUTE: [&str; 2] = ["get_comparison_feed", "get_comparison_feed_bo4e"];

fn src(file: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read src/{file}: {e}"))
}

/// `text` with every space, tab and newline removed.
///
/// The checks below look for call shapes, and rustfmt wraps a call across lines
/// as soon as it is long enough. Matching the wrapped form would make the guard
/// pass or fail on the length of an action name.
fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// One item's own source, from `start` to the next top-level item.
///
/// A fixed-length window would run past the end of a short handler into the
/// next one, and a missing `authorize` there would be masked by its neighbour's.
fn item_source(text: &str, start: usize) -> &str {
    let rest = &text[start..];
    // Past the `pub async fn …(` this starts on, to the next item's `pub`.
    let after_header = rest.find(") ->").map_or(0, |i| i + 4);
    match rest[after_header..].find("\npub ") {
        Some(end) => &rest[..after_header + end],
        None => rest,
    }
}

/// The identifier that follows `needle` in `text`, for every occurrence.
fn identifiers_after(text: &str, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(idx) = rest.find(needle) {
        rest = &rest[idx + needle.len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            found.push(name);
        }
    }
    found
}

/// Every handler the domain router mounts, read out of the routing calls in
/// `main.rs` rather than out of every `handlers::…` mention — so the background
/// Angebot-expiry worker `main` also names is not mistaken for a route.
fn routed_handlers() -> Vec<String> {
    let main = src("main.rs");
    let mut routed = Vec::new();
    for verb in ["get(", "post(", "put(", "patch(", "delete("] {
        routed.extend(identifiers_after(
            &main,
            &format!("{verb}{HANDLER_MODULE}::"),
        ));
    }
    routed.sort();
    routed.dedup();
    routed
}

/// Every `"…"` literal passed as the action argument of an `authorize` call.
fn actions_used_in_code() -> BTreeSet<String> {
    let text = compact(&src(&format!("{HANDLER_MODULE}.rs")));
    let mut found = BTreeSet::new();
    let needle = "authorize(&cedar,&claims,\"";
    let mut rest = text.as_str();
    while let Some(idx) = rest.find(needle) {
        rest = &rest[idx + needle.len()..];
        let Some(close) = rest.find('"') else { break };
        found.insert(rest[..close].to_owned());
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

// ── The three failure classes ─────────────────────────────────────────────────

/// Every routed handler authenticates and authorizes, except the two the
/// statute makes public.
#[test]
fn every_rest_handler_authenticates_and_authorizes() {
    let routed = routed_handlers();
    assert!(
        routed.len() >= 25,
        "only {} routed handlers found — the extractor has drifted from main.rs",
        routed.len()
    );

    let text = src(&format!("{HANDLER_MODULE}.rs"));
    let mut offenders = Vec::new();
    for name in routed {
        if PUBLIC_BY_STATUTE.contains(&name.as_str()) {
            continue;
        }
        let Some(start) = text.find(&format!("pub async fn {name}(")) else {
            offenders.push(format!("{name}: routed but not defined"));
            continue;
        };
        let body = compact(item_source(&text, start));
        // `(claims:Claims` / `,claims:Claims` and never `_claims`: a discarded
        // extractor is the shape this whole file exists to catch, and it is the
        // shape sixteen of these handlers had.
        if !body.contains("(claims:Claims") && !body.contains(",claims:Claims") {
            offenders.push(format!(
                "{name}: no `Claims` extractor, or one bound to `_claims` and discarded — \
                 the token is verified and then ignored"
            ));
        }
        if !body.contains("authorize(&cedar,&claims,") {
            offenders.push(format!(
                "{name}: does not call `authorize` — every holder of a token for this tenant \
                 reaches it, and the catalogue routes among them set what customers are billed"
            ));
        }
    }

    assert!(
        offenders.is_empty(),
        "REST handlers missing authentication or authorization:\n  {}",
        offenders.join("\n  ")
    );
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
    let used = actions_used_in_code();
    let permitted = actions_permitted_in_policy();
    // `use-mcp` is the blanket gate the shared MCP middleware applies
    // (`mako_service::mcp_auth::McpAuth::authenticate`), one crate over.
    let enforced_elsewhere: BTreeSet<String> = [MCP_GATE.to_owned()].into();
    let dead: Vec<_> = permitted
        .difference(&used)
        .filter(|a| !enforced_elsewhere.contains(*a))
        .cloned()
        .collect();
    assert!(
        dead.is_empty(),
        "these Cedar actions are granted by policy but checked nowhere — either the \
         endpoint lost its check (and is now unauthorized) or the grant is dead: {dead:?}"
    );
}

/// `Claims` only exists as a `FromRequestParts` extractor; nothing inserts it
/// into request extensions, so `Extension<Claims>` is a guaranteed 500.
#[test]
fn no_handler_extracts_claims_as_a_request_extension() {
    let file = format!("{HANDLER_MODULE}.rs");
    assert!(
        !src(&file).contains("Extension<Claims>"),
        "src/{file} extracts `Extension<Claims>`, but no layer inserts it, so every request \
         to those handlers returns 500 — take `claims: Claims` instead"
    );
}

/// The enforcer is actually injected, and the MCP surface is gated by the same
/// policy rather than by the API-key path alone.
#[test]
fn the_router_injects_the_enforcer_and_gates_mcp_with_it() {
    let main = compact(&src("main.rs"));
    assert!(
        main.contains(
            "CedarEnforcer::from_policy_str(include_str!(\"../policies/productd.cedar\"))"
        ),
        "main.rs does not load policies/productd.cedar — every `authorize` call would then \
         fail to find its `Extension<Arc<CedarEnforcer>>` and answer 500"
    );
    assert!(
        main.contains(".layer(Extension(cedar))"),
        "the enforcer is built and never layered into the router"
    );
    assert!(
        main.contains("McpAuth::from_auth_config_oidc("),
        "the MCP surface is built with `from_auth_config`, which carries no verifier and no \
         policy — `use-mcp` is then never checked and the Angebot register is served to any \
         caller the API-key path admits"
    );
}

// ── The § 41c exemption, and what bounds it ───────────────────────────────────

/// The two public feeds are the *only* unauthenticated routes.
///
/// Written as an equality rather than as a subset check: the risk this guards
/// is not that one of these two loses its exemption, it is that a third route
/// quietly joins them.
#[test]
fn only_the_two_statutory_feeds_are_unauthenticated() {
    let text = src(&format!("{HANDLER_MODULE}.rs"));
    let mut without_claims = Vec::new();
    for name in routed_handlers() {
        let Some(start) = text.find(&format!("pub async fn {name}(")) else {
            continue;
        };
        let body = compact(item_source(&text, start));
        if !body.contains("(claims:Claims") && !body.contains(",claims:Claims") {
            without_claims.push(name);
        }
    }
    without_claims.sort();
    let mut expected: Vec<String> = PUBLIC_BY_STATUTE.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(
        without_claims, expected,
        "the set of routes served without a bearer token has changed. Only the two § 41c \
         EnWG comparison feeds may be public, and only because the statute obliges their \
         publication"
    );
}

/// The exemption's justification, made checkable.
///
/// § 41c obliges publication of the *tariff offer* — not of the draft that is
/// still being priced, and not of the categories that never appear in a
/// comparison portal. Both feeds serve `fetch_comparison_feed`, so the bound
/// lives in one query, and it is that bound rather than a credential which
/// makes the open route safe.
#[test]
fn the_public_feeds_serve_only_published_rows() {
    let pg = src("pg.rs");
    let start = pg
        .find("pub async fn fetch_comparison_feed(")
        .expect("fetch_comparison_feed is defined");
    let body = item_source(&pg, start);
    assert!(
        body.contains("product_status = 'PUBLISHED'"),
        "fetch_comparison_feed no longer restricts the § 41c feed to PUBLISHED rows, so the \
         two unauthenticated routes now serve draft and withdrawn tariffs to the open internet"
    );
    assert!(
        body.contains("category = ANY($2)") && body.contains("FEED_CATEGORIES"),
        "fetch_comparison_feed no longer restricts the § 41c feed to the FEED_CATEGORIES \
         allowlist, so categories that are not a comparable tariff offer are published"
    );

    let handlers = src(&format!("{HANDLER_MODULE}.rs"));
    for name in PUBLIC_BY_STATUTE {
        let start = handlers
            .find(&format!("pub async fn {name}("))
            .unwrap_or_else(|| panic!("{name} is defined"));
        let body = compact(item_source(&handlers, start));
        assert!(
            body.contains("fetch_comparison_feed(&pool,&lf_mp_id,&q)"),
            "{name} no longer reads through `fetch_comparison_feed`, so the PUBLISHED bound \
             above no longer applies to it"
        );
    }
}

// ── What the policy actually decides ──────────────────────────────────────────

fn enforcer() -> CedarEnforcer {
    CedarEnforcer::from_policy_str(POLICY).expect("productd.cedar parses")
}

/// A verified token for this tenant carrying no market role — an auditor's, or
/// the narrow service credential `billingd` resolves products with.
fn role_less() -> CedarPrincipal {
    CedarPrincipal {
        sub: "svc-billingd".to_owned(),
        tenant: TENANT.to_owned(),
        roles: vec![],
    }
}

/// The supplier's own identity — staff, or a peer service's credential.
fn operator() -> CedarPrincipal {
    CedarPrincipal {
        sub: "pricing-desk".to_owned(),
        tenant: TENANT.to_owned(),
        roles: vec!["LF".to_owned()],
    }
}

/// The actions a token of the tenant reaches without any market role.
const ROLE_LESS_ACTIONS: [&str; 2] = ["read-product", "read-marktpreise"];

#[test]
fn a_role_less_token_reads_the_catalogue_and_moves_no_price() {
    let enforcer = enforcer();
    let auditor = role_less();
    for action in ROLE_LESS_ACTIONS {
        assert!(
            enforcer.check(&auditor, action, TENANT).is_ok(),
            "a role-less caller of the tenant must reach `{action}` — `billingd` resolves \
             products with exactly such a credential, and an endpoint no caller can reach is \
             worse than one too many can"
        );
    }
    for action in actions_permitted_in_policy() {
        if ROLE_LESS_ACTIONS.contains(&action.as_str()) {
            continue;
        }
        assert!(
            enforcer.check(&auditor, &action, TENANT).is_err(),
            "`{action}` is reachable without a market role"
        );
    }
}

/// The escalation this policy exists to stop, named as the acts rather than as
/// a set difference: with nothing but a valid tenant token, a caller could
/// rewrite a live tariff's Arbeitspreis, import an EPEX curve every § 41a
/// customer is then billed against, or accept an Angebot in the supplier's name
/// — which emits the CloudEvent `vertragd` turns into a supply contract.
#[test]
fn a_role_less_token_cannot_reprice_the_catalogue() {
    let enforcer = enforcer();
    let auditor = role_less();
    for action in [
        "write-product",
        "write-marktpreise",
        "write-angebot",
        "versenden-angebot",
        "entscheiden-angebot",
        "expire-angebote",
        // The MCP tools read the Angebot register — named prospects and the
        // bespoke prices quoted to them.
        MCP_GATE,
    ] {
        assert!(
            enforcer.check(&auditor, action, TENANT).is_err(),
            "{action} is reachable with a token that carries no market role"
        );
    }
}

#[test]
fn an_operator_reaches_every_action_of_its_own_tenant() {
    let enforcer = enforcer();
    let operator = operator();
    for action in actions_permitted_in_policy() {
        assert!(
            enforcer.check(&operator, &action, TENANT).is_ok(),
            "the LF operating this deployment is refused {action:?} — an endpoint no caller \
             can reach is worse than one too many can"
        );
    }
}

/// The three roles that are not the LF but do operate a catalogue of their own:
/// ENERGIEDIENSTLEISTUNG covers Messstellenbetrieb an MSB sells, and an ESA
/// sells the same catalogue of services.
#[test]
fn the_msb_and_esa_deployments_reach_their_own_catalogue() {
    let enforcer = enforcer();
    for role in ["MSB", "ESA", "ADMIN"] {
        let principal = CedarPrincipal {
            sub: format!("operator-{role}"),
            tenant: TENANT.to_owned(),
            roles: vec![role.to_owned()],
        };
        for action in actions_permitted_in_policy() {
            assert!(
                enforcer.check(&principal, &action, TENANT).is_ok(),
                "a {role} deployment is refused {action:?} in its own catalogue"
            );
        }
    }
}

#[test]
fn a_token_from_another_tenant_reaches_nothing() {
    let enforcer = enforcer();
    let foreign = CedarPrincipal {
        sub: "svc-foreign".to_owned(),
        tenant: "9900000000001".to_owned(),
        roles: vec![
            "LF".to_owned(),
            "MSB".to_owned(),
            "ESA".to_owned(),
            "ADMIN".to_owned(),
        ],
    };
    for action in actions_permitted_in_policy() {
        assert!(
            enforcer.check(&foreign, &action, TENANT).is_err(),
            "an operator of another tenant reaches {action:?} — the catalogue of a supplier \
             it does not operate"
        );
    }
}

/// The routes that must never lose their write action, named as routes rather
/// than as actions — so a later refactor that moves one of them onto the
/// role-less read action fails here.
#[test]
fn the_repricing_routes_authorize_a_write_action() {
    let text = src(&format!("{HANDLER_MODULE}.rs"));
    for (handler, expected) in [
        ("put_product", "write-product"),
        ("delete_product", "write-product"),
        ("put_energiemix", "write-product"),
        ("delete_energiemix_handler", "write-product"),
        ("put_epex_prices", "write-marktpreise"),
        ("put_nehs_price", "write-marktpreise"),
        ("post_angebot_annehmen", "entscheiden-angebot"),
        ("post_angebot_versenden", "versenden-angebot"),
    ] {
        let start = text
            .find(&format!("pub async fn {handler}("))
            .unwrap_or_else(|| panic!("{handler} is defined"));
        let body = compact(item_source(&text, start));
        assert!(
            body.contains(&format!("authorize(&cedar,&claims,\"{expected}\"")),
            "{handler} does not authorize {expected:?}"
        );
        assert!(
            !ROLE_LESS_ACTIONS.contains(&expected),
            "{handler} authorizes {expected:?}, which any token of the tenant reaches"
        );
    }
}

/// The resource tenant every handler passes is the deployment's, not the
/// token's.
///
/// Passing `claims.tenant()` would make the whole
/// `principal_tenant == resource_tenant` clause tautological — every policy in
/// this file would then admit every tenant.
#[test]
fn the_resource_tenant_is_the_deployments_own() {
    let text = compact(&src(&format!("{HANDLER_MODULE}.rs")));
    let needle = "authorize(&cedar,&claims,";
    let mut calls = 0usize;
    let mut offenders = Vec::new();
    let mut rest = text.as_str();
    while let Some(idx) = rest.find(needle) {
        rest = &rest[idx + needle.len()..];
        calls += 1;
        let Some(close) = rest.find(')') else { break };
        let args = &rest[..close];
        // `"action",<resource_tenant>` — the resource tenant is what follows the
        // action literal.
        let Some((action, resource)) = args.rsplit_once(',') else {
            offenders.push(args.to_owned());
            continue;
        };
        if resource != "&cfg.tenant" {
            offenders.push(format!("{action} → {resource}"));
        }
    }
    assert!(calls > 0, "the extractor found no `authorize` calls");
    assert!(
        offenders.is_empty(),
        "{} of {calls} `authorize` calls do not scope the decision to the deployment's own \
         tenant. Passing the token's tenant would make \
         `context.principal_tenant == context.resource_tenant` true for every caller: {:?}",
        offenders.len(),
        offenders
    );
}
