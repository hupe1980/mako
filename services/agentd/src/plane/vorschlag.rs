//! The proposal that is waiting on every Freigabe.
//!
//! A decision held for a person is answered from the same reads every time
//! somebody asks the desk assistant „Was schlägst du vor?". Asking once per
//! Freigabe instead of once per look costs one model run per entry, and the
//! person starts from a checked answer rather than an empty form.
//!
//! The tick reads processd's open Freigaben and admits one desk-assistant run
//! per entry ([`Plane::vorschlag_zulassen`]). It is a pull, not an event:
//! processd's queue is already the durable record, so an entry a tick missed
//! is admitted by the next one, and admission keyed on the entry id makes any
//! number of ticks — on any number of instances — one run. The run sends
//! nothing: the proposal is prefilled in the Desk, and the send is the
//! person's.

use std::sync::Arc;
use std::time::Duration;

use agentplane::tools::{ToolClient, ToolId};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::Plane;

/// The skill that proposes.
pub const SKILL: &str = "desk-assistant";

/// The question the proposal run is asked — the manifest proposes only when
/// asked what to do.
pub const FRAGE: &str = "Was schlägst du vor?";

/// The tool listing the open Freigaben, on the `processd` server.
const LISTE: &str = "list_pending_approvals";

/// Entries read per tick; a saturated page is reported, and the next tick
/// admits what this one did not reach.
const SEITE: u32 = 200;

/// The admission event id of one Freigabe's proposal.
#[must_use]
pub fn ereignis(freigabe: &str) -> String {
    format!("vorschlag:{freigabe}")
}

/// The entry ids of a `list_pending_approvals` result, and whether the page
/// was full.
#[must_use]
pub fn eintraege(output: &Value) -> (Vec<String>, bool) {
    let ids = output["freigaben"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| f["id"].as_str())
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    (ids, output["saturated"].as_bool().unwrap_or(false))
}

/// One tick: admit a proposal run for every open Freigabe that has none.
/// Returns how many runs were freshly admitted.
///
/// # Errors
///
/// processd's tool failing or answering something that is not a list.
pub async fn tick(plane: &Plane, processd: &dyn ToolClient) -> Result<usize, String> {
    let tool = ToolId::new("processd", LISTE);
    let output = processd
        .call(&tool, &serde_json::json!({ "limit": SEITE }), None)
        .await
        .map_err(|e| e.to_string())?;
    let (ids, saturated) = eintraege(&output);
    if saturated {
        warn!(
            read = ids.len(),
            "agentd vorschlaege: more Freigaben are open than one tick reads — the rest are \
             admitted on the next ticks"
        );
    }
    let mut fresh = 0;
    for id in &ids {
        match plane.vorschlag_zulassen(id).await {
            Some(a) if a.fresh => fresh += 1,
            Some(a) if a.refused.is_some() => warn!(
                freigabe = %id,
                refused = a.refused.as_deref().unwrap_or_default(),
                "agentd vorschlaege: the proposal run was not admitted"
            ),
            Some(_) => {}
            None => return Err(format!("{SKILL} is not activated")),
        }
    }
    Ok(fresh)
}

/// Run the tick until shutdown.
pub fn spawn(
    state: Arc<crate::handlers::AppState>,
    processd: Arc<dyn ToolClient>,
    every: Duration,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => match tick(&state.plane, processd.as_ref()).await {
                    Ok(0) => {}
                    Ok(n) => info!(admitted = n, "agentd vorschlaege: proposal runs admitted"),
                    Err(e) => warn!(error = %e, "agentd vorschlaege: tick failed"),
                },
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_ids_are_read_off_the_listing() {
        let out = serde_json::json!({
            "freigaben": [{ "id": "a" }, { "id": "" }, { "id": "b" }, {}],
            "count": 4,
            "saturated": true,
        });
        assert_eq!(
            eintraege(&out),
            (vec!["a".to_owned(), "b".to_owned()], true)
        );
        assert_eq!(eintraege(&serde_json::json!({})), (vec![], false));
    }

    #[test]
    fn one_freigabe_is_one_admission_event() {
        assert_eq!(ereignis("e1"), ereignis("e1"));
        assert_ne!(ereignis("e1"), ereignis("e2"));
    }
}
