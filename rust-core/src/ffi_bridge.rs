use std::ffi::{c_char, CStr, CString};
use std::ptr;
use std::slice;

use crate::downloads_engine::MantaDownloadManager;
use crate::module_bus::ModuleBus;
use crate::net_stream::{register_host_net_transport, MantaNetFetchCallback};
use crate::page_compiler::PageRuntimeCompiler;
use crate::site_isolation::SiteIsolationModule;
use crate::tab_manager::TabManager;
use crate::MantaEngine;

unsafe fn cstr_to_rust(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

fn rust_to_cstr(s: &str) -> *mut c_char {
    let clean = s.replace('\0', "");
    match CString::new(clean) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_init_engine(
    storage_path: *const c_char,
    download_dir: *const c_char,
) -> u32 {
    let sp = unsafe { cstr_to_rust(storage_path) };
    let dd = unsafe { cstr_to_rust(download_dir) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 1;
    };
    engine.init_with_paths(&sp, &dd)
}

#[no_mangle]
pub extern "C" fn manta_abi_get_worker_cores() -> u32 {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 2;
    };
    engine.workers.core_count() as u32
}

#[no_mangle]
pub extern "C" fn manta_abi_register_net_transport(callback: Option<MantaNetFetchCallback>) {
    register_host_net_transport(callback);
}

#[no_mangle]
pub extern "C" fn manta_abi_create_tab() -> u32 {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 1;
    };
    let id = engine.tabs.create_tab();
    engine.controls.ensure_tab_state(id);
    id
}

#[no_mangle]
pub extern "C" fn manta_abi_switch_tab(tab_id: u32) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    let engine_ref = &mut *engine;
    engine_ref.tabs.switch_to_tab(tab_id, &mut engine_ref.vault)
}

#[no_mangle]
pub extern "C" fn manta_abi_close_tab(tab_id: u32) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    let engine_ref = &mut *engine;
    let closed = engine_ref.tabs.close_tab(tab_id, &mut engine_ref.vault);
    engine_ref.controls.remove_tab(tab_id);
    engine_ref.storage.remove_tab_session_state(tab_id);
    engine_ref.modules.dispatch_tab_closed(tab_id);
    closed
}

#[no_mangle]
pub extern "C" fn manta_abi_active_tab_id() -> u32 {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine.tabs.active_tab_id().unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn manta_abi_open_tab_count() -> u32 {
    let Ok(engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine.tabs.open_tab_count() as u32
}

#[no_mangle]
pub extern "C" fn manta_abi_get_memory_stats_json() -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("{}");
    };
    rust_to_cstr(&engine.memory_stats_json())
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_compile_kernel_script(origin_url: *const c_char) -> *mut c_char {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    let script = PageRuntimeCompiler::compile_runtime_script_with_settings(
        &orig,
        &engine.storage,
        &engine.controls.settings,
    );
    rust_to_cstr(&script)
}

/// Lock-free end-to-end site navigation: fetches HTML + external CSS/JS over Keep-Alive
/// outside `MantaEngine::global().lock()`, then renders & returns the `DisplayListFrame`.
#[no_mangle]
pub unsafe extern "C" fn manta_abi_navigate_url(
    tab_id: u32,
    url: *const c_char,
) -> *mut c_char {
    let raw_url = unsafe { cstr_to_rust(url) };
    let (resolved_url, cookie_hdr, max_bytes) = {
        let Ok(engine) = MantaEngine::global().lock() else {
            return rust_to_cstr("MANTA_ERR|Engine Lock Poisoned");
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
        Err(e) => return rust_to_cstr(&format!("MANTA_ERR|{}", e)),
    };

    let Ok(mut engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("MANTA_ERR|Engine Lock Poisoned");
    };
    let engine_ref = &mut *engine;
    let final_url = bundle.final_url.clone();
    match engine_ref.tabs.apply_fetched_page_bundle(
        tab_id,
        bundle,
        &mut engine_ref.modules,
        &mut engine_ref.vault,
        &mut engine_ref.storage,
        &mut engine_ref.downloads,
    ) {
        Ok(frame) => {
            engine_ref
                .controls
                .ensure_tab_state(tab_id)
                .record_navigation(&final_url, &frame.page_title);
            rust_to_cstr(&frame.to_compact_wire_string())
        }
        Err(e) => rust_to_cstr(&format!("MANTA_ERR|{}", e)),
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_stream_html_chunk(
    tab_id: u32,
    url: *const c_char,
    html_chunk: *const c_char,
    is_eof: bool,
) -> *mut c_char {
    let url_str = unsafe { cstr_to_rust(url) };
    let chunk_str = unsafe { cstr_to_rust(html_chunk) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("MANTA_ERR|Engine Lock Poisoned");
    };
    let engine_ref = &mut *engine;
    if is_eof && !url_str.is_empty() {
        engine_ref
            .controls
            .ensure_tab_state(tab_id)
            .record_navigation(&url_str, "");
    }
    match engine_ref.tabs.load_stream_chunk(
        tab_id,
        &url_str,
        chunk_str.as_bytes(),
        is_eof,
        &mut engine_ref.modules,
        &mut engine_ref.vault,
        &mut engine_ref.storage,
        &mut engine_ref.downloads,
    ) {
        Ok(frame) => {
            if is_eof {
                engine_ref
                    .controls
                    .ensure_tab_state(tab_id)
                    .record_navigation(&url_str, &frame.page_title);
            }
            rust_to_cstr(&frame.to_compact_wire_string())
        }
        Err(e) => rust_to_cstr(&format!("MANTA_ERR|{}", e)),
    }
}

#[no_mangle]
pub extern "C" fn manta_abi_render_viewport_frame(
    tab_id: u32,
    viewport_w: f32,
    scroll_y: f32,
    viewport_h: f32,
) -> *mut c_char {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    match engine
        .tabs
        .render_viewport_frame(tab_id, viewport_w, scroll_y, viewport_h)
    {
        Some(frame) => rust_to_cstr(&frame.to_compact_wire_string()),
        None => rust_to_cstr(""),
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_resolve_omnibox(raw_input: *const c_char) -> *mut c_char {
    let raw = unsafe { cstr_to_rust(raw_input) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    rust_to_cstr(&engine.controls.settings.resolve_omnibox_input(&raw))
}

#[no_mangle]
pub extern "C" fn manta_abi_can_go_back(tab_id: u32) -> bool {
    let Ok(engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine
        .controls
        .get_tab_state(tab_id)
        .map(|s| s.can_go_back())
        .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn manta_abi_can_go_forward(tab_id: u32) -> bool {
    let Ok(engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine
        .controls
        .get_tab_state(tab_id)
        .map(|s| s.can_go_forward())
        .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn manta_abi_go_back(tab_id: u32) -> *mut c_char {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    let st = engine.controls.ensure_tab_state(tab_id);
    if let Some((url, _)) = st.go_back() {
        rust_to_cstr(url)
    } else {
        rust_to_cstr("")
    }
}

#[no_mangle]
pub extern "C" fn manta_abi_go_forward(tab_id: u32) -> *mut c_char {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    let st = engine.controls.ensure_tab_state(tab_id);
    if let Some((url, _)) = st.go_forward() {
        rust_to_cstr(url)
    } else {
        rust_to_cstr("")
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_find_in_page(tab_id: u32, query: *const c_char) -> u32 {
    let q = unsafe { cstr_to_rust(query) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    let count = engine.tabs.find_in_page(tab_id, &q);
    engine.controls.update_find_in_page(tab_id, &q, count);
    count as u32
}

#[no_mangle]
pub extern "C" fn manta_abi_get_reader_text(tab_id: u32) -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    rust_to_cstr(&engine.tabs.get_reader_text(tab_id))
}

#[no_mangle]
pub extern "C" fn manta_abi_focus_node(tab_id: u32, node_id: u32) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.tabs.focus_node(tab_id, node_id)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_set_input_value(
    tab_id: u32,
    node_id: u32,
    value: *const c_char,
) -> bool {
    let val_str = unsafe { cstr_to_rust(value) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    let engine_ref = &mut *engine;
    engine_ref
        .tabs
        .set_input_value_and_persist(tab_id, node_id, &val_str, &mut engine_ref.storage)
        .is_ok()
}

#[no_mangle]
pub extern "C" fn manta_abi_toggle_checkable(tab_id: u32, node_id: u32) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.tabs.toggle_checkable(tab_id, node_id).unwrap_or(false)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_dispatch_node_event(
    tab_id: u32,
    node_id: u32,
    event_type: *const c_char,
) -> bool {
    let ev = unsafe { cstr_to_rust(event_type) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    let engine_ref = &mut *engine;
    engine_ref.tabs.dispatch_node_event(
        tab_id,
        node_id,
        &ev,
        &mut engine_ref.storage,
        &mut engine_ref.downloads,
    )
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_attach_file_to_input(
    tab_id: u32,
    node_id: u32,
    filename: *const c_char,
    mime_type: *const c_char,
    data_ptr: *const u8,
    data_len: u32,
) -> bool {
    let fname = unsafe { cstr_to_rust(filename) };
    let mime = unsafe { cstr_to_rust(mime_type) };
    let bytes = if data_ptr.is_null() || data_len == 0 {
        Vec::new()
    } else {
        unsafe { slice::from_raw_parts(data_ptr, data_len as usize).to_vec() }
    };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine
        .tabs
        .attach_file_to_input(tab_id, node_id, &fname, &mime, bytes)
        .is_ok()
}

#[no_mangle]
pub extern "C" fn manta_abi_submit_form(tab_id: u32, form_or_control_node_id: u32) -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    match engine.tabs.submit_form(tab_id, form_or_control_node_id) {
        Ok(sub) => {
            let body_preview = String::from_utf8_lossy(&sub.body);
            let out = format!(
                "{}|{}|{}|{}",
                sub.method, sub.action_url, sub.content_type, body_preview
            );
            rust_to_cstr(&out)
        }
        Err(_) => rust_to_cstr(""),
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_submit_form_bytes(
    tab_id: u32,
    form_or_control_node_id: u32,
    out_len: *mut u32,
) -> *mut u8 {
    let Ok(engine) = MantaEngine::global().lock() else {
        if !out_len.is_null() {
            unsafe { *out_len = 0; }
        }
        return ptr::null_mut();
    };
    let Ok(sub) = engine.tabs.submit_form(tab_id, form_or_control_node_id) else {
        if !out_len.is_null() {
            unsafe { *out_len = 0; }
        }
        return ptr::null_mut();
    };
    let header = format!("{}|{}|{}\0", sub.method, sub.action_url, sub.content_type);
    let total = header.len() + sub.body.len();
    let mut buf = Vec::with_capacity(total);
    buf.extend_from_slice(header.as_bytes());
    buf.extend_from_slice(&sub.body);
    let mut boxed = buf.into_boxed_slice();
    let len = boxed.len() as u32;
    let ptr = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    if !out_len.is_null() {
        unsafe { *out_len = len; }
    }
    ptr
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_save_form_draft(
    origin_url: *const c_char,
    field_key: *const c_char,
    value: *const c_char,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let fk = unsafe { cstr_to_rust(field_key) };
    let val = unsafe { cstr_to_rust(value) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.save_form_draft(&orig, &fk, &val)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_get_form_draft(
    origin_url: *const c_char,
    field_key: *const c_char,
) -> *mut c_char {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let fk = unsafe { cstr_to_rust(field_key) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    rust_to_cstr(engine.storage.get_form_draft(&orig, &fk).as_deref().unwrap_or(""))
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_media_open_stream(
    source_url: *const c_char,
    ring_bytes_pow2: u32,
) -> u32 {
    let url = unsafe { cstr_to_rust(source_url) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine.media.open_stream(&url, ring_bytes_pow2).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_media_ingest_manifest(
    stream_id: u32,
    base_url: *const c_char,
    manifest_text: *const c_char,
) -> bool {
    let base = unsafe { cstr_to_rust(base_url) };
    let text = unsafe { cstr_to_rust(manifest_text) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.media.ingest_manifest(stream_id, &base, &text)
}

#[no_mangle]
pub extern "C" fn manta_abi_media_next_prefetch_json(stream_id: u32) -> *mut c_char {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("{}");
    };
    match engine.media.next_prefetch_request(stream_id) {
        Some((idx, url, range)) => {
            let json = format!(
                "{{\"segment_index\":{},\"url\":\"{}\",\"range\":\"{}\"}}",
                idx,
                url.replace('\\', "\\\\").replace('"', "\\\""),
                range.unwrap_or_default().replace('"', "\\\"")
            );
            rust_to_cstr(&json)
        }
        None => rust_to_cstr("{}"),
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_media_feed_bytes(
    stream_id: u32,
    data_ptr: *const u8,
    data_len: u32,
    download_ms: u32,
    rtt_ms: u32,
    seg_dur_ms: u32,
) -> u32 {
    if data_ptr.is_null() || data_len == 0 {
        return 0;
    }
    let slice = unsafe { slice::from_raw_parts(data_ptr, data_len as usize) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine.media.feed_bytes(
        stream_id,
        slice,
        download_ms as u64,
        rtt_ms,
        seg_dur_ms as u64,
    ) as u32
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_media_read_chunk(
    stream_id: u32,
    out_buf: *mut u8,
    max_len: u32,
) -> u32 {
    if out_buf.is_null() || max_len == 0 {
        return 0;
    }
    let out = unsafe { slice::from_raw_parts_mut(out_buf, max_len as usize) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine.media.read_playback_chunk(stream_id, out) as u32
}

#[no_mangle]
pub extern "C" fn manta_abi_media_seek_ms(stream_id: u32, target_ms: u64) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.media.seek_to_ms(stream_id, target_ms)
}

#[no_mangle]
pub extern "C" fn manta_abi_media_get_stats_json(stream_id: u32) -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("{}");
    };
    rust_to_cstr(&engine.media.get_stats_json(stream_id))
}

#[no_mangle]
pub extern "C" fn manta_abi_media_close_stream(stream_id: u32) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.media.close_stream(stream_id)
}

#[no_mangle]
pub extern "C" fn manta_abi_set_ephemeral_mode(enabled: bool) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.set_ephemeral_mode(enabled);
    engine.controls.settings.ephemeral_zero_trace_mode = enabled;
    true
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_set_vault_passphrase(passphrase: *const c_char) -> bool {
    let p = unsafe { cstr_to_rust(passphrase) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.set_vault_passphrase(&p);
    true
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_storage_set(
    origin_url: *const c_char,
    key: *const c_char,
    value: *const c_char,
    is_session: bool,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let k = unsafe { cstr_to_rust(key) };
    let v = unsafe { cstr_to_rust(value) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    if is_session {
        engine.storage.set_session_storage(&orig, &k, &v)
    } else {
        engine.storage.set_local_storage(&orig, &k, &v)
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_storage_get(
    origin_url: *const c_char,
    key: *const c_char,
    is_session: bool,
) -> *mut c_char {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let k = unsafe { cstr_to_rust(key) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    let val = if is_session {
        engine.storage.get_session_storage(&orig, &k)
    } else {
        engine.storage.get_local_storage(&orig, &k)
    };
    rust_to_cstr(val.as_deref().unwrap_or(""))
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_storage_remove(
    origin_url: *const c_char,
    key: *const c_char,
    is_session: bool,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let k = unsafe { cstr_to_rust(key) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    if is_session {
        engine.storage.remove_session_storage(&orig, &k)
    } else {
        engine.storage.remove_local_storage(&orig, &k)
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_cookie_set(
    origin_url: *const c_char,
    raw_cookie_header: *const c_char,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let cookie = unsafe { cstr_to_rust(raw_cookie_header) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.set_cookie_from_header(&orig, &cookie)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_cookie_get(origin_url: *const c_char) -> *mut c_char {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    rust_to_cstr(&engine.storage.get_cookie_header_for_url(&orig, false))
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_idb_put(
    origin_url: *const c_char,
    store_name: *const c_char,
    key: *const c_char,
    json_val: *const c_char,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let store = unsafe { cstr_to_rust(store_name) };
    let k = unsafe { cstr_to_rust(key) };
    let v = unsafe { cstr_to_rust(json_val) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.put_idb_object(&orig, &store, &k, &v)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_idb_get(
    origin_url: *const c_char,
    store_name: *const c_char,
    key: *const c_char,
) -> *mut c_char {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let store = unsafe { cstr_to_rust(store_name) };
    let k = unsafe { cstr_to_rust(key) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    rust_to_cstr(
        engine
            .storage
            .db
            .get_object(&orig, &store, &k)
            .as_deref()
            .unwrap_or(""),
    )
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_idb_delete(
    origin_url: *const c_char,
    store_name: *const c_char,
    key: *const c_char,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let store = unsafe { cstr_to_rust(store_name) };
    let k = unsafe { cstr_to_rust(key) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.delete_idb_object(&orig, &store, &k)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_vfs_save_file(
    origin_url: *const c_char,
    virtual_path: *const c_char,
    mime_type: *const c_char,
    data_ptr: *const u8,
    data_len: u32,
) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let vpath = unsafe { cstr_to_rust(virtual_path) };
    let mime = unsafe { cstr_to_rust(mime_type) };
    let slice = if data_ptr.is_null() || data_len == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(data_ptr, data_len as usize) }
    };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.save_vfs_file(&orig, &vpath, &mime, slice)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_save_page_offline(
    url: *const c_char,
    html_text: *const c_char,
) -> bool {
    let u = unsafe { cstr_to_rust(url) };
    let h = unsafe { cstr_to_rust(html_text) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine
        .storage
        .save_offline_page_snapshot(&u, h.as_bytes())
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_load_page_offline(url: *const c_char) -> *mut c_char {
    let u = unsafe { cstr_to_rust(url) };
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("");
    };
    let html = engine
        .storage
        .load_offline_page_snapshot(&u)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    rust_to_cstr(&html)
}

#[no_mangle]
pub extern "C" fn manta_abi_list_saved_tabs_json() -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("[]");
    };
    rust_to_cstr(&engine.storage.export_saved_tabs_json())
}

#[no_mangle]
pub extern "C" fn manta_abi_inspect_storage_manifest_json() -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("{}");
    };
    rust_to_cstr(&engine.storage.inspect_storage_manifest_json())
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_clear_origin_data(origin_url: *const c_char) -> bool {
    let orig = unsafe { cstr_to_rust(origin_url) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.clear_origin(&orig)
}

#[no_mangle]
pub extern "C" fn manta_abi_shred_all_storage_and_ram() -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.clear_all_custom_storage_and_ram();
    true
}

#[no_mangle]
pub extern "C" fn manta_abi_flush_storage() -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.storage.save_to_backing_file().is_ok()
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_start_download(
    url: *const c_char,
    suggested_filename: *const c_char,
) -> u32 {
    let u = unsafe { cstr_to_rust(url) };
    let name = unsafe { cstr_to_rust(suggested_filename) };

    let (dl_id, dl_dir, max_bytes) = {
        let Ok(mut engine) = MantaEngine::global().lock() else {
            return 0;
        };
        let id = engine.downloads.register_pending(&u, &name);
        (
            id,
            engine.downloads.download_dir().to_string(),
            engine.downloads.max_single_file_bytes(),
        )
    };

    let mut standalone_modules = ModuleBus::new();
    standalone_modules.register(Box::new(SiteIsolationModule::new()));

    match MantaDownloadManager::stream_download_standalone(
        dl_id,
        &dl_dir,
        max_bytes,
        &u,
        &name,
        &standalone_modules,
    ) {
        Ok(item) => {
            if let Ok(mut engine) = MantaEngine::global().lock() {
                engine.downloads.finalize_pending(dl_id, item);
            }
            dl_id
        }
        Err(_) => {
            if let Ok(mut engine) = MantaEngine::global().lock() {
                engine.downloads.mark_failed(dl_id);
            }
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_save_data_to_downloads(
    filename: *const c_char,
    mime_type: *const c_char,
    data_ptr: *const u8,
    data_len: u32,
) -> u32 {
    let fname = unsafe { cstr_to_rust(filename) };
    let mime = unsafe { cstr_to_rust(mime_type) };
    let slice = if data_ptr.is_null() || data_len == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(data_ptr, data_len as usize) }
    };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    engine
        .downloads
        .save_direct_bytes("manta://direct-save", &fname, &mime, slice)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn manta_abi_list_downloads_json() -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("[]");
    };
    rust_to_cstr(&engine.downloads.export_downloads_json())
}

#[no_mangle]
pub extern "C" fn manta_abi_delete_download(
    download_id: u32,
    delete_file_from_disk: bool,
) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine
        .downloads
        .delete_download(download_id, delete_file_from_disk)
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_set_setting(
    key: *const c_char,
    value: *const c_char,
) -> bool {
    let k = unsafe { cstr_to_rust(key) };
    let v = unsafe { cstr_to_rust(value) };
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    let ok = engine.controls.settings.set_by_key(&k, &v);
    if ok && k.trim() == "ephemeral_zero_trace_mode" {
        let eph = engine.controls.settings.ephemeral_zero_trace_mode;
        engine.storage.set_ephemeral_mode(eph);
    }
    ok
}

#[no_mangle]
pub extern "C" fn manta_abi_export_settings_json() -> *mut c_char {
    let Ok(engine) = MantaEngine::global().lock() else {
        return rust_to_cstr("{}");
    };
    rust_to_cstr(&engine.controls.settings.export_json())
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe {
            let _ = CString::from_raw(ptr);
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn manta_abi_free_bytes(ptr: *mut u8, len: u32) {
    if !ptr.is_null() && len > 0 {
        unsafe {
            let _ = Vec::from_raw_parts(ptr, len as usize, len as usize);
        }
    }
}
