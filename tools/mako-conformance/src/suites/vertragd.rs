//! `vertragd`: the contract system a Stadtwerk's ERP most often replaces.

use serde_json::json;

use crate::suite::{BoxFuture, Ctx, Inbound, Refusal, Suite};

/// The contract service's suite.
#[derive(Debug, Clone, Copy)]
pub struct Vertragd;

impl Suite for Vertragd {
    fn service(&self) -> &'static str {
        "vertragd"
    }

    fn definition(&self) -> &'static str {
        include_str!("vertragd.rs")
    }

    /// Every `de.mako.vertrag.*` type: vertragd is their only emitter.
    fn emits(&self) -> Vec<&'static str> {
        mako_events::all()
            .iter()
            .copied()
            .filter(|t| t.starts_with("de.mako.vertrag."))
            .collect()
    }

    /// makod's process events, relayed by marktd. A completion for a process
    /// no contract waits on changes nothing.
    fn inbound(&self) -> Vec<Inbound> {
        vec![Inbound {
            path: "/api/v1/events",
            event: json!({
                "specversion": "1.0",
                "id": "conformance-inbound-1",
                "source": "urn:mako:makod:tenant:conformance",
                "type": mako_events::mako::PROCESS_COMPLETED,
                "subject": "00000000-0000-4000-8000-00000000c0de",
                "data": { "accepted": true, "malo_id": "51238696012" },
            }),
        }]
    }

    /// A contract read, refused to a principal without a supplier role.
    fn refusal(&self) -> Refusal {
        Refusal {
            path: "/api/v1/vertraege/expiring",
            tool: "list_expiring_contracts",
            arguments: json!({}),
        }
    }

    /// A Kunde and an Ersatzversorgung contract: the contract announces
    /// `de.mako.vertrag.ersatzversorgung.begonnen`, a typed event.
    fn seed<'a>(&'a self, ctx: &'a Ctx) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let kunde = ctx
                .operator(
                    "POST",
                    "/api/v1/kunden",
                    Some(&json!({ "kundentyp": "B2C" })),
                )
                .await?;
            let id = kunde.body["kunden_id"].as_str().ok_or_else(|| {
                anyhow::anyhow!(
                    "POST /api/v1/kunden answered {}: {}",
                    kunde.status,
                    kunde.body
                )
            })?;
            let heute = mako_fristen::heute();
            let ende = heute + time::Duration::days(60);
            let vertrag = ctx
                .operator(
                    "POST",
                    &format!("/api/v1/kunden/{id}/vertraege"),
                    Some(&json!({
                        "kundentyp": "B2C",
                        "vertragsart": "ERSATZVERSORGUNG",
                        "vertragsbeginn": heute.to_string(),
                        "vertragsende": ende.to_string(),
                        "komponenten": [{
                            "sparte": "STROM",
                            "malo_id": "51238696012",
                            "nb_mp_id": "9900000000001",
                            "product_code": "STROM-ERSATZ",
                            "lieferbeginn": heute.to_string(),
                            "lieferende": ende.to_string(),
                        }],
                    })),
                )
                .await?;
            anyhow::ensure!(
                vertrag.status == 201,
                "POST /api/v1/kunden/{id}/vertraege answered {}: {}",
                vertrag.status,
                vertrag.body
            );
            Ok(())
        })
    }
}
