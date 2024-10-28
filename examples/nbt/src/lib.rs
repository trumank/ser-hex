use std::{collections::HashMap, io::Read};

use byteorder::{ReadBytesExt, BE};
use tracing::instrument;

type Result<R> = std::result::Result<R, Box<dyn std::error::Error>>;

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_nbt_tracing() -> Result<()> {
        let mut input = std::io::Cursor::new(include_bytes!("../level.nbt"));
        let res = ser_hex::read_from_stream("trace_tracing.json", &mut input, read)?;
        println!("{res:#?}");
        Ok(())
    }

    #[test]
    fn test_nbt_tracer() -> Result<()> {
        let mut input = std::io::Cursor::new(include_bytes!("../level.nbt"));
        let mut tracer = ser_hex_tracer::TracerReader::new_options(
            &mut input,
            ser_hex_tracer::TracerOptions { skip_frames: 3 }, // depends on amount of inlining for build config
        );
        let res = read(&mut tracer);
        tracer.trace().save("trace_tracer.json").unwrap();
        println!("{res:#?}");
        Ok(())
    }
}

type TagByte = i8;
type TagShort = i16;
type TagInt = i32;
type TagLong = i64;
type TagFloat = f32;
type TagDouble = f64;
type TagByteArray = Vec<u8>;
type TagString = String;
type TagList = Vec<Tag>;
type TagCompound = HashMap<String, Tag>;
type TagIntArray = Vec<i32>;

#[derive(Debug)]
pub struct NamedTag {
    pub name: String,
    pub value: Tag,
}

#[derive(Debug)]
pub enum Tag {
    End,
    Byte(TagByte),
    Short(TagShort),
    Int(TagInt),
    Long(TagLong),
    Float(TagFloat),
    Double(TagDouble),
    ByteArray(TagByteArray),
    String(TagString),
    List(TagList),
    Compound(TagCompound),
    IntArray(TagIntArray),
}

#[instrument(skip_all)]
fn read_tag<R: Read>(reader: &mut R, tag: u8) -> Result<Tag> {
    Ok(match tag {
        0 => Tag::End,
        1 => Tag::Byte(read_tag_byte(reader)?),
        2 => Tag::Short(read_tag_short(reader)?),
        3 => Tag::Int(read_tag_int(reader)?),
        4 => Tag::Long(read_tag_long(reader)?),
        5 => Tag::Float(read_tag_float(reader)?),
        6 => Tag::Double(read_tag_double(reader)?),
        7 => Tag::ByteArray(read_tag_byte_array(reader)?),
        8 => Tag::String(read_tag_string(reader)?),
        9 => Tag::List(read_tag_list(reader)?),
        10 => Tag::Compound(read_tag_compound(reader)?),
        11 => Tag::IntArray(read_tag_int_array(reader)?),
        _ => unimplemented!("tag {tag}"),
    })
}

#[instrument(skip_all)]
pub fn read<R: Read>(reader: &mut R) -> Result<NamedTag> {
    let tag = reader.read_u8()?;
    let name = read_tag_string(reader)?;
    Ok(NamedTag {
        name,
        value: read_tag(reader, tag)?,
    })
}

#[instrument(skip_all)]
fn read_maybe<R: Read>(reader: &mut R) -> Result<Option<NamedTag>> {
    let tag = reader.read_u8()?;
    Ok(if tag == 0 {
        None
    } else {
        Some(NamedTag {
            name: read_tag_string(reader)?,
            value: read_tag(reader, tag)?,
        })
    })
}

#[instrument(skip_all)]
fn read_string<R: Read>(reader: &mut R) -> Result<String> {
    let length = reader.read_u16::<BE>()?;
    let mut buf = vec![0; length as usize];
    reader.read_exact(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}

#[instrument(skip_all)]
fn read_tag_byte<R: Read>(reader: &mut R) -> Result<TagByte> {
    Ok(reader.read_i8()?)
}
#[instrument(skip_all)]
fn read_tag_short<R: Read>(reader: &mut R) -> Result<TagShort> {
    Ok(reader.read_i16::<BE>()?)
}
#[instrument(skip_all)]
fn read_tag_int<R: Read>(reader: &mut R) -> Result<TagInt> {
    Ok(reader.read_i32::<BE>()?)
}
#[instrument(skip_all)]
fn read_tag_long<R: Read>(reader: &mut R) -> Result<TagLong> {
    Ok(reader.read_i64::<BE>()?)
}
#[instrument(skip_all)]
fn read_tag_float<R: Read>(reader: &mut R) -> Result<TagFloat> {
    Ok(reader.read_f32::<BE>()?)
}
#[instrument(skip_all)]
fn read_tag_double<R: Read>(reader: &mut R) -> Result<TagDouble> {
    Ok(reader.read_f64::<BE>()?)
}
#[instrument(skip_all)]
fn read_tag_byte_array<R: Read>(reader: &mut R) -> Result<TagByteArray> {
    let length = reader.read_u32::<BE>()?;
    let mut buf = vec![0; length as usize];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}
#[instrument(skip_all)]
fn read_tag_string<R: Read>(reader: &mut R) -> Result<TagString> {
    read_string(reader)
}
#[instrument(skip_all)]
fn read_tag_list<R: Read>(reader: &mut R) -> Result<TagList> {
    let tag = reader.read_u8()?;
    let mut entries = vec![];
    for _ in 0..reader.read_u32::<BE>()? {
        entries.push(read_tag(reader, tag)?);
    }
    Ok(entries)
}
#[instrument(skip_all)]
fn read_tag_compound<R: Read>(reader: &mut R) -> Result<TagCompound> {
    let mut entries = HashMap::default();
    while let Some(next) = read_maybe(reader)? {
        entries.insert(next.name, next.value);
    }
    Ok(entries)
}
#[instrument(skip_all)]
fn read_tag_int_array<R: Read>(reader: &mut R) -> Result<TagIntArray> {
    let mut values = vec![];
    for _ in 0..reader.read_u32::<BE>()? {
        values.push(reader.read_i32::<BE>()?);
    }
    Ok(values)
}
