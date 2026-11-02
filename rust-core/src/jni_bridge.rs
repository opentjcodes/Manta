use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use crate::page_compiler::PageRuntimeCompiler;
use crate::site_isolation::IsolationVerdict;
use crate::tab_compressor::TabCompressor;
use crate::MantaEngine;

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct JNIEnv(pub *const *const JNINativeInterface);

pub type JObject = *mut c_void;
pub type JClass = *mut c_void;
pub type JString = *mut c_void;
pub type JBoolean = u8;
pub type JInt = i32;

#[repr(C)]
#[allow(dead_code)]
pub struct JNINativeInterface {
    pub reserved: [*const c_void; 167],
    pub new_string_utf: unsafe extern "system" fn(JNIEnv, *const c_char) -> JString,
    pub get_string_utf_length: unsafe extern "system" fn(JNIEnv, JString) -> JInt,
    pub get_string_utf_chars:
        unsafe extern "system" fn(JNIEnv, JString, *mut JBoolean) -> *const c_char,
    pub release_string_utf_chars: unsafe extern "system" fn(JNIEnv, JString, *const c_char),
}

impl JNIEnv {
    pub unsafe fn get_rust_string(self, jstr: JString) -> String {
        if jstr.is_null() || self.0.is_null() {
            return String::new();
        }
        let table_ptr = unsafe { *self.0 };
        if table_ptr.is_null() {
            return String::new();
        }
        let table = unsafe { &*table_ptr };
        let mut is_copy: JBoolean = 0;
        let ptr = unsafe { (table.get_string_utf_chars)(self, jstr, &mut is_copy) };
        if ptr.is_null() {
            return String::new();
        }
        let rust_str = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        unsafe { (table.release_string_utf_chars)(self, jstr, ptr) };
        rust_str
    }

    pub unsafe fn new_java_string(self, text: &str) -> JString {
        if self.0.is_null() {
            return ptr::null_mut();
        }
        let table_ptr = unsafe { *self.0 };
        if table_ptr.is_null() {
            return ptr::null_mut();
        }
        let table = unsafe { &*table_ptr };
        let sanitized = text.replace('\0', "");
        match CString::new(sanitized) {
            Ok(cstr) => unsafe { (table.new_string_utf)(self, cstr.as_ptr()) },
            Err(_) => ptr::null_mut(),
        }
    }
}

/// Initializes Manta Multi-Core Rust Engine with its custom clearable storage file path.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeInitEngine(
    env: JNIEnv,
    _class: JClass,
    j_storage_path: JString,
) -> JInt {
    let storage_path = unsafe { env.get_rust_string(j_storage_path) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 1;
    };
    engine.init_with_storage_path(&storage_path) as JInt
}

/// Returns the number of active Rust worker cores in `MantaWorkerPool`.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetWorkerCoreCount(
    _env: JNIEnv,
    _class: JClass,
) -> JInt {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 2;
    };
    engine.workers.core_count() as JInt
}

/// Pre-warms DNS resolution for a URL on a background Rust worker thread.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativePrewarmDns(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
) {
    let url = unsafe { env.get_rust_string(j_url) };
    if let Ok(engine) = MantaEngine::global().lock() {
        engine.workers.prewarm_url_dns(&url);
    }
}

/// Creates a new isolated tab sandbox and returns its TabId.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeCreateNewTab(
    _env: JNIEnv,
    _class: JClass,
) -> JInt {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 1;
    };
    let id = engine.tabs.create_tab();
    engine.controls.ensure_tab_state(id);
    id as JInt
}

/// Compresses and stores a tab's state in Rust's VolatileRamVault when switching tabs.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeCompressTabState(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    j_url: JString,
    j_title: JString,
    scroll_y: JInt,
) -> JInt {
    let url = unsafe { env.get_rust_string(j_url) };
    let title = unsafe { env.get_rust_string(j_title) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine
        .controls
        .ensure_tab_state(tab_id as u32)
        .record_navigation(&url, &title);
    TabCompressor::store_tab_snapshot(&mut engine.vault, tab_id as u32, &url, &title, scroll_y)
        as JInt
}

/// Restores a tab's compressed snapshot (`scrollY|title|url`) from Rust's VolatileRamVault.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeRestoreTabState(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    match TabCompressor::restore_tab_snapshot(&engine.vault, tab_id as u32) {
        Some((scroll_y, title, url)) => {
            let formatted = format!("{}|{}|{}", scroll_y, title, url);
            unsafe { env.new_java_string(&formatted) }
        }
        None => unsafe { env.new_java_string("") },
    }
}

/// Switches active tab and automatically hibernates the previous tab's DOM arena in Rust RAM.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSwitchTab(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) -> JBoolean {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    let engine_ref = &mut *engine;
    if engine_ref
        .tabs
        .switch_to_tab(tab_id as u32, &mut engine_ref.vault)
    {
        1
    } else {
        0
    }
}

/// Toggles Desktop Site Mode (`request_desktop_site`) for a tab in Rust.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSetDesktopMode(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    enabled: JBoolean,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        engine
            .controls
            .set_desktop_mode(tab_id as u32, enabled != 0);
    }
}

/// Returns whether Desktop Site Mode is active for a tab.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeIsDesktopMode(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) -> JBoolean {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.controls.is_desktop_mode(tab_id as u32))
}

/// Sets page zoom percentage (`50` ..= `300`) for a tab.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSetTabZoom(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    zoom_percent: JInt,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        engine
            .controls
            .set_zoom_percent(tab_id as u32, zoom_percent.max(50) as u16);
    }
}

/// Gets page zoom percentage for a tab.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetTabZoom(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) -> JInt {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 100;
    };
    engine.controls.get_zoom_percent(tab_id as u32) as JInt
}

/// Returns the Rust-compiled Chrome Web API & Origin-Isolated Custom Storage runtime script.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetRuntimeScript(
    env: JNIEnv,
    _class: JClass,
    j_origin_url: JString,
) -> JString {
    let origin_url = unsafe { env.get_rust_string(j_origin_url) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    let script =
        PageRuntimeCompiler::compile_runtime_script_for_origin(&origin_url, &engine.storage);
    unsafe { env.new_java_string(&script) }
}

/// Checks if a URL is a Cloudflare Turnstile, hCaptcha, or reCAPTCHA verification endpoint.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeIsChallengeUrl(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
) -> JBoolean {
    let url = unsafe { env.get_rust_string(j_url) };
    u8::from(PageRuntimeCompiler::is_verification_challenge_url(&url))
}

/// Validates a URL against Manta's `SiteIsolationEngine` (SSRF / Private Network guard & scheme security).
/// Returns `"BLOCKED"` if denied, or the validated/upgraded URL string.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeValidateUrl(
    env: JNIEnv,
    _class: JClass,
    j_initiator_url: JString,
    j_target_url: JString,
    is_main_frame: JBoolean,
) -> JString {
    let initiator = unsafe { env.get_rust_string(j_initiator_url) };
    let target = unsafe { env.get_rust_string(j_target_url) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string(&target) };
    };
    let init_opt = if initiator.is_empty() {
        None
    } else {
        Some(initiator.as_str())
    };
    match engine
        .isolation
        .validate_request(init_opt, &target, is_main_frame != 0)
    {
        IsolationVerdict::Allow { sanitized_url } => unsafe { env.new_java_string(&sanitized_url) },
        IsolationVerdict::UpgradeToHttps { upgraded_url } => unsafe {
            env.new_java_string(&upgraded_url)
        },
        IsolationVerdict::Deny { .. } => unsafe { env.new_java_string("BLOCKED") },
    }
}

/// Writes an entry into Manta's origin-isolated custom storage engine (`localStorage` or `sessionStorage`).
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeStorageSetItem(
    env: JNIEnv,
    _class: JClass,
    j_origin_url: JString,
    j_key: JString,
    j_val: JString,
    is_session: JBoolean,
) -> JBoolean {
    let origin = unsafe { env.get_rust_string(j_origin_url) };
    let key = unsafe { env.get_rust_string(j_key) };
    let val = unsafe { env.get_rust_string(j_val) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    let ok = if is_session != 0 {
        engine.storage.set_session_storage(&origin, &key, &val)
    } else {
        let saved = engine.storage.set_local_storage(&origin, &key, &val);
        if saved {
            let _ = engine.storage.save_to_backing_file();
        }
        saved
    };
    u8::from(ok)
}

/// Reads an entry from Manta's origin-isolated custom storage engine.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeStorageGetItem(
    env: JNIEnv,
    _class: JClass,
    j_origin_url: JString,
    j_key: JString,
    is_session: JBoolean,
) -> JString {
    let origin = unsafe { env.get_rust_string(j_origin_url) };
    let key = unsafe { env.get_rust_string(j_key) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    let val = if is_session != 0 {
        engine.storage.get_session_storage(&origin, &key)
    } else {
        engine.storage.get_local_storage(&origin, &key)
    };
    unsafe { env.new_java_string(val.as_deref().unwrap_or("")) }
}

/// Removes an entry from Manta's origin-isolated custom storage engine.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeStorageRemoveItem(
    env: JNIEnv,
    _class: JClass,
    j_origin_url: JString,
    j_key: JString,
    is_session: JBoolean,
) {
    let origin = unsafe { env.get_rust_string(j_origin_url) };
    let key = unsafe { env.get_rust_string(j_key) };
    if let Ok(mut engine) = MantaEngine::global().lock() {
        if is_session != 0 {
            engine.storage.remove_session_storage(&origin, &key);
        } else if engine.storage.remove_local_storage(&origin, &key) {
            let _ = engine.storage.save_to_backing_file();
        }
    }
}

/// Clears all custom storage data (`localStorage`, `sessionStorage`, `cookies`) for a single origin.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeStorageClearOrigin(
    env: JNIEnv,
    _class: JClass,
    j_origin_url: JString,
) {
    let origin = unsafe { env.get_rust_string(j_origin_url) };
    if let Ok(mut engine) = MantaEngine::global().lock() {
        if engine.storage.clear_origin(&origin) {
            let _ = engine.storage.save_to_backing_file();
        }
    }
}

/// Flushes Manta's custom binary storage container to disk without writing any browser caches.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeFlushCustomStorage(
    _env: JNIEnv,
    _class: JClass,
) -> JBoolean {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.storage.save_to_backing_file().is_ok())
}

/// Toggles a bookmark inside Manta's custom storage engine.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeToggleBookmark(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
    j_title: JString,
) -> JBoolean {
    let url = unsafe { env.get_rust_string(j_url) };
    let title = unsafe { env.get_rust_string(j_title) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    let added = engine.storage.toggle_bookmark(&url, &title);
    let _ = engine.storage.save_to_backing_file();
    u8::from(added)
}

/// Streams a chunk of HTML/data into a tab and returns the serialized `MANTA_FRAME` DisplayList.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeStreamHtmlChunk(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    j_url: JString,
    j_html_chunk: JString,
    is_eof: JBoolean,
) -> JString {
    let url = unsafe { env.get_rust_string(j_url) };
    let chunk_str = unsafe { env.get_rust_string(j_html_chunk) };

    // Pre-warm any `<link rel="preconnect">` hosts discovered in this chunk on background cores
    let preconnect_hosts = PageRuntimeCompiler::extract_preconnect_hosts(&chunk_str);

    let Ok(mut engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("MANTA_ERR|Engine Lock Poisoned") };
    };
    for host_url in preconnect_hosts {
        engine.workers.prewarm_url_dns(&host_url);
    }

    let engine_ref = &mut *engine;
    match engine_ref.tabs.load_stream_chunk(
        tab_id as u32,
        &url,
        chunk_str.as_bytes(),
        is_eof != 0,
        &mut engine_ref.modules,
        &mut engine_ref.vault,
        &mut engine_ref.storage,
    ) {
        Ok(frame) => unsafe { env.new_java_string(&frame.to_compact_wire_string()) },
        Err(e) => unsafe { env.new_java_string(&format!("MANTA_ERR|{}", e)) },
    }
}

/// Copies text into Manta's size-capped Rust ClipboardSandbox.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeCopyToClipboard(
    env: JNIEnv,
    _class: JClass,
    j_text: JString,
) -> JBoolean {
    let text = unsafe { env.get_rust_string(j_text) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.clipboard.copy_text(&text))
}

/// Reads the current text stored in Manta's Rust ClipboardSandbox.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetClipboardText(
    env: JNIEnv,
    _class: JClass,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    unsafe { env.new_java_string(engine.clipboard.paste_text()) }
}

/// Closes a tab and releases its DOM/JS memory buffers while preserving saved user storage.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeCloseTab(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        let engine_ref = &mut *engine;
        engine_ref
            .tabs
            .close_tab(tab_id as u32, &mut engine_ref.vault);
        engine_ref.controls.remove_tab(tab_id as u32);
        engine_ref.modules.dispatch_tab_closed(tab_id as u32);
    }
}

/// Explicitly clears all custom storage, tabs, and volatile RAM when requested by the user.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeClearAllCustomStorageAndRam(
    _env: JNIEnv,
    _class: JClass,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        engine.clear_all_custom_storage_and_ram();
    }
}

/// Returns current total memory used by active tabs and transient RAM pool in bytes.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetEngineRamBytes(
    _env: JNIEnv,
    _class: JClass,
) -> JInt {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 0;
    };
    (engine.tabs.total_allocated_bytes() + engine.vault.used_bytes()) as JInt
}

/// Returns current bytes stored inside Manta's custom user storage container.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetCustomStorageBytes(
    _env: JNIEnv,
    _class: JClass,
) -> JInt {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine.storage.used_bytes() as JInt
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "system" fn mock_new_string_utf(_env: JNIEnv, utf: *const c_char) -> JString {
        if utf.is_null() {
            return ptr::null_mut();
        }
        let s = unsafe { CStr::from_ptr(utf).to_owned() };
        s.into_raw() as JString
    }

    unsafe extern "system" fn mock_get_string_utf_len(_env: JNIEnv, jstr: JString) -> JInt {
        if jstr.is_null() {
            return 0;
        }
        let cstr = unsafe { CStr::from_ptr(jstr as *const c_char) };
        cstr.to_bytes().len() as JInt
    }

    unsafe extern "system" fn mock_get_string_utf_chars(
        _env: JNIEnv,
        jstr: JString,
        is_copy: *mut JBoolean,
    ) -> *const c_char {
        if !is_copy.is_null() {
            unsafe { *is_copy = 0 };
        }
        jstr as *const c_char
    }

    unsafe extern "system" fn mock_release_string_utf_chars(
        _env: JNIEnv,
        _jstr: JString,
        _chars: *const c_char,
    ) {
    }

    #[test]
    fn test_jni_bridge_isolation_and_custom_storage() {
        let table = JNINativeInterface {
            reserved: [ptr::null(); 167],
            new_string_utf: mock_new_string_utf,
            get_string_utf_length: mock_get_string_utf_len,
            get_string_utf_chars: mock_get_string_utf_chars,
            release_string_utf_chars: mock_release_string_utf_chars,
        };
        let table_ptr: *const JNINativeInterface = &table;
        let env = JNIEnv(&table_ptr);

        unsafe {
            let empty_path = CString::new("").unwrap();
            let tab_id = Java_app_manta_browser_core_MantaNative_nativeInitEngine(
                env,
                ptr::null_mut(),
                empty_path.as_ptr() as JString,
            );
            assert!(tab_id >= 1);

            let init_c = CString::new("https://public-site.com").unwrap();
            let ssrf_c = CString::new("http://127.0.0.1:9000/secret").unwrap();
            let verdict_jstr = Java_app_manta_browser_core_MantaNative_nativeValidateUrl(
                env,
                ptr::null_mut(),
                init_c.as_ptr() as JString,
                ssrf_c.as_ptr() as JString,
                0,
            );
            let verdict_owned = CString::from_raw(verdict_jstr as *mut c_char);
            assert_eq!(verdict_owned.to_str().unwrap(), "BLOCKED");

            let key_c = CString::new("token").unwrap();
            let val_c = CString::new("abc123").unwrap();
            let saved = Java_app_manta_browser_core_MantaNative_nativeStorageSetItem(
                env,
                ptr::null_mut(),
                init_c.as_ptr() as JString,
                key_c.as_ptr() as JString,
                val_c.as_ptr() as JString,
                0,
            );
            assert_eq!(saved, 1);

            Java_app_manta_browser_core_MantaNative_nativeClearAllCustomStorageAndRam(
                env,
                ptr::null_mut(),
            );
            assert_eq!(
                Java_app_manta_browser_core_MantaNative_nativeGetCustomStorageBytes(
                    env,
                    ptr::null_mut()
                ),
                0
            );
        }
    }
}
