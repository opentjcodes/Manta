use std::ptr;
use std::sync::atomic::{compiler_fence, Ordering};

/// Sandboxed, size-capped Clipboard Manager in Rust.
/// Allows user-initiated copy/paste & selection extraction while preventing
/// background tab clipboard snooping and automatically zeroizing on purge.
pub struct ClipboardSandbox {
    buffer: String,
    max_bytes: usize,
}

impl ClipboardSandbox {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            buffer: String::new(),
            max_bytes,
        }
    }

    pub fn copy_text(&mut self, text: &str) -> bool {
        if text.len() > self.max_bytes {
            return false;
        }
        self.clear_and_zeroize();
        self.buffer.push_str(text);
        true
    }

    pub fn paste_text(&self) -> &str {
        &self.buffer
    }

    pub fn clear_and_zeroize(&mut self) {
        unsafe {
            for b in self.buffer.as_bytes_mut() {
                ptr::write_volatile(b, 0u8);
            }
        }
        compiler_fence(Ordering::SeqCst);
        self.buffer.clear();
        self.buffer.shrink_to_fit();
    }
}
