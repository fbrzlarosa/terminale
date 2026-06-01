//! Profile collection and default-selection configuration.

use crate::profile::Profile;
use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Profiles section: the list of available shells and the chosen default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ProfilesConfig {
    /// Name of the profile to launch when no `--profile` flag is passed.
    /// Must match an entry in `profiles`.
    pub default: Option<String>,
    /// All known profiles. Edit / add freely.
    pub profiles: Vec<Profile>,
}

impl ProfilesConfig {
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.profiles.is_empty() {
            return Ok(());
        }
        if let Some(name) = &self.default {
            if !self.profiles.iter().any(|p| &p.name == name) {
                return Err(ConfigError::Invalid {
                    field: "profiles.default",
                    message: "name does not match any profile",
                });
            }
        }
        Ok(())
    }
}
