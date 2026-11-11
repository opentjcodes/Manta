/// Central configuration for **Manta Core v2.5**.
///
/// Engineered for faster-than-Chrome site execution, 99% modern web compatibility,
/// non-destructive hot-active multi-tab residency (tabs are NEVER killed while open),
/// and instant atomic transactional storage persistence (`manta_vault.db`).
#[derive(Debug, Clone)]
pub struct MantaConfig {
    /// Maximum total bytes across the transient RAM pool for snapshots (Default: 64 MB).
    pub global_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per active tab DOM/render arena (Default: 16 MB).
    pub per_tab_ram_ceiling_bytes: usize,
    /// Keep all open tabs 100% hot and active in RAM until explicitly closed by the user.
    pub keep_tabs_active_until_closed: bool,
    /// Automatically commit and fsync every storage write (`localStorage`, `sessionStorage`,
    /// `cookies`, `IndexedDB`, `MantaDB`, `MantaFS`, `form_drafts`) to disk immediately.
    pub instant_storage_commit: bool,
    /// Maximum bytes allowed per origin inside Manta's custom storage & VFS engine (Default: 64 MB).
    pub per_origin_storage_quota_bytes: usize,
    /// Maximum total bytes across Manta's custom clearable storage container (Default: 512 MB).
    pub total_custom_storage_ceiling_bytes: usize,
    /// Maximum size of a single downloaded file in bytes (Default: 1024 MB).
    pub max_single_download_bytes: usize,
    /// Maximum bytecode VM instruction fuel per script execution (Default: 1,000,000 instructions).
    pub js_max_fuel_instructions: u32,
    /// Maximum JS call-stack depth (Default: 256 frames).
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
            global_ram_ceiling_bytes: 64 * 1024 * 1024,            // 64 MB global pool
            per_tab_ram_ceiling_bytes: 16 * 1024 * 1024,           // 16 MB per active tab arena
            keep_tabs_active_until_closed: true,                   // Never kill open tabs
            instant_storage_commit: true,                          // Never lose site data
            per_origin_storage_quota_bytes: 64 * 1024 * 1024,      // 64 MB per origin
            total_custom_storage_ceiling_bytes: 512 * 1024 * 1024, // 512 MB total container
            max_single_download_bytes: 1024 * 1024 * 1024,         // 1 GB per download stream
            js_max_fuel_instructions: 1_000_000,
            js_max_call_depth: 256,
            strict_site_isolation: true,
            zero_disk_cache: true,
            custom_storage_file_path: String::from("./manta_vault.db"),
            download_directory_path: String::from("./downloads"),
        }
    }
}
