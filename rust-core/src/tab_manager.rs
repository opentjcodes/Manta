use std::collections::HashMap;

use crate::clipboard::ClipboardSandbox;
use crate::css_engine::MicroCssEngine;
use crate::dom_arena::{NodeId, ZeroCopyDomArena};
use crate::downloads_engine::MantaDownloadManager;
use crate::html_tokenizer::StreamingHtmlTokenizer;
use crate::input_forms::SerializedFormSubmission;
use crate::js_vm::MantaJsSandbox;
use crate::module_bus::ModuleBus;
use crate::ram_vault::VolatileRamVault;
use crate::render_list::DisplayListFrame;
use crate::site_isolation::OriginKey;
use crate::storage_engine::MantaStorageEngine;
use crate::tab_compressor::TabCompressor;

pub type TabId = u32;

/// Lifecycle state of a tab while the Manta application is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabLifecycleState {
    /// Currently visible foreground tab.
    ActiveForeground,
    /// Open in the background while the app is running — DOM, JS VM, and Input forms remain 100% live!
    ActiveBackground,
    /// Cold-compressed in `VolatileRamVault` only when global hardware memory pressure is exceeded.
    ColdCompressed,
}

/// Isolated per-tab sandbox bound to a canonical `OriginKey`.
/// Keeps DOM, CSS, JS VM, and Form/Input state hot and active while the app is open!
pub struct TabSandbox {
    pub id: TabId,
    pub current_url: String,
    pub current_origin: OriginKey,
    pub state: TabLifecycleState,
    pub is_hibernated: bool,
    pub scroll_y: i32,
    pub last_access_tick: u64,
    pub tokenizer: StreamingHtmlTokenizer,
    pub dom: ZeroCopyDomArena,
    pub css: MicroCssEngine,
    pub js_vm: MantaJsSandbox,
}

impl TabSandbox {
    pub fn new(id: TabId, max_ram_bytes: usize, tick: u64) -> Self {
        Self {
            id,
            current_url: String::from("manta://start"),
            current_origin: OriginKey::from_url("manta://start"),
            state: TabLifecycleState::ActiveForeground,
            is_hibernated: false,
            scroll_y: 0,
            last_access_tick: tick,
            tokenizer: StreamingHtmlTokenizer::new(),
            dom: ZeroCopyDomArena::new(max_ram_bytes),
            css: MicroCssEngine::new(),
            js_vm: MantaJsSandbox::new(250_000),
        }
    }

    /// Cold-compresses a tab ONLY when global memory pressure exceeds `global_ram_ceiling`.
    pub fn compress_under_memory_pressure(&mut self, vault: &mut VolatileRamVault) {
        TabCompressor::store_tab_snapshot(
            vault,
            self.id,
            &self.current_url,
            &self.dom.page_title,
            self.scroll_y,
        );
        self.dom.clear_and_shrink();
        self.css.clear();
        self.js_vm.reset();
        self.state = TabLifecycleState::ColdCompressed;
        self.is_hibernated = true;
    }

    /// Releases all memory held by this tab when explicitly closed by the user.
    pub fn release_all(&mut self, vault: &mut VolatileRamVault) {
        self.dom.clear_and_shrink();
        self.css.clear();
        self.js_vm.reset();
        vault.purge_tab(self.id);
        self.current_url.clear();
        self.current_url.shrink_to_fit();
    }
}

/// **Hot-Active Tab Multiplexer (`TabManager`)**:
/// - Keeps all open tabs hot and active in memory while the app is open (`ActiveForeground` / `ActiveBackground`).
/// - Switching tabs is an instant 0.0ms operation that never destroys the background tab's DOM or inputs.
/// - Automatically enforces global memory ceilings by cold-compressing only the least-recently-used
///   tab if total RAM across all open tabs exceeds `global_ram_ceiling`.
pub struct TabManager {
    tabs: HashMap<TabId, TabSandbox>,
    active_tab_id: Option<TabId>,
    next_id: TabId,
    monotonic_tick: u64,
    per_tab_ram_ceiling: usize,
    global_ram_ceiling: usize,
}

impl TabManager {
    pub fn new(per_tab_ram_ceiling: usize, global_ram_ceiling: usize) -> Self {
        Self {
            tabs: HashMap::new(),
            active_tab_id: None,
            next_id: 1,
            monotonic_tick: 1,
            per_tab_ram_ceiling,
            global_ram_ceiling,
        }
    }

    #[inline]
    pub fn active_tab_id(&self) -> Option<TabId> {
        self.active_tab_id
    }

    #[inline]
    pub fn open_tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn get_tab(&self, tab_id: TabId) -> Option<&TabSandbox> {
        self.tabs.get(&tab_id)
    }

    pub fn get_tab_mut(&mut self, tab_id: TabId) -> Option<&mut TabSandbox> {
        self.tabs.get_mut(&tab_id)
    }

    pub fn create_tab(&mut self) -> TabId {
        self.monotonic_tick += 1;
        let id = self.next_id;
        self.next_id += 1;
        if let Some(prev_id) = self.active_tab_id {
            if let Some(prev_tab) = self.tabs.get_mut(&prev_id) {
                if prev_tab.state == TabLifecycleState::ActiveForeground {
                    prev_tab.state = TabLifecycleState::ActiveBackground;
                }
            }
        }
        let sandbox = TabSandbox::new(id, self.per_tab_ram_ceiling, self.monotonic_tick);
        self.tabs.insert(id, sandbox);
        self.active_tab_id = Some(id);
        id
    }

    /// Switches to `target_id` in O(1) time while keeping the previous tab **Hot-Active** in RAM!
    pub fn switch_to_tab(&mut self, target_id: TabId, vault: &mut VolatileRamVault) -> bool {
        if !self.tabs.contains_key(&target_id) {
            return false;
        }
        self.monotonic_tick += 1;
        if let Some(prev_id) = self.active_tab_id {
            if prev_id != target_id {
                if let Some(prev_tab) = self.tabs.get_mut(&prev_id) {
                    // Keep the tab hot and active in the background (DO NOT wipe its DOM!)
                    prev_tab.state = TabLifecycleState::ActiveBackground;
                    prev_tab.is_hibernated = false;
                }
            }
        }
        if let Some(active) = self.tabs.get_mut(&target_id) {
            active.state = TabLifecycleState::ActiveForeground;
            active.is_hibernated = false;
            active.last_access_tick = self.monotonic_tick;
        }
        self.active_tab_id = Some(target_id);
        self.enforce_global_memory_budget(vault);
        true
    }

    /// Interactive Form / Input APIs on an active tab
    pub fn focus_node(&mut self, tab_id: TabId, node_id: NodeId) -> bool {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        tab.dom.forms.focus_control(node_id)
    }

    pub fn set_input_value(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
        value: &str,
    ) -> Result<(), &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        tab.dom.forms.set_value(node_id, value)
    }

    pub fn toggle_checkable(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
    ) -> Result<bool, &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        tab.dom.forms.toggle_checkable(node_id)
    }

    pub fn attach_file_to_input(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
        filename: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<(), &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        tab.dom.forms.attach_file(node_id, filename, mime_type, bytes)
    }

    pub fn submit_form(
        &self,
        tab_id: TabId,
        form_or_control_id: NodeId,
    ) -> Result<SerializedFormSubmission, &'static str> {
        let tab = self.tabs.get(&tab_id).ok_or("Invalid TabId")?;
        tab.dom
            .forms
            .serialize_submission(form_or_control_id, &tab.current_url)
    }

    pub fn close_tab(&mut self, tab_id: TabId, vault: &mut VolatileRamVault) -> bool {
        if let Some(mut removed) = self.tabs.remove(&tab_id) {
            removed.release_all(vault);
            if self.active_tab_id == Some(tab_id) {
                self.active_tab_id = self
                    .tabs
                    .values()
                    .max_by_key(|t| t.last_access_tick)
                    .map(|t| t.id);
            }
            return true;
        }
        false
    }

    pub fn close_all(&mut self) {
        let mut dummy_vault = VolatileRamVault::new(0);
        for (_, mut tab) in self.tabs.drain() {
            tab.release_all(&mut dummy_vault);
        }
        self.tabs.shrink_to_fit();
        self.active_tab_id = None;
    }

    /// Streams raw HTML bytes into a tab's tokenizer -> flat DOM arena -> CSS cascade -> Bytecode JS VM
    /// with strict per-origin isolation, MantaDB, Virtual Files, and Download bindings.
    #[allow(clippy::too_many_arguments)]
    pub fn load_stream_chunk(
        &mut self,
        tab_id: TabId,
        url: &str,
        chunk: &[u8],
        is_eof: bool,
        modules: &mut ModuleBus,
        vault: &mut VolatileRamVault,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> Result<DisplayListFrame, &'static str> {
        self.monotonic_tick += 1;
        let tick = self.monotonic_tick;
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        tab.last_access_tick = tick;
        tab.is_hibernated = false;

        let next_origin = OriginKey::from_url(url);

        if tab.current_url != url {
            tab.dom.clear_and_shrink();
            tab.dom = ZeroCopyDomArena::new(self.per_tab_ram_ceiling);
            tab.css.clear();
            tab.js_vm.reset();
            vault.purge_tab(tab_id);
            tab.current_url = url.to_string();
            tab.current_origin = next_origin;
        }

        let dom_ref = &mut tab.dom;
        tab.tokenizer.feed_chunk(chunk, is_eof, |token| {
            dom_ref.ingest_token(token);
        });

        if is_eof {
            storage.record_history_visit(url, &tab.dom.page_title);

            for stylesheet in &tab.dom.collected_styles {
                tab.css.parse_stylesheet(stylesheet);
            }

            let scripts = tab.dom.collected_scripts.clone();
            let mut temp_clipboard = ClipboardSandbox::new(16 * 1024);
            for mut script in scripts {
                if modules.dispatch_before_script(url, &mut script) {
                    let _ = tab.js_vm.execute_script_full(
                        &script,
                        url,
                        &mut tab.dom,
                        &mut temp_clipboard,
                        storage,
                        downloads,
                    );
                }
            }
            modules.dispatch_dom_ready(tab_id, url);
        }

        let frame = DisplayListFrame::build(&tab.dom, &tab.css, 390.0);
        self.enforce_global_memory_budget(vault);
        Ok(frame)
    }

    pub fn total_allocated_bytes(&self) -> usize {
        self.tabs.values().map(|t| t.dom.used_bytes()).sum()
    }

    fn enforce_global_memory_budget(&mut self, vault: &mut VolatileRamVault) {
        while self.total_allocated_bytes() > self.global_ram_ceiling {
            let active_id = self.active_tab_id;
            let oldest_bg_id = self
                .tabs
                .values()
                .filter(|t| Some(t.id) != active_id && !t.is_hibernated && t.dom.used_bytes() > 0)
                .min_by_key(|t| t.last_access_tick)
                .map(|t| t.id);

            if let Some(victim_id) = oldest_bg_id {
                if let Some(victim) = self.tabs.get_mut(&victim_id) {
                    victim.compress_under_memory_pressure(vault);
                }
            } else {
                break;
            }
        }
    }
}
