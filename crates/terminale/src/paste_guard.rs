//! Paste-safety guard: multi-line / control-char paste confirmation modal.
//!
//! When the user pastes text that contains a newline AND the safety policy
//! requires confirmation (either `paste_confirm_multiline` is on, or
//! `paste_confirm_when_unbracketed` is on and the focused program has NOT
//! enabled bracketed paste), we show this popup before sending anything to
//! the PTY.
//!
//! The dialog follows the same windowed-popup pattern as
//! [`crate::password_prompt::PasswordPrompt`]: its own winit + egui + wgpu
//! window, floated over the terminal, closed on Confirm / Cancel / Esc /
//! focus-loss.

use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiState;
use std::sync::Arc;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

// ── PasteGuardOutcome ─────────────────────────────────────────────────────────

/// The user's response once the confirmation dialog closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteGuardOutcome {
    /// The user confirmed; send the (possibly stripped) payload.
    Confirm,
    /// The user cancelled; discard the clipboard contents.
    Cancel,
}

// ── paste_needs_confirm ───────────────────────────────────────────────────────

/// Decide whether the paste-guard dialog should be shown before sending
/// `text` to the PTY.
///
/// Returns `true` when all of the following hold:
/// - the text contains a newline character (`\n`, or `\r` after CRLF→LF
///   normalisation), AND
/// - at least one of the policy conditions is met:
///   - `paste_confirm_multiline` is on (confirm for every multi-line paste), OR
///   - `paste_confirm_when_unbracketed` is on AND `bracketed_active` is `false`
///     (the focused program has NOT enabled bracketed paste — the dangerous case).
///
/// This is a pure function — all state comes in via arguments, making it
/// trivially unit-testable.
#[must_use]
pub fn paste_needs_confirm(
    text: &str,
    bracketed_active: bool,
    confirm_multiline: bool,
    confirm_when_unbracketed: bool,
) -> bool {
    let is_multiline = text.contains('\n') || text.contains('\r');
    if !is_multiline {
        return false;
    }
    confirm_multiline || (confirm_when_unbracketed && !bracketed_active)
}

// ── strip_control_chars ───────────────────────────────────────────────────────

/// Strip non-printable control bytes from `text`, keeping `\n`, `\t`, and
/// `\r` (the "safe" whitespace bytes that every terminal expects to see).
///
/// Any byte in the ranges `0x00..=0x08`, `0x0B..=0x0C`, `0x0E..=0x1F`, and
/// `0x7F` (DEL) is dropped. This removes ESC, NUL, BEL, SO, SI, and other
/// bytes that could trigger unintended terminal state changes when pasted.
#[must_use]
pub fn strip_control_chars(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            // Keep printable chars and the three safe whitespace bytes.
            !c.is_control() || matches!(c, '\n' | '\t' | '\r')
        })
        .collect()
}

// ── build_paste_preview ───────────────────────────────────────────────────────

/// Build the preview shown in the confirmation dialog body.
///
/// Shows up to `max_lines` lines of `text`, followed by a summary line with
/// the total line and character counts. Lines beyond `max_lines` are hidden
/// (replaced by `"…and N more lines"`).
#[must_use]
pub fn build_paste_preview(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let total_lines = lines.len();
    let total_chars = text.chars().count();

    let shown: Vec<&str> = lines.iter().take(max_lines).copied().collect();
    let hidden = total_lines.saturating_sub(max_lines);

    let mut out = shown.join("\n");
    if hidden > 0 {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "…and {hidden} more line{}",
            if hidden == 1 { "" } else { "s" }
        ));
    }
    // Append the summary footer.
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!(
        "── {total_lines} line{}, {total_chars} character{} ──",
        if total_lines == 1 { "" } else { "s" },
        if total_chars == 1 { "" } else { "s" },
    ));
    out
}

// ── PasteGuardDialog ─────────────────────────────────────────────────────────

const WIN_WIDTH: f32 = 500.0;
const WIN_HEIGHT: f32 = 310.0;
const PREVIEW_LINES: usize = 6;

/// A modal-ish popup asking the user to confirm or cancel a pending paste.
pub struct PasteGuardDialog {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,

    /// The text that would be pasted (used for preview and final dispatch).
    text: String,
    /// Whether the focused program has bracketed paste enabled (context only
    /// — shown in the warning text).
    bracketed: bool,
    /// Preview string shown in the dialog body (pre-built, cached).
    preview: String,

    /// Set once the user responds.
    outcome: Option<PasteGuardOutcome>,
    /// True once the window should be torn down.
    requested_close: bool,
    /// Set so the "Paste" button grabs focus on the first frame.
    first_frame: bool,
}

impl PasteGuardDialog {
    /// Open the paste-guard confirmation dialog centred over `parent`.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        parent: &Window,
        text: String,
        bracketed: bool,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Self {
        let preview = build_paste_preview(&text, PREVIEW_LINES);

        let scale = parent.scale_factor();
        let parent_pos = parent
            .outer_position()
            .unwrap_or(winit::dpi::PhysicalPosition::new(0, 0));
        let parent_size = parent.outer_size();
        let win_w_px = WIN_WIDTH * scale as f32;
        let win_h_px = WIN_HEIGHT * scale as f32;
        let pos = winit::dpi::PhysicalPosition::new(
            parent_pos.x + ((parent_size.width as f32 - win_w_px) / 2.0) as i32,
            parent_pos.y + ((parent_size.height as f32 - win_h_px) / 2.0) as i32,
        );

        let attrs = Window::default_attributes()
            .with_title("terminale — paste confirmation")
            .with_inner_size(LogicalSize::new(WIN_WIDTH, WIN_HEIGHT))
            .with_decorations(false)
            .with_resizable(false)
            .with_position(pos)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_visible(false);
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_skip_taskbar(true)
        };
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create paste guard window"),
        );

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("paste guard surface");

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
                )
            })
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let egui_ctx = egui::Context::default();
        configure_visuals(&egui_ctx);
        // Install Hack as a Proportional fallback so geometric/arrow icons
        // (↑ ↓ ▲ ▼ ⊕ ● etc.) never render as tofu in this window.
        crate::egui_icons::install_icon_font(&egui_ctx);
        let viewport_id = egui_ctx.viewport_id();
        let egui_state = EguiState::new(
            egui_ctx.clone(),
            viewport_id,
            &*window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = EguiRenderer::new(&device, format, None, 1, false);

        let mut this = Self {
            window,
            surface,
            surface_config,
            device,
            queue,
            egui_ctx,
            egui_state,
            egui_renderer,
            text,
            bracketed,
            preview,
            outcome: None,
            requested_close: false,
            first_frame: true,
        };

        this.render_frame();

        #[cfg(windows)]
        set_dwm_cloak(&this.window, true);
        this.window.set_visible(true);
        #[cfg(windows)]
        set_dwm_cloak(&this.window, false);
        this.window.focus_window();

        this
    }

    /// Stable OS window id.
    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Take the user's outcome (one-shot). `None` until the user responds.
    pub fn take_outcome(&mut self) -> Option<PasteGuardOutcome> {
        self.outcome.take()
    }

    /// The full text of the pending paste, for use after confirmation.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Handle one winit event. Returns `true` when the dialog should be dropped.
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::Focused(false) => {
                // Click-outside cancels (treats as cancel, not confirm — safer).
                self.outcome = Some(PasteGuardOutcome::Cancel);
                self.requested_close = true;
            }
            WindowEvent::KeyboardInput {
                event:
                    winit::event::KeyEvent {
                        logical_key: Key::Named(NamedKey::Escape),
                        state: winit::event::ElementState::Pressed,
                        ..
                    },
                ..
            } => {
                self.outcome = Some(PasteGuardOutcome::Cancel);
                self.requested_close = true;
            }
            _ => {}
        }

        // Don't let `RedrawRequested` re-arm itself: egui-winit reports
        // repaint == true for it, so calling request_redraw here would spin a
        // ~60 fps repaint loop while the window merely sits open. The paint
        // happens in the RedrawRequested arm; render schedules its own
        // follow-up repaint when an animation actually needs one.
        let response = self.egui_state.on_window_event(&self.window, event);
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            self.window.request_redraw();
        }

        if matches!(event, WindowEvent::RedrawRequested) {
            self.render_frame();
        }

        // Outcome set → close after this event is processed so the App can read
        // the outcome on the same tick (same pattern as PasswordPrompt).
        if self.outcome.is_some() {
            return false;
        }

        self.requested_close
    }

    fn render_frame(&mut self) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_ctx.clone();
        let full_output = ctx.run(raw_input, |ctx| self.build_ui(ctx));

        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }
        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        let screen = ScreenDescriptor {
            size_in_pixels: [self.surface_config.width, self.surface_config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("paste guard encoder"),
            });

        self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &paint_jobs,
            &screen,
        );

        let frame = if let Ok(f) = self.surface.get_current_texture() {
            f
        } else {
            self.surface.configure(&self.device, &self.surface_config);
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("paste guard pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.043,
                                g: 0.051,
                                b: 0.071,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &paint_jobs, &screen);
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
    }

    fn build_ui(&mut self, ctx: &egui::Context) {
        let mut confirm = false;
        let mut cancel = false;

        let frame = egui::Frame::default()
            .fill(egui::Color32::from_rgb(13, 15, 22))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 48, 70)))
            .inner_margin(egui::Margin::symmetric(22.0, 18.0));

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            // Title
            ui.label(
                egui::RichText::new("Confirm paste")
                    .size(17.0)
                    .strong()
                    .color(egui::Color32::from_rgb(225, 232, 250)),
            );
            ui.add_space(2.0);

            // Warning line
            let warn_text = if self.bracketed {
                "Multi-line paste — application has bracketed paste enabled."
            } else {
                "Multi-line paste — application does NOT have bracketed paste enabled. \
                 A newline may immediately execute the command."
            };
            let warn_color = if self.bracketed {
                egui::Color32::from_rgb(150, 160, 190)
            } else {
                egui::Color32::from_rgb(240, 160, 60)
            };
            ui.label(egui::RichText::new(warn_text).small().color(warn_color));
            ui.add_space(10.0);

            // Preview box
            egui::Frame::default()
                .fill(egui::Color32::from_rgb(20, 24, 36))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(50, 60, 90)))
                .rounding(6.0)
                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(140.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&self.preview)
                                        .monospace()
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(200, 210, 230)),
                                )
                                .wrap(),
                            );
                        });
                });

            ui.add_space(14.0);

            // Buttons
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Confirm button — primary action
                    let paste_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("  Paste  ")
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(60, 110, 230))
                        .min_size(egui::vec2(0.0, 32.0))
                        .rounding(8.0),
                    );
                    if self.first_frame {
                        paste_btn.request_focus();
                        self.first_frame = false;
                    }
                    if paste_btn.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                        confirm = true;
                    }
                    ui.add_space(8.0);
                    // Cancel button
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("  Cancel  ")
                                    .color(egui::Color32::from_rgb(200, 208, 228)),
                            )
                            .fill(egui::Color32::from_rgb(30, 36, 52))
                            .min_size(egui::vec2(0.0, 32.0))
                            .rounding(8.0),
                        )
                        .clicked()
                    {
                        cancel = true;
                    }
                });
            });
        });

        if confirm {
            self.outcome = Some(PasteGuardOutcome::Confirm);
            self.requested_close = true;
        } else if cancel {
            self.outcome = Some(PasteGuardOutcome::Cancel);
            self.requested_close = true;
        }
    }
}

// ── platform helpers ──────────────────────────────────────────────────────────

#[cfg(windows)]
fn set_dwm_cloak(window: &Window, cloaked: bool) {
    use std::ffi::c_void;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(
            hwnd: *mut c_void,
            dwAttribute: u32,
            pvAttribute: *const c_void,
            cbAttribute: u32,
        ) -> i32;
    }
    const DWMWA_CLOAK: u32 = 13;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = h.hwnd.get() as *mut c_void;
    let value: i32 = i32::from(cloaked);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CLOAK,
            std::ptr::from_ref::<i32>(&value) as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

fn configure_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = egui::Color32::from_rgb(13, 15, 22);
    style.visuals.panel_fill = egui::Color32::from_rgb(13, 15, 22);
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(225, 232, 250));
    ctx.set_style(style);
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── paste_needs_confirm ───────────────────────────────────────────────────

    #[test]
    fn single_line_never_needs_confirm() {
        // Single-line text never triggers confirmation, regardless of config.
        assert!(!paste_needs_confirm("hello", false, true, true));
        assert!(!paste_needs_confirm("hello", true, true, true));
        assert!(!paste_needs_confirm("hello world", false, false, false));
    }

    #[test]
    fn multiline_confirm_multiline_flag() {
        // confirm_multiline=true triggers for multi-line, regardless of bracketed.
        assert!(paste_needs_confirm("line1\nline2", true, true, false));
        assert!(paste_needs_confirm("line1\nline2", false, true, false));
        assert!(paste_needs_confirm("line1\nline2", true, true, true));
    }

    #[test]
    fn multiline_confirm_when_unbracketed_active() {
        // confirm_when_unbracketed=true only triggers when bracketed is OFF.
        assert!(paste_needs_confirm("line1\nline2", false, false, true));
        // Bracketed paste ON → no confirm (application handles it safely).
        assert!(!paste_needs_confirm("line1\nline2", true, false, true));
    }

    #[test]
    fn multiline_both_flags_off_no_confirm() {
        // Neither confirm flag set → never confirm, even multi-line.
        assert!(!paste_needs_confirm("line1\nline2", false, false, false));
        assert!(!paste_needs_confirm("line1\nline2", true, false, false));
    }

    #[test]
    fn carriage_return_counts_as_multiline() {
        // \r without \n still counts as a line-ending for safety.
        assert!(paste_needs_confirm("line1\rline2", false, false, true));
    }

    #[test]
    fn empty_string_never_needs_confirm() {
        assert!(!paste_needs_confirm("", false, true, true));
    }

    // ── strip_control_chars ───────────────────────────────────────────────────

    #[test]
    fn strips_esc_and_nul() {
        let input = "hello\x1b[31mworld\x00";
        let out = strip_control_chars(input);
        // ESC (0x1b) and NUL (0x00) must be removed; printable chars kept.
        assert_eq!(out, "hello[31mworld");
    }

    #[test]
    fn keeps_newline_tab_cr() {
        let input = "line1\nline2\ttabbed\r\n";
        let out = strip_control_chars(input);
        assert_eq!(out, input, "\\n, \\t, \\r must be preserved");
    }

    #[test]
    fn keeps_printable_unicode() {
        let input = "Ciao\u{1F600}mondo";
        let out = strip_control_chars(input);
        assert_eq!(out, input);
    }

    #[test]
    fn strips_del_0x7f() {
        let input = "abc\x7fdef";
        let out = strip_control_chars(input);
        assert_eq!(out, "abcdef");
    }

    #[test]
    fn no_op_on_clean_text() {
        let input = "Just a normal line.\n";
        assert_eq!(strip_control_chars(input), input);
    }

    // ── build_paste_preview ───────────────────────────────────────────────────

    #[test]
    fn preview_single_line() {
        let preview = build_paste_preview("hello", 6);
        assert!(preview.contains("hello"));
        assert!(preview.contains("1 line"));
        assert!(preview.contains("5 character"));
    }

    #[test]
    fn preview_exactly_max_lines() {
        let text = "a\nb\nc\nd\ne\nf";
        let preview = build_paste_preview(text, 6);
        // All 6 lines shown, no "…and N more" line.
        assert!(
            !preview.contains("…and"),
            "should not truncate at exactly max_lines"
        );
        assert!(preview.contains("6 lines"));
    }

    #[test]
    fn preview_truncates_beyond_max_lines() {
        let text = "a\nb\nc\nd\ne\nf\ng\nh";
        let preview = build_paste_preview(text, 6);
        assert!(preview.contains("…and 2 more lines"));
        assert!(preview.contains("8 lines"));
    }

    #[test]
    fn preview_one_more_line_singular() {
        let text = "a\nb\nc\nd\ne\nf\ng";
        let preview = build_paste_preview(text, 6);
        assert!(preview.contains("…and 1 more line"));
        assert!(!preview.contains("…and 1 more lines"));
    }

    #[test]
    fn preview_empty_string() {
        let preview = build_paste_preview("", 6);
        assert!(preview.contains("0 lines") || preview.contains("0 character"));
    }
}
