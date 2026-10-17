//! `netzbilanzd` — the Netzbetreiber's outbound billing daemon.
//!
//! Port: `:8680`
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/billing/run` | Settle, check and store invoices for a period |
//! | `GET`  | `/api/v1/billing/drafts` | List invoices — filter by Sparte, page by cursor |
//! | `GET`  | `/api/v1/billing/drafts/{id}` | One invoice in full |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/dispatch` | Merge Fremdkosten, re-check, send via `makod` |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/reject` | Discard a draft and reopen the period |
//! | `POST` | `/api/v1/billing/drafts/dispatch-batch` | Dispatch many, each in its own transaction |
//! | `POST` | `/api/v1/billing/drafts/{id}/storno` | Stornorechnung — recomputed and negated |
//! | `POST` | `/api/v1/billing/drafts/{id}/korrektur` | Korrekturrechnung from corrected inputs |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/mark-paid` | REMADV 33001 |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/mark-disputed` | REMADV 33002/33003/33004 |
//! | `POST` | `/api/v1/billing/mmm-run/{malo_id}` | Monthly MMM saldo, measured half from `edmd` |
//! | `POST` | `/api/v1/billing/ggv-nne/{ggv_malo_id}` | §42b EnWG per-tenant NNE |
//! | `GET`  | `/api/v1/billing/malo/{malo_id}` | Billing history for one MaLo |
//! | `GET`  | `/api/v1/billing/summary` | Monthly totals by PID, Sparte, status, Rechnungsart |
//! | `GET`  | `/api/v1/billing/audit` | § 147 AO / § 14b UStG export |
//! | `GET/PUT` | `/api/v1/billing/fremdkosten/{draft_id}` | Typed external-cost pass-through |
//! | `POST` | `/api/v1/webhooks/remadv` | REMADV CloudEvent ingest |
//! | `GET/PUT` | `/api/v1/redispatch/kostenblatt/{activation_id}` | Redispatch 2.0 cost sheet |
//! | `POST` | `/api/v1/redispatch/kostenblatt/{activation_id}/compute` | Quantify from `edmd` Lastgang |
//! | `GET`  | `/api/v1/redispatch/kostenblatt` | List a month's records |
//! | `GET`  | `/api/v1/redispatch/kostenblatt/gaps/{year}/{month}` | Unquantified activations |
//! | `POST` | `/api/v1/redispatch/kostenblatt/submit/{year}/{month}` | Submit the month |
//! | `POST` | `/api/v1/redispatch/verguetung/{activation_id}/compute` | §13a Abs. 2 EnWG |
//! | `POST` | `/api/v1/redispatch/ausfallarbeit/*` | BilAReM Kap. 3 compute surface |
//! | `POST` | `/api/v1/mabis/sync` | File a Summenzeitreihe (MSCONS 13003) with the BIKO |
//! | `GET`  | `/api/v1/mabis/runs[/{id}]` | Submission runs, per-territory series |
//! | `PUT`  | `/api/v1/mabis/runs/{id}/retry` | Retry a failed run under a new version |
//! | `POST` | `/api/v1/mabis/datenstatus` | Record the BIKO's Datenstatus (IFTSTA 21003/21004) |
//! | `POST` | `/api/v1/mabis/pruefmitteilung` | Record a Prüfmitteilung (IFTSTA 21000/21001) |
//! | `GET`  | `/api/v1/mabis/korrekturbedarf` | Open negative Prüfmitteilungen |
//! | `POST` | `/mcp` | Read-only MCP tooling |
//!
//! Every route above takes a verified OIDC bearer and a Cedar decision from
//! `policies/netzbilanzd.cedar`, except `POST /api/v1/webhooks/remadv`, which is
//! authenticated by the `inbound_secret` HMAC. The daemon refuses to start
//! where either is unconfigured. The `/api/v1/mabis/*` routes are mounted only
//! when the configuration carries a `[mabis]` section.
//!
//! Liveness and readiness (`/health`, `/health/ready`) are mounted by
//! `mako_service::run`.

#![deny(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<netzbilanzd::Netzbilanzd>().await
}
