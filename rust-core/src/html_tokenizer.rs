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
    Comment,
    Doctype,
    RawText,
}

/// Streaming HTML5 Tokenizer designed for chunk-by-chunk ingestion:
/// - Preserves multi-byte UTF-8 sequences cleanly across 4 KB network chunk boundaries.
/// - Switches to `RawText` mode inside `<script>` and `<style>` tags so `<` operators
///   in JavaScript or CSS never corrupt the DOM tree.
/// - Decodes HTML5 named, decimal, and hexadecimal character entities (`&amp;`, `&lt;`, `&#x2022;`, etc.).
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
    comment_tail: [u8; 2],
    utf8_Carry: Vec<u8>,
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
            comment_tail: [0, 0],
            utf8_Carry: Vec::with_capacity(4),
        }
    }

    /// Feeds a byte chunk from the network stream and emits parsed tokens immediately.
    pub fn feed_chunk<F>(&mut self, chunk: &[u8], is_eof: bool, mut on_token: F)
    where
        F: FnMut(HtmlToken),
    {
        let combined: Vec<u8>;
        let input_bytes: &[u8] = if self.utf8_Carry.is_empty() {
            chunk
        } else {
            let mut tmp = std::mem::take(&mut self.utf8_Carry);
            tmp.extend_from_slice(chunk);
            combined = tmp;
            &combined
        };

        let valid_text = match std::str::from_utf8(input_bytes) {
            Ok(s) => s,
            Err(e) => {
                let valid_up_to = e.valid_up_to();
                if !is_eof && e.error_len().is_none() {
                    self.utf8_Carry.extend_from_slice(&input_bytes[valid_up_to..]);
                }
                std::str::from_utf8(&input_bytes[..valid_up_to]).unwrap_or("")
            }
        };

        for ch in valid_text.chars() {
            let b = if ch.is_ascii() { ch as u8 } else { 0 };
            match self.state {
                LexState::Data => {
                    if b == b'<' {
                        self.flush_text(&mut on_token);
                        self.state = LexState::TagOpen;
                    } else {
                        self.text_buf.push(ch);
                    }
                }
                LexState::RawText => {
                    self.text_buf.push(ch);
                    let close_seq = format!("</{}>", self.rawtext_end_tag);
                    if self.text_buf.len() >= close_seq.len() {
                        let tail_start = self.text_buf.len() - close_seq.len();
                        if self.text_buf[tail_start..].eq_ignore_ascii_case(&close_seq) {
                            let content = self.text_buf[..tail_start].to_string();
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
                        self.comment_tail = [0, 0];
                        self.state = LexState::Comment;
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
                LexState::Comment => {
                    if b == b'>'
                        && ((self.comment_tail[0] == b'-' && self.comment_tail[1] == b'-')
                            || self.comment_tail[1] == 0)
                    {
                        self.state = LexState::Data;
                    } else {
                        self.comment_tail[0] = self.comment_tail[1];
                        self.comment_tail[1] = b;
                    }
                }
                LexState::Doctype => {
                    if b == b'>' {
                        self.state = LexState::Data;
                    }
                }
            }
        }

        if is_eof {
            self.flush_text(&mut on_token);
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
                && (name == "script" || name == "style" || name == "textarea");
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
        return char::from_u32(code);
    }
    if let Some(dec) = entity.strip_prefix('#') {
        let code = dec.parse::<u32>().ok()?;
        return char::from_u32(code);
    }
    match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "bull" => Some('•'),
        "hellip" => Some('…'),
        _ => None,
    }
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
    fn test_script_rawtext_with_comparison_operators_and_entities() {
        let mut tok = StreamingHtmlTokenizer::new();
        let mut tokens = Vec::new();
        let html = b"<h1>R&amp;D &bull; 100%</h1><script>if (a < b && c > d) { x = 1; }</script><p>Done</p>";
        tok.feed_chunk(html, true, |t| tokens.push(t));

        assert!(tokens.contains(&HtmlToken::Text("R&D • 100%".to_string())));
        assert!(tokens.contains(&HtmlToken::Text(
            "if (a < b && c > d) { x = 1; }".to_string()
        )));
        assert!(tokens.contains(&HtmlToken::Text("Done".to_string())));
    }
}
