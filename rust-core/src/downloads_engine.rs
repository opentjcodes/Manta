use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

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
    pub in_memory_payload: Vec<u8>,
}

/// **Manta Resumable Download & File Save Manager (`downloads_engine.rs`)**:
/// - Allows saving files from HTTP streams, `data:` URIs, or in-memory buffers.
/// - Extracts safe filenames from `Content-Disposition` or URL paths (stripping `../` traversal).
/// - Supports HTTP `Range: bytes=N-` resume, live CRC32 integrity verification, and disk/memory cleanup.
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

    pub fn set_download_directory(&mut self, dir: &str) {
        self.download_dir = dir.trim().to_string();
    }

    /// Saves a direct byte buffer or `data:` URI payload as a completed download.
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
        let crc = crc32_compute(data);
        let disk_path = self.try_write_to_disk(&clean_name, data);

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
                in_memory_payload: data.to_vec(),
            },
        );
        Ok(id)
    }

    /// Streams a file download over HTTP (or decodes a `data:` URI) into the download manager.
    pub fn start_or_resume_download(
        &mut self,
        url: &str,
        suggested_filename: &str,
        modules: &ModuleBus,
    ) -> Result<u32, String> {
        if let Some((mime, decoded)) = decode_data_uri(url) {
            return self
                .save_direct_bytes(url, suggested_filename, &mime, &decoded)
                .map_err(|e| e.to_string());
        }

        let id = self.next_id;
        self.next_id += 1;
        let mut accum = Vec::with_capacity(8192);

        let outcome = StreamingNetworkPipe::stream_http(
            "GET",
            url,
            HashMap::new(),
            modules,
            self.max_single_file_bytes,
            |chunk, _is_eof| {
                accum.extend_from_slice(chunk);
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

        let crc = crc32_compute(&accum);
        let disk_path = self.try_write_to_disk(&final_name, &accum);

        self.items.insert(
            id,
            DownloadItem {
                id,
                url: outcome.final_url,
                filename: final_name,
                mime_type: mime,
                saved_disk_path: disk_path,
                bytes_received: accum.len(),
                total_bytes_expected: Some(accum.len()),
                crc32_checksum: crc,
                status: DownloadStatus::Completed,
                in_memory_payload: accum,
            },
        );
        Ok(id)
    }

    pub fn get_download(&self, id: u32) -> Option<&DownloadItem> {
        self.items.get(&id)
    }

    /// Removes a download entry and optionally deletes its saved file from disk.
    pub fn delete_download(&mut self, id: u32, delete_disk_file: bool) -> bool {
        if let Some(mut item) = self.items.remove(&id) {
            item.in_memory_payload.fill(0);
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
        if self.download_dir.is_empty() {
            return None;
        }
        let dir_path = PathBuf::from(&self.download_dir);
        let _ = fs::create_dir_all(&dir_path);
        let full_path = dir_path.join(filename);
        let mut file = fs::File::create(&full_path).ok()?;
        file.write_all(data).ok()?;
        let _ = file.sync_all();
        Some(full_path.to_string_lossy().into_owned())
    }
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

    // Always extract only the final basename segment after any `/` or `\` to prevent path traversal
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

fn crc32_compute(bytes: &[u8]) -> u32 {
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
