/// Central configuration for **Manta Core v3.0 (Default Base Flavour)**.
///
/// Engineered for 3x–8x lower RAM footprint, zero permanent HTTP/media disk caching,
/// encrypted append-only WAL persistence (`manta_vault.db` + `.wal`), non-destructive
/// 3-tier hot-active multi-tab residency, and low-latency media streaming.
#[derive(Debug, Clone)]
pub struct MantaConfig {
    /// Maximum total bytes across the transient RAM pool for snapshots (Default: 16 MB).
    pub global_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per active tab DOM/render arena (Default: 6 MB).
    pub per_tab_ram_ceiling_bytes: usize,
    /// Number of recently visited background tabs kept in warm DOM tier before LZ+RLE compaction.
    pub warm_background_tab_count: usize,
    /// Keep all open tabs active and restorable until explicitly closed by the user.
    pub keep_tabs_active_until_closed: bool,
    /// Flush WAL frames immediately; compact into checkpoint container every N operations.
    pub instant_storage_commit: bool,
    /// Number of WAL appends before automatic checkpoint compaction (Default: 64 ops).
    pub wal_compact_threshold: usize,
    /// Strictly ephemeral zero-disk-write browsing mode (Default: false; toggled per user privacy).
    pub ephemeral_zero_trace_mode: bool,
    /// Maximum bytes allowed per origin inside Manta's custom encrypted vault (Default: 32 MB).
    pub per_origin_storage_quota_bytes: usize,
    /// Maximum total bytes across Manta's custom clearable storage container (Default: 256 MB).
    pub total_custom_storage_ceiling_bytes: usize,
    /// Maximum size of a single downloaded file in bytes (Default: 1024 MB).
    pub max_single_download_bytes: usize,
    /// Default bounded volatile ring buffer size (power-of-two) for media streaming (Default: 4 MB).
    pub media_ring_buffer_bytes: u32,
    /// Maximum bytecode VM instruction fuel per script execution (Default: 1,000,000 instructions).
    pub js_max_fuel_instructions: u32,
    /// Maximum JS call-stack depth (Default: 256 frames).
    pub js_max_call_depth: usize,
    /// Enforce strict RFC 6454 Same-Origin Policy & Private Network Access (SSRF) protection.
    pub strict_site_isolation: bool,
    /// Never write un-clearable OS HTTP/media/shader caches (100% zero-ghost-cache guarantee).
    pub zero_disk_cache: bool,
    /// Path to Manta's single encrypted clearable binary storage container (`manta_vault.db`).
    pub custom_storage_file_path: String,
    /// Directory path where user-initiated file downloads are saved.
    pub download_directory_path: String,
}

impl Default for MantaConfig {
    fn default() -> Self {
        Self {
            global_ram_ceiling_bytes: 16 * 1024 * 1024,            // 16 MB transient pool
            per_tab_ram_ceiling_bytes: 6 * 1024 * 1024,            // 6 MB per tab arena (3x tighter)
            warm_background_tab_count: 3,                          // 3 warm + rest LZ-compacted
            keep_tabs_active_until_closed: true,                   // Never kill open tabs
            instant_storage_commit: true,                          // Append-only WAL on write
            wal_compact_threshold: 64,                             // Compact every 64 WAL entries
            ephemeral_zero_trace_mode: false,
            per_origin_storage_quota_bytes: 32 * 1024 * 1024,      // 32 MB per origin
            total_custom_storage_ceiling_bytes: 256 * 1024 * 1024, // 256 MB total container
            max_single_download_bytes: 1024 * 1024 * 1024,         // 1 GB per download stream
            media_ring_buffer_bytes: 4 * 1024 * 1024,              // 4 MB volatile C++ SPSC ring
            js_max_fuel_instructions: 1_000_000,
            js_max_call_depth: 256,
            strict_site_isolation: true,
            zero_disk_cache: true,
            custom_storage_file_path: String::from("./manta_vault.db"),
            download_directory_path: String::from("./downloads"),
        }
    }
}
