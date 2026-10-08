mod bits;
mod command;
pub mod events;
mod parse;
pub use parse::{Parsed, PlayerInfo, parse};

use std::{fs::File, io::{self, Read}, path::Path};
pub use command::{
    Command,
    read_command
};

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
}



/// Читает демку; zstd (`.dem.zst`, FACEIT CS2) и gzip (`.dem.gz`) распознаются по сигнатуре.
pub fn read_demo<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>>{
    let mut file = File::open(path)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    decompress(contents)
}

pub fn decompress(contents: Vec<u8>) -> io::Result<Vec<u8>> {
    if contents.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
        zstd::stream::decode_all(contents.as_slice())
    } else if contents.starts_with(&[0x1F, 0x8B]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(contents.as_slice()).read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(contents)
    }
}



pub fn read_varint(data: &[u8], pos: &mut usize) -> io::Result<u32>{
    let mut value: u32 = 0;

    for shift in (0..35).step_by(7) {
        let byte = *data.get(*pos).ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        *pos += 1;
        if shift == 28 && byte > 0x0f {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid varint so big"));
        }
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }

    Err(io::Error::new(io::ErrorKind::InvalidInput, "varint слишком длинный"))
}