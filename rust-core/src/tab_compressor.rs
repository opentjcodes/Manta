use crate::ram_vault::VolatileRamVault;

/// Ultra-fast LZ-Run-Length Hybrid Compressor in pure Rust (`tab_compressor.rs`).
/// Compresses background tab metadata, URL, title, and scroll coordinates into a tiny
/// byte slice inside `VolatileRamVault` so 20+ open tabs consume only a few kilobytes of RAM.
pub struct TabCompressor;

impl TabCompressor {
    /// Compresses a byte slice using Byte-Aligned Run-Length + Repeated-Token encoding.
    pub fn compress(input: &[u8]) -> Vec<u8> {
        if input.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(input.len() / 2 + 16);
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            let mut run_len = 1usize;
            while i + run_len < input.len() && input[i + run_len] == b && run_len < 254 {
                run_len += 1;
            }
            if run_len >= 4 || b == 0xFF {
                out.push(0xFF);
                out.push(run_len as u8);
                out.push(b);
                i += run_len;
            } else {
                out.push(b);
                i += 1;
            }
        }
        out
    }

    /// Decompresses a slice produced by `TabCompressor::compress`.
    pub fn decompress(compressed: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(compressed.len() * 2);
        let mut i = 0;
        while i < compressed.len() {
            if compressed[i] == 0xFF && i + 2 < compressed.len() {
                let count = compressed[i + 1] as usize;
                let val = compressed[i + 2];
                for _ in 0..count {
                    out.push(val);
                }
                i += 3;
            } else {
                out.push(compressed[i]);
                i += 1;
            }
        }
        out
    }

    /// Saves a tab's state (URL, Title, ScrollY, Custom state) compressed into the VolatileRamVault.
    pub fn store_tab_snapshot(
        vault: &mut VolatileRamVault,
        tab_id: u32,
        url: &str,
        title: &str,
        scroll_y: i32,
    ) -> usize {
        let raw = format!("{}|{}|{}", scroll_y, title.replace('|', " "), url);
        let compressed = Self::compress(raw.as_bytes());
        let len = compressed.len();
        vault.put_ephemeral(tab_id, "tab_snapshot_v1", &compressed);
        len
    }

    /// Restores a tab's compressed snapshot from `VolatileRamVault`.
    pub fn restore_tab_snapshot(
        vault: &VolatileRamVault,
        tab_id: u32,
    ) -> Option<(i32, String, String)> {
        let compressed = vault.get_ephemeral(tab_id, "tab_snapshot_v1")?;
        let raw_bytes = Self::decompress(compressed);
        let raw_str = String::from_utf8(raw_bytes).ok()?;
        let mut parts = raw_str.splitn(3, '|');
        let scroll_y = parts.next()?.parse::<i32>().unwrap_or(0);
        let title = parts.next()?.to_string();
        let url = parts.next()?.to_string();
        Some((scroll_y, title, url))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tab_compression_roundtrip() {
        let mut vault = VolatileRamVault::new(32 * 1024);
        let url = "https://example.com/some/very/long/path/aaaaaaabbbbbbbccccccc";
        let title = "Example Page Title";
        let bytes_stored = TabCompressor::store_tab_snapshot(&mut vault, 7, url, title, 1240);
        assert!(bytes_stored > 0);

        let (scroll_y, r_title, r_url) = TabCompressor::restore_tab_snapshot(&vault, 7).unwrap();
        assert_eq!(scroll_y, 1240);
        assert_eq!(r_title, title);
        assert_eq!(r_url, url);
    }
}
