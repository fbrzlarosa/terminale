//! Window management, tabs, drag-out, and Quake mode for `terminale`.
//!
//! Built on `winit` for cross-platform windowing and `global-hotkey` for the
//! Quake-style toggle. A real `App` driver will spawn windows and route input
//! events to sessions in `terminale-core`.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

use terminale_config::Config;
use terminale_core::SessionId;

/// State of a single tab inside a window.
#[derive(Debug, Clone)]
pub struct TabState {
    /// Display title shown in the tab strip.
    pub title: String,
    /// The session backing this tab.
    pub session: SessionId,
    /// Whether this tab is the active one in its window.
    pub active: bool,
}

impl TabState {
    /// Construct a new tab bound to the given session.
    #[must_use]
    pub fn new(title: impl Into<String>, session: SessionId) -> Self {
        Self {
            title: title.into(),
            session,
            active: false,
        }
    }
}

/// Aggregate application state, shared across windows.
#[derive(Debug)]
pub struct AppState {
    /// Loaded configuration.
    pub config: Config,
    /// Tabs across all windows, in insertion order.
    pub tabs: Vec<TabState>,
}

impl AppState {
    /// Create an empty state with the given configuration.
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            tabs: Vec::new(),
        }
    }

    /// Push a new tab and return its index.
    pub fn push_tab(&mut self, tab: TabState) -> usize {
        let idx = self.tabs.len();
        self.tabs.push(tab);
        idx
    }

    /// Activate the tab at `idx`, deactivating all others. Returns `false`
    /// if `idx` is out of range.
    pub fn activate(&mut self, idx: usize) -> bool {
        if idx >= self.tabs.len() {
            return false;
        }
        for (i, t) in self.tabs.iter_mut().enumerate() {
            t.active = i == idx;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activates_the_requested_tab() {
        let mut app = AppState::new(Config::default());
        let s1 = SessionId::new();
        let s2 = SessionId::new();
        app.push_tab(TabState::new("a", s1));
        app.push_tab(TabState::new("b", s2));
        assert!(app.activate(1));
        assert!(!app.tabs[0].active);
        assert!(app.tabs[1].active);
    }

    #[test]
    fn rejects_out_of_range_activation() {
        let mut app = AppState::new(Config::default());
        assert!(!app.activate(0));
    }
}
