//! Which model answers: the providers the app and the CLI offer, their defaults and clients.

use std::sync::Arc;

use maluos_llm::LlmClient;
use maluos_llm::keys::Provider;
use serde::{Deserialize, Serialize};

/// A provider the user can pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Choice {
    #[default]
    Anthropic,
    OpenAi,
    Ollama,
    /// A client the host supplied (tests, embedding hosts).
    Custom,
}

impl Choice {
    pub const ALL: [Choice; 3] = [Choice::Anthropic, Choice::OpenAi, Choice::Ollama];

    pub fn label(self) -> &'static str {
        match self {
            Choice::Anthropic => "Anthropic",
            Choice::OpenAi => "OpenAI",
            Choice::Ollama => "Ollama",
            Choice::Custom => "Custom",
        }
    }

    /// The name on the command line and in `assistant.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Choice::Anthropic => "anthropic",
            Choice::OpenAi => "openai",
            Choice::Ollama => "ollama",
            Choice::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Choice> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => Choice::Anthropic,
            "openai" | "gpt" => Choice::OpenAi,
            "ollama" | "local" => Choice::Ollama,
            _ => return None,
        })
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Choice::Anthropic => maluos_llm::anthropic::DEFAULT_MODEL,
            Choice::OpenAi => maluos_llm::openai::DEFAULT_MODEL,
            Choice::Ollama => "glm-5.3-flash:cloud",
            Choice::Custom => "custom",
        }
    }

    /// Models offered in a picker besides what the user types.
    pub fn suggestions(self) -> &'static [&'static str] {
        match self {
            Choice::Anthropic => &["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"],
            Choice::OpenAi => &["gpt-5.1", "gpt-5.1-mini"],
            Choice::Ollama => &["glm-5.3-flash:cloud", "llama3.3", "qwen3"],
            Choice::Custom => &[],
        }
    }

    /// Whether the provider can be used now, and where its key comes from (or why not).
    pub fn status(self) -> (bool, String) {
        match self {
            Choice::Anthropic | Choice::OpenAi => {
                let p = if self == Choice::Anthropic { Provider::Anthropic } else { Provider::OpenAi };
                match p.env_vars().iter().find(|v| env_set(v)) {
                    Some(v) => (true, format!("key from {v}")),
                    None => (false, format!("no key: set {}", p.env_vars()[0])),
                }
            }
            Choice::Ollama => (true, format!("{} (no key needed)", ollama_url())),
            Choice::Custom => (true, "custom client".into()),
        }
    }

    /// The native client for this provider.
    #[cfg(all(feature = "native", not(target_arch = "wasm32")))]
    pub fn client(self) -> Result<Arc<dyn LlmClient>, String> {
        match self {
            Choice::Anthropic => maluos_llm::AnthropicClient::from_env().map(|c| Arc::new(c) as Arc<dyn LlmClient>).map_err(|e| e.to_string()),
            Choice::OpenAi => maluos_llm::OpenAiClient::openai_from_env().map(|c| Arc::new(c) as Arc<dyn LlmClient>).map_err(|e| e.to_string()),
            Choice::Ollama => {
                let t = Arc::new(maluos_llm::UreqTransport::default());
                Ok(Arc::new(maluos_llm::OpenAiClient::compatible(t, "ollama", ollama_url(), None)))
            }
            Choice::Custom => Err("no client".into()),
        }
    }

    #[cfg(not(all(feature = "native", not(target_arch = "wasm32"))))]
    pub fn client(self) -> Result<Arc<dyn LlmClient>, String> {
        Err("the assistant needs the desktop build".into())
    }
}

/// The first provider with credentials: Anthropic, then OpenAI, else Ollama (no key).
pub fn default_choice() -> Choice {
    for (choice, p) in [(Choice::Anthropic, Provider::Anthropic), (Choice::OpenAi, Provider::OpenAi)] {
        if p.env_vars().iter().any(|v| env_set(v)) {
            return choice;
        }
    }
    Choice::Ollama
}

/// Ollama's OpenAI-compatible endpoint: `MALUOS_SLIDES_OLLAMA_URL`, else the local default.
pub fn ollama_url() -> String {
    std::env::var("MALUOS_SLIDES_OLLAMA_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| maluos_llm::openai::OLLAMA_BASE_URL.to_string())
}

fn env_set(name: &str) -> bool {
    std::env::var(name).map(|k| !k.trim().is_empty()).unwrap_or(false)
}
