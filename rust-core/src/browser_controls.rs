use std::collections::HashMap;

/// Per-tab Chrome-parity browser controls managed in Rust:
/// - Navigation history stack (Back, Forward, CanGoBack, CanGoForward)
/// - Desktop Site mode (`request_desktop_site`)
/// - Page Zoom percentage (`50%` ..= `300%`)
/// - Audio Mute toggle
/// - Find-in-Page state
#[derive(Debug, Clone)]
pub struct TabControlState {
    pub tab_id: u32,
    pub history_entries: Vec<(String, String)>, // (url, title)
    pub history_cursor: usize,
    pub desktop_mode: bool,
    pub zoom_percent: u16,
    pub audio_muted: bool,
    pub last_find_query: String,
}

impl TabControlState {
    pub fn new(tab_id: u32) -> Self {
        Self {
            tab_id,
            history_entries: Vec::with_capacity(16),
            history_cursor: 0,
            desktop_mode: false,
            zoom_percent: 100,
            audio_muted: false,
            last_find_query: String::new(),
        }
    }

    pub fn record_navigation(&mut self, url: &str, title: &str) {
        if url.trim().is_empty() {
            return;
        }
        if !self.history_entries.is_empty() && self.history_cursor + 1 < self.history_entries.len()
        {
            self.history_entries.truncate(self.history_cursor + 1);
        }
        if let Some((last_url, last_title)) = self.history_entries.last_mut() {
            if last_url == url {
                if !title.is_empty() {
                    *last_title = title.to_string();
                }
                return;
            }
        }
        if self.history_entries.len() >= 64 {
            self.history_entries.remove(0);
        }
        self.history_entries
            .push((url.to_string(), title.to_string()));
        self.history_cursor = self.history_entries.len().saturating_sub(1);
    }

    pub fn can_go_back(&self) -> bool {
        self.history_cursor > 0 && !self.history_entries.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.history_entries.is_empty() && self.history_cursor + 1 < self.history_entries.len()
    }

    pub fn go_back(&mut self) -> Option<&(String, String)> {
        if self.can_go_back() {
            self.history_cursor -= 1;
            self.history_entries.get(self.history_cursor)
        } else {
            None
        }
    }

    pub fn go_forward(&mut self) -> Option<&(String, String)> {
        if self.can_go_forward() {
            self.history_cursor += 1;
            self.history_entries.get(self.history_cursor)
        } else {
            None
        }
    }
}

/// Manages Chrome-parity controls across all open tabs.
pub struct ChromeControlsManager {
    tab_controls: HashMap<u32, TabControlState>,
}

impl Default for ChromeControlsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeControlsManager {
    pub fn new() -> Self {
        Self {
            tab_controls: HashMap::new(),
        }
    }

    pub fn ensure_tab_state(&mut self, tab_id: u32) -> &mut TabControlState {
        self.tab_controls
            .entry(tab_id)
            .or_insert_with(|| TabControlState::new(tab_id))
    }

    pub fn set_desktop_mode(&mut self, tab_id: u32, enabled: bool) {
        self.ensure_tab_state(tab_id).desktop_mode = enabled;
    }

    pub fn is_desktop_mode(&self, tab_id: u32) -> bool {
        self.tab_controls
            .get(&tab_id)
            .map(|s| s.desktop_mode)
            .unwrap_or(false)
    }

    pub fn set_zoom_percent(&mut self, tab_id: u32, zoom: u16) {
        self.ensure_tab_state(tab_id).zoom_percent = zoom.clamp(50, 300);
    }

    pub fn get_zoom_percent(&self, tab_id: u32) -> u16 {
        self.tab_controls
            .get(&tab_id)
            .map(|s| s.zoom_percent)
            .unwrap_or(100)
    }

    pub fn remove_tab(&mut self, tab_id: u32) {
        self.tab_controls.remove(&tab_id);
    }

    pub fn clear_all(&mut self) {
        self.tab_controls.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigation_history_and_zoom_controls() {
        let mut mgr = ChromeControlsManager::new();
        let st = mgr.ensure_tab_state(1);
        st.record_navigation("https://google.com", "Google");
        st.record_navigation("https://github.com", "GitHub");
        st.record_navigation("https://rust-lang.org", "Rust");

        assert!(st.can_go_back());
        assert!(!st.can_go_forward());
        assert_eq!(st.go_back().unwrap().0, "https://github.com");
        assert!(st.can_go_forward());
        assert_eq!(st.go_forward().unwrap().0, "https://rust-lang.org");

        mgr.set_zoom_percent(1, 150);
        assert_eq!(mgr.get_zoom_percent(1), 150);
        mgr.set_desktop_mode(1, true);
        assert!(mgr.is_desktop_mode(1));
    }
}
