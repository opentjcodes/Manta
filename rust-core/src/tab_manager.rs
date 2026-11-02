use std::collections::HashMap;

use crate::clipboard::ClipboardSandbox;
use crate::css_engine::MicroCssEngine;
use crate::dom_arena::ZeroCopyDomArena;
use crate::html_tokenizer::StreamingHtmlTokenizer;
use crate::js_vm::MantaJsSandbox;
use crate::module_bus::ModuleBus;
use crate::ram_vault::VolatileRamVault;
use crate::render_list::DisplayListFrame;
use crate::site_isolation::OriginKey;
use crate::storage_engine::MantaStorageEngine;

pub type TabId = u32;

/// Isolated per-tab sandbox bound to a canonical `OriginKey`.
/// Consumes only a few kilobytes of RAM and supports instant background hibernation.
pub struct TabSandbox {
    pub id: TabId,
    pub current_url: String,
    pub current_origin: OriginKey,
    pub is_hibernated: bool,
    pub tokenizer: StreamingHtmlTokenizer,
    pub dom: ZeroCopyDomArena,
    pub css: MicroCssEngine,
    pub js_vm: MantaJsSandbox,
}

impl TabSandbox {
    pub fn new(id: TabId, max_ram_bytes: usize) -> Self {
        Self {
            id,
            current_url: String::from("manta://start"),
            current_origin: OriginKey::from_url("manta://start"),
            is_hibernated: false,
            tokenizer: StreamingHtmlTokenizer::new(),
            dom: ZeroCopyDomArena::new(max_ram_bytes),
            css: MicroCssEngine::new(),
            js_vm: MantaJsSandbox::new(50_000),
        }
    }

    /// Hibernates a background tab's DOM/CSS/JS arenas so it consumes < 1 KB of RAM while inactive.
    pub fn hibernate(&mut self) {
        self.dom.clear_and_shrink();
        self.css.clear();
        self.js_vm.reset();
        self.is_hibernated = true;
    }

    /// Releases all memory held by this tab on close.
    pub fn release_all(&mut self, vault: &mut VolatileRamVault) {
        self.dom.clear_and_shrink();
        self.css.clear();
        self.js_vm.reset();
        vault.purge_tab(self.id);
        self.current_url.clear();
        self.current_url.shrink_to_fit();
    }
}

/// Multiplexes all open tabs with strict origin boundary enforcement and background hibernation.
pub struct TabManager {
    tabs: HashMap<TabId, TabSandbox>,
    active_tab_id: Option<TabId>,
    next_id: TabId,
    per_tab_ram_ceiling: usize,
}

impl TabManager {
    pub fn new(per_tab_ram_ceiling: usize) -> Self {
        Self {
            tabs: HashMap::new(),
            active_tab_id: None,
            next_id: 1,
            per_tab_ram_ceiling,
        }
    }

    #[inline]
    pub fn active_tab_id(&self) -> Option<TabId> {
        self.active_tab_id
    }

    pub fn create_tab(&mut self) -> TabId {
        let id = self.next_id;
        self.next_id += 1;
        let sandbox = TabSandbox::new(id, self.per_tab_ram_ceiling);
        self.tabs.insert(id, sandbox);
        self.active_tab_id = Some(id);
        id
    }

    pub fn switch_to_tab(&mut self, target_id: TabId, _vault: &mut VolatileRamVault) -> bool {
        if !self.tabs.contains_key(&target_id) {
            return false;
        }
        if let Some(prev_id) = self.active_tab_id {
            if prev_id != target_id {
                if let Some(prev_tab) = self.tabs.get_mut(&prev_id) {
                    prev_tab.hibernate();
                }
            }
        }
        if let Some(active) = self.tabs.get_mut(&target_id) {
            active.is_hibernated = false;
        }
        self.active_tab_id = Some(target_id);
        true
    }

    pub fn close_tab(&mut self, tab_id: TabId, vault: &mut VolatileRamVault) -> bool {
        if let Some(mut removed) = self.tabs.remove(&tab_id) {
            removed.release_all(vault);
            if self.active_tab_id == Some(tab_id) {
                self.active_tab_id = self.tabs.keys().next().copied();
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
    /// with strict per-origin isolation and custom storage bindings.
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
    ) -> Result<DisplayListFrame, &'static str> {
        let tab = self.tabs.get_mut(&tab_id).ok_or("Invalid TabId")?;
        let next_origin = OriginKey::from_url(url);

        if tab.current_url != url {
            // If crossing origins, reset the JS VM & DOM arena completely (Strict Site Isolation!)
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
            for stylesheet in &tab.dom.collected_styles {
                tab.css.parse_stylesheet(stylesheet);
            }

            let scripts = tab.dom.collected_scripts.clone();
            let mut temp_clipboard = ClipboardSandbox::new(4096);
            for mut script in scripts {
                if modules.dispatch_before_script(url, &mut script) {
                    let _ = tab.js_vm.execute_script_with_storage(
                        &script,
                        url,
                        &mut tab.dom,
                        &mut temp_clipboard,
                        storage,
                    );
                }
            }
            modules.dispatch_dom_ready(tab_id, url);
        }

        Ok(DisplayListFrame::build(&tab.dom, &tab.css, 390.0))
    }

    pub fn total_allocated_bytes(&self) -> usize {
        self.tabs.values().map(|t| t.dom.used_bytes()).sum()
    }
}
