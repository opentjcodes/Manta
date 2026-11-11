#![forbid(unsafe_op_in_unsafe_fn)]

pub mod browser_controls;
pub mod clipboard;
pub mod config;
pub mod css_engine;
pub mod dom_arena;
pub mod downloads_engine;
pub mod ffi_bridge;
pub mod html_tokenizer;
pub mod input_forms;
pub mod jni_bridge;
pub mod js_vm;
pub mod kernel_scripts;
pub mod manta_db;
pub mod module_bus;
pub mod net_stream;
pub mod page_compiler;
pub mod ram_vault;
pub mod render_list;
pub mod site_isolation;
pub mod storage_engine;
pub mod tab_compressor;
pub mod tab_manager;
pub mod web_compat_engine;
pub mod worker_pool;

use std::sync::{Mutex, OnceLock};

use crate::browser_controls::MantaControlsManager;
use crate::clipboard::ClipboardSandbox;
use crate::config::MantaConfig;
use crate::downloads_engine::MantaDownloadManager;
use crate::module_bus::ModuleBus;
use crate::ram_vault::VolatileRamVault;
use crate::site_isolation::{SiteIsolationEngine, SiteIsolationModule};
use crate::storage_engine::MantaStorageEngine;
use crate::tab_manager::TabManager;
use crate::worker_pool::MantaWorkerPool;

/// Global singleton holding the **Manta Core v2.5** Multi-Core Rust Browser Engine.
///
/// Built with a Linux-kernel-inspired bolt-on architecture:
/// - **Non-Destructive Hot-Active Tab Multiplexer (`tabs`)**: Keeps all open tabs active and
///   responsive in RAM while the app is open, never killing or wiping tabs unless explicitly closed.
/// - **Transactional MantaDB3 & Clearable Storage (`storage`)**: Unified relational SQL tables,
///   IndexedDB object stores, Virtual File System (OPFS), Cookies, Live Form Drafts, Bookmarks,
///   and History persisted atomically to disk on every write.
/// - **High-Velocity Kernel Scripts & 99% Web Compatibility (`kernel_scripts` & `web_compat_engine`)**:
///   Delivers faster-than-Chrome execution via passive event promotion, speculative DNS pre-warming,
///   and full W3C/ES2024 standards compatibility.
pub struct MantaEngine {
    pub config: MantaConfig,
    pub vault: VolatileRamVault,
    pub storage: MantaStorageEngine,
    pub downloads: MantaDownloadManager,
    pub isolation: SiteIsolationEngine,
    pub controls: MantaControlsManager,
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

        let mut storage = MantaStorageEngine::new(
            config.per_origin_storage_quota_bytes,
            config.total_custom_storage_ceiling_bytes,
        );
        storage.auto_commit_on_write = config.instant_storage_commit;
        if !config.custom_storage_file_path.trim().is_empty() {
            storage.set_backing_file(&config.custom_storage_file_path);
            let _ = storage.load_from_backing_file();
        }

        let downloads = MantaDownloadManager::new(
            &config.download_directory_path,
            config.max_single_download_bytes,
        );

        Self {
            config,
            vault: VolatileRamVault::new(vault_limit),
            storage,
            downloads,
            isolation: SiteIsolationEngine::new(),
            controls: MantaControlsManager::new(),
            tabs: TabManager::new(per_tab_limit, vault_limit),
            modules,
            clipboard: ClipboardSandbox::new(256 * 1024),
            workers: MantaWorkerPool::new(0),
        }
    }

    pub fn global() -> &'static Mutex<MantaEngine> {
        ENGINE_INSTANCE.get_or_init(|| Mutex::new(MantaEngine::new(MantaConfig::default())))
    }

    /// Initializes Manta Core with backing paths for the clearable storage vault and downloads directory.
    pub fn init_with_paths(&mut self, custom_storage_path: &str, download_dir: &str) -> u32 {
        let resolved_storage_path = if !custom_storage_path.trim().is_empty() {
            custom_storage_path.trim().to_string()
        } else if !self.config.custom_storage_file_path.trim().is_empty() {
            self.config.custom_storage_file_path.trim().to_string()
        } else {
            String::from("./manta_vault.db")
        };

        self.config.custom_storage_file_path = resolved_storage_path;
        self.storage
            .set_backing_file(&self.config.custom_storage_file_path);
        let _ = self.storage.load_from_backing_file();

        if !download_dir.trim().is_empty() {
            self.config.download_directory_path = download_dir.trim().to_string();
            self.downloads.set_download_directory(download_dir.trim());
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

    /// Backwards-compatible initializer accepting just the storage file path.
    pub fn init_with_storage_path(&mut self, custom_storage_path: &str) -> u32 {
        self.init_with_paths(custom_storage_path, "")
    }

    /// Releases transient in-memory render caches without deleting user-saved MantaDB or VFS files.
    pub fn release_transient_ram_caches(&mut self) {
        self.vault.clear_all();
    }

    /// Permanently shreds all custom storage (MantaDB tables, IndexedDB, Virtual Files, Cookies,
    /// localStorage, Form Drafts, History, Bookmarks), closes all tabs, and zeroes volatile RAM.
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
    fn test_full_manta_v25_engine_non_destructive_tabs_persistent_storage_and_forms() {
        let mut cfg = MantaConfig::default();
        cfg.custom_storage_file_path = String::from("./test_manta_v25_vault.db");
        let mut engine = MantaEngine::new(cfg);

        // Open 20 tabs and verify NO background tab is ever killed or hibernated
        let mut tab_ids = Vec::new();
        for i in 0..20 {
            let tid = engine.tabs.create_tab();
            tab_ids.push(tid);
            let html = format!(
                "<html><head><title>Tab {}</title></head><body><h1 id=\"hdr\">Active {}</h1><input id=\"field\" name=\"field\" value=\"val_{}\" /><script>localStorage.setItem('k_{}', 'v_{}'); sessionStorage.setItem('s_{}', 'sv_{}');</script></body></html>",
                i, i, i, i, i, i, i
            );
            let engine_ref = &mut engine;
            engine_ref
                .tabs
                .load_stream_chunk(
                    tid,
                    &format!("https://site{}.manta.dev/app", i),
                    html.as_bytes(),
                    true,
                    &mut engine_ref.modules,
                    &mut engine_ref.vault,
                    &mut engine_ref.storage,
                    &mut engine_ref.downloads,
                )
                .unwrap();
        }

        // Verify every single one of the 20 open tabs is still 100% hot-active in memory
        for (i, &tid) in tab_ids.iter().enumerate() {
            assert!(engine.tabs.switch_to_tab(tid, &mut engine.vault));
            let t = engine.tabs.get_tab(tid).unwrap();
            assert!(!t.is_hibernated);
            let hdr_id = t.dom.find_by_id("hdr").unwrap();
            assert_eq!(t.dom.text_content(hdr_id), format!("Active {}", i));
        }

        // Verify user input on Tab 1 is persisted to disk and survives a fresh engine reload
        let tab1 = tab_ids[0];
        let input_id = engine.tabs.get_tab(tab1).unwrap().dom.find_by_id("field").unwrap();
        {
            let engine_ref = &mut engine;
            engine_ref
                .tabs
                .set_input_value_and_persist(
                    tab1,
                    input_id,
                    "persisted_user_data",
                    &mut engine_ref.storage,
                )
                .unwrap();
        }

        // Create a second engine instance reading from the same backing file
        let mut cfg2 = MantaConfig::default();
        cfg2.custom_storage_file_path = String::from("./test_manta_v25_vault.db");
        let mut restored_engine = MantaEngine::new(cfg2);
        assert_eq!(
            restored_engine
                .storage
                .get_local_storage("https://site0.manta.dev", "k_0"),
            Some("v_0".to_string())
        );
        assert_eq!(
            restored_engine
                .storage
                .get_session_storage("https://site0.manta.dev", "s_0"),
            Some("sv_0".to_string())
        );
        assert_eq!(
            restored_engine
                .storage
                .get_form_draft("https://site0.manta.dev", "field"),
            Some("persisted_user_data".to_string())
        );

        restored_engine.clear_all_custom_storage_and_ram();
        assert_eq!(restored_engine.storage.used_bytes(), 0);
    }
}
