//! The extension-point trait.

use serde_json::Value;

use crate::{PluginContext, PluginError};

/// Enriches or annotates CloudEvents before the event bus delivers them.
///
/// Called once per `EventBus::publish()` for every registered plugin, in
/// registration order. A plugin that returns `Err(_)` is logged and skipped;
/// the event is still delivered.
pub trait CloudEventPlugin: Send + Sync + 'static {
    /// Unique plugin name, used in log lines.
    fn name(&self) -> &str;

    /// Mutate `payload` in place to add or remove fields.
    ///
    /// The CloudEvents envelope fields (`type`, `source`, `id`, `time`) are
    /// present on entry and must not be renamed or removed — subscribers match
    /// on them, and `marktd`'s fan-out routes on `type`.
    fn on_event(
        &self,
        ce_type: &str,
        payload: &mut Value,
        ctx: &PluginContext,
    ) -> Result<(), PluginError>;
}
