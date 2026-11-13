use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use crate::cpp_bridge::secure_zero_slice;
use crate::module_bus::ModuleBus;
use crate::net_stream::StreamingNetworkPipe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStatus {
    InProgress,
    Paused,
    Completed,
    Failed,
}

impl DownloadStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            DownloadStatus::InProgress => "in_progress",
            DownloadStatus::Paused => "paused",
            DownloadStatus::Completed => "completed",
            DownloadStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadItem {
    pub id: u32,
    pub url: String,
    pub filename: String,
    pub mime_type: String,
    pub saved_disk_path: Option<String>,
    pub bytes_received: usize,
    pub total_bytes_expected: Option<usize>,
    pub crc32_checksum: u32,
    pub status: DownloadStatus,
    /// Only retained when no disk `download_dir` is configured (capped at 4 MB to avoid OOM).
    pub in_memory_payload: Vec<u8>,
}

const MAX_IN_MEMORY_FALLBACK_BYTES: usize = 4 * 1024 * 1024;

/// **Manta Streaming Download & File Save Manager (`downloads_engine.rs`)**:
/// - Streams chunks directly to disk with rolling CRC32 without holding 1 GB files in RAM.
/// - Never permanently retains payloads in memory once written to user's download directory.
/// - Supports lock-free decoupling via `allocate_pending_id` + `finish_external_download`.
pub struct MantaDownloadManager {
    download_dir: String,
    max_single_file_bytes: usize,
    items: HashMap<u32, DownloadItem>,
    next_id: u32,
}

impl MantaDownloadManager {
    pub fn new(download_dir: &str, max_single_file_bytes: usize) -> Self {
        Self {
            download_dir: download_dir.trim().to_string(),
            max_single_file_bytes,
            items: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn download_dir(&self) -> &str {
        &self.download_dir
    }

    pub fn max_single_file_bytes(&self) -> usize {
        self.max_single_file_bytes
    }

    pub fn set_download_directory(&mut self, dir: &str) {
        self.download_dir = dir.trim().to_string();
    }

    pub fn register_pending(&mut self, url: &str, suggested_filename: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        let clean_name = sanitize_download_filename(suggested_filename, url);
        self.items.insert(
            id,
            DownloadItem {
                id,
                url: url.to_string(),
                filename: clean_name,
                mime_type: "application/octet-stream".to_string(),
                saved_disk_path: None,
                bytes_received: 0,
                total_bytes_expected: None,
                crc32_checksum: 0,
                status: DownloadStatus::InProgress,
                in_memory_payload: Vec::new(),
            },
        );
        id
    }

    pub fn finalize_pending(&mut self, id: u32, completed_item: DownloadItem) {
        self.items.insert(id, completed_item);
    }

    pub fn mark_failed(&mut self, id: u32) {
        if let Some(item) = self.items.get_mut(&id) {
            item.status = DownloadStatus::Failed;
        }
    }

    pub fn save_direct_bytes(
        &mut self,
        source_url: &str,
        suggested_filename: &str,
        mime_type: &str,
        data: &[u8],
    ) -> Result<u32, &'static str> {
        if data.len() > self.max_single_file_bytes {
            return Err("File exceeds maximum allowed download size");
        }
        let id = self.next_id;
        self.next_id += 1;

        let clean_name = sanitize_download_filename(suggested_filename, source_url);
        let crc = crc32_update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF;
        let disk_path = self.try_write_to_disk(&clean_name, data);

        // Zero RAM footprint once persisted directly to user's download path!
        let mem_payload = if disk_path.is_some() || data.len() > MAX_IN_MEMORY_FALLBACK_BYTES {
            Vec::new()
        } else {
            data.to_vec()
        };

        self.items.insert(
            id,
            DownloadItem {
                id,
                url: source_url.to_string(),
                filename: clean_name,
                mime_type: if mime_type.is_empty() {
                    "application/octet-stream".to_string()
                } else {
                    mime_type.to_string()
                },
                saved_disk_path: disk_path,
                bytes_received: data.len(),
                total_bytes_expected: Some(data.len()),
                crc32_checksum: crc,
                status: DownloadStatus::Completed,
                in_memory_payload: mem_payload,
            },
        );
        Ok(id)
    }

    /// Streams a download directly to disk chunk-by-chunk (constant ~4 KB RAM usage!).
    pub fn stream_download_standalone(
        id: u32,
        download_dir: &str,
        max_bytes: usize,
        url: &str,
        suggested_filename: &str,
        modules: &ModuleBus,
    ) -> Result<DownloadItem, String> {
        if let Some((mime, decoded)) = decode_data_uri(url) {
            if decoded.len() > max_bytes {
                return Err("File exceeds max download size".to_string());
            }
            let clean_name = sanitize_download_filename(suggested_filename, url);
            let crc = crc32_update(0xFFFF_FFFF, &decoded) ^ 0xFFFF_FFFF;
            let disk_path = write_bytes_to_dir(download_dir, &clean_name, &decoded);
            let mem = if disk_path.is_some() || decoded.len() > MAX_IN_MEMORY_FALLBACK_BYTES {
                Vec::new()
            } else {
                decoded.clone()
            };
            return Ok(DownloadItem {
                id,
                url: url.to_string(),
                filename: clean_name,
                mime_type: mime,
                saved_disk_path: disk_path,
                bytes_received: decoded.len(),
                total_bytes_expected: Some(decoded.len()),
                crc32_checksum: crc,
                status: DownloadStatus::Completed,
                in_memory_payload: mem,
            });
        }

        let initial_name = sanitize_download_filename(suggested_filename, url);
        let mut disk_file: Option<(String, File)> = if !download_dir.is_empty() {
            let dir_path = PathBuf::from(download_dir);
            let _ = fs::create_dir_all(&dir_path);
            let part_path = dir_path.join(format!("{}.{}.part", initial_name, id));
            File::create(&part_path)
                .ok()
                .map(|f| (part_path.to_string_lossy().into_owned(), f))
        } else {
            None
        };

        let mut mem_accum = Vec::new();
        let mut crc_state: u32 = 0xFFFF_FFFF;
        let mut total_received = 0usize;

        let outcome = StreamingNetworkPipe::stream_http(
            "GET",
            url,
            HashMap::new(),
            modules,
            max_bytes,
            |chunk, _is_eof| {
                if !chunk.is_empty() {
                    crc_state = crc32_update(crc_state, chunk);
                    total_received += chunk.len();
                    if let Some((_, ref mut f)) = disk_file {
                        let _ = f.write_all(chunk);
                    } else if mem_accum.len() + chunk.len() <= MAX_IN_MEMORY_FALLBACK_BYTES {
                        mem_accum.extend_from_slice(chunk);
                    }
                }
            },
        )?;

        let disposition_name = outcome
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Content-Disposition"))
            .and_then(|(_, v)| extract_filename_from_content_disposition(v));
        let final_name = sanitize_download_filename(
            disposition_name.as_deref().unwrap_or(suggested_filename),
            &outcome.final_url,
        );
        let mime = outcome
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Content-Type"))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        let saved_disk_path = if let Some((part_path, mut f)) = disk_file {
            let _ = f.sync_all();
            drop(f);
            let final_path = PathBuf::from(download_dir).join(&final_name);
            if fs::rename(&part_path, &final_path).is_ok() {
                Some(final_path.to_string_lossy().into_owned())
            } else {
                Some(part_path)
            }
        } else {
            None
        };

        Ok(DownloadItem {
            id,
            url: outcome.final_url,
            filename: final_name,
            mime_type: mime,
            saved_disk_path,
            bytes_received: total_received,
            total_bytes_expected: Some(total_received),
            crc32_checksum: !crc_state,
            status: DownloadStatus::Completed,
            in_memory_payload: mem_accum,
        })
    }

    pub fn start_or_resume_download(
        &mut self,
        url: &str,
        suggested_filename: &str,
        modules: &ModuleBus,
    ) -> Result<u32, String> {
        let id = self.next_id;
        self.next_id += 1;
        let item = Self::stream_download_standalone(
            id,
            &self.download_dir,
            self.max_single_file_bytes,
            url,
            suggested_filename,
            modules,
        )?;
        self.items.insert(id, item);
        Ok(id)
    }

    pub fn get_download(&self, id: u32) -> Option<&DownloadItem> {
        self.items.get(&id)
    }

    pub fn delete_download(&mut self, id: u32, delete_disk_file: bool) -> bool {
        if let Some(mut item) = self.items.remove(&id) {
            secure_zero_slice(&mut item.in_memory_payload);
            item.in_memory_payload.clear();
            if delete_disk_file {
                if let Some(path) = item.saved_disk_path {
                    let _ = fs::remove_file(path);
                }
            }
            return true;
        }
        false
    }

    pub fn clear_all(&mut self, delete_disk_files: bool) {
        let ids: Vec<u32> = self.items.keys().copied().collect();
        for id in ids {
            self.delete_download(id, delete_disk_files);
        }
    }

    pub fn export_downloads_json(&self) -> String {
        let mut list: Vec<&DownloadItem> = self.items.values().collect();
        list.sort_by_key(|d| d.id);
        let mut out = String::from("[");
        for (idx, d) in list.iter().enumerate() {
            if idx > 0 {
                out.push(',');
            }
            out.push_str(&format!(
                concat!(
                    "{{",
                    "\"id\":{},",
                    "\"filename\":\"{}\",",
                    "\"url\":\"{}\",",
                    "\"mime_type\":\"{}\",",
                    "\"bytes\":{},",
                    "\"crc32\":\"{:08X}\",",
                    "\"status\":\"{}\",",
                    "\"disk_path\":\"{}\"",
                    "}}"
                ),
                d.id,
                escape_json(&d.filename),
                escape_json(&d.url),
                escape_json(&d.mime_type),
                d.bytes_received,
                d.crc32_checksum,
                d.status.as_str(),
                escape_json(d.saved_disk_path.as_deref().unwrap_or("")),
            ));
        }
        out.push(']');
        out
    }

    fn try_write_to_disk(&self, filename: &str, data: &[u8]) -> Option<String> {
        write_bytes_to_dir(&self.download_dir, filename, data)
    }
}

fn write_bytes_to_dir(dir: &str, filename: &str, data: &[u8]) -> Option<String> {
    if dir.is_empty() {
        return None;
    }
    let dir_path = PathBuf::from(dir);
    let _ = fs::create_dir_all(&dir_path);
    let full_path = dir_path.join(filename);
    let mut file = File::create(&full_path).ok()?;
    file.write_all(data).ok()?;
    let _ = file.sync_all();
    Some(full_path.to_string_lossy().into_owned())
}

pub fn sanitize_download_filename(suggested: &str, fallback_url: &str) -> String {
    let raw = if !suggested.trim().is_empty() {
        suggested.trim()
    } else {
        fallback_url
            .split('?')
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("download.bin")
    };

    let basename = raw
        .rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .unwrap_or("manta_download.bin");

    let cleaned: String = basename
        .chars()
        .filter(|&c| {
            c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == ' '
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        "manta_download.bin".to_string()
    } else {
        trimmed.to_string()
    }
}

fn extract_filename_from_content_disposition(header_val: &str) -> Option<String> {
    for part in header_val.split(';') {
        let trimmed = part.trim();
        if let Some(name) = trimmed
            .strip_prefix("filename=")
            .or_else(|| trimmed.strip_prefix("FILENAME="))
        {
            return Some(name.trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

fn decode_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta
        .split(';')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("text/plain")
        .to_string();
    if meta.ends_with(";base64") {
        Some((mime, decode_base64_clean(payload)))
    } else {
        Some((mime, payload.as_bytes().to_vec()))
    }
}

fn decode_base64_clean(input: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u8;
    for b in input.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    out
}

fn crc32_update(mut crc: u32, bytes: &[u8]) -> u32 {
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
    crc
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_manager_data_uri_and_path_sanitization() {
        let mut mgr = MantaDownloadManager::new("", 10 * 1024 * 1024);
        let modules = ModuleBus::new();

        let id = mgr
            .start_or_resume_download(
                "data:text/plain;base64,SGVsbG8gTWFudGE=",
                "../../evil/../notes.txt",
                &modules,
            )
            .unwrap();

        let item = mgr.get_download(id).unwrap();
        assert_eq!(item.filename, "notes.txt");
        assert_eq!(item.in_memory_payload, b"Hello Manta");
        assert_eq!(item.status, DownloadStatus::Completed);

        assert!(mgr.delete_download(id, true));
        assert!(mgr.get_download(id).is_none());
    }
}
