#![forbid(unsafe_op_in_unsafe_fn)]

pub mod browser_controls;
pub mod clipboard;
pub mod config;
pub mod cpp_bridge;
pub mod crypto_vault;
pub mod css_engine;
pub mod deflate;
pub mod dom_arena;
pub mod downloads_engine;
pub mod ffi_bridge;
pub mod html_tokenizer;
pub mod input_forms;
pub mod jni_bridge;
pub mod js_vm;
pub mod kernel_scripts;
pub mod manta_db;
pub mod media_stream;
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
use crate::media_stream::MantaMediaManager;
use crate::module_bus::ModuleBus;
use crate::ram_vault::VolatileRamVault;
use crate::site_isolation::{SiteIsolationEngine, SiteIsolationModule};
use crate::storage_engine::MantaStorageEngine;
use crate::tab_manager::TabManager;
use crate::worker_pool::MantaWorkerPool;

/// Global singleton holding the **Manta Core v3.0 (Default Base Flavour)** Engine.
///
/// Combines Safe Rust architecture with C++17 SIMD, `mmap`/`madvise`, lock-free SPSC media ring
/// buffers, and verified OpenSSL TLS:
/// - **3x–8x Lower Device RAM**: 48-byte interned `DomNode`s, `mmap` slab page release, DisplayList
///   eviction, and C++ Hash-LZ + RLE background tab compaction with zero tab killing.
/// - **Encrypted Append-Only WAL Storage**: `MANTADB4` + ChaCha20/HMAC-SHA256 `.wal` persistence
///   with strictly volatile RAM-only `sessionStorage` and zero permanent HTTP/media disk caching.
/// - **Low-Latency Video/Audio Streaming**: Adaptive bitrate HLS/DASH/Range over bounded volatile
///   C++ ring buffers.
pub struct MantaEngine {
    pub config: MantaConfig,
    pub vault: VolatileRamVault,
    pub storage: MantaStorageEngine,
    pub downloads: MantaDownloadManager,
    pub media: MantaMediaManager,
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
        storage.wal_compact_threshold = config.wal_compact_threshold;
        storage.ephemeral_only = config.ephemeral_zero_trace_mode;

        if !config.ephemeral_zero_trace_mode && !config.custom_storage_file_path.trim().is_empty() {
            storage.set_backing_file(&config.custom_storage_file_path);
            let _ = storage.load_from_backing_file();
        }

        let downloads = MantaDownloadManager::new(
            &config.download_directory_path,
            config.max_single_download_bytes,
        );
        let media = MantaMediaManager::new(config.media_ring_buffer_bytes);

        Self {
            config,
            vault: VolatileRamVault::new(vault_limit),
            storage,
            downloads,
            media,
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

    pub fn init_with_paths(&mut self, custom_storage_path: &str, download_dir: &str) -> u32 {
        let resolved_storage_path = if !custom_storage_path.trim().is_empty() {
            custom_storage_path.trim().to_string()
        } else if !self.config.custom_storage_file_path.trim().is_empty() {
            self.config.custom_storage_file_path.trim().to_string()
        } else {
            String::from("./manta_vault.db")
        };

        self.config.custom_storage_file_path = resolved_storage_path;
        if !self.storage.ephemeral_only {
            self.storage
                .set_backing_file(&self.config.custom_storage_file_path);
            let _ = self.storage.load_from_backing_file();
        }

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

    pub fn init_with_storage_path(&mut self, custom_storage_path: &str) -> u32 {
        self.init_with_paths(custom_storage_path, "")
    }

    pub fn release_transient_ram_caches(&mut self) {
        self.vault.clear_all();
    }

    pub fn memory_stats_json(&self) -> String {
        format!(
            "{{\"tab_arena_bytes\":{},\"ram_vault_bytes\":{},\"storage_ram_bytes\":{},\"open_tabs\":{},\"compacted_tabs\":{},\"active_streams\":{}}}",
            self.tabs.total_allocated_bytes(),
            self.vault.used_bytes(),
            self.storage.used_bytes(),
            self.tabs.open_tab_count(),
            self.tabs.compacted_tab_count(),
            self.media.active_stream_count()
        )
    }

    pub fn clear_all_custom_storage_and_ram(&mut self) {
        self.tabs.close_all();
        self.media.clear_all();
        self.vault.clear_all();
        self.storage.clear_all();
        self.controls.clear_all();
        self.clipboard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_manta_v30_engine_tiered_active_tabs_encrypted_wal_and_media() {
        let test_db = "./test_manta_v30_vault.db";
        let _ = std::fs::remove_file(test_db);
        let _ = std::fs::remove_file(format!("{}.wal", test_db));
        let _ = std::fs::remove_file(format!("{}.key", test_db));

        let mut cfg = MantaConfig::default();
        cfg.custom_storage_file_path = String::from(test_db);
        let mut engine = MantaEngine::new(cfg);

        // Open 20 tabs: older background tabs compact cleanly with C++ LZ+RLE, NEVER killed!
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

        // Background tiering compacted older tabs while keeping all 20 open and non-hibernated
        assert!(engine.tabs.compacted_tab_count() >= 15);
        for (i, &tid) in tab_ids.iter().enumerate() {
            assert!(engine.tabs.switch_to_tab(tid, &mut engine.vault));
            let t = engine.tabs.get_tab(tid).unwrap();
            assert!(!t.is_hibernated);
            let hdr_id = t.dom.find_by_id("hdr").unwrap();
            assert_eq!(t.dom.text_content(hdr_id), format!("Active {}", i));
        }

        // Persist user input on Tab 1 via encrypted WAL
        let tab1 = tab_ids[0];
        let input_id = engine
            .tabs
            .get_tab(tab1)
            .unwrap()
            .dom
            .find_by_id("field")
            .unwrap();
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
        // Session storage stays active in memory during session
        assert_eq!(
            engine
                .storage
                .get_session_storage("https://site0.manta.dev", "s_0"),
            Some("sv_0".to_string())
        );

        // Create second engine instance loading from encrypted WAL + vault key
        let mut cfg2 = MantaConfig::default();
        cfg2.custom_storage_file_path = String::from(test_db);
        let mut restored_engine = MantaEngine::new(cfg2);
        assert_eq!(
            restored_engine
                .storage
                .get_local_storage("https://site0.manta.dev", "k_0"),
            Some("v_0".to_string())
        );
        // sessionStorage strictly never written to permanent disk!
        assert_eq!(
            restored_engine
                .storage
                .get_session_storage("https://site0.manta.dev", "s_0"),
            None
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
