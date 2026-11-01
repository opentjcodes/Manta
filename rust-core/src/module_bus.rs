use std::collections::HashMap;

/// Represents an outgoing HTTP request before it touches the socket.
#[derive(Debug, Clone)]
pub struct InterceptedRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub is_cors_preflight: bool,
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
    /// Proceed with potentially sanitized URL/headers.
    Proceed,
    /// Block the request immediately (0 network bytes sent).
    Block { reason: &'static str },
    /// Return an instant synthetic response (e.g., 0ms CORS OPTIONS preflight).
    SynthesizeResponse(InterceptedResponse),
}

/// Modular extension trait: new browser capabilities are added as isolated structs
/// implementing `MantaFeatureModule` without modifying core engine files.
pub trait MantaFeatureModule: Send + Sync {
    /// Unique identifier for this module (e.g. `"privacy_shield"`, `"cors_dev"`).
    fn id(&self) -> &'static str;

    /// Returns whether this module is currently active.
    fn is_enabled(&self) -> bool;

    /// Enable or disable the module at runtime.
    fn set_enabled(&mut self, enabled: bool);

    /// Hook called before any network request is opened.
    fn on_before_request(&self, _req: &mut InterceptedRequest) -> ModuleDecision {
        ModuleDecision::Proceed
    }

    /// Hook called as response headers arrive from the stream.
    fn on_response_headers(&self, _req: &InterceptedRequest, _res: &mut InterceptedResponse) {}

    /// Hook called before a `<script>` block is executed in the JS sandbox.
    fn on_before_script_eval(&self, _origin: &str, _script_source: &mut String) -> bool {
        true
    }
}

/// Registry and dispatcher for all modular browser features.
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
}
