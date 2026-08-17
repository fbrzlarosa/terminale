//! Global Quake hotkey on Wayland, via the XDG desktop portal.
//!
//! On Windows, macOS and X11 the Quake toggle is a system-wide key grab owned
//! by [`global_hotkey`]. Wayland has no equivalent: a compositor never lets a
//! client see keys pressed while another window has focus, by design. Under a
//! Wayland session the X11 grab still *registers* (XWayland accepts it) but
//! only ever fires while an X11 window has focus — so from the user's side the
//! Quake hotkey simply does nothing, which is exactly the symptom this module
//! removes.
//!
//! `org.freedesktop.portal.GlobalShortcuts` is the sanctioned replacement
//! (GNOME 48+, KDE Plasma 6+). The desktop — not the app — owns the binding:
//! terminale asks for a shortcut with a *preferred* trigger, the desktop
//! confirms it with the user once and remembers it, and from then on delivers
//! an `Activated` signal whenever it fires, regardless of focus. The user can
//! also re-bind it later from their desktop's keyboard settings, which is
//! strictly better than an app-owned grab.
//!
//! Everything here is best-effort: no portal, no backend, or a user who
//! declines the binding all degrade to "the X11 grab is what you get", with one
//! log line saying so.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
use futures_util::StreamExt;
use winit::event_loop::EventLoopProxy;

use crate::desktop_shortcut::xdg_trigger;
use crate::UserEvent;

/// Our application-side id for the shortcut. Stable across releases: the
/// desktop keys the user's remembered binding off it.
const SHORTCUT_ID: &str = "toggle-quake";

/// Description the desktop shows in its shortcut confirmation dialog and in
/// the system keyboard-settings list.
const SHORTCUT_DESCRIPTION: &str = "Show or hide the terminale drop-down (Quake mode)";

/// Register the Quake toggle with the desktop's global-shortcuts portal.
///
/// Spawns a task on `runtime` that owns the portal session for the process
/// lifetime and forwards each activation to the winit loop as
/// [`UserEvent::ToggleQuake`]. Returns immediately; the portal round-trip (and
/// the user-facing confirmation dialog it may show the first time) happens in
/// the background so startup is never blocked on it.
///
/// `binding` is the user's configured hotkey in terminale syntax
/// (e.g. `Ctrl+\`); it is translated into the XDG "shortcuts" trigger syntax
/// and offered to the desktop as the *preferred* trigger. The desktop is free
/// to assign something else — it owns the final say — so the app never assumes
/// which keys ended up bound.
pub(crate) fn spawn(
    runtime: &tokio::runtime::Runtime,
    binding: &str,
    proxy: EventLoopProxy<UserEvent>,
) {
    let trigger = xdg_trigger(binding);
    if trigger.is_none() && !binding.trim().is_empty() {
        tracing::debug!(
            binding,
            "could not express this hotkey in XDG shortcut syntax; \
             the portal will pick a trigger instead"
        );
    }
    runtime.spawn(async move {
        if let Err(e) = run(trigger, proxy).await {
            // The common failure on a normal (non-Flatpak) install is
            // "An app id is required": the portal identifies callers by their
            // application id, which a process only carries when the desktop
            // launched it from its `.desktop` entry — not when it was started
            // from a shell. Say so, and point at the path that always works.
            tracing::info!(
                ?e,
                "the desktop's global-shortcuts portal did not accept the Quake shortcut \
                 (launching terminale from the application menu rather than a shell is \
                 what gives it the application id the portal requires). Bind a key to \
                 `terminale --toggle-quake` instead — Settings › Desktop integration has \
                 a one-click button for it on GNOME"
            );
        }
    });
}

/// Own the portal session and pump activations until the event loop goes away.
async fn run(
    trigger: Option<String>,
    proxy: EventLoopProxy<UserEvent>,
) -> Result<(), ashpd::Error> {
    let portal = GlobalShortcuts::new().await?;
    let session = portal.create_session().await?;

    // Subscribe BEFORE binding: the desktop may activate the shortcut as soon
    // as the binding is confirmed, and a stream created afterwards would miss
    // that first press.
    let mut activations = portal.receive_activated().await?;

    let mut shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION);
    if let Some(trigger) = trigger.as_deref() {
        shortcut = shortcut.preferred_trigger(trigger);
    }
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None)
        .await?
        .response()?;

    if !bound.shortcuts().iter().any(|s| s.id() == SHORTCUT_ID) {
        tracing::info!(
            "the desktop did not bind the Quake shortcut (declined, or already bound \
             elsewhere); keeping the X11 key grab"
        );
        return Ok(());
    }
    for s in bound.shortcuts() {
        tracing::info!(
            id = s.id(),
            trigger = s.trigger_description(),
            "Quake hotkey registered with the desktop global-shortcuts portal"
        );
    }
    // Tell the app to release the OS key grab: under Wayland it can still fire
    // while an XWayland window has focus, and two live registrations would
    // toggle twice per press.
    if proxy.send_event(UserEvent::PortalShortcutBound).is_err() {
        return Ok(());
    }

    while let Some(activated) = activations.next().await {
        if activated.shortcut_id() != SHORTCUT_ID {
            continue;
        }
        if proxy.send_event(UserEvent::ToggleQuake).is_err() {
            // Event loop gone — drop the session and stop.
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shortcut id is what the desktop keys the user's remembered binding
    /// off, so it must stay byte-stable across releases.
    #[test]
    fn shortcut_id_is_stable() {
        assert_eq!(SHORTCUT_ID, "toggle-quake");
    }

    /// The preferred trigger handed to the portal comes from the user's own
    /// config binding, in XDG shortcut syntax.
    #[test]
    fn preferred_trigger_comes_from_the_config_binding() {
        assert_eq!(xdg_trigger("Ctrl+\\").as_deref(), Some("CTRL+backslash"));
    }
}
