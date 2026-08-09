# mako-plugin

**Operator extension point for the mako event bus — the `CloudEventPlugin` trait and its registry.**

A deployment can enrich or annotate every CloudEvent before it is delivered —
adding an operator identifier, tagging events for a downstream ERP, dropping a
field an internal policy forbids — without forking mako.

That is the whole crate: one trait, one registry, one host call-site.

---

## `CloudEventPlugin`

```rust
pub trait CloudEventPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn on_event(&self, ce_type: &str, payload: &mut Value, ctx: &PluginContext)
        -> Result<(), PluginError>;
}
```

The CloudEvents envelope fields (`type`, `source`, `id`, `time`) are present on
entry and must not be renamed or removed — subscribers match on them, and
`marktd`'s fan-out routes on `type`.

**Informatorisches Unbundling:** a plugin registered in an NB-role service must
not copy LF customer data into an enriched event. §6a EnWG applies to operator
extensions exactly as it applies to mako's own code.

---

## `PluginRegistry`

Built during service construction, wrapped in an `Arc`, handed to the event bus.
Plugins run in registration order.

```rust
let mut registry = PluginRegistry::new();
registry.register_cloud_event(Box::new(MyEnricher));

let bus = WebhookBus::new(config).with_plugins(Arc::new(registry));
```

A plugin that returns `Err` is logged and skipped — the event is still
delivered. An operator customisation must not be able to suppress a regulated
market notification.

With no plugin registered the bus checks `is_empty()` first, so a zero-plugin
deployment pays nothing.

---

## `PluginContext`

```rust
pub struct PluginContext {
    /// Operator tenant identifier (the BDEW Marktpartner code).
    pub tenant: String,
    /// Plugin-specific configuration, supplied by the registering daemon.
    pub config: serde_json::Value,
}
```

Read-only. The bus derives `tenant` from the CloudEvent `source`
(`urn:mako:{service}:tenant:{tenant}`).

---

## Scope

Plugins are compiled into the daemon. There is deliberately **no dynamic
loading tier**: mako daemons ship as distroless images built per deployment, so
"rebuild with your plugin" is already the delivery model, and a sandboxed WASM
runtime would add an attack surface and a JIT dependency for a capability the
build step already provides.
