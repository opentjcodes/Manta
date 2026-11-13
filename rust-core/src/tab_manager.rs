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

/// 3-Tier Non-Destructive Active Tab Lifecycle:
/// Open tabs are **NEVER killed or deleted**; background tabs automatically trim GPU DisplayLists
/// and compress cold state snapshots via C++ SIMD LZ+RLE to cut device RAM by 3x–8x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabLifecycleState {
    ActiveForeground,
    ActiveBackgroundWarm,
    ActiveBackgroundCompacted,
}

/// Isolated per-tab sandbox bound to a canonical `OriginKey`.
pub struct TabSandbox {
    pub id: TabId,
    pub current_url: String,
    pub current_origin: OriginKey,
    pub state: TabLifecycleState,
    pub is_hibernated: bool,
    pub scroll_y: i32,
    pub last_access_tick: u64,
    pub stream_in_progress: bool,
    pub tokenizer: StreamingHtmlTokenizer,
    pub dom: ZeroCopyDomArena,
    pub css: MicroCssEngine,
    pub js_vm: MantaJsSandbox,
    pub cached_frame: Option<DisplayListFrame>,
    pub cached_frame_gen: u64,
    pub cached_frame_scroll: i32,
    pub cached_frame_width: f32,
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
            stream_in_progress: false,
            tokenizer: StreamingHtmlTokenizer::new(),
            dom: ZeroCopyDomArena::new(max_ram_bytes),
            css: MicroCssEngine::new(),
            js_vm: MantaJsSandbox::new(1_000_000),
            cached_frame: None,
            cached_frame_gen: 0,
            cached_frame_scroll: -1,
            cached_frame_width: 0.0,
        }
    }

    pub fn invalidate_frame_cache(&mut self) {
        self.cached_frame = None;
        self.cached_frame_gen = 0;
    }

    /// Reclaims transient render & OS slab pages on background compaction without destroying live DOM/inputs.
    pub fn compact_background_memory(&mut self, vault: &mut VolatileRamVault) {
        self.state = TabLifecycleState::ActiveBackgroundCompacted;
        self.is_hibernated = false;
        self.cached_frame = None;
        self.cached_frame_gen = 0;
        self.tokenizer.shrink_buffers();
        self.dom.release_transient_sources_and_compact();
        TabCompressor::store_tab_snapshot(
            vault,
            self.id,
            &self.current_url,
            &self.dom.page_title,
            self.scroll_y,
        );
    }

    pub fn release_all(&mut self, vault: &mut VolatileRamVault) {
        self.cached_frame = None;
        self.dom.clear_and_shrink();
        self.css.clear();
        self.js_vm.reset();
        vault.purge_tab(self.id);
        self.current_url.clear();
        self.current_url.shrink_to_fit();
    }
}

/// **3-Tier Non-Destructive Hot-Active Tab Multiplexer (`TabManager`)**:
/// - Keeps all open tabs active and instantaneously restorable (never killed while open!).
/// - Keeps foreground + top 3 warm background tabs ready, while compacting older background tabs
///   with C++ LZ+RLE + OS `madvise(MADV_DONTNEED)` and evicting offscreen GPU DisplayLists.
/// - Caches culled `DisplayListFrame`s with dirty generation tracking to cut CPU/GPU recomputations.
pub struct TabManager {
    tabs: HashMap<TabId, TabSandbox>,
    active_tab_id: Option<TabId>,
    next_id: TabId,
    monotonic_tick: u64,
    per_tab_ram_ceiling: usize,
    #[allow(dead_code)]
    global_ram_ceiling: usize,
    warm_background_limit: usize,
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
            warm_background_limit: 3,
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
                prev_tab.state = TabLifecycleState::ActiveBackgroundWarm;
                prev_tab.is_hibernated = false;
                prev_tab.cached_frame = None;
            }
        }
        let sandbox = TabSandbox::new(id, self.per_tab_ram_ceiling, self.monotonic_tick);
        self.tabs.insert(id, sandbox);
        self.active_tab_id = Some(id);
        id
    }

    /// Switches to `target_id` and enforces the 3-tier memory policy on older background tabs.
    pub fn switch_to_tab(&mut self, target_id: TabId, vault: &mut VolatileRamVault) -> bool {
        if !self.tabs.contains_key(&target_id) {
            return false;
        }
        self.monotonic_tick += 1;
        if let Some(prev_id) = self.active_tab_id {
            if prev_id != target_id {
                if let Some(prev_tab) = self.tabs.get_mut(&prev_id) {
                    prev_tab.state = TabLifecycleState::ActiveBackgroundWarm;
                    prev_tab.is_hibernated = false;
                    prev_tab.cached_frame = None;
                }
            }
        }
        if let Some(active) = self.tabs.get_mut(&target_id) {
            active.state = TabLifecycleState::ActiveForeground;
            active.is_hibernated = false;
            active.last_access_tick = self.monotonic_tick;
        }
        self.active_tab_id = Some(target_id);
        self.enforce_background_tiering(vault);
        true
    }

    fn enforce_background_tiering(&mut self, vault: &mut VolatileRamVault) {
        let active_id = self.active_tab_id;
        let mut bg_tabs: Vec<(TabId, u64)> = self
            .tabs
            .values()
            .filter(|t| Some(t.id) != active_id)
            .map(|t| (t.id, t.last_access_tick))
            .collect();

        if bg_tabs.len() <= self.warm_background_limit {
            return;
        }

        // Most recently accessed first
        bg_tabs.sort_by(|a, b| b.1.cmp(&a.1));
        for (idx, (tid, _)) in bg_tabs.into_iter().enumerate() {
            if idx >= self.warm_background_limit {
                if let Some(tab) = self.tabs.get_mut(&tid) {
                    if tab.state != TabLifecycleState::ActiveBackgroundCompacted {
                        tab.compact_background_memory(vault);
                    }
                }
            }
        }
    }

    pub fn focus_node(&mut self, tab_id: TabId, node_id: NodeId) -> bool {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        let ok = tab.dom.forms.focus_control(node_id);
        if ok {
            tab.invalidate_frame_cache();
        }
        ok
    }

    pub fn set_input_value(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
        value: &str,
    ) -> Result<(), &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        let res = tab.dom.forms.set_value(node_id, value);
        if res.is_ok() {
            tab.invalidate_frame_cache();
        }
        res
    }

    pub fn set_input_value_and_persist(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
        value: &str,
        storage: &mut MantaStorageEngine,
    ) -> Result<(), &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        tab.dom.forms.set_value(node_id, value)?;
        tab.invalidate_frame_cache();
        if let Some(ctrl) = tab.dom.forms.get_control(node_id) {
            let key = if !ctrl.name.is_empty() {
                ctrl.name.clone()
            } else {
                format!("node_{}", node_id)
            };
            storage.save_form_draft(&tab.current_url, &key, value);
        }
        Ok(())
    }

    pub fn toggle_checkable(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
    ) -> Result<bool, &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        let res = tab.dom.forms.toggle_checkable(node_id);
        if res.is_ok() {
            tab.invalidate_frame_cache();
        }
        res
    }

    pub fn dispatch_node_event(
        &mut self,
        tab_id: TabId,
        node_id: NodeId,
        event_type: &str,
        storage: &mut MantaStorageEngine,
        downloads: &mut MantaDownloadManager,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(&tab_id) else {
            return false;
        };
        let mut fired = false;
        let path = tab.dom.event_bubbling_path(node_id);
        let mut clip = ClipboardSandbox::new(64 * 1024);
        for target in path {
            let listeners = tab.dom.get_event_listeners(target, event_type);
            for code in listeners {
                if !code.starts_with("__manta_fn:") {
                    let _ = tab.js_vm.execute_script_full(
                        &code,
                        &tab.current_url,
                        &mut tab.dom,
                        &mut clip,
                        storage,
                        downloads,
                    );
                    fired = true;
                }
            }
        }
        if fired {
            tab.invalidate_frame_cache();
        }
        fired
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
        let res = tab.dom.forms.attach_file(node_id, filename, mime_type, bytes);
        if res.is_ok() {
            tab.invalidate_frame_cache();
        }
        res
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

    pub fn render_viewport_frame(
        &mut self,
        tab_id: TabId,
        viewport_w: f32,
        scroll_y: f32,
        viewport_h: f32,
    ) -> Option<DisplayListFrame> {
        let tab = self.tabs.get_mut(&tab_id)?;
        tab.scroll_y = scroll_y as i32;
        tab.css.viewport_width = viewport_w;

        let scroll_i = scroll_y as i32;
        if tab.cached_frame_gen == tab.dom.generation
            && (tab.cached_frame_scroll - scroll_i).abs() < 64
            && (tab.cached_frame_width - viewport_w).abs() < 1.0
        {
            if let Some(ref frame) = tab.cached_frame {
                return Some(frame.clone());
            }
        }

        let frame = DisplayListFrame::build_culled(&tab.dom, &tab.css, viewport_w, scroll_y, viewport_h);
        tab.cached_frame = Some(frame.clone());
        tab.cached_frame_gen = tab.dom.generation;
        tab.cached_frame_scroll = scroll_i;
        tab.cached_frame_width = viewport_w;
        Some(frame)
    }

    pub fn find_in_page(&self, tab_id: TabId, query: &str) -> usize {
        self.tabs
            .get(&tab_id)
            .map(|t| t.dom.find_text_matches(query).len())
            .unwrap_or(0)
    }

    pub fn get_reader_text(&self, tab_id: TabId) -> String {
        self.tabs
            .get(&tab_id)
            .map(|t| t.dom.extract_reader_text())
            .unwrap_or_default()
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
                if let Some(new_active) = self.active_tab_id {
                    if let Some(t) = self.tabs.get_mut(&new_active) {
                        t.state = TabLifecycleState::ActiveForeground;
                        t.is_hibernated = false;
                    }
                }
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
        tab.state = TabLifecycleState::ActiveForeground;
        tab.invalidate_frame_cache();

        let next_origin = OriginKey::from_url(url);
        let url_without_hash = url.split('#').next().unwrap_or(url);
        let prev_without_hash = tab.current_url.split('#').next().unwrap_or(&tab.current_url);

        if !tab.stream_in_progress
            && (url_without_hash != prev_without_hash || tab.dom.nodes().len() <= 1)
        {
            tab.dom.clear_and_shrink();
            tab.dom = ZeroCopyDomArena::new(self.per_tab_ram_ceiling);
            tab.css.clear();
            tab.js_vm.reset();
            tab.current_origin = next_origin;
        }
        tab.current_url = url.to_string();
        tab.stream_in_progress = !is_eof;

        let dom_ref = &mut tab.dom;
        tab.tokenizer.feed_chunk(chunk, is_eof, |token| {
            dom_ref.ingest_token(token);
        });

        if is_eof {
            storage.record_history_visit(url, &tab.dom.page_title);

            let styles = std::mem::take(&mut tab.dom.collected_styles);
            for stylesheet in &styles {
                tab.css.parse_stylesheet(stylesheet);
            }
            drop(styles);

            let scripts = std::mem::take(&mut tab.dom.collected_scripts);
            let mut temp_clipboard = ClipboardSandbox::new(64 * 1024);
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

            // Reclaim raw CSS/JS sources & unused OS slab pages immediately!
            tab.dom.release_transient_sources_and_compact();
            modules.dispatch_dom_ready(tab_id, url);
        }

        let frame = DisplayListFrame::build(&tab.dom, &tab.css, 390.0);
        if is_eof {
            let tab_ref = self.tabs.get_mut(&tab_id).unwrap();
            tab_ref.cached_frame = Some(frame.clone());
            tab_ref.cached_frame_gen = tab_ref.dom.generation;
            tab_ref.cached_frame_width = 390.0;
            self.enforce_background_tiering(vault);
        }
        Ok(frame)
    }

    pub fn total_allocated_bytes(&self) -> usize {
        self.tabs.values().map(|t| t.dom.used_bytes()).sum()
    }

    pub fn compacted_tab_count(&self) -> usize {
        self.tabs
            .values()
            .filter(|t| t.state == TabLifecycleState::ActiveBackgroundCompacted)
            .count()
    }
}
