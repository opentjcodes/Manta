/// **99% Modern Web Compatibility Engine (`web_compat_engine.rs`)**:
///
/// Ensures modern Single-Page Applications, streaming video sites, and complex web apps
/// run cleanly without missing-API crashes or false automation blocks:
/// - **ES2024 / WHATWG Web API Completeness**: `Promise.withResolvers`, `Object.groupBy`,
///   `Map.groupBy`, `URL.canParse`, `structuredClone`, `crypto.randomUUID`,
///   `Array.prototype.toSorted/toReversed/toSpliced/with/at`.
/// - **Navigator Client Hints & Capabilities**: `navigator.userAgentData`, `navigator.locks`,
///   `navigator.storage.estimate/persist`, `BroadcastChannel`, `window.chrome` object.
/// - **SPA History Router & Media Capabilities Shims**: `history.pushState`/`replaceState`
///   and `navigator.mediaCapabilities.decodingInfo` fast hardware codec hints.
pub struct WebCompatEngine;

impl WebCompatEngine {
    pub fn compile_web_compat_layer() -> &'static str {
        r#"(function() {
    if (window.__mantaWebCompat99) return;
    window.__mantaWebCompat99 = true;

    try {
        if (navigator.webdriver === true) {
            const navProto = Object.getPrototypeOf(navigator);
            const getter = function webdriver() { return false; };
            Object.defineProperty(getter, 'toString', {
                value: function() { return 'function get webdriver() { [native code] }'; }
            });
            Object.defineProperty(navProto, 'webdriver', {
                get: getter,
                configurable: true,
                enumerable: true
            });
        }
    } catch (_) {}

    /* 1. Standard window.chrome, Client Hints & MediaCapabilities */
    try {
        if (!window.chrome) {
            window.chrome = {
                app: { isInstalled: false },
                runtime: { OnInstalledReason: {}, OnRestartRequiredReason: {}, PlatformArch: {}, PlatformOs: {} },
                loadTimes: function() { return {}; },
                csi: function() { return {}; }
            };
        }
        if (!navigator.userAgentData) {
            const uaData = {
                brands: [
                    { brand: 'Not(A:Brand', version: '99' },
                    { brand: 'Manta', version: '132' },
                    { brand: 'Chromium', version: '132' }
                ],
                mobile: /Mobile|Android/i.test(navigator.userAgent),
                platform: /Android/i.test(navigator.userAgent) ? 'Android' : 'Linux',
                getHighEntropyValues: function(hints) {
                    return Promise.resolve({
                        brands: this.brands,
                        mobile: this.mobile,
                        platform: this.platform,
                        architecture: 'arm',
                        bitness: '64',
                        model: '',
                        platformVersion: '15.0.0',
                        uaFullVersion: '132.0.0.0',
                        fullVersionList: this.brands
                    });
                },
                toJSON: function() {
                    return { brands: this.brands, mobile: this.mobile, platform: this.platform };
                }
            };
            Object.defineProperty(Object.getPrototypeOf(navigator), 'userAgentData', {
                get: function() { return uaData; },
                configurable: true,
                enumerable: true
            });
        }
        if (!navigator.mediaCapabilities) {
            navigator.mediaCapabilities = {
                decodingInfo: function() {
                    return Promise.resolve({ supported: true, smooth: true, powerEfficient: true });
                },
                encodingInfo: function() {
                    return Promise.resolve({ supported: true, smooth: true, powerEfficient: true });
                }
            };
        }
    } catch (_) {}

    /* 2. ES2024 & WHATWG Standards Polyfills */
    try {
        if (typeof Promise.withResolvers !== 'function') {
            Promise.withResolvers = function() {
                let resolve, reject;
                const promise = new Promise(function(res, rej) { resolve = res; reject = rej; });
                return { promise: promise, resolve: resolve, reject: reject };
            };
        }
        if (typeof Object.groupBy !== 'function') {
            Object.groupBy = function(items, callbackFn) {
                const obj = Object.create(null);
                let idx = 0;
                for (const item of items) {
                    const key = callbackFn(item, idx++);
                    if (!(key in obj)) obj[key] = [];
                    obj[key].push(item);
                }
                return obj;
            };
        }
        if (typeof Map.groupBy !== 'function') {
            Map.groupBy = function(items, callbackFn) {
                const map = new Map();
                let idx = 0;
                for (const item of items) {
                    const key = callbackFn(item, idx++);
                    if (!map.has(key)) map.set(key, []);
                    map.get(key).push(item);
                }
                return map;
            };
        }
        if (typeof URL !== 'undefined' && typeof URL.canParse !== 'function') {
            URL.canParse = function(url, base) {
                try { new URL(url, base); return true; } catch (_) { return false; }
            };
        }
        if (typeof window.structuredClone !== 'function') {
            window.structuredClone = function(val) {
                if (val === undefined) return undefined;
                return JSON.parse(JSON.stringify(val));
            };
        }
        if (typeof crypto !== 'undefined' && typeof crypto.randomUUID !== 'function') {
            crypto.randomUUID = function() {
                return '10000000-1000-4000-8000-100000000000'.replace(/[018]/g, function(c) {
                    const n = Number(c);
                    return (n ^ (crypto.getRandomValues(new Uint8Array(1))[0] & (15 >> (n / 4)))).toString(16);
                });
            };
        }
        if (!Array.prototype.toSorted) {
            Array.prototype.toSorted = function(compareFn) { return Array.from(this).sort(compareFn); };
        }
        if (!Array.prototype.toReversed) {
            Array.prototype.toReversed = function() { return Array.from(this).reverse(); };
        }
        if (!Array.prototype.toSpliced) {
            Array.prototype.toSpliced = function(start, deleteCount) {
                const copy = Array.from(this);
                const items = Array.prototype.slice.call(arguments, 2);
                copy.splice.apply(copy, [start, deleteCount].concat(items));
                return copy;
            };
        }
        if (!Array.prototype.with) {
            Array.prototype.with = function(index, value) {
                const copy = Array.from(this);
                copy[index < 0 ? copy.length + index : index] = value;
                return copy;
            };
        }
        if (!Array.prototype.at) {
            Array.prototype.at = function(n) {
                const idx = Math.trunc(n) || 0;
                return this[idx < 0 ? this.length + idx : idx];
            };
        }
    } catch (_) {}

    /* 3. Navigator Storage Quota, Web Locks & BroadcastChannel Shims */
    try {
        if (navigator.storage) {
            if (typeof navigator.storage.persist !== 'function') {
                navigator.storage.persist = function() { return Promise.resolve(true); };
            }
            if (typeof navigator.storage.persisted !== 'function') {
                navigator.storage.persisted = function() { return Promise.resolve(true); };
            }
            if (typeof navigator.storage.estimate !== 'function') {
                navigator.storage.estimate = function() {
                    return Promise.resolve({ usage: 1048576, quota: 536870912 });
                };
            }
        }
        if (!navigator.locks) {
            navigator.locks = {
                request: function(_name, optionsOrCallback, maybeCallback) {
                    const cb = typeof optionsOrCallback === 'function' ? optionsOrCallback : maybeCallback;
                    return Promise.resolve().then(function() { return cb ? cb({ name: _name, mode: 'exclusive' }) : undefined; });
                },
                query: function() { return Promise.resolve({ held: [], pending: [] }); }
            };
        }
        if (typeof BroadcastChannel === 'undefined') {
            window.BroadcastChannel = function(name) {
                this.name = name;
                this.onmessage = null;
                this.postMessage = function() {};
                this.close = function() {};
                this.addEventListener = function() {};
                this.removeEventListener = function() {};
            };
        }
    } catch (_) {}

    /* 4. SPA History Router Hook */
    try {
        const origPushState = History.prototype.pushState;
        const origReplaceState = History.prototype.replaceState;
        History.prototype.pushState = function(state, unused, url) {
            const ret = origPushState.apply(this, arguments);
            try {
                window.dispatchEvent(new CustomEvent('manta:locationchange', { detail: { url: String(url || window.location.href) } }));
            } catch (_) {}
            return ret;
        };
        History.prototype.replaceState = function(state, unused, url) {
            const ret = origReplaceState.apply(this, arguments);
            try {
                window.dispatchEvent(new CustomEvent('manta:locationchange', { detail: { url: String(url || window.location.href) } }));
            } catch (_) {}
            return ret;
        };
    } catch (_) {}
})();"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_web_compat_layer_contains_modern_standards() {
        let script = WebCompatEngine::compile_web_compat_layer();
        assert!(script.contains("Promise.withResolvers"));
        assert!(script.contains("Object.groupBy"));
        assert!(script.contains("URL.canParse"));
        assert!(script.contains("navigator.userAgentData"));
        assert!(script.contains("navigator.mediaCapabilities"));
        assert!(script.contains("History.prototype.pushState"));
    }
}
