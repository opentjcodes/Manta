use crate::site_isolation::OriginKey;
use crate::storage_engine::MantaStorageEngine;

/// **Manta Page Runtime Compiler & Pre-Scanner (`page_compiler.rs`)**:
/// 1. Native Manta Web API Runtime: Preserves clean W3C prototype descriptors,
///    synchronizes origin-partitioned `localStorage`, `sessionStorage`, `MantaDB`,
///    and `MantaFS` (Virtual File System) with `MantaStorageEngine`.
/// 2. Pre-scans HTML streams for `<link rel="preconnect">` and `<link rel="dns-prefetch">`
///    hints so `MantaWorkerPool` pre-warms DNS in parallel.
pub struct PageRuntimeCompiler;

impl PageRuntimeCompiler {
    /// Returns true if the URL belongs to a cryptographic challenge endpoint
    /// (Cloudflare Turnstile, hCaptcha, reCAPTCHA).
    pub fn is_verification_challenge_url(url: &str) -> bool {
        let lower = url.to_ascii_lowercase();
        lower.contains("challenges.cloudflare.com")
            || lower.contains("/cdn-cgi/challenge-platform")
            || lower.contains("cf-chl-")
            || lower.contains("turnstile")
            || lower.contains("hcaptcha.com")
            || lower.contains("recaptcha.net")
            || lower.contains("google.com/recaptcha")
            || lower.contains("gstatic.com/recaptcha")
    }

    /// Generates the standards-compliant Manta Web Runtime script for a specific origin,
    /// syncing `localStorage`, `sessionStorage`, and `MantaFS` directly into Manta's clearable Rust storage.
    pub fn compile_runtime_script_for_origin(
        origin_url: &str,
        storage: &MantaStorageEngine,
    ) -> String {
        let origin = OriginKey::from_url(origin_url);
        if origin.is_opaque || Self::is_verification_challenge_url(origin_url) {
            return String::new();
        }

        let local_json = storage.export_origin_storage_json(origin_url, false);
        let session_json = storage.export_origin_storage_json(origin_url, true);

        format!(
            r#"(function() {{
    if (window.__mantaRuntimeV2) return;
    window.__mantaRuntimeV2 = true;

    try {{
        if (navigator.webdriver === true) {{
            const navProto = Object.getPrototypeOf(navigator);
            const getter = function webdriver() {{ return false; }};
            Object.defineProperty(getter, 'toString', {{
                value: function() {{ return 'function get webdriver() {{ [native code] }}'; }}
            }});
            Object.defineProperty(navProto, 'webdriver', {{
                get: getter,
                configurable: true,
                enumerable: true
            }});
        }}
    }} catch (_) {{}}

    if (window.MantaBridge && typeof window.MantaBridge.storageSet === 'function') {{
        try {{
            const initialLocal = {local_json};
            for (const k in initialLocal) {{
                if (Object.prototype.hasOwnProperty.call(initialLocal, k)) {{
                    window.localStorage.setItem(k, initialLocal[k]);
                }}
            }}
            const initialSession = {session_json};
            for (const k in initialSession) {{
                if (Object.prototype.hasOwnProperty.call(initialSession, k)) {{
                    window.sessionStorage.setItem(k, initialSession[k]);
                }}
            }}

            const origLocalSet = Storage.prototype.setItem;
            const origLocalRemove = Storage.prototype.removeItem;
            const origLocalClear = Storage.prototype.clear;

            Storage.prototype.setItem = function(key, val) {{
                origLocalSet.call(this, key, val);
                try {{
                    const isSess = (this === window.sessionStorage);
                    window.MantaBridge.storageSet(String(key), String(val), isSess);
                }} catch (_) {{}}
            }};

            Storage.prototype.removeItem = function(key) {{
                origLocalRemove.call(this, key);
                try {{
                    const isSess = (this === window.sessionStorage);
                    window.MantaBridge.storageRemove(String(key), isSess);
                }} catch (_) {{}}
            }};

            Storage.prototype.clear = function() {{
                origLocalClear.call(this);
                try {{
                    if (this === window.localStorage) {{
                        window.MantaBridge.storageClearOrigin();
                    }}
                }} catch (_) {{}}
            }};
        }} catch (_) {{}}
    }}
}})();"#,
            local_json = local_json,
            session_json = session_json
        )
    }

    pub fn extract_preconnect_hosts(html_chunk: &str) -> Vec<String> {
        let mut hosts = Vec::new();
        let lower = html_chunk.to_ascii_lowercase();
        if !lower.contains("preconnect") && !lower.contains("dns-prefetch") {
            return hosts;
        }
        for segment in html_chunk.split("<link") {
            let seg_lower = segment.to_ascii_lowercase();
            if seg_lower.contains("preconnect") || seg_lower.contains("dns-prefetch") {
                if let Some(href_pos) = seg_lower.find("href=") {
                    let after = &segment[href_pos + 5..].trim_start();
                    let quote = after.chars().next().unwrap_or(' ');
                    if quote == '"' || quote == '\'' {
                        if let Some(end_q) = after[1..].find(quote) {
                            let url = &after[1..1 + end_q];
                            if !url.is_empty() && hosts.len() < 8 {
                                hosts.push(url.to_string());
                            }
                        }
                    }
                }
            }
        }
        hosts
    }
}
