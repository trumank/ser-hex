use std::{
    collections::hash_map::DefaultHasher,
    ops::{Range, RangeBounds},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context as _, Result};
use eframe::{egui, epaint::Hsva, Frame, NativeOptions};
use egui::Context;
use fs_err as fs;

use egui_memory_editor::{MemoryEditor, RenderCtx, SpanQuery};
use intervaltree::IntervalTree;
use notify::INotifyWatcher;
use notify_debouncer_mini::{DebouncedEvent, DebouncedEventKind, Debouncer};
use ser_hex::{Action, ReadSpan};

pub fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(data), Some(trace)) = (args.next(), args.next()) else {
        bail!("usage: ser-hex-viewer <DATA PATH> <TRACE PATH>");
    };
    let data = Data {
        data: fs::read(data).context("Failed to load data")?,
    };
    let trace = FileTrace::new(&data, trace).context("Failed to load trace")?;

    let app = App::new(data, vec![trace])?;
    let _ = eframe::run_native(
        "Ser-Hex viewer",
        NativeOptions::default(),
        Box::new(|_cc| Box::new(app)),
    );
    Ok(())
}

type SparseTreeSpan = ReadSpan<ser_hex::TreeSpan>;

trait SparseTreeSpanTrait {
    fn build_tree(&self) -> IntervalTree<usize, FlatSpan>;
    fn collect_spans(&self, index: &mut usize, path: &mut Vec<usize>, spans: &mut Vec<FlatSpan>);
    fn build_full_spans(&self, index: &mut usize) -> FullTreeSpan;
}

impl SparseTreeSpanTrait for SparseTreeSpan {
    fn build_tree(&self) -> IntervalTree<usize, FlatSpan> {
        let mut index = 0;

        let mut spans = vec![];
        let mut path = vec![];
        self.collect_spans(&mut index, &mut path, &mut spans);
        IntervalTree::from_iter(spans.into_iter().map(|s| intervaltree::Element {
            range: s.range.clone(),
            value: s,
        }))
    }
    fn collect_spans(&self, index: &mut usize, path: &mut Vec<usize>, spans: &mut Vec<FlatSpan>) {
        path.push(0);
        for (i, action) in self.actions.iter().enumerate() {
            *path.last_mut().unwrap() = i;
            match action {
                Action::Read(size) => {
                    spans.push(FlatSpan {
                        range: *index..*index + size,
                        name: self.name.to_string(),
                        path: path.clone(),
                    });
                    *index += size;
                }
                Action::Seek(i) => {
                    /*
                    spans.push(TreeSpan {
                        range: *index..*index + size,
                        name: "read".into(),
                    });
                    */
                    *index = *i;
                }
                Action::Span(span) => {
                    span.0.collect_spans(index, path, spans);
                }
            }
        }
        path.pop();
    }
    fn build_full_spans(&self, index: &mut usize) -> FullTreeSpan {
        FullTreeSpan {
            name: self.name.to_string(),
            actions: self
                .actions
                .iter()
                .map(|action| match action {
                    Action::Read(size) => {
                        let start = *index;
                        *index += size;
                        FullAction::Read(start..*index)
                    }
                    Action::Seek(i) => {
                        let start = *index;
                        *index = *i;
                        FullAction::Seek(start, *index)
                    }
                    Action::Span(span) => FullAction::Span(span.0.build_full_spans(index)),
                })
                .collect(),
        }
    }
}
impl FullTreeSpan {
    fn ui(&self, ui: &mut egui::Ui, index: usize, path_select: Option<&[usize]>) -> TreeResponse {
        let mut res = TreeResponse::None;
        egui::CollapsingHeader::new(self.name.as_str())
            .open(path_select.map(|p| p.first() == Some(&index)))
            .show(ui, |ui| {
                let mut ui_action =
                    |ui: &mut egui::Ui,
                     index: usize,
                     action: &FullAction,
                     path_select: Option<&[usize]>| match action {
                        FullAction::Read(range) => {
                            let scroll_to_me = path_select
                                .and_then(|p| {
                                    p.split_first().and_then(|(first, rest)| {
                                        (*first == index && rest.is_empty()).then_some(true)
                                    })
                                })
                                .unwrap_or_default();
                            let button_res = ui.button(format!("read {}", range.len()));
                            if scroll_to_me {
                                button_res.scroll_to_me(None);
                            }
                            if button_res.clicked() {
                                res = TreeResponse::Goto(range.start);
                            }
                        }
                        FullAction::Seek(from, to) => {
                            ui.label(format!("seek {} => {}", from, to));
                        }
                        FullAction::Span(s) => {
                            ui.push_id(index, |ui| {
                                let r = s.ui(ui, index, path_select);
                                if !matches!(r, TreeResponse::None) {
                                    res = r
                                };
                            });
                        }
                    };
                let n = 100;
                let path_select = path_select.and_then(|p| {
                    p.split_first()
                        .and_then(|(first, rest)| (*first == index).then_some(rest))
                });
                for (i, chunk) in self.actions.chunks(n).enumerate() {
                    let base_index = n * i;
                    if self.actions.len() > n {
                        egui::CollapsingHeader::new(format!(
                            "{}-{}:",
                            base_index,
                            base_index + chunk.len()
                        ))
                        .open(path_select.map(|p| {
                            p.first()
                                .map(|p| (base_index..base_index + n).contains(p))
                                .unwrap_or_default()
                        }))
                        .show(ui, |ui| {
                            for (ci, action) in chunk.iter().enumerate() {
                                ui_action(ui, base_index + ci, action, path_select);
                            }
                        });
                    } else {
                        for (ci, action) in chunk.iter().enumerate() {
                            ui_action(ui, base_index + ci, action, path_select);
                        }
                    }
                }
            });
        res
    }
}

#[derive(Debug, Clone)]
enum TreeResponse {
    None,
    Goto(usize),
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct FlatSpan {
    range: Range<usize>,
    name: String,
    path: Vec<usize>,
}
impl RangeBounds<usize> for FlatSpan {
    fn start_bound(&self) -> std::ops::Bound<&usize> {
        self.range.start_bound()
    }
    fn end_bound(&self) -> std::ops::Bound<&usize> {
        self.range.end_bound()
    }
}

#[derive(Debug)]
pub enum FullAction {
    Read(Range<usize>),
    Seek(usize, usize), // from, to
    Span(FullTreeSpan),
}

#[derive(Debug)]
pub struct FullTreeSpan {
    pub name: String,
    pub actions: Vec<FullAction>,
}

pub struct Trace {
    full_tree: FullTreeSpan,
    interval_tree: IntervalTree<usize, FlatSpan>,
    mem_editor: MemoryEditor,
    is_open: bool,
}
impl Trace {
    fn load<P: AsRef<Path>>(memory: &Data, path: P) -> Result<Self> {
        let file = fs::File::open(path.as_ref())?;
        let reader = std::io::BufReader::new(file);

        let data: SparseTreeSpan = serde_json::from_reader(reader)?;

        let interval_tree = data.build_tree();
        let full_tree = data.build_full_spans(&mut 0);

        let mut mem_editor = MemoryEditor::new()
            .with_address_range("All", 0..memory.data.len())
            .with_window_title(path.as_ref().to_string_lossy());

        mem_editor.options.column_count = 16;

        Ok(Trace {
            full_tree,
            interval_tree,
            mem_editor,
            is_open: true,
        })
    }
}

struct FileTrace {
    path: PathBuf,
    trace: Trace,
}
impl FileTrace {
    fn new<P: AsRef<Path>>(data: &Data, path: P) -> Result<Self> {
        Ok(Self {
            trace: Trace::load(data, &path)?,
            path: fs::canonicalize(path)?,
        })
    }
    fn reload(&mut self, data: &Data) -> Result<()> {
        self.trace = Trace::load(data, &self.path)?;
        Ok(())
    }
}

pub struct App {
    data: Data,
    traces: Vec<FileTrace>,
    path_select: Option<Vec<usize>>,
    watcher: Option<Debouncer<INotifyWatcher>>,
    rx: Option<std::sync::mpsc::Receiver<PathBuf>>,
}
impl App {
    fn new(data: Data, traces: Vec<FileTrace>) -> Result<Self> {
        Ok(Self {
            data,
            traces,
            path_select: None,
            watcher: None,
            rx: None,
        })
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        if let Some(rx) = &self.rx {
            for path in rx.try_iter() {
                let trace = self.traces.iter_mut().find(|trace| trace.path == path);
                println!("reloading {path:?}");
                if let Err(err) = trace.expect("missing?").reload(&self.data) {
                    eprintln!("failed to reload trace {err:?}")
                }
            }
        } else {
            let ctx = ctx.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            self.rx = Some(rx);
            let mut watcher = notify_debouncer_mini::new_debouncer(
                std::time::Duration::from_millis(200),
                move |res| match res {
                    Ok(events) => {
                        for event in events {
                            if let DebouncedEvent {
                                path,
                                kind: DebouncedEventKind::Any,
                            } = event
                            {
                                tx.send(path).unwrap()
                            }
                        }
                        ctx.request_repaint();
                    }
                    Err(err) => eprintln!("error with watcher {err}"),
                },
            )
            .unwrap();
            for trace in &self.traces {
                watcher
                    .watcher()
                    .watch(&trace.path, notify::RecursiveMode::NonRecursive)
                    .unwrap();
            }
            self.watcher = Some(watcher);
        }

        struct SpanQueryImpl<'tree> {
            tree: &'tree IntervalTree<usize, FlatSpan>,
        }
        impl<'tree> SpanQuery for SpanQueryImpl<'tree> {
            fn get_spans<'a>(
                &'a self,
                range: Range<egui_memory_editor::Address>,
            ) -> Box<dyn Iterator<Item = egui_memory_editor::Span> + 'a> {
                Box::new(self.tree.query(range).map(|r| {
                    use std::hash::Hash;
                    use std::hash::Hasher;
                    let mut s = DefaultHasher::new();
                    r.value.name.hash(&mut s);

                    let hash = s.finish();

                    let color = Hsva::new((hash % 256) as f32 / 256.0, 1., 0.5, 1.);

                    egui_memory_editor::Span {
                        range: r.range.clone(),
                        color: color.into(),
                    }
                }))
            }
        }

        for trace in &mut self.traces {
            let interval_tree = &trace.trace.interval_tree;
            let full_tree = &trace.trace.full_tree;

            let span_query = Box::new(SpanQueryImpl {
                tree: interval_tree,
            });
            let hover_byte = Box::new(|ui: &mut egui::Ui, address| {
                for range in interval_tree.query_point(address) {
                    ui.label(format!("{address}: {}", range.value.name));
                    let mut span = full_tree;
                    ui.label(format!("{}, span: {}", 0, span.name));
                    for (depth, span_index) in range.value.path.iter().enumerate() {
                        match &span.actions[*span_index] {
                            FullAction::Read(range) => {
                                ui.label(format!("{}, read: {}", depth + 1, range.len()));
                            }
                            FullAction::Seek(from, to) => {
                                ui.label(format!("{}, seek: {} => {}", depth + 1, from, to));
                            }
                            FullAction::Span(s) => {
                                span = s;
                                ui.label(format!("{}, span: {}", depth + 1, s.name));
                            }
                        }
                    }
                }
            });
            let color_byte = Box::new(|address| {
                if let Some(first) = interval_tree.query_point(address).next() {
                    use std::hash::Hash;
                    use std::hash::Hasher;
                    let mut s = DefaultHasher::new();
                    first.value.name.hash(&mut s);

                    let hash = s.finish();

                    egui::Color32::from_rgb(hash as u8, (hash >> 8) as u8, (hash >> 16) as u8)
                } else {
                    egui::Color32::BROWN
                }
            });

            egui::Window::new("trace" /*trace.mem_editor.window_name.clone()*/)
                .open(&mut trace.trace.is_open)
                .hscroll(false)
                .vscroll(false)
                .resizable(true)
                .show(ctx, |ui| {
                    let mut tree_res = TreeResponse::None;
                    //self.shrink_window_ui(ui);
                    egui::SidePanel::left("left").show_inside(ui, |ui| {
                        ui.label("side panel");
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            tree_res =
                                trace
                                    .trace
                                    .full_tree
                                    .ui(ui, 0, self.path_select.take().as_deref())
                        });
                    });

                    // https://github.com/emilk/egui/issues/901
                    egui::TopBottomPanel::bottom("bottom")
                        .show_separator_line(false)
                        .show_inside(ui, |_| ());

                    egui::CentralPanel::default().show_inside(ui, |ui| {
                        match tree_res {
                            TreeResponse::None => {}
                            TreeResponse::Goto(address) => {
                                trace
                                    .trace
                                    .mem_editor
                                    .frame_data
                                    .set_highlight_address(address);
                                trace.trace.mem_editor.frame_data.goto_address_line =
                                    Some(address / trace.trace.mem_editor.options.column_count);
                            }
                        }
                        let prev_selection =
                            trace.trace.mem_editor.frame_data.selected_highlight_address;
                        trace.trace.mem_editor.draw_editor_contents_read_only(
                            ui,
                            &mut self.data,
                            |data, address| data.read_value(address).into(),
                            RenderCtx {
                                span_query,
                                hover_byte,
                                color_byte,
                            },
                        );
                        let new_selection =
                            trace.trace.mem_editor.frame_data.selected_highlight_address;
                        if prev_selection != new_selection {
                            if let Some(selection) = new_selection {
                                // TODO find "narrowest" span in case of multiple
                                if let Some(span) =
                                    interval_tree.query(selection..selection + 1).next()
                                {
                                    let mut path_select = vec![0];
                                    path_select.extend(&span.value.path);
                                    self.path_select = Some(path_select);
                                }
                            }
                        }
                    });
                });
        }
    }
}

pub struct Data {
    data: Vec<u8>,
}

impl Data {
    pub fn read_value(&mut self, address: usize) -> u8 {
        self.data[address]
    }

    pub fn write_value(&mut self, address: usize, val: u8) {
        self.data[address] = val
    }
}
