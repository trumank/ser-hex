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
    data: Vec<u8>,
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
    pub fn tracer(&self) -> &Tracer {
        &self.tracer
    }
    pub fn trace(&self) -> Trace<&[u8]> {
        self.tracer.trace()
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
    pub fn data(&self) -> &[u8] {
        &self.data
    }
    pub fn read(&mut self, bytes: &[u8]) {
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

        self.ops.push(Op {
            count: bytes.len(),
            stack,
        });

        self.data.extend(bytes);
    }
    pub fn trace(&self) -> Trace<&[u8]> {
        #[derive(Debug)]
        enum TreeNode {
            Frame(Frame),
            Read { count: usize },
        }
        impl TreeNode {
            fn convert(self) -> Action<TreeSpan> {
                match self {
                    TreeNode::Frame(frame) => Action::Span(TreeSpan(ReadSpan {
                        name: symbolize(frame.ip, frame.id).name.into(),
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

        // find common frames at bottom of stack
        let mut skip_start = 0;
        if let Some(first) = self.ops.first() {
            'find_start: while let Some(start) = first.stack.get(skip_start) {
                let start_addr = start.symbol_address();
                for op in &self.ops {
                    let c = op.stack.get(skip_start);
                    if c.is_none() || c.unwrap().symbol_address() != start_addr {
                        break 'find_start;
                    }
                }
                skip_start += 1;
            }
        }

        // find common frames at top of stack
        let mut skip_end = 0;
        if let Some(first) = self.ops.first() {
            let s = &first.stack[skip_start..];
            'find_end: while let Some(end) =
                s.len().checked_sub(skip_end + 1).and_then(|i| s.get(i))
            {
                let start_addr = end.symbol_address();
                for op in &self.ops {
                    let s = &op.stack[skip_start..];
                    let c = s.len().checked_sub(skip_end + 1).and_then(|i| s.get(i));
                    if c.is_none() || c.unwrap().symbol_address() != start_addr {
                        break 'find_end;
                    }
                }
                skip_end += 1;
            }
        }

        let root = self.ops.first().map(|root| {
            let stack = &root.stack[skip_start..(root.stack.len() - skip_end)];
            let mut root = Frame::new(stack[0].symbol_address() as u64, stack[0].ip() as u64);
            for op in &self.ops {
                root.insert(&op.stack[skip_start..(op.stack.len() - skip_end)], op.count);
            }
            TreeNode::Frame(root).convert()
        });
        Trace {
            data: &self.data,
            start_index: 0,
            root: Action::Span(TreeSpan(ReadSpan {
                name: "root".into(),
                actions: root.into_iter().collect(),
            })),
        }
    }
}

struct Op {
    count: usize,
    stack: Vec<backtrace::Frame>,
}

fn symbolize(ip: u64, id: u64) -> Symbol {
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
                //ptr: addr.map(|a| a as u64).unwrap_or(0),
                //name: format!("0x{ip:X?} {addr:X?} {}", name.unwrap_or_default()),
                name: name.unwrap_or_else(|| format!("0x{id:X?}")),
            }
        })
        .clone()
}
#[derive(Debug, Clone)]
pub struct Symbol {
    //ptr: u64,
    name: String,
}
pub static SYMBOLS: LazyLock<Mutex<BTreeMap<u64, Symbol>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));
