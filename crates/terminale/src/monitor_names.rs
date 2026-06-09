//! Cross-platform friendly monitor name resolution and primary-monitor detection.
//!
//! # Friendly-name APIs
//!
//! [`friendly_monitor_name`] queries the OS for a human-readable display name
//! (e.g. `"BenQ EW3270U"`, `"Built-in Retina Display"`, `"HDMI-1"`).
//! [`friendly_monitor_label`] always returns a usable string: it falls back to
//! `"Display N (WxH)"` when the OS probe returns nothing.
//!
//! # Primary-monitor API
//!
//! [`os_primary_monitor`] returns the OS-authoritative primary monitor handle
//! from a slice of winit `MonitorHandle`s. On Windows it uses
//! `EnumDisplayMonitors` + `GetMonitorInfoW` to find the entry flagged with
//! `MONITORINFOF_PRIMARY` (independent of which monitor the application window
//! currently lives on, which is what `Window::primary_monitor()` may return).
//! On macOS and Linux it returns `None`, deferring to winit's
//! `Window::primary_monitor()` which is already authoritative there
//! (`NSScreen.screens()[0]` and XRandR primary / compositor-flagged output,
//! respectively).
//!
//! # Platform notes
//!
//! * **Windows** — calls `QueryDisplayConfig` + `DisplayConfigGetDeviceInfo`
//!   (`DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME`) to read the EDID-derived
//!   friendly name (e.g. `"Dell U2720Q"`, `"Generic PnP Monitor"`). This is
//!   the name shown in *Settings → Display*. Always better than winit's GDI
//!   device path (`\\.\DISPLAY1`).
//! * **macOS** — reads `NSScreen.localizedName` (10.15+). winit already returns
//!   this via `MonitorHandle::name()` on recent versions; we call it directly
//!   for robustness on older builds and to guarantee a non-empty result.
//! * **Linux / others** — trusts `MonitorHandle::name()`. winit returns the
//!   XRandR connector name on X11 (`HDMI-1`, `eDP-1`, `DP-2`) and
//!   `wl_output.name` on Wayland (GNOME 42+, KDE 5.27+). These are already
//!   user-friendly.
//!
//! # Invariants
//!
//! * This module MUST only be used from code that already runs on the main
//!   thread (the egui/winit event loop). On macOS the AppKit call requires it.
//! * No panics on any OS: every code path has an explicit fallback. In
//!   particular, winit's inherent `MonitorHandle::name()` / `size()` unwrap a
//!   fallible OS call and panic on a handle invalidated by a standby/resume
//!   cycle — always read those through [`monitor_name`] / [`monitor_size`],
//!   never the inherent methods.
//! * No new external crates for Linux; only the `windows-sys` crate (already
//!   in the workspace tree) is gated behind `cfg(target_os = "windows")`.

use winit::dpi::PhysicalSize;
use winit::monitor::MonitorHandle;

// ── Panic-safe monitor probes ──────────────────────────────────────────────

thread_local! {
    /// Set while a monitor probe ([`monitor_name`] / [`monitor_size`]) is inside
    /// its `catch_unwind`. The release-Windows panic hook (which pops a fatal
    /// message box) consults [`monitor_panic_is_caught`] to stay quiet for the
    /// panics we recover from here — a transient invalid monitor handle must not
    /// also flash a "fatal error" dialog.
    static MONITOR_PANIC_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the current thread is inside a monitor probe's `catch_unwind` — i.e.
/// an in-flight panic from a winit monitor query will be caught and degraded to
/// a `None` result.
// Only referenced by the release-Windows panic hook; dead code elsewhere.
#[allow(dead_code)]
pub(crate) fn monitor_panic_is_caught() -> bool {
    MONITOR_PANIC_GUARD.with(std::cell::Cell::get)
}

/// Run `f` under the monitor-panic guard, catching any unwind into `None`.
fn caught<T>(f: impl FnOnce() -> T) -> Option<T> {
    MONITOR_PANIC_GUARD.with(|g| g.set(true));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    MONITOR_PANIC_GUARD.with(|g| g.set(false));
    r.ok()
}

/// Panic-safe [`MonitorHandle::name`].
///
/// winit's `MonitorHandle::name()` calls `GetMonitorInfoW(..).unwrap()`
/// internally on Windows (`winit/src/platform_impl/windows/monitor.rs:155`).
/// When the OS resumes from standby it invalidates monitor handles, so
/// `GetMonitorInfoW` fails with `ERROR_INVALID_MONITOR_HANDLE` (1461, *"the
/// screen handle is not valid"*) and the inherent `name()` **panics** — most
/// often on a [`MonitorHandle`] we stored *before* standby. The other OS
/// back-ends can fault the same way on display reconfiguration. We catch the
/// unwind and return `None`; callers already treat a missing name as a soft
/// fallback. Always route monitor-name reads through this, never the inherent
/// `MonitorHandle::name()`.
pub fn monitor_name(mon: &MonitorHandle) -> Option<String> {
    caught(|| mon.name()).flatten()
}

/// Panic-safe [`MonitorHandle::size`]. winit's inherent `size()` unwraps the
/// same fallible `GetMonitorInfoW` call (`monitor.rs:171`); see [`monitor_name`]
/// for why that panics across a standby/resume cycle. Returns `None` on an
/// invalid handle so callers can fall back instead of crashing.
pub fn monitor_size(mon: &MonitorHandle) -> Option<PhysicalSize<u32>> {
    caught(|| mon.size())
}

// ── Platform-specific implementations ──────────────────────────────────────

#[cfg(target_os = "windows")]
mod imp {
    //! Windows implementation using `DisplayConfigGetDeviceInfo`.
    //!
    //! # Safety invariants
    //!
    //! Every `unsafe` block below calls Win32 functions that are
    //! `SAFETY`-annotated individually. All pointers are stack/vec
    //! references whose lifetimes outlive the Win32 call frame.

    use winit::monitor::MonitorHandle;

    // We use the windows-sys crate (already in the dependency tree via winit /
    // global-hotkey). Features required:
    //   Win32_Foundation          → ERROR_SUCCESS, BOOL
    //   Win32_Devices_Display     → QueryDisplayConfig, DisplayConfigGetDeviceInfo, …
    //   Win32_Graphics_Gdi        → EnumDisplayMonitors, GetMonitorInfoW, MONITORINFOEXW
    //   Win32_UI_WindowsAndMessaging → MONITORINFOF_PRIMARY (lives here, NOT in Gdi)
    use windows_sys::Win32::Devices::Display::{
        DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
        DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
        DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
    };
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HMONITOR, MONITORINFOEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    /// Query a friendly monitor name for the given [`MonitorHandle`].
    ///
    /// Returns `None` when the Win32 APIs fail or return an empty name.
    pub(super) fn friendly_name(mon: &MonitorHandle) -> Option<String> {
        // winit on Windows returns the GDI device path, e.g. "\\.\DISPLAY1".
        let gdi = super::monitor_name(mon)?;

        // Step 1: ask DisplayConfig how large the output buffers need to be.
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        let rc = unsafe {
            // SAFETY: both out-params are valid stack-allocated u32s.
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if rc != ERROR_SUCCESS {
            return None;
        }

        let mut paths = vec![
            // SAFETY: zeroed DISPLAYCONFIG_PATH_INFO is a valid default.
            unsafe { std::mem::zeroed::<DISPLAYCONFIG_PATH_INFO>() };
            path_count as usize
        ];
        let mut modes = vec![
            // SAFETY: zeroed DISPLAYCONFIG_MODE_INFO is a valid default.
            unsafe { std::mem::zeroed::<DISPLAYCONFIG_MODE_INFO>() };
            mode_count as usize
        ];

        // Step 2: populate the path + mode arrays.
        let rc = unsafe {
            // SAFETY: paths/modes are valid, correctly-sized, writable slices.
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }

        // Step 3: for each active path, check if the source GDI device name
        // matches `gdi`; if so, fetch the target's friendly name.
        for path in &paths[..path_count as usize] {
            // --- query the source device name ---
            let mut src = unsafe { std::mem::zeroed::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() };
            src.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: u32::try_from(std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>())
                    .unwrap_or(u32::MAX),
                adapterId: path.sourceInfo.adapterId,
                id: path.sourceInfo.id,
            };
            let rc = unsafe {
                // SAFETY: src is a valid, correctly-sized struct with a
                // matching `header.size` field — exactly what the API requires.
                DisplayConfigGetDeviceInfo(std::ptr::addr_of_mut!(src.header).cast())
            };
            #[allow(clippy::cast_possible_wrap)]
            if rc != ERROR_SUCCESS as i32 {
                continue;
            }

            // Convert the wide-char GDI name to a Rust String for comparison.
            let nul = src
                .viewGdiDeviceName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(src.viewGdiDeviceName.len());
            let src_gdi = String::from_utf16_lossy(&src.viewGdiDeviceName[..nul]);
            if src_gdi != gdi {
                continue;
            }

            // --- query the target (monitor) friendly name ---
            let mut tgt = unsafe { std::mem::zeroed::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() };
            tgt.header = DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                size: u32::try_from(std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>())
                    .unwrap_or(u32::MAX),
                adapterId: path.targetInfo.adapterId,
                id: path.targetInfo.id,
            };
            let rc = unsafe {
                // SAFETY: tgt is a valid, correctly-sized struct with a
                // matching `header.size` field.
                DisplayConfigGetDeviceInfo(std::ptr::addr_of_mut!(tgt.header).cast())
            };
            #[allow(clippy::cast_possible_wrap)]
            if rc != ERROR_SUCCESS as i32 {
                continue;
            }

            let nul = tgt
                .monitorFriendlyDeviceName
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(tgt.monitorFriendlyDeviceName.len());
            let name = String::from_utf16_lossy(&tgt.monitorFriendlyDeviceName[..nul]);
            let trimmed = name.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }

        None
    }

    /// Find the OS-flagged primary monitor by enumerating GDI monitor handles.
    ///
    /// Returns the GDI device name (e.g. `"\\.\DISPLAY1"`) of the monitor
    /// whose `MONITORINFOEXW.dwFlags` has `MONITORINFOF_PRIMARY` set.
    ///
    /// # Safety invariants
    ///
    /// `EnumDisplayMonitors` runs a callback that accumulates results into a
    /// `Vec` on the stack of this function.  The pointer we pass as `lparam`
    /// is a `*mut Vec<(HMONITOR, String)>` whose lifetime is strictly bounded
    /// by this call frame — the callback only executes synchronously during
    /// `EnumDisplayMonitors`, so it can never outlive the pointed-to value.
    pub(super) fn primary_monitor_by_enumeration() -> Option<String> {
        // Collect (HMONITOR, GDI-device-name) pairs via the callback.
        let mut monitors: Vec<(HMONITOR, String)> = Vec::new();

        unsafe extern "system" fn enum_cb(
            hmon: HMONITOR,
            _hdc: windows_sys::Win32::Graphics::Gdi::HDC,
            _rect: *mut windows_sys::Win32::Foundation::RECT,
            lparam: windows_sys::Win32::Foundation::LPARAM,
        ) -> windows_sys::Win32::Foundation::BOOL {
            // SAFETY: lparam is the raw pointer we passed below; its target is
            // a live `Vec<(HMONITOR, String)>` on the caller's stack frame.
            // The callback runs synchronously inside `EnumDisplayMonitors`, so
            // the pointer is valid for the entire execution window here.
            let out = &mut *(lparam as *mut Vec<(HMONITOR, String)>);
            let mut info = std::mem::zeroed::<MONITORINFOEXW>();
            info.monitorInfo.cbSize =
                u32::try_from(std::mem::size_of::<MONITORINFOEXW>()).unwrap_or(u32::MAX);
            // SAFETY: `info` is a correctly-sized, writable struct whose
            // `cbSize` is initialised — exactly what `GetMonitorInfoW` requires.
            if GetMonitorInfoW(hmon, std::ptr::addr_of_mut!(info.monitorInfo)) != 0 {
                let nul = info
                    .szDevice
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(info.szDevice.len());
                let name = String::from_utf16_lossy(&info.szDevice[..nul]);
                out.push((hmon, name));
            }
            1 // TRUE — keep enumerating
        }

        unsafe {
            // SAFETY: `monitors` is alive for the entire `EnumDisplayMonitors`
            // call; the callback pointer is a valid `extern "system" fn`.
            EnumDisplayMonitors(
                std::ptr::null_mut(), // hdc = NULL → enumerate all monitors
                std::ptr::null(),
                Some(enum_cb),
                std::ptr::addr_of_mut!(monitors) as windows_sys::Win32::Foundation::LPARAM,
            );
        }

        // Find the entry with MONITORINFOF_PRIMARY.
        for (hmon, gdi_name) in &monitors {
            let mut info = unsafe { std::mem::zeroed::<MONITORINFOEXW>() };
            info.monitorInfo.cbSize =
                u32::try_from(std::mem::size_of::<MONITORINFOEXW>()).unwrap_or(u32::MAX);
            let ok = unsafe {
                // SAFETY: `info` is a correctly-sized struct with a valid `cbSize`.
                GetMonitorInfoW(*hmon, std::ptr::addr_of_mut!(info.monitorInfo))
            };
            if ok != 0 && (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0 {
                return Some(gdi_name.clone());
            }
        }

        None
    }
}

// macOS: read NSScreen.localizedName via winit's name() which already returns
// it on recent macOS (10.15+). We also try it first; only fall through to our
// own AppKit query if the result looks like a path token.
#[cfg(target_os = "macos")]
mod imp {
    use winit::monitor::MonitorHandle;

    /// Return the localized display name.
    ///
    /// winit already calls `NSScreen.localizedName` on macOS and returns it
    /// from `MonitorHandle::name()`. We just unwrap and validate it here.
    pub(super) fn friendly_name(mon: &MonitorHandle) -> Option<String> {
        let n = super::monitor_name(mon)?;
        let t = n.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    /// On macOS winit's `Window::primary_monitor()` already returns
    /// `NSScreen.screens()[0]` which is the OS-authoritative primary.
    /// No supplemental GDI-style enumeration is needed.
    pub(super) fn primary_monitor_by_enumeration() -> Option<String> {
        None
    }
}

// Linux / other unixes: XRandR output names and wl_output.name are already
// user-friendly (HDMI-1, DP-2, eDP-1). Just sanitise and return.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod imp {
    use winit::monitor::MonitorHandle;

    /// Return the connector name exposed by winit (XRandR / `wl_output.name`).
    pub(super) fn friendly_name(mon: &MonitorHandle) -> Option<String> {
        let n = super::monitor_name(mon)?;
        let t = n.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    }

    /// On Linux winit's `Window::primary_monitor()` already returns the
    /// XRandR primary (set via `xrandr --primary`) on X11 and the
    /// compositor-flagged output on Wayland. No supplemental enumeration
    /// is needed.
    pub(super) fn primary_monitor_by_enumeration() -> Option<String> {
        None
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Probe the OS for the user-visible name of `mon`.
///
/// Returns `None` when the platform cannot supply a name (rare; the caller
/// should fall back to an index-based label via [`friendly_monitor_label`]).
pub fn friendly_monitor_name(mon: &MonitorHandle) -> Option<String> {
    imp::friendly_name(mon)
}

/// Return the OS-authoritative primary [`MonitorHandle`] from `monitors`.
///
/// On **Windows** this enumerates GDI monitor handles via
/// `EnumDisplayMonitors` + `GetMonitorInfoW`, picks the one with
/// `MONITORINFOF_PRIMARY`, then matches its GDI device name against each
/// handle's `MonitorHandle::name()` (also a GDI path on Windows).  This is
/// independent of which display the application window currently lives on,
/// which is the known failure mode of `Window::primary_monitor()` on Windows.
///
/// On **macOS** and **Linux** returns `None`; callers should fall back to
/// `Window::primary_monitor()` which is already authoritative there.
///
/// Returns `None` on an empty slice (no monitors connected), on any Win32
/// API failure, or when no `MONITORINFOF_PRIMARY` entry is found (degenerate
/// driver state).
pub fn os_primary_monitor(monitors: &[MonitorHandle]) -> Option<MonitorHandle> {
    if monitors.is_empty() {
        return None;
    }
    let primary_gdi = imp::primary_monitor_by_enumeration()?;
    // Match the GDI device name against winit's handle names (also GDI on
    // Windows; pass-through from XRandR/wl_output on other platforms).
    monitors
        .iter()
        .find(|m| monitor_name(m).as_deref() == Some(primary_gdi.as_str()))
        .cloned()
}

/// Returns `true` when `s` looks like a raw Win32 device-namespace path
/// (e.g. `\\.\DISPLAY1` or `\\?\...`). Used as a safety net so we never
/// surface a GDI path even if a future winit version leaks one through.
fn looks_like_gdi_path(s: &str) -> bool {
    s.starts_with(r"\\.\") || s.starts_with(r"\\?\")
}

/// Always returns a usable label for the given monitor.
///
/// Preference order:
/// 1. [`friendly_monitor_name`] — the OS-supplied friendly name.
/// 2. `"Display {idx+1} ({w}x{h})"` — synthesised from index and resolution.
///
/// The `zero_based_idx` is the 0-based position in the `available_monitors()`
/// enumeration (passed through as-is; the label displays `idx + 1`).
pub fn friendly_monitor_label(mon: &MonitorHandle, zero_based_idx: usize) -> String {
    if let Some(name) = friendly_monitor_name(mon) {
        if !name.is_empty() && !looks_like_gdi_path(&name) {
            return name;
        }
    }
    let size = monitor_size(mon).unwrap_or_default();
    format!(
        "Display {} ({}x{})",
        zero_based_idx + 1,
        size.width,
        size.height
    )
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::{caught, looks_like_gdi_path, monitor_panic_is_caught, os_primary_monitor};

    /// A probe that panics (winit's inherent `name()`/`size()` do exactly this
    /// on a monitor handle invalidated by standby/resume) must be degraded to
    /// `None`, not propagated. This is the whole point of the panic-safe layer.
    #[test]
    fn caught_swallows_panic_into_none() {
        let r: Option<u32> = caught(|| panic!("invalid display handle"));
        assert!(r.is_none());
    }

    /// A non-panicking probe returns its value untouched.
    #[test]
    fn caught_passes_value_through() {
        assert_eq!(caught(|| 42_u32), Some(42));
    }

    /// The guard the panic hook reads must be reset to `false` after a caught
    /// panic — otherwise a *subsequent*, genuinely-fatal panic would be wrongly
    /// suppressed. Suppress the default hook's noise for the deliberate panic.
    #[test]
    fn guard_is_cleared_after_caught_panic() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = caught(|| panic!("boom"));
        std::panic::set_hook(prev);
        assert!(
            !monitor_panic_is_caught(),
            "guard leaked true past the catch_unwind"
        );
    }

    #[test]
    fn gdi_path_detection() {
        assert!(looks_like_gdi_path(r"\\.\DISPLAY1"));
        assert!(looks_like_gdi_path(r"\\.\DISPLAY2"));
        assert!(looks_like_gdi_path(r"\\?\{...}"));
        assert!(!looks_like_gdi_path("BenQ EW3270U"));
        assert!(!looks_like_gdi_path("HDMI-1"));
        assert!(!looks_like_gdi_path("Built-in Retina Display"));
        assert!(!looks_like_gdi_path(""));
    }

    /// `os_primary_monitor` must never panic on an empty slice and must
    /// return `None` (no monitors → no primary).
    #[test]
    fn os_primary_monitor_empty_slice_returns_none() {
        assert!(os_primary_monitor(&[]).is_none());
    }
}
