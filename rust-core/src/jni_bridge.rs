use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr;

use crate::MantaEngine;

// =============================================================================
// ZERO-DEPENDENCY RAW JNI C-ABI DEFINITIONS
// Matches the exact `const struct JNINativeInterface_ **` ABI from `jni.h`.
// Index 167 = NewStringUTF, 168 = GetStringUTFLength,
// Index 169 = GetStringUTFChars, 170 = ReleaseStringUTFChars.
// =============================================================================
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

/// Initializes Manta Rust Engine and returns the initial tab ID.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeInitEngine(
    _env: JNIEnv,
    _class: JClass,
    dev_cors_permissive: JBoolean,
) -> JInt {
    let Ok(mut engine) = MantaEngine::global().lock() else {
        return 1;
    };
    engine.config.dev_cors_permissive = dev_cors_permissive != 0;
    engine
        .modules
        .set_module_enabled("cors_dev", dev_cors_permissive != 0);
    engine.tabs.create_tab() as JInt
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
    engine.tabs.create_tab() as JInt
}

/// Switches active tab and automatically hibernates the previous tab in Rust RAM.
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

/// Toggles Developer CORS Permissive mode at runtime.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSetCorsDevMode(
    _env: JNIEnv,
    _class: JClass,
    enabled: JBoolean,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        let active = enabled != 0;
        engine.config.dev_cors_permissive = active;
        engine.modules.set_module_enabled("cors_dev", active);
    }
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

    let Ok(mut engine) = MantaEngine::global().lock() else {
        return unsafe { env.new_java_string("MANTA_ERR|Engine Lock Poisoned") };
    };
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
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeSanitizeAndCheckUrl(
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

/// Copies text into Manta's size-capped, zeroizable Rust ClipboardSandbox.
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
    if engine.clipboard.copy_text(&text) {
        1
    } else {
        0
    }
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

/// Closes a tab and immediately zeroizes its RAM buffers.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativeCloseTabAndZeroize(
    _env: JNIEnv,
    _class: JClass,
    tab_id: JInt,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        let engine_ref = &mut *engine;
        engine_ref
            .tabs
            .close_tab(tab_id as u32, &mut engine_ref.vault);
    }
}

/// Called when the user closes Manta or taps Panic Purge: cryptographically zeroes all RAM.
#[no_mangle]
pub unsafe extern "system" fn Java_app_manta_browser_core_MantaNative_nativePurgeAllVolatileRam(
    _env: JNIEnv,
    _class: JClass,
) {
    if let Ok(mut engine) = MantaEngine::global().lock() {
        engine.purge_all_volatile_state();
    }
}

/// Returns current total memory used by all active tabs and the volatile vault in bytes.
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
    fn test_jni_bridge_roundtrip_and_zeroize() {
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
            let tab_id =
                Java_app_manta_browser_core_MantaNative_nativeInitEngine(env, ptr::null_mut(), 1);
            assert!(tab_id >= 1);

            let url_c = CString::new("https://example.com/?utm_source=ads&ok=1").unwrap();
            let clean_jstr = Java_app_manta_browser_core_MantaNative_nativeSanitizeAndCheckUrl(
                env,
                ptr::null_mut(),
                url_c.as_ptr() as JString,
            );
            let clean_owned = CString::from_raw(clean_jstr as *mut c_char);
            assert_eq!(clean_owned.to_str().unwrap(), "https://example.com/?ok=1");

            Java_app_manta_browser_core_MantaNative_nativePurgeAllVolatileRam(env, ptr::null_mut());
            let ram = Java_app_manta_browser_core_MantaNative_nativeGetEngineRamBytes(
                env,
                ptr::null_mut(),
            );
            assert_eq!(ram, 0);
        }
    }
}
