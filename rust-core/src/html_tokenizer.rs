use crate::cpp_bridge::scan_html_delim;

/// Events emitted by Manta's streaming HTML5 state-machine tokenizer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlToken {
    StartTag {
        name: String,
        attributes: Vec<(String, String)>,
        self_closing: bool,
    },
    EndTag {
        name: String,
    },
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LexState {
    Data,
    TagOpen,
    EndTagOpen,
    TagName,
    BeforeAttrName,
    AttrName,
    AfterAttrName,
    BeforeAttrValue,
    AttrValueQuoted(u8),
    AttrValueUnquoted,
    SelfClosingStartTag,
    CommentDash1,
    CommentDash2,
    CommentBody,
    CommentEndDash1,
    CommentEndDash2,
    Doctype,
    RawText,
}

/// High-Speed Streaming HTML5 Tokenizer with C++ SIMD Delimiter Scanning:
/// - Jumps across plain text runs via SSE2/NEON `manta_cpp_scan_html_delim`.
/// - Zero-allocation `RawText` closing tag matching for `<script>`, `<style>`, `<textarea>`, `<title>`.
/// - Strict `<!-- ... -->` comment parsing (`>` inside comments never leaks markup).
/// - Comprehensive HTML5 + Windows-1252 named/decimal/hex character entity decoding.
pub struct StreamingHtmlTokenizer {
    state: LexState,
    current_tag_name: String,
    current_is_end_tag: bool,
    current_self_closing: bool,
    current_attrs: Vec<(String, String)>,
    current_attr_name: String,
    current_attr_val: String,
    text_buf: String,
    rawtext_end_tag: String,
    utf8_carry: Vec<u8>,
}

impl Default for StreamingHtmlTokenizer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingHtmlTokenizer {
    pub fn new() -> Self {
        Self {
            state: LexState::Data,
            current_tag_name: String::with_capacity(16),
            current_is_end_tag: false,
            current_self_closing: false,
            current_attrs: Vec::with_capacity(4),
            current_attr_name: String::with_capacity(16),
            current_attr_val: String::with_capacity(32),
            text_buf: String::with_capacity(128),
            rawtext_end_tag: String::new(),
            utf8_carry: Vec::with_capacity(4),
        }
    }

    pub fn shrink_buffers(&mut self) {
        self.text_buf.clear();
        self.text_buf.shrink_to_fit();
        self.current_tag_name.clear();
        self.current_tag_name.shrink_to_fit();
        self.current_attr_name.clear();
        self.current_attr_name.shrink_to_fit();
        self.current_attr_val.clear();
        self.current_attr_val.shrink_to_fit();
        self.current_attrs.clear();
        self.current_attrs.shrink_to_fit();
        self.rawtext_end_tag.clear();
        self.rawtext_end_tag.shrink_to_fit();
    }

    /// Feeds a byte chunk from the network stream and emits parsed tokens immediately.
    pub fn feed_chunk<F>(&mut self, chunk: &[u8], is_eof: bool, mut on_token: F)
    where
        F: FnMut(HtmlToken),
    {
        let combined: Vec<u8>;
        let input_bytes: &[u8] = if self.utf8_carry.is_empty() {
            chunk
        } else {
            let mut tmp = std::mem::take(&mut self.utf8_carry);
            tmp.extend_from_slice(chunk);
            combined = tmp;
            &combined
        };

        let valid_text = match std::str::from_utf8(input_bytes) {
            Ok(s) => s,
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                if !is_eof && e.error_len().is_none() {
                    self.utf8_carry.extend_from_slice(&input_bytes[valid_up_to..]);
                }
                std::str::from_utf8(&input_bytes[..valid_up_to]).unwrap_or("")
            }
        };

        let raw_bytes = valid_text.as_bytes();
        let mut byte_idx = 0usize;

        while byte_idx < raw_bytes.len() {
            // Fast path: SIMD skip straight to next '<' in Data state
            if self.state == LexState::Data {
                let mut scan_pos = byte_idx;
                loop {
                    let next_delim = scan_html_delim(raw_bytes, scan_pos);
                    if next_delim >= raw_bytes.len() {
                        self.text_buf.push_str(&valid_text[byte_idx..]);
                        byte_idx = raw_bytes.len();
                        break;
                    }
                    if raw_bytes[next_delim] == b'<' {
                        if next_delim > byte_idx {
                            self.text_buf.push_str(&valid_text[byte_idx..next_delim]);
                        }
                        self.flush_text(&mut on_token);
                        self.state = LexState::TagOpen;
                        byte_idx = next_delim + 1;
                        break;
                    }
                    // Found '&': advance past '&' and keep SIMD scanning
                    scan_pos = next_delim + 1;
                }
                continue;
            }

            let ch = valid_text[byte_idx..].chars().next().unwrap();
            byte_idx += ch.len_utf8();
            let b = if ch.is_ascii() { ch as u8 } else { 0 };

            match self.state {
                LexState::Data => {}
                LexState::RawText => {
                    self.text_buf.push(ch);
                    if b == b'>' {
                        if let Some(tag_start) = match_rawtext_close(&self.text_buf, &self.rawtext_end_tag) {
                            let content = self.text_buf[..tag_start].to_string();
                            self.text_buf.clear();
                            if !content.trim().is_empty() {
                                on_token(HtmlToken::Text(content));
                            }
                            let end_name = std::mem::take(&mut self.rawtext_end_tag);
                            on_token(HtmlToken::EndTag { name: end_name });
                            self.state = LexState::Data;
                        }
                    }
                }
                LexState::TagOpen => {
                    if b == b'/' {
                        self.current_is_end_tag = true;
                        self.current_tag_name.clear();
                        self.state = LexState::EndTagOpen;
                    } else if b == b'!' {
                        self.state = LexState::CommentDash1;
                    } else if b == b'?' {
                        self.state = LexState::Doctype;
                    } else if ch.is_ascii_alphabetic() {
                        self.current_is_end_tag = false;
                        self.current_self_closing = false;
                        self.current_tag_name.clear();
                        self.current_attrs.clear();
                        self.current_tag_name.push(ch.to_ascii_lowercase());
                        self.state = LexState::TagName;
                    } else {
                        self.text_buf.push('<');
                        self.text_buf.push(ch);
                        self.state = LexState::Data;
                    }
                }
                LexState::CommentDash1 => {
                    if b == b'-' {
                        self.state = LexState::CommentDash2;
                    } else if b == b'>' {
                        self.state = LexState::Data;
                    } else {
                        self.state = LexState::Doctype;
                    }
                }
                LexState::CommentDash2 => {
                    if b == b'-' {
                        self.state = LexState::CommentBody;
                    } else if b == b'>' {
                        self.state = LexState::Data;
                    } else {
                        self.state = LexState::Doctype;
                    }
                }
                LexState::CommentBody => {
                    if b == b'-' {
                        self.state = LexState::CommentEndDash1;
                    }
                }
                LexState::CommentEndDash1 => {
                    if b == b'-' {
                        self.state = LexState::CommentEndDash2;
                    } else {
                        self.state = LexState::CommentBody;
                    }
                }
                LexState::CommentEndDash2 => {
                    if b == b'>' || b == b'!' {
                        if b == b'>' {
                            self.state = LexState::Data;
                        }
                    } else if b == b'-' {
                        // Stay in CommentEndDash2 for `--->`
                    } else {
                        self.state = LexState::CommentBody;
                    }
                }
                LexState::Doctype => {
                    if b == b'>' {
                        self.state = LexState::Data;
                    }
                }
                LexState::EndTagOpen => {
                    if ch.is_ascii_alphabetic() {
                        self.current_tag_name.push(ch.to_ascii_lowercase());
                        self.state = LexState::TagName;
                    } else if b == b'>' {
                        self.state = LexState::Data;
                    } else {
                        self.state = LexState::Doctype;
                    }
                }
                LexState::TagName => {
                    if ch.is_ascii_whitespace() {
                        self.state = LexState::BeforeAttrName;
                    } else if b == b'/' {
                        self.state = LexState::SelfClosingStartTag;
                    } else if b == b'>' {
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.current_tag_name.push(ch.to_ascii_lowercase());
                    }
                }
                LexState::BeforeAttrName => {
                    if ch.is_ascii_whitespace() {
                        continue;
                    } else if b == b'/' {
                        self.state = LexState::SelfClosingStartTag;
                    } else if b == b'>' {
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.current_attr_name.clear();
                        self.current_attr_val.clear();
                        self.current_attr_name.push(ch.to_ascii_lowercase());
                        self.state = LexState::AttrName;
                    }
                }
                LexState::AttrName => {
                    if ch.is_ascii_whitespace() {
                        self.state = LexState::AfterAttrName;
                    } else if b == b'=' {
                        self.state = LexState::BeforeAttrValue;
                    } else if b == b'/' {
                        self.push_current_attr();
                        self.state = LexState::SelfClosingStartTag;
                    } else if b == b'>' {
                        self.push_current_attr();
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.current_attr_name.push(ch.to_ascii_lowercase());
                    }
                }
                LexState::AfterAttrName => {
                    if ch.is_ascii_whitespace() {
                        continue;
                    } else if b == b'=' {
                        self.state = LexState::BeforeAttrValue;
                    } else if b == b'/' {
                        self.push_current_attr();
                        self.state = LexState::SelfClosingStartTag;
                    } else if b == b'>' {
                        self.push_current_attr();
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.push_current_attr();
                        self.current_attr_name.clear();
                        self.current_attr_val.clear();
                        self.current_attr_name.push(ch.to_ascii_lowercase());
                        self.state = LexState::AttrName;
                    }
                }
                LexState::BeforeAttrValue => {
                    if ch.is_ascii_whitespace() {
                        continue;
                    } else if b == b'"' || b == b'\'' {
                        self.state = LexState::AttrValueQuoted(b);
                    } else if b == b'>' {
                        self.push_current_attr();
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.current_attr_val.push(ch);
                        self.state = LexState::AttrValueUnquoted;
                    }
                }
                LexState::AttrValueQuoted(quote_byte) => {
                    if b == quote_byte {
                        self.push_current_attr();
                        self.state = LexState::BeforeAttrName;
                    } else {
                        self.current_attr_val.push(ch);
                    }
                }
                LexState::AttrValueUnquoted => {
                    if ch.is_ascii_whitespace() {
                        self.push_current_attr();
                        self.state = LexState::BeforeAttrName;
                    } else if b == b'>' {
                        self.push_current_attr();
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.current_attr_val.push(ch);
                    }
                }
                LexState::SelfClosingStartTag => {
                    if b == b'>' {
                        self.current_self_closing = true;
                        self.emit_current_tag(&mut on_token);
                    } else {
                        self.state = LexState::BeforeAttrName;
                    }
                }
            }
        }

        if is_eof {
            self.flush_text(&mut on_token);
            self.shrink_buffers();
        }
    }

    fn push_current_attr(&mut self) {
        if !self.current_attr_name.is_empty() {
            let decoded_val = decode_html_entities(&self.current_attr_val);
            self.current_attr_val.clear();
            self.current_attrs
                .push((std::mem::take(&mut self.current_attr_name), decoded_val));
        }
    }

    fn emit_current_tag<F>(&mut self, on_token: &mut F)
    where
        F: FnMut(HtmlToken),
    {
        let name = std::mem::take(&mut self.current_tag_name);
        if name.is_empty() {
            self.state = LexState::Data;
            return;
        }
        if self.current_is_end_tag {
            on_token(HtmlToken::EndTag { name });
            self.state = LexState::Data;
        } else {
            let is_void = matches!(
                name.as_str(),
                "meta"
                    | "link"
                    | "img"
                    | "br"
                    | "hr"
                    | "input"
                    | "source"
                    | "area"
                    | "base"
                    | "col"
                    | "embed"
                    | "param"
                    | "track"
                    | "wbr"
            );
            let is_rawtext = !self.current_self_closing
                && (name == "script" || name == "style" || name == "textarea" || name == "title");
            if is_rawtext {
                self.rawtext_end_tag = name.clone();
            }
            on_token(HtmlToken::StartTag {
                name,
                attributes: std::mem::take(&mut self.current_attrs),
                self_closing: self.current_self_closing || is_void,
            });
            self.state = if is_rawtext {
                LexState::RawText
            } else {
                LexState::Data
            };
        }
    }

    fn flush_text<F>(&mut self, on_token: &mut F)
    where
        F: FnMut(HtmlToken),
    {
        if !self.text_buf.is_empty() {
            let raw = std::mem::take(&mut self.text_buf);
            let decoded = decode_html_entities(&raw);
            let collapsed = collapse_whitespace(&decoded);
            if !collapsed.is_empty() {
                on_token(HtmlToken::Text(collapsed));
            }
        }
    }
}

/// Zero-allocation check for closing rawtext tag (`</script>`, `</style>`, `</textarea>`, `</title>`).
fn match_rawtext_close(buf: &str, tag_name: &str) -> Option<usize> {
    let bytes = buf.as_bytes();
    if bytes.is_empty() || bytes[bytes.len() - 1] != b'>' {
        return None;
    }
    let mut end = bytes.len() - 1;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let tag_bytes = tag_name.as_bytes();
    let needed = 2 + tag_bytes.len();
    if end < needed {
        return None;
    }
    let start = end - needed;
    if bytes[start] == b'<'
        && bytes[start + 1] == b'/'
        && bytes[start + 2..end].eq_ignore_ascii_case(tag_bytes)
    {
        Some(start)
    } else {
        None
    }
}

pub fn decode_html_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp_idx) = rest.find('&') {
        out.push_str(&rest[..amp_idx]);
        let after_amp = &rest[amp_idx + 1..];
        if let Some(semi_idx) = after_amp.find(';') {
            if semi_idx <= 12 {
                let entity = &after_amp[..semi_idx];
                if let Some(decoded_ch) = resolve_entity(entity) {
                    out.push(decoded_ch);
                    rest = &after_amp[semi_idx + 1..];
                    continue;
                }
            }
        }
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    out
}

fn resolve_entity(entity: &str) -> Option<char> {
    if let Some(hex) = entity.strip_prefix("#x").or_else(|| entity.strip_prefix("#X")) {
        let code = u32::from_str_radix(hex, 16).ok()?;
        return map_windows1252_or_unicode(code);
    }
    if let Some(dec) = entity.strip_prefix('#') {
        let code = dec.parse::<u32>().ok()?;
        return map_windows1252_or_unicode(code);
    }
    match entity {
        "amp" | "AMP" => Some('&'),
        "lt" | "LT" => Some('<'),
        "gt" | "GT" => Some('>'),
        "quot" | "QUOT" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "copy" | "COPY" => Some('©'),
        "reg" | "REG" => Some('®'),
        "trade" => Some('™'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "bull" => Some('•'),
        "hellip" => Some('…'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        "laquo" => Some('«'),
        "raquo" => Some('»'),
        "euro" => Some('€'),
        "pound" => Some('£'),
        "yen" => Some('¥'),
        "cent" => Some('¢'),
        "sect" => Some('§'),
        "deg" => Some('°'),
        "plusmn" => Some('±'),
        "times" => Some('×'),
        "divide" => Some('÷'),
        "middot" => Some('·'),
        "frac12" => Some('½'),
        "frac14" => Some('¼'),
        "frac34" => Some('¾'),
        "larr" => Some('←'),
        "rarr" => Some('→'),
        "uarr" => Some('↑'),
        "darr" => Some('↓'),
        _ => None,
    }
}

fn map_windows1252_or_unicode(code: u32) -> Option<char> {
    let mapped = match code {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        0 => 0xFFFD,
        _ => code,
    };
    char::from_u32(mapped)
}

fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_space = false;
    for c in input.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_rawtext_comments_with_gt_and_entities() {
        let mut tok = StreamingHtmlTokenizer::new();
        let mut tokens = Vec::new();
        let html = b"<!-- hidden a > b --><title>Manta &rsquo;26</title><h1>R&amp;D &bull; 100%</h1><script>if (a < b && c > d) { x = 1; }</script><p>Done</p>";
        tok.feed_chunk(html, true, |t| tokens.push(t));

        assert!(!tokens.contains(&HtmlToken::Text("b -->".to_string())));
        assert!(tokens.contains(&HtmlToken::Text("Manta ’26".to_string())));
        assert!(tokens.contains(&HtmlToken::Text("R&D • 100%".to_string())));
        assert!(tokens.contains(&HtmlToken::Text(
            "if (a < b && c > d) { x = 1; }".to_string()
        )));
        assert!(tokens.contains(&HtmlToken::Text("Done".to_string())));
    }
}
