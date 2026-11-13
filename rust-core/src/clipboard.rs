use crate::cpp_bridge::secure_zero_string;

/// Sandboxed, size-capped Clipboard Manager in Safe Rust + C++ volatile zeroing.
/// Supports user-initiated copy/paste & selection extraction with cryptographic clear.
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
        secure_zero_string(&mut self.buffer);
        self.buffer.push_str(text);
        true
    }

    pub fn paste_text(&self) -> &str {
        &self.buffer
    }

    pub fn clear(&mut self) {
        secure_zero_string(&mut self.buffer);
        self.buffer.shrink_to_fit();
    }
}

impl Drop for ClipboardSandbox {
    fn drop(&mut self) {
        secure_zero_string(&mut self.buffer);
    }
}
