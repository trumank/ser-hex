use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use itertools::Itertools;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget,
    Widget,
};
use ratatui::{crossterm, Frame, Terminal};
use tui_tree_widget::{Tree, TreeData, TreeState};

#[must_use]
struct App<'trace> {
    tree_state: TreeState<Path>,
    hex_state: HexState,
    tree_trait: TraceTree<'trace>,
}

struct TraceTree<'trace> {
    trace: &'trace ser_hex::Trace,
    nodes: BTreeMap<Path, Rc<TraceNode<'trace>>>,
    root: Rc<TraceNode<'trace>>,
}

#[derive(Debug, Clone)]
struct TraceNode<'trace> {
    identifier: Path,
    start: usize,
    end: usize,
    action: &'trace ser_hex::Action<ser_hex::TreeSpan>,
    children: Vec<Rc<TraceNode<'trace>>>,
}

#[derive(Default, Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Hash)]
pub struct Path<T: AsRef<[u8]> = Vec<u8>>(T);
impl<T: AsRef<[u8]>> Path<T> {}
impl Path {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, max: usize, n: usize) {
        let num_bytes = if max == 0 {
            1
        } else {
            (max.ilog2() / 8) as usize + 1
        };
        let b = n.to_be_bytes();
        self.0.extend_from_slice(&b[b.len() - num_bytes..]);
    }
    pub fn pop(&mut self, max: usize) {
        let num_bytes = if max == 0 {
            1
        } else {
            (max.ilog2() / 8) as usize + 1
        };
        self.0.truncate(self.0.len() - num_bytes);
    }
    pub fn as_slice(&self) -> Path<&[u8]> {
        Path(self.0.as_slice())
    }
}
impl<'a> Path<&'a [u8]> {
    pub fn split_next(&self, max: usize) -> (usize, Path<&'a [u8]>) {
        let num_bytes = if max == 0 {
            1
        } else {
            (max.ilog2() / 8) as usize + 1
        };
        let mut result = 0;
        let arr = self.0;
        for (i, &byte) in arr[..num_bytes].iter().rev().enumerate() {
            result |= (byte as usize) << (i * 8);
        }
        (result, Path(&arr[num_bytes..]))
    }
}

impl<'trace> TraceTree<'trace> {
    fn new(trace: &'trace ser_hex::Trace) -> Self {
        fn convert<'trace>(
            offset: &mut usize,
            action: &'trace ser_hex::Action<ser_hex::TreeSpan>,
            nodes: &mut BTreeMap<Path, Rc<TraceNode<'trace>>>,
            path: &mut Path,
        ) -> Rc<TraceNode<'trace>> {
            let start = *offset;
            match action {
                ser_hex::Action::Read(r) => {
                    *offset += r;
                    let node: Rc<_> = TraceNode {
                        identifier: path.clone(),
                        start,
                        end: *offset,
                        action,
                        children: vec![],
                    }
                    .into();
                    nodes.insert(path.clone(), node.clone());
                    node
                }
                ser_hex::Action::Seek(s) => {
                    *offset = *s;
                    let node: Rc<_> = TraceNode {
                        identifier: path.clone(),
                        start: *offset,
                        end: *s,
                        action,
                        children: vec![],
                    }
                    .into();
                    nodes.insert(path.clone(), node.clone());
                    node
                }
                ser_hex::Action::Span(s) => {
                    let mut children = vec![];

                    let start = *offset;
                    for (i, child) in s.0.actions.iter().enumerate() {
                        path.push(s.0.actions.len(), i);
                        children.push(convert(offset, child, nodes, path));
                        path.pop(s.0.actions.len());
                    }

                    let node: Rc<_> = TraceNode {
                        identifier: path.clone(),
                        start,
                        end: *offset,
                        action,
                        children,
                    }
                    .into();
                    nodes.insert(path.clone(), node.clone());
                    node
                }
            }
        }

        let mut nodes = Default::default();

        let root = convert(&mut 0, &trace.root, &mut nodes, &mut Path::new());

        Self { trace, nodes, root }
    }
}

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
                let d: String = data.iter().map(|b| format!("{b:02X}")).join(" ");

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

impl<'trace> App<'trace> {
    fn new(trace: &'trace ser_hex::Trace) -> Self {
        Self {
            tree_state: TreeState::default(),
            hex_state: HexState::default(),
            tree_trait: TraceTree::new(trace),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();

        let widget = Tree::new(&self.tree_trait)
            .block(
                Block::bordered()
                    .title("Tree Widget")
                    .title_bottom(format!("{:?}", self.tree_state)),
            )
            .experimental_scrollbar(Some(Scrollbar::new(ScrollbarOrientation::VerticalRight)))
            .highlight_style(
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            );

        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Fill(1),
                Constraint::Max(self.hex_state.desired_width()),
            ])
            .split(area);

        frame.render_stateful_widget(widget, layout[0], &mut self.tree_state);
        frame.render_stateful_widget(
            HexView {
                tree_trait: &self.tree_trait,
                tree_state: &self.tree_state,
            },
            layout[1],
            &mut self.hex_state,
        );
    }
}

struct HexState {
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
    fn dec_columns(&mut self) -> bool {
        if self.columns > 1 {
            self.columns -= 1;
            true
        } else {
            false
        }
    }
    fn inc_columns(&mut self) -> bool {
        self.columns += 1;
        true
    }
    fn desired_width(&self) -> u16 {
        self.columns as u16 * 4 + 13
    }
}

struct HexView<'a> {
    tree_trait: &'a TraceTree<'a>,
    tree_state: &'a TreeState<Path>,
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

                struct ByteStyle {
                    byte_type: ByteType,
                    symbol: char,
                    highlight: bool,
                }
                #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
                enum ByteType {
                    Null,
                    Other,
                    Ascii,
                }
                impl ByteType {
                    fn color(self) -> Color {
                        match self {
                            ByteType::Null => Color::DarkGray,
                            ByteType::Other => Color::White,
                            ByteType::Ascii => Color::Red,
                        }
                    }
                }

                let style = |(j, b): (usize, &u8)| {
                    let (byte_type, symbol) = if b.is_ascii_graphic() {
                        (ByteType::Ascii, *b as char)
                    } else if *b == 0 {
                        (ByteType::Null, '.')
                    } else {
                        (ByteType::Other, '.')
                    };
                    ByteStyle {
                        byte_type,
                        symbol,
                        highlight: range
                            .as_ref()
                            .is_some_and(|r| r.contains(&((i * columns) + j))),
                    }
                };

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

fn main() -> std::io::Result<()> {
    let mut deserializer = serde_json::Deserializer::from_reader(std::io::BufReader::new(
        std::fs::File::open(std::env::args().nth(1).expect("expected path"))?,
    ));
    deserializer.disable_recursion_limit();
    use serde::de::Deserialize;
    let data = ser_hex::Trace::deserialize(&mut deserializer)?;

    // Terminal initialization
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    // App
    let app = App::new(&data);
    let res = run_app(&mut terminal, app);

    // restore terminal
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

    Ok(())
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> std::io::Result<()> {
    const DEBOUNCE: Duration = Duration::from_millis(20); // 50 FPS

    let before = Instant::now();
    terminal.draw(|frame| app.draw(frame))?;
    let mut last_render_took = before.elapsed();

    let mut debounce: Option<Instant> = None;

    loop {
        let timeout = debounce.map_or(DEBOUNCE, |start| DEBOUNCE.saturating_sub(start.elapsed()));
        if crossterm::event::poll(timeout)? {
            let update = match crossterm::event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(())
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.tree_state.select_relative(|current| {
                            current.map_or(0, |current| current.saturating_add(20))
                        })
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.tree_state.select_relative(|current| {
                            current.map_or(0, |current| current.saturating_sub(20))
                        })
                    }

                    KeyCode::Char('g') => app.tree_state.select_first(),
                    KeyCode::Char('G') => app.tree_state.select_last(),

                    KeyCode::Char('j') => app.tree_state.select_relative(|current| {
                        current.map_or(0, |current| current.saturating_add(1))
                    }),
                    KeyCode::Char('k') => app.tree_state.select_relative(|current| {
                        current.map_or(0, |current| current.saturating_sub(1))
                    }),
                    KeyCode::Char('h') => app.tree_state.key_left(),
                    KeyCode::Char('l') => {
                        //app.state.key_right()

                        // open node or move down if node already open or empty
                        let state = &mut app.tree_state;
                        if let Some(selected) = state.selected() {
                            let has_children = !app
                                .tree_trait
                                .nodes
                                .get(selected)
                                .unwrap()
                                .children
                                .is_empty();

                            if has_children && state.open(selected.clone()) {
                                true
                            } else {
                                app.tree_state.select_relative(|current| {
                                    current.map_or(0, |current| current.saturating_add(1))
                                })
                            }
                        } else {
                            false
                        }
                    }

                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('\n' | ' ') => app.tree_state.toggle_selected(),
                    KeyCode::Left => app.tree_state.key_left(),
                    KeyCode::Right => app.tree_state.key_right(),
                    KeyCode::Down => app.tree_state.key_down(),
                    KeyCode::Up => app.tree_state.key_up(),
                    KeyCode::Esc => app.tree_state.select(Some(Path::new())),
                    KeyCode::Home => app.tree_state.select_first(),
                    KeyCode::End => app.tree_state.select_last(),
                    KeyCode::PageDown => app.tree_state.scroll_down(3),
                    KeyCode::PageUp => app.tree_state.scroll_up(3),
                    KeyCode::Char('-') => app.hex_state.dec_columns(),
                    KeyCode::Char('=') => app.hex_state.inc_columns(),
                    _ => false,
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => app.tree_state.scroll_down(1),
                    MouseEventKind::ScrollUp => app.tree_state.scroll_up(1),
                    MouseEventKind::Down(_button) => app
                        .tree_state
                        .click_at(Position::new(mouse.column, mouse.row)),
                    _ => false,
                },
                Event::Resize(_, _) => true,
                _ => false,
            };
            if update {
                debounce.get_or_insert_with(Instant::now);
            }
        }
        if debounce.is_some_and(|debounce| debounce.elapsed() > DEBOUNCE) {
            let before = Instant::now();
            terminal.draw(|frame| {
                app.draw(frame);

                // Performance info in top right corner
                {
                    let text = format!(
                        " {} {last_render_took:?} {:.1} FPS",
                        frame.count(),
                        1.0 / last_render_took.as_secs_f64()
                    );
                    #[allow(clippy::cast_possible_truncation)]
                    let area = Rect {
                        y: 0,
                        height: 1,
                        x: frame.area().width.saturating_sub(text.len() as u16),
                        width: text.len() as u16,
                    };

                    frame.render_widget(
                        Span::styled(text, Style::new().fg(Color::Black).bg(Color::Gray)),
                        area,
                    );
                }
            })?;
            last_render_took = before.elapsed();

            debounce = None;
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_path() {
        let data = vec![
            (12, 2),
            (12, 0),
            (300, 270),
            (12345, 4),
            (255, 255),
            (256, 256),
        ];
        let mut path = Path::new();
        for (max, n) in &data {
            path.push(*max, *n);
        }
        dbg!(&path);

        let mut path = path.as_slice();
        let mut n;
        for (max, a) in data {
            (n, path) = path.split_next(max);
            dbg!((max, n));
            assert_eq!(a, n);
        }
    }
}
