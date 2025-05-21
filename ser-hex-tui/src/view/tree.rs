use std::collections::HashSet;

use itertools::Itertools as _;
use ratatui::{
    style::{Color, Style, Stylize as _},
    text::{Line, Span},
    widgets::Widget,
};
use tui_tree_widget::TreeData;

use crate::{Path, TraceNode, TraceTree};

impl TreeData for TraceTree<'_> {
    type Identifier = Path;

    fn get_nodes(
        &self,
        open_identifiers: &HashSet<Self::Identifier>,
    ) -> Vec<tui_tree_widget::Node<Self::Identifier>> {
        fn collect_visible(
            node: &TraceNode,
            open: &HashSet<Path>,
            nodes: &mut Vec<tui_tree_widget::Node<Path>>,
            depth: usize,
        ) {
            nodes.push(tui_tree_widget::Node {
                depth,
                has_children: !node.children.is_empty(),
                height: 1,
                identifier: node.identifier.clone(),
            });
            if open.contains(&node.identifier) {
                for child in &node.children {
                    collect_visible(child, open, nodes, depth + 1);
                }
            }
        }

        let mut nodes = vec![];
        collect_visible(&self.root, open_identifiers, &mut nodes, 0);

        nodes
    }

    fn render(
        &self,
        identifier: &Self::Identifier,
        area: ratatui::layout::Rect,
        buffer: &mut ratatui::buffer::Buffer,
    ) {
        let node = self.nodes.get(identifier).unwrap();
        let mut line = vec![];
        match node.action {
            ser_hex::Action::Read(_) => {
                line.push(Span::styled(
                    format!("Read ({}) ", node.end - node.start),
                    Style::new().fg(Color::LightGreen),
                ));

                let data = &self.trace.data[node.start..node.end];
                let limit = 100;
                let mut d: String = data
                    .iter()
                    .take(limit)
                    .map(|b| format!("{b:02X}"))
                    .join(" ");
                if data.len() > limit {
                    d.push_str("...");
                }

                line.push(Span::styled(
                    format!("[{d}] "),
                    Style::new().fg(Color::LightYellow),
                ));

                match data.len() {
                    4 => {
                        line.push(Span::styled(
                            format!("{} ", u32::from_le_bytes(data.try_into().unwrap())),
                            Style::new().fg(Color::Magenta),
                        ));
                    }
                    8 => {
                        line.push(Span::styled(
                            format!("{} ", u64::from_le_bytes(data.try_into().unwrap())),
                            Style::new().fg(Color::Magenta),
                        ));
                        let mut buffer = dtoa::Buffer::new();
                        let s = buffer.format(f64::from_le_bytes(data.try_into().unwrap()));
                        line.push(Span::styled(
                            format!("{s} "),
                            Style::new().fg(Color::LightRed),
                        ));
                    }
                    _ => {}
                }
                let max_non_null = data.split(|&b| b == 0).map(|s| s.len()).max().unwrap_or(0);
                if data.is_ascii() && max_non_null >= 4 {
                    line.push(Span::styled(
                        format!("{:?} ", String::from_utf8_lossy(data)),
                        Style::new().fg(Color::Red),
                    ));
                }
                //write!(&mut preview, "{:?} ", String::from_utf8_lossy(data)).unwrap();
            }
            ser_hex::Action::Seek(_) => {
                line.push(Span::styled(
                    format!("Seek ({} -> {}) ", node.start, node.end),
                    Style::new().fg(Color::Red),
                ));
            }
            ser_hex::Action::Span(s) => {
                line.push(Span::styled(
                    format!("Span ({}) ", node.end - node.start),
                    Style::new(),
                ));
                line.push(Span::styled(
                    format!("{}", s.0.name),
                    Style::new().italic().fg(Color::LightCyan),
                ));
            }
        }

        Widget::render(Line::from(line), area, buffer);
    }
}
