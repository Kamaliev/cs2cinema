use std::io;
use crate::read_varint;

#[derive(Debug)]
pub struct Command<'a> {
    pub kind: u32,
    pub tick: i32,
    pub compressed: bool,
    pub payload: &'a [u8],
}


/// Одна команда демки не может разворачиваться больше чем в 128 МиБ: `decompress_vec`
/// выделяет заявленный в snappy-заголовке размер сразу.
pub const MAX_COMMAND_UNPACKED: usize = 128 << 20;

impl<'a> Command<'a> {
    pub fn decode_payload(&self) -> io::Result<Vec<u8>> {
        if self.compressed {
            let claimed = snap::raw::decompress_len(self.payload)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            if claimed > MAX_COMMAND_UNPACKED {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "команда демки заявляет {} МиБ после распаковки — больше лимита {} МиБ",
                        claimed >> 20,
                        MAX_COMMAND_UNPACKED >> 20
                    ),
                ));
            }
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