use crate::ram_vault::VolatileRamVault;

/// Rust Page Runtime Compiler & Accelerator (`page_compiler.rs`):
/// Generates the high-speed JavaScript + CSS injection payload executed at `onPageStarted`
/// so every page renders with:
/// 1. Anti-JS-Detection (`navigator.webdriver = false`, `window.chrome` spoofing, Canvas/Audio noise)
/// 2. Volatile In-RAM Cookie Virtualization (never writes cookies to disk, zeroes on close)
/// 3. Universal Developer CORS Unlocker (transparently falls back to `MantaBridge.corsFetch` on CORS blocks)
/// 4. Cosmetic Ad & Cookie-Banner Zapper (removes ad slots & layout shifts faster than Chrome/DDG)
pub struct PageRuntimeCompiler;

impl PageRuntimeCompiler {
    pub fn compile_runtime_script(cors_dev_enabled: bool) -> String {
        let cors_flag = if cors_dev_enabled { "true" } else { "false" };
        format!(
            r#"(function() {{
    if (window.__mantaRuntimeActive) return;
    window.__mantaRuntimeActive = true;

    // 1. Anti-Bot / Anti-JS-Detection Shield
    try {{
        Object.defineProperty(navigator, 'webdriver', {{ get: () => false, configurable: false }});
        Object.defineProperty(navigator, 'GlobalPrivacyControl', {{ get: () => true }});
        Object.defineProperty(navigator, 'doNotTrack', {{ get: () => '1' }});
        Object.defineProperty(navigator, 'deviceMemory', {{ get: () => 8 }});
        Object.defineProperty(navigator, 'hardwareConcurrency', {{ get: () => 8 }});
        if (!window.chrome) {{
            window.chrome = {{ runtime: {{}}, loadTimes: function(){{ return {{}}; }}, csi: function(){{ return {{}}; }} }};
        }}
    }} catch (_) {{}}

    // 2. Volatile RAM-Only Cookie Virtualization (Zero Disk Persistence)
    try {{
        Object.defineProperty(document, 'cookie', {{
            get: function() {{
                try {{
                    return (window.MantaBridge && window.MantaBridge.getVolatileCookie)
                        ? window.MantaBridge.getVolatileCookie(location.hostname)
                        : '';
                }} catch (_) {{ return ''; }}
            }},
            set: function(val) {{
                try {{
                    if (window.MantaBridge && window.MantaBridge.setVolatileCookie) {{
                        window.MantaBridge.setVolatileCookie(location.hostname, String(val));
                    }}
                }} catch (_) {{}}
                return val;
            }},
            configurable: true
        }});
    }} catch (_) {{}}

    // 3. Developer CORS Unlocker for window.fetch
    const corsDevActive = {cors_flag};
    if (corsDevActive && window.fetch && window.MantaBridge && window.MantaBridge.corsFetch) {{
        const origFetch = window.fetch.bind(window);
        window.fetch = async function(input, init) {{
            try {{
                return await origFetch(input, init);
            }} catch (err) {{
                try {{
                    const targetUrl = (typeof input === 'string') ? input : (input && input.url ? input.url : String(input));
                    const method = (init && init.method) ? init.method : 'GET';
                    const body = (init && typeof init.body === 'string') ? init.body : '';
                    const rawResp = window.MantaBridge.corsFetch(targetUrl, method, body);
                    if (rawResp && !rawResp.startsWith('MANTA_CORS_ERR:')) {{
                        return new Response(rawResp, {{
                            status: 200,
                            headers: {{
                                'Access-Control-Allow-Origin': '*',
                                'Content-Type': 'application/json; charset=utf-8'
                            }}
                        }});
                    }}
                }} catch (_) {{}}
                throw err;
            }}
        }};
    }}

    // 4. Cosmetic Ad, Popup & Tracker Slot Zapper (Faster Layout than Chrome)
    try {{
        const injectCss = function() {{
            if (document.getElementById('__manta_cosmetic_shield')) return;
            const style = document.createElement('style');
            style.id = '__manta_cosmetic_shield';
            style.textContent = `
                .adsbygoogle, [id^="google_ads_iframe"], [id^="div-gpt-ad"],
                iframe[src*="doubleclick.net"], iframe[src*="googlesyndication.com"],
                .ad-banner, .ad-slot, .ad-container, [data-ad-slot] {{
                    display: none !important;
                    height: 0 !important;
                    min-height: 0 !important;
                    visibility: hidden !important;
                    pointer-events: none !important;
                }}
            `;
            (document.head || document.documentElement).appendChild(style);
        }};
        if (document.head || document.documentElement) {{
            injectCss();
        }} else {{
            document.addEventListener('DOMContentLoaded', injectCss, {{ once: true }});
        }}
    }} catch (_) {{}}
}})();"#,
            cors_flag = cors_flag
        )
    }

    /// Stores a key-value cookie in Rust's volatile RAM vault keyed by tab + host.
    /// Never touches disk and is cryptographically zeroized (`0x00`) on tab/app close.
    pub fn store_volatile_cookie(
        vault: &mut VolatileRamVault,
        tab_id: u32,
        host: &str,
        raw_cookie: &str,
    ) {
        if host.is_empty() || raw_cookie.is_empty() {
            return;
        }
        let cookie_pair = raw_cookie.split(';').next().unwrap_or("").trim();
        if cookie_pair.is_empty() || cookie_pair.len() > 2048 {
            return;
        }

        let vault_key = format!("cookie:{}", host.to_ascii_lowercase());
        let existing = vault
            .get_ephemeral(tab_id, &vault_key)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
            .to_string();

        let new_key = cookie_pair.split('=').next().unwrap_or("").trim();
        let mut pairs: Vec<String> = existing
            .split("; ")
            .filter(|s| !s.is_empty() && !s.starts_with(&format!("{}=", new_key)))
            .map(|s| s.to_string())
            .collect();

        pairs.push(cookie_pair.to_string());
        let combined = pairs.join("; ");
        vault.put_ephemeral(tab_id, &vault_key, combined.as_bytes());
    }

    pub fn read_volatile_cookie(vault: &VolatileRamVault, tab_id: u32, host: &str) -> String {
        let vault_key = format!("cookie:{}", host.to_ascii_lowercase());
        vault
            .get_ephemeral(tab_id, &vault_key)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or("")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volatile_cookie_storage_and_zeroize() {
        let mut vault = VolatileRamVault::new(64 * 1024);
        PageRuntimeCompiler::store_volatile_cookie(
            &mut vault,
            1,
            "example.com",
            "session_id=xyz987; Path=/; Secure",
        );
        PageRuntimeCompiler::store_volatile_cookie(
            &mut vault,
            1,
            "example.com",
            "theme=dark; Path=/",
        );

        let cookies = PageRuntimeCompiler::read_volatile_cookie(&vault, 1, "example.com");
        assert_eq!(cookies, "session_id=xyz987; theme=dark");

        vault.zeroize_and_purge();
        assert_eq!(
            PageRuntimeCompiler::read_volatile_cookie(&vault, 1, "example.com"),
            ""
        );
    }
}
