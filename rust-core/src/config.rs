/// Central configuration for **Manta Core v3.1**.
///
/// Engineered for ultra-low RAM footprint, deterministic fixed-latency compute pacing
/// across all sites, zero permanent HTTP/media disk caching, and transparent,
/// user-accessible device file storage (`user_data_directory`).
#[derive(Debug, Clone)]
pub struct MantaConfig {
    /// Maximum total bytes across the transient RAM pool for snapshots (Default: 12 MB).
    pub global_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per active tab DOM/render arena (Default: 4 MB).
    pub per_tab_ram_ceiling_bytes: usize,
    /// Number of recently visited background tabs kept warm before LZ+RLE compaction.
    pub warm_background_tab_count: usize,
    /// Keep all open tabs active and restorable until explicitly closed by the user.
    pub keep_tabs_active_until_closed: bool,
    /// Deterministic per-frame layout time budget in microseconds (Default: 6,000 us = 6 ms).
    pub frame_layout_budget_us: u64,
    /// Deterministic per-script JS execution time budget in microseconds (Default: 12,000 us = 12 ms).
    pub js_time_budget_us: u64,
    /// Strictly ephemeral zero-disk-write browsing mode (Default: false; toggled per user privacy).
    pub ephemeral_zero_trace_mode: bool,
    /// Maximum bytes allowed per origin on disk & RAM (Default: 32 MB).
    pub per_origin_storage_quota_bytes: usize,
    /// Maximum total bytes across Manta's user data directory (Default: 256 MB).
    pub total_custom_storage_ceiling_bytes: usize,
    /// Maximum size of a single downloaded file in bytes (Default: 1024 MB).
    pub max_single_download_bytes: usize,
    /// Default bounded volatile ring buffer size (power-of-two) for media streaming (Default: 4 MB).
    pub media_ring_buffer_bytes: u32,
    /// Maximum bytecode VM instruction fuel per script execution (Default: 500,000 instructions).
    pub js_max_fuel_instructions: u32,
    /// Maximum JS call-stack depth (Default: 256 frames).
    pub js_max_call_depth: usize,
    /// Enforce strict RFC 6454 Same-Origin Policy & Private Network Access (SSRF) protection.
    pub strict_site_isolation: bool,
    /// Never write un-clearable OS HTTP/media/shader caches (100% zero-ghost-cache guarantee).
    pub zero_disk_cache: bool,
    /// User-accessible directory where Manta stores transparent browsing data files.
    pub user_data_directory: String,
    /// Directory path where user-initiated file downloads are saved.
    pub download_directory_path: String,
}

impl Default for MantaConfig {
    fn default() -> Self {
        Self {
            global_ram_ceiling_bytes: 12 * 1024 * 1024,            // 12 MB transient pool
            per_tab_ram_ceiling_bytes: 4 * 1024 * 1024,            // 4 MB per tab arena
            warm_background_tab_count: 2,                          // 2 warm + rest LZ-compacted
            keep_tabs_active_until_closed: true,                   // Never kill open tabs
            frame_layout_budget_us: 6_000,                         // 6 ms fixed layout ceiling
            js_time_budget_us: 12_000,                             // 12 ms fixed JS slice ceiling
            ephemeral_zero_trace_mode: false,
            per_origin_storage_quota_bytes: 32 * 1024 * 1024,      // 32 MB per origin
            total_custom_storage_ceiling_bytes: 256 * 1024 * 1024, // 256 MB total user data
            max_single_download_bytes: 1024 * 1024 * 1024,         // 1 GB per download stream
            media_ring_buffer_bytes: 4 * 1024 * 1024,              // 4 MB volatile C++ SPSC ring
            js_max_fuel_instructions: 500_000,
            js_max_call_depth: 256,
            strict_site_isolation: true,
            zero_disk_cache: true,
            user_data_directory: String::from("./manta_user_data"),
            download_directory_path: String::from("./downloads"),
        }
    }
}
