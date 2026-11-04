use crate::css_engine::{ComputedStyle, MicroCssEngine};
use crate::dom_arena::{NodeId, NodeKind, ZeroCopyDomArena, NO_NODE};
use crate::input_forms::FormControlKind;

/// Hardware-friendly draw command emitted by Manta's layout pass, including interactive
/// `<input>`, `<textarea>`, `<select>`, and `<button>` hit-target metadata (`INPUT_BOX`).
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
    DrawInputBox {
        node_id: NodeId,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        display_text: String,
        is_focused: bool,
        is_checked: bool,
    },
}

/// Complete frame output for a tab, ready for instant C-ABI / JNI transfer to any platform UI.
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
    pub fn build(
        dom: &ZeroCopyDomArena,
        css: &MicroCssEngine,
        viewport_width: f32,
    ) -> DisplayListFrame {
        let mut commands = Vec::with_capacity(64);
        let mut cursor_y = 16.0f32;
        let horizontal_padding = 16.0f32;
        let mut page_bg = 0xFF09090B;

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

    /// Serializes the frame into Manta's compact wire format (`MANTA_FRAME`).
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
                PaintCommand::DrawInputBox {
                    node_id,
                    x,
                    y,
                    width,
                    height,
                    display_text,
                    is_focused,
                    is_checked,
                } => {
                    out.push_str(&format!(
                        "INPUT|{}|{:.1}|{:.1}|{:.1}|{:.1}|{}|{}|{}\n",
                        node_id,
                        x,
                        y,
                        width,
                        height,
                        u8::from(*is_focused),
                        u8::from(*is_checked),
                        display_text.replace('\n', " ").replace('|', " ")
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
        if tag == "hr" {
            *cursor_y += 8.0;
            commands.push(PaintCommand::FillRect {
                x,
                y: *cursor_y,
                width: avail_width,
                height: 1.5,
                color_argb: 0xFF232734,
                corner_radius: 1.0,
            });
            *cursor_y += 10.0;
            return;
        }

        // Render interactive `<input>`, `<textarea>`, and `<select>` controls
        if let Some(ctrl) = dom.control_for_node(node_id) {
            if ctrl.kind != FormControlKind::Hidden {
                *cursor_y += style.margin_vertical;
                let box_h = style.explicit_height.unwrap_or(42.0);
                let is_focused = dom.forms.focused_node() == Some(node_id);
                let display_text = match &ctrl.kind {
                    FormControlKind::TextInput { input_type } if input_type == "password" => {
                        "•".repeat(ctrl.value.chars().count())
                    }
                    FormControlKind::Checkbox => {
                        format!(
                            "[{}] {}",
                            if ctrl.checked { "✓" } else { " " },
                            ctrl.name
                        )
                    }
                    FormControlKind::Radio => {
                        format!(
                            "({}) {}",
                            if ctrl.checked { "•" } else { " " },
                            ctrl.name
                        )
                    }
                    _ => {
                        if ctrl.value.is_empty() {
                            ctrl.placeholder.clone()
                        } else {
                            ctrl.value.clone()
                        }
                    }
                };

                commands.push(PaintCommand::DrawInputBox {
                    node_id,
                    x,
                    y: *cursor_y,
                    width: avail_width,
                    height: box_h,
                    display_text,
                    is_focused,
                    is_checked: ctrl.checked,
                });
                *cursor_y += box_h + 6.0;
                return;
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
                corner_radius: style.corner_radius,
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

            let prefix = if node.parent != NO_NODE {
                if let NodeKind::Element { ref tag, .. } = dom.nodes()[node.parent as usize].kind {
                    if tag == "li" {
                        "• "
                    } else {
                        ""
                    }
                } else {
                    ""
                }
            } else {
                ""
            };

            let full_text = if prefix.is_empty() {
                text.clone()
            } else {
                format!("{}{}", prefix, text)
            };

            commands.push(PaintCommand::DrawText {
                x: x + parent_style.padding,
                y: *cursor_y + parent_style.padding,
                text: full_text.clone(),
                color_argb: parent_style.text_color,
                font_size_sp: parent_style.font_size_sp,
                bold: parent_style.font_weight_bold,
                link_href: current_link.clone(),
            });

            let chars_per_line = (avail_width / (parent_style.font_size_sp * 0.55)).max(12.0);
            let lines = ((full_text.len() as f32) / chars_per_line).ceil().max(1.0);
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
