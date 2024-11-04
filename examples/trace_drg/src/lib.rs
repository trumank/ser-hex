#![allow(clippy::missing_transmute_annotations)]

use std::cell::RefCell;
use std::cell::UnsafeCell;

use patternsleuth::resolvers::futures::future::join_all;
use patternsleuth::resolvers::impl_resolver_singleton;
use patternsleuth::resolvers::impl_try_collector;
use patternsleuth::resolvers::unreal::save_game::UGameplayStaticsLoadGameFromSlot;
use patternsleuth::resolvers::Context as _;
use patternsleuth::scanner::Pattern;
use ser_hex_tracer::Tracer;

impl_try_collector! {
    #[derive(Debug, PartialEq)]
    pub struct Addresses {
        load_game_from_slot: UGameplayStaticsLoadGameFromSlot,
        serialize: Serialize,
    }
}

#[derive(Debug, PartialEq)]
pub struct Serialize(pub usize);
impl_resolver_singleton!(collect, Serialize);
impl_resolver_singleton!(PEImage, Serialize, |ctx| async {
    let patterns =
        ["4D 85 C0 74 ?? 48 89 5C 24 ?? 48 89 6C 24 ?? 57 48 83 EC 20 F6 41 ?? 01 49 8B F8 48 8B EA 48 8B D9 75 ?? 48 8B 01 48 89 74 24 ?? 48 8B B1 ?? ?? ?? ?? FF 50 ?? 48 8D 0C 3E 48 3B C8 7F"];

    let res = join_all(patterns.iter().map(|p| ctx.scan(Pattern::new(p).unwrap()))).await;

    Ok(Self(
        res.into_iter()
            .flatten()
            .next()
            .context("expected at least one")?,
    ))
});

proxy_dll::proxy_dll!([d3d11], main);

retour::static_detour! {
    static LoadGameFromSlot: unsafe extern "system" fn(*const FString, u32) -> *const ();
    static DetourSerialize: unsafe extern "system" fn(*mut (), *mut (), u64);
}

thread_local! {
    static READER_STACK: RefCell<Vec<Option<(*mut (), Tracer)>>> = const { RefCell::new(vec![]) };
}

#[derive(Debug)]
#[repr(C)]
struct FMemoryReader {
    pad: UnsafeCell<[u8; 0x98]>,
    offset: u64,
}

#[derive(Debug)]
#[repr(C)]
struct FString {
    data: *const u16,
    num: u32,
    max: u32,
}
impl std::fmt::Display for FString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let slice = unsafe { std::slice::from_raw_parts(self.data, self.num as usize) };
        let end = slice.iter().position(|c| *c == 0).unwrap_or(slice.len());
        write!(f, "{}", String::from_utf16_lossy(&slice[0..end]))
    }
}

#[allow(unused)]
fn main() {
    println!("Intercepted main!");

    let image = patternsleuth::process::internal::read_image().unwrap();
    println!("scanning");
    let addresses = image.resolve(Addresses::resolver()).unwrap();
    println!("found {addresses:#x?}");

    let out_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .join("traces");
    std::fs::create_dir(&out_dir).ok();
    println!("trace dir = {out_dir:?}");

    unsafe {
        LoadGameFromSlot
            .initialize(
                std::mem::transmute(addresses.load_game_from_slot.0),
                move |path, slot| {
                    let str_path = (*path).to_string();
                    println!("tracing {str_path:?}...");

                    READER_STACK.with_borrow_mut(|s| s.push(None));
                    let r = LoadGameFromSlot.call(path, slot);
                    if let Some((_, tracer)) = READER_STACK.with_borrow_mut(|s| s.pop()).unwrap() {
                        println!("finalizing...");

                        let trace = tracer.trace();
                        let out_path = out_dir.join(
                            std::path::Path::new(&str_path)
                                .with_extension("json")
                                .file_name()
                                .unwrap(),
                        );
                        println!("saving to {out_path:?}");
                        trace.save(out_path).unwrap();
                    }
                    println!("finished trace");
                    r
                },
            )
            .unwrap();
        LoadGameFromSlot.enable().unwrap();
        DetourSerialize
            .initialize(
                std::mem::transmute(addresses.serialize.0),
                |reader, out, len| {
                    //println!("Serialize {reader:?} {out:?} {len}");
                    DetourSerialize.call(reader, out, len);
                    READER_STACK.with_borrow_mut(|s| {
                        if let Some(last) = s.last_mut() {
                            if last.is_none() {
                                *last = Some((reader, Tracer::new()));
                            }
                            let (r, tracer) = last.as_mut().unwrap();
                            if *r == reader {
                                tracer.read(std::slice::from_raw_parts(out.cast(), len as usize));
                                let reader = &*(reader as *mut FMemoryReader);

                                // verify offset hasn't been changed on us from somewhere else
                                assert_eq!(reader.offset, tracer.data().len() as u64);
                            }
                        }
                    });
                },
            )
            .unwrap();
        DetourSerialize.enable().unwrap();
    }
}
