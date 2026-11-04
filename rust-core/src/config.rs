/// Central configuration for **Manta Core v2.0**.
///
/// Designed for maximum browsing velocity, full W3C/HTML5 capability (forms, inputs,
/// file uploads, resumable downloads, relational + IndexedDB + Virtual File System storage),
/// hot-active multi-tab responsiveness, and zero un-clearable disk bloat.
#[derive(Debug, Clone)]
pub struct MantaConfig {
    /// Maximum total bytes across the transient RAM pool before cold-compressing oldest background tabs (Default: 16 MB).
    pub global_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per active tab DOM/render arena (Default: 2 MB).
    pub per_tab_ram_ceiling_bytes: usize,
    /// Maximum number of simultaneously hot-active tabs kept uncompressed in RAM (Default: 16 tabs).
    pub max_hot_active_tabs: usize,
    /// Maximum bytes allowed per origin inside Manta's custom storage & VFS engine (Default: 16 MB).
    pub per_origin_storage_quota_bytes: usize,
    /// Maximum total bytes across Manta's custom clearable storage container (Default: 128 MB).
    pub total_custom_storage_ceiling_bytes: usize,
    /// Maximum size of a single downloaded file in bytes (Default: 512 MB).
    pub max_single_download_bytes: usize,
    /// Maximum bytecode VM instruction fuel per script execution (Default: 250,000 instructions).
    pub js_max_fuel_instructions: u32,
    /// Maximum JS call-stack depth (Default: 128 frames).
    pub js_max_call_depth: usize,
    /// Enforce strict RFC 6454 Same-Origin Policy & Private Network Access (SSRF) protection.
    pub strict_site_isolation: bool,
    /// Never write un-clearable OS HTTP/shader caches (100% zero-ghost-cache guarantee).
    pub zero_disk_cache: bool,
    /// Path to Manta's single clearable custom binary storage & database container (`manta_vault.db`).
    pub custom_storage_file_path: String,
    /// Directory path where user-initiated file downloads are saved.
    pub download_directory_path: String,
}

impl Default for MantaConfig {
    fn default() -> Self {
        Self {
            global_ram_ceiling_bytes: 16 * 1024 * 1024,          // 16 MB global hot pool
            per_tab_ram_ceiling_bytes: 2 * 1024 * 1024,          // 2 MB per active tab arena
            max_hot_active_tabs: 16,                             // Keep up to 16 tabs hot in RAM
            per_origin_storage_quota_bytes: 16 * 1024 * 1024,    // 16 MB per origin (DB + VFS + KV)
            total_custom_storage_ceiling_bytes: 128 * 1024 * 1024, // 128 MB total clearable container
            max_single_download_bytes: 512 * 1024 * 1024,        // 512 MB per download stream
            js_max_fuel_instructions: 250_000,
            js_max_call_depth: 128,
            strict_site_isolation: true,
            zero_disk_cache: true,
            custom_storage_file_path: String::new(),
            download_directory_path: String::new(),
        }
    }
}
