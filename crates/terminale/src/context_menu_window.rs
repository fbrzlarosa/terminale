//! Borderless OS popup window used as the right-click context menu.
//!
//! Lives in its own winit window so it can extend past the main terminal
//! window's edges (native popup behaviour). Closes when it loses focus,
//! when the user presses Esc, or when an item is clicked.
//!
//! Submenus are drawn **inside the same window** in a second egui [`egui::Area`]
//! positioned to the right of the base column. This is the key fix: with two
//! separate OS windows the parent stops receiving `CursorMoved` the instant
//! the pointer crosses into the child, freezing egui's hover state and
//! `open_submenu_idx`. A single window avoids that entirely — the OS always
//! delivers cursor events to the window under the pointer, which is now always
//! the one we own.

use egui::Id;
use egui::Order;
use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State as EguiState;
use std::sync::Arc;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId, WindowLevel};

/// One row in the popup.
#[derive(Debug, Clone)]
pub struct MenuEntry {
    /// Optional leading glyph (an emoji / symbol). Rendered with egui's
    /// proportional font, which bundles `NotoEmoji` + `emoji-icon-font`,
    /// so any standard symbol shows rather than a tofu box.
    pub icon: Option<String>,
    pub label: String,
    /// Hotkey hint shown right-aligned. When `submenu` is `Some`, the
    /// chevron glyph (▶) replaces this column and `hotkey` is ignored.
    pub hotkey: Option<String>,
    pub enabled: bool,
    pub separator_before: bool,
    /// Opaque id passed back to the caller when this item is clicked.
    /// Ignored when `submenu` is `Some`.
    pub action_id: u32,
    /// When `Some`, clicking this row opens a child popup instead of
    /// dispatching `action_id`. The chevron (▶) is shown in the hotkey
    /// column. Mouse-only; keyboard nav is a known v1 limitation.
    pub submenu: Option<Vec<MenuEntry>>,
}

const ITEM_HEIGHT: f32 = 30.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const MENU_WIDTH: f32 = 260.0;
const TOP_PADDING: f32 = 6.0;
const BOTTOM_PADDING: f32 = 6.0;
/// Horizontal overlap between the base column and the flyout column (logical px).
const FLYOUT_OVERLAP: f32 = 4.0;

/// Compute the logical-pixel size the window should have.
///
/// When `open_submenu_idx` is `Some(i)` and `entries[i].submenu` is `Some`,
/// the window grows rightward to accommodate the flyout column. Height
/// expands when the flyout would extend below the base column.
fn desired_size(entries: &[MenuEntry], open_submenu_idx: Option<usize>) -> (f32, f32) {
    let base_h = column_height(entries);

    let flyout = open_submenu_idx
        .and_then(|i| entries.get(i))
        .and_then(|e| e.submenu.as_deref());

    if let Some(children) = flyout {
        let row_top = row_top_for(entries, open_submenu_idx.unwrap_or(0));
        let flyout_h = column_height(children);
        let total_h = (row_top + flyout_h).max(base_h);
        let total_w = MENU_WIDTH * 2.0 - FLYOUT_OVERLAP;
        (total_w, total_h)
    } else {
        (MENU_WIDTH, base_h)
    }
}

/// Height of one column of `entries` including top/bottom padding.
fn column_height(entries: &[MenuEntry]) -> f32 {
    let mut h = TOP_PADDING + BOTTOM_PADDING;
    for entry in entries {
        if entry.separator_before {
            h += SEPARATOR_HEIGHT;
        }
        h += ITEM_HEIGHT;
    }
    h
}

/// Logical Y offset (from the window top) where `entries[row]` starts.
/// Includes `TOP_PADDING` and any separators above `row`.
fn row_top_for(entries: &[MenuEntry], row: usize) -> f32 {
    let mut y = TOP_PADDING;
    for (i, entry) in entries.iter().enumerate() {
        if i == row {
            break;
        }
        if entry.separator_before && i > 0 {
            y += SEPARATOR_HEIGHT;
        }
        y += ITEM_HEIGHT;
    }
    if row > 0 {
        if let Some(e) = entries.get(row) {
            if e.separator_before {
                y += SEPARATOR_HEIGHT;
            }
        }
    }
    y
}

/// Vertical placement of the menu's top edge (physical px).
///
/// A menu opened near the bottom of the screen would extend past the bottom
/// edge, leaving its lower items unreachable. When the menu's full height would
/// overflow `work_bottom`, flip it up so its bottom sits at the cursor
/// (`cursor_y - menu_h_px`); otherwise keep it at the cursor. The result is
/// clamped into `[work_top, work_bottom - menu_h_px]` so it never runs off the
/// top either (and pins to `work_top` for a menu taller than the screen).
fn menu_top_y(cursor_y: i32, menu_h_px: i32, work_top: i32, work_bottom: i32) -> i32 {
    let y = if cursor_y + menu_h_px > work_bottom {
        cursor_y - menu_h_px
    } else {
        cursor_y
    };
    let lower = (work_bottom - menu_h_px).max(work_top);
    y.clamp(work_top, lower)
}

/// Shared visual frame used for both the base column and the flyout column.
fn menu_frame() -> egui::Frame {
    egui::Frame::default()
        .fill(egui::Color32::from_rgb(26, 30, 44))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 56, 80)))
        .inner_margin(egui::Margin {
            left: 0.0,
            right: 0.0,
            top: TOP_PADDING,
            bottom: BOTTOM_PADDING,
        })
}

pub struct ContextMenuWindow {
    /// The single OS window that owns both the base column and any open flyout.
    pub window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    egui_ctx: egui::Context,
    egui_state: EguiState,
    egui_renderer: EguiRenderer,

    entries: Vec<MenuEntry>,
    /// Set when the user clicks an enabled leaf item.
    chosen: Option<u32>,
    /// `true` when the window should be torn down on the next event-loop tick.
    requested_close: bool,
    /// Index of the parent row whose flyout is currently drawn inside this
    /// window. `None` = no flyout visible.
    open_submenu_idx: Option<usize>,
    /// Screen position at which this window was originally opened (physical px).
    origin_screen: PhysicalPosition<i32>,
    /// Work-area dimensions of the monitor the window is on (physical px).
    monitor_work_area: Option<PhysicalSize<u32>>,
    /// Top-left of the monitor the window is on (physical px). Needed alongside
    /// the size to compute the screen's bottom edge for the upward flip.
    monitor_origin: Option<PhysicalPosition<i32>>,
    /// When the popup was shown. Used to ignore the spurious `Focused(false)`
    /// macOS emits immediately after a trackpad two-finger tap (the press/release
    /// hands focus straight back to the parent), which would otherwise close the
    /// menu within milliseconds of opening.
    opened_at: std::time::Instant,
}

impl ContextMenuWindow {
    /// Open the popup at the given **screen** coordinate (physical px).
    ///
    /// Pass shared wgpu state (typically from the main `Renderer`) to skip
    /// the cost of creating a new instance/adapter/device on every right-click.
    ///
    /// `focus_on_open` controls whether the new window immediately steals OS
    /// keyboard focus. Pass `true` for the root menu so Esc and click-outside
    /// work correctly.
    pub fn open(
        event_loop: &ActiveEventLoop,
        screen_px: PhysicalPosition<i32>,
        entries: Vec<MenuEntry>,
        instance: Arc<wgpu::Instance>,
        adapter: Arc<wgpu::Adapter>,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        focus_on_open: bool,
    ) -> Self {
        let (w, h) = desired_size(&entries, None);

        // Window is created hidden and rendered offscreen, then DWM-cloaked
        // and only THEN made visible: the compositor never sees a
        // hidden → visible-with-content transition, so no open animation fires.
        let attrs = Window::default_attributes()
            .with_title("terminale — menu")
            .with_inner_size(LogicalSize::new(w, h))
            .with_decorations(false)
            .with_resizable(false)
            .with_position(screen_px)
            .with_window_level(WindowLevel::AlwaysOnTop)
            // Transparent so the empty L-shaped region (right of the base
            // column, below a shorter flyout) shows the desktop/terminal
            // behind instead of an opaque dark block. Only the menu_frame()
            // panels paint their (opaque) fill.
            .with_transparent(true)
            .with_visible(false);
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_skip_taskbar(true).with_undecorated_shadow(false)
        };
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create context menu window"),
        );

        let monitor_work_area = window.current_monitor().map(|m| m.size());
        let monitor_origin = window.current_monitor().map(|m| m.position());

        // Bottom-edge flip: a menu opened near the bottom of the screen would
        // extend past the screen edge, leaving its lower items unreachable.
        // Reposition the (still hidden) window upward so the whole base column
        // fits. resize_for_flyout() reapplies this when a submenu changes the
        // height, so the anchor stored below stays the raw cursor point.
        if let Some(size) = monitor_work_area {
            let scale = window.scale_factor() as f32;
            let work_top = monitor_origin.map_or(0, |o| o.y);
            let work_bottom = work_top + size.height as i32;
            let y = menu_top_y(screen_px.y, (h * scale) as i32, work_top, work_bottom);
            if y != screen_px.y {
                window.set_outer_position(PhysicalPosition::new(screen_px.x, y));
            }
        }

        let surface = instance
            .create_surface(Arc::clone(&window))
            .expect("menu surface");

        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        // egui prefers a linear (non-sRGB) framebuffer — see settings window.
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
            // Prefer an alpha-blended composite mode so the transparent clear
            // is honoured by the compositor; fall back to whatever is offered.
            alpha_mode: caps
                .alpha_modes
                .iter()
                .copied()
                .find(|m| {
                    matches!(
                        m,
                        wgpu::CompositeAlphaMode::PreMultiplied
                            | wgpu::CompositeAlphaMode::PostMultiplied
                    )
                })
                .unwrap_or(caps.alpha_modes[0]),
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

        #[cfg(windows)]
        disable_dwm_transitions(&window);

        let mut this = Self {
            window,
            surface,
            surface_config,
            device,
            queue,
            egui_ctx,
            egui_state,
            egui_renderer,
            entries,
            chosen: None,
            requested_close: false,
            open_submenu_idx: None,
            origin_screen: screen_px,
            monitor_work_area,
            monitor_origin,
            opened_at: std::time::Instant::now(),
        };

        // Render the first frame while the window is still hidden so the
        // compositor sees a fully-drawn window when we make it visible.
        this.render_frame();

        // DWM-cloak before show to suppress the OS create animation.
        #[cfg(windows)]
        set_dwm_cloak(&this.window, true);

        this.window.set_visible(true);

        #[cfg(windows)]
        set_dwm_cloak(&this.window, false);

        if focus_on_open {
            this.window.focus_window();
        }

        this
    }

    /// Force a specific submenu index open immediately (for demo / screenshot
    /// verification — sets the flyout visible on the first frame).
    pub fn force_open_submenu(&mut self, idx: usize) {
        self.open_submenu_idx = Some(idx);
        self.resize_for_flyout();
    }

    /// The OS window id for routing events.
    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Take the chosen action (one-shot — clears the slot).
    pub fn take_chosen(&mut self) -> Option<u32> {
        self.chosen.take()
    }

    /// Handle one winit event. Returns `true` when the popup should be dropped.
    pub fn handle_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CloseRequested => return true,
            WindowEvent::Focused(false) => {
                // Click-outside-to-close: a focus-loss normally means the user
                // clicked elsewhere. But macOS hands focus straight back to the
                // parent right after a trackpad two-finger tap (the right-click
                // press/release that opened us), firing a spurious Focused(false)
                // within a few ms. During a short grace window, re-grab focus
                // instead of closing so the menu survives the tap *and* stays
                // focused — keeping Esc and a genuine later click-outside working.
                const FOCUS_GRACE: std::time::Duration = std::time::Duration::from_millis(350);
                if self.opened_at.elapsed() < FOCUS_GRACE {
                    self.window.focus_window();
                } else {
                    self.requested_close = true;
                }
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
            WindowEvent::Resized(new_size) => {
                // Keep the wgpu surface in sync whenever the window grows to
                // fit a newly opened flyout. Without this the surface stays at
                // the original size and the flyout column is clipped or shows
                // undefined content.
                self.surface_config.width = new_size.width.max(1);
                self.surface_config.height = new_size.height.max(1);
                self.surface.configure(&self.device, &self.surface_config);
                self.window.request_redraw();
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

        if self.chosen.is_some() {
            return false;
        }

        self.requested_close
    }

    /// Resize the OS window to fit the currently open flyout (if any) and
    /// reconfigure the wgpu surface to match. Also repositions the window
    /// when a right-edge flip is needed to keep the flyout on-screen.
    fn resize_for_flyout(&mut self) {
        let scale = self.window.scale_factor() as f32;
        let (lw, lh) = desired_size(&self.entries, self.open_submenu_idx);

        // Right-edge flip: if the flyout would overflow the monitor, move the
        // window left so the flyout appears to the left of the base column.
        // Bottom-edge flip: recompute the top edge from the (possibly taller)
        // flyout height so a menu near the bottom stays fully on-screen.
        if let Some(work_area) = self.monitor_work_area {
            let work_top = self.monitor_origin.map_or(0, |o| o.y);
            let work_bottom = work_top + work_area.height as i32;
            let y = menu_top_y(
                self.origin_screen.y,
                (lh * scale) as i32,
                work_top,
                work_bottom,
            );
            let candidate_right = self.origin_screen.x + (lw * scale) as i32;
            let x = if candidate_right > work_area.width as i32 {
                self.origin_screen.x - ((MENU_WIDTH - FLYOUT_OVERLAP) * scale) as i32
            } else {
                self.origin_screen.x
            };
            self.window.set_outer_position(PhysicalPosition::new(x, y));
        }

        // request_inner_size delivers a Resized event which updates
        // surface_config and reconfigures the surface there.
        let _ = self.window.request_inner_size(LogicalSize::new(lw, lh));
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
                label: Some("menu encoder"),
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
                    label: Some("menu pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Fully transparent: the menu_frame() panels paint
                            // their own opaque fill; everything else (the empty
                            // L-region around a short flyout) stays see-through.
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 0.0,
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

    #[allow(clippy::too_many_lines)]
    fn build_ui(&mut self, ctx: &egui::Context) {
        let mut clicked_leaf: Option<u32> = None;
        let prev_open_submenu = self.open_submenu_idx;
        let mut hovered_submenu: Option<usize> = None;
        let mut hovered_submenu_has_data = false;

        // Reserve a consistent left gutter for icons whenever any entry
        // has one, so labels line up regardless of which rows carry a glyph.
        let has_icons = self.entries.iter().any(|e| e.icon.is_some());
        let icon_gutter = if has_icons { 26.0 } else { 0.0 };

        // ---- Base column ---------------------------------------------------
        // Constrained to exactly MENU_WIDTH so it never bleeds into the flyout
        // area when the window is wider.
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let col_rect = egui::Rect::from_min_size(
                    ui.min_rect().min,
                    egui::vec2(MENU_WIDTH, ui.available_height()),
                );
                let mut col_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(col_rect)
                        .layout(egui::Layout::top_down(egui::Align::LEFT)),
                );
                menu_frame().show(&mut col_ui, |ui| {
                    ui.style_mut().spacing.item_spacing = egui::vec2(0.0, 0.0);

                    for (i, entry) in self.entries.iter().enumerate() {
                        if entry.separator_before && i > 0 {
                            ui.add_space(4.0);
                            let avail = ui.available_width();
                            let pad = 12.0;
                            let (rect, _) = ui
                                .allocate_exact_size(egui::vec2(avail, 1.0), egui::Sense::hover());
                            ui.painter().line_segment(
                                [
                                    egui::pos2(rect.left() + pad, rect.center().y),
                                    egui::pos2(rect.right() - pad, rect.center().y),
                                ],
                                egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 56, 80)),
                            );
                            ui.add_space(4.0);
                        }

                        let avail = ui.available_width();
                        let sense = if entry.enabled {
                            egui::Sense::click()
                        } else {
                            egui::Sense::hover()
                        };
                        let (rect, resp) =
                            ui.allocate_exact_size(egui::vec2(avail, ITEM_HEIGHT), sense);

                        if ui.is_rect_visible(rect) {
                            draw_row(
                                ui,
                                entry,
                                rect,
                                resp.hovered() && entry.enabled,
                                icon_gutter,
                            );
                        }

                        if entry.submenu.is_some() {
                            if resp.hovered() && entry.enabled {
                                hovered_submenu = Some(i);
                                hovered_submenu_has_data = true;
                            }
                        } else if resp.clicked() && entry.enabled {
                            clicked_leaf = Some(entry.action_id);
                        }
                    }
                });
            });

        // ---- Flyout column -------------------------------------------------
        // Drawn as an egui Area in the Foreground layer so it renders on top
        // of the base column's right border. Because everything is one window
        // egui reports pointer position continuously — no CursorMoved events
        // are ever lost.
        if let Some(parent_idx) = self.open_submenu_idx {
            if let Some(children) = self
                .entries
                .get(parent_idx)
                .and_then(|e| e.submenu.as_deref())
            {
                let flyout_x = MENU_WIDTH - FLYOUT_OVERLAP;
                let flyout_y = row_top_for(&self.entries, parent_idx);
                let flyout_size = egui::vec2(MENU_WIDTH, column_height(children));
                let flyout_pos = egui::pos2(flyout_x, flyout_y);

                let has_flyout_icons = children.iter().any(|e| e.icon.is_some());
                let flyout_icon_gutter = if has_flyout_icons { 26.0 } else { 0.0 };

                // Clone to avoid the borrow-while-mutable restriction.
                let children_owned: Vec<MenuEntry> = children.to_vec();

                egui::Area::new(Id::new("ctxmenu_flyout"))
                    .order(Order::Foreground)
                    .fixed_pos(flyout_pos)
                    .show(ctx, |ui| {
                        let area_rect = egui::Rect::from_min_size(flyout_pos, flyout_size);
                        let mut inner_ui = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(area_rect)
                                .layout(egui::Layout::top_down(egui::Align::LEFT)),
                        );
                        menu_frame().show(&mut inner_ui, |ui| {
                            ui.style_mut().spacing.item_spacing = egui::vec2(0.0, 0.0);

                            for (ci, child) in children_owned.iter().enumerate() {
                                if child.separator_before && ci > 0 {
                                    ui.add_space(4.0);
                                    let avail = ui.available_width();
                                    let pad = 12.0;
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(avail, 1.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().line_segment(
                                        [
                                            egui::pos2(rect.left() + pad, rect.center().y),
                                            egui::pos2(rect.right() - pad, rect.center().y),
                                        ],
                                        egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 56, 80)),
                                    );
                                    ui.add_space(4.0);
                                }

                                let avail = ui.available_width();
                                let sense = if child.enabled {
                                    egui::Sense::click()
                                } else {
                                    egui::Sense::hover()
                                };
                                let (rect, resp) =
                                    ui.allocate_exact_size(egui::vec2(avail, ITEM_HEIGHT), sense);

                                if ui.is_rect_visible(rect) {
                                    draw_row(
                                        ui,
                                        child,
                                        rect,
                                        resp.hovered() && child.enabled,
                                        flyout_icon_gutter,
                                    );
                                }

                                if resp.clicked() && child.enabled {
                                    clicked_leaf = Some(child.action_id);
                                }
                            }
                        });
                    });
            }
        }

        // ---- Dispatch ------------------------------------------------------
        if let Some(id) = clicked_leaf {
            self.chosen = Some(id);
            self.requested_close = true;
        }

        // Update open_submenu_idx via the state machine. The flyout closes as
        // soon as the cursor leaves the parent row — unless it is inside the
        // flyout itself, so its children stay reachable.
        let pointer_in_flyout = prev_open_submenu
            .and_then(|parent_idx| {
                self.entries
                    .get(parent_idx)
                    .and_then(|e| e.submenu.as_deref())
                    .map(|children| {
                        let flyout_rect = egui::Rect::from_min_size(
                            egui::pos2(
                                MENU_WIDTH - FLYOUT_OVERLAP,
                                row_top_for(&self.entries, parent_idx),
                            ),
                            egui::vec2(MENU_WIDTH, column_height(children)),
                        );
                        ctx.pointer_latest_pos()
                            .is_some_and(|p| flyout_rect.contains(p))
                    })
            })
            .unwrap_or(false);
        let (new_idx, should_resize) = submenu_transition(
            prev_open_submenu,
            hovered_submenu,
            hovered_submenu_has_data,
            pointer_in_flyout,
        );
        self.open_submenu_idx = new_idx;

        if should_resize || new_idx != prev_open_submenu {
            self.resize_for_flyout();
        }
    }
}

/// Draw one menu row into `rect`. Shared by the base column and the flyout.
fn draw_row(
    ui: &mut egui::Ui,
    entry: &MenuEntry,
    rect: egui::Rect,
    hovered: bool,
    icon_gutter: f32,
) {
    if hovered {
        // Highlight background + 3 px accent bar on the left edge.
        ui.painter()
            .rect_filled(rect, 0.0, egui::Color32::from_rgb(44, 60, 110));
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top()),
            egui::vec2(3.0, rect.height()),
        );
        ui.painter()
            .rect_filled(bar, 0.0, egui::Color32::from_rgb(110, 160, 240));
    }

    let label_color = if entry.enabled {
        egui::Color32::from_rgb(230, 234, 248)
    } else {
        egui::Color32::from_rgb(96, 102, 120)
    };

    if let Some(icon) = &entry.icon {
        ui.painter().text(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            icon,
            egui::FontId::new(13.5, egui::FontFamily::Proportional),
            label_color,
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 14.0 + icon_gutter, rect.center().y),
        egui::Align2::LEFT_CENTER,
        &entry.label,
        egui::FontId::new(13.5, egui::FontFamily::Proportional),
        label_color,
    );

    // Right column: chevron ▶ U+25B6 for submenu parents, hotkey for leaves.
    if entry.submenu.is_some() {
        let chevron_color = if entry.enabled {
            egui::Color32::from_rgb(140, 150, 180)
        } else {
            egui::Color32::from_rgb(80, 84, 100)
        };
        ui.painter().text(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "\u{25B6}",
            egui::FontId::new(11.5, egui::FontFamily::Proportional),
            chevron_color,
        );
    } else if let Some(hot) = &entry.hotkey {
        let hot_color = if entry.enabled {
            egui::Color32::from_rgb(140, 150, 180)
        } else {
            egui::Color32::from_rgb(80, 84, 100)
        };
        ui.painter().text(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            hot,
            egui::FontId::new(11.5, egui::FontFamily::Proportional),
            hot_color,
        );
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

#[cfg(windows)]
fn disable_dwm_transitions(window: &Window) {
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
    const DWMWA_TRANSITIONS_FORCEDISABLED: u32 = 3;

    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return;
    };
    let hwnd = h.hwnd.get() as *mut c_void;
    let value: i32 = 1;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            std::ptr::from_ref::<i32>(&value) as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

fn configure_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.window_fill = egui::Color32::from_rgb(26, 30, 44);
    style.visuals.panel_fill = egui::Color32::from_rgb(26, 30, 44);
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(230, 234, 248));
    style.spacing.item_spacing = egui::vec2(0.0, 0.0);
    style.spacing.window_margin = egui::Margin::ZERO;
    ctx.set_style(style);
}

/// Pure helper: submenu open-on-hover state machine.
///
/// Given:
/// - `prev`     — `open_submenu_idx` at the start of the frame
/// - `hovered`  — the submenu-parent row the cursor is over (`None` = leaf or gap)
/// - `has_data` — whether the hovered row has a submenu payload
///
/// Returns `(new_open_submenu_idx, should_resize)`.
///
/// Rules:
/// - Hovering a *new* submenu parent → trigger resize, update idx.
/// - Hovering the *same* parent → no-op.
/// - Not over a submenu parent (`hovered = None`):
///   - pointer still inside the open flyout → keep it open (children stay
///     clickable);
///   - otherwise → close the flyout and resize the window back down. The
///     submenu must not linger once the cursor leaves the parent row.
fn submenu_transition(
    prev: Option<usize>,
    hovered: Option<usize>,
    has_data: bool,
    pointer_in_flyout: bool,
) -> (Option<usize>, bool) {
    match hovered {
        Some(idx) => {
            let fire = Some(idx) != prev && has_data;
            (Some(idx), fire)
        }
        None if pointer_in_flyout => (prev, false),
        None => (None, prev.is_some()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── submenu hover state machine ──────────────────────────────────────────

    /// Hovering a submenu parent that is NOT currently open must trigger a
    /// resize and update the tracked index.
    #[test]
    fn submenu_transition_new_parent_fires_open() {
        let (new_idx, fire) = submenu_transition(None, Some(2), true, false);
        assert_eq!(new_idx, Some(2));
        assert!(fire, "moving to a new submenu parent must trigger resize");
    }

    /// Hovering the same submenu parent that is already open must NOT re-fire.
    #[test]
    fn submenu_transition_same_parent_no_refire() {
        let (new_idx, fire) = submenu_transition(Some(1), Some(1), true, false);
        assert_eq!(new_idx, Some(1));
        assert!(!fire, "hovering same parent must not trigger resize");
    }

    /// Moving from one submenu parent to another must trigger a new resize.
    #[test]
    fn submenu_transition_different_parent_fires_open() {
        let (new_idx, fire) = submenu_transition(Some(0), Some(3), true, false);
        assert_eq!(new_idx, Some(3));
        assert!(
            fire,
            "switching to a different submenu parent must trigger resize"
        );
    }

    /// Not over the parent but the pointer is INSIDE the flyout → keep it open
    /// so its children stay clickable.
    #[test]
    fn submenu_transition_pointer_in_flyout_keeps_open() {
        let (new_idx, fire) = submenu_transition(Some(2), None, false, true);
        assert_eq!(new_idx, Some(2), "pointer inside flyout must keep it open");
        assert!(!fire, "staying inside the flyout must not trigger resize");
    }

    /// Leaving the parent row for another row (pointer NOT in the flyout) must
    /// CLOSE the flyout and resize the window back down.
    #[test]
    fn submenu_transition_leaf_outside_flyout_closes() {
        let (new_idx, resize) = submenu_transition(Some(2), None, false, false);
        assert_eq!(
            new_idx, None,
            "leaving the parent row must close the flyout"
        );
        assert!(
            resize,
            "closing the flyout must resize the window back down"
        );
    }

    /// When no submenu was open and cursor is over a leaf, both stay None/false.
    #[test]
    fn submenu_transition_leaf_with_no_open_child() {
        let (new_idx, fire) = submenu_transition(None, None, false, false);
        assert_eq!(new_idx, None);
        assert!(!fire);
    }

    /// Hovering a submenu parent that has no data (e.g. disabled row) must not
    /// trigger a resize even though `hovered` is Some.
    #[test]
    fn submenu_transition_no_data_does_not_fire() {
        let (new_idx, fire) = submenu_transition(None, Some(1), false, false);
        assert_eq!(new_idx, Some(1), "index still updates to track hover");
        assert!(!fire, "no submenu data means no resize");
    }

    // ── desired_size ─────────────────────────────────────────────────────────

    /// Without a flyout the window is exactly MENU_WIDTH wide.
    #[test]
    fn desired_size_no_flyout() {
        let entries = vec![
            MenuEntry {
                icon: None,
                label: "Copy".into(),
                hotkey: None,
                enabled: true,
                separator_before: false,
                action_id: 0,
                submenu: None,
            },
            MenuEntry {
                icon: None,
                label: "Paste".into(),
                hotkey: None,
                enabled: true,
                separator_before: false,
                action_id: 1,
                submenu: None,
            },
        ];
        let (w, _h) = desired_size(&entries, None);
        assert!(
            (w - MENU_WIDTH).abs() < 0.1,
            "width without flyout must equal MENU_WIDTH"
        );
    }

    /// With a flyout open the window must be wider than MENU_WIDTH.
    #[test]
    fn desired_size_with_flyout_is_wider() {
        let child = MenuEntry {
            icon: None,
            label: "Child".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: 10,
            submenu: None,
        };
        let entries = vec![MenuEntry {
            icon: None,
            label: "Parent".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: 0,
            submenu: Some(vec![child]),
        }];
        let (w, _h) = desired_size(&entries, Some(0));
        assert!(
            w > MENU_WIDTH,
            "window with flyout must be wider than MENU_WIDTH"
        );
    }

    // ── menu_top_y (bottom-edge flip) ──────────────────────────────────────────

    #[test]
    fn menu_stays_at_cursor_when_it_fits() {
        // 100px-tall menu, cursor mid-screen, 0..1000 → unchanged.
        assert_eq!(menu_top_y(200, 100, 0, 1000), 200);
    }

    #[test]
    fn menu_flips_up_near_bottom_edge() {
        // 950 + 100 = 1050 > 1000 → flip up so the bottom sits at the cursor.
        assert_eq!(menu_top_y(950, 100, 0, 1000), 850);
    }

    #[test]
    fn menu_flip_pins_to_top_when_taller_than_screen() {
        // A menu taller than the screen can't fully fit; pin to the top edge.
        assert_eq!(menu_top_y(950, 1200, 0, 1000), 0);
    }

    #[test]
    fn menu_top_y_respects_nonzero_monitor_origin() {
        // Secondary monitor spanning y in [1080, 2080).
        assert_eq!(menu_top_y(2050, 100, 1080, 2080), 1950); // flips up
        assert_eq!(menu_top_y(1100, 100, 1080, 2080), 1100); // fits, unchanged
    }

    // ── column_height ─────────────────────────────────────────────────────────

    /// A single-item column has height = TOP_PADDING + ITEM_HEIGHT + BOTTOM_PADDING.
    #[test]
    fn column_height_single_item() {
        let entries = vec![MenuEntry {
            icon: None,
            label: "A".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: 0,
            submenu: None,
        }];
        let h = column_height(&entries);
        let expected = TOP_PADDING + ITEM_HEIGHT + BOTTOM_PADDING;
        assert!((h - expected).abs() < 0.1);
    }

    // ── MenuEntry submenu flag ───────────────────────────────────────────────

    /// A leaf entry must have `submenu = None`.
    #[test]
    fn menu_entry_leaf_has_no_submenu() {
        let entry = MenuEntry {
            icon: None,
            label: "Copy".into(),
            hotkey: Some("Ctrl+C".into()),
            enabled: true,
            separator_before: false,
            action_id: 1,
            submenu: None,
        };
        assert!(entry.submenu.is_none());
    }

    /// A submenu-parent entry must have a non-empty `submenu`.
    #[test]
    fn menu_entry_parent_has_children() {
        let child = MenuEntry {
            icon: None,
            label: "Child".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: 10,
            submenu: None,
        };
        let parent = MenuEntry {
            icon: None,
            label: "Parent".into(),
            hotkey: None,
            enabled: true,
            separator_before: false,
            action_id: 0,
            submenu: Some(vec![child]),
        };
        assert!(parent.submenu.is_some());
        assert_eq!(parent.submenu.as_ref().unwrap().len(), 1);
    }
}
