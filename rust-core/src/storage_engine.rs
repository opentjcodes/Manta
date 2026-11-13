use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};

use crate::cpp_bridge::secure_zero_slice;
use crate::crypto_vault::{
    generate_nonce_12, generate_volatile_entropy_32, open_record, pbkdf2_hmac_sha256, seal_record,
};
use crate::manta_db::{DbValue, MantaDatabaseEngine, RelationalTable, VirtualFileEntry};
use crate::site_isolation::OriginKey;

const STORAGE_MAGIC_V2: &[u8; 8] = b"MANTADB2";
const STORAGE_MAGIC_V3: &[u8; 8] = b"MANTADB3";
const STORAGE_MAGIC_V4: &[u8; 8] = b"MANTADB4";
const WAL_RECORD_MAGIC: &[u8; 4] = b"MWAL";

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

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalOp {
    SetLocal = 1,
    RemoveLocal = 2,
    ClearLocal = 3,
    SaveFormDraft = 4,
    SetCookie = 5,
    RecordHistory = 6,
    ClearHistory = 7,
    ToggleBookmark = 8,
    IdbPut = 9,
    IdbDelete = 10,
    VfsSave = 11,
    ClearOrigin = 12,
}

impl WalOp {
    fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(WalOp::SetLocal),
            2 => Some(WalOp::RemoveLocal),
            3 => Some(WalOp::ClearLocal),
            4 => Some(WalOp::SaveFormDraft),
            5 => Some(WalOp::SetCookie),
            6 => Some(WalOp::RecordHistory),
            7 => Some(WalOp::ClearHistory),
            8 => Some(WalOp::ToggleBookmark),
            9 => Some(WalOp::IdbPut),
            10 => Some(WalOp::IdbDelete),
            11 => Some(WalOp::VfsSave),
            12 => Some(WalOp::ClearOrigin),
            _ => None,
        }
    }
}

pub struct MantaStorageEngine {
    origins: HashMap<String, OriginStorageBucket>,
    bookmarks: Vec<BookmarkEntry>,
    history: Vec<HistoryVisitEntry>,
    pub db: MantaDatabaseEngine,
    per_origin_quota_bytes: usize,
    total_ceiling_bytes: usize,
    kv_cookie_used_bytes: usize,
    backing_file_path: Option<String>,
    wal_file_path: Option<String>,
    key_file_path: Option<String>,
    vault_key: [u8; 32],
    nonce_counter: u64,
    wal_pending_ops: usize,
    pub wal_compact_threshold: usize,
    pub auto_commit_on_write: bool,
    pub ephemeral_only: bool,
}

impl Drop for MantaStorageEngine {
    fn drop(&mut self) {
        secure_zero_slice(&mut self.vault_key);
    }
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
            wal_file_path: None,
            key_file_path: None,
            vault_key: generate_volatile_entropy_32(),
            nonce_counter: 1,
            wal_pending_ops: 0,
            wal_compact_threshold: 64,
            auto_commit_on_write: true,
            ephemeral_only: false,
        }
    }

    pub fn set_backing_file(&mut self, path: &str) {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            self.backing_file_path = Some(trimmed.to_string());
            self.wal_file_path = Some(format!("{}.wal", trimmed));
            self.key_file_path = Some(format!("{}.key", trimmed));
            self.ensure_or_load_key_file();
        }
    }

    pub fn backing_file(&self) -> Option<&str> {
        self.backing_file_path.as_deref()
    }

    pub fn set_ephemeral_mode(&mut self, enabled: bool) {
        self.ephemeral_only = enabled;
    }

    pub fn set_vault_passphrase(&mut self, passphrase: &str) {
        let salt = b"MantaVaultV4::PBKDF2::Salt::2026";
        self.vault_key = pbkdf2_hmac_sha256(passphrase.as_bytes(), salt, 10_000);
    }

    fn ensure_or_load_key_file(&mut self) {
        if self.ephemeral_only {
            return;
        }
        let Some(ref kpath) = self.key_file_path else {
            return;
        };
        if let Ok(mut f) = fs::File::open(kpath) {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_ok() && buf.len() == 32 {
                self.vault_key.copy_from_slice(&buf);
                secure_zero_slice(&mut buf);
                return;
            }
            secure_zero_slice(&mut buf);
        }
        if let Ok(mut f) = fs::File::create(kpath) {
            let _ = f.write_all(&self.vault_key);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(kpath, fs::Permissions::from_mode(0o600));
            }
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

    fn append_wal_op(&mut self, op: WalOp, fields: &[&[u8]]) {
        if self.ephemeral_only || !self.auto_commit_on_write || self.wal_file_path.is_none() {
            return;
        }

        let mut plain = Vec::with_capacity(64);
        plain.push(op as u8);
        write_u32(&mut plain, fields.len() as u32);
        for field in fields {
            write_bytes(&mut plain, field);
        }

        self.nonce_counter = self.nonce_counter.wrapping_add(1);
        let nonce = generate_nonce_12(self.nonce_counter);
        let sealed = seal_record(&self.vault_key, &nonce, WAL_RECORD_MAGIC, &plain);
        secure_zero_slice(&mut plain);

        if let Some(ref wpath) = self.wal_file_path {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(wpath) {
                let mut frame = Vec::with_capacity(8 + sealed.len());
                frame.extend_from_slice(WAL_RECORD_MAGIC);
                write_u32(&mut frame, sealed.len() as u32);
                frame.extend_from_slice(&sealed);
                let _ = file.write_all(&frame);
                let _ = file.flush();
            }
        }

        self.wal_pending_ops += 1;
        if self.wal_pending_ops >= self.wal_compact_threshold.max(8) {
            let _ = self.save_to_backing_file();
        }
    }

    pub fn inspect_storage_manifest_json(&self) -> String {
        let mut out = String::from("{");
        out.push_str(&format!(
            "\"total_bytes\":{},\"wal_pending_ops\":{},\"ephemeral_only\":{},\"bookmarks_count\":{},\"history_count\":{},\"origins\":[",
            self.used_bytes(),
            self.wal_pending_ops,
            self.ephemeral_only,
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
            let sess_count = kv_bucket.map(|b| b.session_storage.len()).unwrap_or(0);
            let draft_count = kv_bucket.map(|b| b.form_drafts.len()).unwrap_or(0);
            let cookie_count = kv_bucket.map(|b| b.cookies.len()).unwrap_or(0);
            let kv_bytes = kv_bucket.map(|b| b.used_bytes).unwrap_or(0);
            let db_bytes = db_part.map(|p| p.used_bytes).unwrap_or(0);

            out.push_str(&format!(
                "{{\"origin\":\"{}\",\"used_bytes\":{},\"local_storage_keys\":{},\"session_storage_keys\":{},\"form_drafts\":{},\"cookies\":{},\"tables\":[",
                escape_json_str(origin_str),
                kv_bytes + db_bytes,
                kv_count,
                sess_count,
                draft_count,
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
        if ok {
            self.append_wal_op(
                WalOp::SetLocal,
                &[origin_url.as_bytes(), key.as_bytes(), value.as_bytes()],
            );
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
        if ok {
            self.append_wal_op(WalOp::RemoveLocal, &[origin_url.as_bytes(), key.as_bytes()]);
        }
        ok
    }

    pub fn clear_local_storage_only(&mut self, origin_url: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque {
            return false;
        }
        let Some(bucket) = self.origins.get_mut(&origin_key.serialize()) else {
            return false;
        };
        let freed: usize = bucket
            .local_storage
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum();
        bucket.local_storage.clear();
        bucket.used_bytes = bucket.used_bytes.saturating_sub(freed);
        self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(freed);
        self.append_wal_op(WalOp::ClearLocal, &[origin_url.as_bytes()]);
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
        self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(freed);
        true
    }

    pub fn save_form_draft(&mut self, origin_url: &str, field_key: &str, value: &str) -> bool {
        let ok = self.apply_save_form_draft(origin_url, field_key, value);
        if ok {
            self.append_wal_op(
                WalOp::SaveFormDraft,
                &[origin_url.as_bytes(), field_key.as_bytes(), value.as_bytes()],
            );
        }
        ok
    }

    fn apply_save_form_draft(&mut self, origin_url: &str, field_key: &str, value: &str) -> bool {
        let origin_key = OriginKey::from_url(origin_url);
        if origin_key.is_opaque || field_key.trim().is_empty() {
            return false;
        }
        let serialized_origin = origin_key.serialize();
        let entry_cost = field_key.len() + value.len();

        let bucket = self.origins.entry(serialized_origin).or_default();
        let prev_cost = bucket
            .form_drafts
            .get(field_key)
            .map(|old| field_key.len() + old.len())
            .unwrap_or(0);

        let next_bucket = bucket.used_bytes.saturating_sub(prev_cost) + entry_cost;
        let next_total = self.kv_cookie_used_bytes.saturating_sub(prev_cost) + entry_cost;
        if next_bucket > self.per_origin_quota_bytes
            || next_total + self.db.total_used_bytes > self.total_ceiling_bytes
        {
            return false;
        }

        bucket
            .form_drafts
            .insert(field_key.to_string(), value.to_string());
        bucket.used_bytes = next_bucket;
        self.kv_cookie_used_bytes = next_total;
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

    pub fn clear_form_drafts_for_origin(&mut self, origin_url: &str) {
        let origin_key = OriginKey::from_url(origin_url);
        if let Some(bucket) = self.origins.get_mut(&origin_key.serialize()) {
            let freed: usize = bucket
                .form_drafts
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum();
            bucket.form_drafts.clear();
            bucket.used_bytes = bucket.used_bytes.saturating_sub(freed);
            self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(freed);
        }
    }

    pub fn put_idb_object(
        &mut self,
        origin_url: &str,
        store_name: &str,
        key: &str,
        json_val: &str,
    ) -> bool {
        let ok = self.db.put_object(origin_url, store_name, key, json_val);
        if ok {
            self.append_wal_op(
                WalOp::IdbPut,
                &[
                    origin_url.as_bytes(),
                    store_name.as_bytes(),
                    key.as_bytes(),
                    json_val.as_bytes(),
                ],
            );
        }
        ok
    }

    pub fn delete_idb_object(&mut self, origin_url: &str, store_name: &str, key: &str) -> bool {
        let ok = self.db.delete_object(origin_url, store_name, key);
        if ok {
            self.append_wal_op(
                WalOp::IdbDelete,
                &[origin_url.as_bytes(), store_name.as_bytes(), key.as_bytes()],
            );
        }
        ok
    }

    pub fn save_vfs_file(
        &mut self,
        origin_url: &str,
        virtual_path: &str,
        mime_type: &str,
        data: &[u8],
    ) -> bool {
        let ok = self.db.save_virtual_file(origin_url, virtual_path, mime_type, data);
        if ok {
            self.append_wal_op(
                WalOp::VfsSave,
                &[
                    origin_url.as_bytes(),
                    virtual_path.as_bytes(),
                    mime_type.as_bytes(),
                    data,
                ],
            );
        }
        ok
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
        let Some(part) = self.db.partitions.get(&origin_key.serialize()) else {
            return "{}".to_string();
        };
        let mut out = String::from("{");
        for (idx, (store_name, store)) in part.object_stores.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            out.push('"');
            out.push_str(&escape_json_str(store_name));
            out.push_str("\":");
            out.push_str(&map_to_json_object(&store.records));
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

    pub fn set_cookie_from_header(&mut self, origin_url: &str, raw_set_cookie: &str) -> bool {
        let ok = self.apply_set_cookie_from_header(origin_url, raw_set_cookie);
        if ok {
            self.append_wal_op(
                WalOp::SetCookie,
                &[origin_url.as_bytes(), raw_set_cookie.as_bytes()],
            );
        }
        ok
    }

    fn apply_set_cookie_from_header(&mut self, origin_url: &str, raw_set_cookie: &str) -> bool {
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
        pairs.sort();
        pairs.join("; ")
    }

    pub fn record_history_visit(&mut self, url: &str, title: &str) {
        let clean_url = url.trim();
        if clean_url.is_empty() || clean_url.starts_with("manta://") {
            return;
        }
        self.apply_record_history_visit(clean_url, title);
        self.append_wal_op(WalOp::RecordHistory, &[clean_url.as_bytes(), title.as_bytes()]);
    }

    fn apply_record_history_visit(&mut self, clean_url: &str, title: &str) {
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
        self.append_wal_op(WalOp::ClearHistory, &[]);
    }

    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        let clean_url = url.trim();
        if clean_url.is_empty() {
            return false;
        }
        let added = self.apply_toggle_bookmark(clean_url, title);
        self.append_wal_op(WalOp::ToggleBookmark, &[clean_url.as_bytes(), title.as_bytes()]);
        added
    }

    fn apply_toggle_bookmark(&mut self, clean_url: &str, title: &str) -> bool {
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

    pub fn clear_origin(&mut self, origin_url: &str) -> bool {
        let removed = self.apply_clear_origin(origin_url);
        if removed {
            self.append_wal_op(WalOp::ClearOrigin, &[origin_url.as_bytes()]);
        }
        removed
    }

    fn apply_clear_origin(&mut self, origin_url: &str) -> bool {
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

    pub fn clear_all(&mut self) {
        self.origins.clear();
        self.origins.shrink_to_fit();
        self.bookmarks.clear();
        self.bookmarks.shrink_to_fit();
        self.history.clear();
        self.history.shrink_to_fit();
        self.db.clear_all();
        self.kv_cookie_used_bytes = 0;
        self.wal_pending_ops = 0;

        secure_zero_slice(&mut self.vault_key);
        self.vault_key = generate_volatile_entropy_32();

        if let Some(ref path) = self.backing_file_path {
            let _ = fs::remove_file(path);
            let _ = fs::remove_file(format!("{}.tmp", path));
        }
        if let Some(ref wpath) = self.wal_file_path {
            let _ = fs::remove_file(wpath);
        }
        if let Some(ref kpath) = self.key_file_path {
            if let Ok(mut f) = OpenOptions::new().write(true).open(kpath) {
                let zeros = [0u8; 32];
                let _ = f.write_all(&zeros);
                let _ = f.sync_all();
            }
            let _ = fs::remove_file(kpath);
        }
    }

    fn serialize_plain_payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(self.used_bytes() + 1024);

        write_u32(&mut payload, self.bookmarks.len() as u32);
        for bm in &self.bookmarks {
            write_str(&mut payload, &bm.url);
            write_str(&mut payload, &bm.title);
        }

        write_u32(&mut payload, self.history.len() as u32);
        for h in &self.history {
            write_str(&mut payload, &h.url);
            write_str(&mut payload, &h.title);
            write_u32(&mut payload, h.visit_count);
        }

        write_u32(&mut payload, self.origins.len() as u32);
        for (origin_str, bucket) in &self.origins {
            write_str(&mut payload, origin_str);

            write_u32(&mut payload, bucket.local_storage.len() as u32);
            for (k, v) in &bucket.local_storage {
                write_str(&mut payload, k);
                write_str(&mut payload, v);
            }

            write_u32(&mut payload, 0);

            write_u32(&mut payload, bucket.form_drafts.len() as u32);
            for (k, v) in &bucket.form_drafts {
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

        write_u32(&mut payload, self.db.partitions.len() as u32);
        for (origin_str, part) in &self.db.partitions {
            write_str(&mut payload, origin_str);

            write_u32(&mut payload, part.tables.len() as u32);
            for (table_name, table) in &part.tables {
                write_str(&mut payload, table_name);
                write_u64(&mut payload, table.next_rowid);
                write_u32(&mut payload, table.columns.len() as u32);
                for col in &table.columns {
                    write_str(&mut payload, col);
                }
                write_u32(&mut payload, table.rows.len() as u32);
                for (rowid, vals) in &table.rows {
                    write_u64(&mut payload, *rowid);
                    write_u32(&mut payload, vals.len() as u32);
                    for val in vals {
                        write_db_value(&mut payload, val);
                    }
                }
            }

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

        payload
    }

    pub fn serialize_to_bytes(&self) -> Vec<u8> {
        let mut payload = self.serialize_plain_payload();
        let nonce = generate_nonce_12(self.nonce_counter.wrapping_add(0x4D41_4E54));
        let sealed = seal_record(&self.vault_key, &nonce, STORAGE_MAGIC_V4, &payload);
        secure_zero_slice(&mut payload);

        let mut frame = Vec::with_capacity(8 + sealed.len());
        frame.extend_from_slice(STORAGE_MAGIC_V4);
        frame.extend_from_slice(&sealed);
        frame
    }

    pub fn deserialize_from_bytes(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        if bytes.len() < 12 {
            return Err("Invalid custom storage header");
        }
        if &bytes[0..8] == STORAGE_MAGIC_V4 {
            let mut decrypted = open_record(&self.vault_key, STORAGE_MAGIC_V4, &bytes[8..])?;
            let res = self.parse_plain_payload(&decrypted, true);
            secure_zero_slice(&mut decrypted);
            return res;
        }

        let is_v3 = &bytes[0..8] == STORAGE_MAGIC_V3;
        let is_v2 = &bytes[0..8] == STORAGE_MAGIC_V2;
        if !is_v3 && !is_v2 {
            return Err("Unknown custom storage header magic");
        }
        let mut cursor = 8usize;
        let expected_crc = read_u32(bytes, &mut cursor).ok_or("Truncated checksum")?;
        let payload = &bytes[12..];
        if crc32_hash(payload) != expected_crc {
            return Err("Custom storage CRC32 integrity mismatch");
        }
        self.parse_plain_payload(payload, is_v3)
    }

    fn parse_plain_payload(&mut self, payload: &[u8], has_v3_fields: bool) -> Result<(), &'static str> {
        let mut p_cur = 0usize;

        let bm_count = read_u32(payload, &mut p_cur).ok_or("Invalid bookmark count")?;
        let mut new_bookmarks = Vec::with_capacity((bm_count as usize).min(1024));
        for _ in 0..bm_count {
            let url = read_str(payload, &mut p_cur).ok_or("Corrupt bookmark URL")?;
            let title = read_str(payload, &mut p_cur).ok_or("Corrupt bookmark title")?;
            new_bookmarks.push(BookmarkEntry { url, title });
        }

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

            if has_v3_fields {
                let sess_count = read_u32(payload, &mut p_cur).ok_or("Corrupt session count")?;
                for _ in 0..sess_count {
                    let _k = read_str(payload, &mut p_cur).ok_or("Corrupt session key")?;
                    let _v = read_str(payload, &mut p_cur).ok_or("Corrupt session value")?;
                }

                let draft_count = read_u32(payload, &mut p_cur).ok_or("Corrupt draft count")?;
                for _ in 0..draft_count {
                    let k = read_str(payload, &mut p_cur).ok_or("Corrupt draft key")?;
                    let v = read_str(payload, &mut p_cur).ok_or("Corrupt draft value")?;
                    let cost = k.len() + v.len();
                    bucket.used_bytes += cost;
                    kv_total_bytes += cost;
                    bucket.form_drafts.insert(k, v);
                }
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

        self.db.clear_all();
        if p_cur < payload.len() {
            let db_part_count = read_u32(payload, &mut p_cur).ok_or("Corrupt DB part count")?;
            for _ in 0..db_part_count {
                let origin_str = read_str(payload, &mut p_cur).ok_or("Corrupt DB origin")?;

                if has_v3_fields {
                    let table_count = read_u32(payload, &mut p_cur).ok_or("Corrupt table count")?;
                    for _ in 0..table_count {
                        let tname = read_str(payload, &mut p_cur).ok_or("Corrupt table name")?;
                        let next_rowid = read_u64(payload, &mut p_cur).ok_or("Corrupt next_rowid")?;
                        let col_count = read_u32(payload, &mut p_cur).ok_or("Corrupt col count")?;
                        let mut cols = Vec::with_capacity((col_count as usize).min(256));
                        for _ in 0..col_count {
                            cols.push(read_str(payload, &mut p_cur).ok_or("Corrupt col name")?);
                        }
                        let row_count = read_u32(payload, &mut p_cur).ok_or("Corrupt row count")?;
                        let mut rows = Vec::with_capacity((row_count as usize).min(65536));
                        let mut table_bytes = 0usize;
                        for _ in 0..row_count {
                            let rowid = read_u64(payload, &mut p_cur).ok_or("Corrupt rowid")?;
                            let val_count = read_u32(payload, &mut p_cur).ok_or("Corrupt val count")?;
                            let mut vals = Vec::with_capacity((val_count as usize).min(256));
                            for _ in 0..val_count {
                                let dv = read_db_value(payload, &mut p_cur).ok_or("Corrupt DbValue")?;
                                table_bytes += dv.approx_bytes();
                                vals.push(dv);
                            }
                            table_bytes += 16;
                            rows.push((rowid, vals));
                        }
                        let part = self.db.partitions.entry(origin_str.clone()).or_default();
                        part.used_bytes += table_bytes;
                        self.db.total_used_bytes += table_bytes;
                        part.tables.insert(
                            tname.clone(),
                            RelationalTable {
                                name: tname,
                                columns: cols,
                                rows,
                                next_rowid,
                                used_bytes: table_bytes,
                            },
                        );
                    }
                }

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

    pub fn save_to_backing_file(&mut self) -> Result<(), String> {
        if self.ephemeral_only {
            return Ok(());
        }
        let Some(path) = self.backing_file_path.clone() else {
            return Ok(());
        };
        if self.used_bytes() == 0 && self.bookmarks.is_empty() && self.history.is_empty() {
            let _ = fs::remove_file(&path);
            if let Some(ref wpath) = self.wal_file_path {
                let _ = fs::remove_file(wpath);
            }
            self.wal_pending_ops = 0;
            return Ok(());
        }

        self.ensure_or_load_key_file();
        let data = self.serialize_to_bytes();
        let tmp_path = format!("{}.tmp", path);
        {
            let mut f = fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
            f.write_all(&data).map_err(|e| e.to_string())?;
            f.sync_all().map_err(|e| e.to_string())?;
        }
        fs::rename(&tmp_path, &path).map_err(|e| e.to_string())?;

        if let Some(ref wpath) = self.wal_file_path {
            let _ = fs::remove_file(wpath);
        }
        self.wal_pending_ops = 0;
        Ok(())
    }

    pub fn load_from_backing_file(&mut self) -> Result<(), String> {
        if self.ephemeral_only {
            return Ok(());
        }
        self.ensure_or_load_key_file();

        if let Some(ref path) = self.backing_file_path {
            if let Ok(mut f) = fs::File::open(path) {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
                self.deserialize_from_bytes(&buf)
                    .map_err(|e| e.to_string())?;
            }
        }

        if let Some(wpath) = self.wal_file_path.clone() {
            if let Ok(mut wf) = fs::File::open(&wpath) {
                let mut wbuf = Vec::new();
                if wf.read_to_end(&mut wbuf).is_ok() {
                    self.replay_wal_bytes(&wbuf);
                }
            }
        }
        Ok(())
    }

    fn replay_wal_bytes(&mut self, bytes: &[u8]) {
        let mut cur = 0usize;
        while cur + 8 <= bytes.len() {
            if &bytes[cur..cur + 4] != WAL_RECORD_MAGIC {
                break;
            }
            cur += 4;
            let Some(frame_len) = read_u32(bytes, &mut cur) else {
                break;
            };
            let len = frame_len as usize;
            if cur + len > bytes.len() {
                break;
            }
            let sealed = &bytes[cur..cur + len];
            cur += len;

            if let Ok(mut plain) = open_record(&self.vault_key, WAL_RECORD_MAGIC, sealed) {
                self.apply_decoded_wal_record(&plain);
                secure_zero_slice(&mut plain);
                self.wal_pending_ops += 1;
            }
        }
    }

    fn apply_decoded_wal_record(&mut self, plain: &[u8]) {
        if plain.is_empty() {
            return;
        }
        let Some(op) = WalOp::from_u8(plain[0]) else {
            return;
        };
        let mut cur = 1usize;
        let Some(field_count) = read_u32(plain, &mut cur) else {
            return;
        };
        let mut fields = Vec::with_capacity((field_count as usize).min(8));
        for _ in 0..field_count {
            let Some(b) = read_bytes(plain, &mut cur) else {
                return;
            };
            fields.push(b);
        }

        let str_at = |i: usize| -> String {
            fields
                .get(i)
                .and_then(|b| std::str::from_utf8(b).ok())
                .unwrap_or("")
                .to_string()
        };

        match op {
            WalOp::SetLocal if fields.len() >= 3 => {
                let _ = self.set_kv_internal(&str_at(0), &str_at(1), &str_at(2), false);
            }
            WalOp::RemoveLocal if fields.len() >= 2 => {
                let _ = self.remove_kv_internal(&str_at(0), &str_at(1), false);
            }
            WalOp::ClearLocal if !fields.is_empty() => {
                let orig = OriginKey::from_url(&str_at(0));
                if let Some(bucket) = self.origins.get_mut(&orig.serialize()) {
                    let freed: usize = bucket
                        .local_storage
                        .iter()
                        .map(|(k, v)| k.len() + v.len())
                        .sum();
                    bucket.local_storage.clear();
                    bucket.used_bytes = bucket.used_bytes.saturating_sub(freed);
                    self.kv_cookie_used_bytes = self.kv_cookie_used_bytes.saturating_sub(freed);
                }
            }
            WalOp::SaveFormDraft if fields.len() >= 3 => {
                self.apply_save_form_draft(&str_at(0), &str_at(1), &str_at(2));
            }
            WalOp::SetCookie if fields.len() >= 2 => {
                self.apply_set_cookie_from_header(&str_at(0), &str_at(1));
            }
            WalOp::RecordHistory if fields.len() >= 2 => {
                self.apply_record_history_visit(&str_at(0), &str_at(1));
            }
            WalOp::ClearHistory => {
                self.history.clear();
            }
            WalOp::ToggleBookmark if fields.len() >= 2 => {
                self.apply_toggle_bookmark(&str_at(0), &str_at(1));
            }
            WalOp::IdbPut if fields.len() >= 4 => {
                self.db.put_object(&str_at(0), &str_at(1), &str_at(2), &str_at(3));
            }
            WalOp::IdbDelete if fields.len() >= 3 => {
                self.db.delete_object(&str_at(0), &str_at(1), &str_at(2));
            }
            WalOp::VfsSave if fields.len() >= 4 => {
                self.db
                    .save_virtual_file(&str_at(0), &str_at(1), &str_at(2), &fields[3]);
            }
            WalOp::ClearOrigin if !fields.is_empty() => {
                self.apply_clear_origin(&str_at(0));
            }
            _ => {}
        }
    }
}

fn map_to_json_object(map: &HashMap<String, String>) -> String {
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

fn write_db_value(out: &mut Vec<u8>, val: &DbValue) {
    match val {
        DbValue::Null => out.push(0),
        DbValue::Int(i) => {
            out.push(1);
            out.extend_from_slice(&i.to_le_bytes());
        }
        DbValue::Float(f) => {
            out.push(2);
            out.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        DbValue::Text(s) => {
            out.push(3);
            write_str(out, s);
        }
        DbValue::Blob(b) => {
            out.push(4);
            write_bytes(out, b);
        }
    }
}

fn read_db_value(buf: &[u8], cursor: &mut usize) -> Option<DbValue> {
    if *cursor >= buf.len() {
        return None;
    }
    let tag = buf[*cursor];
    *cursor += 1;
    match tag {
        0 => Some(DbValue::Null),
        1 => {
            if *cursor + 8 > buf.len() {
                return None;
            }
            let mut arr = [0u8; 8];
            arr.copy_from_slice(&buf[*cursor..*cursor + 8]);
            *cursor += 8;
            Some(DbValue::Int(i64::from_le_bytes(arr)))
        }
        2 => {
            let bits = read_u64(buf, cursor)?;
            Some(DbValue::Float(f64::from_bits(bits)))
        }
        3 => Some(DbValue::Text(read_str(buf, cursor)?)),
        4 => Some(DbValue::Blob(read_bytes(buf, cursor)?)),
        _ => None,
    }
}

fn write_u32(out: &mut Vec<u8>, val: u32) {
    out.extend_from_slice(&val.to_le_bytes());
}

fn write_u64(out: &mut Vec<u8>, val: u64) {
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

fn read_u64(buf: &[u8], cursor: &mut usize) -> Option<u64> {
    if *cursor + 8 > buf.len() {
        return None;
    }
    let mut arr = [0u8; 8];
    arr.copy_from_slice(&buf[*cursor..*cursor + 8]);
    *cursor += 8;
    Some(u64::from_le_bytes(arr))
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
    fn test_encrypted_wal_replay_and_volatile_session_storage() {
        let db_path = "./test_manta_wal_v4.db";
        let _ = fs::remove_file(db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path));
        let _ = fs::remove_file(format!("{}.key", db_path));

        {
            let mut store = MantaStorageEngine::new(1024 * 1024, 4 * 1024 * 1024);
            store.set_backing_file(db_path);

            let origin = "https://manta.dev/workspace";
            assert!(store.set_local_storage(origin, "theme", "obsidian"));
            assert!(store.set_session_storage(origin, "volatile_tab", "only_in_ram"));
            assert!(store.save_form_draft(origin, "/workspace::input::title", "Manta v3.0"));
            assert!(store.set_cookie_from_header(origin, "auth=token_xyz; Path=/; Secure"));
            assert!(store.put_idb_object(origin, "cache", "k1", "{\"v\":1}"));

            let wal_bytes = fs::read(format!("{}.wal", db_path)).unwrap();
            assert!(!wal_bytes.windows(9).any(|w| w == b"token_xyz"));
            assert!(!wal_bytes.windows(8).any(|w| w == b"obsidian"));
        }

        let mut restored = MantaStorageEngine::new(1024 * 1024, 4 * 1024 * 1024);
        restored.set_backing_file(db_path);
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
            restored.get_form_draft("https://manta.dev", "/workspace::input::title"),
            Some("Manta v3.0".to_string())
        );
        assert_eq!(
            restored.get_cookie_header_for_url("https://manta.dev", true),
            "auth=token_xyz"
        );

        restored.clear_all();
        assert!(!std::path::Path::new(db_path).exists());
        assert!(!std::path::Path::new(&format!("{}.wal", db_path)).exists());
        assert!(!std::path::Path::new(&format!("{}.key", db_path)).exists());
    }
}
