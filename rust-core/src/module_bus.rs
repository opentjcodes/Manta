use std::collections::HashMap;

/// Represents an outgoing HTTP request before it touches the socket.
#[derive(Debug, Clone)]
pub struct InterceptedRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
}

/// Represents an incoming HTTP response before it enters the streaming tokenizer.
#[derive(Debug, Clone)]
pub struct InterceptedResponse {
    pub status_code: u16,
    pub headers: HashMap<String, String>,
    pub synthetic_body: Option<Vec<u8>>,
}

/// Action decided by a Manta module during request interception.
#[derive(Debug, Clone)]
pub enum ModuleDecision {
    Proceed,
    Block { reason: &'static str },
    SynthesizeResponse(InterceptedResponse),
}

/// **Bolt-On Modular Extension Trait (`MantaFeatureModule`)**:
/// Designed like loadable Linux kernel modules (`insmod` / `modprobe`):
/// Any future repository or subsystem (WebAssembly runtime, DevTools protocol,
/// Password Manager, Media/Codec pipeline, Custom Protocol handlers) bolts onto
/// Manta Core by implementing this trait and registering with `ModuleBus`.
pub trait MantaFeatureModule: Send + Sync {
    fn id(&self) -> &'static str;
    fn is_enabled(&self) -> bool;
    fn set_enabled(&mut self, enabled: bool);
    fn on_init(&mut self) {}
    fn on_configure(&mut self, _key: &str, _value: &str) {}
    fn on_before_request(&self, _req: &mut InterceptedRequest) -> ModuleDecision {
        ModuleDecision::Proceed
    }
    fn on_response_headers(&self, _req: &InterceptedRequest, _res: &mut InterceptedResponse) {}
    fn on_before_script_eval(&self, _origin: &str, _script_source: &mut String) -> bool {
        true
    }
    fn on_dom_ready(&self, _tab_id: u32, _origin: &str) {}
    fn on_tab_closed(&mut self, _tab_id: u32) {}
}

/// Registry and lifecycle dispatcher for all bolt-on Manta modules.
pub struct ModuleBus {
    modules: Vec<Box<dyn MantaFeatureModule>>,
}

impl Default for ModuleBus {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleBus {
    pub fn new() -> Self {
        Self {
            modules: Vec::with_capacity(8),
        }
    }

    pub fn register(&mut self, module: Box<dyn MantaFeatureModule>) {
        self.modules.push(module);
    }

    pub fn dispatch_init(&mut self) {
        for m in &mut self.modules {
            if m.is_enabled() {
                m.on_init();
            }
        }
    }

    pub fn configure_module(&mut self, id: &str, key: &str, value: &str) -> bool {
        for m in &mut self.modules {
            if m.id() == id {
                m.on_configure(key, value);
                return true;
            }
        }
        false
    }

    pub fn set_module_enabled(&mut self, id: &str, enabled: bool) -> bool {
        for m in &mut self.modules {
            if m.id() == id {
                m.set_enabled(enabled);
                return true;
            }
        }
        false
    }

    pub fn dispatch_before_request(&self, req: &mut InterceptedRequest) -> ModuleDecision {
        for module in &self.modules {
            if !module.is_enabled() {
                continue;
            }
            match module.on_before_request(req) {
                ModuleDecision::Proceed => continue,
                decision => return decision,
            }
        }
        ModuleDecision::Proceed
    }

    pub fn dispatch_response_headers(
        &self,
        req: &InterceptedRequest,
        res: &mut InterceptedResponse,
    ) {
        for module in &self.modules {
            if module.is_enabled() {
                module.on_response_headers(req, res);
            }
        }
    }

    pub fn dispatch_before_script(&self, origin: &str, script: &mut String) -> bool {
        for module in &self.modules {
            if module.is_enabled() && !module.on_before_script_eval(origin, script) {
                return false;
            }
        }
        true
    }

    pub fn dispatch_dom_ready(&self, tab_id: u32, origin: &str) {
        for module in &self.modules {
            if module.is_enabled() {
                module.on_dom_ready(tab_id, origin);
            }
        }
    }

    pub fn dispatch_tab_closed(&mut self, tab_id: u32) {
        for module in &mut self.modules {
            if module.is_enabled() {
                module.on_tab_closed(tab_id);
            }
        }
    }
}
