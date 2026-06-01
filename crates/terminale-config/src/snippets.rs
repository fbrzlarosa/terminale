//! User-defined text snippet library.
//!
//! Each [`Snippet`] has a human-readable name, an optional description, and a
//! body that supports the same escape sequences as the `send:` keybind action
//! (`\n`, `\r`, `\t`, `\e`, `\\`, `\xNN`). The `body` is decoded by
//! [`terminale_config::decode_send_string`] at insertion time; no pre-processing
//! is done at load time.
//!
//! Snippets are written in `config.toml` as a repeated `[[snippets]]` table:
//!
//! ```toml
//! [[snippets]]
//! name = "Git status"
//! body = "git status\n"
//! description = "Show the working-tree status"
//!
//! [[snippets]]
//! name = "Docker ps"
//! body = "docker ps --format 'table {{.ID}}\t{{.Names}}\t{{.Status}}'\n"
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// A single named text snippet in the user's library.
///
/// The `body` field supports the same escape sequences as the `send:` keybind
/// action: `\n` → LF, `\r` → CR, `\t` → TAB, `\e` → ESC (0x1B), `\\` → `\`,
/// `\xNN` → byte `NN`. Any other `\X` is left verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Snippet {
    /// Short display name shown in the snippet picker. Must be non-empty.
    pub name: String,
    /// Text body to insert into the focused pane when this snippet is selected.
    /// Supports `\n`, `\r`, `\t`, `\e`, `\\`, `\xNN` escape sequences.
    pub body: String,
    /// Optional one-line description shown as the secondary label in the picker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Snippet {
    /// Validate this snippet: `name` must be non-empty.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when `name` is empty.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.name.trim().is_empty() {
            return Err(ConfigError::Invalid {
                field: "snippets[].name",
                message: "snippet name must not be empty",
            });
        }
        Ok(())
    }
}
