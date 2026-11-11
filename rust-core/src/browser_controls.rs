use std::collections::HashMap;

/// Cookie policy selectable by the user in Manta Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookiePolicy {
    AllowAll,
    PartitionThirdParty,
    BlockThirdParty,
    BlockAll,
}

/// Media autoplay policy selectable in Manta Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoplayPolicy {
    AllowAll,
    AllowMutedOnly,
    BlockAll,
}

/// Complete browser settings & high-velocity kernel controls matrix (`MantaSettings`).
#[derive(Debug, Clone)]
pub struct MantaSettings {
    pub search_engine_name: String,
    pub search_url_template: String,
    pub homepage_url: String,
    pub new_tab_url: String,
    pub javascript_enabled: bool,
    pub images_enabled: bool,
    pub popups_blocked: bool,
    pub do_not_track: bool,
    pub global_privacy_control: bool,
    pub https_only_mode: bool,
    pub dns_preconnect_enabled: bool,
    pub dark_mode_forced: bool,
    pub default_zoom_percent: u16,
    pub font_scale_percent: u16,
    pub minimum_font_size_sp: u8,
    pub cookie_policy: CookiePolicy,
    pub autoplay_policy: AutoplayPolicy,
    pub download_auto_save: bool,
    pub download_directory: String,
    pub custom_user_agent: String,
    /// Keeps all open tabs active in memory until explicitly closed by the user.
    pub keep_tabs_active_until_closed: bool,
    /// Automatically saves user-typed form/input fields per origin so site inputs are never lost.
    pub auto_save_form_inputs: bool,
    /// Promotes scroll-blocking touch/wheel listeners to `{ passive: true }` for 120 FPS scrolling.
    pub kernel_passive_events: bool,
    /// Speculatively pre-warms DNS/TCP on link hover/touchstart before click completion.
    pub kernel_speculative_preconnect: bool,
    /// Coalesces idle & layout tasks to prevent main-thread jank.
    pub kernel_layout_accelerator: bool,
    /// Injects the 99% modern web compatibility layer (ES2024, Client Hints, SPA router hooks).
    pub web_compat_99_enabled: bool,
}

impl Default for MantaSettings {
    fn default() -> Self {
        Self {
            search_engine_name: "Google".to_string(),
            search_url_template: "https://www.google.com/search?q={query}".to_string(),
            homepage_url: "manta://start".to_string(),
            new_tab_url: "manta://start".to_string(),
            javascript_enabled: true,
            images_enabled: true,
            popups_blocked: true,
            do_not_track: true,
            global_privacy_control: true,
            https_only_mode: false,
            dns_preconnect_enabled: true,
            dark_mode_forced: true,
            default_zoom_percent: 100,
            font_scale_percent: 100,
            minimum_font_size_sp: 10,
            cookie_policy: CookiePolicy::PartitionThirdParty,
            autoplay_policy: AutoplayPolicy::AllowMutedOnly,
            download_auto_save: true,
            download_directory: String::from("./downloads"),
            custom_user_agent: String::new(),
            keep_tabs_active_until_closed: true,
            auto_save_form_inputs: true,
            kernel_passive_events: true,
            kernel_speculative_preconnect: true,
            kernel_layout_accelerator: true,
            web_compat_99_enabled: true,
        }
    }
}

impl MantaSettings {
    /// Resolves any omnibox input (URL, IP, localhost, `manta://`, or search query) into a target URL.
    pub fn resolve_omnibox_input(&self, raw_input: &str) -> String {
        let trimmed = raw_input.trim();
        if trimmed.is_empty() {
            return self.new_tab_url.clone();
        }
        if trimmed.starts_with("manta://")
            || trimmed.starts_with("http://")
            || trimmed.starts_with("https://")
            || trimmed.starts_with("data:")
        {
            return trimmed.to_string();
        }
        if trimmed.starts_with("localhost") || trimmed.starts_with("127.0.0.1") {
            return format!("http://{}", trimmed);
        }
        if trimmed.contains('.') && !trimmed.contains(' ') {
            return format!("https://{}", trimmed);
        }
        let encoded = url_encode_query(trimmed);
        self.search_url_template.replace("{query}", &encoded)
    }

    /// Updates a setting dynamically by key name.
    pub fn set_by_key(&mut self, key: &str, value: &str) -> bool {
        let v = value.trim();
        match key.trim() {
            "search_engine_name" => self.search_engine_name = v.to_string(),
            "search_url_template" => self.search_url_template = v.to_string(),
            "homepage_url" => self.homepage_url = v.to_string(),
            "new_tab_url" => self.new_tab_url = v.to_string(),
            "javascript_enabled" => self.javascript_enabled = parse_bool(v),
            "images_enabled" => self.images_enabled = parse_bool(v),
            "popups_blocked" => self.popups_blocked = parse_bool(v),
            "do_not_track" => self.do_not_track = parse_bool(v),
            "global_privacy_control" => self.global_privacy_control = parse_bool(v),
            "https_only_mode" => self.https_only_mode = parse_bool(v),
            "dns_preconnect_enabled" => self.dns_preconnect_enabled = parse_bool(v),
            "dark_mode_forced" => self.dark_mode_forced = parse_bool(v),
            "keep_tabs_active_until_closed" => self.keep_tabs_active_until_closed = parse_bool(v),
            "auto_save_form_inputs" => self.auto_save_form_inputs = parse_bool(v),
            "kernel_passive_events" => self.kernel_passive_events = parse_bool(v),
            "kernel_speculative_preconnect" => self.kernel_speculative_preconnect = parse_bool(v),
            "kernel_layout_accelerator" => self.kernel_layout_accelerator = parse_bool(v),
            "web_compat_99_enabled" => self.web_compat_99_enabled = parse_bool(v),
            "default_zoom_percent" => {
                if let Ok(z) = v.parse::<u16>() {
                    self.default_zoom_percent = z.clamp(25, 500);
                } else {
                    return false;
                }
            }
            "font_scale_percent" => {
                if let Ok(s) = v.parse::<u16>() {
                    self.font_scale_percent = s.clamp(50, 300);
                } else {
                    return false;
                }
            }
            "download_auto_save" => self.download_auto_save = parse_bool(v),
            "download_directory" => self.download_directory = v.to_string(),
            "custom_user_agent" => self.custom_user_agent = v.to_string(),
            "cookie_policy" => {
                self.cookie_policy = match v.to_ascii_lowercase().as_str() {
                    "allow_all" => CookiePolicy::AllowAll,
                    "block_third_party" => CookiePolicy::BlockThirdParty,
                    "block_all" => CookiePolicy::BlockAll,
                    _ => CookiePolicy::PartitionThirdParty,
                };
            }
            "autoplay_policy" => {
                self.autoplay_policy = match v.to_ascii_lowercase().as_str() {
                    "allow_all" => AutoplayPolicy::AllowAll,
                    "block_all" => AutoplayPolicy::BlockAll,
                    _ => AutoplayPolicy::AllowMutedOnly,
                };
            }
            _ => return false,
        }
        true
    }

    pub fn export_json(&self) -> String {
        format!(
            concat!(
                "{{",
                "\"search_engine_name\":\"{}\",",
                "\"search_url_template\":\"{}\",",
                "\"homepage_url\":\"{}\",",
                "\"javascript_enabled\":{},",
                "\"images_enabled\":{},",
                "\"popups_blocked\":{},",
                "\"do_not_track\":{},",
                "\"global_privacy_control\":{},",
                "\"https_only_mode\":{},",
                "\"dns_preconnect_enabled\":{},",
                "\"dark_mode_forced\":{},",
                "\"default_zoom_percent\":{},",
                "\"font_scale_percent\":{},",
                "\"download_auto_save\":{},",
                "\"download_directory\":\"{}\",",
                "\"keep_tabs_active_until_closed\":{},",
                "\"auto_save_form_inputs\":{},",
                "\"kernel_passive_events\":{},",
                "\"kernel_speculative_preconnect\":{},",
                "\"kernel_layout_accelerator\":{},",
                "\"web_compat_99_enabled\":{}",
                "}}"
            ),
            escape_json(&self.search_engine_name),
            escape_json(&self.search_url_template),
            escape_json(&self.homepage_url),
            self.javascript_enabled,
            self.images_enabled,
            self.popups_blocked,
            self.do_not_track,
            self.global_privacy_control,
            self.https_only_mode,
            self.dns_preconnect_enabled,
            self.dark_mode_forced,
            self.default_zoom_percent,
            self.font_scale_percent,
            self.download_auto_save,
            escape_json(&self.download_directory),
            self.keep_tabs_active_until_closed,
            self.auto_save_form_inputs,
            self.kernel_passive_events,
            self.kernel_speculative_preconnect,
            self.kernel_layout_accelerator,
            self.web_compat_99_enabled,
        )
    }
}

/// Per-tab navigation, zoom, find-in-page, reader mode, and media controls managed in Rust.
#[derive(Debug, Clone)]
pub struct TabControlState {
    pub tab_id: u32,
    pub history_entries: Vec<(String, String)>, // (url, title)
    pub history_cursor: usize,
    pub desktop_mode: bool,
    pub reader_mode: bool,
    pub zoom_percent: u16,
    pub audio_muted: bool,
    pub fullscreen: bool,
    pub find_query: String,
    pub find_match_count: usize,
    pub find_active_ordinal: usize,
}

impl TabControlState {
    pub fn new(tab_id: u32, default_zoom: u16) -> Self {
        Self {
            tab_id,
            history_entries: Vec::with_capacity(16),
            history_cursor: 0,
            desktop_mode: false,
            reader_mode: false,
            zoom_percent: default_zoom,
            audio_muted: false,
            fullscreen: false,
            find_query: String::new(),
            find_match_count: 0,
            find_active_ordinal: 0,
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
        if self.history_entries.len() >= 128 {
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

/// Manages global `MantaSettings` and per-tab `TabControlState` across all open tabs.
pub struct MantaControlsManager {
    pub settings: MantaSettings,
    tab_controls: HashMap<u32, TabControlState>,
}

impl Default for MantaControlsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MantaControlsManager {
    pub fn new() -> Self {
        Self {
            settings: MantaSettings::default(),
            tab_controls: HashMap::new(),
        }
    }

    pub fn ensure_tab_state(&mut self, tab_id: u32) -> &mut TabControlState {
        let default_zoom = self.settings.default_zoom_percent;
        self.tab_controls
            .entry(tab_id)
            .or_insert_with(|| TabControlState::new(tab_id, default_zoom))
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
        self.ensure_tab_state(tab_id).zoom_percent = zoom.clamp(25, 500);
    }

    pub fn get_zoom_percent(&self, tab_id: u32) -> u16 {
        self.tab_controls
            .get(&tab_id)
            .map(|s| s.zoom_percent)
            .unwrap_or(self.settings.default_zoom_percent)
    }

    pub fn update_find_in_page(&mut self, tab_id: u32, query: &str, total_matches: usize) {
        let st = self.ensure_tab_state(tab_id);
        st.find_query = query.to_string();
        st.find_match_count = total_matches;
        st.find_active_ordinal = if total_matches > 0 { 1 } else { 0 };
    }

    pub fn remove_tab(&mut self, tab_id: u32) {
        self.tab_controls.remove(&tab_id);
    }

    pub fn clear_all(&mut self) {
        self.tab_controls.clear();
    }
}

fn parse_bool(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "1" | "yes" | "on")
}

fn url_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manta_settings_and_navigation_controls() {
        let mut mgr = MantaControlsManager::new();
        assert_eq!(
            mgr.settings.resolve_omnibox_input("rust lang"),
            "https://www.google.com/search?q=rust+lang"
        );
        assert_eq!(
            mgr.settings.resolve_omnibox_input("manta.dev"),
            "https://manta.dev"
        );
        assert!(mgr.settings.keep_tabs_active_until_closed);
        assert!(mgr.settings.auto_save_form_inputs);

        assert!(mgr.settings.set_by_key("default_zoom_percent", "125"));
        let st = mgr.ensure_tab_state(1);
        assert_eq!(st.zoom_percent, 125);
        st.record_navigation("https://manta.dev", "Manta");
        st.record_navigation("https://manta.dev/docs", "Docs");
        assert!(st.can_go_back());
        assert_eq!(st.go_back().unwrap().0, "https://manta.dev");
    }
}
