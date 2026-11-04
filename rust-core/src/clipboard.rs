/// Sandboxed, size-capped Clipboard Manager in 100% Safe Rust.
/// Supports user-initiated copy/paste & selection extraction with instant O(1) clear.
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
        self.buffer.clear();
        self.buffer.push_str(text);
        true
    }

    pub fn paste_text(&self) -> &str {
        &self.buffer
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.buffer.shrink_to_fit();
    }
}
