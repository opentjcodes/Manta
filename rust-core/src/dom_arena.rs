use std::ptr;
use std::sync::atomic::{compiler_fence, Ordering};

use crate::html_tokenizer::HtmlToken;

/// Compact 32-bit node index into the flat `ZeroCopyDomArena`.
/// Eliminates 64-bit pointer overhead and `Rc<RefCell<...>>` cycles completely.
pub type NodeId = u32;

pub const NO_NODE: NodeId = u32::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    Document,
    Element {
        tag: String,
        id_attr: Option<String>,
        classes: Vec<String>,
        attributes: Vec<(String, String)>,
        inline_style: Option<String>,
    },
    Text(String),
}

#[derive(Debug, Clone)]
pub struct DomNode {
    pub id: NodeId,
    pub parent: NodeId,
    pub first_child: NodeId,
    pub last_child: NodeId,
    pub next_sibling: NodeId,
    pub kind: NodeKind,
}

/// Contiguous Flat Index DOM Arena.
/// Stores all DOM nodes inside a single `Vec<DomNode>` with strict byte accounting.
pub struct ZeroCopyDomArena {
    nodes: Vec<DomNode>,
    open_stack: Vec<NodeId>,
    pub collected_styles: Vec<String>,
    pub collected_scripts: Vec<String>,
    pub page_title: String,
    approx_bytes: usize,
    max_bytes: usize,
}

impl ZeroCopyDomArena {
    pub fn new(max_bytes: usize) -> Self {
        let mut arena = Self {
            nodes: Vec::with_capacity(128),
            open_stack: Vec::with_capacity(16),
            collected_styles: Vec::new(),
            collected_scripts: Vec::new(),
            page_title: String::from("Manta"),
            approx_bytes: 0,
            max_bytes,
        };
        let root_id = arena.alloc_node(NO_NODE, NodeKind::Document);
        arena.open_stack.push(root_id);
        arena
    }

    #[inline]
    pub fn nodes(&self) -> &[DomNode] {
        &self.nodes
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.approx_bytes
    }

    pub fn ingest_token(&mut self, token: HtmlToken) {
        if self.approx_bytes >= self.max_bytes {
            return;
        }
        let current_parent = *self.open_stack.last().unwrap_or(&0);

        match token {
            HtmlToken::StartTag {
                name,
                attributes,
                self_closing,
            } => {
                let mut id_attr = None;
                let mut classes = Vec::new();
                let mut inline_style = None;

                for (k, v) in &attributes {
                    match k.as_str() {
                        "id" => id_attr = Some(v.clone()),
                        "class" => {
                            classes.extend(v.split_whitespace().map(|s| s.to_string()));
                        }
                        "style" => inline_style = Some(v.clone()),
                        _ => {}
                    }
                }

                let node_id = self.alloc_node(
                    current_parent,
                    NodeKind::Element {
                        tag: name,
                        id_attr,
                        classes,
                        attributes,
                        inline_style,
                    },
                );

                if !self_closing {
                    self.open_stack.push(node_id);
                }
            }
            HtmlToken::EndTag { name } => {
                // Pop back to matching open tag
                if let Some(pos) = self.open_stack.iter().rposition(|&nid| {
                    matches!(
                        &self.nodes[nid as usize].kind,
                        NodeKind::Element { tag, .. } if tag == &name
                    )
                }) {
                    self.open_stack.truncate(pos);
                }
            }
            HtmlToken::Text(txt) => {
                if let Some(parent_node) = self.nodes.get(current_parent as usize) {
                    if let NodeKind::Element { tag, .. } = &parent_node.kind {
                        match tag.as_str() {
                            "style" => {
                                self.approx_bytes += txt.len();
                                self.collected_styles.push(txt);
                                return;
                            }
                            "script" => {
                                self.approx_bytes += txt.len();
                                self.collected_scripts.push(txt);
                                return;
                            }
                            "title" => {
                                self.page_title = txt;
                                return;
                            }
                            _ => {}
                        }
                    }
                }
                self.alloc_node(current_parent, NodeKind::Text(txt));
            }
        }
    }

    pub fn find_by_id(&self, target_id: &str) -> Option<NodeId> {
        for node in &self.nodes {
            if let NodeKind::Element {
                id_attr: Some(ref id),
                ..
            } = node.kind
            {
                if id == target_id {
                    return Some(node.id);
                }
            }
        }
        None
    }

    pub fn query_selector(&self, selector: &str) -> Option<NodeId> {
        let sel = selector.trim();
        if let Some(id) = sel.strip_prefix('#') {
            return self.find_by_id(id);
        }
        if let Some(cls) = sel.strip_prefix('.') {
            for node in &self.nodes {
                if let NodeKind::Element { ref classes, .. } = node.kind {
                    if classes.iter().any(|c| c == cls) {
                        return Some(node.id);
                    }
                }
            }
            return None;
        }
        let tag_target = sel.to_ascii_lowercase();
        for node in &self.nodes {
            if let NodeKind::Element { ref tag, .. } = node.kind {
                if tag == &tag_target {
                    return Some(node.id);
                }
            }
        }
        None
    }

    pub fn set_text_content(&mut self, node_id: NodeId, new_text: String) -> bool {
        let idx = node_id as usize;
        if idx >= self.nodes.len() {
            return false;
        }
        let first_child = self.nodes[idx].first_child;
        if first_child != NO_NODE {
            if let NodeKind::Text(ref mut existing) = self.nodes[first_child as usize].kind {
                *existing = new_text;
                return true;
            }
        }
        self.alloc_node(node_id, NodeKind::Text(new_text));
        true
    }

    pub fn text_content(&self, node_id: NodeId) -> String {
        let mut out = String::new();
        self.collect_text_recursive(node_id, &mut out);
        out
    }

    fn collect_text_recursive(&self, node_id: NodeId, out: &mut String) {
        let Some(node) = self.nodes.get(node_id as usize) else {
            return;
        };
        match &node.kind {
            NodeKind::Text(t) => {
                if !out.is_empty() {
                    out.push(' ');
                }
                out.push_str(t);
            }
            _ => {
                let mut child = node.first_child;
                while child != NO_NODE {
                    self.collect_text_recursive(child, out);
                    child = self.nodes[child as usize].next_sibling;
                }
            }
        }
    }

    /// Zeroizes all text strings and attributes in the DOM arena before releasing memory.
    pub fn zeroize_and_clear(&mut self) {
        for node in &mut self.nodes {
            match &mut node.kind {
                NodeKind::Text(s) => zeroize_string(s),
                NodeKind::Element {
                    tag,
                    id_attr,
                    classes,
                    attributes,
                    inline_style,
                } => {
                    zeroize_string(tag);
                    if let Some(id) = id_attr {
                        zeroize_string(id);
                    }
                    for c in classes {
                        zeroize_string(c);
                    }
                    for (k, v) in attributes {
                        zeroize_string(k);
                        zeroize_string(v);
                    }
                    if let Some(st) = inline_style {
                        zeroize_string(st);
                    }
                }
                NodeKind::Document => {}
            }
        }
        for s in &mut self.collected_styles {
            zeroize_string(s);
        }
        for s in &mut self.collected_scripts {
            zeroize_string(s);
        }
        zeroize_string(&mut self.page_title);

        self.nodes.clear();
        self.nodes.shrink_to_fit();
        self.open_stack.clear();
        self.collected_styles.clear();
        self.collected_scripts.clear();
        self.approx_bytes = 0;
    }

    fn alloc_node(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.nodes.len() as NodeId;
        let node_cost = std::mem::size_of::<DomNode>()
            + match &kind {
                NodeKind::Document => 0,
                NodeKind::Text(t) => t.len(),
                NodeKind::Element {
                    tag, attributes, ..
                } => tag.len() + attributes.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>(),
            };
        self.approx_bytes += node_cost;

        self.nodes.push(DomNode {
            id,
            parent,
            first_child: NO_NODE,
            last_child: NO_NODE,
            next_sibling: NO_NODE,
            kind,
        });

        if parent != NO_NODE && (parent as usize) < self.nodes.len() {
            let prev_last = self.nodes[parent as usize].last_child;
            if prev_last == NO_NODE {
                self.nodes[parent as usize].first_child = id;
            } else {
                self.nodes[prev_last as usize].next_sibling = id;
            }
            self.nodes[parent as usize].last_child = id;
        }
        id
    }
}

fn zeroize_string(s: &mut String) {
    unsafe {
        for b in s.as_bytes_mut() {
            ptr::write_volatile(b, 0u8);
        }
    }
    compiler_fence(Ordering::SeqCst);
    s.clear();
}
