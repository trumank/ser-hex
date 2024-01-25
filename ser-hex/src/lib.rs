use serde::{Deserialize, Serialize};
use tracing::{
    span,
    subscriber::{self, Subscriber},
    Event, Id, Metadata,
};
use tracing_core::span::Current;

use std::{
    collections::HashMap,
    fs,
    io::{Read, Seek},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub fn read<'t, 'r: 't, P: AsRef<Path>, R: Read + 'r, F, T>(
    out_path: P,
    reader: &'r mut R,
    f: F,
) -> T
where
    F: Fn(&mut TraceReader<&'r mut R>) -> T,
{
    CounterSubscriber::read(out_path.as_ref().to_owned(), reader, f)
}

pub struct TraceReader<R: Read> {
    reader: R,
    sub: CounterSubscriber,
}

impl<R: Read> TraceReader<R> {
    fn new(reader: R, sub: CounterSubscriber) -> Self {
        Self { reader, sub }
    }
}
impl<R: Read + Seek> Seek for TraceReader<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(pos).map(|to| {
            self.sub.seek_action(to);
            to
        })
    }
}
impl<R: Read> Read for TraceReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf).map(|s| {
            self.sub.read_action(s);
            s
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Action<S> {
    Read(usize),
    Seek(usize),
    Span(S),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadSpan<S> {
    pub name: std::borrow::Cow<'static, str>,
    pub actions: Vec<Action<S>>,
}
impl<S> ReadSpan<S> {
    fn new(name: &'static str) -> Self {
        Self {
            name: name.into(),
            actions: vec![],
        }
    }
}

struct CounterSubscriberInner {
    out_path: PathBuf,
    last_id: u64,
    root_span: Option<Id>,
    spans: HashMap<Id, ReadSpan<Id>>,
    metadata: HashMap<Id, &'static Metadata<'static>>,
    stack: Vec<Id>,
}
impl CounterSubscriberInner {
    fn new(out_path: PathBuf) -> Self {
        Self {
            out_path,
            last_id: Default::default(),
            root_span: Default::default(),
            spans: Default::default(),
            metadata: Default::default(),
            stack: Default::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TreeSpan(pub ReadSpan<TreeSpan>);
impl TreeSpan {
    fn into_tree(id: Id, spans: &mut HashMap<Id, ReadSpan<Id>>) -> Self {
        let read_span = spans.remove(&id).unwrap();
        Self(ReadSpan {
            name: read_span.name,
            actions: read_span
                .actions
                .into_iter()
                .map(|a| match a {
                    Action::Read(i) => Action::Read(i),
                    Action::Seek(i) => Action::Seek(i),
                    Action::Span(id) => Action::Span(Self::into_tree(id, spans)),
                })
                .collect(),
        })
    }
}

impl Drop for CounterSubscriberInner {
    fn drop(&mut self) {
        let tree = TreeSpan::into_tree(self.root_span.as_ref().cloned().unwrap(), &mut self.spans);
        let json = serde_json::to_string(&tree).unwrap();
        fs::write(&self.out_path, json).unwrap();
    }
}

#[derive(Clone)]
pub struct CounterSubscriber {
    inner: Arc<Mutex<CounterSubscriberInner>>,
}
impl CounterSubscriber {
    fn new(out_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CounterSubscriberInner::new(out_path))),
        }
    }
    pub fn read<'t, 'r: 't, R: Read + 'r, P, F, T>(out_path: P, reader: &'r mut R, f: F) -> T
    where
        F: Fn(&mut TraceReader<&'r mut R>) -> T,
        P: Into<PathBuf>,
    {
        let sub = Self::new(out_path.into());
        let mut reader = TraceReader::new(reader, sub.clone());
        tracing::subscriber::with_default(sub, || f(&mut reader))
    }
    fn read_action(&self, size: usize) {
        let mut lock = self.inner.lock().unwrap();
        let current = lock.stack.last().cloned().unwrap();
        lock.spans
            .get_mut(&current)
            .unwrap()
            .actions
            .push(Action::Read(size));
    }
    fn seek_action(&self, to: u64) {
        let mut lock = self.inner.lock().unwrap();
        let current = lock.stack.last().cloned().unwrap();
        lock.spans
            .get_mut(&current)
            .unwrap()
            .actions
            .push(Action::Seek(to as usize));
    }
}

impl Subscriber for CounterSubscriber {
    fn register_callsite(&self, _meta: &Metadata<'_>) -> subscriber::Interest {
        subscriber::Interest::always()
    }

    fn new_span(&self, new_span: &span::Attributes<'_>) -> Id {
        let mut lock = self.inner.lock().unwrap();

        let metadata = new_span.metadata();
        let name = metadata.name();
        lock.last_id += 1;
        let id = lock.last_id;
        let id = Id::from_u64(id);

        lock.spans.insert(id.clone(), ReadSpan::new(name));
        lock.metadata.insert(id.clone(), metadata);
        assert_eq!(new_span.parent(), None);
        assert!(new_span.is_contextual());
        // TODO set root here if new_span.is_root()?
        id
    }
    fn try_close(&self, _id: Id) -> bool {
        true
    }
    fn current_span(&self) -> Current {
        let lock = self.inner.lock().unwrap();
        if let Some(id) = lock.stack.last() {
            let metadata = lock.metadata[id];
            Current::new(id.clone(), metadata)
        } else {
            Current::none()
        }
    }

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}
    fn record(&self, _: &Id, _values: &span::Record<'_>) {}
    fn event(&self, _event: &Event<'_>) {}

    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn enter(&self, span: &Id) {
        let mut lock = self.inner.lock().unwrap();
        if let Some(current) = lock.stack.last().cloned() {
            lock.spans
                .get_mut(&current)
                .unwrap()
                .actions
                .push(Action::Span(span.clone()));
        } else {
            lock.root_span = Some(span.clone());
        }
        lock.stack.push(span.clone());
    }
    fn exit(&self, span: &Id) {
        let mut lock = self.inner.lock().unwrap();
        assert_eq!(&lock.stack.pop().unwrap(), span);
    }
}

#[cfg(test)]
mod test {
    use std::io::Error;

    use byteorder::{ReadBytesExt, LE};
    use tracing::instrument;

    use super::*;

    #[instrument(name = "read_nested_stuff", skip_all)]
    fn read_nested_stuff<R: Read + Seek>(reader: &mut R) -> Result<(), Error> {
        let _a = reader.read_u32::<LE>()?;
        Ok(())
    }

    #[instrument(name = "read_stuff", skip_all)]
    fn read_stuff<R: Read + Seek>(reader: &mut R) -> Result<(), Error> {
        let _a = reader.read_u8()?;
        read_nested_stuff(reader)?;
        reader.seek(std::io::SeekFrom::Current(-1))?;
        let _c = reader.read_u8()?;
        Ok(())
    }

    #[test]
    fn test_trace() -> Result<(), Error> {
        let mut reader = std::io::Cursor::new(vec![1, 2, 3, 4, 5, 6]);

        read("trace.json", &mut reader, read_stuff)?;

        Ok(())
    }
}
