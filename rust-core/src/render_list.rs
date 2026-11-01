use crate::css_engine::{ComputedStyle, MicroCssEngine};
use crate::dom_arena::{NodeId, NodeKind, ZeroCopyDomArena, NO_NODE};

/// Hardware-friendly draw command emitted by Manta's layout pass.
#[derive(Debug, Clone)]
pub enum PaintCommand {
    FillRect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        color_argb: u32,
        corner_radius: f32,
    },
    DrawText {
        x: f32,
        y: f32,
        text: String,
        color_argb: u32,
        font_size_sp: f32,
        bold: bool,
        link_href: Option<String>,
    },
}

/// Complete frame output for a tab, ready for instant JNI binary transfer to Kotlin UI.
#[derive(Debug, Clone)]
pub struct DisplayListFrame {
    pub page_title: String,
    pub background_argb: u32,
    pub content_height: f32,
    pub commands: Vec<PaintCommand>,
    pub dom_nodes_count: usize,
    pub ram_used_bytes: usize,
}

impl DisplayListFrame {
    /// Builds a DisplayListFrame in a single depth-first pass over the flat DOM arena.
    pub fn build(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
    ) -> DisplayListFrame {
        let mut commands = Vec::with_capacity(64);
        let mut cursor_y = 16.0f32;
        let horizontal_padding = 16.0f32;
        let mut page_bg = 0xFF09090B; // Manta pure obsidian dark background

        if !dom.nodes().is_empty() {
            layout_node_recursive(
                0,
                dom,
                css,
                horizontal_padding,
                viewport_width - (horizontal_padding * 2.0),
                &mut cursor_y,
                &mut page_bg,
                None,
                &mut commands,
            );
        }

        DisplayListFrame {
            page_title: dom.page_title.clone(),
            background_argb: page_bg,
            content_height: cursor_y + 32.0,
            commands,
            dom_nodes_count: dom.nodes().len(),
            ram_used_bytes: dom.used_bytes(),
        }
    }

    /// Serializes the frame into a compact, zero-JSON-overhead binary buffer for JNI transfer,
    /// or a human-readable structured summary for inspection.
    pub fn to_compact_wire_string(&self) -> String {
        let mut out = String::with_capacity(256 + self.commands.len() * 48);
        out.push_str(&format!(
            "MANTA_FRAME|title={}|bg=#{:08X}|height={:.1}|nodes={}|ramKB={:.2}\n",
            self.page_title.replace('|', " "),
            self.background_argb,
            self.content_height,
            self.dom_nodes_count,
            self.ram_used_bytes as f32 / 1024.0
        ));

        for cmd in &self.commands {
            match cmd {
                PaintCommand::FillRect {
                    x,
                    y,
                    width,
                    height,
                    color_argb,
                    corner_radius,
                } => {
                    out.push_str(&format!(
                        "RECT|{:.1}|{:.1}|{:.1}|{:.1}|#{:08X}|{:.1}\n",
                        x, y, width, height, color_argb, corner_radius
                    ));
                }
                PaintCommand::DrawText {
                    x,
                    y,
                    text,
                    color_argb,
                    font_size_sp,
                    bold,
                    link_href,
                } => {
                    out.push_str(&format!(
                        "TEXT|{:.1}|{:.1}|{:.1}|{}|#{:08X}|{}|{}\n",
                        x,
                        y,
                        font_size_sp,
                        if *bold { 1 } else { 0 },
                        color_argb,
                        link_href.as_deref().unwrap_or(""),
                        text.replace('\n', " ").replace('|', " ")
                    ));
                }
            }
        }
        out
    }
}

#[allow(clippy::too_many_arguments)]
fn layout_node_recursive(
    node_id: NodeId,
    dom: &ZeroCopyDomArena,
    css: &MicroCssEngine,
    x: f32,
    avail_width: f32,
    cursor_y: &mut f32,
    page_bg: &mut u32,
    inherited_link: Option<String>,
    commands: &mut Vec<PaintCommand>,
) {
    let Some(node) = dom.nodes().get(node_id as usize) else {
        return;
    };

    let style: ComputedStyle = css.compute_style(node);
    if style.display_none {
        return;
    }

    let mut current_link = inherited_link;

    if let NodeKind::Element {
        ref tag,
        ref attributes,
        ..
    } = node.kind
    {
        if tag == "body" && style.background_color != 0 {
            *page_bg = style.background_color;
        }
        if tag == "a" {
            for (k, v) in attributes {
                if k == "href" {
                    current_link = Some(v.clone());
                }
            }
        }

        *cursor_y += style.margin_vertical;

        if style.background_color != 0 && tag != "body" {
            commands.push(PaintCommand::FillRect {
                x,
                y: *cursor_y,
                width: avail_width,
                height: style.font_size_sp + (style.padding * 2.0) + 8.0,
                color_argb: style.background_color,
                corner_radius: 10.0,
            });
        }
    }

    if let NodeKind::Text(ref text) = node.kind {
        if !text.is_empty() {
            let parent_style = if node.parent != NO_NODE {
                css.compute_style(&dom.nodes()[node.parent as usize])
            } else {
                ComputedStyle::default()
            };

            commands.push(PaintCommand::DrawText {
                x: x + parent_style.padding,
                y: *cursor_y + parent_style.padding,
                text: text.clone(),
                color_argb: parent_style.text_color,
                font_size_sp: parent_style.font_size_sp,
                bold: parent_style.font_weight_bold,
                link_href: current_link.clone(),
            });

            let chars_per_line = (avail_width / (parent_style.font_size_sp * 0.55)).max(12.0);
            let lines = ((text.len() as f32) / chars_per_line).ceil().max(1.0);
            *cursor_y += lines * (parent_style.font_size_sp * 1.45) + 4.0;
        }
    }

    let mut child = node.first_child;
    while child != NO_NODE {
        layout_node_recursive(
            child,
            dom,
            css,
            x + style.padding,
            (avail_width - style.padding * 2.0).max(60.0),
            cursor_y,
            page_bg,
            current_link.clone(),
            commands,
        );
        child = dom.nodes()[child as usize].next_sibling;
    }
}
