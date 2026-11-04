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

/// Global singleton holding the **Manta Core v2.0** Multi-Core Rust Browser Engine.
///
/// Built with a Linux-kernel-inspired bolt-on architecture:
/// - **Hot-Active Tab Multiplexer (`tabs`)**: Keeps all open tabs active and responsive in RAM
///   while the app is open, only compressing cold tabs when global memory pressure demands it.
/// - **MantaDB & Clearable Storage (`storage`)**: Unified relational B-Tree tables, IndexedDB
///   object stores, Virtual File System (OPFS), Cookies, Bookmarks, and History stored in a
///   single zero-ghost-page binary container that can be inspected and permanently cleared.
/// - **Interactive Input & Form Engine (`input_forms`)**: Full HTML5 `<input>`, `<textarea>`,
///   `<select>`, file-upload, and `GET`/`POST` (`urlencoded` & `multipart`) form submission.
/// - **Resumable Download & File Save Manager (`downloads`)**: Saves files, streams HTTP
///   downloads with `Range` resume support, and verifies CRC32 integrity.
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

        let storage = MantaStorageEngine::new(
            config.per_origin_storage_quota_bytes,
            config.total_custom_storage_ceiling_bytes,
        );

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
        if !custom_storage_path.trim().is_empty() {
            self.config.custom_storage_file_path = custom_storage_path.trim().to_string();
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

    /// Backwards-compatible initializer accepting just the storage file path.
    pub fn init_with_storage_path(&mut self, custom_storage_path: &str) -> u32 {
        self.init_with_paths(custom_storage_path, "")
    }

    /// Releases transient in-memory render caches without deleting user-saved MantaDB or VFS files.
    pub fn release_transient_ram_caches(&mut self) {
        self.vault.clear_all();
    }

    /// Permanently shreds all custom storage (MantaDB tables, IndexedDB, Virtual Files, Cookies,
    /// localStorage, History, Bookmarks), closes all tabs, and zeroes volatile RAM.
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
    fn test_full_manta_v2_engine_hot_tabs_inputs_db_and_downloads() {
        let mut engine = MantaEngine::new(MantaConfig::default());
        let tab1 = engine.tabs.create_tab();
        let tab2 = engine.tabs.create_tab();

        // Load interactive page with form, input, and script in Tab 1
        let html1 = br#"
            <html>
              <head><title>Manta Workspace</title></head>
              <body>
                <h1 id="banner">Ready</h1>
                <form id="login-form" action="https://example.com/submit" method="POST">
                  <input id="user-input" name="username" type="text" value="alice" />
                  <input id="remember-chk" name="remember" type="checkbox" checked />
                </form>
                <script>
                  function computeScore(base, bonus) {
                      return base + bonus;
                  }
                  let total = computeScore(80, 20);
                  document.getElementById("banner").textContent = "Score: " + total;
                  localStorage.setItem("last_score", "" + total);
                  MantaFS.writeFile("/notes/todo.txt", "Ship Manta Core v2.0");
                </script>
              </body>
            </html>
        "#;

        let engine_ref = &mut engine;
        let frame1 = engine_ref
            .tabs
            .load_stream_chunk(
                tab1,
                "https://example.com/workspace",
                html1,
                true,
                &mut engine_ref.modules,
                &mut engine_ref.vault,
                &mut engine_ref.storage,
                &mut engine_ref.downloads,
            )
            .expect("tab1 stream should succeed");

        assert_eq!(frame1.page_title, "Manta Workspace");

        // Switch to Tab 2 and back to Tab 1 -> Verify Tab 1 stayed HOT-ACTIVE in RAM!
        assert!(engine.tabs.switch_to_tab(tab2, &mut engine.vault));
        assert!(engine.tabs.switch_to_tab(tab1, &mut engine.vault));

        let tab1_sandbox = engine.tabs.get_tab(tab1).unwrap();
        assert!(!tab1_sandbox.is_hibernated);
        let banner_id = tab1_sandbox.dom.find_by_id("banner").unwrap();
        assert_eq!(tab1_sandbox.dom.text_content(banner_id), "Score: 100");

        // Modify input value and submit form
        let input_id = tab1_sandbox.dom.find_by_id("user-input").unwrap();
        engine
            .tabs
            .set_input_value(tab1, input_id, "manta_admin")
            .unwrap();
        let submission = engine.tabs.submit_form(tab1, input_id).unwrap();
        assert_eq!(submission.action_url, "https://example.com/submit");
        assert_eq!(submission.method, "POST");
        assert!(String::from_utf8_lossy(&submission.body).contains("username=manta_admin"));

        // Verify Virtual File System saved the file and can be inspected & cleared
        let saved_note = engine
            .storage
            .db
            .read_virtual_file("https://example.com", "/notes/todo.txt")
            .unwrap();
        assert_eq!(saved_note.data, b"Ship Manta Core v2.0");

        let manifest = engine.storage.inspect_storage_manifest_json();
        assert!(manifest.contains("/notes/todo.txt"));

        engine.clear_all_custom_storage_and_ram();
        assert_eq!(engine.storage.used_bytes(), 0);
    }
}
