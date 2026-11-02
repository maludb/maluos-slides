//! Where API keys come from: an environment variable, never a settings file or a deck. The
//! MaluOS vault (`/run/maluos/vault.sock`) is the next source once its protocol is documented;
//! the lookup is a function parameter so a host can plug it in without touching this crate.

use crate::LlmError;

/// A provider whose key is looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
}

impl Provider {
    /// The variables consulted, most specific first: the MaluOS Slides one, then the provider's
    /// conventional one.
    pub fn env_vars(self) -> [&'static str; 2] {
        match self {
            Provider::Anthropic => ["MALUOS_SLIDES_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"],
            Provider::OpenAi => ["MALUOS_SLIDES_OPENAI_API_KEY", "OPENAI_API_KEY"],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Anthropic => "anthropic",
            Provider::OpenAi => "openai",
        }
    }
}

/// The key for `provider` from the process environment.
pub fn api_key_from_env(provider: Provider) -> Result<String, LlmError> {
    api_key_with(provider, |name| std::env::var(name).ok())
}

/// The key for `provider` from any lookup (a test, a vault client). Empty and blank values count
/// as unset.
pub fn api_key_with(provider: Provider, lookup: impl Fn(&str) -> Option<String>) -> Result<String, LlmError> {
    for name in provider.env_vars() {
        if let Some(value) = lookup(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }
    let [specific, general] = provider.env_vars();
    Err(LlmError::NoApiKey(format!("set {specific} or {general} for {}", provider.name())))
}
