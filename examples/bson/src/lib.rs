use std::{collections::HashMap, io::Read};

use byteorder::{ReadBytesExt, LE};
use tracing::instrument;

type Result<R> = std::result::Result<R, Box<dyn std::error::Error>>;

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_bson() -> Result<()> {
        let mut input = std::io::Cursor::new(include_bytes!("../example.bson"));
        let res = ser_hex::read_from_stream("trace.json", &mut input, read)?;
        println!("{res:#?}");
        Ok(())
    }
}

#[instrument(skip_all)]
pub fn read<R: Read>(reader: &mut R) -> Result<Element> {
    let _size = reader.read_u32::<LE>()?;
    let mut map = HashMap::default();
    loop {
        let type_ = reader.read_i8()?;
        if type_ == 0 {
            break;
        }
        let name = read_cstr(reader)?;
        let element = match type_ {
            1 => read_double(reader)?,
            2 => read_string(reader)?,
            3 => read(reader)?,
            4 => read(reader)?,
            16 => read_i32(reader)?,
            _ => todo!("type {type_}"),
        };
        map.insert(name, element);
    }
    Ok(Element::Map(map))
}

#[derive(Debug)]
pub enum Element {
    Double(f64),
    I32(i32),
    String(String),
    Map(HashMap<String, Element>),
    Array(HashMap<String, Element>),
}

#[instrument(skip_all)]
fn read_double<R: Read>(reader: &mut R) -> Result<Element> {
    Ok(Element::Double(reader.read_f64::<LE>()?))
}

#[instrument(skip_all)]
fn read_i32<R: Read>(reader: &mut R) -> Result<Element> {
    Ok(Element::I32(reader.read_i32::<LE>()?))
}

#[instrument(skip_all)]
fn read_cstr<R: Read>(reader: &mut R) -> Result<String> {
    let mut buf = vec![];
    loop {
        let next = reader.read_u8()?;
        if next == 0 {
            break;
        }
        buf.push(next);
    }
    Ok(String::from_utf8(buf)?)
}

#[instrument(skip_all)]
fn read_string<R: Read>(reader: &mut R) -> Result<Element> {
    let length = reader.read_u32::<LE>()?;
    let mut buf = vec![0; length as usize];
    reader.read_exact(&mut buf)?;
    Ok(Element::String(String::from_utf8(
        buf.into_iter().take_while(|b| *b != 0).collect::<Vec<_>>(),
    )?))
}
