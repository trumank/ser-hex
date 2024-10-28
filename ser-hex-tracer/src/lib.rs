use std::{
    collections::BTreeMap,
    io::Read,
    sync::{LazyLock, Mutex},
};

use ser_hex::{Action, ReadSpan, Trace, TreeSpan};

#[derive(Default)]
pub struct TracerOptions {
    /// Number of frames at the top of the stack to skip: e.g. skip frames from the tracer or
    /// other instrumentation functions
    pub skip_frames: usize,
}

#[derive(Default)]
pub struct Tracer {
    offset: usize,
    ops: Vec<Op>,
    options: TracerOptions,
}
pub struct TracerReader<R: Read> {
    tracer: Tracer,
    inner: R,
}
impl<R: Read> TracerReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            tracer: Tracer::new(),
            inner,
        }
    }
    pub fn new_options(inner: R, options: TracerOptions) -> Self {
        Self {
            tracer: Tracer::new_options(options),
            inner,
        }
    }
    pub fn get_trace(&self) -> Trace {
        self.tracer.get_trace()
    }
}
impl<R: Read> Read for TracerReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner
            .read(buf)
            .inspect(|count| self.tracer.read(&buf[..*count]))
    }
}

impl Tracer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn new_options(options: TracerOptions) -> Self {
        Self {
            options,
            ..Default::default()
        }
    }
    pub fn read(&mut self, bytes: &[u8]) {
        let offset = self.offset;
        let mut stack = vec![];
        let mut i = 0;
        backtrace::trace(|frame| {
            i += 1;
            if i > self.options.skip_frames {
                stack.push(frame.clone());
            }
            true
        });
        stack.reverse();

        let count = bytes.len();

        self.ops.push(Op {
            data: bytes.to_vec(),
            offset,
            count,
            stack,
        });

        self.offset += count;
    }
    pub fn get_trace(&self) -> Trace {
        let mut data = vec![];
        for op in &self.ops {
            assert_eq!(op.offset, data.len());
            data.extend_from_slice(&op.data);
        }
        #[derive(Debug)]
        enum TreeNode {
            Frame(Frame),
            Read { count: usize },
        }
        impl TreeNode {
            fn convert(self) -> Action<TreeSpan> {
                match self {
                    TreeNode::Frame(frame) => Action::Span(TreeSpan(ReadSpan::<TreeSpan> {
                        name: symbolize(frame.ip).name.into(),
                        actions: frame.children.into_iter().map(|c| c.convert()).collect(),
                    })),
                    TreeNode::Read { count } => Action::Read(count),
                }
            }
        }
        #[derive(Debug)]
        struct Frame {
            id: u64,
            ip: u64,
            children: Vec<TreeNode>,
        }
        impl Frame {
            fn new(id: u64, ip: u64) -> Self {
                Frame {
                    id,
                    ip,
                    children: Vec::new(),
                }
            }
            fn insert(&mut self, path: &[backtrace::Frame], count: usize) {
                if path.is_empty() {
                    self.children.push(TreeNode::Read { count });
                    return;
                }
                let rest = &path[1..];
                match self.children.last_mut() {
                    Some(TreeNode::Frame(frame)) if frame.id == path[0].symbol_address() as u64 => {
                        frame.insert(rest, count);
                    }
                    _ => {
                        let mut new_child =
                            Frame::new(path[0].symbol_address() as u64, path[0].ip() as u64);
                        new_child.insert(rest, count);
                        self.children.push(TreeNode::Frame(new_child));
                    }
                }
            }
        }
        //if ops.is_empty() {
        //    return None;
        //}
        let mut root = Frame::new(
            self.ops[0].stack[0].symbol_address() as u64,
            self.ops[0].stack[0].ip() as u64,
        );
        for op in &self.ops {
            root.insert(&op.stack, op.count);
        }
        Trace {
            data,
            root: Action::Span(TreeSpan(ReadSpan {
                name: "root".into(),
                actions: vec![TreeNode::Frame(root).convert()],
            })),
        }
    }
}

struct Op {
    data: Vec<u8>,
    offset: usize,
    count: usize,
    stack: Vec<backtrace::Frame>,
}

fn symbolize(ip: u64) -> Symbol {
    SYMBOLS
        .lock()
        .unwrap()
        .entry(ip)
        .or_insert_with(|| {
            let mut name = None;
            let mut addr = None;
            backtrace::resolve(ip as *mut _, |symbol| {
                name = symbol.name().map(|n| n.to_string());
                addr = symbol.addr()
            });
            Symbol {
                ptr: addr.map(|a| a as u64).unwrap_or(0),
                //name: format!("0x{ip:X?} {addr:X?} {}", name.unwrap_or_default()),
                name: name.unwrap_or_else(|| format!("0x{ip:X?}")),
            }
        })
        .clone()
}
#[derive(Debug, Clone)]
pub struct Symbol {
    ptr: u64,
    name: String,
}
pub static SYMBOLS: LazyLock<Mutex<BTreeMap<u64, Symbol>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
