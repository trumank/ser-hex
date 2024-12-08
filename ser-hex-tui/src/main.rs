use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use itertools::Itertools;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, Widget};
use ratatui::{crossterm, Frame, Terminal};
use tui_tree_widget::{Tree, TreeData, TreeState};

#[must_use]
struct App<'trace> {
    state: TreeState<Vec<usize>>,
    tree_trait: TraceTree<'trace>,
}

struct TraceTree<'trace> {
    trace: &'trace ser_hex::Trace,
    nodes: BTreeMap<Vec<usize>, TraceNode<'trace>>,
    root: TraceNode<'trace>,
}

#[derive(Debug, Clone)]
struct TraceNode<'trace> {
    identifier: Vec<usize>,
    start: usize,
    end: usize,
    action: &'trace ser_hex::Action<ser_hex::TreeSpan>,
    children: Vec<TraceNode<'trace>>,
}

impl<'trace> TraceTree<'trace> {
    fn new(trace: &'trace ser_hex::Trace) -> Self {
        fn convert<'trace>(
            offset: &mut usize,
            action: &'trace ser_hex::Action<ser_hex::TreeSpan>,
            nodes: &mut BTreeMap<Vec<usize>, TraceNode<'trace>>,
            path: &mut Vec<usize>,
        ) -> TraceNode<'trace> {
            let start = *offset;
            match action {
                ser_hex::Action::Read(r) => {
                    *offset += r;
                    let node = TraceNode {
                        identifier: path.clone(),
                        start,
                        end: *offset,
                        action,
                        children: vec![],
                    };
                    nodes.insert(path.clone(), node.clone());
                    node
                }
                ser_hex::Action::Seek(s) => {
                    *offset = *s;
                    let node = TraceNode {
                        identifier: path.clone(),
                        start: *offset,
                        end: *s,
                        action,
                        children: vec![],
                    };
                    nodes.insert(path.clone(), node.clone());
                    node
                }
                ser_hex::Action::Span(s) => {
                    let mut children = vec![];

                    let start = *offset;
                    path.push(0);
                    for (i, child) in s.0.actions.iter().enumerate() {
                        *path.last_mut().unwrap() = i;
                        children.push(convert(offset, child, nodes, path));
                    }
                    path.pop();

                    let node = TraceNode {
                        identifier: path.clone(),
                        start,
                        end: *offset,
                        action,
                        children,
                    };
                    nodes.insert(path.clone(), node.clone());
                    node
                }
            }
        }

        let mut nodes = BTreeMap::default();

        let root = convert(&mut 0, &trace.root, &mut nodes, &mut vec![]);

        Self { trace, nodes, root }
    }
}

impl TreeData for TraceTree<'_> {
    type Identifier = Vec<usize>;

    fn get_nodes(
        &self,
        open_identifiers: &HashSet<Self::Identifier>,
    ) -> Vec<tui_tree_widget::Node<Self::Identifier>> {
        fn collect_visible(
            node: &TraceNode,
            open: &HashSet<Vec<usize>>,
            nodes: &mut Vec<tui_tree_widget::Node<Vec<usize>>>,
        ) {
            nodes.push(tui_tree_widget::Node {
                depth: node.identifier.len(),
                has_children: !node.children.is_empty(),
                height: 1,
                identifier: node.identifier.clone(),
            });
            if open.contains(&node.identifier) {
                for child in &node.children {
                    collect_visible(child, open, nodes);
                }
            }
        }

        let mut nodes = vec![];
        collect_visible(&self.root, open_identifiers, &mut nodes);

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
            state: TreeState::default(),
            tree_trait: TraceTree::new(trace),
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();

        let widget = Tree::new(&self.tree_trait)
            .block(
                Block::bordered()
                    .title("Tree Widget")
                    .title_bottom(format!("{:?}", self.state)),
            )
            .experimental_scrollbar(Some(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .track_symbol(None)
                    .end_symbol(None),
            ))
            .highlight_style(
                Style::new()
                    .fg(Color::Black)
                    .bg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            );

        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Fill(1), Constraint::Max(40)])
            .split(area);

        let d = format!("{:?}", self.state.selected());

        frame.render_stateful_widget(widget, layout[0], &mut self.state);
        frame.render_widget(
            Paragraph::new(d).block(Block::new().borders(Borders::ALL)),
            layout[1],
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
                        app.state.select_relative(|current| {
                            current.map_or(0, |current| current.saturating_add(20))
                        })
                    }
                    KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.state.select_relative(|current| {
                            current.map_or(0, |current| current.saturating_sub(20))
                        })
                    }

                    KeyCode::Char('g') => app.state.select_first(),
                    KeyCode::Char('G') => app.state.select_last(),

                    KeyCode::Char('j') => app.state.select_relative(|current| {
                        current.map_or(0, |current| current.saturating_add(1))
                    }),
                    KeyCode::Char('k') => app.state.select_relative(|current| {
                        current.map_or(0, |current| current.saturating_sub(1))
                    }),
                    KeyCode::Char('h') => app.state.key_left(),
                    KeyCode::Char('l') => {
                        //app.state.key_right()

                        // open node or move down if node already open or empty
                        let state = &mut app.state;
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
                                app.state.select_relative(|current| {
                                    current.map_or(0, |current| current.saturating_add(1))
                                })
                            }
                        } else {
                            false
                        }
                    }

                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('\n' | ' ') => app.state.toggle_selected(),
                    KeyCode::Left => app.state.key_left(),
                    KeyCode::Right => app.state.key_right(),
                    KeyCode::Down => app.state.key_down(),
                    KeyCode::Up => app.state.key_up(),
                    KeyCode::Esc => app.state.select(Some(Vec::new())),
                    KeyCode::Home => app.state.select_first(),
                    KeyCode::End => app.state.select_last(),
                    KeyCode::PageDown => app.state.scroll_down(3),
                    KeyCode::PageUp => app.state.scroll_up(3),
                    _ => false,
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => app.state.scroll_down(1),
                    MouseEventKind::ScrollUp => app.state.scroll_up(1),
                    MouseEventKind::Down(_button) => {
                        app.state.click_at(Position::new(mouse.column, mouse.row))
                    }
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
