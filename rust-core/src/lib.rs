#![forbid(unsafe_op_in_unsafe_fn)]

pub mod clipboard;
pub mod config;
pub mod cors_proxy;
pub mod css_engine;
pub mod dom_arena;
pub mod html_tokenizer;
pub mod jni_bridge;
pub mod js_vm;
pub mod module_bus;
pub mod net_stream;
pub mod ram_vault;
pub mod render_list;
pub mod shield;
pub mod tab_manager;

use std::sync::{Mutex, OnceLock};

use crate::clipboard::ClipboardSandbox;
use crate::config::MantaConfig;
use crate::cors_proxy::CorsDeveloperModule;
use crate::module_bus::ModuleBus;
use crate::ram_vault::VolatileRamVault;
use crate::shield::PrivacyShieldModule;
use crate::tab_manager::TabManager;

/// Global singleton holding the entire Manta Rust Engine state.
/// Protected by a lightweight lock and strictly bounded by RAM quotas.
pub struct MantaEngine {
    pub config: MantaConfig,
    pub vault: VolatileRamVault,
    pub tabs: TabManager,
    pub modules: ModuleBus,
    pub clipboard: ClipboardSandbox,
}

static ENGINE_INSTANCE: OnceLock<Mutex<MantaEngine>> = OnceLock::new();

impl MantaEngine {
    pub fn new(config: MantaConfig) -> Self {
        let vault_limit = config.global_ram_ceiling_bytes;
        let per_tab_limit = config.per_tab_ram_ceiling_bytes;

        let mut modules = ModuleBus::new();
        modules.register(Box::new(PrivacyShieldModule::new(
            config.block_fingerprinting,
            config.strip_tracking_params,
        )));
        modules.register(Box::new(CorsDeveloperModule::new(
            config.dev_cors_permissive,
        )));

        Self {
            config,
            vault: VolatileRamVault::new(vault_limit),
            tabs: TabManager::new(per_tab_limit),
            modules,
            clipboard: ClipboardSandbox::new(64 * 1024),
        }
    }

    pub fn global() -> &'static Mutex<MantaEngine> {
        ENGINE_INSTANCE.get_or_init(|| Mutex::new(MantaEngine::new(MantaConfig::default())))
    }

    /// Emergency & lifecycle purge: cryptographically zeroes all volatile RAM buffers,
    /// destroys all tab arenas, clears clipboard buffer, and resets memory counters to 0.
    pub fn purge_all_volatile_state(&mut self) {
        self.tabs.close_all_and_zeroize();
        self.vault.zeroize_and_purge();
        self.clipboard.clear_and_zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_lifecycle_and_zero_ram_purge() {
        let mut engine = MantaEngine::new(MantaConfig::default());
        let tab_id = engine.tabs.create_tab();
        let html = b"<html><head><style>body { background: #0a0a0f; color: #ffffff; }</style></head><body><h1>Manta Engine</h1><p id='msg'>Ready</p><script>let x = 42;</script></body></html>";

        let frame = engine
            .tabs
            .load_stream_chunk(
                tab_id,
                "https://manta.local/test",
                html,
                true,
                &mut engine.modules,
                &mut engine.vault,
            )
            .expect("stream load should succeed");

        assert!(!frame.commands.is_empty());
        assert!(engine.tabs.total_allocated_bytes() > 0);

        engine.purge_all_volatile_state();
        assert_eq!(engine.tabs.total_allocated_bytes(), 0);
        assert_eq!(engine.vault.used_bytes(), 0);
    }
}
