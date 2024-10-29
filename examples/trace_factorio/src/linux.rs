use libc::{c_char, c_int, c_void, dl_iterate_phdr, Elf64_Phdr, PT_LOAD};
use object::read::elf::ProgramHeader;
use object::{Object, ObjectSymbol};
use std::{
    path::PathBuf,
    ptr::{null, null_mut},
};

type MainFunc = extern "C" fn(c_int, *const *const c_char, *const *const c_char) -> c_int;

type LibcStartMainFunc = extern "C" fn(
    MainFunc,
    c_int,
    *const *const c_char,
    extern "C" fn() -> c_int,
    extern "C" fn(),
    extern "C" fn(),
    *mut c_void,
) -> c_int;

fn entrypoint(exe: PathBuf) {
    let base = read_image().0 as u64;
    println!("base 0x{:x}", base);

    let file = std::fs::File::open(&exe).unwrap();
    let mmap = unsafe { memmap2::Mmap::map(&file).unwrap() };
    let object = object::File::parse(&*mmap).unwrap();
    let img_base = match &object {
        object::File::Elf64(elf) => elf
            .elf_program_headers()
            .iter()
            .filter(|h| h.p_memsz(elf.endian()) != 0)
            .map(|h| h.p_vaddr(elf.endian()))
            .min(),
        _ => unimplemented!(),
    }
    .unwrap();
    println!("img_base={img_base:x}");

    let virtual_base = base - img_base;

    fn sym(obj: &object::File, virtual_base: u64, sym: &str) -> u64 {
        virtual_base + obj.symbol_by_name(sym).expect(sym).address()
    }
    let map_deserialiser = sym(
        &object,
        virtual_base,
        "_ZN15MapDeserialiserC1EP10ReadStreamP18TargetDeserialiserN12Deserialiser10OwnsStreamE",
    );

    crate::main(crate::Ctx { map_deserialiser });
}

#[no_mangle]
pub extern "C" fn __libc_start_main(
    main: MainFunc,
    argc: c_int,
    argv: *const *const c_char,
    init: extern "C" fn() -> c_int,
    fini: extern "C" fn(),
    rtld_fini: extern "C" fn(),
    stack_end: *mut c_void,
) -> c_int {
    const LOAD_FLAG: &str = "AIO_LOADED";
    if std::env::var(LOAD_FLAG).is_err() {
        std::env::set_var(LOAD_FLAG, "1");

        entrypoint(std::env::current_exe().unwrap());
    }

    let original_libc_start_main: LibcStartMainFunc = unsafe {
        std::mem::transmute(libc::dlsym(
            libc::RTLD_NEXT,
            c"__libc_start_main".as_ptr() as *const _,
        ))
    };

    original_libc_start_main(main, argc, argv, init, fini, rtld_fini, stack_end)
}

unsafe extern "C" fn dl_iterate_phdr_callback(
    info: *mut libc::dl_phdr_info,
    _size: usize,
    data: *mut std::ffi::c_void,
) -> i32 {
    let name = unsafe { std::ffi::CStr::from_ptr((*info).dlpi_name) };
    let name = name.to_str().unwrap();
    let image = data as *mut libc::dl_phdr_info;
    if name.is_empty() {
        *image = *info;
    }
    0
}

pub fn read_image() -> (*const u8, usize) {
    unsafe {
        let mut info = libc::dl_phdr_info {
            dlpi_addr: 0,
            dlpi_name: null(),
            dlpi_phdr: null(),
            dlpi_phnum: 0,
            dlpi_adds: 0,
            dlpi_subs: 0,
            dlpi_tls_modid: 0,
            dlpi_tls_data: null_mut(),
        };
        dl_iterate_phdr(
            Some(dl_iterate_phdr_callback),
            (&mut info) as *mut libc::dl_phdr_info as *mut std::ffi::c_void,
        );

        let base_addr = (info).dlpi_addr as usize;

        let phdr_slice = std::slice::from_raw_parts_mut(
            (info).dlpi_phdr as *mut Elf64_Phdr,
            (info).dlpi_phnum as usize,
        );
        let map_end = phdr_slice
            .iter()
            .filter(|p| p.p_type == PT_LOAD)
            .map(|p| p.p_vaddr + p.p_memsz)
            .max()
            .unwrap_or_default() as usize;
        let map_start = phdr_slice
            .iter()
            .filter(|p| p.p_type == PT_LOAD)
            .map(|p| p.p_vaddr)
            .min()
            .unwrap_or_default() as usize;

        ((base_addr + map_start) as *const u8, map_end - map_start)
    }
}
