//! [`PluginRegistry`] — the set of plugins a daemon registered at startup.

use tracing::{info, warn};

use crate::{CloudEventPlugin, PluginContext};

/// The plugins a daemon registered at startup.
///
/// Built during service construction, wrapped in an `Arc`, and handed to the
/// event bus. It is immutable once shared.
///
/// ```rust,no_run
/// use std::sync::Arc;
/// use mako_plugin::{PluginRegistry, PluginContext};
///
/// let registry = Arc::new(PluginRegistry::default());
///
/// // What `WebhookBus::publish` does internally:
/// let mut payload = serde_json::json!({"type": "de.mako.process.initiated"});
/// let ctx = PluginContext { tenant: "9910000000001".into(), config: Default::default() };
/// registry.run_cloud_event_plugins("de.mako.process.initiated", &mut payload, &ctx);
/// ```
#[derive(Default)]
pub struct PluginRegistry {
    cloud_event: Vec<Box<dyn CloudEventPlugin>>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plugin. Plugins run in registration order.
    pub fn register_cloud_event(&mut self, plugin: Box<dyn CloudEventPlugin>) -> &mut Self {
        info!(
            plugin = plugin.name(),
            "mako-plugin: registered CloudEventPlugin"
        );
        self.cloud_event.push(plugin);
        self
    }

    /// Run every registered plugin over `payload`.
    ///
    /// A failing plugin is logged and skipped: an operator customisation must
    /// not be able to suppress a regulated market notification.
    pub fn run_cloud_event_plugins(
        &self,
        ce_type: &str,
        payload: &mut serde_json::Value,
        ctx: &PluginContext,
    ) {
        for plugin in &self.cloud_event {
            if let Err(e) = plugin.on_event(ce_type, payload, ctx) {
                warn!(
                    plugin = plugin.name(),
                    ce_type,
                    error = %e,
                    "mako-plugin: CloudEventPlugin failed (event still delivered)"
                );
            }
        }
    }

    /// `true` when no plugin is registered — the fast path the bus checks
    /// before building a [`PluginContext`].
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cloud_event.is_empty()
    }

    /// Number of registered plugins.
    #[must_use]
    pub fn plugin_count(&self) -> usize {
        self.cloud_event.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PluginError;
    use serde_json::{Value, json};

    struct Enricher(&'static str);
    impl CloudEventPlugin for Enricher {
        fn name(&self) -> &str {
            self.0
        }
        fn on_event(
            &self,
            _ce_type: &str,
            payload: &mut Value,
            _ctx: &PluginContext,
        ) -> Result<(), PluginError> {
            payload[self.0] = json!(true);
            Ok(())
        }
    }

    struct Failing;
    impl CloudEventPlugin for Failing {
        fn name(&self) -> &str {
            "failing"
        }
        fn on_event(
            &self,
            _ce_type: &str,
            _payload: &mut Value,
            _ctx: &PluginContext,
        ) -> Result<(), PluginError> {
            Err(PluginError::Business {
                name: "failing".into(),
                message: "nope".into(),
            })
        }
    }

    fn ctx() -> PluginContext {
        PluginContext {
            tenant: "9900357000004".into(),
            config: Value::Null,
        }
    }

    #[test]
    fn empty_registry_is_a_no_op() {
        let reg = PluginRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.plugin_count(), 0);
    }

    #[test]
    fn plugins_run_in_registration_order() {
        let mut reg = PluginRegistry::new();
        reg.register_cloud_event(Box::new(Enricher("first")))
            .register_cloud_event(Box::new(Enricher("second")));

        let mut payload = json!({"type": "de.mako.process.initiated"});
        reg.run_cloud_event_plugins("de.mako.process.initiated", &mut payload, &ctx());

        assert_eq!(payload["first"], json!(true));
        assert_eq!(payload["second"], json!(true));
        assert_eq!(reg.plugin_count(), 2);
    }

    /// A failing plugin must not stop the chain or the delivery.
    #[test]
    fn a_failing_plugin_does_not_block_the_rest() {
        let mut reg = PluginRegistry::new();
        reg.register_cloud_event(Box::new(Failing))
            .register_cloud_event(Box::new(Enricher("after")));

        let mut payload = json!({"type": "de.mako.process.initiated"});
        reg.run_cloud_event_plugins("de.mako.process.initiated", &mut payload, &ctx());

        assert_eq!(payload["after"], json!(true));
    }
}
