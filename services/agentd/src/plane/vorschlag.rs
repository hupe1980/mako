//! The proposal that is waiting on every decision held for a person.
//!
//! A decision held for a person is answered from the same reads every time
//! somebody asks the desk assistant „Was schlägst du vor?". Asking once per
//! decision instead of once per look costs one model run per entry, and the
//! person starts from a checked answer rather than an empty form.
//!
//! Two kinds of decision get one ([`Art`]): processd's Freigaben and
//! invoicd's Rechnungsdispute. The tick reads each kind's open list and admits
//! one desk-assistant run per entry ([`Plane::vorschlag_zulassen`]). It is a
//! pull, not an event: the owning service's list is already the durable
//! record, so an entry a tick missed is admitted by the next one, and
//! admission keyed on the kind and the entry id makes any number of ticks — on
//! any number of instances — one run. The run sends nothing: the proposal is
//! prefilled in the Desk, and the send is the person's.
//!
//! billingd's Prüfposten get none: releasing an invoice the risk gate held
//! overrides a deterministic check on money, and a model's reading of the
//! same findings adds no evidence the gate did not already weigh.

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

/// Entries read per tick; a saturated page is reported, and the next tick
/// admits what this one did not reach.
const SEITE: u32 = 200;

/// What a waiting proposal decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Art {
    /// A processd Freigabe: approve or refuse the held answer.
    Freigabe,
    /// An invoicd Rechnungsdisput: close it, or let the refusal stand.
    Disput,
}

impl Art {
    /// Every kind, in the order the tick reads them.
    pub const ALLE: [Self; 2] = [Self::Freigabe, Self::Disput];

    /// The Anker key: the run input's field, and the `vorschlaege` query's.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Freigabe => "freigabe",
            Self::Disput => "disput",
        }
    }

    /// The MCP server whose list holds the open entries.
    #[must_use]
    pub const fn server(self) -> &'static str {
        match self {
            Self::Freigabe => "processd",
            Self::Disput => "invoicd",
        }
    }

    /// The actions a proposal of this kind may name — the Desk's own for it.
    #[must_use]
    pub const fn aktionen(self) -> &'static [&'static str] {
        match self {
            Self::Freigabe => &["freigeben", "ablehnen"],
            Self::Disput => &["beilegen", "aufrechterhalten"],
        }
    }

    /// The kind whose proposal names `aktion`.
    #[must_use]
    pub fn von_aktion(aktion: &str) -> Option<Self> {
        Self::ALLE
            .into_iter()
            .find(|a| a.aktionen().contains(&aktion))
    }

    /// The listing tool, its arguments, and the field its entries are under.
    fn liste(self) -> (&'static str, Value, &'static str) {
        match self {
            Self::Freigabe => (
                "list_pending_approvals",
                serde_json::json!({ "limit": SEITE }),
                "freigaben",
            ),
            Self::Disput => ("list_disputes", serde_json::json!({}), "disputes"),
        }
    }
}

/// The admission event id of one entry's proposal.
#[must_use]
pub fn ereignis(art: Art, id: &str) -> String {
    format!("vorschlag:{}:{id}", art.key())
}

/// The entry ids of a listing of `art`, and whether the page was full.
#[must_use]
pub fn eintraege(art: Art, output: &Value) -> (Vec<String>, bool) {
    let (_, _, feld) = art.liste();
    let ids = output[feld]
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

/// One tick over one kind: admit a proposal run for every open entry that has
/// none. Returns how many runs were freshly admitted.
///
/// # Errors
///
/// The owning service's tool failing, or the desk-assistant not activated.
pub async fn tick(plane: &Plane, art: Art, client: &dyn ToolClient) -> Result<usize, String> {
    let (liste, args, _) = art.liste();
    let tool = ToolId::new(art.server(), liste);
    let output = client
        .call(&tool, &args, None)
        .await
        .map_err(|e| e.to_string())?;
    let (ids, saturated) = eintraege(art, &output);
    if saturated {
        warn!(
            art = art.key(),
            read = ids.len(),
            "agentd vorschlaege: more entries are open than one tick reads — the rest are \
             admitted on the next ticks"
        );
    }
    let mut fresh = 0;
    for id in &ids {
        match plane.vorschlag_zulassen(art, id).await {
            Some(a) if a.fresh => fresh += 1,
            Some(a) if a.refused.is_some() => warn!(
                art = art.key(),
                entry = %id,
                refused = a.refused.as_deref().unwrap_or_default(),
                "agentd vorschlaege: the proposal run was not admitted"
            ),
            Some(_) => {}
            None => return Err(format!("{SKILL} is not activated")),
        }
    }
    Ok(fresh)
}

/// Run the tick until shutdown, over every kind whose service is configured.
pub fn spawn(
    state: Arc<crate::handlers::AppState>,
    quellen: Vec<(Art, Arc<dyn ToolClient>)>,
    every: Duration,
    shutdown: CancellationToken,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(every);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => {
                    // One kind failing leaves the others' proposals coming.
                    for (art, client) in &quellen {
                        match tick(&state.plane, *art, client.as_ref()).await {
                            Ok(0) => {}
                            Ok(n) => info!(art = art.key(), admitted = n, "agentd vorschlaege: proposal runs admitted"),
                            Err(e) => warn!(art = art.key(), error = %e, "agentd vorschlaege: tick failed"),
                        }
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_ids_are_read_off_each_kinds_listing() {
        let out = serde_json::json!({
            "freigaben": [{ "id": "a" }, { "id": "" }, { "id": "b" }, {}],
            "count": 4,
            "saturated": true,
        });
        assert_eq!(
            eintraege(Art::Freigabe, &out),
            (vec!["a".to_owned(), "b".to_owned()], true)
        );
        assert_eq!(eintraege(Art::Disput, &out), (vec![], true));
        let disputes = serde_json::json!({ "disputes": [{ "id": "d1" }], "saturated": false });
        assert_eq!(
            eintraege(Art::Disput, &disputes),
            (vec!["d1".to_owned()], false)
        );
        assert_eq!(
            eintraege(Art::Freigabe, &serde_json::json!({})),
            (vec![], false)
        );
    }

    /// One entry is one admission event, and a Freigabe and a Disput that
    /// happen to share an id are two.
    #[test]
    fn one_entry_of_one_kind_is_one_admission_event() {
        assert_eq!(ereignis(Art::Freigabe, "e1"), ereignis(Art::Freigabe, "e1"));
        assert_ne!(ereignis(Art::Freigabe, "e1"), ereignis(Art::Freigabe, "e2"));
        assert_ne!(ereignis(Art::Freigabe, "e1"), ereignis(Art::Disput, "e1"));
    }

    /// The two kinds name disjoint actions, so a proposal's action says which
    /// decision it is for.
    #[test]
    fn each_action_belongs_to_one_kind() {
        for art in Art::ALLE {
            for a in art.aktionen() {
                assert_eq!(Art::von_aktion(a), Some(art), "{a}");
            }
        }
        assert_eq!(Art::von_aktion("senden"), None);
    }
}
