mod command;

use std::{fs::File, io::{self, Read}, path::Path};
pub use command::{
    Command,
    read_command
};

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/_.rs"));
}



pub fn read_demo<P: AsRef<Path>>(path: P) -> io::Result<Vec<u8>>{
    let mut file = File::open(path)?;
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    Ok(contents)
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