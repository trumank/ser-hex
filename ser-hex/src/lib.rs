use serde::{Deserialize, Serialize};
use tracing::{
    span::{self, EnteredSpan},
    subscriber::{self, DefaultGuard, Subscriber},
    Event, Id, Metadata,
};
use tracing_core::span::Current;

use std::{
    collections::HashMap,
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Build a stream (Cursor<Vec<u8>>) mirroring all the data in the underlying stream and cursor position
fn build_mirror<S: Read + Seek>(stream: &mut S) -> Result<Cursor<Vec<u8>>, io::Error> {
    let pos = stream.stream_position()?;
    stream.seek(SeekFrom::Start(0))?;
    let mut data = vec![];
    stream.read_to_end(&mut data)?;
    let mut cursor = Cursor::new(data);
    stream.seek(SeekFrom::Start(pos))?;
    cursor.seek(SeekFrom::Start(pos))?;
    Ok(cursor)
}

pub fn read<'t, 'r: 't, P: AsRef<Path>, R: Read + Seek + 'r, F, T>(
    out_path: P,
    reader: &'r mut R,
    f: F,
) -> T
where
    F: FnOnce(&mut TraceStream<&'r mut R>) -> T,
{
    let cursor = build_mirror(reader).unwrap();
    CounterSubscriber::read(out_path.as_ref().to_owned(), Some(cursor), reader, f)
}

pub fn read_incremental<'t, 'r: 't, P: AsRef<Path>, R: Read + 'r, F, T>(
    out_path: P,
    reader: &'r mut R,
    f: F,
) -> T
where
    F: FnOnce(&mut TraceStream<&'r mut R>) -> T,
{
    CounterSubscriber::read(out_path.as_ref().to_owned(), None, reader, f)
}

pub struct TraceStream<S> {
    stream: S,

    // first drop span
    #[allow(unused)]
    scope_guard: EnteredSpan,

    // then drop subscriber guard
    #[allow(unused)]
    guard: Option<DefaultGuard>,

    // finally drop subscriber which writes trace
    subscriber: CounterSubscriber,
}

impl<S: Read + Seek> TraceStream<S> {
    pub fn new<P: Into<PathBuf>>(trace_path: P, mut inner_stream: S) -> Self {
        let cursor = build_mirror(&mut inner_stream).unwrap();
        let subscriber = CounterSubscriber::new(trace_path.into(), cursor);
        let guard = Some(tracing::subscriber::set_default(subscriber.clone()));
        Self::new_internal(inner_stream, subscriber, guard)
    }
}
impl<S> TraceStream<S> {
    pub fn new_incremental<P: Into<PathBuf>>(trace_path: P, inner_stream: S) -> Self {
        let subscriber = CounterSubscriber::new(trace_path.into(), Cursor::new(vec![]));
        let guard = Some(tracing::subscriber::set_default(subscriber.clone()));
        Self::new_internal(inner_stream, subscriber, guard)
    }
}
impl<S> TraceStream<S> {
    fn new_internal(stream: S, subscriber: CounterSubscriber, guard: Option<DefaultGuard>) -> Self {
        Self {
            stream,
            scope_guard: tracing::info_span!("root").entered(),
            guard,
            subscriber,
        }
    }
}
impl<R: Read + Seek> Seek for TraceStream<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.stream
            .seek(pos)
            .inspect(|&to| self.subscriber.seek_action(to))
    }
}
impl<R: Read> Read for TraceStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stream
            .read(buf)
            .inspect(|&s| self.subscriber.read_action(buf, s))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Action<S> {
    Read(usize),
    Seek(usize),
    Span(S),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReadSpan<S = TreeSpan> {
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
    start_index: usize,
    data: Cursor<Vec<u8>>,
    last_id: u64,
    root_span: Option<Id>,
    spans: HashMap<Id, ReadSpan<Id>>,
    metadata: HashMap<Id, &'static Metadata<'static>>,
    stack: Vec<Id>,
}
impl CounterSubscriberInner {
    fn new(out_path: PathBuf, mut data: Cursor<Vec<u8>>) -> Self {
        Self {
            out_path,
            start_index: data.stream_position().unwrap() as usize,
            data,
            last_id: Default::default(),
            root_span: Default::default(),
            spans: Default::default(),
            metadata: Default::default(),
            stack: Default::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Trace<D: AsRef<[u8]> = Vec<u8>> {
    #[serde(
        serialize_with = "base64::serialize",
        deserialize_with = "base64::deserialize",
        bound(deserialize = "D: From<Vec<u8>>")
    )]
    pub data: D,
    pub start_index: usize,
    pub root: Action<TreeSpan>,
}
impl<D: AsRef<[u8]>> Trace<D> {
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), std::io::Error> {
        let json = serde_json::to_string(&self).unwrap();
        fs::write(path, json)
    }
}

mod base64 {
    use base64::prelude::*;
    use serde::{Deserialize, Serialize};
    use serde::{Deserializer, Serializer};

    pub fn serialize<V, S: Serializer>(v: V, s: S) -> Result<S::Ok, S::Error>
    where
        V: AsRef<[u8]>,
    {
        let base64 = BASE64_STANDARD.encode(v.as_ref());
        String::serialize(&base64, s)
    }

    pub fn deserialize<'de, V: From<Vec<u8>>, D: Deserializer<'de>>(d: D) -> Result<V, D::Error> {
        let base64 = String::deserialize(d)?;
        BASE64_STANDARD
            .decode(base64.as_bytes())
            .map_err(serde::de::Error::custom)
            .map(|v| v.into())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TreeSpan(pub ReadSpan);
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
        Trace {
            data: std::mem::take(&mut self.data).into_inner(),
            start_index: self.start_index,
            root: Action::Span(tree),
        }
        .save(&self.out_path)
        .unwrap()
    }
}

#[derive(Clone)]
struct CounterSubscriber {
    inner: Arc<Mutex<CounterSubscriberInner>>,
}
impl CounterSubscriber {
    fn new(out_path: PathBuf, data: Cursor<Vec<u8>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(CounterSubscriberInner::new(out_path, data))),
        }
    }
    fn read<'d, 't, 'r: 't, R: Read + 'r, P, F, T>(
        out_path: P,
        data: Option<Cursor<Vec<u8>>>,
        reader: &'r mut R,
        f: F,
    ) -> T
    where
        F: FnOnce(&mut TraceStream<&'r mut R>) -> T,
        P: Into<PathBuf>,
    {
        let sub = Self::new(out_path.into(), data.unwrap_or_default());
        tracing::subscriber::with_default(sub.clone(), || {
            // must build TraceStream after defualt subscriber is set because it enters root span
            f(&mut TraceStream::new_internal(reader, sub, None))
        })
    }
    fn read_action(&self, buf: &[u8], size: usize) {
        let mut lock = self.inner.lock().unwrap();
        let current = lock.stack.last().cloned().unwrap();
        lock.data.write_all(&buf[..size]).unwrap();
        lock.spans
            .get_mut(&current)
            .unwrap()
            .actions
            .push(Action::Read(size));
    }
    fn seek_action(&self, to: u64) {
        let mut lock = self.inner.lock().unwrap();
        let current = lock.stack.last().cloned().unwrap();
        lock.data.seek(SeekFrom::Start(to)).unwrap();
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
        reader.seek(std::io::SeekFrom::Current(1))?;
        let _c = reader.read_u8()?;
        reader.seek(std::io::SeekFrom::Current(-1))?;
        let _c = reader.read_u8()?;
        Ok(())
    }

    fn new_reader() -> Cursor<Vec<u8>> {
        let mut reader = std::io::Cursor::new(vec![
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20,
        ]);
        reader.seek(SeekFrom::Start(2)).unwrap();
        reader
    }

    #[test]
    fn test_trace_read() -> Result<(), Error> {
        read("trace_read.json", &mut new_reader(), |s| {
            read_stuff(s)?;
            read_stuff(s)
        })?;

        Ok(())
    }

    #[test]
    fn test_trace_read_incremental() -> Result<(), Error> {
        read_incremental("trace_read_incremental.json", &mut new_reader(), |s| {
            read_stuff(s)?;
            read_stuff(s)
        })?;

        Ok(())
    }

    #[test]
    fn test_trace_stream() -> Result<(), Error> {
        let mut s = TraceStream::new("trace_stream.json", new_reader());
        read_stuff(&mut s)?;
        read_stuff(&mut s)?;

        Ok(())
    }

    #[test]
    fn test_trace_stream_incremental() -> Result<(), Error> {
        let mut s = TraceStream::new_incremental("trace_stream_incremental.json", new_reader());
        read_stuff(&mut s)?;
        read_stuff(&mut s)?;

        Ok(())
    }
}
