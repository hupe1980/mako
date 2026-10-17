// Cedar ABAC policies for processd
//
// Context fields injected by every handler check:
//   context.principal_tenant  — mako_tenant JWT claim
//   context.principal_roles   — mako_roles JWT claim  (e.g. ["LF", "NB"])
//   context.resource_tenant   — owning tenant of the target data row
//
// Completeness:
//   Cedar is default-deny, so an action checked in a handler but named in no
//   policy below makes that endpoint return 403 to every caller. The
//   `authorization_guard` test pins both directions — every action checked in
//   code is permitted here, and every action permitted here is checked in code.
//
// Why writes are role-gated:
//   processd does not merely report decisions, it *makes* them. Approving a
//   queue entry dispatches the market answer; `start-supply` / `end-supply`
//   initiate and terminate a supply relationship on the market. Those are the
//   LF's own processes, so they require the LF role rather than "any
//   authenticated caller" — an NB principal holding a token for this
//   deployment has no business starting a supply in the operator's name.

// ── Reads — any authenticated caller of the tenant ───────────────────────────
//
// The decision log, the approval queue and the EoG case log are operational
// evidence: § 20 Abs. 1 EnWG parity audits and the § 38 EnWG timer both have to
// be readable by whoever is answering for them. The process read model —
// `read-process` (`/obs/processes`), `read-kpi` (`/obs/kpis` and the § 7a Abs. 5
// EnWG Gleichbehandlung report), `read-overdue` (`/obs/overdue`) — is the same
// kind of evidence, read by the same callers.

permit(
    principal,
    action in [
        Action::"read-decisions",
        Action::"read-queue",
        Action::"read-eog",
        Action::"read-neuanlage",
        Action::"read-process",
        Action::"read-kpi",
        Action::"read-overdue",
        Action::"use-mcp"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant
};

// ── Deciding a queued process — NB, LF or MSB ────────────────────────────────
//
// An approval-queue entry is decided by the role that owes the answer: the NB
// for an escalated or held-back GPKE / GeLi Gas Anmeldung, the LF for the GPKE
// Lieferende / Beendigung der Zuordnung queries, the MSB for an escalated
// MSB-Wechsel or a § 14a Steuerungsauftrag whose SteuerbareRessource could not
// be auto-checked. Approving dispatches the market message, so this is a write.
//
// Every role that enqueues must appear here: an entry whose role is absent is a
// permanent 403.

permit(
    principal,
    action == Action::"decide-queue",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    (context.principal_roles.contains("NB") ||
     context.principal_roles.contains("LF") ||
     context.principal_roles.contains("MSB"))
};

// ── Identifying a Neuanlage — NB only ────────────────────────────────────────
//
// `PUT /api/v1/neuanlage/{id}/identifikation` names the Marktlokation the NB
// matched a newly commissioned Anmeldung to. It is what ends the `E_0608`
// Prüflauf and lets the answer go out, so it is a write and it belongs to the
// grid operator's own NIS — no other role has a basis for it.

permit(
    principal,
    action == Action::"identify-neuanlage",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};

// ── Initiating a supply process — LF only ────────────────────────────────────
//
// `start-supply` dispatches UTILMD 55001/44001 and `end-supply` 55004/44004; both
// commit the operator to a market position in the LF role. § 7 EnWG separation
// is enforced at the binary level by the `role-lf-*` Cargo features; this is
// the same boundary at the token level.

permit(
    principal,
    action == Action::"initiate-supply",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("LF")
};

// ── Rebuilding the process read model — ADMIN only ───────────────────────────
//
// `POST /api/v1/obs/rebuild` replays marktd's CloudEvent log into
// `process_projections`. It rewrites the evidence the Gleichbehandlungsbericht
// is drawn from, so it is an operator capability, not a market role's.

permit(
    principal,
    action == Action::"rebuild-projection",
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("ADMIN")
};

// ── The Sperr-/Entsperrauftrag queue — NB only ───────────────────────────────
//
//   "read-sperr-order"    — GET the queue, a single order, or /stats, and the
//                           four Sperrauftrag MCP tools. The queue is dispatch
//                           data — which household is to be disconnected, and
//                           where the technician should meet them.
//   "create-sperr-order"  — POST a new order. Schedules a physical visit.
//   "execute-sperr-order" — PUT .../execute or .../fail. Reports the outcome to
//                           the Lieferant as IFTSTA 21039; the message cannot be
//                           taken back, only corrected by another one.
//   "cancel-sperr-order"  — PUT .../cancel. Withdraws a pending order. It only
//                           ever prevents a disconnection, but an order cancelled
//                           without telling the Lieferant still leaves their
//                           process waiting, so it is held to the same role.
//
// The NB receives the Sperrauftrag, sends the field team, and reports the
// Auftragsstatus; no other role has a basis for any of it.

permit(
    principal,
    action in [
        Action::"read-sperr-order",
        Action::"create-sperr-order",
        Action::"execute-sperr-order",
        Action::"cancel-sperr-order"
    ],
    resource
) when {
    context.principal_tenant == context.resource_tenant &&
    context.principal_roles.contains("NB")
};
