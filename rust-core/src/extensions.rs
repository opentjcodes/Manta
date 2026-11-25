use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::json_lite::{
    escape_json_str, json_get, json_get_i64, json_get_str, json_get_str_array, map_to_json_object,
    map_to_pretty_json, parse_json, JsonVal,
};
use crate::module_bus::{InterceptedRequest, MantaFeatureModule, ModuleDecision};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionRunAt {
    DocumentStart,
    DocumentEnd,
    DocumentIdle,
}

impl ExtensionRunAt {
    fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "document_start" => ExtensionRunAt::DocumentStart,
            "document_end" => ExtensionRunAt::DocumentEnd,
            _ => ExtensionRunAt::DocumentIdle,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContentScriptRule {
    pub matches: Vec<String>,
    pub exclude_matches: Vec<String>,
    pub js_sources: Vec<String>,
    pub css_sources: Vec<String>,
    pub run_at: ExtensionRunAt,
}

#[derive(Debug, Clone)]
pub struct FirefoxExtension {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_version: u8,
    pub enabled: bool,
    pub root_dir: Option<PathBuf>,
    pub permissions: Vec<String>,
    pub content_scripts: Vec<ContentScriptRule>,
    pub blocked_url_patterns: Vec<String>,
    pub storage_local: HashMap<String, String>,
    pub raw_manifest_json: String,
}

/// Network-layer blocker automatically synced with installed Firefox extensions
/// (supports MV3 `declarative_net_request` & custom `manta_block_rules` lists).
pub struct ExtensionNetModule {
    enabled: bool,
    shared_block_patterns: Arc<RwLock<Vec<String>>>,
}

impl MantaFeatureModule for ExtensionNetModule {
    fn id(&self) -> &'static str {
        "firefox_extensions_net"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn on_before_request(&self, req: &mut InterceptedRequest) -> ModuleDecision {
        let Ok(patterns) = self.shared_block_patterns.read() else {
            return ModuleDecision::Proceed;
        };
        if patterns.is_empty() {
            return ModuleDecision::Proceed;
        }
        let lower_url = req.url.to_ascii_lowercase();
        for pat in patterns.iter() {
            if matches_block_filter(pat, &lower_url) {
                return ModuleDecision::Block {
                    reason: "Blocked by installed Firefox Extension rule",
                };
            }
        }
        ModuleDecision::Proceed
    }
}

/// **Optional Firefox WebExtensions Host Engine (`extensions.rs`)**:
/// - Zero runtime cost when no extensions are installed (`is_empty()` fast path).
/// - Loads unpacked Firefox MV2 & MV3 extensions (`manifest.json`, `content_scripts`,
///   `css`, `js`, `declarative_net_request` rules) directly from user device folders.
/// - Persists per-extension `browser.storage.local` transparently as JSON under
///   `<storage_dir>/extensions_data/<ext_id>.json`.
pub struct ExtensionHost {
    extensions: HashMap<String, FirefoxExtension>,
    storage_root: Option<PathBuf>,
    shared_block_patterns: Arc<RwLock<Vec<String>>>,
}

impl Default for ExtensionHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionHost {
    pub fn new() -> Self {
        Self {
            extensions: HashMap::new(),
            storage_root: None,
            shared_block_patterns: Arc::new(RwLock::new(Vec::new())),
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    #[inline]
    pub fn installed_count(&self) -> usize {
        self.extensions.len()
    }

    pub fn set_storage_root(&mut self, root_dir: &str) {
        let trimmed = root_dir.trim();
        if !trimmed.is_empty() {
            self.storage_root = Some(PathBuf::from(trimmed));
        }
    }

    pub fn create_net_module(&self) -> Box<dyn MantaFeatureModule> {
        Box::new(ExtensionNetModule {
            enabled: true,
            shared_block_patterns: Arc::clone(&self.shared_block_patterns),
        })
    }

    fn sync_net_block_rules(&self) {
        if let Ok(mut guard) = self.shared_block_patterns.write() {
            guard.clear();
            for ext in self.extensions.values() {
                if ext.enabled {
                    guard.extend(ext.blocked_url_patterns.iter().cloned());
                }
            }
        }
    }

    /// Installs any unpacked Firefox extension folder containing `manifest.json` (MV2 or MV3).
    pub fn install_from_dir(&mut self, dir_path: &str) -> Result<String, String> {
        let root = PathBuf::from(dir_path.trim());
        let manifest_path = root.join("manifest.json");
        let raw = fs::read_to_string(&manifest_path)
            .map_err(|e| format!("Failed reading Firefox manifest.json: {}", e))?;

        let Some(JsonVal::Obj(pairs)) = parse_json(&raw) else {
            return Err("Invalid manifest.json structure".to_string());
        };

        let name = json_get_str(&pairs, "name").unwrap_or_else(|| "Unnamed Extension".to_string());
        let version = json_get_str(&pairs, "version").unwrap_or_else(|| "1.0.0".to_string());
        let manifest_version = json_get_i64(&pairs, "manifest_version").unwrap_or(3).clamp(2, 3) as u8;

        let gecko_id = json_get(&pairs, "browser_specific_settings")
            .and_then(|v| match v {
                JsonVal::Obj(p) => json_get(p, "gecko"),
                _ => None,
            })
            .and_then(|v| match v {
                JsonVal::Obj(p) => json_get_str(p, "id"),
                _ => None,
            });
        let id = sanitize_ext_id(gecko_id.as_deref().unwrap_or(&name));

        let mut permissions = json_get_str_array(&pairs, "permissions");
        permissions.extend(json_get_str_array(&pairs, "host_permissions"));

        let mut content_scripts = Vec::new();
        if let Some(JsonVal::Arr(cs_arr)) = json_get(&pairs, "content_scripts") {
            for item in cs_arr {
                let JsonVal::Obj(cs_obj) = item else {
                    continue;
                };
                let matches = json_get_str_array(cs_obj, "matches");
                let exclude_matches = json_get_str_array(cs_obj, "exclude_matches");
                let run_at = json_get_str(cs_obj, "run_at")
                    .map(|s| ExtensionRunAt::from_str(&s))
                    .unwrap_or(ExtensionRunAt::DocumentIdle);

                let mut js_sources = Vec::new();
                for rel_js in json_get_str_array(cs_obj, "js") {
                    if let Some(content) = read_safe_subfile(&root, &rel_js) {
                        js_sources.push(content);
                    }
                }
                let mut css_sources = Vec::new();
                for rel_css in json_get_str_array(cs_obj, "css") {
                    if let Some(content) = read_safe_subfile(&root, &rel_css) {
                        css_sources.push(content);
                    }
                }

                if !matches.is_empty() && (!js_sources.is_empty() || !css_sources.is_empty()) {
                    content_scripts.push(ContentScriptRule {
                        matches,
                        exclude_matches,
                        js_sources,
                        css_sources,
                        run_at,
                    });
                }
            }
        }

        let mut blocked_url_patterns = json_get_str_array(&pairs, "manta_block_rules");
        if let Some(JsonVal::Obj(dnr_obj)) = json_get(&pairs, "declarative_net_request") {
            if let Some(JsonVal::Arr(resources)) = json_get(dnr_obj, "rule_resources") {
                for res_item in resources {
                    if let JsonVal::Obj(r_obj) = res_item {
                        if let Some(rule_path) = json_get_str(r_obj, "path") {
                            if let Some(rules_raw) = read_safe_subfile(&root, &rule_path) {
                                parse_dnr_block_rules(&rules_raw, &mut blocked_url_patterns);
                            }
                        }
                    }
                }
            }
        }

        let mut storage_local = HashMap::new();
        if let Some(data_path) = self.ext_storage_file(&id) {
            if let Ok(existing_raw) = fs::read_to_string(&data_path) {
                if let Some(JsonVal::Obj(kv_pairs)) = parse_json(&existing_raw) {
                    for (k, v) in kv_pairs {
                        if let JsonVal::Str(s) = v {
                            storage_local.insert(k, s);
                        }
                    }
                }
            }
        }

        self.extensions.insert(
            id.clone(),
            FirefoxExtension {
                id: id.clone(),
                name,
                version,
                manifest_version,
                enabled: true,
                root_dir: Some(root),
                permissions,
                content_scripts,
                blocked_url_patterns,
                storage_local,
                raw_manifest_json: raw,
            },
        );
        self.sync_net_block_rules();
        Ok(id)
    }

    pub fn set_enabled(&mut self, ext_id: &str, enabled: bool) -> bool {
        let Some(ext) = self.extensions.get_mut(ext_id) else {
            return false;
        };
        ext.enabled = enabled;
        self.sync_net_block_rules();
        true
    }

    pub fn uninstall(&mut self, ext_id: &str) -> bool {
        if self.extensions.remove(ext_id).is_some() {
            if let Some(data_path) = self.ext_storage_file(ext_id) {
                let _ = fs::remove_file(data_path);
            }
            self.sync_net_block_rules();
            return true;
        }
        false
    }

    pub fn clear_all(&mut self) {
        self.extensions.clear();
        self.sync_net_block_rules();
        if let Some(ref root) = self.storage_root {
            let _ = fs::remove_dir_all(root.join("extensions_data"));
        }
    }

    pub fn collect_for_url(&self, url: &str) -> (Vec<String>, Vec<(String, String)>, Vec<(String, String)>) {
        if self.extensions.is_empty() {
            return (Vec::new(), Vec::new(), Vec::new());
        }
        let mut css_list = Vec::new();
        let mut start_js = Vec::new();
        let mut end_js = Vec::new();

        for ext in self.extensions.values() {
            if !ext.enabled {
                continue;
            }
            for rule in &ext.content_scripts {
                if !rule_matches_url(rule, url) {
                    continue;
                }
                css_list.extend(rule.css_sources.iter().cloned());
                for js in &rule.js_sources {
                    let wrapped = wrap_content_script_with_browser_api(ext, js);
                    if rule.run_at == ExtensionRunAt::DocumentStart {
                        start_js.push((ext.id.clone(), wrapped));
                    } else {
                        end_js.push((ext.id.clone(), wrapped));
                    }
                }
            }
        }
        (css_list, start_js, end_js)
    }

    pub fn compile_host_scripts_for_url(&self, url: &str) -> String {
        if self.extensions.is_empty() {
            return String::new();
        }
        let (css_list, start_js, end_js) = self.collect_for_url(url);
        if css_list.is_empty() && start_js.is_empty() && end_js.is_empty() {
            return String::new();
        }
        let mut out = String::with_capacity(2048);
        for css in css_list {
            out.push_str(&format!(
                "(function(){{try{{const s=document.createElement('style');s.textContent=\"{}\";(document.head||document.documentElement).appendChild(s);}}catch(_){{}}}})();\n",
                escape_json_str(&css)
            ));
        }
        for (_, js) in start_js.into_iter().chain(end_js.into_iter()) {
            out.push_str(&js);
            out.push('\n');
        }
        out
    }

    pub fn ext_storage_set(&mut self, ext_id: &str, key: &str, value: &str) -> bool {
        let path_opt = self.ext_storage_file(ext_id);
        let Some(ext) = self.extensions.get_mut(ext_id) else {
            return false;
        };
        ext.storage_local.insert(key.to_string(), value.to_string());
        if let Some(path) = path_opt {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let json = map_to_pretty_json(&ext.storage_local);
            let _ = fs::write(path, json.as_bytes());
        }
        true
    }

    pub fn ext_storage_get(&self, ext_id: &str, key: &str) -> Option<String> {
        self.extensions.get(ext_id)?.storage_local.get(key).cloned()
    }

    pub fn list_extensions_json(&self) -> String {
        let mut list: Vec<&FirefoxExtension> = self.extensions.values().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        let mut out = String::from("[");
        for (i, ext) in list.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"id\":\"{}\",\"name\":\"{}\",\"version\":\"{}\",\"manifest_version\":{},\"enabled\":{},\"content_scripts\":{},\"block_rules\":{}}}",
                escape_json_str(&ext.id),
                escape_json_str(&ext.name),
                escape_json_str(&ext.version),
                ext.manifest_version,
                ext.enabled,
                ext.content_scripts.len(),
                ext.blocked_url_patterns.len()
            ));
        }
        out.push(']');
        out
    }

    fn ext_storage_file(&self, ext_id: &str) -> Option<PathBuf> {
        let root = self.storage_root.as_ref()?;
        Some(
            root.join("extensions_data")
                .join(format!("{}.json", sanitize_ext_id(ext_id))),
        )
    }
}

fn wrap_content_script_with_browser_api(ext: &FirefoxExtension, script_src: &str) -> String {
    let storage_json = map_to_json_object(&ext.storage_local);
    format!(
        "(function(){{const __manta_ext_id=\"{id}\";const __manta_ext_store={store};const browser={{runtime:{{id:__manta_ext_id,getURL:function(p){{return \"moz-extension://\"+__manta_ext_id+\"/\"+p;}},getManifest:function(){{return {{name:\"{name}\",version:\"{ver}\",manifest_version:{mv}}};}}}},storage:{{local:{{get:function(k){{return Promise.resolve(__manta_ext_store[k]);}},set:function(obj){{Object.assign(__manta_ext_store,obj);return Promise.resolve();}},remove:function(k){{delete __manta_ext_store[k];return Promise.resolve();}}}}}},i18n:{{getMessage:function(k){{return k;}}}}}};const chrome=browser;{src}}})();",
        id = escape_json_str(&ext.id),
        store = storage_json,
        name = escape_json_str(&ext.name),
        ver = escape_json_str(&ext.version),
        mv = ext.manifest_version,
        src = script_src,
    )
}

fn rule_matches_url(rule: &ContentScriptRule, url: &str) -> bool {
    let matched = rule.matches.iter().any(|p| matches_firefox_pattern(p, url));
    if !matched {
        return false;
    }
    !rule
        .exclude_matches
        .iter()
        .any(|p| matches_firefox_pattern(p, url))
}

pub fn matches_firefox_pattern(pattern: &str, url: &str) -> bool {
    let pat = pattern.trim();
    if pat == "<all_urls>" || pat == "*" {
        return url.starts_with("http://") || url.starts_with("https://");
    }
    let Some((pat_scheme, pat_rest)) = pat.split_once("://") else {
        return false;
    };
    let Some((url_scheme, url_rest)) = url.split_once("://") else {
        return false;
    };

    if pat_scheme == "*" {
        if url_scheme != "http" && url_scheme != "https" {
            return false;
        }
    } else if !pat_scheme.eq_ignore_ascii_case(url_scheme) {
        return false;
    }

    let (pat_host, pat_path) = match pat_rest.find('/') {
        Some(i) => (&pat_rest[..i], &pat_rest[i..]),
        None => (pat_rest, "/*"),
    };
    let (url_host_port, url_path) = match url_rest.find('/') {
        Some(i) => (&url_rest[..i], &url_rest[i..]),
        None => (url_rest, "/"),
    };
    let url_host = url_host_port.split(':').next().unwrap_or(url_host_port);

    if pat_host != "*" {
        if let Some(base_domain) = pat_host.strip_prefix("*.") {
            let uh = url_host.to_ascii_lowercase();
            let bd = base_domain.to_ascii_lowercase();
            if uh != bd && !uh.ends_with(&format!(".{}", bd)) {
                return false;
            }
        } else if !pat_host.eq_ignore_ascii_case(url_host) {
            return false;
        }
    }

    wildcard_match(pat_path, url_path)
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    if pattern == "/*" || pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut pos = 0usize;
    for (idx, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if idx == 0 {
            if !text.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if idx == parts.len() - 1 {
            return text[pos..].ends_with(part);
        } else if let Some(found) = text[pos..].find(part) {
            pos += found + part.len();
        } else {
            return false;
        }
    }
    true
}

fn matches_block_filter(filter: &str, lower_url: &str) -> bool {
    let f = filter.trim().to_ascii_lowercase();
    if f.is_empty() {
        return false;
    }
    if let Some(domain) = f.strip_prefix("||") {
        let clean = domain.trim_end_matches('^');
        return lower_url.contains(clean);
    }
    lower_url.contains(&f)
}

fn parse_dnr_block_rules(rules_json: &str, out: &mut Vec<String>) {
    let Some(JsonVal::Arr(arr)) = parse_json(rules_json) else {
        return;
    };
    for item in arr {
        let JsonVal::Obj(rule) = item else {
            continue;
        };
        let is_block = json_get(&rule, "action")
            .and_then(|v| match v {
                JsonVal::Obj(a) => json_get_str(a, "type"),
                _ => None,
            })
            .map(|t| t.eq_ignore_ascii_case("block"))
            .unwrap_or(false);
        if !is_block {
            continue;
        }
        if let Some(JsonVal::Obj(cond)) = json_get(&rule, "condition") {
            if let Some(uf) = json_get_str(cond, "urlFilter") {
                if !uf.is_empty() {
                    out.push(uf);
                }
            }
        }
    }
}

fn read_safe_subfile(root: &Path, rel: &str) -> Option<String> {
    let clean = rel.trim().replace('\\', "/");
    if clean.contains("..") || clean.starts_with('/') {
        return None;
    }
    fs::read_to_string(root.join(clean)).ok()
}

fn sanitize_ext_id(raw: &str) -> String {
    let clean: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '@' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "manta_ext".to_string()
    } else {
        clean
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_bus::ModuleBus;

    #[test]
    fn test_firefox_webextension_mv3_install_match_content_and_blocker() {
        let ext_dir = "./test_manta_ff_ext";
        let _ = fs::remove_dir_all(ext_dir);
        fs::create_dir_all(ext_dir).unwrap();

        let manifest = r#"{
            "manifest_version": 3,
            "name": "Manta Dark Shield",
            "version": "1.2.0",
            "browser_specific_settings": { "gecko": { "id": "dark-shield@manta.dev" } },
            "permissions": ["storage", "declarativeNetRequest"],
            "content_scripts": [{
                "matches": ["*://*.example.com/*"],
                "js": ["content.js"],
                "css": ["theme.css"],
                "run_at": "document_end"
            }],
            "declarative_net_request": {
                "rule_resources": [{ "id": "ruleset_1", "enabled": true, "path": "rules.json" }]
            }
        }"#;
        fs::write(format!("{}/manifest.json", ext_dir), manifest).unwrap();
        fs::write(
            format!("{}/content.js", ext_dir),
            "document.getElementById('banner').textContent = 'Patched by ' + browser.runtime.id;",
        )
        .unwrap();
        fs::write(
            format!("{}/theme.css", ext_dir),
            "#banner { color: #00FF00; }",
        )
        .unwrap();
        fs::write(
            format!("{}/rules.json", ext_dir),
            r#"[{"id":1,"action":{"type":"block"},"condition":{"urlFilter":"||tracker.ads.net^"}}]"#,
        )
        .unwrap();

        let mut host = ExtensionHost::new();
        let mut bus = ModuleBus::new();
        bus.register(host.create_net_module());

        let ext_id = host.install_from_dir(ext_dir).unwrap();
        assert_eq!(ext_id, "dark-shield@manta.dev");

        assert!(matches_firefox_pattern("*://*.example.com/*", "https://example.com/home"));
        assert!(matches_firefox_pattern("*://*.example.com/*", "https://docs.example.com/a"));
        assert!(!matches_firefox_pattern("*://*.example.com/*", "https://other.org/a"));

        let (css, _, end_js) = host.collect_for_url("https://sub.example.com/page");
        assert_eq!(css.len(), 1);
        assert_eq!(end_js.len(), 1);
        assert!(end_js[0].1.contains("dark-shield@manta.dev"));

        let mut req = InterceptedRequest {
            method: "GET".to_string(),
            url: "https://tracker.ads.net/pixel.js".to_string(),
            headers: HashMap::new(),
        };
        assert!(matches!(
            bus.dispatch_before_request(&mut req),
            ModuleDecision::Block { .. }
        ));

        assert!(host.set_enabled(&ext_id, false));
        assert!(matches!(
            bus.dispatch_before_request(&mut req),
            ModuleDecision::Proceed
        ));

        assert!(host.uninstall(&ext_id));
        let _ = fs::remove_dir_all(ext_dir);
    }
}
