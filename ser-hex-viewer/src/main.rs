use std::{collections::hash_map::DefaultHasher, ops::RangeBounds, path::Path};

use anyhow::{bail, Context as _, Result};
use eframe::{egui, epaint::Hsva, Frame, NativeOptions};
use egui::Context;

use egui_memory_editor::{MemoryEditor, RenderCtx, SpanQuery};
use intervaltree::IntervalTree;
use ser_hex::{Action, ReadSpan};

pub fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(data), Some(trace)) = (args.next(), args.next()) else {
        bail!("usage: ser-hex-viewer <DATA PATH> <TRACE PATH>");
    };
    let data = Data {
        data: std::fs::read(data).with_context(|| "Failed to data from path {data}")?,
    };
    let trace = Trace::load(&data, trace).with_context(|| "Failed to data: {trace}")?;

    let app = App::new(data, vec![trace]);
    let _ = eframe::run_native(
        "Ser-Hex viewer",
        NativeOptions::default(),
        Box::new(|_cc| Box::new(app)),
    );
    Ok(())
}

type Span = ReadSpan<ser_hex::TreeSpan>;

trait SpanTreeBuilder {
    fn build_tree(&self) -> IntervalTree<usize, TreeSpan>;
    fn collect_spans(&self, index: &mut usize, path: &mut Vec<usize>, spans: &mut Vec<TreeSpan>);
}

impl SpanTreeBuilder for Span {
    fn build_tree(&self) -> IntervalTree<usize, TreeSpan> {
        let mut index = 0;

        let mut spans = vec![];
        let mut path = vec![];
        self.collect_spans(&mut index, &mut path, &mut spans);
        IntervalTree::from_iter(spans.into_iter().map(|s| intervaltree::Element {
            range: s.range.clone(),
            value: s,
        }))
    }
    fn collect_spans(&self, index: &mut usize, path: &mut Vec<usize>, spans: &mut Vec<TreeSpan>) {
        path.push(0);
        for (i, action) in self.actions.iter().enumerate() {
            *path.last_mut().unwrap() = i;
            match action {
                Action::Read(size) => {
                    spans.push(TreeSpan {
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
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct TreeSpan {
    range: std::ops::Range<usize>,
    name: String,
    path: Vec<usize>,
}
impl RangeBounds<usize> for TreeSpan {
    fn start_bound(&self) -> std::ops::Bound<&usize> {
        self.range.start_bound()
    }
    fn end_bound(&self) -> std::ops::Bound<&usize> {
        self.range.end_bound()
    }
}

pub struct Trace {
    data: Span,
    interval_tree: IntervalTree<usize, TreeSpan>,
    mem_editor: MemoryEditor,
    is_open: bool,
}
impl Trace {
    fn load<P: AsRef<Path>>(memory: &Data, path: P) -> Result<Self> {
        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);

        let data: Span = serde_json::from_reader(reader)?;

        let interval_tree = data.build_tree();

        let mut mem_editor = MemoryEditor::new()
            .with_address_range("All", 0..memory.data.len())
            .with_window_title(path.as_ref().to_string_lossy());

        mem_editor.options.column_count = 16;

        Ok(Trace {
            data,
            interval_tree,
            mem_editor,
            is_open: true,
        })
    }
}

pub struct App {
    data: Data,
    traces: Vec<Trace>,
}
impl App {
    fn new(data: Data, traces: Vec<Trace>) -> Self {
        Self { data, traces }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut Frame) {
        struct SpanQueryImpl<'tree> {
            tree: &'tree IntervalTree<usize, TreeSpan>,
        }
        impl<'tree> SpanQuery for SpanQueryImpl<'tree> {
            fn get_spans<'a>(
                &'a self,
                range: std::ops::Range<egui_memory_editor::Address>,
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
            let interval_tree = &trace.interval_tree;
            let data = &trace.data;
            trace.mem_editor.window_ui_read_only(
                ctx,
                &mut trace.is_open,
                &mut self.data,
                |data, address| data.read_value(address).into(),
                RenderCtx {
                    span_query: Box::new(SpanQueryImpl {
                        tree: interval_tree,
                    }),
                    hover_byte: Box::new(|ui: &mut egui::Ui, address| {
                        for range in interval_tree.query_point(address) {
                            ui.label(format!("{address}: {}", range.value.name));
                            let mut span = data;
                            ui.label(format!("{}, span: {}", 0, span.name));
                            for (depth, span_index) in range.value.path.iter().enumerate() {
                                match &span.actions[*span_index] {
                                    Action::Read(s) => {
                                        ui.label(format!("{}, read: {}", depth + 1, s));
                                    }
                                    Action::Seek(s) => {
                                        ui.label(format!("{}, seek: {}", depth + 1, s));
                                    }
                                    Action::Span(s) => {
                                        span = &s.0;
                                        ui.label(format!("{}, span: {}", depth + 1, s.0.name));
                                    }
                                }
                            }
                        }
                    }),
                    color_byte: Box::new(|address| {
                        if let Some(first) = interval_tree.query_point(address).next() {
                            use std::hash::Hash;
                            use std::hash::Hasher;
                            let mut s = DefaultHasher::new();
                            first.value.name.hash(&mut s);

                            let hash = s.finish();

                            egui::Color32::from_rgb(
                                hash as u8,
                                (hash >> 8) as u8,
                                (hash >> 16) as u8,
                            )
                        } else {
                            egui::Color32::BROWN
                        }
                    }),
                },
            );
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
