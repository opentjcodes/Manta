use std::ffi::{c_char, CStr, CString};
use std::ptr;
use std::slice;

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

/// Initializes Manta Core v2.0 via the Universal Bolt-On C-ABI.
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
        Ok(frame) => rust_to_cstr(&frame.to_compact_wire_string()),
        Err(e) => rust_to_cstr(&format!("MANTA_ERR|{}", e)),
    }
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
    engine.tabs.set_input_value(tab_id, node_id, &val_str).is_ok()
}

#[no_mangle]
pub extern "C" fn manta_abi_toggle_checkable(tab_id: u32, node_id: u32) -> bool {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return false;
    };
    engine.tabs.toggle_checkable(tab_id, node_id).unwrap_or(false)
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
        let ok = engine.storage.set_local_storage(&orig, &k, &v);
        if ok {
            let _ = engine.storage.save_to_backing_file();
        }
        ok
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
    let ok = engine.storage.db.save_virtual_file(&orig, &vpath, &mime, slice);
    if ok {
        let _ = engine.storage.save_to_backing_file();
    }
    ok
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
    let ok = engine.storage.clear_origin(&orig);
    if ok {
        let _ = engine.storage.save_to_backing_file();
    }
    ok
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
    let Ok(engine) = MantaEngine::global().lock() else {
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
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 0;
    };
    let engine_ref = &mut *engine;
    engine_ref
        .downloads
        .start_or_resume_download(&u, &name, &engine_ref.modules)
        .unwrap_or(0)
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
    engine.controls.settings.set_by_key(&k, &v)
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
