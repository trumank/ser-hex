use std::collections::HashMap;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Color,
    widgets::{
        canvas::{Canvas, Points},
        Block, Widget,
    },
};
use tui_tree_widget::TreeState;

use crate::{Path, TraceTree};

use super::{byte_style, ByteType};

pub struct Minimap<'a> {
    pub tree_trait: &'a TraceTree<'a>,
    pub tree_state: &'a TreeState<Path>,
}
impl Widget for Minimap<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let data = &self.tree_trait.trace.data;

        let margin = 3;

        let px_width = area.width as usize - margin;
        let px_height = 2 * (area.height as usize) - margin;
        let pixels = 1 + px_width * (px_height + 1);

        let range = self.tree_state.selected().map(|selected| {
            let selected = &self.tree_trait.nodes[selected];
            selected.start..selected.end
        });

        Canvas::default()
            .block(Block::bordered().title("Canvas"))
            .x_bounds([0.0, px_width as f64])
            .y_bounds([0.0, px_height as f64])
            .marker(ratatui::symbols::Marker::HalfBlock)
            .paint(|ctx| {
                ctx.layer();

                let mut color_bins: Vec<(Color, Vec<(f64, f64)>)> = vec![];
                for y in 0..=px_height {
                    for x in 0..=px_width {
                        let start = (x + y * px_width) * data.len() / pixels;
                        let end = ((x + y * px_width + 1) * data.len()).div_ceil(pixels);

                        let mut bytes: HashMap<ByteType, usize> = Default::default();

                        let mut highlight = false;
                        for (i, b) in data
                            .iter()
                            .copied()
                            .enumerate()
                            .skip(start)
                            .take(end - start)
                        {
                            let style = byte_style(range.clone(), i, b);
                            highlight |= style.highlight;
                            *bytes.entry(style.byte_type).or_default() += 1;
                        }

                        let style = bytes
                            .into_iter()
                            .max_by_key(|(s, c)| (*c, *s))
                            .map(|(s, _)| s);
                        let color = if highlight {
                            Color::LightGreen
                        } else if let Some(style) = style {
                            style.color()
                        } else {
                            Color::LightCyan
                        };

                        // get bin by color (or create new)
                        let entry = color_bins.iter_mut().find(|(c, _)| *c == color);
                        let entry = &mut if let Some(entry) = entry {
                            entry
                        } else {
                            color_bins.push((color, vec![]));
                            color_bins.last_mut().unwrap()
                        }
                        .1;

                        entry.push((x as f64, (px_height - y) as f64));
                    }
                }

                for (color, coords) in &color_bins {
                    ctx.draw(&Points {
                        color: *color,
                        coords,
                    });
                }
            })
            .render(area, buf);
    }
}
