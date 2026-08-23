//! Focus-loss policy shared by the popup windows (confirm-close, paste guard,
//! password prompt, context menu).
//!
//! Every one of those popups wants the same rule: *losing focus means the user
//! clicked somewhere else, so give up and close*. Implemented naively — cancel
//! on any `WindowEvent::Focused(false)` — that rule closes the popup within a
//! millisecond of opening it, because two platforms deliver a focus-loss the
//! popup never earned:
//!
//! * **X11.** Handing focus to a freshly mapped window produces a
//!   `Focused(false)` *for that new window*, delivered just before the
//!   `Focused(true)` that actually gives it focus. Measured on GNOME/Mutter
//!   (XWayland), the confirm-close dialog: opened at T, `Focused(false)` at
//!   T+1.3 ms, the parent's own `Focused(false)` at T+2.1 ms, and the dialog's
//!   real `Focused(true)` at T+2.2 ms. A popup that cancels on the first event
//!   is gone before the user can read it — and since the close it was asking
//!   about is then treated as cancelled, closing a window becomes impossible.
//! * **macOS.** A trackpad two-finger tap hands focus straight back to the
//!   parent once the tap's press/release completes, a few ms after the popup it
//!   opened has taken focus.
//!
//! So a focus-loss only counts as a click-outside once the popup has actually
//! held focus ([`PopupFocus::focus_gained`]) *and* has been on screen longer
//! than [`FOCUS_GRACE`]. Anything earlier is [`FocusLoss::Spurious`]: the
//! caller ignores it, and re-asserts focus so the keyboard routes (Esc, Enter)
//! keep working even when the platform bounced focus back to the parent.

use std::time::{Duration, Instant};

/// How long after opening a focus-loss is still treated as platform noise.
///
/// Sized for the macOS trackpad tap (a press/release that can span a couple of
/// hundred ms), which is far longer than the X11 map-time ordering needs.
const FOCUS_GRACE: Duration = Duration::from_millis(350);

/// What a `WindowEvent::Focused(false)` on a popup window actually means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FocusLoss {
    /// The user really did click (or tab) away — honour the popup's
    /// click-outside rule.
    ClickOutside,
    /// Platform noise around the popup taking focus in the first place. Ignore
    /// it, and re-assert focus.
    Spurious,
}

/// Tracks just enough about a popup's focus history to tell those two apart.
#[derive(Debug)]
pub(crate) struct PopupFocus {
    /// When the popup was created.
    opened_at: Instant,
    /// Whether a real `Focused(true)` has ever arrived.
    ever_focused: bool,
}

impl PopupFocus {
    /// Start tracking, as of now.
    pub(crate) fn new() -> Self {
        Self {
            opened_at: Instant::now(),
            ever_focused: false,
        }
    }

    /// Record a `WindowEvent::Focused(true)`.
    pub(crate) fn focus_gained(&mut self) {
        self.ever_focused = true;
    }

    /// Classify a `WindowEvent::Focused(false)`.
    pub(crate) fn focus_lost(&self) -> FocusLoss {
        self.classify(Instant::now())
    }

    /// [`Self::focus_lost`] against an explicit clock, so the policy is
    /// testable without sleeping.
    fn classify(&self, now: Instant) -> FocusLoss {
        if !self.ever_focused || now.saturating_duration_since(self.opened_at) < FOCUS_GRACE {
            FocusLoss::Spurious
        } else {
            FocusLoss::ClickOutside
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The X11 case: the focus-loss that arrives *before* the popup was ever
    /// focused must never close it. This is the one that made closing a window
    /// impossible.
    #[test]
    fn focus_loss_before_first_focus_is_spurious() {
        let f = PopupFocus::new();
        assert_eq!(f.focus_lost(), FocusLoss::Spurious);
        // Still spurious however long the popup has been up: never having held
        // focus means no click-outside can have happened.
        let f = PopupFocus::new();
        assert_eq!(
            f.classify(f.opened_at + FOCUS_GRACE * 10),
            FocusLoss::Spurious
        );
    }

    /// The macOS case: focused, then bounced back to the parent while the tap
    /// that opened the popup completes.
    #[test]
    fn focus_loss_inside_grace_is_spurious() {
        let mut f = PopupFocus::new();
        f.focus_gained();
        assert_eq!(f.focus_lost(), FocusLoss::Spurious);
    }

    /// The real thing: focused, settled, then the user clicked elsewhere.
    #[test]
    fn focus_loss_after_grace_is_a_click_outside() {
        let mut f = PopupFocus::new();
        f.focus_gained();
        assert_eq!(
            f.classify(f.opened_at + FOCUS_GRACE + Duration::from_millis(1)),
            FocusLoss::ClickOutside
        );
    }

    /// The grace window is measured from opening, not from the last event, so
    /// a popup that has been up for a while reacts to the first focus-loss.
    #[test]
    fn grace_is_measured_from_open() {
        let mut f = PopupFocus::new();
        f.focus_gained();
        let now = f.opened_at + FOCUS_GRACE;
        assert_eq!(f.classify(now), FocusLoss::ClickOutside);
        let just_before = f.opened_at + (FOCUS_GRACE - Duration::from_millis(1));
        assert_eq!(f.classify(just_before), FocusLoss::Spurious);
    }
}
