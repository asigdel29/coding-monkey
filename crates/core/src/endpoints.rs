/*
   File: crates/core/src/endpoints.rs

   Purpose
   Resolve a [`Provider`] to its HTTP endpoint and API key. Centralizing
   this here (rather than re-deriving it in every LLM client) is what makes
   self-hosting a one-line change: a `SelfHosted` provider points at any
   OpenAI-compatible server (Ollama, llama.cpp, vLLM, LM Studio) via an
   environment-supplied base URL, with an optional key.

   History
   Date         Author          Changes
   2026-06-09   Anubhav Sigdel  initial — provider endpoint resolution
*/

use crate::models::{ModelSpec, Provider};

/// Environment variable holding a self-hosted endpoint's base or full URL.
pub const SELF_HOSTED_URL_ENV: &str = "MONKEY_SELF_HOSTED_URL";
/// Environment variable holding a self-hosted endpoint's API key (optional;
/// many local servers need none).
pub const SELF_HOSTED_KEY_ENV: &str = "MONKEY_SELF_HOSTED_KEY";

/// A resolved provider endpoint: where to POST, the key to use (if any),
/// and whether a key is mandatory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderWire {
    /// Full chat-completions URL.
    pub url: String,
    /// API key value, if one is set.
    pub key: Option<String>,
    /// Whether a key is required (hosted providers) or optional (self-hosted).
    pub key_required: bool,
}

/// Resolve `provider` to its endpoint and key, reading the environment.
///
/// Hosted providers return a fixed URL and read their key env var (the key
/// may be absent — callers check `key_required`). `SelfHosted` reads
/// [`SELF_HOSTED_URL_ENV`] (an error if unset) and an optional key from
/// [`SELF_HOSTED_KEY_ENV`].
pub fn provider_wire(provider: Provider) -> Result<ProviderWire, String> {
    match provider {
        Provider::OpenRouter => Ok(ProviderWire {
            url: "https://openrouter.ai/api/v1/chat/completions".into(),
            key: std::env::var("OPENROUTER_API_KEY").ok(),
            key_required: true,
        }),
        Provider::Openai => Ok(ProviderWire {
            url: "https://api.openai.com/v1/chat/completions".into(),
            key: std::env::var("OPENAI_API_KEY").ok(),
            key_required: true,
        }),
        Provider::SelfHosted => {
            let raw = std::env::var(SELF_HOSTED_URL_ENV)
                .map_err(|_| format!("{SELF_HOSTED_URL_ENV} not set"))?;
            Ok(ProviderWire {
                url: normalize_self_hosted_url(&raw),
                key: std::env::var(SELF_HOSTED_KEY_ENV).ok(),
                key_required: false,
            })
        }
    }
}

/// Resolve a *model's* endpoint and key, preferring a per-model override.
///
/// When `model.base_url` is set, the request targets that endpoint (normalized
/// like a self-hosted URL) and reads an optional key from the env var named by
/// `model.api_key_env`; the key is never required, matching local servers that
/// take none. This is what lets several [`Provider::SelfHosted`] models point
/// at different hosts. When `base_url` is `None`, resolution falls back to the
/// provider-wide [`provider_wire`], so existing models behave exactly as before.
pub fn model_wire(model: &ModelSpec) -> Result<ProviderWire, String> {
    match &model.base_url {
        Some(raw) => Ok(ProviderWire {
            url: normalize_self_hosted_url(raw),
            key: model
                .api_key_env
                .as_ref()
                .and_then(|v| std::env::var(v).ok()),
            key_required: false,
        }),
        None => provider_wire(model.provider),
    }
}

/// Accept either a full chat-completions URL or a base URL and produce the
/// full endpoint, so users can paste whatever their server prints (e.g.
/// `http://localhost:11434` or `http://localhost:11434/v1`).
pub fn normalize_self_hosted_url(url: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    if u.ends_with("/chat/completions") {
        u.to_string()
    } else if u.ends_with("/v1") {
        format!("{u}/chat/completions")
    } else {
        format!("{u}/v1/chat/completions")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_base_and_full_urls() {
        assert_eq!(
            normalize_self_hosted_url("http://localhost:11434"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            normalize_self_hosted_url("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            normalize_self_hosted_url("http://host/v1/chat/completions"),
            "http://host/v1/chat/completions"
        );
    }

    #[test]
    fn hosted_providers_have_fixed_urls_and_require_keys() {
        let w = provider_wire(Provider::OpenRouter).unwrap();
        assert!(w.url.contains("openrouter.ai"));
        assert!(w.key_required);
        let w = provider_wire(Provider::Openai).unwrap();
        assert!(w.url.contains("api.openai.com"));
        assert!(w.key_required);
    }

    fn spec(base_url: Option<&str>, api_key_env: Option<&str>) -> ModelSpec {
        ModelSpec {
            id: "local-test".into(),
            display_name: "Local Test".into(),
            provider: Provider::OpenRouter,
            tier: crate::models::ModelTier::Balanced,
            input_cost_per_1k: 0.0,
            output_cost_per_1k: 0.0,
            context_window: 8_192,
            base_url: base_url.map(str::to_string),
            api_key_env: api_key_env.map(str::to_string),
        }
    }

    #[test]
    fn model_base_url_overrides_provider() {
        // A per-model base URL is normalized and used, with no key required,
        // regardless of the model's nominal provider.
        let w = model_wire(&spec(Some("http://lan-box:8000"), None)).unwrap();
        assert_eq!(w.url, "http://lan-box:8000/v1/chat/completions");
        assert!(!w.key_required);
        assert!(w.key.is_none());
    }

    #[test]
    fn model_without_base_url_falls_back_to_provider() {
        // No override → identical to provider-wide resolution.
        let m = spec(None, None);
        assert_eq!(model_wire(&m).unwrap(), provider_wire(m.provider).unwrap());
    }

    #[test]
    fn self_hosted_errors_without_url_env() {
        // Only meaningful when the env var is unset; tolerate either state so
        // the test is not order-dependent with other env-mutating tests.
        match std::env::var(SELF_HOSTED_URL_ENV) {
            Err(_) => assert!(provider_wire(Provider::SelfHosted).is_err()),
            Ok(_) => assert!(provider_wire(Provider::SelfHosted).is_ok()),
        }
    }
}
