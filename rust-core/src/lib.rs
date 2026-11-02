#![forbid(unsafe_op_in_unsafe_fn)]

pub mod browser_controls;
pub mod clipboard;
pub mod config;
pub mod css_engine;
pub mod dom_arena;
pub mod html_tokenizer;
pub mod jni_bridge;
pub mod js_vm;
pub mod module_bus;
pub mod net_stream;
pub mod page_compiler;
pub mod ram_vault;
pub mod render_list;
pub mod site_isolation;
pub mod storage_engine;
pub mod tab_compressor;
pub mod tab_manager;
pub mod worker_pool;

use std::sync::{Mutex, OnceLock};

use crate::browser_controls::ChromeControlsManager;
use crate::clipboard::ClipboardSandbox;
use crate::config::MantaConfig;
use crate::module_bus::ModuleBus;
use crate::ram_vault::VolatileRamVault;
use crate::site_isolation::{SiteIsolationEngine, SiteIsolationModule};
use crate::storage_engine::MantaStorageEngine;
use crate::tab_manager::TabManager;
use crate::worker_pool::MantaWorkerPool;

/// Global singleton holding the Manta Multi-Core Rust Browser Engine state.
/// Designed for stock-Chrome compatibility, strict per-origin site isolation,
/// zero disk cache bloat, and a clearable custom binary storage engine.
pub struct MantaEngine {
    pub config: MantaConfig,
    pub vault: VolatileRamVault,
    pub storage: MantaStorageEngine,
    pub isolation: SiteIsolationEngine,
    pub controls: ChromeControlsManager,
    pub tabs: TabManager,
    pub modules: ModuleBus,
    pub clipboard: ClipboardSandbox,
    pub workers: MantaWorkerPool,
}

static ENGINE_INSTANCE: OnceLock<Mutex<MantaEngine>> = OnceLock::new();

impl MantaEngine {
    pub fn new(config: MantaConfig) -> Self {
        let vault_limit = config.global_ram_ceiling_bytes;
        let per_tab_limit = config.per_tab_ram_ceiling_bytes;

        let mut modules = ModuleBus::new();
        modules.register(Box::new(SiteIsolationModule::new()));

        let storage = MantaStorageEngine::new(
            config.per_origin_storage_quota_bytes,
            config.total_custom_storage_ceiling_bytes,
        );

        Self {
            config,
            vault: VolatileRamVault::new(vault_limit),
            storage,
            isolation: SiteIsolationEngine::new(),
            controls: ChromeControlsManager::new(),
            tabs: TabManager::new(per_tab_limit),
            modules,
            clipboard: ClipboardSandbox::new(64 * 1024),
            workers: MantaWorkerPool::new(0),
        }
    }

    pub fn global() -> &'static Mutex<MantaEngine> {
        ENGINE_INSTANCE.get_or_init(|| Mutex::new(MantaEngine::new(MantaConfig::default())))
    }

    /// Initializes or reloads the custom user storage file path and returns the active tab ID.
    pub fn init_with_storage_path(&mut self, custom_storage_path: &str) -> u32 {
        if !custom_storage_path.trim().is_empty() {
            self.config.custom_storage_file_path = custom_storage_path.trim().to_string();
            self.storage.set_backing_file(&self.config.custom_storage_file_path);
            let _ = self.storage.load_from_backing_file();
        }
        self.modules.dispatch_init();
        if let Some(active) = self.tabs.active_tab_id() {
            active
        } else {
            let id = self.tabs.create_tab();
            self.controls.ensure_tab_state(id);
            id
        }
    }

    /// Releases transient in-memory render caches without deleting user-saved custom storage.
    pub fn release_transient_ram_caches(&mut self) {
        self.vault.clear_all();
    }

    /// Explicitly clears all user data in the custom storage solution, transient RAM arenas,
    /// and clipboard when requested by the user.
    pub fn clear_all_custom_storage_and_ram(&mut self) {
        self.tabs.close_all();
        self.vault.clear_all();
        self.storage.clear_all();
        self.controls.clear_all();
        self.clipboard.clear();
        let _ = self.storage.save_to_backing_file();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_lifecycle_and_custom_storage_isolation() {
        let mut engine = MantaEngine::new(MantaConfig::default());
        assert!(engine.workers.core_count() >= 1);

        let tab_id = engine.tabs.create_tab();
        let html = b"<html><head><title>Manta Chrome Parity</title></head><body><h1 id=\"hdr\">Hello</h1><script>let x = 40 + 2; document.getElementById(\"hdr\").textContent = \"Answer: \" + x;</script></body></html>";

        let frame = engine
            .tabs
            .load_stream_chunk(
                tab_id,
                "https://example.com/app",
                html,
                true,
                &mut engine.modules,
                &mut engine.vault,
                &mut engine.storage,
            )
            .expect("stream load should succeed");

        assert_eq!(frame.page_title, "Manta Chrome Parity");
        assert!(!frame.commands.is_empty());

        // Verify origin-partitioned custom storage works and persists independently of transient RAM cache
        assert!(engine.storage.set_local_storage(
            "https://example.com/app",
            "theme",
            "dark"
        ));
        assert_eq!(
            engine
                .storage
                .get_local_storage("https://example.com/other", "theme"),
            Some("dark".to_string())
        );
        assert_eq!(
            engine
                .storage
                .get_local_storage("https://another-site.org", "theme"),
            None
        );

        engine.release_transient_ram_caches();
        assert_eq!(engine.vault.used_bytes(), 0);
        // User storage remains intact after transient cache release
        assert_eq!(
            engine
                .storage
                .get_local_storage("https://example.com", "theme"),
            Some("dark".to_string())
        );

        // Explicit user storage clear wipes custom storage cleanly
        engine.clear_all_custom_storage_and_ram();
        assert_eq!(engine.storage.used_bytes(), 0);
    }
}
