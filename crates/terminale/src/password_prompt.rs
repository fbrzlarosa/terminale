//! Borderless OS popup window that prompts for an SSH secret at connect time.
//!
//! Used when a host needs a password (or an encrypted-key passphrase) that
//! isn't yet in the OS keychain. The secret is collected with a masked input,
//! held only in memory, and handed back to the connect path; the user can tick
//! "Remember in keychain" to persist it (the only way a secret ever lands in
//! storage — and that storage is the OS keychain, never `config.toml`).
//!
//! Modelled on [`crate::context_menu_window::ContextMenuWindow`]: its own
//! winit + egui + wgpu window so it can float above the terminal like a native
//! dialog. Closes on submit, Cancel, Esc, or loss of focus.

use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiState;
use std::sync::Arc;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

const WIN_WIDTH: f32 = 420.0;
const WIN_HEIGHT: f32 = 232.0;

/// The user's response once the prompt closes.
pub struct PromptOutcome {
    /// The entered secret (password / passphrase). Never logged.
    pub secret: String,
    /// Whether to persist the secret in the OS keychain for next time.
    pub remember: bool,
}

/// A modal-ish popup collecting one SSH secret.
pub struct PasswordPrompt {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,

    /// Index of the host in `config.ssh_hosts` this prompt is for — echoed
    /// back to the App so it knows which connection to resume.
    host_idx: usize,
    /// Title line (e.g. `deploy@10.0.0.5`).
    endpoint: String,
    /// Whether we're asking for a key passphrase (vs a login password).
    is_passphrase: bool,

    secret: String,
    remember: bool,

    /// Set on submit; carries the collected secret + remember flag.
    submitted: Option<PromptOutcome>,
    /// True once the window should be torn down.
    requested_close: bool,
    /// Set so the text field grabs focus on the first frame.
    first_frame: bool,
}

impl PasswordPrompt {
    /// Open the prompt centred over `parent`. `host_idx` is echoed back on
    /// submit so the caller can resume the right connection.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        parent: &Window,
        host_idx: usize,
        endpoint: String,
        is_passphrase: bool,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Self {
        // Centre over the parent window.
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

        let attrs = crate::app_icon::with_app_identity(Window::default_attributes())
            .with_title("terminale — SSH credential")
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
                .expect("failed to create password prompt window"),
        );

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("prompt surface");

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
            host_idx,
            endpoint,
            is_passphrase,
            secret: String::new(),
            remember: false,
            submitted: None,
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

    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// The host index this prompt belongs to.
    #[must_use]
    pub fn host_idx(&self) -> usize {
        self.host_idx
    }

    /// Take the submitted outcome (one-shot). `None` until the user submits.
    pub fn take_outcome(&mut self) -> Option<PromptOutcome> {
        self.submitted.take()
    }

    /// Handle one winit event. Returns `true` when the prompt should be dropped.
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::Focused(false) => {
                // Click-outside cancels (treated as no secret entered).
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

        // Submit closes after this event is processed (so the App can read
        // the outcome on the same tick).
        if self.submitted.is_some() {
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
                label: Some("prompt encoder"),
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
                    label: Some("prompt pass"),
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
        let mut submit = false;
        let mut cancel = false;

        let frame = egui::Frame::default()
            .fill(egui::Color32::from_rgb(13, 15, 22))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(40, 48, 70)))
            .inner_margin(egui::Margin::symmetric(22.0, 18.0));

        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let title = if self.is_passphrase {
                "Key passphrase required"
            } else {
                "Password required"
            };
            ui.label(
                egui::RichText::new(title)
                    .size(17.0)
                    .strong()
                    .color(egui::Color32::from_rgb(225, 232, 250)),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(&self.endpoint)
                    .monospace()
                    .color(egui::Color32::from_rgb(150, 160, 190)),
            );
            ui.add_space(14.0);

            let field = ui.add(
                egui::TextEdit::singleline(&mut self.secret)
                    .password(true)
                    .desired_width(f32::INFINITY)
                    .hint_text(if self.is_passphrase {
                        "passphrase"
                    } else {
                        "password"
                    }),
            );
            // Grab focus on the very first frame so the user can just type.
            if self.first_frame {
                field.request_focus();
                self.first_frame = false;
            }
            // Enter submits.
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }

            ui.add_space(12.0);
            ui.checkbox(
                &mut self.remember,
                "Remember in OS keychain (encrypted by the OS)",
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Stored only in your platform credential store — never in config.toml.",
                )
                .small()
                .color(egui::Color32::from_rgb(120, 130, 160)),
            );

            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new("  Connect  ")
                                    .strong()
                                    .color(egui::Color32::WHITE),
                            )
                            .fill(egui::Color32::from_rgb(60, 110, 230))
                            .min_size(egui::vec2(0.0, 32.0))
                            .rounding(8.0),
                        )
                        .clicked()
                    {
                        submit = true;
                    }
                    ui.add_space(8.0);
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

        if submit && !self.secret.is_empty() {
            self.submitted = Some(PromptOutcome {
                secret: std::mem::take(&mut self.secret),
                remember: self.remember,
            });
            self.requested_close = true;
        }
        if cancel {
            self.requested_close = true;
        }
    }
}

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
