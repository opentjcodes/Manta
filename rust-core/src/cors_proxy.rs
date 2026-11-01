use std::collections::HashMap;

use crate::module_bus::{
    InterceptedRequest, InterceptedResponse, MantaFeatureModule, ModuleDecision,
};

/// Developer CORS Engine:
/// Allows developers to test local and remote web apps, APIs, and microservices inside Manta
/// without being blocked by Cross-Origin Resource Sharing restrictions.
pub struct CorsDeveloperModule {
    enabled: bool,
}

impl CorsDeveloperModule {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl MantaFeatureModule for CorsDeveloperModule {
    fn id(&self) -> &'static str {
        "cors_dev"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn on_before_request(&self, req: &mut InterceptedRequest) -> ModuleDecision {
        // If a web app under test sends an OPTIONS preflight request, synthesize an instant
        // 0ms 204 No Content permissive response so preflights never fail or add latency.
        if req.method.eq_ignore_ascii_case("OPTIONS") || req.is_cors_preflight {
            let mut headers = HashMap::new();
            let origin = req
                .headers
                .get("Origin")
                .or_else(|| req.headers.get("origin"))
                .cloned()
                .unwrap_or_else(|| "*".to_string());

            headers.insert("Access-Control-Allow-Origin".to_string(), origin);
            headers.insert(
                "Access-Control-Allow-Methods".to_string(),
                "GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD".to_string(),
            );
            headers.insert("Access-Control-Allow-Headers".to_string(), "*".to_string());
            headers.insert(
                "Access-Control-Allow-Credentials".to_string(),
                "true".to_string(),
            );
            headers.insert("Access-Control-Max-Age".to_string(), "86400".to_string());

            return ModuleDecision::SynthesizeResponse(InterceptedResponse {
                status_code: 204,
                headers,
                synthetic_body: Some(Vec::new()),
            });
        }
        ModuleDecision::Proceed
    }

    fn on_response_headers(&self, req: &InterceptedRequest, res: &mut InterceptedResponse) {
        let origin = req
            .headers
            .get("Origin")
            .or_else(|| req.headers.get("origin"))
            .cloned()
            .unwrap_or_else(|| "*".to_string());

        res.headers
            .insert("Access-Control-Allow-Origin".to_string(), origin);
        res.headers.insert(
            "Access-Control-Allow-Methods".to_string(),
            "GET, POST, PUT, PATCH, DELETE, OPTIONS, HEAD".to_string(),
        );
        res.headers
            .insert("Access-Control-Allow-Headers".to_string(), "*".to_string());
        res.headers.insert(
            "Access-Control-Expose-Headers".to_string(),
            "*".to_string(),
        );
        res.headers.insert(
            "Access-Control-Allow-Credentials".to_string(),
            "true".to_string(),
        );

        // Remove restrictive framing headers during local/dev site testing
        res.headers.remove("X-Frame-Options");
        res.headers.remove("x-frame-options");
        res.headers.remove("Cross-Origin-Resource-Policy");
        res.headers.remove("Cross-Origin-Embedder-Policy");
    }
}
