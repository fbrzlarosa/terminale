//! Inline AI assistant — a borderless egui sub-window that streams a
//! response from the configured provider (Claude / OpenAI / local Ollama)
//! and lets the user inject a suggested shell command into the active PTY.
//!
//! Architecture mirrors [`crate::settings_window`]: a separate winit
//! window with its own wgpu surface that *shares* the main renderer's
//! device/queue. The async provider call runs on a Tokio runtime; each
//! streamed chunk is forwarded to the winit loop via
//! [`crate::UserEvent::Ai`] so rendering stays on the main thread.

use crate::{AiEvent, UserEvent};
use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiState;
use std::sync::Arc;
use terminale_config::AiConfig;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// One message in the conversation transcript.
struct ChatMsg {
    role: Role,
    text: String,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Role {
    User,
    Assistant,
}

pub struct AiAssistantWindow {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,

    ai: AiConfig,
    proxy: EventLoopProxy<UserEvent>,
    rt: tokio::runtime::Handle,

    /// Conversation so far (oldest first).
    transcript: Vec<ChatMsg>,
    /// The current in-flight assistant answer being streamed.
    streaming: Option<String>,
    /// True while a request is in flight (input disabled).
    busy: bool,
    /// Last error, shown inline.
    error: Option<String>,
    /// The user's input box contents.
    input: String,
    /// Which provider is selected for this session (defaults to config).
    provider: String,

    /// Set when the user clicks "Inject" — the host reads this and writes
    /// the command into the active PTY, then clears it.
    inject_request: Option<String>,
    requested_close: bool,
    next_repaint: Option<std::time::Instant>,
    first_frame_done: bool,
}

impl AiAssistantWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        ai: AiConfig,
        proxy: EventLoopProxy<UserEvent>,
        rt: tokio::runtime::Handle,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        // When `Some`, the window opens with this prompt already submitted
        // (used by "Explain Selection" — the assistant starts answering
        // immediately instead of waiting for the user to type).
        initial_prompt: Option<String>,
    ) -> Self {
        let mut attrs = Window::default_attributes()
            .with_title("terminale — AI")
            .with_inner_size(winit::dpi::LogicalSize::new(560.0, 520.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(420.0, 360.0))
            .with_decorations(false)
            .with_visible(false);
        if let Some(icon) = crate::app_icon::load_app_icon() {
            attrs = attrs.with_window_icon(Some(icon));
        }
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create AI window"),
        );

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("ai surface");
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
        // Non-blocking present so dragging the window stays smooth (see the
        // settings window for the full rationale: AutoVsync + 1-frame latency
        // parks acquire on the compositor and makes drags drop frames).
        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            wgpu::PresentMode::Mailbox
        } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::AutoVsync
        };
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
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

        let provider = ai.default_provider.clone();
        let mut this = Self {
            window,
            surface,
            surface_config,
            device,
            queue,
            egui_ctx,
            egui_state,
            egui_renderer,
            ai,
            proxy,
            rt,
            transcript: Vec::new(),
            streaming: None,
            busy: false,
            error: None,
            input: String::new(),
            provider,
            inject_request: None,
            requested_close: false,
            next_repaint: None,
            first_frame_done: false,
        };
        this.render_frame();
        #[cfg(windows)]
        set_dwm_cloak(&this.window, true);
        this.window.set_visible(true);
        #[cfg(windows)]
        set_dwm_cloak(&this.window, false);
        this.window.focus_window();
        this.first_frame_done = true;
        // Auto-submit the seed prompt (e.g. "Explain this output: …") so the
        // assistant is already streaming an answer when the window appears.
        if let Some(prompt) = initial_prompt {
            this.input = prompt;
            this.ask();
        }
        this
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn next_repaint(&self) -> Option<std::time::Instant> {
        self.next_repaint
    }

    /// Apply an updated `AiConfig` from the settings live-apply path.
    ///
    /// This allows the `render_markdown` toggle (and future config fields) to
    /// take effect while the window is already open, without requiring the user
    /// to close and reopen the assistant.
    pub fn set_config(&mut self, ai: AiConfig) {
        self.ai = ai;
    }

    pub fn pump_repaint(&mut self) {
        if let Some(deadline) = self.next_repaint {
            if std::time::Instant::now() >= deadline {
                self.next_repaint = None;
                self.window.request_redraw();
            }
        }
    }

    /// Pull a pending command-injection request (one-shot).
    pub fn take_inject(&mut self) -> Option<String> {
        self.inject_request.take()
    }

    /// Feed a streamed AI event in from the host event loop.
    pub fn push_ai_event(&mut self, event: AiEvent) {
        match event {
            AiEvent::Chunk(text) => {
                self.streaming
                    .get_or_insert_with(String::new)
                    .push_str(&text);
            }
            AiEvent::Done => {
                if let Some(s) = self.streaming.take() {
                    if !s.trim().is_empty() {
                        self.transcript.push(ChatMsg {
                            role: Role::Assistant,
                            text: s,
                        });
                    }
                }
                self.busy = false;
            }
            AiEvent::Error(e) => {
                self.streaming = None;
                self.busy = false;
                self.error = Some(e);
            }
        }
        self.window.request_redraw();
    }

    /// Returns `true` when the window asked to close.
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        if matches!(event, WindowEvent::CloseRequested) {
            return true;
        }
        if let WindowEvent::KeyboardInput { event: ke, .. } = event {
            use winit::keyboard::{Key, NamedKey};
            if ke.state == winit::event::ElementState::Pressed
                && matches!(ke.logical_key, Key::Named(NamedKey::Escape))
            {
                return true;
            }
        }
        let response = self.egui_state.on_window_event(&self.window, event);
        if response.repaint {
            self.window.request_redraw();
        }
        if let WindowEvent::Resized(size) = event {
            self.surface_config.width = size.width.max(1);
            self.surface_config.height = size.height.max(1);
            self.surface.configure(&self.device, &self.surface_config);
            self.window.request_redraw();
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.render_frame();
        }
        if self.requested_close {
            self.requested_close = false;
            return true;
        }
        false
    }

    /// Kick off a streaming request for the current input.
    /// Submit `prompt` programmatically (e.g. "Explain Selection" when the
    /// window is already open). No-op while a previous request is streaming.
    pub fn submit_prompt(&mut self, prompt: String) {
        if self.busy {
            return;
        }
        self.input = prompt;
        self.ask();
        self.window.request_redraw();
    }

    fn ask(&mut self) {
        let prompt = self.input.trim().to_string();
        if prompt.is_empty() || self.busy {
            return;
        }
        self.input.clear();
        self.error = None;
        self.transcript.push(ChatMsg {
            role: Role::User,
            text: prompt.clone(),
        });
        self.streaming = Some(String::new());
        self.busy = true;

        // Resolve provider + credentials. Env vars beat config so secrets
        // never have to live in the TOML.
        let provider = self.provider.clone();
        let (secret, model, max_tokens) = match provider.as_str() {
            "openai" => (
                env_or(&self.ai.openai.api_key, "OPENAI_API_KEY"),
                self.ai.openai.model.clone(),
                Some(self.ai.openai.max_tokens),
            ),
            "ollama" => (String::new(), self.ai.ollama.model.clone(), None),
            _ => (
                env_or(&self.ai.claude.api_key, "ANTHROPIC_API_KEY"),
                self.ai.claude.model.clone(),
                Some(self.ai.claude.max_tokens),
            ),
        };
        let ollama_url = self.ai.ollama.url.clone();

        // Build the conversation with a terminal-focused system prompt so
        // command suggestions come back in fenced blocks we can extract.
        let mut messages = vec![terminale_ai::AiMessage::system(
            "You are a terminal assistant embedded in a shell. Be concise. \
             When you suggest a shell command, put ONLY the command inside a \
             fenced code block (```), no prose inside the block.",
        )];
        for m in &self.transcript {
            messages.push(match m.role {
                Role::User => terminale_ai::AiMessage::user(m.text.clone()),
                Role::Assistant => terminale_ai::AiMessage::assistant(m.text.clone()),
            });
        }
        let req = terminale_ai::AiRequest {
            model,
            messages,
            max_tokens,
            temperature: None,
        };

        let proxy = self.proxy.clone();
        self.rt.spawn(async move {
            let provider = terminale_ai::build_provider(&provider, secret, ollama_url);
            match provider.stream(req).await {
                Ok(mut rx) => {
                    while let Some(chunk) = rx.recv().await {
                        match chunk {
                            terminale_ai::StreamChunk::Text(t) => {
                                if proxy.send_event(UserEvent::Ai(AiEvent::Chunk(t))).is_err() {
                                    return;
                                }
                            }
                            terminale_ai::StreamChunk::Done => {
                                let _ = proxy.send_event(UserEvent::Ai(AiEvent::Done));
                                return;
                            }
                            terminale_ai::StreamChunk::Error(e) => {
                                let _ = proxy.send_event(UserEvent::Ai(AiEvent::Error(e)));
                                return;
                            }
                        }
                    }
                    let _ = proxy.send_event(UserEvent::Ai(AiEvent::Done));
                }
                Err(e) => {
                    let _ = proxy.send_event(UserEvent::Ai(AiEvent::Error(e.to_string())));
                }
            }
        });
    }

    fn render_frame(&mut self) {
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let ctx = self.egui_ctx.clone();
        let mut ask_clicked = false;
        let mut inject: Option<String> = None;
        let mut close = false;
        let mut start_drag = false;
        let full_output = ctx.run(raw_input, |ctx| {
            self.draw_ui(
                ctx,
                &mut ask_clicked,
                &mut inject,
                &mut close,
                &mut start_drag,
            );
        });
        if start_drag {
            // Non-modal at our level; lets you reposition the AI window.
            let _ = self.window.drag_window();
        }
        if ask_clicked {
            self.ask();
        }
        if let Some(cmd) = inject {
            self.inject_request = Some(cmd);
        }
        if close {
            self.requested_close = true;
        }

        let repaint_delay = full_output
            .viewport_output
            .values()
            .map(|v| v.repaint_delay)
            .min()
            .unwrap_or(std::time::Duration::MAX);
        if repaint_delay.is_zero() {
            self.next_repaint = None;
            self.window.request_redraw();
        } else if repaint_delay < std::time::Duration::from_secs(1) {
            self.next_repaint = std::time::Instant::now().checked_add(repaint_delay);
        } else {
            self.next_repaint = None;
        }

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
                label: Some("ai encoder"),
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
                    label: Some("ai pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.043,
                                g: 0.050,
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

    fn draw_ui(
        &mut self,
        ctx: &egui::Context,
        ask_clicked: &mut bool,
        inject: &mut Option<String>,
        close: &mut bool,
        start_drag: &mut bool,
    ) {
        // Title bar — a drag handle (to move the borderless window) plus the
        // provider picker and a close button. Mirrors the settings window.
        egui::TopBottomPanel::top("ai_title")
            .exact_height(34.0)
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(8, 10, 16))
                    .inner_margin(egui::Margin::symmetric(8.0, 0.0)),
            )
            .show(ctx, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 34.0),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        // "×" (U+00D7) is in the base font — unlike many glyph
                        // icons, it always renders rather than showing tofu.
                        if ui.button("×").clicked() {
                            *close = true;
                        }
                        egui::ComboBox::from_id_salt("ai_provider_pick")
                            .selected_text(provider_label(&self.provider))
                            .show_ui(ui, |ui| {
                                for p in ["claude", "openai", "ollama"] {
                                    ui.selectable_value(
                                        &mut self.provider,
                                        p.to_string(),
                                        provider_label(p),
                                    );
                                }
                            });
                        // The remaining width is the window-drag handle.
                        let drag = ui.allocate_response(
                            egui::vec2(ui.available_width(), 28.0),
                            egui::Sense::click_and_drag(),
                        );
                        if drag.is_pointer_button_down_on() {
                            *start_drag = true;
                        }
                        ui.painter().text(
                            egui::pos2(drag.rect.left() + 4.0, drag.rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            "AI assistant",
                            egui::FontId::new(13.0, egui::FontFamily::Proportional),
                            egui::Color32::from_rgb(200, 210, 235),
                        );
                    },
                );
            });

        // Input bar at the bottom.
        egui::TopBottomPanel::bottom("ai_input")
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(15, 17, 25))
                    .inner_margin(egui::Margin::symmetric(12.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let hint = if self.busy {
                        "Thinking…"
                    } else {
                        "Ask anything — e.g. \"find the 10 biggest files here\""
                    };
                    let edit = egui::TextEdit::multiline(&mut self.input)
                        .desired_rows(2)
                        .desired_width(ui.available_width() - 84.0)
                        .hint_text(hint)
                        .lock_focus(true);
                    let resp = ui.add_enabled(!self.busy, edit);
                    // Ctrl+Enter (or Enter without shift) submits.
                    if resp.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift)
                    {
                        *ask_clicked = true;
                    }
                    ui.vertical(|ui| {
                        if ui
                            .add_enabled(
                                !self.busy,
                                egui::Button::new(
                                    egui::RichText::new("Ask").color(egui::Color32::WHITE),
                                )
                                .min_size(egui::vec2(70.0, 28.0))
                                .fill(egui::Color32::from_rgb(60, 110, 230)),
                            )
                            .clicked()
                        {
                            *ask_clicked = true;
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "{} · {}",
                                provider_label(&self.provider),
                                self.model_for_display()
                            ))
                            .small()
                            .color(egui::Color32::from_rgb(110, 120, 150)),
                        );
                    });
                });
            });

        // Conversation.
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(egui::Color32::from_rgb(11, 13, 20))
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if self.transcript.is_empty() && self.streaming.is_none() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(
                                        "Ask the assistant for a command or an explanation.\nSuggested commands get an Inject button.",
                                    )
                                    .color(egui::Color32::from_rgb(120, 130, 160)),
                                );
                            });
                        }
                        let render_md = self.ai.render_markdown;
                        for msg in &self.transcript {
                            bubble(ui, msg.role, &msg.text, inject, render_md);
                        }
                        if let Some(s) = &self.streaming {
                            bubble(ui, Role::Assistant, s, inject, render_md);
                        }
                        if let Some(e) = &self.error {
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(format!("⚠ {e}"))
                                    .color(egui::Color32::from_rgb(240, 120, 120)),
                            );
                        }
                    });
            });
    }

    fn model_for_display(&self) -> String {
        match self.provider.as_str() {
            "openai" => self.ai.openai.model.clone(),
            "ollama" => self.ai.ollama.model.clone(),
            _ => self.ai.claude.model.clone(),
        }
    }
}

/// Render one chat bubble. If the assistant message contains a command,
/// show an Inject button that hands it back to the host.
///
/// `render_markdown` controls whether assistant replies are rendered as
/// formatted markdown or as raw plain text (mirrors `ai.render_markdown`).
/// User bubbles always use plain text — they are the user's raw input.
fn bubble(
    ui: &mut egui::Ui,
    role: Role,
    text: &str,
    inject: &mut Option<String>,
    render_markdown: bool,
) {
    let (label, color) = match role {
        Role::User => ("You", egui::Color32::from_rgb(120, 150, 230)),
        Role::Assistant => ("AI", egui::Color32::from_rgb(150, 210, 160)),
    };
    ui.add_space(8.0);
    ui.label(egui::RichText::new(label).small().strong().color(color));
    if role == Role::Assistant {
        crate::markdown::render(ui, text, render_markdown);
    } else {
        ui.label(egui::RichText::new(text).color(egui::Color32::from_rgb(220, 226, 240)));
    }
    if role == Role::Assistant {
        if let Some(cmd) = extract_command(text) {
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            // U+2B07 down arrow — covered by the bundled
                            // NotoEmoji. U+23CE (return symbol) is in none
                            // of egui's fonts and rendered as a tofu box.
                            egui::RichText::new(format!(
                                "\u{2B07}  Inject:  {}",
                                truncate(&cmd, 48)
                            ))
                            .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(40, 90, 60))
                        .rounding(6.0),
                    )
                    .on_hover_text("Type this command into the active terminal")
                    .clicked()
                {
                    *inject = Some(cmd.clone());
                }
                if ui.small_button("Copy").clicked() {
                    ui.output_mut(|o| o.copied_text = cmd.clone());
                }
            });
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let t: String = s.chars().take(n).collect();
        format!("{t}…")
    }
}

/// Strip a leading shell-prompt indicator (`$ `, `> `, or a PowerShell
/// `PS …> ` prompt) from a line. Models routinely echo the prompt inside
/// code blocks (`$ ls`, `PS C:\> dir`); injecting that verbatim would make
/// the shell choke on the literal `$`/`PS`. Returns the bare command.
fn strip_shell_prompt(line: &str) -> &str {
    let t = line.trim();
    // PowerShell prompt: "PS C:\Users\x> cmd" or "PS> cmd".
    if t.starts_with("PS ") || t.starts_with("PS>") {
        if let Some(idx) = t.find("> ") {
            return t[idx + 2..].trim_start();
        }
    }
    // POSIX user prompt "$ cmd" or a continuation "> cmd".
    for p in ["$ ", "> "] {
        if let Some(rest) = t.strip_prefix(p) {
            return rest.trim_start();
        }
    }
    t
}

/// Extract a shell command from assistant text: prefer the first fenced
/// code block's content; else the first line that looks like a prompt. Any
/// shell-prompt prefix the model echoed on the command line is stripped.
fn extract_command(text: &str) -> Option<String> {
    // Fenced block.
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        // Skip an optional language tag up to the first newline.
        let body_start = after.find('\n').map_or(0, |i| i + 1);
        let body = &after[body_start..];
        if let Some(end) = body.find("```") {
            let cmd = body[..end].trim();
            if !cmd.is_empty() {
                // First non-empty line of the block, minus any echoed prompt.
                if let Some(first) = cmd.lines().find(|l| !l.trim().is_empty()) {
                    let stripped = strip_shell_prompt(first);
                    if !stripped.is_empty() {
                        return Some(stripped.to_string());
                    }
                }
            }
        }
    }
    // Prompt-style line.
    for line in text.lines() {
        let t = line.trim();
        for p in ["$ ", "# ", "> "] {
            if let Some(rest) = t.strip_prefix(p) {
                if !rest.trim().is_empty() {
                    return Some(rest.trim().to_string());
                }
            }
        }
    }
    None
}

fn provider_label(p: &str) -> &'static str {
    match p {
        "openai" => "OpenAI",
        "ollama" => "Ollama",
        _ => "Claude",
    }
}

fn env_or(config_value: &str, var: &str) -> String {
    if !config_value.trim().is_empty() {
        return config_value.to_string();
    }
    std::env::var(var).unwrap_or_default()
}

#[cfg(windows)]
fn set_dwm_cloak(window: &Window, cloaked: bool) {
    use std::ffi::c_void;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: *mut c_void, attr: u32, val: *const c_void, sz: u32) -> i32;
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
    style.visuals.window_fill = egui::Color32::from_rgb(11, 13, 20);
    style.visuals.panel_fill = egui::Color32::from_rgb(11, 13, 20);
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(220, 226, 240));
    style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(70, 100, 170);
    style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(80, 120, 200);
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(60, 110, 230);
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::{extract_command, strip_shell_prompt};

    #[test]
    fn extracts_command_from_fenced_block() {
        let t = "Sure, try:\n```bash\nls -la\n```\nThat lists the files.";
        assert_eq!(extract_command(t).as_deref(), Some("ls -la"));
    }

    #[test]
    fn strips_dollar_prompt_inside_fence() {
        // The classic bug: a model echoes the `$` prompt in the code block.
        let t = "```sh\n$ cargo build --release\n```";
        assert_eq!(extract_command(t).as_deref(), Some("cargo build --release"));
    }

    #[test]
    fn strips_powershell_prompt() {
        let t = "```powershell\nPS C:\\Users\\me> Get-ChildItem\n```";
        assert_eq!(extract_command(t).as_deref(), Some("Get-ChildItem"));
        assert_eq!(strip_shell_prompt("PS> echo hi"), "echo hi");
        assert_eq!(strip_shell_prompt("$ npm test"), "npm test");
        // A bare command is returned untouched.
        assert_eq!(strip_shell_prompt("git status"), "git status");
    }

    #[test]
    fn falls_back_to_prompt_style_line() {
        let t = "You can run:\n$ git status\nto see changes.";
        assert_eq!(extract_command(t).as_deref(), Some("git status"));
    }

    #[test]
    fn none_for_plain_prose() {
        assert!(extract_command("Just an explanation, no command here.").is_none());
    }
}
