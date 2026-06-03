//! AI assistant provider configuration.

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Anthropic-specific knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ClaudeAiConfig {
    /// Anthropic API key. **Never serialized to `config.toml`** — the key is
    /// persisted in the OS keychain (id [`crate::secrets::AI_CLAUDE_KEY_ID`])
    /// and hydrated into this in-memory field at load. Legacy plaintext
    /// values still *deserialize* and are migrated to the keychain on the
    /// next save. Empty = fall back to `$ANTHROPIC_API_KEY` at request time.
    #[serde(skip_serializing)]
    pub api_key: String,
    /// Default model name.
    pub model: String,
    /// Soft cap on output tokens per request.
    pub max_tokens: u32,
}

impl Default for ClaudeAiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "claude-opus-4-7".into(),
            max_tokens: 4096,
        }
    }
}

/// OpenAI-specific knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct OpenAiAiConfig {
    /// OpenAI API key. **Never serialized to `config.toml`** — persisted in
    /// the OS keychain (id [`crate::secrets::AI_OPENAI_KEY_ID`]) exactly like
    /// the Claude key above. Empty = fall back to `$OPENAI_API_KEY`.
    #[serde(skip_serializing)]
    pub api_key: String,
    /// Default model name.
    pub model: String,
    /// Soft cap on output tokens per request.
    pub max_tokens: u32,
}

impl Default for OpenAiAiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "gpt-4o".into(),
            max_tokens: 4096,
        }
    }
}

/// Ollama-specific knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct OllamaAiConfig {
    /// Daemon endpoint (defaults to the standard localhost port).
    pub url: String,
    /// Default model name (e.g. `"llama3.1"`).
    pub model: String,
}

impl Default for OllamaAiConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:11434/api/chat".into(),
            model: "llama3.1".into(),
        }
    }
}

/// When the proactive command-suggestion bar proposes a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SuggestionTrigger {
    /// Never propose anything (the bar stays hidden even when enabled).
    Off,
    /// Only propose when the user explicitly asks (the `SuggestCommand`
    /// keybinding / palette action). No background provider calls.
    Manual,
    /// Propose automatically once the terminal has been idle at a prompt
    /// for [`AiSuggestionsConfig::idle_secs`] seconds. One proposal per
    /// prompt — it does not re-fire until new output arrives.
    #[default]
    Auto,
}

impl SuggestionTrigger {
    /// Every variant, in display order — for dropdown iteration.
    #[must_use]
    pub fn all() -> [Self; 3] {
        [Self::Off, Self::Manual, Self::Auto]
    }

    /// Human-readable label for the Settings dropdown.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Manual => "Manual (on keypress)",
            Self::Auto => "Automatic (when idle)",
        }
    }
}

/// Proactive AI command-suggestion bar — the strip at the bottom of the window
/// that reads recent terminal output and proposes the next command, with an
/// inject button so the user can drop it onto the prompt for review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AiSuggestionsConfig {
    /// Master switch. When `false` the bar never appears and no provider
    /// calls are ever made for suggestions.
    pub enabled: bool,
    /// When a suggestion is proposed (see [`SuggestionTrigger`]).
    pub trigger: SuggestionTrigger,
    /// Seconds of terminal idle (no new output) before an `Auto` suggestion
    /// fires. Clamped to `1..=60`.
    pub idle_secs: u32,
    /// How many trailing lines of terminal output are sent to the model as
    /// context. Clamped to `10..=2000`.
    pub context_lines: u32,
}

impl Default for AiSuggestionsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            trigger: SuggestionTrigger::Auto,
            idle_secs: 4,
            context_lines: 200,
        }
    }
}

impl AiSuggestionsConfig {
    /// Validate the numeric bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `idle_secs` or `context_lines`
    /// fall outside their accepted range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=60).contains(&self.idle_secs) {
            return Err(ConfigError::Invalid {
                field: "ai.suggestions.idle_secs",
                message: "must be between 1 and 60",
            });
        }
        if !(10..=2000).contains(&self.context_lines) {
            return Err(ConfigError::Invalid {
                field: "ai.suggestions.context_lines",
                message: "must be between 10 and 2000",
            });
        }
        Ok(())
    }
}

/// Per-provider configuration block for the v2.0 AI assistant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct AiConfig {
    /// Which provider should be used when the user invokes the
    /// assistant without specifying one. Must match one of the keys
    /// below (`"claude"`, `"openai"`, `"ollama"`).
    pub default_provider: String,
    /// Anthropic Claude credentials + defaults.
    pub claude: ClaudeAiConfig,
    /// OpenAI Chat Completions credentials + defaults.
    pub openai: OpenAiAiConfig,
    /// Ollama (local) endpoint + default model.
    pub ollama: OllamaAiConfig,
    /// Render assistant replies as markdown — fenced code blocks, inline code,
    /// bold/italic, headings, lists, links. Disable to see the raw text.
    pub render_markdown: bool,
    /// When enabled, show an unobtrusive hint after a command block finishes
    /// with a non-zero exit code, reminding the user that "Fix last command"
    /// is available.  The hint never sends anything to a remote provider on
    /// its own — it only prompts; the action is always explicit.
    ///
    /// Default `false` (opt-in, because the reminder appears after every
    /// failed command and some users find that noisy).
    pub offer_fix_on_failure: bool,
    /// Proactive command-suggestion bar settings.
    pub suggestions: AiSuggestionsConfig,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            default_provider: "claude".into(),
            claude: ClaudeAiConfig::default(),
            openai: OpenAiAiConfig::default(),
            ollama: OllamaAiConfig::default(),
            render_markdown: true,
            offer_fix_on_failure: false,
            suggestions: AiSuggestionsConfig::default(),
        }
    }
}

impl AiConfig {
    /// Validate the AI configuration sub-sections.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when a nested section is out of range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.suggestions.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        AiConfig::default()
            .validate()
            .expect("default AI config must validate");
    }

    /// SEC: API keys must NEVER serialize into config.toml — they live in
    /// the OS keychain. A legacy plaintext key must still *deserialize*
    /// (that's the migration path).
    #[test]
    fn api_keys_never_serialize_but_still_deserialize() {
        let cfg = AiConfig {
            claude: ClaudeAiConfig {
                api_key: "sk-ant-SECRET".into(),
                ..Default::default()
            },
            openai: OpenAiAiConfig {
                api_key: "sk-proj-SECRET".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let toml = toml::to_string(&cfg).expect("serialize");
        assert!(
            !toml.contains("SECRET") && !toml.contains("api_key"),
            "API keys leaked into serialized config:\n{toml}"
        );

        // Legacy config with a plaintext key still parses (migration path).
        let legacy = "[claude]\napi_key = \"sk-ant-OLD\"\n";
        let parsed: AiConfig = toml::from_str(legacy).expect("legacy key must deserialize");
        assert_eq!(parsed.claude.api_key, "sk-ant-OLD");
    }

    #[test]
    fn suggestions_default_is_auto_enabled() {
        let s = AiSuggestionsConfig::default();
        assert!(s.enabled, "suggestions enabled by default");
        assert_eq!(s.trigger, SuggestionTrigger::Auto);
        assert_eq!(s.idle_secs, 4);
        assert_eq!(s.context_lines, 200);
    }

    #[test]
    fn rejects_out_of_range_idle_secs() {
        let invalid = |secs| AiSuggestionsConfig {
            idle_secs: secs,
            ..Default::default()
        };
        assert!(invalid(0).validate().is_err());
        assert!(invalid(61).validate().is_err());
        assert!(invalid(60).validate().is_ok());
        assert!(invalid(1).validate().is_ok());
    }

    #[test]
    fn rejects_out_of_range_context_lines() {
        let with = |lines| AiSuggestionsConfig {
            context_lines: lines,
            ..Default::default()
        };
        assert!(with(9).validate().is_err());
        assert!(with(2001).validate().is_err());
        assert!(with(10).validate().is_ok());
        assert!(with(2000).validate().is_ok());
    }

    #[test]
    fn trigger_serializes_snake_case() {
        let toml = toml::to_string(&AiSuggestionsConfig {
            trigger: SuggestionTrigger::Manual,
            ..Default::default()
        })
        .unwrap();
        assert!(
            toml.contains("trigger = \"manual\""),
            "expected snake_case trigger, got:\n{toml}"
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let original = AiSuggestionsConfig {
            enabled: false,
            trigger: SuggestionTrigger::Off,
            idle_secs: 30,
            context_lines: 500,
        };
        let toml = toml::to_string(&original).unwrap();
        let parsed: AiSuggestionsConfig = toml::from_str(&toml).unwrap();
        assert_eq!(parsed.enabled, original.enabled);
        assert_eq!(parsed.trigger, original.trigger);
        assert_eq!(parsed.idle_secs, original.idle_secs);
        assert_eq!(parsed.context_lines, original.context_lines);
    }

    #[test]
    fn trigger_all_covers_every_variant_and_has_labels() {
        for t in SuggestionTrigger::all() {
            assert!(!t.label().is_empty());
        }
        assert_eq!(SuggestionTrigger::all().len(), 3);
    }
}
