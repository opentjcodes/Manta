use crate::browser_controls::MantaSettings;
use crate::kernel_scripts::MantaKernelScripts;
use crate::site_isolation::OriginKey;
use crate::storage_engine::MantaStorageEngine;
use crate::web_compat_engine::WebCompatEngine;

/// **Manta Page Runtime Compiler & Pre-Scanner (`page_compiler.rs`)**:
/// 1. Compiles the combined **99% Web Compatibility Layer (`WebCompatEngine`)** +
///    **High-Velocity Custom Kernel Script (`MantaKernelScripts`)** for any target origin.
/// 2. Automatically bypasses script injection on cryptographic verification challenges
///    (Cloudflare Turnstile, hCaptcha, reCAPTCHA) so challenges pass with zero friction.
/// 3. Pre-scans HTML streams for `<link rel="preconnect">`, `<link rel="dns-prefetch">`,
///    and `<script src="...">` hosts so `MantaWorkerPool` pre-warms DNS in parallel.
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

    /// Generates the complete Manta Web Runtime & Kernel script for a specific origin.
    pub fn compile_runtime_script_for_origin(
        origin_url: &str,
        storage: &MantaStorageEngine,
    ) -> String {
        Self::compile_runtime_script_with_settings(origin_url, storage, &MantaSettings::default())
    }

    /// Generates the complete Manta Web Runtime & Kernel script using live `MantaSettings`.
    pub fn compile_runtime_script_with_settings(
        origin_url: &str,
        storage: &MantaStorageEngine,
        settings: &MantaSettings,
    ) -> String {
        let origin = OriginKey::from_url(origin_url);
        if origin.is_opaque || Self::is_verification_challenge_url(origin_url) {
            return String::new();
        }

        let mut script = String::with_capacity(8192);
        if settings.web_compat_99_enabled {
            script.push_str(WebCompatEngine::compile_web_compat_layer());
            script.push('\n');
        }
        script.push_str(&MantaKernelScripts::compile_origin_kernel(
            origin_url, storage, settings,
        ));
        script
    }

    pub fn extract_preconnect_hosts(html_chunk: &str) -> Vec<String> {
        let mut hosts = Vec::new();
        let lower = html_chunk.to_ascii_lowercase();
        if !lower.contains("preconnect")
            && !lower.contains("dns-prefetch")
            && !lower.contains("https://")
        {
            return hosts;
        }
        for segment in html_chunk.split('<') {
            let seg_lower = segment.to_ascii_lowercase();
            let is_link_hint = seg_lower.starts_with("link")
                && (seg_lower.contains("preconnect") || seg_lower.contains("dns-prefetch"));
            let is_script_src = seg_lower.starts_with("script") && seg_lower.contains("src=");

            if is_link_hint || is_script_src {
                let attr_key = if is_link_hint { "href=" } else { "src=" };
                if let Some(attr_pos) = seg_lower.find(attr_key) {
                    let after = &segment[attr_pos + attr_key.len()..].trim_start();
                    let quote = after.chars().next().unwrap_or(' ');
                    if quote == '"' || quote == '\'' {
                        if let Some(end_q) = after[1..].find(quote) {
                            let url = &after[1..1 + end_q];
                            if (url.starts_with("https://") || url.starts_with("http://"))
                                && !hosts.contains(&url.to_string())
                                && hosts.len() < 12
                            {
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
