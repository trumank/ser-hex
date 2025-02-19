use ratatui::{
    layout::Rect,
    style::{Color, Style, Stylize as _},
    text::{Line, Span},
    widgets::{
        Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget,
        Widget,
    },
};
use tui_tree_widget::TreeState;

use crate::{view::byte_style, Path, TraceTree};

pub struct HexState {
    scroll_state: ScrollbarState,
    columns: usize,
}
impl Default for HexState {
    fn default() -> Self {
        Self {
            scroll_state: Default::default(),
            columns: 16,
        }
    }
}
impl HexState {
    pub fn dec_columns(&mut self) -> bool {
        if self.columns > 1 {
            self.columns -= 1;
            true
        } else {
            false
        }
    }
    pub fn inc_columns(&mut self) -> bool {
        self.columns += 1;
        true
    }
    pub fn desired_width(&self) -> u16 {
        self.columns as u16 * 4 + 13
    }
}

pub struct HexView<'a> {
    pub tree_trait: &'a TraceTree<'a>,
    pub tree_state: &'a TreeState<Path>,
}
impl StatefulWidget for HexView<'_> {
    type State = HexState;

    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let data = &self.tree_trait.trace.data;
        let columns = state.columns;

        let height = area.height as usize;
        let (scroll, range) = if let Some(selected) = self.tree_state.selected() {
            let selected = &self.tree_trait.nodes[selected];
            let rows = data.len().div_ceil(columns);
            (
                (rows * selected.start / data.len()).saturating_sub(height / 2),
                Some(selected.start..selected.end),
            )
        } else {
            (0, None)
        };

        let total_rows = data.len().div_ceil(columns);

        state.scroll_state = state
            .scroll_state
            .content_length(total_rows)
            .position(scroll);

        let hex_view = data
            .chunks(columns)
            .enumerate()
            .skip(scroll)
            .take(height)
            .map(|(i, chunk)| {
                let mut line = vec![];
                line.push(Span::styled(
                    format!("{:08X}: ", i * columns),
                    Style::new().fg(Color::DarkGray),
                ));

                let mut ascii = vec![];
                trait SpanExt {
                    fn r(self, reverse: bool) -> Self;
                }
                impl SpanExt for Span<'_> {
                    fn r(self, reverse: bool) -> Self {
                        if reverse {
                            self.reversed()
                        } else {
                            self
                        }
                    }
                }

                let style = |(j, b): (usize, &u8)| byte_style(range.clone(), (i * columns) + j, *b);

                let mut iter = chunk.iter().enumerate().peekable();
                while let Some(item) = iter.next() {
                    let s = style(item);
                    let (_j, b) = item;
                    line.push(
                        Span::raw(format!("{:02X}", b))
                            .fg(s.byte_type.color())
                            .r(s.highlight),
                    );
                    if let Some(next) = iter.peek() {
                        let next_s = style(*next);
                        let highlight_space = s.highlight && next_s.highlight;
                        let color = s.byte_type.min(next_s.byte_type).color();
                        line.push(Span::raw(" ").fg(color).r(highlight_space));
                    } else {
                        line.push(Span::raw(" "));
                    }
                    ascii.push(
                        Span::raw(s.symbol.to_string())
                            .fg(s.byte_type.color())
                            .r(s.highlight),
                    );
                }
                line.push(Span::raw("   ".repeat(columns - chunk.len())));

                line.extend(ascii);

                Line::from(line)
            })
            .collect::<Vec<_>>();

        let paragraph = Paragraph::new(hex_view)
            .block(Block::default().borders(Borders::ALL).title("Hex View"));

        paragraph.render(area, buf);
        Scrollbar::new(ScrollbarOrientation::VerticalRight).render(
            area,
            buf,
            &mut state.scroll_state,
        );
    }
}
