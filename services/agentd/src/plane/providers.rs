//! Model drivers, built from `[providers.*]`.
//!
//! Two names are in play and keeping them apart is the whole design of this
//! module. The **table key** is what a manifest's `spec.models` refers to; the
//! `backend` field is which wire that driver speaks. So a deployment may
//! register `[providers.anthropic]` backed by `chat-completions` against its own
//! vLLM, and not one of the 28 manifests changes — which matters because a
//! manifest edit is a digest change and a review.
//!
//! ## Why the self-hosted wire is not an afterthought
//!
//! `chat-completions` reaches TGI, vLLM, Ollama and llama.cpp. For a German
//! utility that is the difference between an easy conversation with a data
//! protection officer and a hard one: customer data reaching a third-party
//! inference endpoint is an Art. 28 / Art. 44 DSGVO question, and *the endpoint
//! is ours* answers it without a contract.
//!
//! ## An unknown backend refuses to start
//!
//! A `[providers.x] backend = "opnai"` that logged a warning and carried on
//! would present as every run of every manifest naming `x` failing at its first
//! model call — the same wiring mistake reported once per request instead of
//! once.

use std::sync::Arc;

use agentplane::model::ModelProvider;
use secrecy::ExposeSecret as _;

use crate::config::ProviderConfig;

/// Build one driver.
///
/// # Errors
///
/// When the backend name is unknown, a required field is missing, or the driver
/// itself refuses to construct. All three are deployment faults.
pub async fn build(name: &str, cfg: &ProviderConfig) -> Result<Arc<dyn ModelProvider>, String> {
    let key = cfg.api_key.expose_secret();
    let fail = |what: &str| format!("providers.{name}: {what}");

    match cfg.backend.as_str() {
        "anthropic" => {
            let mut d = agentplane::model::anthropic::Anthropic::new(require_key(key, name)?)
                .map_err(|e| fail(&e.to_string()))?;
            if let Some(base) = &cfg.api_base {
                d = d.base(base.clone());
            }
            Ok(Arc::new(d) as Arc<dyn ModelProvider>)
        }
        "openai" => {
            let mut d = agentplane::model::openai::OpenAi::new(require_key(key, name)?)
                .map_err(|e| fail(&e.to_string()))?;
            if let Some(base) = &cfg.api_base {
                d = d.base(base.clone());
            }
            Ok(Arc::new(d) as Arc<dyn ModelProvider>)
        }
        "gemini" => {
            let mut d = agentplane::model::gemini::Gemini::new(require_key(key, name)?)
                .map_err(|e| fail(&e.to_string()))?;
            if let Some(base) = &cfg.api_base {
                d = d.base(base.clone());
            }
            Ok(Arc::new(d) as Arc<dyn ModelProvider>)
        }
        // The OpenAI-compatible wire, pointed at something you run. `api_base`
        // is required rather than defaulted: there is no sensible default for
        // "your own server", and guessing one would send prompts somewhere.
        "chat-completions" => {
            let base = cfg.api_base.as_ref().ok_or_else(|| {
                fail("chat-completions needs `api_base` — the endpoint you run it on")
            })?;
            let mut d = agentplane::model::chat_completions::ChatCompletions::new(base.clone())
                .map_err(|e| fail(&e.to_string()))?;
            // A local server usually wants no bearer at all; a hosted router
            // does. Empty means none rather than an empty `Authorization`.
            if !key.is_empty() {
                d = d.bearer(key.to_owned());
            }
            Ok(Arc::new(d) as Arc<dyn ModelProvider>)
        }
        #[cfg(feature = "bedrock")]
        "bedrock" => {
            let region = cfg
                .aws_region
                .as_ref()
                .ok_or_else(|| fail("bedrock needs `aws_region`"))?;
            // Credentials come from the standard AWS chain — IAM role,
            // environment, profile — and deliberately never from agentd.toml.
            let d = agentplane::model::bedrock::Bedrock::from_env(region.clone())
                .await
                .map_err(|e| fail(&e))?;
            Ok(Arc::new(d) as Arc<dyn ModelProvider>)
        }
        #[cfg(not(feature = "bedrock"))]
        "bedrock" => Err(fail(
            "this binary was built without the `bedrock` feature — rebuild agentd with \
             `--features bedrock`, which pulls in the AWS SDK",
        )),
        other => Err(fail(&format!(
            "unknown backend `{other}` — expected one of: anthropic, openai, gemini, \
             chat-completions, bedrock"
        ))),
    }
}

/// An API key that is actually present.
///
/// Refused rather than defaulted to the empty string: an empty bearer is a 401
/// per model call, discovered as "the agent is broken" rather than as "the key
/// is missing".
fn require_key<'a>(key: &'a str, name: &str) -> Result<&'a str, String> {
    if key.is_empty() {
        return Err(format!(
            "providers.{name}: no api_key configured (use `api_key = \"env:VAR\"` to read \
             one from the environment)"
        ));
    }
    Ok(key)
}

/// Build every configured driver.
///
/// # Errors
///
/// The first driver that cannot be built, naming it.
pub async fn build_all(
    providers: &std::collections::HashMap<String, ProviderConfig>,
) -> Result<Vec<(String, Arc<dyn ModelProvider>)>, String> {
    let mut built = Vec::with_capacity(providers.len());
    for (name, cfg) in providers {
        built.push((name.clone(), build(name, cfg).await?));
    }
    if built.is_empty() {
        return Err(
            "no model provider configured. agentd cannot run an agent without one — \
                    declare at least one [providers.<name>] matching a manifest's `spec.models`."
                .to_owned(),
        );
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    fn cfg(backend: &str, key: &str, base: Option<&str>) -> ProviderConfig {
        ProviderConfig {
            backend: backend.to_owned(),
            api_base: base.map(str::to_owned),
            api_key: SecretString::from(key.to_owned()),
            aws_region: None,
        }
    }

    /// Each shipped backend name builds a driver.
    ///
    /// The failure this prevents is a rename in agentplane that leaves a branch
    /// here matching a name nothing produces — discovered at deployment.
    #[tokio::test]
    async fn every_documented_backend_builds() {
        for (backend, base) in [
            ("anthropic", None),
            ("openai", None),
            ("gemini", None),
            ("chat-completions", Some("http://localhost:8000/v1")),
        ] {
            build(backend, &cfg(backend, "k", base))
                .await
                .unwrap_or_else(|e| panic!("{backend} should build: {e}"));
        }
    }

    /// A typo in `backend` is a startup failure that names the alternatives.
    #[tokio::test]
    async fn an_unknown_backend_is_refused() {
        let err = build("x", &cfg("opnai", "k", None))
            .await
            .expect_err("unknown backend");
        assert!(err.contains("unknown backend"), "{err}");
        assert!(
            err.contains("chat-completions"),
            "it lists the options: {err}"
        );
    }

    /// A hosted driver with no key is refused rather than left to 401.
    #[tokio::test]
    async fn a_missing_api_key_is_refused() {
        let err = build("openai", &cfg("openai", "", None))
            .await
            .expect_err("no key");
        assert!(err.contains("api_key"), "{err}");
    }

    /// The self-hosted wire needs somewhere to go, and says so.
    #[tokio::test]
    async fn chat_completions_without_a_base_is_refused() {
        let err = build("local", &cfg("chat-completions", "", None))
            .await
            .expect_err("no base");
        assert!(err.contains("api_base"), "{err}");
    }

    /// A local endpoint may have no key at all — that is not an error.
    #[tokio::test]
    async fn a_keyless_local_endpoint_is_allowed() {
        build(
            "local",
            &cfg("chat-completions", "", Some("http://ollama:11434/v1")),
        )
        .await
        .expect("a local server needs no bearer");
    }

    /// No providers at all is refused, with the reason.
    #[tokio::test]
    async fn no_providers_is_a_startup_failure() {
        let err = build_all(&std::collections::HashMap::new())
            .await
            .expect_err("no providers");
        assert!(err.contains("no model provider"), "{err}");
    }
}
