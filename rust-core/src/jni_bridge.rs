use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use crate::MantaEngine;

// =============================================================================
// ZERO-DEPENDENCY RAW JNI C-ABI DEFINITIONS
// Eliminates the external `jni` crate for ultra-fast builds and tiny `.so` size.
// =============================================================================
#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct JNIEnv(*const JNINativeInterface);

pub type JObject = *mut c_void;
pub type JClass = *mut c_void;
pub type JString = *mut c_void;
pub type JBoolean = u8;
pub type JInt = i32;

#[repr(C)]
pub struct JNINativeInterface {
    reserved: [*const c_void; 165],
    new_string_utf: unsafe extern "system" fn(JNIEnv, *const c_char) -> JString,
    get_string_utf_length: unsafe extern "system" fn(JNIEnv, JString) -> JInt,
    get_string_utf_chars: unsafe extern "system" fn(JNIEnv, JString, *mut JBoolean) -> *const c_char,
    release_string_utf_chars: unsafe extern "system" fn(JNIEnv, JString, *const c_char),
}

impl JNIEnv {
    unsafe fn get_rust_string(self, jstr: JString) -> String {
        if jstr.is_null() || self.0.is_null() {
            return String::new();
        }
        let table = unsafe { &*(*self.0) };
        let mut is_copy: JBoolean = 0;
        let ptr = unsafe { (table.get_string_utf_chars)(self, jstr, &mut is_copy) };
        if ptr.is_null() {
            return String::new();
        }
        let rust_str = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        unsafe { (table.release_string_utf_chars)(self, jstr, ptr) };
        rust_str
    }

    unsafe fn new_java_string(self, text: &str) -> JString {
        if self.0.is_null() {
            return ptr::null_mut();
        }
        let table = unsafe { &*(*self.0) };
        let sanitized = text.replace('\0', "");
        match CString::new(sanitized) {
            Ok(cstr) => unsafe { (table.new_string_utf)(self, cstr.as_ptr()) },
            Err(_) => ptr::null_mut(),
        }
    }
}

/// Initializes Manta Rust Engine and returns the initial tab ID.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_initEngine(
    _env: JNIEnv,
    _class: JClass,
    dev_cors_permissive: JBoolean,
) -> JInt {
    let mut engine = MantaEngine::global().lock().unwrap();
    engine.config.dev_cors_permissive = dev_cors_permissive != 0;
    engine
        .modules
        .set_module_enabled("cors_dev", dev_cors_permissive != 0);
    engine.tabs.create_tab() as JInt
}

/// Streams a chunk of HTML/data into a tab and returns the serialized `MANTA_FRAME` DisplayList.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_streamHtmlChunk(
    env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
    j_url: JString,
    j_html_chunk: JString,
    is_eof: JBoolean,
) -> JString {
    let url = unsafe { env.get_rust_string(j_url) };
    let chunk_str = unsafe { env.get_rust_string(j_html_chunk) };

    let mut engine = MantaEngine::global().lock().unwrap();
    let engine_ref = &mut *engine;

    match engine_ref.tabs.load_stream_chunk(
        tab_id as u32,
        &url,
        chunk_str.as_bytes(),
        is_eof != 0,
        &mut engine_ref.modules,
        &mut engine_ref.vault,
    ) {
        Ok(frame) => unsafe { env.new_java_string(&frame.to_compact_wire_string()) },
        Err(e) => unsafe { env.new_java_string(&format!("MANTA_ERR|{}", e)) },
    }
}

/// Sanitizes a URL (strips `utm_*`, `fbclid`, `gclid`, etc.) and checks tracker blocklist in Rust.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_sanitizeAndCheckUrl(
    env: JNIEnv,
    _class: JClass,
    j_url: JString,
) -> JString {
    let raw = unsafe { env.get_rust_string(j_url) };
    if crate::shield::PrivacyShieldModule::is_blocked_domain(&raw) {
        return unsafe { env.new_java_string("BLOCKED") };
    }
    let clean = crate::shield::PrivacyShieldModule::sanitize_url(&raw);
    unsafe { env.new_java_string(&clean) }
}

/// Closes a tab and immediately zeroizes its RAM buffers.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_closeTabAndZeroize(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) {
    let mut engine = MantaEngine::global().lock().unwrap();
    let engine_ref = &mut *engine;
    engine_ref
        .tabs
        .close_tab(tab_id as u32, &mut engine_ref.vault);
}

/// Called when the user closes Manta or taps Panic Purge: cryptographically zeroes all RAM.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_purgeAllVolatileRam(
    _env: JNIEnv,
    _class: JClass,
) {
    let mut engine = MantaEngine::global().lock().unwrap();
    engine.purge_all_volatile_state();
}

/// Returns current total memory used by all active tabs and the volatile vault in bytes.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_getEngineRamBytes(
    _env: JNIEnv,
    _class: JClass,
) -> JInt {
    let engine = MantaEngine::global().lock().unwrap();
    (engine.tabs.total_allocated_bytes() + engine.vault.used_bytes()) as JInt
}
