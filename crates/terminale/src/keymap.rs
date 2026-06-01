//! Custom multi-action keybind resolution.
//!
//! Resolves a pressed key combo against the user's `[[keybinds.custom]]`
//! table, returning the ordered list of [`ResolvedAction`]s to execute.
//!
//! Each action in the list is either a named built-in [`ShortcutAction`]
//! or a `SendString` payload (a `Vec<u8>` already decoded from the
//! `send:…` escape syntax).

use terminale_config::{CustomKeybind, KeyActionSpec};
use winit::keyboard::{ModifiersState, PhysicalKey};

use crate::{
    shortcuts::{parse_binding, pressed_key_name},
    ShortcutAction,
};

/// A fully-resolved action from a custom keybind entry.
#[derive(Debug, Clone)]
pub(crate) enum ResolvedAction {
    /// Dispatch a built-in shortcut.
    Shortcut(ShortcutAction),
    /// Write raw bytes to the focused pane's PTY.
    SendString(Vec<u8>),
}

/// Try to match `mods` + `physical` + `logical` against every entry in
/// `custom`, returning the resolved action list for the **first** match.
///
/// Returns `None` when no custom binding matches the pressed combo.
pub(crate) fn resolve_custom(
    mods: &ModifiersState,
    physical: PhysicalKey,
    logical: &winit::keyboard::Key,
    custom: &[CustomKeybind],
) -> Option<Vec<ResolvedAction>> {
    let key = pressed_key_name(physical, logical)?;
    let pressed = crate::shortcuts::ModFlags {
        ctrl: mods.control_key(),
        shift: mods.shift_key(),
        alt: mods.alt_key(),
        meta: mods.super_key(),
    };

    for bind in custom {
        if bind.keys.is_empty() {
            continue;
        }
        let Some((bm, bk)) = parse_binding(&bind.keys) else {
            continue;
        };
        if bm != pressed || !bk.eq_ignore_ascii_case(&key) {
            continue;
        }
        // Combo matched — resolve the action list, skipping unknown names.
        let resolved: Vec<ResolvedAction> = bind
            .actions
            .iter()
            .filter_map(resolve_action_spec)
            .collect();
        if !resolved.is_empty() {
            return Some(resolved);
        }
    }
    None
}

/// Resolve a single [`KeyActionSpec`] into a [`ResolvedAction`].
/// Returns `None` for unknown action names (so they are silently skipped).
fn resolve_action_spec(spec: &KeyActionSpec) -> Option<ResolvedAction> {
    // A `send:…` spec decodes directly to bytes.
    if let Some(bytes) = spec.as_send_bytes() {
        return Some(ResolvedAction::SendString(bytes));
    }
    // Otherwise try to map the name to a built-in ShortcutAction.
    let name = spec.action_name()?;
    let action = action_from_name(name)?;
    Some(ResolvedAction::Shortcut(action))
}

/// Map a case-insensitive action name string to a [`ShortcutAction`].
/// Returns `None` for unknown names so callers can skip them gracefully.
#[must_use]
#[allow(clippy::too_many_lines)]
pub(crate) fn action_from_name(name: &str) -> Option<ShortcutAction> {
    Some(match name.to_lowercase().as_str() {
        "newtab" => ShortcutAction::NewTab,
        "closetab" => ShortcutAction::CloseTab,
        "nexttab" => ShortcutAction::NextTab,
        "prevtab" => ShortcutAction::PrevTab,
        "movetableft" => ShortcutAction::MoveTabLeft,
        "movetabright" => ShortcutAction::MoveTabRight,
        "profilepicker" => ShortcutAction::ProfilePicker,
        "restarttab" => ShortcutAction::RestartTab,
        "copy" => ShortcutAction::Copy,
        "paste" => ShortcutAction::Paste,
        "selectall" => ShortcutAction::SelectAll,
        "find" => ShortcutAction::Find,
        "clear" => ShortcutAction::Clear,
        "settings" => ShortcutAction::Settings,
        "fontincrease" => ShortcutAction::FontIncrease,
        "fontdecrease" => ShortcutAction::FontDecrease,
        "fontreset" => ShortcutAction::FontReset,
        "scrolllineup" => ShortcutAction::ScrollLineUp,
        "scrolllinedown" => ShortcutAction::ScrollLineDown,
        "scrollpageup" => ShortcutAction::ScrollPageUp,
        "scrollpagedown" => ShortcutAction::ScrollPageDown,
        "scrolltop" => ShortcutAction::ScrollTop,
        "scrollbottom" => ShortcutAction::ScrollBottom,
        "aiassistant" => ShortcutAction::AiAssistant,
        "commandpalette" => ShortcutAction::CommandPalette,
        "explainselection" => ShortcutAction::ExplainSelection,
        "clearscrollback" => ShortcutAction::ClearScrollback,
        "reopenclosedtab" => ShortcutAction::ReopenClosedTab,
        "newsshetab" | "newsshtab" => ShortcutAction::NewSshTab,
        "togglestayontop" => ShortcutAction::ToggleStayOnTop,
        "snaptop" => ShortcutAction::SnapTop,
        "snapbottom" => ShortcutAction::SnapBottom,
        "snapleft" => ShortcutAction::SnapLeft,
        "snapright" => ShortcutAction::SnapRight,
        "snapcenter" => ShortcutAction::SnapCenter,
        "snapmaximize" => ShortcutAction::SnapMaximize,
        "snaptopleft" => ShortcutAction::SnapTopLeft,
        "snaptopright" => ShortcutAction::SnapTopRight,
        "snapbottomleft" => ShortcutAction::SnapBottomLeft,
        "snapbottomright" => ShortcutAction::SnapBottomRight,
        "showsnaplayouts" => ShortcutAction::ShowSnapLayouts,
        "splitright" => ShortcutAction::SplitRight,
        "splitdown" => ShortcutAction::SplitDown,
        "splitleft" => ShortcutAction::SplitLeft,
        "splitup" => ShortcutAction::SplitUp,
        "closepane" => ShortcutAction::ClosePane,
        "focuspaneleft" => ShortcutAction::FocusPaneLeft,
        "focuspaneright" => ShortcutAction::FocusPaneRight,
        "focuspaneup" => ShortcutAction::FocusPaneUp,
        "focuspanedown" => ShortcutAction::FocusPaneDown,
        "togglepanezoom" => ShortcutAction::TogglePaneZoom,
        "resizepaneleft" => ShortcutAction::ResizePaneLeft,
        "resizepaneright" => ShortcutAction::ResizePaneRight,
        "resizepaneup" => ShortcutAction::ResizePaneUp,
        "resizepanedown" => ShortcutAction::ResizePaneDown,
        "activatetab1" => ShortcutAction::ActivateTab1,
        "activatetab2" => ShortcutAction::ActivateTab2,
        "activatetab3" => ShortcutAction::ActivateTab3,
        "activatetab4" => ShortcutAction::ActivateTab4,
        "activatetab5" => ShortcutAction::ActivateTab5,
        "activatetab6" => ShortcutAction::ActivateTab6,
        "activatetab7" => ShortcutAction::ActivateTab7,
        "activatetab8" => ShortcutAction::ActivateTab8,
        "activatetab9" => ShortcutAction::ActivateTab9,
        "lasttab" => ShortcutAction::LastTab,
        "prevprompt" => ShortcutAction::PrevPrompt,
        "nextprompt" => ShortcutAction::NextPrompt,
        "copymode" => ShortcutAction::CopyMode,
        "quickselect" => ShortcutAction::QuickSelect,
        "paneselect" => ShortcutAction::PaneSelect,
        "reloadconfig" => ShortcutAction::ReloadConfig,
        "togglefullscreen" => ShortcutAction::ToggleFullscreen,
        "togglezenmode" => ShortcutAction::ToggleZenMode,
        "togglebroadcastinput" => ShortcutAction::ToggleBroadcastInput,
        "newwindow" => ShortcutAction::NewWindow,
        "movetabtonewwindow" => ShortcutAction::MoveTabToNewWindow,
        "movepanenewtab" => ShortcutAction::MovePaneToNewTab,
        "movepanetonewwindow" => ShortcutAction::MovePaneToNewWindow,
        "opensnippets" => ShortcutAction::OpenSnippets,
        "fixlastcommand" => ShortcutAction::FixLastCommand,
        "saveworkspace" => ShortcutAction::SaveWorkspace,
        "openworkspace" => ShortcutAction::OpenWorkspace,
        "copylastcommandoutput" => ShortcutAction::CopyLastCommandOutput,
        "copyblockoutput" => ShortcutAction::CopyBlockOutput,
        "copylastcommand" => ShortcutAction::CopyLastCommand,
        "rerunlastcommand" => ShortcutAction::RerunLastCommand,
        "editlastcommand" => ShortcutAction::EditLastCommand,
        "opencommandhistory" => ShortcutAction::OpenCommandHistory,
        "exportscrollback" => ShortcutAction::ExportScrollback,
        "toggletabpin" => ShortcutAction::ToggleTabPin,
        // Pane swap / rotate.
        "movepaneleft" => ShortcutAction::MovePaneLeft,
        "movepaneright" => ShortcutAction::MovePaneRight,
        "movepaneup" => ShortcutAction::MovePaneUp,
        "movepanedown" => ShortcutAction::MovePaneDown,
        "rotatepanes" => ShortcutAction::RotatePanes,
        "rotatepaneback" | "rotatepanersback" | "rotatepanesback" => {
            ShortcutAction::RotatePanesBack
        }
        "jumptoprevfailedcommand" | "prevfailedcommand" => ShortcutAction::JumpToPrevFailedCommand,
        "jumptonextfailedcommand" | "nextfailedcommand" => ShortcutAction::JumpToNextFailedCommand,
        "openfailedcommandpicker" | "failedcommandpicker" => {
            ShortcutAction::OpenFailedCommandPicker
        }
        "newtabgroup" => ShortcutAction::NewTabGroup,
        "assigntabtogroup" => ShortcutAction::AssignTabToGroup,
        "cleartabgroup" => ShortcutAction::ClearTabGroup,
        "suggestcommand" => ShortcutAction::SuggestCommand,
        "renametabgroup" => ShortcutAction::RenameTabGroup,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminale_config::{CustomKeybind, KeyActionSpec, KeyTable, KeyTableEntry, KeybindsConfig};

    // ── action_from_name ──────────────────────────────────────────────────────

    #[test]
    fn action_from_name_case_insensitive() {
        assert!(matches!(
            action_from_name("newtab"),
            Some(ShortcutAction::NewTab)
        ));
        assert!(matches!(
            action_from_name("NewTab"),
            Some(ShortcutAction::NewTab)
        ));
        assert!(matches!(
            action_from_name("NEWTAB"),
            Some(ShortcutAction::NewTab)
        ));
    }

    #[test]
    fn action_from_name_unknown_returns_none() {
        assert!(action_from_name("DoesNotExist").is_none());
        assert!(action_from_name("").is_none());
    }

    /// `ToggleFullscreen` and `ToggleZenMode` resolve from their name strings.
    #[test]
    fn action_from_name_fullscreen_and_zen_resolve() {
        assert_eq!(
            action_from_name("togglefullscreen"),
            Some(ShortcutAction::ToggleFullscreen),
            "togglefullscreen must resolve to ToggleFullscreen"
        );
        assert_eq!(
            action_from_name("ToggleFullscreen"),
            Some(ShortcutAction::ToggleFullscreen),
            "ToggleFullscreen (mixed case) must resolve"
        );
        assert_eq!(
            action_from_name("togglezenmode"),
            Some(ShortcutAction::ToggleZenMode),
            "togglezenmode must resolve to ToggleZenMode"
        );
        assert_eq!(
            action_from_name("ToggleZenMode"),
            Some(ShortcutAction::ToggleZenMode),
            "ToggleZenMode (mixed case) must resolve"
        );
    }

    #[test]
    fn action_from_name_all_builtins_resolve() {
        // Spot-check a representative sample of each category.
        let cases = [
            ("copy", ShortcutAction::Copy),
            ("paste", ShortcutAction::Paste),
            ("settings", ShortcutAction::Settings),
            ("commandpalette", ShortcutAction::CommandPalette),
            ("splitright", ShortcutAction::SplitRight),
            ("closepane", ShortcutAction::ClosePane),
            ("reloadconfig", ShortcutAction::ReloadConfig),
            ("copymode", ShortcutAction::CopyMode),
            ("quickselect", ShortcutAction::QuickSelect),
            ("togglefullscreen", ShortcutAction::ToggleFullscreen),
            ("togglezenmode", ShortcutAction::ToggleZenMode),
        ];
        for (name, expected) in cases {
            assert_eq!(
                action_from_name(name),
                Some(expected),
                "action_from_name(\"{name}\") should resolve"
            );
        }
    }

    // ── Multi-action sequence ordering ────────────────────────────────────────

    #[test]
    fn multi_action_sequence_order_preserved() {
        // Build a config with two consecutive named actions.
        let cfg = KeybindsConfig {
            custom: vec![CustomKeybind {
                keys: "Ctrl+Alt+X".to_string(),
                actions: vec![
                    KeyActionSpec::Action("NewTab".to_string()),
                    KeyActionSpec::Action("CommandPalette".to_string()),
                ],
            }],
            ..Default::default()
        };

        // Manually resolve without going through the winit key path.
        let resolved: Vec<ResolvedAction> = cfg.custom[0]
            .actions
            .iter()
            .filter_map(resolve_action_spec)
            .collect();

        assert_eq!(resolved.len(), 2, "two actions expected");
        assert!(
            matches!(
                resolved[0],
                ResolvedAction::Shortcut(ShortcutAction::NewTab)
            ),
            "first action must be NewTab"
        );
        assert!(
            matches!(
                resolved[1],
                ResolvedAction::Shortcut(ShortcutAction::CommandPalette)
            ),
            "second action must be CommandPalette"
        );
    }

    #[test]
    fn unknown_action_name_skipped_gracefully() {
        let spec = KeyActionSpec::Action("UnknownActionThatDoesNotExist".to_string());
        // resolve_action_spec must return None — no panic, no error.
        assert!(
            resolve_action_spec(&spec).is_none(),
            "unknown action name must be silently skipped"
        );
    }

    // ── KeyTable config tests ────────────────────────────────────────────────

    /// Default KeybindsConfig has an empty key_tables list.
    #[test]
    fn key_tables_default_is_empty() {
        let cfg = KeybindsConfig::default();
        assert!(
            cfg.key_tables.is_empty(),
            "key_tables must default to empty"
        );
    }

    /// Resolving a key within a table returns its action list.
    #[test]
    fn key_table_entry_actions_resolve() {
        let table = KeyTable {
            name: "nav".to_string(),
            leader: "Ctrl+B".to_string(),
            timeout_ms: 1500,
            bindings: vec![KeyTableEntry {
                key: "N".to_string(),
                actions: vec![
                    KeyActionSpec::Action("NextTab".to_string()),
                    KeyActionSpec::Action("Copy".to_string()),
                ],
            }],
        };
        // Simulate what handle_key_table_input does: find the entry for key "N".
        let entry = table
            .bindings
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case("N"));
        assert!(entry.is_some(), "entry for key N must be found");
        let resolved: Vec<ResolvedAction> = entry
            .unwrap()
            .actions
            .iter()
            .filter_map(resolve_action_spec)
            .collect();
        assert_eq!(resolved.len(), 2, "two actions expected");
        assert!(
            matches!(
                resolved[0],
                ResolvedAction::Shortcut(ShortcutAction::NextTab)
            ),
            "first action must be NextTab"
        );
        assert!(
            matches!(resolved[1], ResolvedAction::Shortcut(ShortcutAction::Copy)),
            "second action must be Copy"
        );
    }
}
