//! Plugin error type.

/// Error returned by any plugin extension point.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// The plugin returned a business-logic failure (not a crash).
    #[error("plugin '{name}' error: {message}")]
    Business { name: String, message: String },

    /// JSON serialisation / deserialisation failed at the plugin boundary.
    #[error("plugin '{name}' serialise error: {source}")]
    Serialise {
        name: String,
        #[source]
        source: serde_json::Error,
    },

    /// WASM plugin panicked or trapped.
    #[error("plugin '{name}' wasm trap: {message}")]
    WasmTrap { name: String, message: String },

    /// Plugin configuration is invalid.
    #[error("plugin '{name}' config error: {message}")]
    Config { name: String, message: String },
}

impl PluginError {
    pub fn business(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Business {
            name: name.into(),
            message: message.into(),
        }
    }
}
