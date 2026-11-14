use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use crate::module_bus::ModuleBus;
use crate::page_compiler::PageRuntimeCompiler;
use crate::site_isolation::{IsolationVerdict, SiteIsolationModule};
use crate::tab_compressor::TabCompressor;
use crate::tab_manager::TabManager;
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

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeNavigateUrl(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    j_url: JString,
) -> JString {
    let raw_url = unsafe { env.get_rust_string(j_url) };
    let (resolved_url, cookie_hdr, max_bytes) = {
        let Ok(engine) = MantaEngine::global().lock() else {
            return unsafe { env.new_java_string("MANTA_ERR|Engine Lock Poisoned") };
        };
        let resolved = engine.controls.settings.resolve_omnibox_input(&raw_url);
        let cookies = engine.storage.get_cookie_header_for_url(&resolved, true);
        (resolved, cookies, engine.config.per_tab_ram_ceiling_bytes)
    };

    let mut standalone_modules = ModuleBus::new();
    standalone_modules.register(Box::new(SiteIsolationModule::new()));

    let bundle = match TabManager::fetch_page_bundle(
        &resolved_url,
        &cookie_hdr,
        &standalone_modules,
        max_bytes,
    ) {
        Ok(b) => b,
        Err(e) => return unsafe { env.new_java_string(&format!("MANTA_ERR|{}", e)) },
    };

    let Ok(mut engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("MANTA_ERR|Engine Lock Poisoned") };
    };
    let engine_ref = &mut *engine;
    let final_url = bundle.final_url.clone();
    match engine_ref.tabs.apply_fetched_page_bundle(
        tab_id as u32,
        bundle,
        &mut engine_ref.modules,
        &mut engine_ref.vault,
        &mut engine_ref.storage,
        &mut engine_ref.downloads,
    ) {
        Ok(frame) => {
            engine_ref
                .controls
                .ensure_tab_state(tab_id as u32)
                .record_navigation(&final_url, &frame.page_title);
            unsafe { env.new_java_string(&frame.to_compact_wire_string()) }
        }
        Err(e) => unsafe { env.new_java_string(&format!("MANTA_ERR|{}", e)) },
    }
}

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
    if let Some(tab) = engine.tabs.get_tab_mut(tab_id as u32) {
        tab.scroll_y = scroll_y;
        tab.is_hibernated = false;
    }
    engine.storage.record_history_visit(&url, &title);
    engine
        .storage
        .save_tab_session_state(tab_id as u32, &url, &title, scroll_y);
    TabCompressor::store_tab_snapshot(&mut engine.vault, tab_id as u32, &url, &title, scroll_y)
        as JInt
}

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
        None => {
            if let Some(tab) = engine.tabs.get_tab(tab_id as u32) {
                let formatted =
                    format!("{}|{}|{}", tab.scroll_y, tab.dom.page_title, tab.current_url);
                unsafe { env.new_java_string(&formatted) }
            } else {
                unsafe { env.new_java_string("") }
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeListSavedTabsJson(
    env: JNIEnv,
    _class: JClass,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("[]") };
    };
    let json = engine.storage.export_saved_tabs_json();
    unsafe { env.new_java_string(&json) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSavePageOffline(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
    j_html: JString,
) -> JBoolean {
    let url = unsafe { env.get_rust_string(j_url) };
    let html = unsafe { env.get_rust_string(j_html) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.storage.save_offline_page_snapshot(&url, html.as_bytes()))
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeLoadPageOffline(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
) -> JString {
    let url = unsafe { env.get_rust_string(j_url) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    let html = engine
        .storage
        .load_offline_page_snapshot(&url)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    unsafe { env.new_java_string(&html) }
}

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
    u8::from(engine_ref.tabs.switch_to_tab(tab_id as u32, &mut engine_ref.vault))
}

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
            .set_zoom_percent(tab_id as u32, zoom_percent.max(25) as u16);
    }
}

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

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeResolveOmnibox(
    env: JNIEnv,
    _class: JClass,
    j_input: JString,
) -> JString {
    let raw = unsafe { env.get_rust_string(j_input) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    let resolved = engine.controls.settings.resolve_omnibox_input(&raw);
    unsafe { env.new_java_string(&resolved) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeFindInPage(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    j_query: JString,
) -> JInt {
    let query = unsafe { env.get_rust_string(j_query) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    let count = engine.tabs.find_in_page(tab_id as u32, &query);
    engine.controls.update_find_in_page(tab_id as u32, &query, count);
    count as JInt
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetReaderModeText(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("") };
    };
    let txt = engine.tabs.get_reader_text(tab_id as u32);
    unsafe { env.new_java_string(&txt) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSetEphemeralMode(
    _env: JNIEnv,
    _class: JClass,
    enabled: JBoolean,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        let on = enabled != 0;
        engine.storage.set_ephemeral_mode(on);
        engine.controls.settings.ephemeral_zero_trace_mode = on;
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeGetMemoryStatsJson(
    env: JNIEnv,
    _class: JClass,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("{}") };
    };
    let json = engine.memory_stats_json();
    unsafe { env.new_java_string(&json) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeMediaOpenStream(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
    ring_bytes: JInt,
) -> JInt {
    let url = unsafe { env.get_rust_string(j_url) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine
        .media
        .open_stream(&url, ring_bytes.max(0) as u32)
        .unwrap_or(0) as JInt
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeMediaIngestManifest(
    env: JNIEnv,
    _class: JClass,
    stream_id: JInt,
    j_base_url: JString,
    j_manifest: JString,
) -> JBoolean {
    let base = unsafe { env.get_rust_string(j_base_url) };
    let text = unsafe { env.get_rust_string(j_manifest) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.media.ingest_manifest(stream_id as u32, &base, &text))
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeMediaGetStatsJson(
    env: JNIEnv,
    _class: JClass,
    stream_id: JInt,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("{}") };
    };
    let json = engine.media.get_stats_json(stream_id as u32);
    unsafe { env.new_java_string(&json) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeMediaCloseStream(
    _env: JNIEnv,
    _class: JClass,
    stream_id: JInt,
) -> JBoolean {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.media.close_stream(stream_id as u32))
}

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
    let script = PageRuntimeCompiler::compile_runtime_script_with_settings(
        &origin_url,
        &engine.storage,
        &engine.controls.settings,
    );
    unsafe { env.new_java_string(&script) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeIsChallengeUrl(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
) -> JBoolean {
    let url = unsafe { env.get_rust_string(j_url) };
    u8::from(PageRuntimeCompiler::is_verification_challenge_url(&url))
}

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
        engine.storage.set_local_storage(&origin, &key, &val)
    };
    u8::from(ok)
}

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
        } else {
            engine.storage.remove_local_storage(&origin, &key);
        }
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeStorageClearOrigin(
    env: JNIEnv,
    _class: JClass,
    j_origin_url: JString,
) {
    let origin = unsafe { env.get_rust_string(j_origin_url) };
    if let Ok(mut engine) = MantaEngine::global().lock() {
        engine.storage.clear_origin(&origin);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeInspectStorageManifest(
    env: JNIEnv,
    _class: JClass,
) -> JString {
    let Ok(engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("{}") };
    };
    let json = engine.storage.inspect_storage_manifest_json();
    unsafe { env.new_java_string(&json) }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeFlushCustomStorage(
    _env: JNIEnv,
    _class: JClass,
) -> JBoolean {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    u8::from(engine.storage.save_to_backing_file().is_ok())
}

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
    u8::from(added)
}

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
        &mut engine_ref.downloads,
    ) {
        Ok(frame) => {
            if is_eof != 0 && !url.is_empty() {
                engine_ref
                    .controls
                    .ensure_tab_state(tab_id as u32)
                    .record_navigation(&url, &frame.page_title);
            }
            unsafe { env.new_java_string(&frame.to_compact_wire_string()) }
        }
        Err(e) => unsafe { env.new_java_string(&format!("MANTA_ERR|{}", e)) },
    }
}

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
        engine_ref.storage.remove_tab_session_state(tab_id as u32);
        engine_ref.modules.dispatch_tab_closed(tab_id as u32);
    }
}

#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeClearAllCustomStorageAndRam(
    _env: JNIEnv,
    _class: JClass,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        engine.clear_all_custom_storage_and_ram();
    }
}

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
