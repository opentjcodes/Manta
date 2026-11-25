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
pub mod extensions;
pub mod ffi_bridge;
pub mod html_tokenizer;
pub mod input_forms;
pub mod jni_bridge;
pub mod js;
pub mod json_lite;
pub mod kernel_scripts;
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

pub use js as js_vm;

use std::sync::{Mutex, OnceLock};

use crate::browser_controls::MantaControlsManager;
use crate::clipboard::ClipboardSandbox;
use crate::config::MantaConfig;
use crate::downloads_engine::MantaDownloadManager;
use crate::extensions::ExtensionHost;
use crate::media_stream::MantaMediaManager;
use crate::module_bus::ModuleBus;
use crate::net_stream::clear_keep_alive_pool;
use crate::ram_vault::VolatileRamVault;
use crate::site_isolation::{SiteIsolationEngine, SiteIsolationModule};
use crate::storage_engine::MantaStorageEngine;
use crate::tab_manager::TabManager;
use crate::worker_pool::MantaWorkerPool;

pub struct MantaEngine {
    pub config: MantaConfig,
    pub vault: VolatileRamVault,
    pub storage: MantaStorageEngine,
    pub downloads: MantaDownloadManager,
    pub media: MantaMediaManager,
    pub extensions: ExtensionHost,
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

        let mut extensions = ExtensionHost::new();
        let mut modules = ModuleBus::new();
        modules.register(Box::new(SiteIsolationModule::new()));
        modules.register(extensions.create_net_module());

        let mut storage = MantaStorageEngine::new(
            config.per_origin_storage_quota_bytes,
            config.total_custom_storage_ceiling_bytes,
        );
        storage.ephemeral_only = config.ephemeral_zero_trace_mode;

        if !config.ephemeral_zero_trace_mode && !config.user_data_directory.trim().is_empty() {
            storage.set_storage_dir(&config.user_data_directory);
            let _ = storage.load_from_backing_file();
            extensions.set_storage_root(&config.user_data_directory);
        }

        let downloads = MantaDownloadManager::new(
            &config.download_directory_path,
            config.max_single_download_bytes,
        );
        let media = MantaMediaManager::new(config.media_ring_buffer_bytes);

        let tabs = TabManager::new_with_budgets(
            per_tab_limit,
            vault_limit,
            config.warm_background_tab_count,
            config.frame_layout_budget_us,
            config.js_max_fuel_instructions,
            config.js_time_budget_us,
        );

        Self {
            config,
            vault: VolatileRamVault::new(vault_limit),
            storage,
            downloads,
            media,
            extensions,
            isolation: SiteIsolationEngine::new(),
            controls: MantaControlsManager::new(),
            tabs,
            modules,
            clipboard: ClipboardSandbox::new(256 * 1024),
            workers: MantaWorkerPool::new(0),
        }
    }

    pub fn global() -> &'static Mutex<MantaEngine> {
        ENGINE_INSTANCE.get_or_init(|| Mutex::new(MantaEngine::new(MantaConfig::default())))
    }

    pub fn init_with_paths(&mut self, user_data_dir: &str, download_dir: &str) -> u32 {
        let resolved_storage_dir = if !user_data_dir.trim().is_empty() {
            user_data_dir.trim().to_string()
        } else if !self.config.user_data_directory.trim().is_empty() {
            self.config.user_data_directory.trim().to_string()
        } else {
            String::from("./manta_user_data")
        };

        self.config.user_data_directory = resolved_storage_dir;
        if !self.storage.ephemeral_only {
            self.storage
                .set_storage_dir(&self.config.user_data_directory);
            let _ = self.storage.load_from_backing_file();
            self.extensions
                .set_storage_root(&self.config.user_data_directory);
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
        let sdir = self
            .storage
            .storage_dir()
            .unwrap_or_default()
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        format!(
            "{{\"tab_arena_bytes\":{},\"ram_vault_bytes\":{},\"storage_ram_bytes\":{},\"open_tabs\":{},\"compacted_tabs\":{},\"active_streams\":{},\"installed_extensions\":{},\"layout_budget_us\":{},\"js_budget_us\":{},\"storage_dir\":\"{}\"}}",
            self.tabs.total_allocated_bytes(),
            self.vault.used_bytes(),
            self.storage.used_bytes(),
            self.tabs.open_tab_count(),
            self.tabs.compacted_tab_count(),
            self.media.active_stream_count(),
            self.extensions.installed_count(),
            self.config.frame_layout_budget_us,
            self.config.js_time_budget_us,
            sdir
        )
    }

    pub fn clear_all_custom_storage_and_ram(&mut self) {
        clear_keep_alive_pool();
        self.tabs.close_all();
        self.media.clear_all();
        self.extensions.clear_all();
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
    fn test_full_manta_v40_engine_tiered_tabs_firefox_ext_and_device_files() {
        let test_dir = "./test_manta_v40_user_data";
        let ext_dir = "./test_manta_v40_ext";
        let _ = std::fs::remove_dir_all(test_dir);
        let _ = std::fs::remove_dir_all(ext_dir);
        std::fs::create_dir_all(ext_dir).unwrap();

        std::fs::write(
            format!("{}/manifest.json", ext_dir),
            r#"{
                "manifest_version": 3,
                "name": "Manta Enhancer",
                "version": "1.0",
                "browser_specific_settings": { "gecko": { "id": "enhancer@manta.dev" } },
                "content_scripts": [{
                    "matches": ["*://*.manta.dev/*"],
                    "js": ["patch.js"],
                    "run_at": "document_end"
                }]
            }"#,
        )
        .unwrap();
        std::fs::write(
            format!("{}/patch.js", ext_dir),
            "let el = document.getElementById('ext_slot'); if (el) { el.textContent = browser.runtime.id; }",
        )
        .unwrap();

        let mut cfg = MantaConfig::default();
        cfg.user_data_directory = String::from(test_dir);
        let mut engine = MantaEngine::new(cfg);
        let ext_id = engine.extensions.install_from_dir(ext_dir).unwrap();
        assert_eq!(ext_id, "enhancer@manta.dev");

        let mut tab_ids = Vec::new();
        for i in 0..20 {
            let tid = engine.tabs.create_tab();
            tab_ids.push(tid);
            let html = format!(
                "<html><head><title>Tab {}</title></head><body><h1 id=\"hdr\">Active {}</h1><div id=\"ext_slot\"></div><input id=\"field\" name=\"field\" value=\"val_{}\" /><script>localStorage.setItem('k_{}', 'v_{}'); sessionStorage.setItem('s_{}', 'sv_{}');</script></body></html>",
                i, i, i, i, i, i, i
            );
            let engine_ref = &mut engine;
            engine_ref
                .tabs
                .load_stream_chunk_full(
                    tid,
                    &format!("https://site{}.manta.dev/app", i),
                    html.as_bytes(),
                    true,
                    &[],
                    &[],
                    &mut engine_ref.modules,
                    &mut engine_ref.vault,
                    &mut engine_ref.storage,
                    &mut engine_ref.downloads,
                    Some(&engine_ref.extensions),
                )
                .unwrap();
        }

        // Verify optional Firefox extension ran seamlessly on matching origin
        let slot_id = engine
            .tabs
            .get_tab(tab_ids[0])
            .unwrap()
            .dom
            .find_by_id("ext_slot")
            .unwrap();
        assert_eq!(
            engine
                .tabs
                .get_tab(tab_ids[0])
                .unwrap()
                .dom
                .text_content(slot_id),
            "enhancer@manta.dev"
        );

        assert!(engine.tabs.compacted_tab_count() >= 17);
        for (i, &tid) in tab_ids.iter().enumerate() {
            assert!(engine.tabs.switch_to_tab(tid, &mut engine.vault));
            let t = engine.tabs.get_tab(tid).unwrap();
            assert!(!t.is_hibernated);
            let hdr_id = t.dom.find_by_id("hdr").unwrap();
            assert_eq!(t.dom.text_content(hdr_id), format!("Active {}", i));
        }

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

        // Restart engine from user device folder and verify full transparent read-back
        let mut cfg2 = MantaConfig::default();
        cfg2.user_data_directory = String::from(test_dir);
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
        assert!(!std::path::Path::new(test_dir).exists());
        let _ = std::fs::remove_dir_all(ext_dir);
    }
}
