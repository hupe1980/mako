//! MCP server for `vertragd` — Contract & Customer Management.

use axum::{
    Router,
    middleware::{self, Next},
};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    prompt, prompt_handler, prompt_router, schemars, tool, tool_handler, tool_router,
    transport::streamable_http_server::StreamableHttpServerConfig,
};
use schemars::JsonSchema;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct VertragdMcpState {
    pub pool: PgPool,
    pub tenant: String,
    pub auth: mako_service::mcp_auth::McpAuth,
    /// The operator policy and posture this deployment runs under.
    pub policy: Arc<mako_service::policy::PolicyReport>,
}

/// A tool that takes no arguments.
///
/// **Not `serde_json::Value`.** Its JSON Schema is the empty schema — no
/// `type` — and the MCP specification requires a tool's `inputSchema` to have
/// root type `object`. `rmcp` asserts that while building the router, so a
/// single argument-less tool declared as `Parameters<serde_json::Value>`
/// panics the whole service at startup. Nothing else catches it: the router is
/// built in `main`, and no test starts the binary.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct NoParams {}

/// How far ahead to look, in days.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct LookAheadDaysParams {
    /// Days ahead. Defaults to 30 and is clamped to the tool's own range.
    pub days: Option<i64>,
}

/// Page size for the Anmeldungen listing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListAnmeldungenParams {
    /// Max results (default 200, max 500).
    pub limit: Option<i64>,
    /// Only those makod has not accepted yet — no process carries them.
    #[serde(default)]
    pub ohne_prozess: bool,
}

/// The MaLos a worklist page names.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KundenZuMalosParams {
    /// One to 200 MaLo-IDs.
    pub malos: Vec<String>,
}

/// What `kunden_zu_malos` returns.
#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct KundenZuMalos {
    /// One row per MaLo with a Vertragskomponente in this tenant.
    pub kunden: Vec<crate::pg::MaloKunde>,
}

/// The most MaLos one `kunden_zu_malos` call resolves — a worklist page.
const MALOS_PER_CALL: usize = 200;

/// What `list_anmeldungen` returns.
#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct Anmeldungen {
    /// Lieferstellen in `ANGEMELDET`, by customer name — at most `limit`.
    pub anmeldungen: Vec<crate::pg::Anmeldung>,
    /// How many Lieferstellen are in `ANGEMELDET`, however many are listed.
    pub gesamt: i64,
}

/// Filters for the customer listing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchKundenParams {
    /// What the person typed: the start of any word of a name, or of a Kundennummer, or a whole
    /// MaLo-ID. At least three characters.
    pub query: String,
}

/// What `search_kunden` returns. Its JSON Schema is the tool's `outputSchema`,
/// pinned in `mcp.snapshot.json` for the Desk's generated types.
#[derive(Debug, serde::Serialize, JsonSchema)]
pub struct KundenSuche {
    /// The query, trimmed, as it was searched.
    pub query: String,
    /// At most twenty customers, by name.
    pub treffer: Vec<crate::pg::kunden::KundeTreffer>,
    /// More customers match than were returned — the person should type more.
    pub mehr: bool,
}

/// The most hits one search returns; a person narrows by typing, not paging.
const SEARCH_HITS: usize = 20;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListKundenParams {
    /// `PRIVAT` or `GEWERBE`; unset lists both.
    pub kundentyp: Option<String>,
    /// Page size. Defaults to 100, clamped to 500.
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VertragIdParams {
    pub id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct KundeSubParams {
    pub oidc_sub: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListParams {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MaloProduktParams {
    pub malo_id: String,
    /// Period start (YYYY-MM-DD). With `to`, returns the slices covering it.
    pub from: Option<String>,
    /// Period end (YYYY-MM-DD), inclusive.
    pub to: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct KundeIdParams {
    pub kunden_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckPreisgarantieParams {
    /// UUID of the Versorgungsvertrag to check.
    pub vertrag_id: String,
    /// Proposed Wirksamkeit date (YYYY-MM-DD) — the day the new tariff should take effect.
    pub wirksamkeit: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListKuendigungenParams {
    /// Max results (default 50, max 200).
    pub limit: Option<i64>,
}

#[derive(Clone)]
pub struct VertragdMcpHandler {
    state: Arc<VertragdMcpState>,
    #[expect(
        dead_code,
        reason = "rmcp's `#[tool_handler]` / `#[prompt_handler]` call `Self::tool_router()` / `Self::prompt_router()` per request; the stored router is never read"
    )]
    tool_router: ToolRouter<VertragdMcpHandler>,
    #[expect(
        dead_code,
        reason = "rmcp's `#[tool_handler]` / `#[prompt_handler]` call `Self::tool_router()` / `Self::prompt_router()` per request; the stored router is never read"
    )]
    prompt_router: rmcp::handler::server::router::prompt::PromptRouter<VertragdMcpHandler>,
}

#[tool_router]
impl VertragdMcpHandler {
    fn new(state: Arc<VertragdMcpState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    #[tool(
        description = "The operator policy and posture this vertragd deployment runs under: each [policy.dsr] reading (which categories are portable under Art. 20 DSGVO, which processes are stated as automated decisions under Art. 15 Abs. 1 lit. h) with its value, whether it was set or defaulted, and its source. Read-only; a setting changes only by editing the config and restarting.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_policy(&self) -> Result<CallToolResult, McpError> {
        ContentBlock::json(serde_json::to_value(&*self.state.policy).unwrap_or_default())
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
    }

    #[tool(
        description = "Get a Versorgungsvertrag and all its Vertragskomponenten (STROM/GAS/HEMS/...) by contract UUID.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_vertrag_status(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_vertrag, list_komponenten};
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        match fetch_vertrag(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(v)) => {
                let komp = list_komponenten(&self.state.pool, id)
                    .await
                    .unwrap_or_default();
                ContentBlock::json(serde_json::json!({ "vertrag": v, "komponenten": komp }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("Vertrag {} not found", id),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "List all open Versorgungsverträge (AKTIV, IN_BEARBEITUNG, TEILERFUELLUNG, GEKÜNDIGT).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_offene_vertraege(
        &self,
        Parameters(p): Parameters<ListParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_offene_vertraege;
        match list_offene_vertraege(
            &self.state.pool,
            &self.state.tenant,
            p.limit.unwrap_or(50).min(200),
        )
        .await
        {
            Ok(rows) => {
                ContentBlock::json(serde_json::json!({ "count": rows.len(), "vertraege": rows }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    #[tool(
        description = "Resolve an OIDC sub to a customer profile and their active MaLo IDs. Used for portald authorization.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_kunde_by_sub(
        &self,
        Parameters(p): Parameters<KundeSubParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_kunde_by_sub, list_aktive_malo_ids};
        match fetch_kunde_by_sub(&self.state.pool, &p.oidc_sub, &self.state.tenant).await {
            Ok(Some(k)) => {
                let malo_ids = list_aktive_malo_ids(&self.state.pool, k.id, &self.state.tenant)
                    .await
                    .unwrap_or_default();
                ContentBlock::json(serde_json::json!({ "kunde": k, "active_malo_ids": malo_ids }))
                    .map(|b| CallToolResult::success(vec![b]))
                    .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("No customer with sub={}", p.oidc_sub),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List Versorgungsverträge expiring within N days (vertragsende or preisgarantie_bis).
    ///
    /// Regulatory basis: § 309 Nr. 9 lit. b BGB — 30-day advance notice for renewal.
    /// §41 EnWG — customer notification before price-lock expiry.
    #[tool(
        description = "List contracts expiring within N days (vertragsende or preisgarantie_bis). Default: 30 days. Use for proactive renewal outreach.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_expiring_contracts(
        &self,
        Parameters(p): Parameters<LookAheadDaysParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::find_expiring_vertraege;
        let days = p.days.unwrap_or(30).clamp(1, 365);
        match find_expiring_vertraege(&self.state.pool, &self.state.tenant, days, false).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "look_ahead_days": days,
                "vertraege": rows,
                "hint": "Check auto_renewal field — contracts with auto_renewal=true should receive 30-day advance notice (§ 309 Nr. 9 lit. b BGB).",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List scheduled Tarifwechsel whose price-change notice is still owed.
    #[tool(
        description = "List supplier-initiated product slices that take effect in the future \
                       and whose § 41 Abs. 5 EnWG price-change notice has not gone out yet. A \
                       switch the customer asked for is not listed: it owes no notice. The \
                       required lead depends on the contract: 6 weeks in the Grundversorgung \
                       (§ 5 Abs. 2 StromGVV / GasGVV), 1 month for a Haushaltskunde in a \
                       Sondervertrag, 2 weeks otherwise (§ 41 Abs. 5 Satz 2 EnWG). Each row \
                       states which applied, whether the lead is actually met, and why the last \
                       attempt to issue the notice failed.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pending_tarifwechsel(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::domain::{Vertragsart, preisanpassungsregime};
        let today = mako_fristen::heute();
        let rows = crate::pg::offene_preisanpassungen(&self.state.pool, &self.state.tenant, today)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let pending: Vec<serde_json::Value> = rows
            .iter()
            .map(|r| {
                let regime =
                    preisanpassungsregime(Vertragsart::from_db(&r.vertragsart), r.haushaltskunde);
                let vorlauf = (r.wirksam_ab - today).whole_days();
                serde_json::json!({
                    "komp_id": r.komp_id,
                    "vertrag_id": r.vertrag_id,
                    "malo_id": r.malo_id,
                    "sparte": r.sparte,
                    "bisheriges_produkt": r.bisheriges_produkt,
                    "neues_produkt": r.neues_produkt,
                    "wirksam_ab": r.wirksam_ab.to_string(),
                    "vorlauf_tage": vorlauf,
                    "erforderliche_frist": regime.bezeichnung,
                    "fruehestens_wirksam": regime.fruehestens_wirksam(today).to_string(),
                    "frist_gewahrt": regime.frist.gewahrt(today, r.wirksam_ab),
                    "rechtsgrundlage": regime.rechtsgrundlage,
                    "notif_versuche": r.notif_versuche,
                    "notif_letzter_fehler": r.notif_letzter_fehler,
                })
            })
            .collect();
        ContentBlock::json(serde_json::json!({
            "count": pending.len(),
            "pending": pending,
            "hinweis": "frist_gewahrt=false ist eine Fristverletzung, keine Warnung: die \
                        Anzeige ist dann bereits zu spät.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Which product a MaLo is billed on, over time.
    #[tool(
        description = "Which product a Marktlokation is billed on. With `from`/`to` returns the \
                       valid-time slices covering that period, in order — a Tarifwechsel inside \
                       the period splits it, and each slice is billed under its own product. \
                       Without them, the product in force today. This mapping is a contract \
                       fact (§ 41 Abs. 5 EnWG); ask `productd/resolve_product` for what the code \
                       costs.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_malo_produkt(
        &self,
        Parameters(p): Parameters<MaloProduktParams>,
    ) -> Result<CallToolResult, McpError> {
        let parse = |s: &str| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
                .map_err(|_| McpError::invalid_params("dates must be YYYY-MM-DD", None))
        };
        let heute = mako_fristen::heute();
        let (von, bis) = match (p.from.as_deref(), p.to.as_deref()) {
            (Some(f), Some(t)) => (parse(f)?, parse(t)?),
            _ => (heute, heute),
        };
        let slices =
            crate::pg::malo_slices(&self.state.pool, &self.state.tenant, &p.malo_id, von, bis)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        ContentBlock::json(serde_json::json!({
            "malo_id": p.malo_id,
            "von": von.to_string(),
            "bis": bis.to_string(),
            "slice_count": slices.len(),
            "slices": slices,
            "hinweis": if slices.len() > 1 {
                "Mehr als eine Scheibe: der Zeitraum enthält einen Tarifwechsel und wird \
                 abschnittsweise abgerechnet."
            } else if slices.is_empty() {
                "Keine Produktzuordnung — die Belieferung hat noch nicht begonnen oder \
                 ist beendet."
            } else {
                "Ein Produkt über den gesamten Zeitraum."
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// Every Lieferstelle registered at the Netzbetreiber and not answered yet,
    /// with its customer and the makod process carrying it. Whether one is
    /// late is makod's deadline for its MaLo (`get_process`), never an age
    /// counted here.
    #[tool(
        description = "List Lieferstellen in ANGEMELDET (registered at the NB, not answered yet) with customer, MaLo, Sparte, contract and makod process id, and `gesamt`, how many there are; `ohne_prozess` keeps those no makod process carries yet. Carries no age: whether one is late is the process's Frist — processd list_open_processes.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<Anmeldungen>()
    )]
    async fn list_anmeldungen(
        &self,
        Parameters(p): Parameters<ListAnmeldungenParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = p.limit.unwrap_or(200).clamp(1, 500);
        let seite = crate::pg::list_anmeldungen(
            &self.state.pool,
            &self.state.tenant,
            p.ohne_prozess,
            limit,
        )
        .await
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        serde_json::to_value(Anmeldungen {
            anmeldungen: seite.anmeldungen,
            gesamt: seite.gesamt,
        })
        .map(CallToolResult::structured)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// The customer behind each of a page of MaLos, in one statement — so a
    /// worklist names its rows with one call per page, never one per row.
    #[tool(
        description = "For each of up to 200 MaLo-IDs, the customer of its latest Vertragskomponente: name, Kundennummer, Sparte and status. A MaLo without one is absent.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<KundenZuMalos>()
    )]
    async fn kunden_zu_malos(
        &self,
        Parameters(p): Parameters<KundenZuMalosParams>,
    ) -> Result<CallToolResult, McpError> {
        if p.malos.is_empty() || p.malos.len() > MALOS_PER_CALL {
            return Err(McpError::invalid_params(
                format!("malos holds 1 to {MALOS_PER_CALL} MaLo-IDs"),
                None,
            ));
        }
        let kunden = crate::pg::kunden_zu_malos(&self.state.pool, &self.state.tenant, &p.malos)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        serde_json::to_value(KundenZuMalos { kunden })
            .map(CallToolResult::structured)
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// Get full B2B portfolio for a customer (all active MaLo/Sparte combos).
    #[tool(
        description = "Get all active Vertragskomponenten for a B2B customer (portfolio view). Returns one row per MaLo/Sparte. Useful for Sammelrechnung enumeration.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_customer_portfolio(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_portfolio_by_kunde;
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid kunden_id UUID", None))?;
        match list_portfolio_by_kunde(&self.state.pool, id, &self.state.tenant).await {
            Ok(rows) => {
                let total_malos = rows.iter().filter(|r| r.malo_id.is_some()).count();
                ContentBlock::json(serde_json::json!({
                    "kunden_id": id,
                    "total_active": rows.len(),
                    "total_malos": total_malos,
                    "komponenten": rows,
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Find customers by what a person remembers.
    ///
    /// The start of a first or last name, a company name or a Kundennummer, or
    /// a whole MaLo-ID of one of the customer's Lieferstellen. Returns at most
    /// twenty, by name, and says when more match. The search runs here, in the
    /// tenant's own table — a client keeps no index of customers.
    #[tool(
        description = "Find customers (Kunden) by the start of any word of their name or of their Kundennummer, or by a MaLo-ID of one of their Lieferstellen. At most 20 hits; `mehr` says more match.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<KundenSuche>()
    )]
    async fn search_kunden(
        &self,
        Parameters(p): Parameters<SearchKundenParams>,
    ) -> Result<CallToolResult, McpError> {
        let query = p.query.trim().to_owned();
        if query.chars().count() < 3 {
            return Err(McpError::invalid_params(
                "query: at least three characters",
                None,
            ));
        }
        let limit = i64::try_from(SEARCH_HITS + 1).unwrap_or(i64::MAX);
        let mut treffer =
            crate::pg::kunden::search_kunden(&self.state.pool, &self.state.tenant, &query, limit)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let mehr = treffer.len() > SEARCH_HITS;
        treffer.truncate(SEARCH_HITS);
        serde_json::to_value(KundenSuche {
            query,
            treffer,
            mehr,
        })
        .map(CallToolResult::structured)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
    }

    /// List all Kunden for this tenant (operator / CRM view).
    ///
    /// Use `?kundentyp=B2C|B2B_SLP|B2B_RLM|B2B_HV` to filter.
    #[tool(
        description = "List all customers (Kunden) for this LF tenant. Filter by kundentyp. Lightweight — no JSONB blobs. Use for CRM/ERP sync and churn analysis.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_alle_kunden(
        &self,
        Parameters(p): Parameters<ListKundenParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_kunden;
        let kundentyp = p.kundentyp.as_deref();
        let limit = p.limit.unwrap_or(100).clamp(1, 500);
        match list_kunden(&self.state.pool, &self.state.tenant, kundentyp, limit).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "kunden": rows,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// The earliest lawful Kündigungstermin for a contract, per reason.
    #[tool(
        description = "Compute the earliest lawful Kündigungstermin for a contract, for every \
                       termination reason. The period comes from the reason and the Vertragsart: \
                       § 20 Abs. 1 StromGVV/GasGVV (2 Wochen) in the Grundversorgung, § 41b Abs. 5 \
                       EnWG (6 Wochen) on a move, § 41 Abs. 5 Satz 4 EnWG (fristlos) after a price \
                       change, otherwise the contractual period capped for consumers by § 309 Nr. 9 \
                       lit. c BGB. Each answer names the rule it applied.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn compute_kuendigungsfrist(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::domain::{Kuendigungsgrund, Vertragsart, kuendigungsfrist};
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        let vertrag = crate::pg::fetch_vertrag(&self.state.pool, id, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
            .ok_or_else(|| McpError::resource_not_found(format!("Vertrag {id} not found"), None))?;
        let kunde = crate::pg::fetch_kunde(&self.state.pool, vertrag.kunden_id, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let haushaltskunde = kunde.is_none_or(|k| k.haushaltskunde);
        let today = mako_fristen::heute();
        let art = Vertragsart::from_db(&vertrag.vertragsart);
        let fristen: serde_json::Map<String, serde_json::Value> = [
            Kuendigungsgrund::Ordentlich,
            Kuendigungsgrund::Preisanpassung,
            Kuendigungsgrund::Umzug,
            Kuendigungsgrund::Lieferantenwechsel,
        ]
        .into_iter()
        .map(|grund| {
            let f = kuendigungsfrist(
                today,
                art,
                haushaltskunde,
                grund,
                vertrag.kuendigungsfrist_monate,
                None,
            );
            (
                grund.as_db().to_owned(),
                serde_json::to_value(f).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect();
        ContentBlock::json(serde_json::json!({
            "vertrag_id": id,
            "vertrags_nr": vertrag.vertrags_nr,
            "vertragsart": vertrag.vertragsart,
            "haushaltskunde": haushaltskunde,
            "today": today.to_string(),
            "kuendigungsfrist_monate": vertrag.kuendigungsfrist_monate,
            "vertragsende": vertrag.vertragsende.map(|d| d.to_string()),
            "fristen": fristen,
            "hinweis": "Eine Preisgarantie sperrt den Tarifwechsel, nicht die Kündigung.",
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }

    /// A customer as an operator sees one: Kundennummer, Kundentyp,
    /// Haushaltskunde, name, and every Lieferstelle with the state of its
    /// supply. Payment data and portal identities are not part of it.
    #[tool(
        description = "Get a customer (Kunde) by UUID: Kundennummer, kundentyp, Haushaltskunde, name, and every Lieferstelle (MaLo, Sparte, Vertrags-Nr., Standort, Lieferbeginn/-ende, status). No payment data. Use after search_kunden or with a kunden_id from a contract.",
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = rmcp::handler::server::tool::schema_for_output::<crate::pg::kunden::KundeView>()
    )]
    async fn get_kunde(
        &self,
        Parameters(p): Parameters<KundeIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = uuid::Uuid::parse_str(&p.kunden_id)
            .map_err(|_| McpError::invalid_params("invalid kunden_id UUID", None))?;
        let view = crate::pg::kunden::fetch_kunde_view(&self.state.pool, id, &self.state.tenant)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        match view {
            Some(view) => serde_json::to_value(view)
                .map(CallToolResult::structured)
                .map_err(|e| McpError::internal_error(e.to_string(), None)),
            None => Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "kunde_not_found: no Kunde {id}."
            ))])),
        }
    }

    /// Get a Rahmenvertrag with all its child Versorgungsverträge.
    #[tool(
        description = "Get a B2B Rahmenvertrag (framework contract) by UUID including all active Versorgungsverträge under it. Essential for B2B portfolio management and Sammelrechnung preparation.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_rahmenvertrag(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::{fetch_rahmenvertrag, list_versorgungsvertraege_by_rahmenvertrag};
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        match fetch_rahmenvertrag(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(r)) => {
                let vertraege = list_versorgungsvertraege_by_rahmenvertrag(
                    &self.state.pool,
                    id,
                    &self.state.tenant,
                )
                .await
                .unwrap_or_default();
                ContentBlock::json(serde_json::json!({
                    "rahmenvertrag": r,
                    "versorgungsvertraege": vertraege,
                    "vertraege_count": vertraege.len(),
                    "hint": "Use get_customer_portfolio for MaLo/Sparte details. Use compute_kuendigungsfrist per Vertrag for Kündigung dates.",
                }))
                .map(|b| CallToolResult::success(vec![b]))
                .map_err(|e| McpError::internal_error(e.message, None))
            }
            Ok(None) => Err(McpError::resource_not_found(
                format!("Rahmenvertrag {id} not found"),
                None,
            )),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List contracts in GEKÜNDIGT status (Kündigung dispatched, lieferende in future).
    #[tool(
        description = "List Versorgungsverträge with an active Kündigung (status=GEKÜNDIGT, lieferende in future). Use to monitor contracts approaching their end and prepare Schlussabrechnung.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_pending_kuendigungen(
        &self,
        Parameters(p): Parameters<ListKuendigungenParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::list_pending_kuendigungen;
        let limit = p.limit.unwrap_or(50).clamp(1, 200);
        match list_pending_kuendigungen(&self.state.pool, &self.state.tenant, limit).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "hint": "These contracts have lieferende in the future. Use widerruf-kuendigung to revoke if needed.",
                "vertraege": rows,
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Check whether a Tarifwechsel is blocked by an active Preisgarantie.
    ///
    /// Returns the preisgarantie_bis date and whether the proposed wirksamkeit
    /// falls within the protected window (§41 EnWG price-lock).
    #[tool(
        description = "Check if a Tarifwechsel is blocked by an active Preisgarantie for a contract. Provide the target wirksamkeit date to get a clear BLOCKED/ALLOWED result with the guarantee expiry date.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_preisgarantie(
        &self,
        Parameters(p): Parameters<CheckPreisgarantieParams>,
    ) -> Result<CallToolResult, McpError> {
        let vertrag_id = uuid::Uuid::parse_str(&p.vertrag_id)
            .map_err(|_| McpError::invalid_params("invalid vertrag_id UUID", None))?;
        let wirksamkeit = time::Date::parse(
            &p.wirksamkeit,
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .map_err(|_| McpError::invalid_params("wirksamkeit must be YYYY-MM-DD", None))?;

        let garantie_bis: Option<time::Date> =
            crate::pg::fetch_vertrag(&self.state.pool, vertrag_id, &self.state.tenant)
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?
                .ok_or_else(|| {
                    McpError::resource_not_found(format!("Vertrag {vertrag_id} not found"), None)
                })?
                .preisgarantie_bis;
        let blocked = garantie_bis.is_some_and(|g| wirksamkeit <= g);
        {
            ContentBlock::json(serde_json::json!({
                "vertrag_id": vertrag_id,
                "wirksamkeit": p.wirksamkeit,
                "status": if blocked { "BLOCKED" } else { "ALLOWED" },
                "preisgarantie_bis": garantie_bis.map(|d| d.to_string()),
                "regulatory_basis": "§41 EnWG — price-lock guarantee protects customer from tariff increases",
                "hint": if blocked {
                    "Tarifwechsel is blocked. Operator can bypass with override_preisgarantie=true — document customer consent first."
                } else {
                    "Tarifwechsel is allowed for this wirksamkeit date."
                },
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None))
        }
    }

    /// Get SEPA/IBAN payment information for a customer.
    #[tool(
        description = "Get the Zahlungsinformation (IBAN/BIC/SEPA details) for a customer. Used for accountingd SEPA reconciliation and payment mandate verification.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn get_zahlungsinformation(
        &self,
        Parameters(p): Parameters<KundeIdParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::fetch_zahlungsinformation;
        let id = uuid::Uuid::parse_str(&p.kunden_id)
            .map_err(|_| McpError::invalid_params("invalid kunden_id UUID", None))?;
        match fetch_zahlungsinformation(&self.state.pool, id, &self.state.tenant).await {
            Ok(Some(z)) => ContentBlock::json(serde_json::json!({
                "kunden_id": id,
                "zahlungsinformation": z,
                "hint": "IBAN stored in plaintext JSONB. iban field validated via ISO 13616 mod-97 on PUT.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Ok(None) => ContentBlock::json(serde_json::json!({
                "kunden_id": id,
                "zahlungsinformation": null,
                "hint": "No Zahlungsinformation stored. Use PUT /api/v1/kunden/{id}/zahlungsinformation to store IBAN/BIC.",
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// List contracts eligible for auto-renewal within N days.
    #[tool(
        description = "List contracts with auto_renewal=true whose vertragsende falls within N days. These need a 30-day advance customer notification (§ 309 Nr. 9 lit. b BGB) before automatic renewal.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn list_auto_renewal_due(
        &self,
        Parameters(p): Parameters<LookAheadDaysParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::pg::find_auto_renewal_due;
        let days = p.days.unwrap_or(30).clamp(1, 90);
        match find_auto_renewal_due(&self.state.pool, &self.state.tenant, days).await {
            Ok(rows) => ContentBlock::json(serde_json::json!({
                "count": rows.len(),
                "look_ahead_days": days,
                "contracts": rows,
                "regulatory_note": "§ 309 Nr. 9 lit. b BGB: customer must receive 30-day advance notice before auto-renewal extends the contract.",
                "action_required": !rows.is_empty(),
            }))
            .map(|b| CallToolResult::success(vec![b]))
            .map_err(|e| McpError::internal_error(e.message, None)),
            Err(e) => Err(McpError::internal_error(e.to_string(), None)),
        }
    }

    /// Check MaKo trigger status for a Versorgungsvertrag.
    ///
    /// Shows whether Lieferbeginn UTILMD was dispatched, how many components
    /// are confirmed/pending/rejected, and whether any are stuck.
    #[tool(
        description = "Check the MaKo Lieferbeginn trigger status for a Versorgungsvertrag. Returns component-level AKTIV/ANGEMELDET/ABGELEHNT breakdown, mako_process_id, and stuck detection. Use to diagnose why a contract is stuck in IN_BEARBEITUNG.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn check_mako_trigger_status(
        &self,
        Parameters(p): Parameters<VertragIdParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = uuid::Uuid::parse_str(&p.id)
            .map_err(|_| McpError::invalid_params("invalid UUID", None))?;
        let komps = crate::pg::list_komponenten(&self.state.pool, id)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let mut by_status: std::collections::BTreeMap<&str, Vec<serde_json::Value>> =
            std::collections::BTreeMap::new();
        for k in &komps {
            by_status
                .entry(k.status.as_str())
                .or_default()
                .push(serde_json::json!({
                    "komp_id": k.id,
                    "sparte": k.sparte,
                    "malo_id": k.malo_id,
                    "mako_process_id": k.mako_process_id,
                    "abgelehnt_erc": k.abgelehnt_erc,
                    "abgelehnt_reason": k.abgelehnt_reason,
                }));
        }
        // A registration still sitting in the outbound queue has not reached
        // processd at all — a different problem from one the NB has not
        // answered, and the one an operator otherwise cannot see.
        let offene_dispatches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event_outbox o
               JOIN vertragskomponenten k
                 ON starts_with(o.event_id, k.tenant || '/' || o.ce_type || ':' || k.id::text)
              WHERE k.vertrag_id = $1 AND o.kind = 'command'
                AND o.ce_type IN ('LIEFERBEGINN','LIEFERENDE')
                AND o.delivered_at IS NULL AND o.dead_lettered_at IS NULL",
        )
        .bind(id)
        .fetch_one(&self.state.pool)
        .await
        .unwrap_or(0);
        let dead_dispatches: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM event_outbox o
               JOIN vertragskomponenten k
                 ON starts_with(o.event_id, k.tenant || '/' || o.ce_type || ':' || k.id::text)
              WHERE k.vertrag_id = $1 AND o.kind = 'command'
                AND o.dead_lettered_at IS NOT NULL",
        )
        .bind(id)
        .fetch_one(&self.state.pool)
        .await
        .unwrap_or(0);
        ContentBlock::json(serde_json::json!({
            "vertrag_id": id,
            "komponenten_count": komps.len(),
            "abgeleiteter_vertragsstatus": crate::pg::derive_vertrag_status(&komps),
            "by_status": by_status,
            "offene_dispatches": offene_dispatches,
            "dead_lettered_dispatches": dead_dispatches,
            "all_confirmed": komps
                .iter()
                .all(|k| matches!(k.status.as_str(), "AKTIV" | "BESTAETIGT")),
            "any_rejected": komps.iter().any(|k| k.status == "ABGELEHNT"),
            "hinweis": if dead_dispatches > 0 {
                "Aufgegebene Dispatches: GET /api/v1/outbound/dead und dort erneut einstellen."
            } else if offene_dispatches > 0 {
                "Die Anmeldung wartet noch in der Outbound-Queue; processd hat sie noch nicht gesehen."
            } else {
                "Alle Dispatches sind zugestellt; ein ANGEMELDET-Status wartet auf die Antwort des NB."
            },
        }))
        .map(|b| CallToolResult::success(vec![b]))
        .map_err(|e| McpError::internal_error(e.message, None))
    }
}

#[prompt_router]
impl VertragdMcpHandler {
    #[prompt(
        description = "Review open contracts and identify stuck MaKo workflows or expiring Verträge"
    )]
    fn o2c_review(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "Review the open contract fulfillment pipeline and identify any stuck or expiring contracts.",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "1. Use `list_offene_vertraege` to see all contracts IN_BEARBEITUNG or TEILERFUELLUNG.\n\
                 2. For each, use `get_vertrag_status` to see which Vertragskomponenten are stuck.\n\
                 3. Use `list_anmeldungen` for every Lieferstelle still ANGEMELDET, and makod `get_process` for its MaLo — a lapsed deadline there is the escalation; this server states no Frist.\n\
                 4. ABGELEHNT: abgelehnt_erc is the NB's Antwortcode — its meaning is the Entscheidungsbaum the NB applied, not a fixed list here.\n\
                 5. Use `list_expiring_contracts` with days=30 — contracts needing renewal contact.\n\
                 6. Use `list_pending_tarifwechsel` — check preisanpassung_notif_sent=false → 6-week notice not sent.\n\
                 7. B2B customers: use `get_customer_portfolio` for full MaLo/Sparte overview.",
            ),
        ]
    }

    #[prompt(
        description = "B2B customer onboarding: Rahmenvertrag + N Versorgungsverträge + portal access"
    )]
    fn b2b_onboarding(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "How do I onboard a new B2B customer with multiple delivery sites?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**B2B Onboarding Workflow (Rahmenvertrag + N Versorgungsverträge)**\n\n\
                 **Step 1** — Create Kunde (legal entity):\n\
                 `POST /api/v1/kunden` with `kundentyp=B2B_RLM`, `organisations_id`, `umsatzsteuer_id`.\n\n\
                 **Step 2** — Create Rahmenvertrag (portfolio framework):\n\
                 `POST /api/v1/kunden/{id}/rahmenvertraege` with `gueltig_von`, `kuendigungsfrist_monate`, `rechnungsstellung=SAMMEL`.\n\n\
                 **Step 3** — Create Versorgungsvertrag per delivery site:\n\
                 `POST /api/v1/kunden/{id}/vertraege` with `rahmenvertrag_id`, one `komponenten` entry per sparte.\n\
                 → Automatically dispatches GPKE Lieferbeginn (Strom) or GeLi Gas (Gas) to processd.\n\n\
                 **Step 4** — Add portal users:\n\
                 `POST /api/v1/kunden/{id}/identitaeten` for each OIDC user.\n\
                 `standort_filter=Werk Nord` scopes a site manager to matching MaLos — \
                 the one restriction an identity carries.\n\n\
                 **Step 5** — Set Preisgarantie if applicable:\n\
                 `PUT /api/v1/vertraege/{id}/preisgarantie` with BO4E Preisgarantie COM.\n\n\
                 **Monitoring:** `list_anmeldungen` lists what is not confirmed yet; makod `get_process` says whether its deadline has passed.",
            ),
        ]
    }

    #[prompt(
        description = "GDPR Art. 17 right-to-erasure workflow: verify, anonymize, and document customer PII deletion"
    )]
    fn gdpr_erasure_workflow(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "A customer has submitted a DSGVO Art. 17 right-to-erasure request. How do I process it?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**GDPR Art. 17 — Recht auf Löschung (Erasure) Workflow**\n\n\
                 **Step 1 — Verify identity and legal basis**\n\
                 Confirm the request comes from the data subject or their authorized representative.\n\
                 Check retention obligations: § 147 Abs. 3 AO — contracts are Handelsbriefe (6 years) resp. Buchungsbelege (8 years); invoices 8 years (§ 14b UStG).\n\n\
                 **Step 2 — Look up the customer**\n\
                 Use `list_alle_kunden` or `get_kunde_by_sub` to find the kunden_id.\n\
                 Use `get_kunde_gdpr_export` (via REST) to retrieve all PII fields for the record.\n\n\
                 **Step 3 — Check active contracts**\n\
                 Use `get_customer_portfolio` — active contracts must end before erasure.\n\
                 If AKTIV contracts exist: coordinate Kündigung first (§ 20 Abs. 1 StromGVV / GasGVV notice).\n\n\
                 **Step 4 — Anonymize PII**\n\
                 `POST /api/v1/kunden/{id}/anonymize` with `requested_by = operator_sub`\n\
                 This pseudonymizes: geschaeftspartner, person, zahlungsinformation, umsatzsteuer_id, oidc_sub, email.\n\
                 Contract records are RETAINED (§ 147 AO legal basis, up to 8 years).\n\n\
                 **Step 5 — Verify the anonymization_log entry**\n\
                 The immutable `anonymization_log` table records the operator sub, fields anonymized, and timestamp.\n\
                 This satisfies GDPR Art. 5(2) accountability obligation.\n\n\
                 **Documentation**: Print the anonymization_log entry as proof for the data subject and DPA.",
            ),
        ]
    }

    #[prompt(
        description = "Preisgarantie Tarifwechsel conflict resolution: §41 EnWG price-lock bypass workflow"
    )]
    fn preisgarantie_dispute(&self) -> Vec<PromptMessage> {
        vec![
            PromptMessage::new_text(
                Role::User,
                "An operator wants to change the tariff for a contract but it's blocked by a Preisgarantie. What are the options?",
            ),
            PromptMessage::new_text(
                Role::Assistant,
                "**Preisgarantie Tarifwechsel Conflict — §41 EnWG Resolution**\n\n\
                 **Step 1 — Understand the restriction**\n\
                 Use `check_preisgarantie` with the target `wirksamkeit` date.\n\
                 A BLOCKED result means: `wirksamkeit <= preisgarantie_bis` (price-lock window active).\n\n\
                 **Legal basis**: §41 EnWG — the LF is contractually obligated to maintain the agreed price until `preisgarantie_bis`.\n\
                 Breaking a Preisgarantie exposes the LF to customer claims and BNetzA enforcement.\n\n\
                 **Step 2 — Options**\n\n\
                 **Option A: Wait** — schedule the Tarifwechsel for `wirksamkeit > preisgarantie_bis`.\n\
                 This is the legally correct path. Use `store_pending_tarifwechsel` (via API) with future wirksamkeit.\n\
                 The background worker will apply it automatically and emit §5 Abs. 2 StromGVV/GasGVV (6 Wochen) / §41 Abs. 5 EnWG (1 Monat) 42-day notice.\n\n\
                 **Option B: Operator override with customer consent**\n\
                 Only if the customer has explicitly consented (written waiver of price-lock rights).\n\
                 `POST /api/v1/vertraege/{id}/tarifwechsel` with `override_preisgarantie: true`.\n\
                 REQUIRED: Document customer consent BEFORE calling the API.\n\
                 Every override writes to `preisgarantie_override_log` with the operator JWT sub.\n\
                 This log is immutable and may be reviewed by BNetzA.\n\n\
                 **Step 3 — After the change**\n\
                 Verify `list_pending_tarifwechsel` — `preisanpassung_notif_sent` must become true.\n\
                 The §5 Abs. 2 StromGVV/GasGVV (6 Wochen) / §41 Abs. 5 EnWG (1 Monat) 42-day advance notice will fire automatically.",
            ),
        ]
    }
}

#[prompt_handler]
#[tool_handler]
impl ServerHandler for VertragdMcpHandler {
    /// The revisions every mako MCP server speaks, not the SDK's set.
    fn supported_protocol_versions(
        &self,
    ) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }

    fn get_info(&self) -> ServerConfig {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("vertragd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "vertragd MCP — B2B + B2C Contract & Customer Management.\n\
             Manages Kunden, Rahmenverträge (B2B framework), and Versorgungsverträge.\n\n\
             ## Tools (16)\n\
             - `get_vertrag_status` — full contract + components by UUID\n\
             - `list_offene_vertraege` — AKTIV/IN_BEARBEITUNG/TEILERFUELLUNG/GEKÜNDIGT\n\
             - `get_kunde` — customer profile by UUID (MaLo IDs + identities)\n\
             - `get_kunde_by_sub` — OIDC sub → Kunde + active MaLo IDs (portald auth)\n\
             - `get_rahmenvertrag` — B2B framework contract with all child Versorgungsverträge\n\
             - `list_expiring_contracts` — vertragsende/preisgarantie_bis within N days (§41 EnWG)\n\
             - `list_pending_tarifwechsel` — upcoming Tarifwechsel + §5 Abs. 2 StromGVV/GasGVV (6 Wochen) / §41 Abs. 5 EnWG (1 Monat) notification status\n\
             - `list_pending_kuendigungen` — GEKÜNDIGT contracts with future lieferende\n\
             - `check_preisgarantie` — check if Tarifwechsel is blocked for a contract (§41 EnWG)\n\
             - `check_mako_trigger_status` — Lieferbeginn UTILMD dispatch status per contract\n\
             - `list_anmeldungen` — every Lieferstelle still ANGEMELDET; makod `get_process` holds its deadline\n\
             - `get_customer_portfolio` — B2B MaLo/Sparte portfolio overview\n\
             - `get_zahlungsinformation` — SEPA/IBAN payment details for accountingd reconciliation\n\
             - `list_auto_renewal_due` — auto-renewal contracts within N days (§ 309 Nr. 9 lit. b BGB)\n\
             - `list_alle_kunden` — all customers for CRM/ERP sync\n\
             - `compute_kuendigungsfrist` — earliest valid Kündigung date (§ 20 Abs. 1 StromGVV / GasGVV)\n\n\
             ## Prompts (4)\n\
             - `o2c_review` — full O2C pipeline review + stuck/expiring detection\n\
             - `b2b_onboarding` — step-by-step Rahmenvertrag + N Versorgungsverträge workflow\n\
             - `gdpr_erasure_workflow` — GDPR Art. 17 right-to-erasure step-by-step\n\
             - `preisgarantie_dispute` — Preisgarantie Tarifwechsel conflict resolution",
        )
    }
}

async fn mcp_auth_middleware(
    axum::extract::State(state): axum::extract::State<Arc<VertragdMcpState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    state.auth.authenticate(request, next).await
}

pub fn router(state: Arc<VertragdMcpState>, _shutdown: CancellationToken) -> Router {
    let handler = VertragdMcpHandler::new(Arc::clone(&state));
    let service = mako_service::mcp::service(
        move || Ok(handler.clone()),
        StreamableHttpServerConfig::default(),
    );
    Router::new()
        .route_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state, mcp_auth_middleware))
}

#[cfg(test)]
mod tool_contract_tests {
    use super::VertragdMcpHandler;

    /// The served tool contract is the committed `mcp.snapshot.json`
    /// (`BLESS_MCP_SNAPSHOT=1` rewrites it).
    #[test]
    fn the_tool_contract_matches_the_snapshot() {
        let tools: Vec<serde_json::Value> = VertragdMcpHandler::tool_router()
            .list_all()
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .expect("tools serialize");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("mcp.snapshot.json");
        if let Err(e) = mako_service::mcp_snapshot::compare(&tools, &path) {
            panic!("{e}");
        }
    }
}
