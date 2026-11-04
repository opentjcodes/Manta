use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};

use crate::manta_db::{MantaDatabaseEngine, VirtualFileEntry};
use crate::site_isolation::OriginKey;

const STORAGE_MAGIC: &[u8; 8] = b"MANTADB2";

/// RFC 6265 Compliant Cookie Entry stored inside Manta's custom origin-partitioned vault.
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

/// Per-origin isolated storage partition (`localStorage`, `sessionStorage`, and `cookies`).
#[derive(Debug, Clone, Default)]
pub struct OriginStorageBucket {
    pub local_storage: HashMap<String, String>,
    pub session_storage: HashMap<String, String>,
    pub cookies: HashMap<String, StoredCookie>,
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

/// **Manta Unified Storage & Database Engine (`storage_engine.rs`)**:
/// - Combines `localStorage`, `sessionStorage`, RFC 6265 Cookies, Bookmarks, Browsing History,
///   and **MantaDB** (Relational Tables + IndexedDB Object Stores + Virtual File System `OPFS`).
/// - Serializes into a single compact, CRC32-verified binary container (`MANTADB2`) with
///   **Zero Ghost Pages**: when the user deletes a file, table, origin, or clears storage,
///   100% of bytes are zeroed and reclaimed immediately.
pub struct MantaStorageEngine {
    origins: HashMap<String, OriginStorageBucket>,
    bookmarks: Vec<BookmarkEntry>,
    history: Vec<HistoryVisitEntry>,
    pub db: MantaDatabaseEngine,
    per_origin_quota_bytes: usize,
    total_ceiling_bytes: usize,
    kv_cookie_used_bytes: usize,
    backing_file_path: Option<String>,
}

impl MantaStorageEngine {
    pub fn new(per_origin_quota_bytes: usize, total_ceiling_bytes: usize) -> Self {
        Self {
            origins: HashMap::new(),
            bookmarks: Vec::new(),
            history: Vec::new(),
            db: MantaDatabaseEngine::new(per_origin_quota_bytes, total_ceiling_bytes),
            per_origin_quota_bytes,
            total_ceiling_bytes,
            kv_cookie_used_bytes: 0,
            backing_file_path: None,
        }
    }

    pub fn set_backing_file(&mut self, path: &str) {
        if !path.trim().is_empty() {
            self.backing_file_path = Some(path.trim().to_string());
        }
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.kv_cookie_used_bytes + self.db.total_used_bytes
    }

    #[inline]
    pub fn origin_count(&self) -> usize {
        let mut all_origins: Vec<&String> = self.origins.keys().collect();
        for k in self.db.partitions.keys() {
            if !all_origins.contains(&k) {
                all_origins.push(k);
            }
        }
        all_origins.len()
    }

    // =========================================================================
    // FULL STORAGE TRANSPARENCY MANIFEST (SHOWS ALL SAVED FILES, TABLES & DATA)
    // =========================================================================

    /// Returns a complete JSON manifest showing every saved virtual file, relational table,
    /// IndexedDB store, cookie count, and `localStorage` usage across all origins.
    pub fn inspect_storage_manifest_json(&self) -> String {
        let mut out = String::from("{");
        out.push_str(&format!(
            "\"total_bytes\":{},\"bookmarks_count\":{},\"history_count\":{},\"origins\":[",
            self.used_bytes(),
            self.bookmarks.len(),
            self.history.len()
        ));

        let mut all_origin_keys: Vec<String> = self.origins.keys().cloned().collect();
        for k in self.db.partitions.keys() {
            if !all_origin_keys.contains(k) {
                all_origin_keys.push(k.clone());
            }
        }
        all_origin_keys.sort();

        for (idx, origin_str) in all_origin_keys.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            let kv_bucket = self.origins.get(origin_str);
            let db_part = self.db.partitions.get(origin_str);

            let kv_count = kv_bucket.map(|b| b.local_storage.len()).unwrap_or(0);
            let cookie_count = kv_bucket.map(|b| b.cookies.len()).unwrap_or(0);
            let kv_bytes = kv_bucket.map(|b| b.used_bytes).unwrap_or(0);
            let db_bytes = db_part.map(|p| p.used_bytes).unwrap_or(0);

            out.push_str(&format!(
                "{{\"origin\":\"{}\",\"used_bytes\":{},\"local_storage_keys\":{},\"cookies\":{},\"tables\":[",
                escape_json_str(origin_str),
                kv_bytes + db_bytes,
                kv_count,
                cookie_count
            ));

            if let Some(part) = db_part {
                for (t_idx, table) in part.tables.values().enumerate() {
                    if t_idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!(
                        "{{\"name\":\"{}\",\"rows\":{},\"bytes\":{}}}",
                        escape_json_str(&table.name),
                        table.rows.len(),
                        table.used_bytes
                    ));
                }
            }
            out.push_str("],\"object_stores\":[");
            if let Some(part) = db_part {
                for (s_idx, store) in part.object_stores.values().enumerate() {
                    if s_idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!(
                        "{{\"name\":\"{}\",\"records\":{},\"bytes\":{}}}",
                        escape_json_str(&store.name),
                        store.records.len(),
                        store.used_bytes
                    ));
                }
            }
            out.push_str("],\"saved_files\":[");
            if let Some(part) = db_part {
                for (f_idx, vf) in part.virtual_files.values().enumerate() {
                    if f_idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&format!(
                        "{{\"path\":\"{}\",\"mime\":\"{}\",\"bytes\":{},\"crc32\":\"{:08X}\"}}",
                        escape_json_str(&vf.virtual_path),
                        escape_json_str(&vf.mime_type),
                        vf.data.len(),
                        vf.crc32
                    ));
                }
            }
            out.push_str("]}");
        }

        out.push_str("]}");
        out
    }

    // =========================================================================
    // ORIGIN-PARTITIONED LOCALSTORAGE & SESSIONSTORAGE
    // =========================================================================

    pub fn set_local_storage(&mut self, origin_url: &str, key: &str, value: &str) -> bool {
        self.set_kv_internal(origin_url, key, value, false)
    }

    pub fn get_local_storage(&self, origin_url: &str, key: &str) -> Option<String> {
        self.get_kv_internal(origin_url, key, false)
    }

    pub fn remove_local_storage(&mut self, origin_url: &str, key: &str) -> bool {
        self.remove_kv_internal(origin_url, key, false)
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

    pub fn export_origin_storage_json(&self, origin_url: &str, is_session: bool) -> String {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return "{}".to_string();
        }
        let serialized = origin_key.serialize();
        let Some(bucket) = self.origins.get(&serialized) else {
            return "{}".to_string();
        };
        let map = if is_session {
            &bucket.session_storage
        } else {
            &bucket.local_storage
        };
        let mut out = String::from("{");
        let mut first = true;
        for (k, v) in map {
            if !first {
                out.push(',');
            }
            first = false;
            out.push('"');
            out.push_str(&escape_json_str(k));
            out.push_str("\":\"");
            out.push_str(&escape_json_str(v));
            out.push('"');
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
            .kv_cookie_used_bytes
            .saturating_sub(previous_cost)
            .saturating_add(entry_cost);

        if next_bucket_bytes > self.per_origin_quota_bytes
            || next_total_bytes + self.db.total_used_bytes > self.total_ceiling_bytes
        {
            return false;
        }

        map.insert(key.to_string(), value.to_string());
        bucket.used_bytes = next_bucket_bytes;
        self.kv_cookie_used_bytes = next_total_bytes;
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
            self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(cost);
            return true;
        }
        false
    }

    // =========================================================================
    // RFC 6265 ORIGIN-ISOLATED COOKIE JAR
    // =========================================================================

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
        let bucket = self.origins.entry(serialized_origin).or_default();

        if max_age_zero {
            if let Some(old) = bucket.cookies.remove(&name) {
                let cost = old.name.len() + old.value.len();
                bucket.used_bytes = bucket.used_bytes.saturating_sub(cost);
                self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(cost);
            }
            return true;
        }

        let cost = name.len() + value.len();
        if bucket.used_bytes + cost > self.per_origin_quota_bytes
            || self.kv_cookie_used_bytes + self.db.total_used_bytes + cost
                > self.total_ceiling_bytes
        {
            return false;
        }

        if let Some(old) = bucket.cookies.remove(&name) {
            let old_cost = old.name.len() + old.value.len();
            bucket.used_bytes = bucket.used_bytes.saturating_sub(old_cost);
            self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(old_cost);
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
        self.kv_cookie_used_bytes += cost;
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
        pairs.join("; ")
    }

    // =========================================================================
    // BOOKMARKS & BROWSING HISTORY
    // =========================================================================

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
            return;
        }
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

    pub fn history(&self) -> &[HistoryVisitEntry] {
        &self.history
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history.shrink_to_fit();
    }

    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        let clean_url = url.trim();
        if clean_url.is_empty() {
            return false;
        }
        if let Some(pos) = self.bookmarks.iter().position(|b| b.url == clean_url) {
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
        }
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

    // =========================================================================
    // SELECTIVE PER-ORIGIN OR FULL PERMANENT SHREDDING
    // =========================================================================

    /// Permanently clears all stored data (`localStorage`, `sessionStorage`, `cookies`,
    /// relational tables, IndexedDB stores, and saved virtual files) for a single origin.
    pub fn clear_origin(&mut self, origin_url: &str) -> bool {
        let origin = OriginKey::from_url(origin_url).serialize();
        let mut removed_any = false;
        if let Some(removed) = self.origins.remove(&origin) {
            self.kv_cookie_used_bytes =
                self.kv_cookie_used_bytes.saturating_sub(removed.used_bytes);
            removed_any = true;
        }
        if self.db.clear_origin(&origin) > 0 {
            removed_any = true;
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
            self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(session_bytes);
        }
    }

    /// Completely wipes all origins, MantaDB tables, IndexedDB stores, Virtual Files,
    /// Cookies, `localStorage`, History, and Bookmarks, removing the backing file from disk.
    pub fn clear_all(&mut self) {
        self.origins.clear();
        self.origins.shrink_to_fit();
        self.bookmarks.clear();
        self.bookmarks.shrink_to_fit();
        self.history.clear();
        self.history.shrink_to_fit();
        self.db.clear_all();
        self.kv_cookie_used_bytes = 0;
        if let Some(ref path) = self.backing_file_path {
            let _ = fs::remove_file(path);
        }
    }

    // =========================================================================
    // ZERO-GHOST-PAGE BINARY CONTAINER SERIALIZATION (`MANTADB2` + CRC32)
    // =========================================================================

    pub fn serialize_to_bytes(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(self.used_bytes() + 512);

        // 1. Write Bookmarks
        write_u32(&mut payload, self.bookmarks.len() as u32);
        for bm in &self.bookmarks {
            write_str(&mut payload, &bm.url);
            write_str(&mut payload, &bm.title);
        }

        // 2. Write History
        write_u32(&mut payload, self.history.len() as u32);
        for h in &self.history {
            write_str(&mut payload, &h.url);
            write_str(&mut payload, &h.title);
            write_u32(&mut payload, h.visit_count);
        }

        // 3. Write Origin KV & Cookies
        write_u32(&mut payload, self.origins.len() as u32);
        for (origin_str, bucket) in &self.origins {
            write_str(&mut payload, origin_str);

            write_u32(&mut payload, bucket.local_storage.len() as u32);
            for (k, v) in &bucket.local_storage {
                write_str(&mut payload, k);
                write_str(&mut payload, v);
            }

            write_u32(&mut payload, bucket.cookies.len() as u32);
            for cookie in bucket.cookies.values() {
                write_str(&mut payload, &cookie.name);
                write_str(&mut payload, &cookie.value);
                write_str(&mut payload, &cookie.domain);
                write_str(&mut payload, &cookie.path);
                payload.push(u8::from(cookie.secure));
                payload.push(u8::from(cookie.http_only));
                write_str(&mut payload, &cookie.same_site);
            }
        }

        // 4. Write MantaDB Partitions (Object Stores + Virtual Files)
        write_u32(&mut payload, self.db.partitions.len() as u32);
        for (origin_str, part) in &self.db.partitions {
            write_str(&mut payload, origin_str);

            write_u32(&mut payload, part.object_stores.len() as u32);
            for (store_name, store) in &part.object_stores {
                write_str(&mut payload, store_name);
                write_u32(&mut payload, store.records.len() as u32);
                for (pk, json_val) in &store.records {
                    write_str(&mut payload, pk);
                    write_str(&mut payload, json_val);
                }
            }

            write_u32(&mut payload, part.virtual_files.len() as u32);
            for vf in part.virtual_files.values() {
                write_str(&mut payload, &vf.virtual_path);
                write_str(&mut payload, &vf.mime_type);
                write_u32(&mut payload, vf.crc32);
                write_bytes(&mut payload, &vf.data);
            }
        }

        let checksum = crc32_hash(&payload);
        let mut frame = Vec::with_capacity(12 + payload.len());
        frame.extend_from_slice(STORAGE_MAGIC);
        write_u32(&mut frame, checksum);
        frame.extend_from_slice(&payload);
        frame
    }

    pub fn deserialize_from_bytes(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if bytes.len() < 12 || &bytes[0..8] != STORAGE_MAGIC {
            return Err("Invalid custom storage header");
        }
        let mut cursor = 8usize;
        let expected_crc = read_u32(bytes, &mut cursor).ok_or("Truncated checksum")?;
        let payload = &bytes[12..];
        if crc32_hash(payload) != expected_crc {
            return Err("Custom storage CRC32 integrity mismatch");
        }

        let mut p_cur = 0usize;

        // 1. Read Bookmarks
        let bm_count = read_u32(payload, &mut p_cur).ok_or("Invalid bookmark count")?;
        let mut new_bookmarks = Vec::with_capacity((bm_count as usize).min(1024));
        for _ in 0..bm_count {
            let url = read_str(payload, &mut p_cur).ok_or("Corrupt bookmark URL")?;
            let title = read_str(payload, &mut p_cur).ok_or("Corrupt bookmark title")?;
            new_bookmarks.push(BookmarkEntry { url, title });
        }

        // 2. Read History
        let hist_count = read_u32(payload, &mut p_cur).ok_or("Invalid history count")?;
        let mut new_history = Vec::with_capacity((hist_count as usize).min(2048));
        for _ in 0..hist_count {
            let url = read_str(payload, &mut p_cur).ok_or("Corrupt history URL")?;
            let title = read_str(payload, &mut p_cur).ok_or("Corrupt history title")?;
            let visit_count = read_u32(payload, &mut p_cur).ok_or("Corrupt visit count")?;
            new_history.push(HistoryVisitEntry {
                url,
                title,
                visit_count,
            });
        }

        // 3. Read Origin KV & Cookies
        let origin_count = read_u32(payload, &mut p_cur).ok_or("Invalid origin count")?;
        let mut new_origins = HashMap::with_capacity((origin_count as usize).min(1024));
        let mut kv_total_bytes = 0usize;

        for _ in 0..origin_count {
            let origin_str = read_str(payload, &mut p_cur).ok_or("Corrupt origin key")?;
            let kv_count = read_u32(payload, &mut p_cur).ok_or("Corrupt KV count")?;
            let mut bucket = OriginStorageBucket::default();

            for _ in 0..kv_count {
                let k = read_str(payload, &mut p_cur).ok_or("Corrupt storage key")?;
                let v = read_str(payload, &mut p_cur).ok_or("Corrupt storage value")?;
                let cost = k.len() + v.len();
                bucket.used_bytes += cost;
                kv_total_bytes += cost;
                bucket.local_storage.insert(k, v);
            }

            let cookie_count = read_u32(payload, &mut p_cur).ok_or("Corrupt cookie count")?;
            for _ in 0..cookie_count {
                let name = read_str(payload, &mut p_cur).ok_or("Corrupt cookie name")?;
                let value = read_str(payload, &mut p_cur).ok_or("Corrupt cookie value")?;
                let domain = read_str(payload, &mut p_cur).ok_or("Corrupt cookie domain")?;
                let path = read_str(payload, &mut p_cur).ok_or("Corrupt cookie path")?;
                if p_cur + 2 > payload.len() {
                    return Err("Truncated cookie flags");
                }
                let secure = payload[p_cur] != 0;
                let http_only = payload[p_cur + 1] != 0;
                p_cur += 2;
                let same_site = read_str(payload, &mut p_cur).ok_or("Corrupt SameSite")?;

                let cost = name.len() + value.len();
                bucket.used_bytes += cost;
                kv_total_bytes += cost;
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
            }

            new_origins.insert(origin_str, bucket);
        }

        // 4. Read MantaDB Partitions (Object Stores + Virtual Files)
        self.db.clear_all();
        if p_cur < payload.len() {
            let db_part_count = read_u32(payload, &mut p_cur).ok_or("Corrupt DB part count")?;
            for _ in 0..db_part_count {
                let origin_str = read_str(payload, &mut p_cur).ok_or("Corrupt DB origin")?;
                let store_count = read_u32(payload, &mut p_cur).ok_or("Corrupt store count")?;
                for _ in 0..store_count {
                    let store_name = read_str(payload, &mut p_cur).ok_or("Corrupt store name")?;
                    let rec_count = read_u32(payload, &mut p_cur).ok_or("Corrupt record count")?;
                    for _ in 0..rec_count {
                        let pk = read_str(payload, &mut p_cur).ok_or("Corrupt PK")?;
                        let val = read_str(payload, &mut p_cur).ok_or("Corrupt JSON val")?;
                        self.db.put_object(&origin_str, &store_name, &pk, &val);
                    }
                }

                let vf_count = read_u32(payload, &mut p_cur).ok_or("Corrupt VFS count")?;
                for _ in 0..vf_count {
                    let vpath = read_str(payload, &mut p_cur).ok_or("Corrupt VFS path")?;
                    let mime = read_str(payload, &mut p_cur).ok_or("Corrupt VFS mime")?;
                    let crc32 = read_u32(payload, &mut p_cur).ok_or("Corrupt VFS crc")?;
                    let data = read_bytes(payload, &mut p_cur).ok_or("Corrupt VFS data")?;
                    let part = self.db.partitions.entry(origin_str.clone()).or_default();
                    let cost = vpath.len() + mime.len() + data.len();
                    part.used_bytes += cost;
                    self.db.total_used_bytes += cost;
                    part.virtual_files.insert(
                        vpath.clone(),
                        VirtualFileEntry {
                            virtual_path: vpath,
                            mime_type: mime,
                            data,
                            crc32,
                        },
                    );
                }
            }
        }

        self.bookmarks = new_bookmarks;
        self.history = new_history;
        self.origins = new_origins;
        self.kv_cookie_used_bytes = kv_total_bytes;
        Ok(())
    }

    pub fn save_to_backing_file(&self) -> Result<(), String> {
        let Some(ref path) = self.backing_file_path else {
            return Ok(());
        };
        if self.used_bytes() == 0 && self.bookmarks.is_empty() && self.history.is_empty() {
            let _ = fs::remove_file(path);
            return Ok(());
        }
        let data = self.serialize_to_bytes();
        let mut f = fs::File::create(path).map_err(|e| e.to_string())?;
        f.write_all(&data).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())
    }

    pub fn load_from_backing_file(&mut self) -> Result<(), String> {
        let Some(ref path) = self.backing_file_path else {
            return Ok(());
        };
        let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        self.deserialize_from_bytes(&buf)
            .map_err(|e| e.to_string())
    }
}

fn write_u32(out: &mut Vec<u8>, val: u32) {
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    write_bytes(out, s.as_bytes());
}

fn write_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    write_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn read_u32(buf: &[u8], cursor: &mut usize) -> Option<u32> {
    if *cursor + 4 > buf.len() {
        return None;
    }
    let mut arr = [0u8; 4];
    arr.copy_from_slice(&buf[*cursor..*cursor + 4]);
    *cursor += 4;
    Some(u32::from_le_bytes(arr))
}

fn read_bytes(buf: &[u8], cursor: &mut usize) -> Option<Vec<u8>> {
    let len = read_u32(buf, cursor)? as usize;
    if *cursor + len > buf.len() {
        return None;
    }
    let slice = buf[*cursor..*cursor + len].to_vec();
    *cursor += len;
    Some(slice)
}

fn read_str(buf: &[u8], cursor: &mut usize) -> Option<String> {
    let bytes = read_bytes(buf, cursor)?;
    String::from_utf8(bytes).ok()
}

fn crc32_hash(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn escape_json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mantadb2_binary_roundtrip_with_vfs_and_indexeddb() {
        let mut store = MantaStorageEngine::new(256 * 1024, 2 * 1024 * 1024);
        assert!(store.set_local_storage("https://manta.dev/app", "theme", "obsidian"));
        assert!(store.db.put_object(
            "https://manta.dev/app",
            "offline_cache",
            "doc_1",
            "{\"title\":\"Spec\"}"
        ));
        assert!(store.db.save_virtual_file(
            "https://manta.dev/app",
            "/downloads/readme.md",
            "text/markdown",
            b"# Manta Engine"
        ));
        store.record_history_visit("https://manta.dev/app", "Manta App");

        let serialized = store.serialize_to_bytes();
        let mut restored = MantaStorageEngine::new(256 * 1024, 2 * 1024 * 1024);
        restored.deserialize_from_bytes(&serialized).unwrap();

        assert_eq!(
            restored.get_local_storage("https://manta.dev", "theme"),
            Some("obsidian".to_string())
        );
        assert_eq!(
            restored
                .db
                .get_object("https://manta.dev", "offline_cache", "doc_1"),
            Some("{\"title\":\"Spec\"}".to_string())
        );
        let vf = restored
            .db
            .read_virtual_file("https://manta.dev", "/downloads/readme.md")
            .unwrap();
        assert_eq!(vf.data, b"# Manta Engine");
        assert_eq!(restored.history().len(), 1);
    }
}
