use std::io;
use crate::read_varint;

#[derive(Debug)]
pub struct Command<'a> {
    pub kind: u32,
    pub tick: i32,
    pub compressed: bool,
    pub payload: &'a [u8],
}


impl<'a> Command<'a> {
    pub fn decode_payload(&self) -> io::Result<Vec<u8>> {
        if self.compressed {
            snap::raw::Decoder::new().decompress_vec(self.payload).map_err(
                |e| io::Error::new(io::ErrorKind::InvalidData, e),
            )
        } else {
            Ok(self.payload.to_vec())
        }
    }
    
}



pub fn read_command<'a>(data: &'a [u8], pos: &mut usize) -> io::Result<Command<'a>> {
    let raw_kind = read_varint(data, pos)?;
    let tick = read_varint(data, pos)? as i32;
    let size = read_varint(data, pos)? as usize;
    let end = (*pos).checked_add(size).ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "размер команды большой")
    })?;
    let payload = data.get(*pos..end).ok_or_else(|| {
        io::Error::new(io::ErrorKind::UnexpectedEof, "оборванное содержимое команды")
    })?;
    *pos = end;

    Ok(Command{
        kind: raw_kind & !64,
        tick,
        compressed: raw_kind & 64 != 0,
        payload,
    })
}