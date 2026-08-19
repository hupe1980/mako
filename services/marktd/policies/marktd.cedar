// marktd authorization policies — FV2025-10-01+
//
// Principal attributes (from JWT):
//   context.principal_tenant  — `mako_tenant` custom claim (operator MP-ID)
//   context.principal_roles   — `mako_roles` custom claim (["NB", "LF", ...])
//
// Resource attribute:
//   context.resource_tenant   — owning tenant passed by the handler
//                                (= `[markt] tenant` for this instance)
//
// Isolation model:
//   A token issued by the IDP carries `mako_tenant = "<operator-MP-ID>"`.
//   The running marktd instance is configured with the same MP-ID in `tenant`.
//   Access is permitted only when the two match. This prevents cross-tenant
//   data leakage even if a valid token for another tenant reaches this instance.
//
// Completeness:
//   Cedar is default-deny, so an action string that appears in a handler but in
//   no policy below makes that endpoint permanently return 403. The
//   `cedar_action_coverage` guard test pins both directions — every action
//   checked in code is permitted here, and every action permitted here is
//   checked somewhere in code.
//
// Role model:
//   Reads are open to any authenticated principal of the tenant.
//   Writes to *counterparty-facing authority data* — price sheets, grid and
//   balancing topology, the §36 Grundversorger Feststellung, the §42 Energiemix
//   — require the NB role, because the NB is the party the regulation makes
//   responsible for publishing them.
//   Writes that shape *this deployment's own behaviour* — webhook subscriptions,
//   the dead-letter queue — require the ADMIN role: a subscription is an
//   outbound data-export channel for every event this hub emits, so it is an
//   operator-level capability rather than a market-role one.

// ── Read access — any role, same tenant ──────────────────────────────────────

permit(
    principal,
    action in [
        Action::"read-malo",
        Action::"read-melo",
        Action::"read-nb-contract",
        Action::"read-partner",
        Action::"read-preisblatt",
        Action::"read-versorgungsstatus",
        Action::"read-nelo",
        Action::"read-tranche",
        Action::"read-malo-grid",
        Action::"read-device",
        Action::"read-sr",
        Action::"read-sharing-eligibility",
        Action::"read-netzzugang",
        Action::"read-msb-rv-gas",
        Action::"read-melo-msb",
        Action::"read-bilanzierung",
        Action::"read-mabis-zp",
        Action::"read-grundversorger",
        Action::"read-energiemix",
        Action::"read-correlation",
        Action::"read-lokationszuordnung",
        Action::"read-pricat",
        Action::"read-mmma-preis",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Write access — any role, same tenant ─────────────────────────────────────

permit(
    principal,
    action in [
        Action::"write-malo",
        Action::"write-melo",
        Action::"write-nb-contract",
        Action::"write-partner",
        Action::"write-versorgungsstatus",
        Action::"write-device",
        Action::"write-sr",
        Action::"write-netzzugang",
        Action::"write-msb-rv-gas",
        Action::"write-melo-msb",
        Action::"write-bilanzierung",
        Action::"write-mabis-zp",
        Action::"write-lokationszuordnung"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── NB-authority writes — NB role required ───────────────────────────────────
//
// Each of these is data the Netzbetreiber publishes to counterparties and that
// another service then treats as authoritative:
//
//   write-preisblatt     PreisblattNetznutzung / -Messung / -KA / -Dienstleistung
//                        / -Hardware — the tariff basis invoic-checker validates
//                        INVOIC plausibility against (§ 22 MessZV).
//   write-nelo           Netz-Element-Lokationen for Redispatch 2.0.
//   write-tranche        GPKE Teil 4 „Daten der Tranche" balancing topology.
//   write-malo-grid      NB grid topology feeding processd's Anmeldung STP.
//   write-grundversorger §36 Abs. 2 EnWG Feststellung — addresses the UTILMD
//                        55013/44013 EoG Zuordnung, so a wrong value routes the
//                        statutory fallback supply to the wrong supplier.
//   write-energiemix     §42 EnWG grid-area Energiemix — LFs disclose the
//                        Reststrommix from it.
//   write-mmma-preis     MMMA Gas / MMM Strom settlement prices used by
//                        netzbilanzd's MMM billing and invoicd's check 6.
//   dispatch-pricat      (Re-)sends PRICAT 27003 to every active LF partner.

permit(
    principal,
    action in [
        Action::"write-preisblatt",
        Action::"write-nelo",
        Action::"write-tranche",
        Action::"write-malo-grid",
        Action::"write-grundversorger",
        Action::"write-energiemix",
        Action::"write-mmma-preis",
        Action::"dispatch-pricat"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};

// ── ESA consent registry — MSB or ESA role ───────────────────────────────────
//
// §49 Abs. 2 Nr. 9 MsbG. The registry records the existence and lifecycle of an
// Anschlussnutzer's consent, and a revocation fires the ORDERS 17008
// Abbestellung at makod. Only the two parties to that relationship — the MSB
// that holds the metering point and the ESA the consent authorises — may read
// or change it; an unrelated LF has no lawful interest in it.

permit(
    principal,
    action in [Action::"read-einwilligung", Action::"write-einwilligung"],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("MSB") ||
     context.principal_roles.contains("ESA"))
};

// ── Operator-level capabilities — ADMIN role ─────────────────────────────────
//
// manage-subscription — a webhook subscription receives *every* event this hub
//   emits that matches its filter, including MaLo/MeLo master data and
//   VersorgungsStatus transitions. Registering one is an outbound data-export
//   decision, and `POST /subscriptions/{id}/test` returns the target's HTTP
//   status, which makes an unrestricted version of it a network probe. Both are
//   operator capabilities, not market-role ones.
// manage-fanout — the dead-letter queue and the CloudEvent replay log. § 147 AO
//   / GoBD evidence; requeueing a dead letter re-emits a market event.

permit(
    principal,
    action in [Action::"manage-subscription", Action::"manage-fanout"],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("ADMIN")
};
