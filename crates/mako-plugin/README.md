# mako-plugin

**Plugin infrastructure for mako daemons — extension-point traits, `PluginRegistry`, and optional WASM plugin loading.**

`mako-plugin` lets operators extend mako's behaviour without forking the core:
add custom billing adjustments, inject additional MCP tools into `agentd`,
enrich CloudEvents, or add operator-specific INVOIC validation rules.

Two integration modes are supported:

| Mode | How | Overhead |
|---|---|---|
| **Native Rust** | Implement the trait, register at startup | Zero — single virtual call |
| **WASM** | Compile any language to `.wasm`, load at startup | ~1 MB binary + Wasmtime JIT |

---

## Extension-point traits

### `CloudEventPlugin`

Enrich or filter CloudEvents before they are delivered by the EventBus:

```rust
pub trait CloudEventPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn on_event(&self, ce_type: &str, payload: &mut Value, ctx: &PluginContext)
        -> Result<(), PluginError>;
}
```

**Informatorisches Unbundling:** plugins in NB-role services must not include
LF customer data in enriched events.

---

### `McpToolPlugin`

Add custom MCP tools to the `agentd` LLM agent:

```rust
pub trait McpToolPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn list_tools(&self) -> Vec<McpPluginTool>;
    fn call_tool(&self, tool_name: &str, args: Value, ctx: &PluginContext)
        -> Pin<Box<dyn Future<Output = Result<Value, PluginError>> + Send + '_>>;
}
```

`McpPluginTool` contains `name`, `description`, and `input_schema` (JSON Schema).
Tools are merged with the built-in mako MCP tool list at `agentd` startup.

---

### `BillingPlugin`

Adjust `billingd` positions after the standard calculation:

```rust
pub trait BillingPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn adjust_positions(&self, malo_id: &str, positions: &mut Vec<BillingPosition>,
                        ctx: &PluginContext) -> Result<(), PluginError>;
}
```

Plugins can add discount lines, promotional credits, or §14a Steuerungsrabatt
overrides. Must not modify positions with names starting with `NNE_` or `KA_`
(pass-through grid costs are protected).

---

### `ValidatorPlugin`

Add operator-specific INVOIC plausibility rules to `invoic-checker`:

```rust
pub trait ValidatorPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn validate(&self, rechnung: &Value, ctx: &PluginContext)
        -> Result<Vec<ValidationIssue>, PluginError>;
}
```

`Error`-severity issues trigger a `Dispute` outcome (REMADV 33002).
`Warning` produces auto-accepted `Warn`; `Info` is logged only.

---

### `WebhookPlugin`

Enrich outbound CloudEvent webhook requests before they are sent:

```rust
pub trait WebhookPlugin: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn enrich_request(&self, url: &str, ce_type: &str,
                      headers: &mut HashMap<String, String>,
                      ctx: &PluginContext) -> Result<(), PluginError>;
}
```

Use cases: custom `X-Operator-Signature` headers, outbound event logging,
routing to different webhook URLs per event type.

---

## `PluginRegistry`

All plugins are registered in a `PluginRegistry` at daemon startup:

```rust
let mut registry = PluginRegistry::new();

// Native Rust plugin
registry.register_billing(Arc::new(MyDiscountPlugin));
registry.register_mcp_tool(Arc::new(MyCustomTools));

// WASM plugin (requires feature `wasm`)
#[cfg(feature = "wasm")]
registry.load_wasm("plugins/operator_rules.wasm")?;

// Registry is cloned into the service state
let app = App::new().data(registry);
```

---

## WASM plugins

Enable the `wasm` feature for Extism/Wasmtime-backed WASM loading:

```toml
[dependencies]
mako-plugin = { path = "../crates/mako-plugin", features = ["wasm"] }
```

WASM plugins are sandboxed: they can only call host functions explicitly exposed
via Extism's plugin development kit (PDK). They cannot access the filesystem,
network, or other mako services directly.

WASM plugins can be written in any WASM-targeting language:
Rust, Go, TypeScript, Python, C/C++, Zig, etc.

---

## `PluginContext`

Every plugin call receives a read-only `PluginContext`:

```rust
pub struct PluginContext {
    pub tenant: String,
    pub operator_mp_id: String,
    pub format_version: String,
    pub extra: HashMap<String, Value>,
}
```

Plugins receive context but cannot modify it, and cannot call back into the host
outside of explicitly exposed host functions.
