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
use winit::dpi::{LogicalSize, PhysicalPosition};
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

/// Compute the **fixed** logical-pixel size the OS window should have for the
/// whole life of this menu — the bounding box of the base column plus the
/// widest/tallest flyout any parent row could open.
///
/// The window is sized once, at open, and never resized while the user
/// navigates: opening, switching, or closing a submenu only changes the
/// `apply_flyout_region` clip, not the window itself. This
/// is deliberate — every `request_inner_size` on this surface is a Windows
/// `SetWindowPos` that recreates the swapchain, and the compositor scales the
/// stale buffer to the new size for one frame, which reads as a visible
/// "stretch". A constant window size removes that class of glitch entirely; the
/// region clip makes the window *look* exactly the right shape regardless.
///
/// A menu with no submenu parents is simply `MENU_WIDTH` wide.
fn window_outer_size(entries: &[MenuEntry]) -> (f32, f32) {
    let base_h = column_height(entries);
    let has_submenu = entries.iter().any(|e| e.submenu.is_some());
    if !has_submenu {
        return (MENU_WIDTH, base_h);
    }

    // Tallest point any flyout could reach (its parent row's top + its height),
    // never shorter than the base column.
    let mut height = base_h;
    for (i, entry) in entries.iter().enumerate() {
        if let Some(children) = entry.submenu.as_deref() {
            height = height.max(row_top_for(entries, i) + column_height(children));
        }
    }
    (MENU_WIDTH * 2.0 - FLYOUT_OVERLAP, height)
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
    /// `true` while the open flyout is drawn on the LEFT of the base column
    /// (right-edge flip). The OS window is moved left by the flyout width and
    /// the base column is drawn shifted right by the same amount, so the base
    /// stays visually anchored at the click point — only the flyout side
    /// changes. Decided once at open and constant for the menu's lifetime.
    flyout_on_left: bool,
    /// `(open_submenu_idx, flyout_on_left)` of the window region last applied via
    /// `SetWindowRgn`. The clip is refreshed only when this key changes, so a
    /// per-frame `SetWindowRgn` (which would force a redraw and could flicker) is
    /// avoided while the same flyout stays open. Windows-only.
    #[cfg(windows)]
    last_region_key: Option<(Option<usize>, bool)>,
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
        let (w, h) = window_outer_size(&entries);

        // Window is created hidden and rendered offscreen, then DWM-cloaked
        // and only THEN made visible: the compositor never sees a
        // hidden → visible-with-content transition, so no open animation fires.
        let attrs = crate::app_icon::with_app_identity(Window::default_attributes())
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

        // Panic-safe size read: winit's inherent `MonitorHandle::size()`
        // unwrap-panics on a handle invalidated by a standby/resume cycle.
        let monitor_work_area = window
            .current_monitor()
            .and_then(|m| crate::monitor_names::monitor_size(&m));
        let monitor_origin = window.current_monitor().map(|m| m.position());

        // Decide both screen-edge flips **once**, for the fixed window box —
        // the window is never resized afterwards, so flips can't change while
        // navigating (which would make the base column jump). The base column
        // is always anchored at the cursor; flips only move where the *extra*
        // width/height extends.
        //
        //  • Right-edge flip: if the full (base + flyout) width would overflow
        //    the monitor's right edge, extend the window leftward and draw the
        //    flyout on the LEFT of the base column (see `flyout_on_left`). The
        //    window origin moves left by one column so the base, drawn shifted
        //    right in `build_ui`, still lands at the click point.
        //  • Bottom-edge flip: if the window would overflow the bottom, pull its
        //    top up so it stays fully on-screen.
        let mut flyout_on_left = false;
        if let Some(size) = monitor_work_area {
            let scale = window.scale_factor() as f32;
            let work_top = monitor_origin.map_or(0, |o| o.y);
            let work_bottom = work_top + size.height as i32;
            let work_left = monitor_origin.map_or(0, |o| o.x);
            let work_right = work_left + size.width as i32;

            let has_submenu = entries.iter().any(|e| e.submenu.is_some());
            let candidate_right = screen_px.x + (w * scale) as i32;
            let x = if has_submenu && candidate_right > work_right {
                flyout_on_left = true;
                screen_px.x - ((MENU_WIDTH - FLYOUT_OVERLAP) * scale) as i32
            } else {
                screen_px.x
            };
            let y = menu_top_y(screen_px.y, (h * scale) as i32, work_top, work_bottom);
            if x != screen_px.x || y != screen_px.y {
                window.set_outer_position(PhysicalPosition::new(x, y));
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
            flyout_on_left,
            #[cfg(windows)]
            last_region_key: None,
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
        self.on_submenu_changed();
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

    /// React to a submenu opening, switching, or closing.
    ///
    /// The window is a fixed size for its whole lifetime (see
    /// [`window_outer_size`]), so this no longer touches the window size or
    /// position — doing so on every submenu change is exactly what caused the
    /// one-frame "stretch". All that's left is to schedule a redraw so the next
    /// frame paints the new flyout and refreshes the region clip
    /// ([`Self::build_ui`] reapplies it to match what was painted).
    fn on_submenu_changed(&mut self) {
        self.window.request_redraw();
    }

    /// Clip the OS window to the L-shape actually painted by [`Self::build_ui`]:
    /// the base column plus, when open, the flyout column. The corners left
    /// empty around a short flyout are excluded from the window region, so they
    /// fall back to whatever is behind the window instead of the opaque clear
    /// colour.
    ///
    /// Needed because this borderless, always-on-top surface only advertises
    /// `CompositeAlphaMode::Opaque` on Windows — the transparent clear can't be
    /// composited, so the empty L-region would otherwise show as a black block.
    /// Region coordinates are physical pixels relative to the window's top-left,
    /// matching `build_ui`'s logical layout scaled by `scale_factor`.
    #[cfg(windows)]
    fn apply_flyout_region(&self) {
        use std::ffi::c_void;
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        #[link(name = "gdi32")]
        extern "system" {
            fn CreateRectRgn(x1: i32, y1: i32, x2: i32, y2: i32) -> *mut c_void;
            fn CombineRgn(dst: *mut c_void, src1: *mut c_void, src2: *mut c_void, mode: i32)
                -> i32;
            fn DeleteObject(obj: *mut c_void) -> i32;
        }
        #[link(name = "user32")]
        extern "system" {
            fn SetWindowRgn(hwnd: *mut c_void, hrgn: *mut c_void, redraw: i32) -> i32;
        }
        const RGN_OR: i32 = 2;

        let Ok(handle) = self.window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(h) = handle.as_raw() else {
            return;
        };
        let hwnd = h.hwnd.get() as *mut c_void;

        let scale = self.window.scale_factor() as f32;
        let px = |v: f32| (v * scale).round() as i32;

        // Mirror the column placement computed in `build_ui`.
        let flipped = self.flyout_on_left;
        let base_col_x = if flipped {
            MENU_WIDTH - FLYOUT_OVERLAP
        } else {
            0.0
        };
        let flyout_col_x = if flipped {
            0.0
        } else {
            MENU_WIDTH - FLYOUT_OVERLAP
        };

        let base_h = column_height(&self.entries);
        // SAFETY: CreateRectRgn returns an owned region handle; ownership of the
        // final region transfers to the window via SetWindowRgn, intermediate
        // regions are freed with DeleteObject.
        unsafe {
            let region = CreateRectRgn(px(base_col_x), 0, px(base_col_x + MENU_WIDTH), px(base_h));

            if let Some(idx) = self.open_submenu_idx {
                if let Some(children) = self.entries.get(idx).and_then(|e| e.submenu.as_deref()) {
                    let flyout_y = row_top_for(&self.entries, idx);
                    let flyout_h = column_height(children);
                    let flyout = CreateRectRgn(
                        px(flyout_col_x),
                        px(flyout_y),
                        px(flyout_col_x + MENU_WIDTH),
                        px(flyout_y + flyout_h),
                    );
                    CombineRgn(region, region, flyout, RGN_OR);
                    DeleteObject(flyout);
                }
            }

            // The window now owns `region`; do not free it here.
            SetWindowRgn(hwnd, region, 1);
        }
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

        // Horizontal in-window offsets for the two columns. Normally the base
        // sits at 0 and the flyout opens to its right; in left-flip mode
        // (flyout would overflow the monitor's right edge) the window has been
        // extended leftward, the base is drawn shifted right — keeping it
        // visually anchored at the click point — and the flyout takes x = 0.
        let flipped = self.flyout_on_left;
        let base_col_x = if flipped {
            MENU_WIDTH - FLYOUT_OVERLAP
        } else {
            0.0
        };
        let flyout_col_x = if flipped {
            0.0
        } else {
            MENU_WIDTH - FLYOUT_OVERLAP
        };

        // ---- Base column ---------------------------------------------------
        // Constrained to exactly MENU_WIDTH so it never bleeds into the flyout
        // area when the window is wider.
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let col_rect = egui::Rect::from_min_size(
                    ui.min_rect().min + egui::vec2(base_col_x, 0.0),
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
                let flyout_x = flyout_col_x;
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
                            egui::pos2(flyout_col_x, row_top_for(&self.entries, parent_idx)),
                            egui::vec2(MENU_WIDTH, column_height(children)),
                        );
                        ctx.pointer_latest_pos()
                            .is_some_and(|p| flyout_rect.contains(p))
                    })
            })
            .unwrap_or(false);
        // Clip the window to the L-shape *of the flyout just painted*, before
        // the transition below mutates `open_submenu_idx`. Keying on the painted
        // state keeps the window region in lock-step with the on-screen content:
        // otherwise the frame that switches submenu parents would show the
        // previous flyout clipped to the next parent's shape, a one-frame
        // "stretch". The window itself never resizes (see `window_outer_size`),
        // so the region is the only thing that changes between submenus.
        #[cfg(windows)]
        {
            let key = (self.open_submenu_idx, self.flyout_on_left);
            if self.last_region_key != Some(key) {
                self.apply_flyout_region();
                self.last_region_key = Some(key);
            }
        }

        let (new_idx, changed) = submenu_transition(
            prev_open_submenu,
            hovered_submenu,
            hovered_submenu_has_data,
            pointer_in_flyout,
        );
        self.open_submenu_idx = new_idx;

        if changed || new_idx != prev_open_submenu {
            self.on_submenu_changed();
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

    // ── window_outer_size ──────────────────────────────────────────────────────

    /// Without any submenu parent the window is exactly MENU_WIDTH wide.
    #[test]
    fn window_outer_size_no_submenu_is_menu_width() {
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
        let (w, _h) = window_outer_size(&entries);
        assert!(
            (w - MENU_WIDTH).abs() < 0.1,
            "a menu with no submenus must be exactly MENU_WIDTH wide"
        );
    }

    /// A menu that has any submenu parent is sized to the full two-column box up
    /// front (and stays that width for its whole life — it is never resized when
    /// a flyout opens or closes).
    #[test]
    fn window_outer_size_with_submenu_is_wider() {
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
        let (w, _h) = window_outer_size(&entries);
        assert!(
            w > MENU_WIDTH,
            "a menu with a submenu parent must reserve the full two-column width"
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
