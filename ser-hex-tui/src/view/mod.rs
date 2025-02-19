use std::ops::Range;

use ratatui::style::Color;

pub mod hex;
pub mod minimap;
pub mod tree;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ByteStyle {
    byte_type: ByteType,
    symbol: char,
    highlight: bool,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ByteType {
    Null,
    Other,
    Ascii,
}
impl ByteType {
    fn color(self) -> Color {
        match self {
            ByteType::Null => Color::DarkGray,
            ByteType::Other => Color::White,
            ByteType::Ascii => Color::Red,
        }
    }
}

fn byte_style(range: Option<Range<usize>>, index: usize, byte: u8) -> ByteStyle {
    let (byte_type, symbol) = if byte.is_ascii_graphic() {
        (ByteType::Ascii, byte as char)
    } else if byte == 0 {
        (ByteType::Null, '.')
    } else {
        (ByteType::Other, '.')
    };
    ByteStyle {
        byte_type,
        symbol,
        highlight: range.as_ref().is_some_and(|r| r.contains(&index)),
    }
}
