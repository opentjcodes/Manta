use crate::module_bus::{
    InterceptedRequest, InterceptedResponse, MantaFeatureModule, ModuleDecision,
};

/// Known tracking and telemetry query parameters stripped at the byte-stream level.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "fbclid",
    "gclid",
    "dclid",
    "msclkid",
    "twclid",
    "ttclid",
    "mc_cid",
    "mc_eid",
    "_ga",
    "_gl",
    "yclid",
    "igshid",
    "si",
    "ref_src",
];

/// High-speed domain suffixes blocked from loading (analytics, fingerprinting, telemetry).
const BLOCKED_HOST_SUFFIXES: &[&str] = &[
    "doubleclick.net",
    "google-analytics.com",
    "googletagmanager.com",
    "googlesyndication.com",
    "connect.facebook.net",
    "graph.facebook.com",
    "analytics.tiktok.com",
    "hotjar.com",
    "mixpanel.com",
    "segment.io",
    "amplitude.com",
    "clarity.ms",
    "fullstory.com",
    "fingerprintjs.com",
    "fpjs.io",
    "newrelic.com",
    "sentry.io",
];

/// Anti-fingerprinting, Zero-Cookie, and Anti-JS-Detection Shield.
pub struct PrivacyShieldModule {
    enabled: bool,
    block_fingerprinting: bool,
    strip_params: bool,
}

impl PrivacyShieldModule {
    pub fn new(block_fingerprinting: bool, strip_params: bool) -> Self {
        Self {
            enabled: true,
            block_fingerprinting,
            strip_params,
        }
    }

    /// Cleans tracking parameters from a URL without allocating unnecessary regexes.
    pub fn sanitize_url(raw_url: &str) -> String {
        let Some(q_idx) = raw_url.find('?') else {
            return raw_url.to_string();
        };

        let base = &raw_url[..q_idx];
        let rest = &raw_url[q_idx + 1..];
        let (query_part, fragment_part) = match rest.find('#') {
            Some(f_idx) => (&rest[..f_idx], Some(&rest[f_idx..])),
            None => (rest, None),
        };

        let mut kept_pairs = Vec::new();
        for pair in query_part.split('&') {
            if pair.is_empty() {
                continue;
            }
            let key = pair.split('=').next().unwrap_or("");
            let key_lower = key.to_ascii_lowercase();
            let is_tracker = key_lower.starts_with("utm_")
                || TRACKING_PARAMS.iter().any(|&tp| tp == key_lower);
            if !is_tracker {
                kept_pairs.push(pair);
            }
        }

        let mut out = String::with_capacity(raw_url.len());
        out.push_str(base);
        if !kept_pairs.is_empty() {
            out.push('?');
            out.push_str(&kept_pairs.join("&"));
        }
        if let Some(frag) = fragment_part {
            out.push_str(frag);
        }
        out
    }

    pub fn is_blocked_domain(url: &str) -> bool {
        let host = extract_host(url);
        if host.is_empty() {
            return false;
        }
        let host_lower = host.to_ascii_lowercase();
        BLOCKED_HOST_SUFFIXES.iter().any(|&suffix| {
            host_lower == suffix || host_lower.ends_with(&format!(".{}", suffix))
        })
    }
}

impl MantaFeatureModule for PrivacyShieldModule {
    fn id(&self) -> &'static str {
        "privacy_shield"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn on_before_request(&self, req: &mut InterceptedRequest) -> ModuleDecision {
        if Self::is_blocked_domain(&req.url) {
            return ModuleDecision::Block {
                reason: "Blocked tracker/fingerprinting host",
            };
        }

        if self.strip_params {
            req.url = Self::sanitize_url(&req.url);
        }

        // Enforce strict zero-cookie & anti-tracking request headers
        req.headers.remove("Cookie");
        req.headers.remove("cookie");
        req.headers.remove("X-Client-Data");
        req.headers.insert("DNT".to_string(), "1".to_string());
        req.headers.insert("Sec-GPC".to_string(), "1".to_string());

        // Standardized clean User-Agent that prevents bot-detection blocks while revealing zero device entropy
        req.headers.entry("User-Agent".to_string()).or_insert_with(|| {
            "Mozilla/5.0 (Linux; Android 14; Mobile) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Mobile Safari/537.36".to_string()
        });

        ModuleDecision::Proceed
    }

    fn on_response_headers(&self, _req: &InterceptedRequest, res: &mut InterceptedResponse) {
        // Strip all incoming Set-Cookie and tracking headers before they ever touch RAM state
        res.headers.remove("Set-Cookie");
        res.headers.remove("set-cookie");
        res.headers.remove("Set-Cookie2");
        res.headers.remove("Clear-Site-Data");
        res.headers.remove("Server-Timing");
    }

    fn on_before_script_eval(&self, _origin: &str, script_source: &mut String) -> bool {
        if !self.block_fingerprinting {
            return true;
        }
        // Block known aggressive fingerprinting & bot-probing payloads immediately
        let lower = script_source.to_ascii_lowercase();
        if lower.contains("fingerprintjs")
            || lower.contains("canvas.todataurl") && lower.contains("audiocontext")
        {
            return false;
        }

        // Neutralize inline webdriver / automation detection checks
        if script_source.contains("navigator.webdriver") {
            *script_source = script_source.replace("navigator.webdriver", "false");
        }
        true
    }
}

fn extract_host(url: &str) -> &str {
    let without_scheme = match url.find("://") {
        Some(idx) => &url[idx + 3..],
        None => url,
    };
    let end = without_scheme
        .find(['/', ':', '?', '#'])
        .unwrap_or(without_scheme.len());
    &without_scheme[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracking_param_stripping() {
        let dirty = "https://example.com/article?id=99&utm_source=twitter&fbclid=IwAR123&sort=asc#top";
        let clean = PrivacyShieldModule::sanitize_url(dirty);
        assert_eq!(clean, "https://example.com/article?id=99&sort=asc#top");
    }

    #[test]
    fn test_tracker_domain_blocking() {
        assert!(PrivacyShieldModule::is_blocked_domain(
            "https://www.google-analytics.com/collect"
        ));
        assert!(!PrivacyShieldModule::is_blocked_domain(
            "https://developer.mozilla.org/en-US/"
        ));
    }
}
