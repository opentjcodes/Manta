/// Events emitted by Manta's streaming HTML state-machine tokenizer.
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
    CommentOrDoctype,
}

/// Streaming HTML5 Tokenizer designed for chunk-by-chunk ingestion.
/// Maintains minimal state (< 256 bytes) between 4 KB network chunks.
pub struct StreamingHtmlTokenizer {
    state: LexState,
    current_tag_name: String,
    current_is_end_tag: bool,
    current_self_closing: bool,
    current_attrs: Vec<(String, String)>,
    current_attr_name: String,
    current_attr_val: String,
    text_buf: String,
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
        }
    }

    /// Feeds a byte chunk from the network stream and emits parsed tokens immediately.
    pub fn feed_chunk<F>(&mut self, chunk: &[u8], is_eof: bool, mut on_token: F)
    where
        F: FnMut(HtmlToken),
    {
        for &b in chunk {
            let ch = b as char;
            match self.state {
                LexState::Data => {
                    if b == b'<' {
                        self.flush_text(&mut on_token);
                        self.state = LexState::TagOpen;
                    } else {
                        self.text_buf.push(ch);
                    }
                }
                LexState::TagOpen => {
                    if b == b'/' {
                        self.current_is_end_tag = true;
                        self.current_tag_name.clear();
                        self.state = LexState::EndTagOpen;
                    } else if b == b'!' || b == b'?' {
                        self.state = LexState::CommentOrDoctype;
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
                        self.state = LexState::CommentOrDoctype;
                    }
                }
                LexState::TagName => {
                    if ch.is_ascii_whitespace() {
                        self.state = LexState::BeforeAttrName;
                    } else if b == b'/' {
                        self.state = LexState::SelfClosingStartTag;
                    } else if b == b'>' {
                        self.emit_current_tag(&mut on_token);
                        self.state = LexState::Data;
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
                        self.state = LexState::Data;
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
                        self.state = LexState::Data;
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
                        self.state = LexState::Data;
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
                        self.state = LexState::Data;
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
                        self.state = LexState::Data;
                    } else {
                        self.current_attr_val.push(ch);
                    }
                }
                LexState::SelfClosingStartTag => {
                    if b == b'>' {
                        self.current_self_closing = true;
                        self.emit_current_tag(&mut on_token);
                        self.state = LexState::Data;
                    } else {
                        self.state = LexState::BeforeAttrName;
                    }
                }
                LexState::CommentOrDoctype => {
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
            self.current_attrs.push((
                std::mem::take(&mut self.current_attr_name),
                std::mem::take(&mut self.current_attr_val),
            ));
        }
    }

    fn emit_current_tag<F>(&mut self, on_token: &mut F)
    where
        F: FnMut(HtmlToken),
    {
        let name = std::mem::take(&mut self.current_tag_name);
        if name.is_empty() {
            return;
        }
        if self.current_is_end_tag {
            on_token(HtmlToken::EndTag { name });
        } else {
            let is_void = matches!(
                name.as_str(),
                "meta" | "link" | "img" | "br" | "hr" | "input" | "source" | "area" | "base"
            );
            on_token(HtmlToken::StartTag {
                name,
                attributes: std::mem::take(&mut self.current_attrs),
                self_closing: self.current_self_closing || is_void,
            });
        }
    }

    fn flush_text<F>(&mut self, on_token: &mut F)
    where
        F: FnMut(HtmlToken),
    {
        if !self.text_buf.is_empty() {
            let raw = std::mem::take(&mut self.text_buf);
            let collapsed = collapse_whitespace(&raw);
            if !collapsed.is_empty() {
                on_token(HtmlToken::Text(collapsed));
            }
        }
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
