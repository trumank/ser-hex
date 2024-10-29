#![allow(clippy::missing_transmute_annotations)]

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "windows")]
mod windows;

retour::static_detour! {
    static MapDeserialiser: unsafe extern "system" fn(*const (), *const (), *const(), OwnsStream);
}

struct Ctx {
    map_deserialiser: u64,
}

fn main(ctx: Ctx) {
    println!("Intercepted main!");

    unsafe {
        MapDeserialiser
            .initialize(std::mem::transmute(ctx.map_deserialiser), hook_deserialiser)
            .unwrap();
        MapDeserialiser.enable().unwrap();
    }
}

#[repr(C)]
struct StreamVTable {
    #[cfg(target_os = "linux")]
    dtor: unsafe extern "system" fn(this: *mut *const StreamVTable),
    dtor2: unsafe extern "system" fn(this: *mut *const StreamVTable),
    read:
        unsafe extern "system" fn(this: *mut *const StreamVTable, target: *mut u8, len: u32) -> u32,
    read_byte: unsafe extern "system" fn(this: *mut *const StreamVTable, target: *mut u8) -> u32,
    eof: unsafe extern "system" fn(this: *const *const StreamVTable) -> bool,
    remaining: unsafe extern "system" fn(this: *const *const StreamVTable) -> u64,
}

#[repr(C)]
struct StreamProxy {
    vtable: &'static StreamVTable,
    inner: *mut *const StreamVTable,
    owns_stream: OwnsStream,
    tracer: ser_hex_tracer::Tracer,
}
impl Drop for StreamProxy {
    fn drop(&mut self) {
        println!("finalizing...");
        use sha2::{Digest, Sha256};
        use std::fmt::Write;

        let trace = self.tracer.trace();
        let hash: String = Sha256::digest(trace.data)
            .iter()
            .fold(String::new(), |mut s, b| {
                write!(s, "{b:02x}").unwrap();
                s
            });
        let path = format!("traces/trace-{hash}.json");
        println!("saving to {path}");
        std::fs::create_dir("traces").ok();
        trace.save(path).unwrap();
    }
}
impl StreamProxy {
    fn wrap(inner: *mut *const StreamVTable, owns_stream: OwnsStream) -> Self {
        #[cfg(target_os = "linux")]
        unsafe extern "system" fn dtor(this: *mut *const StreamVTable) {
            println!("StreamProxy: dtor");
            let inner = (*(this as *mut StreamProxy)).inner;
            ((**inner).dtor)(inner);
        }
        unsafe extern "system" fn dtor2(this: *mut *const StreamVTable) {
            println!("StreamProxy: dtor2");
            let owns_stream = { (*(this as *mut StreamProxy)).owns_stream };
            if owns_stream == OwnsStream::True {
                let inner = (*(this as *mut StreamProxy)).inner;
                ((**inner).dtor2)(inner);
            }
            drop(Box::from_raw(this as *mut StreamProxy));
            //std::process::exit(0);
        }
        unsafe extern "system" fn read(
            this: *mut *const StreamVTable,
            target: *mut u8,
            len: u32,
        ) -> u32 {
            let this = &mut *(this as *mut StreamProxy);

            // do read
            let inner = this.inner;
            let count = ((**inner).read)(inner, target, len);

            this.tracer.read(if count == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(target, count as usize)
            });

            count
        }
        unsafe extern "system" fn read_byte(
            _this: *mut *const StreamVTable,
            _target: *mut u8,
        ) -> u32 {
            unimplemented!("StreamProxy: read_byte");
        }
        unsafe extern "system" fn eof(this: *const *const StreamVTable) -> bool {
            println!("StreamProxy: eof");
            let inner = (*(this as *mut StreamProxy)).inner;
            ((**inner).eof)(inner)
        }
        unsafe extern "system" fn remaining(this: *const *const StreamVTable) -> u64 {
            let inner = (*(this as *mut StreamProxy)).inner;
            ((**inner).remaining)(inner)
        }
        Self {
            vtable: &StreamVTable {
                #[cfg(target_os = "linux")]
                dtor,
                dtor2,
                read,
                read_byte,
                eof,
                remaining,
            },
            inner,
            owns_stream,
            tracer: ser_hex_tracer::Tracer::new_options(ser_hex_tracer::TracerOptions {
                skip_frames: 4,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
#[allow(unused)]
enum OwnsStream {
    True,
    False,
}

fn hook_deserialiser(
    this: *const (),
    stream: *const (),
    target_deserialiser: *const (),
    owns_stream: OwnsStream,
) {
    println!("tracing...");
    let proxy_ptr = Box::into_raw(Box::new(StreamProxy::wrap(
        stream as *mut *const StreamVTable,
        owns_stream,
    )));

    unsafe {
        MapDeserialiser.call(
            this,
            proxy_ptr.cast(),
            target_deserialiser,
            OwnsStream::True,
        );
    }
}
