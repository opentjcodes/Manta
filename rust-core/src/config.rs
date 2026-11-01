/// Central configuration for Manta Core.
/// Tuned for minimal battery usage, strict KB-level memory budgets,
/// developer CORS freedom, and complete anti-tracking isolation.
#[derive(Debug, Clone)]
pub struct MantaConfig {
    /// Maximum total bytes across the volatile RAM vault (Default: 4 MB).
    pub global_ram_ceiling_bytes: usize,
    /// Maximum bytes allowed per active tab arena (Default: 512 KB).
    pub per_tab_ram_ceiling_bytes: usize,
    /// Maximum JS VM instruction fuel per script execution (prevents CPU/battery drain).
    pub js_max_fuel_instructions: u32,
    /// Developer mode: automatically bypass CORS restrictions & synthesize OPTIONS preflights.
    pub dev_cors_permissive: bool,
    /// Spoof JS automation/bot probes (`navigator.webdriver = false`, canvas noise, etc.).
    pub block_fingerprinting: bool,
    /// Strip `utm_*`, `fbclid`, `gclid`, and tracking query parameters from all URLs.
    pub strip_tracking_params: bool,
    /// Never store or send HTTP `Cookie` or `Set-Cookie` headers.
    pub strict_zero_cookies: bool,
    /// Bottom dock active tab index (0 = Home, 1 = Search, 2 = Library).
    pub active_dock_section: u8,
}

impl Default for MantaConfig {
    fn default() -> Self {
        Self {
            global_ram_ceiling_bytes: 4 * 1024 * 1024, // 4 MB total ceiling
            per_tab_ram_ceiling_bytes: 512 * 1024,     // 512 KB per tab
            js_max_fuel_instructions: 25_000,          // Fast, battery-safe execution cap
            dev_cors_permissive: true,                 // Enabled by default for site testing
            block_fingerprinting: true,
            strip_tracking_params: true,
            strict_zero_cookies: true,
            active_dock_section: 0,
        }
    }
}
