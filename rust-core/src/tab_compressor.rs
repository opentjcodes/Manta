use crate::ram_vault::VolatileRamVault;

/// Ultra-fast LZ-Run-Length Hybrid Compressor in pure Rust (`tab_compressor.rs`).
/// Used when total open tab memory exceeds the global ceiling, or when saving fast
/// restoration snapshots inside `VolatileRamVault`.
pub struct TabCompressor;

impl TabCompressor {
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
        vault.put_ephemeral(tab_id, "tab_snapshot_v2", &compressed);
        len
    }

    pub fn restore_tab_snapshot(
        vault: &VolatileRamVault,
        tab_id: u32,
    ) -> Option<(i32, String, String)> {
        let compressed = vault.get_ephemeral(tab_id, "tab_snapshot_v2")?;
        let raw_bytes = Self::decompress(compressed);
        let raw_str = String::from_utf8(raw_bytes).ok()?;
        let mut parts = raw_str.splitn(3, '|');
        let scroll_y = parts.next()?.parse::<i32>().unwrap_or(0);
        let title = parts.next()?.to_string();
        let url = parts.next()?.to_string();
        Some((scroll_y, title, url))
    }
}
