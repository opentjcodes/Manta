/// Central configuration for Manta Core.
/// Tuned for stock-Chrome web compatibility, strict KB-level memory budgets,
/// strict per-origin site isolation, zero disk cache bloat, and custom clearable user storage.
#[derive(Debug, Clone)]
pub struct MantaConfig {
    /// Maximum total bytes across the transient in-memory RAM pool (Default: 4 MB).
    pub global_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per active tab DOM/render arena (Default: 512 KB).
    pub per_tab_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per origin inside Manta's custom storage engine (Default: 256 KB).
    pub per_origin_storage_quota_bytes: usize,
    /// Maximum total bytes across Manta's custom clearable storage container (Default: 8 MB).
    pub total_custom_storage_ceiling_bytes: usize,
    /// Maximum bytecode VM instruction fuel per script execution (prevents CPU/battery drain).
    pub js_max_fuel_instructions: u32,
    /// Maximum JS call-stack depth.
    pub js_max_call_depth: usize,
    /// Enforce strict RFC 6454 Same-Origin Policy & Private Network Access (SSRF) protection.
    pub strict_site_isolation: bool,
    /// Never write HTTP/network/media disk caches (100% zero-cache guarantee).
    pub zero_disk_cache: bool,
    /// Path to Manta's single clearable custom binary storage container.
    pub custom_storage_file_path: String,
}

impl Default for MantaConfig {
    fn default() -> Self {
        Self {
            global_ram_ceiling_bytes: 4 * 1024 * 1024,       // 4 MB transient pool ceiling
            per_tab_ram_ceiling_bytes: 512 * 1024,           // 512 KB per active tab arena
            per_origin_storage_quota_bytes: 256 * 1024,      // 256 KB per origin storage quota
            total_custom_storage_ceiling_bytes: 8 * 1024 * 1024, // 8 MB custom storage ceiling
            js_max_fuel_instructions: 50_000,                // Fast, battery-safe bytecode execution cap
            js_max_call_depth: 64,
            strict_site_isolation: true,
            zero_disk_cache: true,
            custom_storage_file_path: String::new(),
        }
    }
}
