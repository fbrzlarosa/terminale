//! Close-confirmation modal (`window.confirm_close`).
//!
//! When enabled, closing a tab or a window first shows this dialog instead
//! of the old "flash + press close again within 1.5 s" mechanism, which
//! users read as "nothing happened". Follows the same windowed-popup
//! pattern as [`crate::paste_guard::PasteGuardDialog`]: its own winit +
//! egui + wgpu window, floated over the terminal, closed on Close /
//! Cancel / Esc / focus-loss.

use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiState;
use std::sync::Arc;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

// ── CloseTarget / ConfirmCloseOutcome ────────────────────────────────────────

/// What a pending close request would close once confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTarget {
    /// The whole terminal window (OS close button / Alt+F4 / title-bar X).
    Window,
    /// One tab, by index at request time.
    Tab(usize),
}

/// The user's response once the confirmation dialog closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmCloseOutcome {
    /// Proceed with the close.
    Confirm,
    /// Keep the tab/window open.
    Cancel,
}

// ── ConfirmCloseDialog ───────────────────────────────────────────────────────

const WIN_WIDTH: f32 = 420.0;
const WIN_HEIGHT: f32 = 170.0;

/// A modal-ish popup asking the user to confirm or cancel a pending close.
pub struct ConfirmCloseDialog {
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,

    /// What confirming would close.
    target: CloseTarget,
    /// The terminal window that owns the request.
    parent_id: WindowId,
    /// Dialog title ("Close window?" / "Close tab?").
    title: String,
    /// Secondary detail line (tab title / open-tab count).
    detail: String,

    /// Set once the user responds.
    outcome: Option<ConfirmCloseOutcome>,
    /// True once the window should be torn down.
    requested_close: bool,
    /// Set so the "Close" button grabs focus on the first frame.
    first_frame: bool,
}

impl ConfirmCloseDialog {
    /// Open the close-confirmation dialog centred over `parent`.
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        parent: &Window,
        target: CloseTarget,
        detail: String,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> Self {
        let title = match target {
            CloseTarget::Window => "Close window?".to_string(),
            CloseTarget::Tab(_) => "Close tab?".to_string(),
        };

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
            .with_title("terminale — confirm close")
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
                .expect("failed to create confirm-close window"),
        );

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("confirm close surface");

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
            target,
            parent_id: parent.id(),
            title,
            detail,
            outcome: None,
            requested_close: false,
            first_frame: true,
        };

        // Paint the first frame BEFORE revealing the window so it never
        // flashes white/blank (same cloak-around-show as the other popups).
        this.render_frame();
        #[cfg(windows)]
        crate::set_dwm_cloak(&this.window, true);
        this.window.set_visible(true);
        #[cfg(windows)]
        crate::set_dwm_cloak(&this.window, false);
        this.window.focus_window();

        this
    }

    /// Stable OS window id.
    #[must_use]
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// What confirming would close.
    #[must_use]
    pub fn target(&self) -> CloseTarget {
        self.target
    }

    /// The terminal window that owns the pending close request.
    #[must_use]
    pub fn parent_id(&self) -> WindowId {
        self.parent_id
    }

    /// Take the user's outcome (one-shot). `None` until the user responds.
    pub fn take_outcome(&mut self) -> Option<ConfirmCloseOutcome> {
        self.outcome.take()
    }

    /// Handle one winit event. Returns `true` when the dialog should be dropped.
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::Focused(false) => {
                // Click-outside cancels (safer than closing).
                self.outcome = Some(ConfirmCloseOutcome::Cancel);
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
                self.outcome = Some(ConfirmCloseOutcome::Cancel);
                self.requested_close = true;
            }
            _ => {}
        }

        // Don't let `RedrawRequested` re-arm itself (idle-CPU guard — see
        // paste_guard for the rationale).
        let response = self.egui_state.on_window_event(&self.window, event);
        if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
            self.window.request_redraw();
        }

        if matches!(event, WindowEvent::RedrawRequested) {
            self.render_frame();
        }

        // Outcome set → close after this event so the App reads the outcome
        // on the same tick (same pattern as PasteGuardDialog).
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
                label: Some("confirm close encoder"),
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
                    label: Some("confirm close pass"),
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
                egui::RichText::new(&self.title)
                    .size(17.0)
                    .strong()
                    .color(egui::Color32::from_rgb(225, 232, 250)),
            );
            ui.add_space(6.0);

            // Detail line (tab title / open-tab count).
            if !self.detail.is_empty() {
                ui.label(
                    egui::RichText::new(&self.detail)
                        .size(13.0)
                        .color(egui::Color32::from_rgb(150, 160, 190)),
                );
            }

            ui.add_space(18.0);

            // Buttons
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Close button — destructive action, red fill.
                    let close_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new("  Close  ")
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(190, 60, 60))
                        .min_size(egui::vec2(0.0, 32.0))
                        .rounding(8.0),
                    );
                    if self.first_frame {
                        close_btn.request_focus();
                        self.first_frame = false;
                    }
                    if close_btn.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
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
            self.outcome = Some(ConfirmCloseOutcome::Confirm);
            self.requested_close = true;
        } else if cancel {
            self.outcome = Some(ConfirmCloseOutcome::Cancel);
            self.requested_close = true;
        }
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
