//! Per-host / per-directory context auto-switch rules.
//!
//! A `[[context_rules]]` entry matches the active tab when its
//! `host_glob` matches the tab's SSH host name **or** its `cwd_glob`
//! matches the tab's current working directory. The first matching rule
//! wins. Matching rules tint the tab chip in `tab_color` and/or overlay a
//! short `badge` text on the pill — the primary use case is a safety cue
//! for production hosts (red tab + "PROD" badge).

use crate::ConfigError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One context-switch rule from the `[[context_rules]]` TOML array.
///
/// A rule is considered to **match** a tab when:
/// - `host_glob` is set and the tab's SSH host name matches the glob, OR
/// - `cwd_glob` is set and the tab's working directory matches the glob.
///
/// At least one of `host_glob` / `cwd_glob` must be provided (validation
/// rejects rules with neither). At least one of `tab_color` / `badge` must
/// also be provided so the rule actually does something visible.
///
/// Glob patterns use `*` to match any sequence of characters (including
/// `/`), and `?` to match any single character. Patterns are applied
/// case-insensitively on all platforms.
///
/// # Example
///
/// ```toml
/// [[context_rules]]
/// name    = "Production"
/// host_glob = "*prod*"
/// tab_color = [200, 50, 50]
/// badge   = "PROD"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ContextRule {
    /// Optional human-readable label shown in the Settings list. Has no
    /// effect on matching; purely for the user's reference.
    #[serde(default)]
    pub name: String,

    /// Glob matched against the tab's SSH host name (set when the tab was
    /// opened via the SSH picker or a `[[ssh_hosts]]` entry). `None` =
    /// skip the host check for this rule.
    ///
    /// Example: `"*prod*"` matches `"db.prod.example.com"`.
    #[serde(default)]
    pub host_glob: Option<String>,

    /// Glob matched against the tab's current working directory (announced
    /// via OSC 7 by the shell). The full path is matched; `None` = skip
    /// the cwd check for this rule.
    ///
    /// Example: `"/srv/production/*"` matches `/srv/production/app`.
    #[serde(default)]
    pub cwd_glob: Option<String>,

    /// RGB colour applied to the tab chip background when this rule
    /// matches. `None` = no colour override (but a `badge` may still be
    /// set). Values are clamped to `0..=255`.
    #[serde(default)]
    pub tab_color: Option<[u8; 3]>,

    /// Short text overlaid on the tab pill when this rule matches (e.g.
    /// `"PROD"`, `"STAGING"`, `"DEV"`). Kept short (≤ 6 chars renders
    /// cleanly). `None` = no badge.
    #[serde(default)]
    pub badge: Option<String>,
}

impl ContextRule {
    /// Validate a single rule: at least one match criterion and at least one
    /// visible override must be set.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Invalid`] when the rule is incomplete.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.host_glob.is_none() && self.cwd_glob.is_none() {
            return Err(ConfigError::Invalid {
                field: "context_rules[].host_glob / cwd_glob",
                message: "at least one of host_glob or cwd_glob must be set",
            });
        }
        if self.tab_color.is_none() && self.badge.is_none() {
            return Err(ConfigError::Invalid {
                field: "context_rules[].tab_color / badge",
                message: "at least one of tab_color or badge must be set",
            });
        }
        Ok(())
    }

    /// Test whether this rule matches the given `ssh_host_name` (the SSH
    /// host the tab is connected to, empty for local tabs) or `cwd` (the
    /// current working directory announced via OSC 7, empty when unknown).
    ///
    /// Returns `true` when the rule matches and should be applied.
    #[must_use]
    pub fn matches(&self, ssh_host_name: &str, cwd: &str) -> bool {
        if let Some(pat) = &self.host_glob {
            if !ssh_host_name.is_empty() && glob_matches(pat, ssh_host_name) {
                return true;
            }
        }
        if let Some(pat) = &self.cwd_glob {
            if !cwd.is_empty() && glob_matches(pat, cwd) {
                return true;
            }
        }
        false
    }
}

/// Evaluate `rules` against `ssh_host_name` and `cwd`, returning the first
/// matching rule (if any). This implements first-match-wins semantics.
#[must_use]
pub fn evaluate_context_rules<'a>(
    rules: &'a [ContextRule],
    ssh_host_name: &str,
    cwd: &str,
) -> Option<&'a ContextRule> {
    rules.iter().find(|r| r.matches(ssh_host_name, cwd))
}

// ── Simple glob matcher ───────────────────────────────────────────────────────

/// Match `text` against `pattern`, where `*` matches any sequence of
/// characters (including `/` and empty) and `?` matches exactly one
/// character. The comparison is case-insensitive.
///
/// This is a recursive implementation suitable for the small patterns and
/// strings found in user config globs. It avoids pulling in the `glob` crate
/// as a new dependency (the feature is already present in the tree via the
/// `regex` dep used elsewhere, but a direct glob → regex translation is overkill
/// here).
#[must_use]
fn glob_matches(pattern: &str, text: &str) -> bool {
    let pat = pattern.to_lowercase();
    let txt = text.to_lowercase();
    glob_match_inner(pat.as_bytes(), txt.as_bytes())
}

fn glob_match_inner(pat: &[u8], txt: &[u8]) -> bool {
    match (pat.first(), txt.first()) {
        (None, None) => true,
        (None, Some(_)) => false,
        (Some(b'*'), _) => {
            // Try matching 0 or more characters.
            let rest = &pat[1..];
            // Skip consecutive stars.
            if rest.first() == Some(&b'*') {
                return glob_match_inner(rest, txt);
            }
            // * matches the empty string here.
            if glob_match_inner(rest, txt) {
                return true;
            }
            // * consumes one character and recurses.
            if txt.is_empty() {
                return false;
            }
            glob_match_inner(pat, &txt[1..])
        }
        (Some(b'?'), Some(_)) => glob_match_inner(&pat[1..], &txt[1..]),
        (Some(b'?'), None) => false,
        (Some(pc), Some(tc)) => pc == tc && glob_match_inner(&pat[1..], &txt[1..]),
        (Some(_), None) => false,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── glob_matches ─────────────────────────────────────────────────────────

    #[test]
    fn glob_star_matches_any_sequence() {
        assert!(glob_matches("*prod*", "db.prod.example.com"));
        assert!(glob_matches("*prod*", "prod"));
        assert!(glob_matches("*prod*", "my-prod-server"));
        assert!(!glob_matches("*prod*", "staging.server"));
    }

    #[test]
    fn glob_star_matches_empty() {
        assert!(glob_matches("*prod", "prod"));
        assert!(glob_matches("prod*", "prod"));
    }

    #[test]
    fn glob_question_matches_single_char() {
        assert!(glob_matches("pro?", "prod"));
        assert!(glob_matches("pro?", "pros"));
        assert!(!glob_matches("pro?", "pro"));
        assert!(!glob_matches("pro?", "prods"));
    }

    #[test]
    fn glob_exact_match() {
        assert!(glob_matches("prod", "prod"));
        // Matching is case-insensitive, so "prod" also matches "PROD".
        assert!(glob_matches("prod", "PROD"));
        // Different strings do not match.
        assert!(!glob_matches("prod", "staging"));
    }

    #[test]
    fn glob_case_insensitive() {
        assert!(glob_matches("*PROD*", "db.prod.example.com"));
        assert!(glob_matches("*prod*", "DB.PROD.EXAMPLE.COM"));
    }

    #[test]
    fn glob_path_wildcards() {
        assert!(glob_matches("/srv/production/*", "/srv/production/app"));
        assert!(glob_matches("/srv/production/*", "/srv/production/app/sub"));
        assert!(!glob_matches("/srv/production/*", "/srv/staging/app"));
    }

    // ── ContextRule::matches ──────────────────────────────────────────────────

    #[test]
    fn rule_host_glob_matches() {
        let r = ContextRule {
            host_glob: Some("*prod*".into()),
            tab_color: Some([200, 50, 50]),
            ..Default::default()
        };
        assert!(r.matches("db.prod.example", ""));
        assert!(!r.matches("staging.server", ""));
    }

    #[test]
    fn rule_cwd_glob_matches() {
        let r = ContextRule {
            cwd_glob: Some("/srv/prod/*".into()),
            tab_color: Some([200, 50, 50]),
            ..Default::default()
        };
        assert!(r.matches("", "/srv/prod/app"));
        assert!(!r.matches("", "/srv/staging/app"));
    }

    #[test]
    fn rule_empty_inputs_do_not_match() {
        let r = ContextRule {
            host_glob: Some("*prod*".into()),
            cwd_glob: Some("/prod/*".into()),
            tab_color: Some([200, 50, 50]),
            ..Default::default()
        };
        // Both inputs empty → no match (glob would match empty incorrectly).
        assert!(!r.matches("", ""));
    }

    // ── evaluate_context_rules ────────────────────────────────────────────────

    #[test]
    fn first_match_wins() {
        let rules = vec![
            ContextRule {
                name: "prod".into(),
                host_glob: Some("*prod*".into()),
                tab_color: Some([200, 50, 50]),
                badge: Some("PROD".into()),
                ..Default::default()
            },
            ContextRule {
                name: "staging".into(),
                host_glob: Some("*staging*".into()),
                tab_color: Some([200, 150, 50]),
                badge: Some("STG".into()),
                ..Default::default()
            },
        ];
        let hit = evaluate_context_rules(&rules, "db.prod.example", "");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap().name, "prod");

        let miss = evaluate_context_rules(&rules, "dev.server", "");
        assert!(miss.is_none());
    }

    #[test]
    fn no_match_returns_none() {
        let rules = vec![ContextRule {
            host_glob: Some("*prod*".into()),
            tab_color: Some([200, 50, 50]),
            ..Default::default()
        }];
        assert!(evaluate_context_rules(&rules, "staging", "/home/user").is_none());
    }

    #[test]
    fn clearing_when_no_rule_matches() {
        // When cwd changes to something that no longer matches any rule,
        // evaluate returns None (the caller clears auto_color/auto_badge).
        let rules = vec![ContextRule {
            cwd_glob: Some("/srv/prod/*".into()),
            tab_color: Some([200, 50, 50]),
            ..Default::default()
        }];
        assert!(evaluate_context_rules(&rules, "", "/srv/prod/app").is_some());
        assert!(evaluate_context_rules(&rules, "", "/home/user/project").is_none());
    }

    // ── ContextRule::validate ─────────────────────────────────────────────────

    #[test]
    fn validate_rejects_no_match_criterion() {
        let r = ContextRule {
            tab_color: Some([255, 0, 0]),
            ..Default::default()
        };
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_rejects_no_override() {
        let r = ContextRule {
            host_glob: Some("*prod*".into()),
            ..Default::default()
        };
        assert!(r.validate().is_err());
    }

    #[test]
    fn validate_accepts_minimal_valid_rule() {
        let r = ContextRule {
            host_glob: Some("*prod*".into()),
            tab_color: Some([200, 50, 50]),
            ..Default::default()
        };
        assert!(r.validate().is_ok());
    }

    // ── Config roundtrip ──────────────────────────────────────────────────────

    #[test]
    fn context_rule_toml_roundtrip() {
        let rule = ContextRule {
            name: "Production".into(),
            host_glob: Some("*prod*".into()),
            cwd_glob: None,
            tab_color: Some([200, 50, 50]),
            badge: Some("PROD".into()),
        };
        let s = toml::to_string(&rule).expect("serialize");
        let back: ContextRule = toml::from_str(&s).expect("deserialize");
        assert_eq!(back.name, "Production");
        assert_eq!(back.host_glob, Some("*prod*".into()));
        assert_eq!(back.tab_color, Some([200, 50, 50]));
        assert_eq!(back.badge, Some("PROD".into()));
    }
}
