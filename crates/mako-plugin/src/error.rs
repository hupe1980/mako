//! Plugin error type.

/// Error returned by a plugin.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// The plugin rejected the input for a business reason.
    #[error("plugin '{name}' error: {message}")]
    Business {
        /// Plugin name, for the log line.
        name: String,
        /// What went wrong.
        message: String,
    },

    /// The plugin produced a payload that is not valid JSON.
    #[error("plugin '{name}' serialise error: {source}")]
    Serialise {
        /// Plugin name, for the log line.
        name: String,
        #[source]
        source: serde_json::Error,
    },

    /// The plugin was configured with values it cannot use.
    #[error("plugin '{name}' config error: {message}")]
    Config {
        /// Plugin name, for the log line.
        name: String,
        /// What is wrong with the configuration.
        message: String,
    },
}

impl PluginError {
    /// Construct a [`Business`](PluginError::Business) error.
    pub fn business(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Business {
            name: name.into(),
            message: message.into(),
        }
    }
}
