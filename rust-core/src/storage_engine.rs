use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::json_lite::{
    escape_json_str, json_get_bool, json_get_i64, json_get_str, map_to_json_object,
    map_to_pretty_json, parse_json, JsonVal,
};
use crate::site_isolation::OriginKey;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: String,
}

#[derive(Debug, Clone, Default)]
pub struct OriginStorageBucket {
    pub local_storage: HashMap<String, String>,
    pub session_storage: HashMap<String, String>,
    pub cookies: HashMap<String, StoredCookie>,
    pub form_drafts: HashMap<String, String>,
    pub object_stores: HashMap<String, HashMap<String, String>>,
    pub saved_files_meta: HashMap<String, (String, usize, Option<Vec<u8>>)>,
    pub used_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkEntry {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryVisitEntry {
    pub url: String,
    pub title: String,
    pub visit_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTabSession {
    pub tab_id: u32,
    pub url: String,
    pub title: String,
    pub scroll_y: i32,
}

/// **Transparent User-Accessible Device File Storage (`storage_engine.rs`)**:
/// Uses shared `json_lite` to persist readable, 100%-deletable files directly inside
/// the user's device folder (`<storage_dir>/`).
pub struct MantaStorageEngine {
    origins: HashMap<String, OriginStorageBucket>,
    bookmarks: Vec<BookmarkEntry>,
    history: Vec<HistoryVisitEntry>,
    saved_tabs: HashMap<u32, SavedTabSession>,
    offline_mem_fallback: HashMap<String, Vec<u8>>,
    per_origin_quota_bytes: usize,
    total_ceiling_bytes: usize,
    total_used_bytes: usize,
    storage_dir_path: Option<PathBuf>,
    suppress_disk_write: bool,
    pub auto_commit_on_write: bool,
    pub ephemeral_only: bool,
}

impl MantaStorageEngine {
    pub fn new(per_origin_quota_bytes: usize, total_ceiling_bytes: usize) -> Self {
        Self {
            origins: HashMap::new(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            saved_tabs: HashMap::new(),
            offline_mem_fallback: HashMap::new(),
            per_origin_quota_bytes,
            total_ceiling_bytes,
            total_used_bytes: 0,
            storage_dir_path: None,
            suppress_disk_write: false,
            auto_commit_on_write: true,
            ephemeral_only: false,
        }
    }

    pub fn set_storage_dir(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }
        let dir_path = if trimmed.ends_with(".db") {
            PathBuf::from(trimmed.trim_end_matches(".db"))
        } else {
            PathBuf::from(trimmed)
        };
        if !self.ephemeral_only {
            let _ = ensure_dir_readable(&dir_path);
        }
        self.storage_dir_path = Some(dir_path);
    }

    pub fn set_backing_file(&mut self, path: &str) {
        self.set_storage_dir(path);
    }

    pub fn storage_dir(&self) -> Option<String> {
        self.storage_dir_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
    }

    pub fn locate_origin_dir(&self, origin_url: &str) -> Option<String> {
        let root = self.storage_dir_path.as_ref()?;
        let origin = OriginKey::from_url(origin_url);
        if origin.is_opaque {
            return None;
        }
        let slug = origin_to_folder_slug(&origin.serialize());
        Some(root.join("sites").join(slug).to_string_lossy().into_owned())
    }

    pub fn set_ephemeral_mode(&mut self, enabled: bool) {
        self.ephemeral_only = enabled;
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.total_used_bytes
    }

    #[inline]
    pub fn origin_count(&self) -> usize {
        self.origins.len()
    }

    fn site_dir(&self, serialized_origin: &str) -> Option<PathBuf> {
        if self.ephemeral_only || self.suppress_disk_write {
            return None;
        }
        let root = self.storage_dir_path.as_ref()?;
        Some(root.join("sites").join(origin_to_folder_slug(serialized_origin)))
    }

    fn site_dir_readonly(&self, serialized_origin: &str) -> Option<PathBuf> {
        let root = self.storage_dir_path.as_ref()?;
        Some(root.join("sites").join(origin_to_folder_slug(serialized_origin)))
    }

    fn persist_origin_meta(&self, serialized_origin: &str) {
        if let Some(dir) = self.site_dir(serialized_origin) {
            let meta_path = dir.join("meta.json");
            if !meta_path.exists() {
                let json = format!("{{\"origin\":\"{}\"}}\n", escape_json_str(serialized_origin));
                let _ = atomic_write_file(&meta_path, json.as_bytes());
            }
        }
    }

    fn persist_origin_local_storage(&self, serialized_origin: &str) {
        let Some(dir) = self.site_dir(serialized_origin) else {
            return;
        };
        let path = dir.join("local_storage.json");
        if let Some(bucket) = self.origins.get(serialized_origin) {
            if bucket.local_storage.is_empty() {
                let _ = fs::remove_file(&path);
            } else {
                self.persist_origin_meta(serialized_origin);
                let json = map_to_pretty_json(&bucket.local_storage);
                let _ = atomic_write_file(&path, json.as_bytes());
            }
        }
        self.write_index_manifest();
    }

    fn persist_origin_form_drafts(&self, serialized_origin: &str) {
        let Some(dir) = self.site_dir(serialized_origin) else {
            return;
        };
        let path = dir.join("form_drafts.json");
        if let Some(bucket) = self.origins.get(serialized_origin) {
            if bucket.form_drafts.is_empty() {
                let _ = fs::remove_file(&path);
            } else {
                self.persist_origin_meta(serialized_origin);
                let json = map_to_pretty_json(&bucket.form_drafts);
                let _ = atomic_write_file(&path, json.as_bytes());
            }
        }
        self.write_index_manifest();
    }

    fn persist_origin_cookies(&self, serialized_origin: &str) {
        let Some(dir) = self.site_dir(serialized_origin) else {
            return;
        };
        let path = dir.join("cookies.json");
        if let Some(bucket) = self.origins.get(serialized_origin) {
            if bucket.cookies.is_empty() {
                let _ = fs::remove_file(&path);
            } else {
                self.persist_origin_meta(serialized_origin);
                let mut cookies: Vec<&StoredCookie> = bucket.cookies.values().collect();
                cookies.sort_by(|a, b| a.name.cmp(&b.name));
                let mut out = String::from("[\n");
                for (idx, c) in cookies.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(",\n");
                    }
                    out.push_str(&format!(
                        "  {{\"name\":\"{}\",\"value\":\"{}\",\"domain\":\"{}\",\"path\":\"{}\",\"secure\":{},\"http_only\":{},\"same_site\":\"{}\"}}",
                        escape_json_str(&c.name),
                        escape_json_str(&c.value),
                        escape_json_str(&c.domain),
                        escape_json_str(&c.path),
                        c.secure,
                        c.http_only,
                        escape_json_str(&c.same_site)
                    ));
                }
                out.push_str("\n]\n");
                let _ = atomic_write_file(&path, out.as_bytes());
            }
        }
        self.write_index_manifest();
    }

    fn persist_origin_idb_store(&self, serialized_origin: &str, store_name: &str) {
        let Some(dir) = self.site_dir(serialized_origin) else {
            return;
        };
        let idb_dir = dir.join("indexeddb");
        let file_path = idb_dir.join(format!("{}.json", safe_file_slug(store_name)));
        if let Some(bucket) = self.origins.get(serialized_origin) {
            if let Some(store) = bucket.object_stores.get(store_name) {
                if store.is_empty() {
                    let _ = fs::remove_file(&file_path);
                } else {
                    self.persist_origin_meta(serialized_origin);
                    let json = map_to_pretty_json(store);
                    let _ = atomic_write_file(&file_path, json.as_bytes());
                }
            } else {
                let _ = fs::remove_file(&file_path);
            }
        }
        self.write_index_manifest();
    }

    fn persist_bookmarks(&self) {
        if self.ephemeral_only || self.suppress_disk_write {
            return;
        }
        let Some(ref root) = self.storage_dir_path else {
            return;
        };
        let path = root.join("bookmarks.json");
        if self.bookmarks.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            let mut out = String::from("[\n");
            for (i, b) in self.bookmarks.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&format!(
                    "  {{\"url\":\"{}\",\"title\":\"{}\"}}",
                    escape_json_str(&b.url),
                    escape_json_str(&b.title)
                ));
            }
            out.push_str("\n]\n");
            let _ = atomic_write_file(&path, out.as_bytes());
        }
        self.write_index_manifest();
    }

    fn persist_history(&self) {
        if self.ephemeral_only || self.suppress_disk_write {
            return;
        }
        let Some(ref root) = self.storage_dir_path else {
            return;
        };
        let path = root.join("history.json");
        if self.history.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            let mut out = String::from("[\n");
            for (i, h) in self.history.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&format!(
                    "  {{\"url\":\"{}\",\"title\":\"{}\",\"visit_count\":{}}}",
                    escape_json_str(&h.url),
                    escape_json_str(&h.title),
                    h.visit_count
                ));
            }
            out.push_str("\n]\n");
            let _ = atomic_write_file(&path, out.as_bytes());
        }
        self.write_index_manifest();
    }

    fn persist_tabs_session(&self) {
        if self.ephemeral_only || self.suppress_disk_write {
            return;
        }
        let Some(ref root) = self.storage_dir_path else {
            return;
        };
        let path = root.join("tabs_session.json");
        if self.saved_tabs.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            let json = format!("{}\n", self.export_saved_tabs_json());
            let _ = atomic_write_file(&path, json.as_bytes());
        }
    }

    fn write_index_manifest(&self) {
        if self.ephemeral_only || self.suppress_disk_write {
            return;
        }
        let Some(ref root) = self.storage_dir_path else {
            return;
        };
        let index_path = root.join("manta_index.json");
        let manifest = self.inspect_storage_manifest_json();
        let _ = atomic_write_file(&index_path, manifest.as_bytes());
    }

    pub fn list_all_saved_files_json(&self) -> String {
        let Some(ref root) = self.storage_dir_path else {
            return "[]".to_string();
        };
        let mut files = Vec::new();
        collect_files_recursive(root, root, &mut files);
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut out = String::from("[");
        for (i, (rel, abs, bytes)) in files.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let category = if rel.starts_with("sites/") {
                "site_data"
            } else if rel.starts_with("offline_pages/") {
                "offline_page"
            } else if rel.starts_with("extensions_data/") {
                "extension_data"
            } else if rel == "history.json" {
                "history"
            } else if rel == "bookmarks.json" {
                "bookmarks"
            } else if rel == "tabs_session.json" {
                "tabs_session"
            } else {
                "manifest"
            };
            out.push_str(&format!(
                "{{\"relative_path\":\"{}\",\"full_path\":\"{}\",\"bytes\":{},\"category\":\"{}\"}}",
                escape_json_str(rel),
                escape_json_str(abs),
                bytes,
                category
            ));
        }
        out.push(']');
        out
    }

    pub fn inspect_storage_manifest_json(&self) -> String {
        let mut out = String::from("{");
        out.push_str(&format!(
            "\"storage_mode\":\"transparent_device_files\",\"storage_dir\":\"{}\",\"total_bytes\":{},\"ephemeral_only\":{},\"saved_tabs_count\":{},\"bookmarks_count\":{},\"history_count\":{},\"origins\":[",
            escape_json_str(self.storage_dir().as_deref().unwrap_or("")),
            self.used_bytes(),
            self.ephemeral_only,
            self.saved_tabs.len(),
            self.bookmarks.len(),
            self.history.len()
        ));

        let mut all_origin_keys: Vec<String> = self.origins.keys().cloned().collect();
        all_origin_keys.sort();

        for (idx, origin_str) in all_origin_keys.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let bucket = &self.origins[origin_str];
            let dir_str = self
                .site_dir_readonly(origin_str)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();

            out.push_str(&format!(
                "{{\"origin\":\"{}\",\"folder\":\"{}\",\"used_bytes\":{},\"local_storage_keys\":{},\"session_storage_keys\":{},\"form_drafts\":{},\"cookies\":{},\"object_stores\":[",
                escape_json_str(origin_str),
                escape_json_str(&dir_str),
                bucket.used_bytes,
                bucket.local_storage.len(),
                bucket.session_storage.len(),
                bucket.form_drafts.len(),
                bucket.cookies.len()
            ));

            for (s_idx, (sname, recs)) in bucket.object_stores.iter().enumerate() {
                if s_idx > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"name\":\"{}\",\"records\":{}}}",
                    escape_json_str(sname),
                    recs.len()
                ));
            }
            out.push_str("],\"saved_files\":[");
            for (f_idx, (vpath, (mime, bytes, _))) in bucket.saved_files_meta.iter().enumerate() {
                if f_idx > 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    "{{\"path\":\"{}\",\"mime\":\"{}\",\"bytes\":{}}}",
                    escape_json_str(vpath),
                    escape_json_str(mime),
                    bytes
                ));
            }
            out.push_str("]}");
        }

        out.push_str("]}");
        out
    }

    pub fn save_tab_session_state(&mut self, tab_id: u32, url: &str, title: &str, scroll_y: i32) {
        if url.trim().is_empty() {
            return;
        }
        self.saved_tabs.insert(
            tab_id,
            SavedTabSession {
                tab_id,
                url: url.to_string(),
                title: title.to_string(),
                scroll_y,
            },
        );
        if self.auto_commit_on_write {
            self.persist_tabs_session();
        }
    }

    pub fn remove_tab_session_state(&mut self, tab_id: u32) {
        if self.saved_tabs.remove(&tab_id).is_some() && self.auto_commit_on_write {
            self.persist_tabs_session();
        }
    }

    pub fn list_saved_tab_sessions(&self) -> Vec<SavedTabSession> {
        let mut list: Vec<SavedTabSession> = self.saved_tabs.values().cloned().collect();
        list.sort_by_key(|s| s.tab_id);
        list
    }

    pub fn export_saved_tabs_json(&self) -> String {
        let list = self.list_saved_tab_sessions();
        let mut out = String::from("[");
        for (i, s) in list.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"tab_id\":{},\"url\":\"{}\",\"title\":\"{}\",\"scroll_y\":{}}}",
                s.tab_id,
                escape_json_str(&s.url),
                escape_json_str(&s.title),
                s.scroll_y
            ));
        }
        out.push(']');
        out
    }

    pub fn save_offline_page_snapshot(&mut self, url: &str, html_bytes: &[u8]) -> bool {
        let slug = safe_file_slug(url);
        if !self.ephemeral_only {
            if let Some(ref root) = self.storage_dir_path {
                let file_path = root.join("offline_pages").join(format!("{}.html", slug));
                if atomic_write_file(&file_path, html_bytes).is_ok() {
                    self.write_index_manifest();
                    return true;
                }
            }
        }
        self.offline_mem_fallback.insert(slug, html_bytes.to_vec());
        true
    }

    pub fn load_offline_page_snapshot(&self, url: &str) -> Option<Vec<u8>> {
        let slug = safe_file_slug(url);
        if let Some(ref root) = self.storage_dir_path {
            let file_path = root.join("offline_pages").join(format!("{}.html", slug));
            if let Ok(bytes) = fs::read(&file_path) {
                return Some(bytes);
            }
        }
        self.offline_mem_fallback.get(&slug).cloned()
    }

    pub fn set_local_storage(&mut self, origin_url: &str, key: &str, value: &str) -> bool {
        if let Some(draft_key) = key.strip_prefix("__manta_draft::") {
            return self.save_form_draft(origin_url, draft_key, value);
        }
        if key == "__manta_cookie_jar" {
            for pair in value.split(';') {
                let trimmed = pair.trim();
                if !trimmed.is_empty() {
                    self.set_cookie_from_header(origin_url, trimmed);
                }
            }
            return true;
        }
        if let Some(idb_rest) = key.strip_prefix("__manta_idb::") {
            if let Some((store, pk)) = idb_rest.split_once("::") {
                return self.put_idb_object(origin_url, store, pk, value);
            }
        }
        let ok = self.set_kv_internal(origin_url, key, value, false);
        if ok && self.auto_commit_on_write {
            let orig = OriginKey::from_url(origin_url).serialize();
            self.persist_origin_local_storage(&orig);
        }
        ok
    }

    pub fn get_local_storage(&self, origin_url: &str, key: &str) -> Option<String> {
        self.get_kv_internal(origin_url, key, false)
    }

    pub fn remove_local_storage(&mut self, origin_url: &str, key: &str) -> bool {
        if let Some(idb_rest) = key.strip_prefix("__manta_idb::") {
            if let Some((store, pk)) = idb_rest.split_once("::") {
                return self.delete_idb_object(origin_url, store, pk);
            }
        }
        let ok = self.remove_kv_internal(origin_url, key, false);
        if ok && self.auto_commit_on_write {
            let orig = OriginKey::from_url(origin_url).serialize();
            self.persist_origin_local_storage(&orig);
        }
        ok
    }

    pub fn clear_local_storage_only(&mut self, origin_url: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return false;
        }
        let serialized = origin_key.serialize();
        let Some(bucket) = self.origins.get_mut(&serialized) else {
            return false;
        };
        let freed: usize = bucket
            .local_storage
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();
        bucket.local_storage.clear();
        bucket.used_bytes = bucket.used_bytes.saturating_sub(freed);
        self.total_used_bytes = self.total_used_bytes.saturating_sub(freed);
        if self.auto_commit_on_write {
            self.persist_origin_local_storage(&serialized);
        }
        true
    }

    pub fn set_session_storage(&mut self, origin_url: &str, key: &str, value: &str) -> bool {
        self.set_kv_internal(origin_url, key, value, true)
    }

    pub fn get_session_storage(&self, origin_url: &str, key: &str) -> Option<String> {
        self.get_kv_internal(origin_url, key, true)
    }

    pub fn remove_session_storage(&mut self, origin_url: &str, key: &str) -> bool {
        self.remove_kv_internal(origin_url, key, true)
    }

    pub fn clear_session_storage_only(&mut self, origin_url: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return false;
        }
        let Some(bucket) = self.origins.get_mut(&origin_key.serialize()) else {
            return false;
        };
        let freed: usize = bucket
            .session_storage
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();
        bucket.session_storage.clear();
        bucket.used_bytes = bucket.used_bytes.saturating_sub(freed);
        self.total_used_bytes = self.total_used_bytes.saturating_sub(freed);
        true
    }

    pub fn save_form_draft(&mut self, origin_url: &str, field_key: &str, value: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque || field_key.trim().is_empty() {
            return false;
        }
        let serialized_origin = origin_key.serialize();
        let entry_cost = field_key.len() + value.len();

        let bucket = self.origins.entry(serialized_origin.clone()).or_default();
        let prev_cost = bucket
            .form_drafts
            .get(field_key)
            .map(|old| field_key.len() + old.len())
            .unwrap_or(0);

        let next_bucket = bucket.used_bytes.saturating_sub(prev_cost) + entry_cost;
        let next_total = self.total_used_bytes.saturating_sub(prev_cost) + entry_cost;
        if next_bucket > self.per_origin_quota_bytes || next_total > self.total_ceiling_bytes {
            return false;
        }

        bucket
            .form_drafts
            .insert(field_key.to_string(), value.to_string());
        bucket.used_bytes = next_bucket;
        self.total_used_bytes = next_total;

        if self.auto_commit_on_write {
            self.persist_origin_form_drafts(&serialized_origin);
        }
        true
    }

    pub fn get_form_draft(&self, origin_url: &str, field_key: &str) -> Option<String> {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return None;
        }
        self.origins
            .get(&origin_key.serialize())?
            .form_drafts
            .get(field_key)
            .cloned()
    }

    pub fn put_idb_object(
        &mut self,
        origin_url: &str,
        store_name: &str,
        key: &str,
        json_val: &str,
    ) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        let clean_store = store_name.trim();
        if origin_key.is_opaque || clean_store.is_empty() || key.is_empty() {
            return false;
        }
        let serialized = origin_key.serialize();
        let cost = key.len() + json_val.len();

        let bucket = self.origins.entry(serialized.clone()).or_default();
        let store = bucket
            .object_stores
            .entry(clean_store.to_string())
            .or_default();
        let old_cost = store
            .get(key)
            .map(|old| key.len() + old.len())
            .unwrap_or(0);

        let next_bucket = bucket.used_bytes.saturating_sub(old_cost) + cost;
        let next_total = self.total_used_bytes.saturating_sub(old_cost) + cost;
        if next_bucket > self.per_origin_quota_bytes || next_total > self.total_ceiling_bytes {
            return false;
        }

        store.insert(key.to_string(), json_val.to_string());
        bucket.used_bytes = next_bucket;
        self.total_used_bytes = next_total;

        if self.auto_commit_on_write {
            self.persist_origin_idb_store(&serialized, clean_store);
        }
        true
    }

    pub fn get_idb_object(
        &self,
        origin_url: &str,
        store_name: &str,
        key: &str,
    ) -> Option<String> {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return None;
        }
        self.origins
            .get(&origin_key.serialize())?
            .object_stores
            .get(store_name.trim())?
            .get(key)
            .cloned()
    }

    pub fn delete_idb_object(&mut self, origin_url: &str, store_name: &str, key: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        let clean_store = store_name.trim();
        if origin_key.is_opaque || clean_store.is_empty() {
            return false;
        }
        let serialized = origin_key.serialize();
        let Some(bucket) = self.origins.get_mut(&serialized) else {
            return false;
        };
        let Some(store) = bucket.object_stores.get_mut(clean_store) else {
            return false;
        };
        if let Some(removed) = store.remove(key) {
            let freed = key.len() + removed.len();
            bucket.used_bytes = bucket.used_bytes.saturating_sub(freed);
            self.total_used_bytes = self.total_used_bytes.saturating_sub(freed);
            if self.auto_commit_on_write {
                self.persist_origin_idb_store(&serialized, clean_store);
            }
            return true;
        }
        false
    }

    pub fn save_vfs_file(
        &mut self,
        origin_url: &str,
        virtual_path: &str,
        mime_type: &str,
        data: &[u8],
    ) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        let clean_vpath = sanitize_virtual_path(virtual_path);
        if origin_key.is_opaque || clean_vpath.is_empty() {
            return false;
        }
        let serialized = origin_key.serialize();
        let cost = data.len();

        let disk_written = if let Some(dir) = self.site_dir(&serialized) {
            self.persist_origin_meta(&serialized);
            let rel = clean_vpath.trim_start_matches('/');
            let full = dir.join("files").join(rel);
            atomic_write_file(&full, data).is_ok()
        } else {
            false
        };

        let bucket = self.origins.entry(serialized.clone()).or_default();
        let prev_len = bucket
            .saved_files_meta
            .get(&clean_vpath)
            .map(|(_, len, _)| *len)
            .unwrap_or(0);
        let next_bucket = bucket.used_bytes.saturating_sub(prev_len) + cost;
        let next_total = self.total_used_bytes.saturating_sub(prev_len) + cost;
        if next_bucket > self.per_origin_quota_bytes || next_total > self.total_ceiling_bytes {
            return false;
        }

        let mem_copy = if disk_written {
            None
        } else {
            Some(data.to_vec())
        };
        bucket.saved_files_meta.insert(
            clean_vpath,
            (
                if mime_type.is_empty() {
                    "application/octet-stream".to_string()
                } else {
                    mime_type.to_string()
                },
                data.len(),
                mem_copy,
            ),
        );
        bucket.used_bytes = next_bucket;
        self.total_used_bytes = next_total;

        if disk_written {
            self.write_index_manifest();
        }
        true
    }

    pub fn read_vfs_file(&self, origin_url: &str, virtual_path: &str) -> Option<Vec<u8>> {
        let origin_key = OriginKey::from_url(origin_url);
        let clean_vpath = sanitize_virtual_path(virtual_path);
        if origin_key.is_opaque || clean_vpath.is_empty() {
            return None;
        }
        let serialized = origin_key.serialize();
        if let Some(dir) = self.site_dir_readonly(&serialized) {
            let full = dir.join("files").join(clean_vpath.trim_start_matches('/'));
            if let Ok(bytes) = fs::read(&full) {
                return Some(bytes);
            }
        }
        self.origins
            .get(&serialized)?
            .saved_files_meta
            .get(&clean_vpath)?
            .2
            .clone()
    }

    pub fn delete_vfs_file(&mut self, origin_url: &str, virtual_path: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        let clean_vpath = sanitize_virtual_path(virtual_path);
        if origin_key.is_opaque || clean_vpath.is_empty() {
            return false;
        }
        let serialized = origin_key.serialize();
        if let Some(dir) = self.site_dir(&serialized) {
            let full = dir.join("files").join(clean_vpath.trim_start_matches('/'));
            let _ = fs::remove_file(full);
        }
        if let Some(bucket) = self.origins.get_mut(&serialized) {
            if let Some((_, len, _)) = bucket.saved_files_meta.remove(&clean_vpath) {
                bucket.used_bytes = bucket.used_bytes.saturating_sub(len);
                self.total_used_bytes = self.total_used_bytes.saturating_sub(len);
                self.write_index_manifest();
                return true;
            }
        }
        false
    }

    pub fn export_origin_storage_json(&self, origin_url: &str, is_session: bool) -> String {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return "{}".to_string();
        }
        let Some(bucket) = self.origins.get(&origin_key.serialize()) else {
            return "{}".to_string();
        };
        let map = if is_session {
            &bucket.session_storage
        } else {
            &bucket.local_storage
        };
        map_to_json_object(map)
    }

    pub fn export_origin_form_drafts_json(&self, origin_url: &str) -> String {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return "{}".to_string();
        }
        let Some(bucket) = self.origins.get(&origin_key.serialize()) else {
            return "{}".to_string();
        };
        map_to_json_object(&bucket.form_drafts)
    }

    pub fn export_origin_idb_json(&self, origin_url: &str) -> String {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return "{}".to_string();
        }
        let Some(bucket) = self.origins.get(&origin_key.serialize()) else {
            return "{}".to_string();
        };
        let mut out = String::from("{");
        for (idx, (store_name, recs)) in bucket.object_stores.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(&escape_json_str(store_name));
            out.push_str("\":");
            out.push_str(&map_to_json_object(recs));
        }
        out.push('}');
        out
    }

    fn set_kv_internal(
        &mut self,
        origin_url: &str,
        key: &str,
        value: &str,
        is_session: bool,
    ) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque || key.is_empty() {
            return false;
        }
        let serialized_origin = origin_key.serialize();
        let entry_cost = key.len() + value.len();

        let bucket = self.origins.entry(serialized_origin).or_default();
        let map = if is_session {
            &mut bucket.session_storage
        } else {
            &mut bucket.local_storage
        };

        let previous_cost = map
            .get(key)
            .map(|old_val| key.len() + old_val.len())
            .unwrap_or(0);

        let next_bucket_bytes = bucket
            .used_bytes
            .saturating_sub(previous_cost)
            .saturating_add(entry_cost);
        let next_total_bytes = self
            .total_used_bytes
            .saturating_sub(previous_cost)
            .saturating_add(entry_cost);

        if next_bucket_bytes > self.per_origin_quota_bytes
            || next_total_bytes > self.total_ceiling_bytes
        {
            return false;
        }

        map.insert(key.to_string(), value.to_string());
        bucket.used_bytes = next_bucket_bytes;
        self.total_used_bytes = next_total_bytes;
        true
    }

    fn get_kv_internal(&self, origin_url: &str, key: &str, is_session: bool) -> Option<String> {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return None;
        }
        let bucket = self.origins.get(&origin_key.serialize())?;
        if is_session {
            bucket.session_storage.get(key).cloned()
        } else {
            bucket.local_storage.get(key).cloned()
        }
    }

    fn remove_kv_internal(&mut self, origin_url: &str, key: &str, is_session: bool) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return false;
        }
        let Some(bucket) = self.origins.get_mut(&origin_key.serialize()) else {
            return false;
        };
        let map = if is_session {
            &mut bucket.session_storage
        } else {
            &mut bucket.local_storage
        };
        if let Some(removed_val) = map.remove(key) {
            let cost = key.len() + removed_val.len();
            bucket.used_bytes = bucket.used_bytes.saturating_sub(cost);
            self.total_used_bytes = self.total_used_bytes.saturating_sub(cost);
            return true;
        }
        false
    }

    pub fn set_cookie_from_header(&mut self, origin_url: &str, raw_set_cookie: &str) -> bool {
        let origin = OriginKey::from_url(origin_url);
        if origin.is_opaque || raw_set_cookie.trim().is_empty() {
            return false;
        }

        let mut parts = raw_set_cookie.split(';');
        let Some(name_val) = parts.next() else {
            return false;
        };
        let Some(eq_idx) = name_val.find('=') else {
            return false;
        };
        let name = name_val[..eq_idx].trim().to_string();
        let value = name_val[eq_idx + 1..].trim().to_string();
        if name.is_empty() {
            return false;
        }

        if (name.starts_with("__Secure-") || name.starts_with("__Host-"))
            && !origin.is_secure_context()
        {
            return false;
        }

        let mut path = "/".to_string();
        let mut secure = false;
        let mut http_only = false;
        let mut same_site = "Lax".to_string();
        let mut max_age_zero = false;

        for attr in parts {
            let trimmed = attr.trim();
            let lower = trimmed.to_ascii_lowercase();
            if lower == "secure" {
                secure = true;
            } else if lower == "httponly" {
                http_only = true;
            } else if let Some(p) = trimmed
                .strip_prefix("Path=")
                .or_else(|| trimmed.strip_prefix("path="))
            {
                path = p.trim().to_string();
            } else if let Some(ss) = trimmed
                .strip_prefix("SameSite=")
                .or_else(|| trimmed.strip_prefix("samesite="))
            {
                same_site = ss.trim().to_string();
            } else if lower == "max-age=0" || lower.starts_with("max-age=-") {
                max_age_zero = true;
            }
        }

        if secure && !origin.is_secure_context() {
            return false;
        }

        let serialized_origin = origin.serialize();
        let bucket = self.origins.entry(serialized_origin.clone()).or_default();

        if max_age_zero {
            if let Some(old) = bucket.cookies.remove(&name) {
                let cost = old.name.len() + old.value.len();
                bucket.used_bytes = bucket.used_bytes.saturating_sub(cost);
                self.total_used_bytes = self.total_used_bytes.saturating_sub(cost);
            }
            if self.auto_commit_on_write {
                self.persist_origin_cookies(&serialized_origin);
            }
            return true;
        }

        let cost = name.len() + value.len();
        if bucket.used_bytes + cost > self.per_origin_quota_bytes
            || self.total_used_bytes + cost > self.total_ceiling_bytes
        {
            return false;
        }

        if let Some(old) = bucket.cookies.remove(&name) {
            let old_cost = old.name.len() + old.value.len();
            bucket.used_bytes = bucket.used_bytes.saturating_sub(old_cost);
            self.total_used_bytes = self.total_used_bytes.saturating_sub(old_cost);
        }

        bucket.cookies.insert(
            name.clone(),
            StoredCookie {
                name,
                value,
                domain: origin.host,
                path,
                secure,
                http_only,
                same_site,
            },
        );
        bucket.used_bytes += cost;
        self.total_used_bytes += cost;

        if self.auto_commit_on_write {
            self.persist_origin_cookies(&serialized_origin);
        }
        true
    }

    pub fn get_cookie_header_for_url(&self, origin_url: &str, include_http_only: bool) -> String {
        let origin = OriginKey::from_url(origin_url);
        if origin.is_opaque {
            return String::new();
        }
        let Some(bucket) = self.origins.get(&origin.serialize()) else {
            return String::new();
        };

        let mut pairs = Vec::with_capacity(bucket.cookies.len());
        for cookie in bucket.cookies.values() {
            if cookie.secure && !origin.is_secure_context() {
                continue;
            }
            if cookie.http_only && !include_http_only {
                continue;
            }
            pairs.push(format!("{}={}", cookie.name, cookie.value));
        }
        pairs.sort();
        pairs.join("; ")
    }

    pub fn record_history_visit(&mut self, url: &str, title: &str) {
        let clean_url = url.trim();
        if clean_url.is_empty() || clean_url.starts_with("manta://") {
            return;
        }
        if let Some(existing) = self.history.iter_mut().find(|h| h.url == clean_url) {
            existing.visit_count = existing.visit_count.saturating_add(1);
            if !title.trim().is_empty() {
                existing.title = title.trim().to_string();
            }
        } else {
            if self.history.len() >= 2048 {
                self.history.remove(0);
            }
            self.history.push(HistoryVisitEntry {
                url: clean_url.to_string(),
                title: if title.trim().is_empty() {
                    clean_url.to_string()
                } else {
                    title.trim().to_string()
                },
                visit_count: 1,
            });
        }
        if self.auto_commit_on_write {
            self.persist_history();
        }
    }

    pub fn history(&self) -> &[HistoryVisitEntry] {
        &self.history
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history.shrink_to_fit();
        self.persist_history();
    }

    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        let clean_url = url.trim();
        if clean_url.is_empty() {
            return false;
        }
        let added = if let Some(pos) = self.bookmarks.iter().position(|b| b.url == clean_url) {
            self.bookmarks.remove(pos);
            false
        } else {
            if self.bookmarks.len() < 1024 {
                self.bookmarks.push(BookmarkEntry {
                    url: clean_url.to_string(),
                    title: if title.trim().is_empty() {
                        clean_url.to_string()
                    } else {
                        title.trim().to_string()
                    },
                });
            }
            true
        };
        if self.auto_commit_on_write {
            self.persist_bookmarks();
        }
        added
    }

    pub fn clear_bookmarks(&mut self) {
        self.bookmarks.clear();
        self.bookmarks.shrink_to_fit();
        self.persist_bookmarks();
    }

    pub fn bookmarks(&self) -> &[BookmarkEntry] {
        &self.bookmarks
    }

    pub fn export_bookmarks_json(&self) -> String {
        let mut out = String::from("[");
        for (idx, b) in self.bookmarks.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                "{{\"url\":\"{}\",\"title\":\"{}\"}}",
                escape_json_str(&b.url),
                escape_json_str(&b.title)
            ));
        }
        out.push(']');
        out
    }

    pub fn clear_origin(&mut self, origin_url: &str) -> bool {
        let origin = OriginKey::from_url(origin_url).serialize();
        let mut removed_any = false;
        if let Some(removed) = self.origins.remove(&origin) {
            self.total_used_bytes = self.total_used_bytes.saturating_sub(removed.used_bytes);
            removed_any = true;
        }
        if let Some(dir) = self.site_dir_readonly(&origin) {
            if dir.exists() {
                let _ = fs::remove_dir_all(&dir);
                removed_any = true;
            }
        }
        if removed_any {
            self.write_index_manifest();
        }
        removed_any
    }

    pub fn clear_all_session_storage(&mut self) {
        for bucket in self.origins.values_mut() {
            let session_bytes: usize = bucket
                .session_storage
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum();
            bucket.session_storage.clear();
            bucket.used_bytes = bucket.used_bytes.saturating_sub(session_bytes);
            self.total_used_bytes = self.total_used_bytes.saturating_sub(session_bytes);
        }
    }

    pub fn clear_all(&mut self) {
        self.origins.clear();
        self.origins.shrink_to_fit();
        self.bookmarks.clear();
        self.bookmarks.shrink_to_fit();
        self.history.clear();
        self.history.shrink_to_fit();
        self.saved_tabs.clear();
        self.saved_tabs.shrink_to_fit();
        self.offline_mem_fallback.clear();
        self.offline_mem_fallback.shrink_to_fit();
        self.total_used_bytes = 0;

        if let Some(ref root) = self.storage_dir_path {
            let _ = fs::remove_file(root.join("bookmarks.json"));
            let _ = fs::remove_file(root.join("history.json"));
            let _ = fs::remove_file(root.join("tabs_session.json"));
            let _ = fs::remove_file(root.join("manta_index.json"));
            let _ = fs::remove_dir_all(root.join("sites"));
            let _ = fs::remove_dir_all(root.join("offline_pages"));
            let _ = fs::remove_dir_all(root.join("extensions_data"));
            let _ = fs::remove_dir(root);
        }
    }

    pub fn save_to_backing_file(&mut self) -> Result<(), String> {
        if self.ephemeral_only || self.storage_dir_path.is_none() {
            return Ok(());
        }
        self.persist_bookmarks();
        self.persist_history();
        self.persist_tabs_session();

        let keys: Vec<String> = self.origins.keys().cloned().collect();
        for origin in keys {
            self.persist_origin_local_storage(&origin);
            self.persist_origin_form_drafts(&origin);
            self.persist_origin_cookies(&origin);
            let store_names: Vec<String> = self
                .origins
                .get(&origin)
                .map(|b| b.object_stores.keys().cloned().collect())
                .unwrap_or_default();
            for sname in store_names {
                self.persist_origin_idb_store(&origin, &sname);
            }
        }
        self.write_index_manifest();
        Ok(())
    }

    pub fn load_from_backing_file(&mut self) -> Result<(), String> {
        if self.ephemeral_only {
            return Ok(());
        }
        let Some(root) = self.storage_dir_path.clone() else {
            return Ok(());
        };
        if !root.exists() {
            return Ok(());
        }

        self.suppress_disk_write = true;

        if let Ok(raw) = fs::read_to_string(root.join("bookmarks.json")) {
            if let Some(JsonVal::Arr(arr)) = parse_json(&raw) {
                self.bookmarks.clear();
                for item in arr {
                    if let JsonVal::Obj(pairs) = item {
                        let url = json_get_str(&pairs, "url").unwrap_or_default();
                        let title = json_get_str(&pairs, "title").unwrap_or_default();
                        if !url.is_empty() {
                            self.bookmarks.push(BookmarkEntry { url, title });
                        }
                    }
                }
            }
        }

        if let Ok(raw) = fs::read_to_string(root.join("history.json")) {
            if let Some(JsonVal::Arr(arr)) = parse_json(&raw) {
                self.history.clear();
                for item in arr {
                    if let JsonVal::Obj(pairs) = item {
                        let url = json_get_str(&pairs, "url").unwrap_or_default();
                        let title = json_get_str(&pairs, "title").unwrap_or_default();
                        let visit_count =
                            json_get_i64(&pairs, "visit_count").unwrap_or(1).max(1) as u32;
                        if !url.is_empty() {
                            self.history.push(HistoryVisitEntry {
                                url,
                                title,
                                visit_count,
                            });
                        }
                    }
                }
            }
        }

        if let Ok(raw) = fs::read_to_string(root.join("tabs_session.json")) {
            if let Some(JsonVal::Arr(arr)) = parse_json(&raw) {
                self.saved_tabs.clear();
                for item in arr {
                    if let JsonVal::Obj(pairs) = item {
                        let tab_id = json_get_i64(&pairs, "tab_id").unwrap_or(0) as u32;
                        let url = json_get_str(&pairs, "url").unwrap_or_default();
                        let title = json_get_str(&pairs, "title").unwrap_or_default();
                        let scroll_y = json_get_i64(&pairs, "scroll_y").unwrap_or(0) as i32;
                        if tab_id > 0 && !url.is_empty() {
                            self.saved_tabs.insert(
                                tab_id,
                                SavedTabSession {
                                    tab_id,
                                    url,
                                    title,
                                    scroll_y,
                                },
                            );
                        }
                    }
                }
            }
        }

        let sites_dir = root.join("sites");
        if let Ok(entries) = fs::read_dir(&sites_dir) {
            for entry in entries.flatten() {
                let sdir = entry.path();
                if !sdir.is_dir() {
                    continue;
                }
                let origin_url = fs::read_to_string(sdir.join("meta.json"))
                    .ok()
                    .and_then(|raw| parse_json(&raw))
                    .and_then(|v| match v {
                        JsonVal::Obj(pairs) => json_get_str(&pairs, "origin"),
                        _ => None,
                    })
                    .unwrap_or_else(|| {
                        folder_slug_to_origin(
                            sdir.file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("https__unknown"),
                        )
                    });

                let serialized = OriginKey::from_url(&origin_url).serialize();
                if serialized == "null" {
                    continue;
                }

                if let Ok(raw) = fs::read_to_string(sdir.join("local_storage.json")) {
                    if let Some(JsonVal::Obj(pairs)) = parse_json(&raw) {
                        for (k, v) in pairs {
                            if let JsonVal::Str(s) = v {
                                let _ = self.set_kv_internal(&serialized, &k, &s, false);
                            }
                        }
                    }
                }

                if let Ok(raw) = fs::read_to_string(sdir.join("form_drafts.json")) {
                    if let Some(JsonVal::Obj(pairs)) = parse_json(&raw) {
                        for (k, v) in pairs {
                            if let JsonVal::Str(s) = v {
                                let _ = self.save_form_draft(&serialized, &k, &s);
                            }
                        }
                    }
                }

                if let Ok(raw) = fs::read_to_string(sdir.join("cookies.json")) {
                    if let Some(JsonVal::Arr(arr)) = parse_json(&raw) {
                        let bucket = self.origins.entry(serialized.clone()).or_default();
                        for item in arr {
                            if let JsonVal::Obj(pairs) = item {
                                let name = json_get_str(&pairs, "name").unwrap_or_default();
                                let value = json_get_str(&pairs, "value").unwrap_or_default();
                                if name.is_empty() {
                                    continue;
                                }
                                let domain = json_get_str(&pairs, "domain").unwrap_or_default();
                                let path =
                                    json_get_str(&pairs, "path").unwrap_or_else(|| "/".to_string());
                                let secure = json_get_bool(&pairs, "secure").unwrap_or(false);
                                let http_only = json_get_bool(&pairs, "http_only").unwrap_or(false);
                                let same_site = json_get_str(&pairs, "same_site")
                                    .unwrap_or_else(|| "Lax".to_string());
                                let cost = name.len() + value.len();
                                bucket.cookies.insert(
                                    name.clone(),
                                    StoredCookie {
                                        name,
                                        value,
                                        domain,
                                        path,
                                        secure,
                                        http_only,
                                        same_site,
                                    },
                                );
                                bucket.used_bytes += cost;
                                self.total_used_bytes += cost;
                            }
                        }
                    }
                }

                let idb_dir = sdir.join("indexeddb");
                if let Ok(idb_entries) = fs::read_dir(&idb_dir) {
                    for idb_ent in idb_entries.flatten() {
                        let p = idb_ent.path();
                        if p.extension().and_then(|e| e.to_str()) != Some("json") {
                            continue;
                        }
                        let store_name = p
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        if let Ok(raw) = fs::read_to_string(&p) {
                            if let Some(JsonVal::Obj(pairs)) = parse_json(&raw) {
                                for (k, v) in pairs {
                                    if let JsonVal::Str(s) = v {
                                        let _ = self.put_idb_object(&serialized, &store_name, &k, &s);
                                    }
                                }
                            }
                        }
                    }
                }

                let files_dir = sdir.join("files");
                if files_dir.is_dir() {
                    let mut found = Vec::new();
                    collect_files_recursive(&files_dir, &files_dir, &mut found);
                    let bucket = self.origins.entry(serialized.clone()).or_default();
                    for (rel, _, size) in found {
                        let vpath = format!("/{}", rel);
                        bucket.saved_files_meta.insert(
                            vpath,
                            ("application/octet-stream".to_string(), size as usize, None),
                        );
                        bucket.used_bytes += size as usize;
                        self.total_used_bytes += size as usize;
                    }
                }
            }
        }

        self.suppress_disk_write = false;
        Ok(())
    }
}

fn ensure_dir_readable(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

fn atomic_write_file(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_dir_readable(parent)?;
    }
    let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
    {
        let mut f = fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
        f.write_all(data).map_err(|e| e.to_string())?;
        let _ = f.sync_all();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o644));
    }
    fs::rename(&tmp_path, path).map_err(|e| e.to_string())?;
    Ok(())
}

fn collect_files_recursive(root: &Path, cur: &Path, out: &mut Vec<(String, String, u64)>) {
    let Ok(entries) = fs::read_dir(cur) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_files_recursive(root, &p, out);
        } else if p.is_file() {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let abs = p.to_string_lossy().into_owned();
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push((rel, abs, size));
        }
    }
}

fn origin_to_folder_slug(serialized_origin: &str) -> String {
    let replaced = serialized_origin.replace("://", "__").replace(':', "__");
    let clean: String = replaced
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "unknown_site".to_string()
    } else {
        clean
    }
}

fn folder_slug_to_origin(slug: &str) -> String {
    if let Some((scheme, rest)) = slug.split_once("__") {
        if let Some((host, port)) = rest.split_once("__") {
            return format!("{}://{}:{}", scheme, host, port);
        }
        return format!("{}://{}", scheme, rest);
    }
    format!("https://{}", slug)
}

fn safe_file_slug(raw: &str) -> String {
    let mut s = String::with_capacity(raw.len().min(96));
    for c in raw.chars().take(96) {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            s.push(c);
        } else {
            s.push('_');
        }
    }
    let trimmed = s.trim_matches('_');
    if trimmed.is_empty() {
        "index".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sanitize_virtual_path(raw: &str) -> String {
    let trimmed = raw.trim().replace('\\', "/");
    let mut parts = Vec::new();
    for seg in trimmed.split('/') {
        let s = seg.trim();
        if s.is_empty() || s == "." || s == ".." {
            continue;
        }
        parts.push(s);
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("/{}", parts.join("/"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transparent_device_files_readback_locate_and_delete() {
        let dir = "./test_manta_transparent_files";
        let _ = fs::remove_dir_all(dir);

        {
            let mut store = MantaStorageEngine::new(1024 * 1024, 4 * 1024 * 1024);
            store.set_storage_dir(dir);

            let origin = "https://manta.dev/workspace";
            assert!(store.set_local_storage(origin, "theme", "obsidian"));
            assert!(store.set_session_storage(origin, "volatile_tab", "only_in_ram"));
            assert!(store.save_form_draft(origin, "/workspace::input::title", "Manta v4.0"));
            assert!(store.set_cookie_from_header(origin, "auth=token_xyz; Path=/; Secure"));
            assert!(store.put_idb_object(origin, "cache", "k1", "{\"v\":1}"));
            assert!(store.save_vfs_file(origin, "/notes/todo.txt", "text/plain", b"Ship fast"));
            store.record_history_visit(origin, "Manta Workspace");
            assert!(store.toggle_bookmark(origin, "Manta Workspace"));
            store.save_tab_session_state(1, origin, "Workspace", 320);
            assert!(store.save_offline_page_snapshot(
                origin,
                b"<html><body><h1>Saved Offline</h1></body></html>"
            ));

            let origin_dir = store.locate_origin_dir(origin).unwrap();
            assert!(Path::new(&origin_dir).join("local_storage.json").exists());
            assert!(Path::new(&origin_dir).join("cookies.json").exists());
            assert!(Path::new(&origin_dir).join("form_drafts.json").exists());
            assert!(Path::new(&origin_dir).join("indexeddb/cache.json").exists());
            assert!(Path::new(&origin_dir).join("files/notes/todo.txt").exists());
            assert!(Path::new(dir).join("bookmarks.json").exists());
            assert!(Path::new(dir).join("history.json").exists());
            assert!(Path::new(dir).join("tabs_session.json").exists());
            assert!(Path::new(dir).join("manta_index.json").exists());
        }

        let mut restored = MantaStorageEngine::new(1024 * 1024, 4 * 1024 * 1024);
        restored.set_storage_dir(dir);
        restored.load_from_backing_file().unwrap();

        assert_eq!(
            restored.get_local_storage("https://manta.dev", "theme"),
            Some("obsidian".to_string())
        );
        assert_eq!(
            restored.get_session_storage("https://manta.dev", "volatile_tab"),
            None
        );
        assert_eq!(
            restored.get_idb_object("https://manta.dev", "cache", "k1"),
            Some("{\"v\":1}".to_string())
        );
        assert_eq!(
            restored.read_vfs_file("https://manta.dev", "/notes/todo.txt"),
            Some(b"Ship fast".to_vec())
        );

        let origin_dir = restored.locate_origin_dir("https://manta.dev").unwrap();
        assert!(restored.clear_origin("https://manta.dev"));
        assert!(!Path::new(&origin_dir).exists());

        restored.clear_all();
        assert!(!Path::new(dir).exists());
    }
}
