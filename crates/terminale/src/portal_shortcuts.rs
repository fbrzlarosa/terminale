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

/// How many times to ask the desktop to bind the shortcut before giving up.
const BIND_ATTEMPTS: u32 = 5;

/// Pause between bind attempts. Long enough for a lingering session from a
/// previous launch to be reaped, short enough that the hotkey is live before
/// the user reaches for it.
const BIND_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(700);

/// Ask the desktop which shortcuts this application already owns. `None` when
/// the call or its response failed — indistinguishable from "none" for our
/// purposes, and the caller treats both the same way.
async fn list_shortcuts(
    portal: &GlobalShortcuts<'_>,
    session: &ashpd::desktop::Session<'_, GlobalShortcuts<'_>>,
) -> Option<ashpd::desktop::global_shortcuts::ListShortcuts> {
    match portal.list_shortcuts(session).await {
        Ok(request) => match request.response() {
            Ok(list) => Some(list),
            Err(e) => {
                tracing::debug!(?e, "portal ListShortcuts response error");
                None
            }
        },
        Err(e) => {
            tracing::debug!(?e, "could not list existing portal shortcuts");
            None
        }
    }
}

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

    // Registering is not reliably a one-shot. Bindings persist per application
    // id, and the desktop refuses `BindShortcuts` while the app already owns
    // shortcuts — including, transiently, when a previous session of the same
    // app has not been torn down yet, which is exactly what a quick restart
    // looks like. Observed live: the identical call is accepted on one launch
    // and answered with a bare cancelled response on the next, leaving the
    // hotkey a coin flip.
    //
    // So: ask what we already own, bind only when the answer is nothing, and
    // give a refusal a few chances to clear before concluding the desktop means
    // it. The retries are cheap and happen on a background task, so a slow
    // convergence costs the user nothing.
    let mut bound_trigger: Option<String> = None;
    for attempt in 1..=BIND_ATTEMPTS {
        if let Some(existing) = list_shortcuts(&portal, &session).await {
            if let Some(s) = existing.shortcuts().iter().find(|s| s.id() == SHORTCUT_ID) {
                tracing::info!(
                    id = s.id(),
                    trigger = s.trigger_description(),
                    "Quake hotkey already registered with the desktop; reusing it"
                );
                bound_trigger = Some(s.trigger_description().to_string());
                break;
            }
        }

        let mut shortcut = NewShortcut::new(SHORTCUT_ID, SHORTCUT_DESCRIPTION);
        if let Some(trigger) = trigger.as_deref() {
            shortcut = shortcut.preferred_trigger(trigger);
        }
        match portal.bind_shortcuts(&session, &[shortcut], None).await {
            Ok(request) => match request.response() {
                Ok(bound) => {
                    if let Some(s) = bound.shortcuts().iter().find(|s| s.id() == SHORTCUT_ID) {
                        tracing::info!(
                            id = s.id(),
                            trigger = s.trigger_description(),
                            "Quake hotkey registered with the desktop global-shortcuts portal"
                        );
                        bound_trigger = Some(s.trigger_description().to_string());
                        break;
                    }
                    tracing::debug!(attempt, "portal accepted the bind but returned no shortcut");
                }
                Err(e) => tracing::debug!(?e, attempt, "portal refused the bind; retrying"),
            },
            Err(e) => tracing::debug!(?e, attempt, "bind request failed; retrying"),
        }
        tokio::time::sleep(BIND_RETRY_DELAY).await;
    }

    if bound_trigger.is_none() {
        tracing::info!(
            attempts = BIND_ATTEMPTS,
            "the desktop would not bind the Quake shortcut; keeping the X11 key grab \
             (under Wayland that only fires while an X11 window has focus — bind a key \
             to `terminale --toggle-quake` for one that always works)"
        );
        return Ok(());
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
