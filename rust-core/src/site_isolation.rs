use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::module_bus::{
    InterceptedRequest, InterceptedResponse, MantaFeatureModule, ModuleDecision,
};

/// Canonical RFC 6454 Web Origin (`scheme://host:port`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OriginKey {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub is_opaque: bool,
}

impl OriginKey {
    pub fn from_url(url: &str) -> Self {
        let trimmed = strip_ascii_control_and_ws(url);
        if trimmed.is_empty()
            || trimmed.starts_with("about:")
            || trimmed.starts_with("data:")
            || trimmed.starts_with("blob:")
        {
            return Self {
                scheme: "opaque".to_string(),
                host: "null".to_string(),
                port: 0,
                is_opaque: true,
            };
        }

        let (scheme, rest) = match trimmed.find("://") {
            Some(idx) => (
                trimmed[..idx].to_ascii_lowercase(),
                &trimmed[idx + 3..],
            ),
            None => ("https".to_string(), trimmed.as_str()),
        };

        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let host_port = authority.rsplit('@').next().unwrap_or(authority);

        let default_port = match scheme.as_str() {
            "http" | "ws" => 80,
            "https" | "wss" => 443,
            "manta" => 0,
            _ => 443,
        };

        let (host, port) = if host_port.starts_with('[') {
            if let Some(close_bracket) = host_port.find(']') {
                let h = host_port[..=close_bracket].to_ascii_lowercase();
                let after = &host_port[close_bracket + 1..];
                let p = after
                    .strip_prefix(':')
                    .and_then(|s| s.parse::<u16>().ok())
                    .unwrap_or(default_port);
                (h, p)
            } else {
                (host_port.to_ascii_lowercase(), default_port)
            }
        } else if let Some(colon) = host_port.rfind(':') {
            let h = host_port[..colon].to_ascii_lowercase();
            let p = host_port[colon + 1..]
                .parse::<u16>()
                .unwrap_or(default_port);
            (h, p)
        } else {
            (host_port.to_ascii_lowercase(), default_port)
        };

        if host.is_empty() {
            return Self {
                scheme: "opaque".to_string(),
                host: "null".to_string(),
                port: 0,
                is_opaque: true,
            };
        }

        Self {
            scheme,
            host,
            port,
            is_opaque: false,
        }
    }

    pub fn serialize(&self) -> String {
        if self.is_opaque {
            return "null".to_string();
        }
        let is_default = (self.scheme == "https" && self.port == 443)
            || (self.scheme == "http" && self.port == 80)
            || self.port == 0;
        if is_default {
            format!("{}://{}", self.scheme, self.host)
        } else {
            format!("{}://{}:{}", self.scheme, self.host, self.port)
        }
    }

    pub fn is_same_origin(&self, other: &OriginKey) -> bool {
        if self.is_opaque || other.is_opaque {
            return false;
        }
        self.scheme == other.scheme && self.host == other.host && self.port == other.port
    }

    pub fn is_secure_context(&self) -> bool {
        matches!(self.scheme.as_str(), "https" | "wss" | "manta")
            || self.host == "localhost"
            || self.host == "127.0.0.1"
            || self.host == "[::1]"
    }

    pub fn is_private_or_loopback_network(&self) -> bool {
        let h = self.host.trim_matches(|c| c == '[' || c == ']');
        if h == "localhost"
            || h.ends_with(".local")
            || h.ends_with(".localhost")
            || h.ends_with(".internal")
            || h == "0.0.0.0"
        {
            return true;
        }
        // Standard IPv6 literal check
        if let Ok(ip6) = h.parse::<Ipv6Addr>() {
            return is_private_or_reserved_ip(IpAddr::V6(ip6));
        }
        // Standard IPv4 literal check
        if let Ok(ip4) = h.parse::<Ipv4Addr>() {
            return is_private_or_reserved_ip(IpAddr::V4(ip4));
        }
        // Obfuscated integer/hex/octal IPv4 literal defense (e.g., 0x7f000001, 2130706433, 0177.0.0.1)
        if let Some(ip4) = parse_nonstandard_ipv4(h) {
            return is_private_or_reserved_ip(IpAddr::V4(ip4));
        }
        false
    }
}

/// Validates any resolved `std::net::IpAddr` against loopback, private, link-local,
/// cloud metadata (`169.254.169.254`), CGNAT (`100.64.0.0/10`), and IPv4-mapped IPv6 bypasses.
pub fn is_private_or_reserved_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || (o[0] == 100 && (o[1] & 0xC0) == 64) // CGNAT 100.64.0.0/10
                || (o[0] == 192 && o[1] == 0 && o[2] == 0) // IETF protocol assignments
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // Benchmarking 198.18.0.0/15
        }
        IpAddr::V6(v6) => {
            if let Some(mapped_v4) = v6.to_ipv4_mapped() {
                return is_private_or_reserved_ip(IpAddr::V4(mapped_v4));
            }
            let segs = v6.segments();
            v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (segs[0] & 0xFE00) == 0xFC00 // Unique Local Address fc00::/7
                || (segs[0] & 0xFFC0) == 0xFE80 // Link-Local fe80::/10
        }
    }
}

fn parse_nonstandard_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let mut vals = Vec::with_capacity(parts.len());
    for p in &parts {
        let v = if let Some(hex) = p.strip_prefix("0x").or_else(|| p.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()?
        } else if p.starts_with('0') && p.len() > 1 {
            u32::from_str_radix(p, 8).ok()?
        } else {
            p.parse::<u32>().ok()?
        };
        vals.push(v);
    }
    match vals.len() {
        1 => Some(Ipv4Addr::from(vals[0])),
        2 if vals[0] <= 255 && vals[1] <= 0xFFFFFF => {
            Some(Ipv4Addr::from((vals[0] << 24) | vals[1]))
        }
        3 if vals[0] <= 255 && vals[1] <= 255 && vals[2] <= 0xFFFF => {
            Some(Ipv4Addr::from((vals[0] << 24) | (vals[1] << 16) | vals[2]))
        }
        4 if vals.iter().all(|&x| x <= 255) => Some(Ipv4Addr::new(
            vals[0] as u8,
            vals[1] as u8,
            vals[2] as u8,
            vals[3] as u8,
        )),
        _ => None,
    }
}

/// Strips ASCII whitespace and C0 control chars (`\x00-\x20`) to defeat `java\tscript:` XSS bypasses.
fn strip_ascii_control_and_ws(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|&c| !c.is_control() && (c as u32) >= 0x20)
        .collect()
}

fn extract_scheme_canonical(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.chars() {
        if c == ':' {
            break;
        }
        if c.is_ascii_control() || c.is_ascii_whitespace() {
            continue;
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Complete capability matrix controllable per origin in Manta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SiteCapability {
    JavaScript,
    CookiesAndStorage,
    FileSystemAccess,
    AutomaticDownloads,
    ClipboardRead,
    ClipboardWrite,
    Popups,
    Geolocation,
    Notifications,
    MediaCamera,
    MediaMicrophone,
    Autoplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Ask,
    Block,
}

#[derive(Debug, Clone, Default)]
pub struct DocumentSecurityPolicy {
    pub disallow_inline_scripts: bool,
    pub disallow_eval: bool,
    pub frame_ancestors_none: bool,
    pub upgrade_insecure_requests: bool,
    pub coop_same_origin: bool,
    pub coep_require_corp: bool,
}

impl DocumentSecurityPolicy {
    pub fn parse_from_headers(headers: &HashMap<String, String>) -> Self {
        let mut policy = Self::default();
        for (k, v) in headers {
            let key_lower = k.to_ascii_lowercase();
            let val_lower = v.to_ascii_lowercase();
            match key_lower.as_str() {
                "content-security-policy" => {
                    if val_lower.contains("script-src") && !val_lower.contains("'unsafe-inline'") {
                        policy.disallow_inline_scripts = true;
                    }
                    if val_lower.contains("script-src") && !val_lower.contains("'unsafe-eval'") {
                        policy.disallow_eval = true;
                    }
                    if val_lower.contains("frame-ancestors 'none'") {
                        policy.frame_ancestors_none = true;
                    }
                    if val_lower.contains("upgrade-insecure-requests") {
                        policy.upgrade_insecure_requests = true;
                    }
                }
                "x-frame-options" => {
                    if val_lower.contains("deny") {
                        policy.frame_ancestors_none = true;
                    }
                }
                "cross-origin-opener-policy" => {
                    if val_lower.contains("same-origin") {
                        policy.coop_same_origin = true;
                    }
                }
                "cross-origin-embedder-policy" => {
                    if val_lower.contains("require-corp") {
                        policy.coep_require_corp = true;
                    }
                }
                _ => {}
            }
        }
        policy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IsolationVerdict {
    Allow { sanitized_url: String },
    UpgradeToHttps { upgraded_url: String },
    Deny { reason: &'static str },
}

/// Enforces strict Manta Site Isolation, Same-Origin Policy, W3C CORS,
/// Private Network Access (PNA/SSRF) defense, and scheme/XSS hardening.
pub struct SiteIsolationEngine {
    permissions: HashMap<(String, SiteCapability), PermissionDecision>,
    hsts_hosts: HashMap<String, bool>,
}

impl Default for SiteIsolationEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl SiteIsolationEngine {
    pub fn new() -> Self {
        Self {
            permissions: HashMap::new(),
            hsts_hosts: HashMap::new(),
        }
    }

    pub fn set_permission(
        &mut self,
        origin_url: &str,
        capability: SiteCapability,
        decision: PermissionDecision,
    ) {
        let origin = OriginKey::from_url(origin_url).serialize();
        self.permissions.insert((origin, capability), decision);
    }

    pub fn check_permission(
        &self,
        origin_url: &str,
        capability: SiteCapability,
    ) -> PermissionDecision {
        let origin = OriginKey::from_url(origin_url).serialize();
        if let Some(&decision) = self.permissions.get(&(origin, capability)) {
            return decision;
        }
        match capability {
            SiteCapability::JavaScript
            | SiteCapability::CookiesAndStorage
            | SiteCapability::FileSystemAccess
            | SiteCapability::AutomaticDownloads
            | SiteCapability::ClipboardWrite
            | SiteCapability::Autoplay => PermissionDecision::Allow,
            SiteCapability::ClipboardRead
            | SiteCapability::Geolocation
            | SiteCapability::Notifications
            | SiteCapability::MediaCamera
            | SiteCapability::MediaMicrophone => PermissionDecision::Ask,
            SiteCapability::Popups => PermissionDecision::Block,
        }
    }

    pub fn validate_request(
        &self,
        initiator_url: Option<&str>,
        target_url: &str,
        is_main_frame: bool,
    ) -> IsolationVerdict {
        let sanitized = strip_ascii_control_and_ws(target_url);
        if sanitized.is_empty() {
            return IsolationVerdict::Deny {
                reason: "Empty target URL",
            };
        }

        // Canonical scheme extraction defeats `java\tscript:`, `\x01javascript:`, etc.
        let scheme = extract_scheme_canonical(&sanitized);
        if matches!(
            scheme.as_str(),
            "javascript" | "vbscript" | "file" | "content" | "intent"
        ) {
            return IsolationVerdict::Deny {
                reason: "Blocked unsafe URI scheme",
            };
        }

        let lower = sanitized.to_ascii_lowercase();
        if lower.starts_with("manta://") || lower == "about:blank" {
            return IsolationVerdict::Allow {
                sanitized_url: sanitized,
            };
        }

        if lower.starts_with("data:") {
            // Restrict dangerous HTML/JS top-level data: navigations while allowing safe media/text data URIs
            if is_main_frame && (lower.contains("text/html") || lower.contains("application/xhtml")) {
                return IsolationVerdict::Deny {
                    reason: "Blocked top-level HTML data: URI navigation",
                };
            }
            return IsolationVerdict::Allow {
                sanitized_url: sanitized,
            };
        }

        let target_origin = OriginKey::from_url(&sanitized);
        if target_origin.is_opaque {
            return IsolationVerdict::Deny {
                reason: "Blocked opaque subresource origin",
            };
        }

        // Strict Private Network Access (PNA / SSRF) protection
        if let Some(init_str) = initiator_url {
            if !init_str.is_empty() && !init_str.starts_with("manta://") {
                let init_origin = OriginKey::from_url(init_str);
                if !init_origin.is_private_or_loopback_network()
                    && target_origin.is_private_or_loopback_network()
                {
                    return IsolationVerdict::Deny {
                        reason: "Blocked cross-origin Private Network Access (SSRF protection)",
                    };
                }

                if !is_main_frame
                    && init_origin.scheme == "https"
                    && target_origin.scheme == "http"
                    && !target_origin.is_private_or_loopback_network()
                {
                    let upgraded = sanitized.replacen("http://", "https://", 1);
                    return IsolationVerdict::UpgradeToHttps {
                        upgraded_url: upgraded,
                    };
                }
            }
        }

        if target_origin.scheme == "http"
            && self
                .hsts_hosts
                .get(&target_origin.host)
                .copied()
                .unwrap_or(false)
        {
            let upgraded = sanitized.replacen("http://", "https://", 1);
            return IsolationVerdict::UpgradeToHttps {
                upgraded_url: upgraded,
            };
        }

        IsolationVerdict::Allow {
            sanitized_url: sanitized,
        }
    }

    pub fn verify_standard_cors_response(
        initiator_origin: &OriginKey,
        target_origin: &OriginKey,
        response_headers: &HashMap<String, String>,
        with_credentials: bool,
    ) -> bool {
        if initiator_origin.is_same_origin(target_origin) {
            return true;
        }
        let allow_origin = response_headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Access-Control-Allow-Origin"))
            .map(|(_, v)| v.trim())
            .unwrap_or("");

        if allow_origin.is_empty() {
            return false;
        }
        if allow_origin == "*" {
            return !with_credentials;
        }
        let serialized_init = initiator_origin.serialize();
        if allow_origin.eq_ignore_ascii_case(&serialized_init) {
            if with_credentials {
                let allow_creds = response_headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("Access-Control-Allow-Credentials"))
                    .map(|(_, v)| v.trim())
                    .unwrap_or("");
                return allow_creds == "true";
            }
            return true;
        }
        false
    }

    pub fn record_hsts_header(&mut self, host: &str, hsts_value: &str) {
        if hsts_value.to_ascii_lowercase().contains("max-age=")
            && !hsts_value.contains("max-age=0")
        {
            self.hsts_hosts.insert(host.to_ascii_lowercase(), true);
        }
    }
}

pub struct SiteIsolationModule {
    enabled: bool,
    engine: SiteIsolationEngine,
}

impl Default for SiteIsolationModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SiteIsolationModule {
    pub fn new() -> Self {
        Self {
            enabled: true,
            engine: SiteIsolationEngine::new(),
        }
    }
}

impl MantaFeatureModule for SiteIsolationModule {
    fn id(&self) -> &'static str {
        "site_isolation"
    }

    fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    fn on_before_request(&self, req: &mut InterceptedRequest) -> ModuleDecision {
        let initiator = req
            .headers
            .get("Origin")
            .or_else(|| req.headers.get("Referer"))
            .map(|s| s.as_str());
        let is_main_frame = initiator.is_none();

        match self.engine.validate_request(initiator, &req.url, is_main_frame) {
            IsolationVerdict::Allow { sanitized_url } => {
                req.url = sanitized_url;
                let origin = OriginKey::from_url(&req.url);
                if origin.is_secure_context() {
                    req.headers
                        .entry("Upgrade-Insecure-Requests".to_string())
                        .or_insert_with(|| "1".to_string());
                }
                ModuleDecision::Proceed
            }
            IsolationVerdict::UpgradeToHttps { upgraded_url } => {
                req.url = upgraded_url;
                ModuleDecision::Proceed
            }
            IsolationVerdict::Deny { reason } => ModuleDecision::Block { reason },
        }
    }

    fn on_response_headers(&self, _req: &InterceptedRequest, _res: &mut InterceptedResponse) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssrf_obfuscated_ips_and_xss_control_char_bypass_blocked() {
        let engine = SiteIsolationEngine::new();

        // Control char obfuscated javascript: XSS
        assert!(matches!(
            engine.validate_request(None, " \x01java\tscript:alert(1)", true),
            IsolationVerdict::Deny { .. }
        ));

        // Integer & Hex SSRF bypasses
        assert!(matches!(
            engine.validate_request(Some("https://evil.com"), "http://2130706433/admin", false),
            IsolationVerdict::Deny { .. }
        ));
        assert!(matches!(
            engine.validate_request(Some("https://evil.com"), "http://0x7f000001:8080/", false),
            IsolationVerdict::Deny { .. }
        ));
        assert!(matches!(
            engine.validate_request(Some("https://evil.com"), "http://[::ffff:169.254.169.254]/latest", false),
            IsolationVerdict::Deny { .. }
        ));
    }
}
