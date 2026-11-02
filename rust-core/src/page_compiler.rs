use crate::cookie_banner::CookieBannerEliminator;
use crate::ram_vault::VolatileRamVault;

/// Rust Page Runtime Compiler & Accelerator (`page_compiler.rs`):
/// Generates the high-speed JavaScript + CSS injection payload executed on pages:
/// 1. Cloudflare Turnstile & CAPTCHA Stealth Integrity (never tampers with challenge iframes,
///    preserves native `Function.prototype.toString` and `Navigator.prototype` descriptors).
/// 2. Cookie-Banner & GDPR Consent Wall Auto-Eliminator (`cookie_banner.rs`).
/// 3. Universal Developer CORS Unlocker (`window.fetch` + `XMLHttpRequest` -> `MantaBridge.corsRequest`).
/// 4. Cosmetic Ad Slot Zapper.
pub struct PageRuntimeCompiler;

impl PageRuntimeCompiler {
    /// Returns true if the URL belongs to Cloudflare Turnstile, hCaptcha, or Google reCAPTCHA.
    /// Manta leaves challenge URLs 100% untouched so cryptographic verification always succeeds.
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

    pub fn compile_runtime_script(cors_dev_enabled: bool) -> String {
        let cors_flag = if cors_dev_enabled { "true" } else { "false" };
        let cookie_css = CookieBannerEliminator::COSMETIC_COOKIE_CSS;
        let cookie_js = CookieBannerEliminator::AUTO_DISMISS_COOKIE_JS;

        format!(
            r#"(function() {{
    if (window.__mantaRuntimeV2) return;
    window.__mantaRuntimeV2 = true;

    const host = (location && location.hostname) ? location.hostname.toLowerCase() : '';
    const isChallengeFrame = (
        host.includes('cloudflare.com') ||
        host.includes('hcaptcha.com') ||
        host.includes('recaptcha.net') ||
        (location && location.pathname && location.pathname.includes('/cdn-cgi/'))
    );

    // 1. Stealth Anti-Webdriver Patch (Preserves native prototype & toString for Cloudflare Turnstile)
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

    // If inside a Cloudflare Turnstile / CAPTCHA iframe, stop here so zero globals or prototypes are altered!
    if (isChallengeFrame) return;

    // 2. Universal Developer CORS Unlocker (Supports BOTH window.fetch and XMLHttpRequest)
    const corsDevActive = {cors_flag};
    if (corsDevActive && window.MantaBridge && window.MantaBridge.corsRequest) {{
        // 2a. Wrap window.fetch
        if (window.fetch) {{
            const nativeFetch = window.fetch.bind(window);
            window.fetch = async function(input, init) {{
                let targetUrl = '';
                let method = 'GET';
                let bodyStr = '';
                let headersObj = {{}};

                try {{
                    targetUrl = (typeof input === 'string') ? input : (input && input.url ? input.url : String(input));
                    if (targetUrl.startsWith('//')) targetUrl = location.protocol + targetUrl;
                    else if (targetUrl.startsWith('/')) targetUrl = location.origin + targetUrl;

                    method = (init && init.method) ? String(init.method).toUpperCase() : (input && input.method ? String(input.method).toUpperCase() : 'GET');
                    if (init && init.headers) {{
                        if (init.headers instanceof Headers) {{
                            init.headers.forEach((v, k) => {{ headersObj[k] = v; }});
                        }} else if (typeof init.headers === 'object') {{
                            Object.assign(headersObj, init.headers);
                        }}
                    }}
                    if (init && init.body) {{
                        bodyStr = (typeof init.body === 'string') ? init.body : JSON.stringify(init.body);
                    }}
                }} catch (_) {{}}

                const isCrossOrigin = targetUrl.startsWith('http') && !targetUrl.startsWith(location.origin);
                const needsBodyBridge = isCrossOrigin && (method === 'POST' || method === 'PUT' || method === 'PATCH' || method === 'DELETE');

                if (needsBodyBridge) {{
                    const rawJson = window.MantaBridge.corsRequest(method, targetUrl, JSON.stringify(headersObj), bodyStr);
                    const parsed = JSON.parse(rawJson);
                    if (!parsed.error) {{
                        return new Response(parsed.body, {{
                            status: parsed.status || 200,
                            headers: {{
                                'Content-Type': parsed.contentType || 'application/json',
                                'Access-Control-Allow-Origin': '*'
                            }}
                        }});
                    }}
                }}

                try {{
                    return await nativeFetch(input, init);
                }} catch (fetchErr) {{
                    if (targetUrl.startsWith('http')) {{
                        const rawJson = window.MantaBridge.corsRequest(method, targetUrl, JSON.stringify(headersObj), bodyStr);
                        const parsed = JSON.parse(rawJson);
                        if (!parsed.error) {{
                            return new Response(parsed.body, {{
                                status: parsed.status || 200,
                                headers: {{
                                    'Content-Type': parsed.contentType || 'application/json',
                                    'Access-Control-Allow-Origin': '*'
                                }}
                            }});
                        }}
                    }}
                    throw fetchErr;
                }}
            }};
        }}

        // 2b. Wrap XMLHttpRequest for Axios / XHR apps
        if (window.XMLHttpRequest) {{
            const OrigXHR = window.XMLHttpRequest;
            const origOpen = OrigXHR.prototype.open;
            const origSend = OrigXHR.prototype.send;
            const origSetHeader = OrigXHR.prototype.setRequestHeader;

            OrigXHR.prototype.open = function(method, url) {{
                this.__mantaMethod = method ? String(method).toUpperCase() : 'GET';
                let resolved = String(url || '');
                if (resolved.startsWith('//')) resolved = location.protocol + resolved;
                else if (resolved.startsWith('/')) resolved = location.origin + resolved;
                this.__mantaUrl = resolved;
                this.__mantaHeaders = {{}};
                return origOpen.apply(this, arguments);
            }};

            OrigXHR.prototype.setRequestHeader = function(k, v) {{
                if (this.__mantaHeaders) this.__mantaHeaders[k] = v;
                return origSetHeader.apply(this, arguments);
            }};

            OrigXHR.prototype.send = function(body) {{
                const xhr = this;
                const url = xhr.__mantaUrl || '';
                const method = xhr.__mantaMethod || 'GET';
                const isCrossOrigin = url.startsWith('http') && !url.startsWith(location.origin);

                if (isCrossOrigin && (method === 'POST' || method === 'PUT' || method === 'PATCH' || method === 'DELETE')) {{
                    try {{
                        const bodyStr = (typeof body === 'string') ? body : (body ? JSON.stringify(body) : '');
                        const rawJson = window.MantaBridge.corsRequest(method, url, JSON.stringify(xhr.__mantaHeaders || {{}}), bodyStr);
                        const parsed = JSON.parse(rawJson);
                        if (!parsed.error) {{
                            Object.defineProperty(xhr, 'readyState', {{ value: 4, configurable: true }});
                            Object.defineProperty(xhr, 'status', {{ value: parsed.status || 200, configurable: true }});
                            Object.defineProperty(xhr, 'responseText', {{ value: parsed.body || '', configurable: true }});
                            Object.defineProperty(xhr, 'response', {{ value: parsed.body || '', configurable: true }});
                            if (typeof xhr.onreadystatechange === 'function') xhr.onreadystatechange();
                            if (typeof xhr.onload === 'function') xhr.onload();
                            return;
                        }}
                    }} catch (_) {{}}
                }}
                return origSend.apply(this, arguments);
            }};
        }}
    }}

    // 3. Cosmetic Ad & Cookie-Banner Zapper
    try {{
        const injectStyles = function() {{
            if (document.getElementById('__manta_shield_css')) return;
            const style = document.createElement('style');
            style.id = '__manta_shield_css';
            style.textContent = `
                .adsbygoogle, [id^="google_ads_iframe"], [id^="div-gpt-ad"],
                iframe[src*="doubleclick.net"], iframe[src*="googlesyndication.com"],
                .ad-banner, .ad-slot, [data-ad-slot] {{
                    display: none !important;
                    height: 0 !important;
                    visibility: hidden !important;
                }}
                {cookie_css}
            `;
            (document.head || document.documentElement).appendChild(style);
        }};
        if (document.head || document.documentElement) injectStyles();
        else document.addEventListener('DOMContentLoaded', injectStyles, {{ once: true }});
    }} catch (_) {{}}

    // 4. Auto-Dismiss Cookie Walls & Unlock Page Scroll
    {cookie_js}
}})();"#,
            cors_flag = cors_flag,
            cookie_css = cookie_css,
            cookie_js = cookie_js
        )
    }

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
        vault.put_ephemeral(tab_id, &vault_key, cookie_pair.as_bytes());
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
