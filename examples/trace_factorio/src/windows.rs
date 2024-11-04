use windows::Win32::System::LibraryLoader::GetModuleHandleA;

proxy_dll::proxy_dll!([winmm], init);

fn init() {
    let exe = std::env::current_exe().unwrap();

    let file = std::fs::File::open(exe.with_extension("pdb")).unwrap();
    let rva = find_sym(file).unwrap().unwrap();

    let module = unsafe {
        GetModuleHandleA(None)
            .expect("could not find main module")
            .0 as u64
    };

    crate::main(crate::Ctx {
        map_deserialiser: module + rva as u64,
    });
}

fn find_sym(file: std::fs::File) -> pdb::Result<Option<u32>> {
    use pdb::FallibleIterator;

    let mut pdb = pdb::PDB::open(file).unwrap();

    let symbol_table = pdb.global_symbols()?;
    let address_map = pdb.address_map()?;

    let needle: pdb::RawString = "??0MapDeserialiser@@QEAA@PEAVReadStream@@PEAVTargetDeserialiser@@W4OwnsStream@Deserialiser@@@Z".into();

    let mut symbols = symbol_table.iter();
    while let Some(symbol) = symbols.next()? {
        match symbol.parse() {
            Ok(pdb::SymbolData::Public(data)) if data.function && data.name == needle => {
                return Ok(Some(data.offset.to_rva(&address_map).unwrap().0));
            }
            _ => {}
        }
    }

    Ok(None)
}
