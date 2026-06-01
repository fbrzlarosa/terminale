// `use super::*` is intentional: this file is a tight sub-module of
// settings_window and inherits all its helpers and types by design.
#[allow(clippy::wildcard_imports)]
use super::*;

impl SettingsWindow {
    pub(super) fn section_shortcuts(&mut self, ui: &mut egui::Ui) {
        page_header(
            ui,
            "Shortcuts",
            "Keyboard shortcuts. Click any combo, press the keys you want.",
        );

        let mut dirty = false;
        // Walk the shortcut catalogue in grouped order. Each tuple is
        // `(section title, [(id, label, get-mut, default)])`. The
        // `id` is used both as a unique recorder handle and as the
        // egui widget id, so it must be stable per row.
        type Getter = fn(&mut terminale_config::ShortcutsConfig) -> &mut String;
        let groups: &[(&str, &[(&str, &str, Getter, &str)])] = &[
            (
                "Tabs",
                &[
                    ("new_tab", "New tab", |s| &mut s.new_tab, "Ctrl+T"),
                    ("close_tab", "Close tab", |s| &mut s.close_tab, "Ctrl+W"),
                    ("next_tab", "Next tab", |s| &mut s.next_tab, "Ctrl+Tab"),
                    (
                        "prev_tab",
                        "Previous tab",
                        |s| &mut s.prev_tab,
                        "Ctrl+Shift+Tab",
                    ),
                    (
                        "move_tab_left",
                        // U+2B05 / U+27A1 emoji arrows — covered by the
                        // bundled NotoEmoji. The plain U+2190 / U+2192
                        // arrows live only in egui's monospace Hack face
                        // and rendered as tofu in these proportional labels.
                        "Move tab ⬅",
                        |s| &mut s.move_tab_left,
                        "Ctrl+Shift+ArrowLeft",
                    ),
                    (
                        "move_tab_right",
                        "Move tab ➡",
                        |s| &mut s.move_tab_right,
                        "Ctrl+Shift+ArrowRight",
                    ),
                    (
                        "profile_picker",
                        "Profile picker",
                        |s| &mut s.profile_picker,
                        "Ctrl+Shift+T",
                    ),
                    (
                        "restart_tab",
                        "Restart crashed tab",
                        |s| &mut s.restart_tab,
                        "Ctrl+Shift+R",
                    ),
                    (
                        "reopen_closed_tab",
                        "Reopen closed tab",
                        |s| &mut s.reopen_closed_tab,
                        "",
                    ),
                    (
                        "new_ssh_tab",
                        "New SSH tab",
                        |s| &mut s.new_ssh_tab,
                        "",
                    ),
                    (
                        "last_tab",
                        "Go to last-used tab",
                        |s| &mut s.last_tab,
                        "",
                    ),
                ],
            ),
            (
                "Tab index",
                &[
                    (
                        "activate_tab_1",
                        "Go to tab 1",
                        |s| &mut s.activate_tab_1,
                        "Ctrl+1",
                    ),
                    (
                        "activate_tab_2",
                        "Go to tab 2",
                        |s| &mut s.activate_tab_2,
                        "Ctrl+2",
                    ),
                    (
                        "activate_tab_3",
                        "Go to tab 3",
                        |s| &mut s.activate_tab_3,
                        "Ctrl+3",
                    ),
                    (
                        "activate_tab_4",
                        "Go to tab 4",
                        |s| &mut s.activate_tab_4,
                        "Ctrl+4",
                    ),
                    (
                        "activate_tab_5",
                        "Go to tab 5",
                        |s| &mut s.activate_tab_5,
                        "Ctrl+5",
                    ),
                    (
                        "activate_tab_6",
                        "Go to tab 6",
                        |s| &mut s.activate_tab_6,
                        "Ctrl+6",
                    ),
                    (
                        "activate_tab_7",
                        "Go to tab 7",
                        |s| &mut s.activate_tab_7,
                        "Ctrl+7",
                    ),
                    (
                        "activate_tab_8",
                        "Go to tab 8",
                        |s| &mut s.activate_tab_8,
                        "Ctrl+8",
                    ),
                    (
                        "activate_tab_9",
                        "Go to last tab (tab 9)",
                        |s| &mut s.activate_tab_9,
                        "Ctrl+9",
                    ),
                ],
            ),
            (
                "Clipboard & editing",
                &[
                    ("copy", "Copy", |s| &mut s.copy, "Ctrl+Shift+C"),
                    ("paste", "Paste", |s| &mut s.paste, "Ctrl+Shift+V"),
                    (
                        "select_all",
                        "Select all",
                        |s| &mut s.select_all,
                        "Ctrl+Shift+A",
                    ),
                    ("find", "Find in buffer", |s| &mut s.find, "Ctrl+Shift+F"),
                    ("clear", "Clear screen", |s| &mut s.clear, "Ctrl+L"),
                    (
                        "clear_scrollback",
                        "Clear scrollback",
                        |s| &mut s.clear_scrollback,
                        "Ctrl+Shift+K",
                    ),
                    (
                        "copy_mode",
                        "Enter copy mode",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.copy_mode,
                        "Ctrl+Shift+X",
                    ),
                    (
                        "quick_select",
                        "Quick select",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.quick_select,
                        "Ctrl+Shift+Space",
                    ),
                    (
                        "pane_select",
                        "Pane select",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.pane_select,
                        "",
                    ),
                ],
            ),
            (
                "View",
                &[
                    ("settings", "Open settings", |s| &mut s.settings, "Ctrl+,"),
                    (
                        "font_increase",
                        "Increase font",
                        |s| &mut s.font_increase,
                        "Ctrl+=",
                    ),
                    (
                        "font_decrease",
                        "Decrease font",
                        |s| &mut s.font_decrease,
                        "Ctrl+-",
                    ),
                    ("font_reset", "Reset font", |s| &mut s.font_reset, "Ctrl+0"),
                    (
                        "stay_on_top",
                        "Toggle stay on top",
                        |s| &mut s.stay_on_top,
                        "",
                    ),
                    (
                        "toggle_fullscreen",
                        "Toggle full-screen",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.toggle_fullscreen,
                        "F11",
                    ),
                    (
                        "toggle_zen_mode",
                        "Toggle zen mode",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.toggle_zen_mode,
                        "",
                    ),
                    (
                        "reload_config",
                        "Reload config",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.reload_config,
                        "",
                    ),
                ],
            ),
            (
                "Window snapping",
                &[
                    ("snap_top", "Snap top half", |s| &mut s.snap_top, ""),
                    (
                        "snap_bottom",
                        "Snap bottom half",
                        |s| &mut s.snap_bottom,
                        "",
                    ),
                    ("snap_left", "Snap left half", |s| &mut s.snap_left, ""),
                    ("snap_right", "Snap right half", |s| &mut s.snap_right, ""),
                    (
                        "snap_center",
                        "Center on monitor",
                        |s| &mut s.snap_center,
                        "",
                    ),
                    (
                        "snap_maximize",
                        "Maximize to monitor",
                        |s| &mut s.snap_maximize,
                        "",
                    ),
                    (
                        "snap_top_left",
                        "Snap top-left quarter",
                        |s| &mut s.snap_top_left,
                        "",
                    ),
                    (
                        "snap_top_right",
                        "Snap top-right quarter",
                        |s| &mut s.snap_top_right,
                        "",
                    ),
                    (
                        "snap_bottom_left",
                        "Snap bottom-left quarter",
                        |s| &mut s.snap_bottom_left,
                        "",
                    ),
                    (
                        "snap_bottom_right",
                        "Snap bottom-right quarter",
                        |s| &mut s.snap_bottom_right,
                        "",
                    ),
                    (
                        "show_snap_layouts",
                        "Show snap layout chooser",
                        |s| &mut s.show_snap_layouts,
                        "",
                    ),
                ],
            ),
            (
                "Split panes",
                &[
                    (
                        "split_right",
                        "Split pane right",
                        |s| &mut s.split_right,
                        "Ctrl+Shift+=",
                    ),
                    (
                        "split_down",
                        "Split pane down",
                        |s| &mut s.split_down,
                        "Ctrl+Shift+-",
                    ),
                    ("split_left", "Split pane left", |s| &mut s.split_left, ""),
                    ("split_up", "Split pane up", |s| &mut s.split_up, ""),
                    (
                        "close_pane",
                        "Close focused pane",
                        |s| &mut s.close_pane,
                        "Ctrl+Shift+W",
                    ),
                    (
                        "toggle_broadcast_input",
                        "Toggle broadcast input",
                        |s: &mut terminale_config::ShortcutsConfig| {
                            &mut s.toggle_broadcast_input
                        },
                        "",
                    ),
                ],
            ),
            (
                "Pane focus",
                &[
                    (
                        "focus_pane_left",
                        "Focus pane left",
                        |s| &mut s.focus_pane_left,
                        "",
                    ),
                    (
                        "focus_pane_right",
                        "Focus pane right",
                        |s| &mut s.focus_pane_right,
                        "",
                    ),
                    (
                        "focus_pane_up",
                        "Focus pane up",
                        |s| &mut s.focus_pane_up,
                        "",
                    ),
                    (
                        "focus_pane_down",
                        "Focus pane down",
                        |s| &mut s.focus_pane_down,
                        "",
                    ),
                    (
                        "toggle_pane_zoom",
                        "Toggle pane zoom",
                        |s| &mut s.toggle_pane_zoom,
                        "Ctrl+Shift+Z",
                    ),
                ],
            ),
            (
                "Pane resize",
                &[
                    (
                        "resize_pane_left",
                        "Resize pane left",
                        |s| &mut s.resize_pane_left,
                        "",
                    ),
                    (
                        "resize_pane_right",
                        "Resize pane right",
                        |s| &mut s.resize_pane_right,
                        "",
                    ),
                    (
                        "resize_pane_up",
                        "Resize pane up",
                        |s| &mut s.resize_pane_up,
                        "",
                    ),
                    (
                        "resize_pane_down",
                        "Resize pane down",
                        |s| &mut s.resize_pane_down,
                        "",
                    ),
                ],
            ),
            (
                "Pane arrangement",
                &[
                    (
                        "move_pane_left",
                        "Move pane left (swap)",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_pane_left,
                        "",
                    ),
                    (
                        "move_pane_right",
                        "Move pane right (swap)",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_pane_right,
                        "",
                    ),
                    (
                        "move_pane_up",
                        "Move pane up (swap)",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_pane_up,
                        "",
                    ),
                    (
                        "move_pane_down",
                        "Move pane down (swap)",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_pane_down,
                        "",
                    ),
                    (
                        "rotate_panes",
                        "Rotate panes forward",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.rotate_panes,
                        "",
                    ),
                    (
                        "rotate_panes_back",
                        "Rotate panes backward",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.rotate_panes_back,
                        "",
                    ),
                ],
            ),
            (
                "Windows & panes",
                &[
                    (
                        "new_window",
                        "Open new window",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.new_window,
                        "Ctrl+Shift+N",
                    ),
                    (
                        "move_tab_to_new_window",
                        "Move tab to new window",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_tab_to_new_window,
                        "",
                    ),
                    (
                        "move_pane_to_new_tab",
                        "Move pane to new tab",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_pane_to_new_tab,
                        "",
                    ),
                    (
                        "move_pane_to_new_window",
                        "Move pane to new window",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.move_pane_to_new_window,
                        "",
                    ),
                ],
            ),
            (
                "Block actions",
                &[
                    (
                        "copy_last_command_output",
                        "Copy last command output",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.copy_last_command_output,
                        "",
                    ),
                    (
                        "copy_block_output",
                        "Copy block output",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.copy_block_output,
                        "",
                    ),
                    (
                        "copy_last_command",
                        "Copy last command",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.copy_last_command,
                        "",
                    ),
                    (
                        "rerun_last_command",
                        "Re-run last command",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.rerun_last_command,
                        "",
                    ),
                    (
                        "edit_last_command",
                        "Edit last command",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.edit_last_command,
                        "",
                    ),
                ],
            ),
            (
                "Assistant",
                &[
                    (
                        "ai_assistant",
                        "AI assistant",
                        |s| &mut s.ai_assistant,
                        "Ctrl+Shift+I",
                    ),
                    (
                        "command_palette",
                        "Command palette",
                        |s| &mut s.command_palette,
                        "Ctrl+Shift+P",
                    ),
                    (
                        "explain_selection",
                        "Explain selection (AI)",
                        |s| &mut s.explain_selection,
                        "Ctrl+Shift+E",
                    ),
                    (
                        "fix_last_command",
                        "Fix last failed command (AI)",
                        |s| &mut s.fix_last_command,
                        "",
                    ),
                ],
            ),
            (
                "Scrollback",
                &[
                    (
                        "scroll_line_up",
                        "Line up",
                        |s| &mut s.scroll_line_up,
                        "Ctrl+Shift+ArrowUp",
                    ),
                    (
                        "scroll_line_down",
                        "Line down",
                        |s| &mut s.scroll_line_down,
                        "Ctrl+Shift+ArrowDown",
                    ),
                    (
                        "scroll_page_up",
                        "Page up",
                        |s| &mut s.scroll_page_up,
                        "Ctrl+Shift+PageUp",
                    ),
                    (
                        "scroll_page_down",
                        "Page down",
                        |s| &mut s.scroll_page_down,
                        "Ctrl+Shift+PageDown",
                    ),
                    (
                        "scroll_top",
                        "Jump to top",
                        |s| &mut s.scroll_top,
                        "Ctrl+Shift+Home",
                    ),
                    (
                        "scroll_bottom",
                        "Jump to bottom",
                        |s| &mut s.scroll_bottom,
                        "Ctrl+Shift+End",
                    ),
                    (
                        "prev_prompt",
                        "Jump to previous prompt",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.prev_prompt,
                        "",
                    ),
                    (
                        "next_prompt",
                        "Jump to next prompt",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.next_prompt,
                        "",
                    ),
                    (
                        "export_scrollback",
                        "Export scrollback to file",
                        |s: &mut terminale_config::ShortcutsConfig| &mut s.export_scrollback,
                        "",
                    ),
                ],
            ),
        ];

        for (group_title, rows) in groups {
            ui.label(
                egui::RichText::new(*group_title)
                    .strong()
                    .color(egui::Color32::from_rgb(140, 160, 200)),
            );
            ui.add_space(4.0);
            card(ui, |ui| {
                for (id, label, getter, default) in *rows {
                    let hr = ui.horizontal(|ui| {
                        field_label(ui, label);
                        let binding = getter(&mut self.config.keybinds.shortcuts);
                        if hotkey_recorder(ui, id, binding, &mut self.recording_hotkey) {
                            dirty = true;
                        }
                        if ui.small_button("Reset").clicked() {
                            let b = getter(&mut self.config.keybinds.shortcuts);
                            *b = (*default).to_string();
                            dirty = true;
                        }
                        if ui.small_button("Clear").clicked() {
                            let b = getter(&mut self.config.keybinds.shortcuts);
                            b.clear();
                            dirty = true;
                        }
                    });
                    self.highlight_row(ui, hr.response.rect, Section::Shortcuts, label);
                }
            });
            ui.add_space(10.0);
        }

        if dirty {
            self.dirty = true;
        }

        // ── Custom multi-action keybinds ─────────────────────────────────────
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new("Custom keybinds")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Each entry maps a combo to an ordered list of actions. \
                 Named actions (e.g. NewTab, Copy) run the built-in command; \
                 entries starting with \"send:\" write bytes to the active pane \
                 (\\n, \\r, \\t, \\e, \\\\, \\xNN are decoded).",
            )
            .weak()
            .small(),
        );
        ui.add_space(6.0);

        let custom = &mut self.config.keybinds.custom;
        let mut remove_idx: Option<usize> = None;

        card(ui, |ui| {
            if custom.is_empty() {
                ui.label(
                    egui::RichText::new("No custom keybinds configured.")
                        .weak()
                        .italics(),
                );
            }

            for (idx, bind) in custom.iter_mut().enumerate() {
                ui.push_id(idx, |ui| {
                    ui.horizontal(|ui| {
                        // Remove button on the left.
                        if ui
                            .small_button(
                                egui::RichText::new("✕").color(egui::Color32::from_rgb(200, 80, 80)),
                            )
                            .on_hover_text("Remove this keybind")
                            .clicked()
                        {
                            remove_idx = Some(idx);
                            dirty = true;
                        }

                        // Keys combo field.
                        ui.label(
                            egui::RichText::new("Keys")
                                .small()
                                .color(egui::Color32::from_rgb(160, 170, 200)),
                        );
                        let kb_id = format!("custom_keys_{idx}");
                        if hotkey_recorder(ui, &kb_id, &mut bind.keys, &mut self.recording_hotkey)
                        {
                            dirty = true;
                        }
                    });

                    // Actions list (one per row).
                    let mut remove_action: Option<usize> = None;
                    for (aidx, action) in bind.actions.iter_mut().enumerate() {
                        ui.push_id(aidx, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0); // indent under the bind header
                                if ui
                                    .small_button(
                                        egui::RichText::new("✕")
                                            .color(egui::Color32::from_rgb(200, 80, 80)),
                                    )
                                    .on_hover_text("Remove this action")
                                    .clicked()
                                {
                                    remove_action = Some(aidx);
                                    dirty = true;
                                }
                                ui.label(
                                    egui::RichText::new(format!("{}", aidx + 1))
                                        .small()
                                        .color(egui::Color32::from_rgb(120, 130, 160)),
                                );
                                let terminale_config::KeyActionSpec::Action(ref mut s) = action;
                                let r = ui.add(
                                    egui::TextEdit::singleline(s)
                                        .hint_text("e.g. NewTab or send:\\n")
                                        .desired_width(280.0),
                                );
                                if r.changed() {
                                    dirty = true;
                                }
                            });
                        });
                    }
                    if let Some(ai) = remove_action {
                        bind.actions.remove(ai);
                        dirty = true;
                    }

                    // "+ Add action" row.
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        if ui.small_button("+ Add action").clicked() {
                            bind.actions.push(terminale_config::KeyActionSpec::Action(
                                String::new(),
                            ));
                            dirty = true;
                        }
                    });

                    ui.separator();
                });
            }

            // "+ Add keybind" row.
            ui.horizontal(|ui| {
                if ui.button("+ Add custom keybind").clicked() {
                    custom.push(terminale_config::CustomKeybind {
                        keys: String::new(),
                        actions: vec![terminale_config::KeyActionSpec::Action(String::new())],
                    });
                    dirty = true;
                }
            });
        });

        if let Some(ri) = remove_idx {
            self.config.keybinds.custom.remove(ri);
        }

        if dirty {
            self.dirty = true;
        }

        // ── Modal key-tables ─────────────────────────────────────────────────
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("Key tables (leader mode)")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Each key table is activated by a leader combo. \
                 While active, the next key dispatches its action list. \
                 Esc or a configurable timeout exits the mode.",
            )
            .weak()
            .small(),
        );
        ui.add_space(6.0);

        let mut remove_table: Option<usize> = None;
        let mut add_table = false;
        // Mutation lists collected during the closure and applied after.
        // (add_binding[table_idx], remove_binding[(table_idx, binding_idx)])
        let mut add_binding_to: Option<usize> = None;
        let mut remove_binding_from: Option<(usize, usize)> = None;

        card(ui, |ui| {
            let tables = &mut self.config.keybinds.key_tables;
            if tables.is_empty() {
                ui.label(
                    egui::RichText::new("No key tables configured.")
                        .weak()
                        .italics(),
                );
            }

            for (tidx, table) in tables.iter_mut().enumerate() {
                ui.push_id(tidx, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button(
                                egui::RichText::new("✕")
                                    .color(egui::Color32::from_rgb(200, 80, 80)),
                            )
                            .on_hover_text("Remove this key table")
                            .clicked()
                        {
                            remove_table = Some(tidx);
                            dirty = true;
                        }
                        field_label(ui, "Name");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut table.name)
                                .hint_text("e.g. pane")
                                .desired_width(120.0),
                        );
                        if r.changed() {
                            dirty = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        field_label(ui, "Leader combo");
                        let kb_id = format!("kt_leader_{tidx}");
                        if hotkey_recorder(
                            ui,
                            &kb_id,
                            &mut table.leader,
                            &mut self.recording_hotkey,
                        ) {
                            dirty = true;
                        }
                    });

                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        field_label(ui, "Timeout (ms)");
                        let mut t = table.timeout_ms;
                        let r = ui.add(
                            egui::Slider::new(&mut t, 100_u32..=30_000_u32).text("ms"),
                        );
                        if r.changed() {
                            table.timeout_ms = t;
                            dirty = true;
                        }
                    });

                    // Bindings list.
                    for (bidx, binding) in table.bindings.iter_mut().enumerate() {
                        ui.push_id(bidx, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                if ui
                                    .small_button(
                                        egui::RichText::new("✕")
                                            .color(egui::Color32::from_rgb(200, 80, 80)),
                                    )
                                    .on_hover_text("Remove this binding")
                                    .clicked()
                                {
                                    remove_binding_from = Some((tidx, bidx));
                                    dirty = true;
                                }
                                field_label(ui, "Key");
                                let kb_id = format!("kt_key_{tidx}_{bidx}");
                                if hotkey_recorder(
                                    ui,
                                    &kb_id,
                                    &mut binding.key,
                                    &mut self.recording_hotkey,
                                ) {
                                    dirty = true;
                                }
                                field_label(ui, "Actions");
                                // Show actions as a comma-joined editable string for
                                // compactness; parse back on change.
                                let mut actions_str: String = binding
                                    .actions
                                    .iter()
                                    .map(|a| {
                                        let terminale_config::KeyActionSpec::Action(s) = a;
                                        s.as_str()
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut actions_str)
                                        .hint_text("e.g. SplitRight, send:ls\\n")
                                        .desired_width(220.0),
                                );
                                if r.changed() {
                                    binding.actions = actions_str
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|s| !s.is_empty())
                                        .map(|s| {
                                            terminale_config::KeyActionSpec::Action(
                                                s.to_string(),
                                            )
                                        })
                                        .collect();
                                    dirty = true;
                                }
                            });
                        });
                    }

                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        if ui.small_button("+ Add binding").clicked() {
                            add_binding_to = Some(tidx);
                            dirty = true;
                        }
                    });

                    ui.separator();
                });
            }

            ui.horizontal(|ui| {
                if ui.button("+ Add key table").clicked() {
                    add_table = true;
                    dirty = true;
                }
            });
        });

        // Apply deferred mutations outside the closure so borrow checker is happy.
        if let Some(ri) = remove_table {
            self.config.keybinds.key_tables.remove(ri);
        }
        if let Some((ti, bi)) = remove_binding_from {
            if let Some(t) = self.config.keybinds.key_tables.get_mut(ti) {
                t.bindings.remove(bi);
            }
        }
        if let Some(ti) = add_binding_to {
            if let Some(t) = self.config.keybinds.key_tables.get_mut(ti) {
                t.bindings.push(terminale_config::KeyTableEntry {
                    key: String::new(),
                    actions: vec![terminale_config::KeyActionSpec::Action(String::new())],
                });
            }
        }
        if add_table {
            self.config.keybinds.key_tables.push(terminale_config::KeyTable {
                name: String::new(),
                leader: String::new(),
                timeout_ms: 1500,
                bindings: Vec::new(),
            });
        }

        if dirty {
            self.dirty = true;
        }

        // ── Custom mouse bindings ────────────────────────────────────────────
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("Custom mouse bindings")
                .strong()
                .color(egui::Color32::from_rgb(140, 160, 200)),
        );
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "Map a mouse button + modifiers + click count to an action sequence. \
                 Matching bindings run their actions and suppress the built-in behaviour \
                 for that press. No entries = all default mouse behaviour is preserved.",
            )
            .weak()
            .small(),
        );
        ui.add_space(6.0);

        let mouse_bindings = &mut self.config.keybinds.mouse;
        let mut remove_mouse_idx: Option<usize> = None;
        let mut mouse_dirty = false;

        card(ui, |ui| {
            if mouse_bindings.is_empty() {
                ui.label(
                    egui::RichText::new("No custom mouse bindings configured.")
                        .weak()
                        .italics(),
                );
            }

            const BUTTON_OPTIONS: &[&str] =
                &["Left", "Right", "Middle", "Back", "Forward"];

            for (idx, bind) in mouse_bindings.iter_mut().enumerate() {
                ui.push_id(idx, |ui| {
                    ui.horizontal(|ui| {
                        // Remove button.
                        if ui
                            .small_button(
                                egui::RichText::new("✕")
                                    .color(egui::Color32::from_rgb(200, 80, 80)),
                            )
                            .on_hover_text("Remove this mouse binding")
                            .clicked()
                        {
                            remove_mouse_idx = Some(idx);
                            mouse_dirty = true;
                        }

                        // Button dropdown.
                        ui.label(
                            egui::RichText::new("Button")
                                .small()
                                .color(egui::Color32::from_rgb(160, 170, 200)),
                        );
                        let selected = BUTTON_OPTIONS
                            .iter()
                            .position(|b| b.eq_ignore_ascii_case(&bind.button))
                            .unwrap_or(0);
                        egui::ComboBox::from_id_salt(format!("mb_btn_{idx}"))
                            .selected_text(
                                BUTTON_OPTIONS
                                    .get(selected)
                                    .copied()
                                    .unwrap_or("Left"),
                            )
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                for opt in BUTTON_OPTIONS {
                                    if ui
                                        .selectable_label(
                                            bind.button.eq_ignore_ascii_case(opt),
                                            *opt,
                                        )
                                        .clicked()
                                    {
                                        bind.button = (*opt).to_string();
                                        mouse_dirty = true;
                                    }
                                }
                            });

                        // Modifiers text field.
                        ui.label(
                            egui::RichText::new("Mods")
                                .small()
                                .color(egui::Color32::from_rgb(160, 170, 200)),
                        );
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut bind.mods)
                                .hint_text("e.g. Ctrl+Shift or empty")
                                .desired_width(140.0),
                        );
                        if r.changed() {
                            mouse_dirty = true;
                        }

                        // Click count selector.
                        ui.label(
                            egui::RichText::new("Clicks")
                                .small()
                                .color(egui::Color32::from_rgb(160, 170, 200)),
                        );
                        let count_labels = ["Single", "Double", "Triple"];
                        let count_idx = (bind.count.saturating_sub(1) as usize).min(2);
                        egui::ComboBox::from_id_salt(format!("mb_cnt_{idx}"))
                            .selected_text(count_labels[count_idx])
                            .width(76.0)
                            .show_ui(ui, |ui| {
                                for (i, label) in count_labels.iter().enumerate() {
                                    if ui
                                        .selectable_label(count_idx == i, *label)
                                        .clicked()
                                    {
                                        bind.count = (i as u8) + 1;
                                        mouse_dirty = true;
                                    }
                                }
                            });
                    });

                    // Actions list.
                    let mut remove_action: Option<usize> = None;
                    for (aidx, action) in bind.actions.iter_mut().enumerate() {
                        ui.push_id(aidx, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(24.0);
                                if ui
                                    .small_button(
                                        egui::RichText::new("✕")
                                            .color(egui::Color32::from_rgb(200, 80, 80)),
                                    )
                                    .on_hover_text("Remove this action")
                                    .clicked()
                                {
                                    remove_action = Some(aidx);
                                    mouse_dirty = true;
                                }
                                ui.label(
                                    egui::RichText::new(format!("{}", aidx + 1))
                                        .small()
                                        .color(egui::Color32::from_rgb(120, 130, 160)),
                                );
                                let terminale_config::KeyActionSpec::Action(ref mut s) = action;
                                let r = ui.add(
                                    egui::TextEdit::singleline(s)
                                        .hint_text("e.g. Paste or send:\\n")
                                        .desired_width(280.0),
                                );
                                if r.changed() {
                                    mouse_dirty = true;
                                }
                            });
                        });
                    }
                    if let Some(ai) = remove_action {
                        bind.actions.remove(ai);
                        mouse_dirty = true;
                    }

                    // "+ Add action" row.
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        if ui.small_button("+ Add action").clicked() {
                            bind.actions
                                .push(terminale_config::KeyActionSpec::Action(String::new()));
                            mouse_dirty = true;
                        }
                    });

                    ui.separator();
                });
            }

            ui.horizontal(|ui| {
                if ui.button("+ Add mouse binding").clicked() {
                    mouse_bindings.push(terminale_config::MouseBinding {
                        button: "Right".to_string(),
                        mods: String::new(),
                        count: 1,
                        actions: vec![terminale_config::KeyActionSpec::Action(String::new())],
                    });
                    mouse_dirty = true;
                }
            });
        });

        if let Some(ri) = remove_mouse_idx {
            self.config.keybinds.mouse.remove(ri);
        }

        if mouse_dirty {
            self.dirty = true;
        }
    }
}