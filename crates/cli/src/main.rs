use std::{env, io};
use prost::Message;

fn main() -> io::Result<()> {
    let path = env::args()
        .nth(1)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "usage: cs2-cli <demo.dem>",
            )
        })?;

    let data = demo::read_demo(&path)?;

    println!("demo size: {} bytes", data.len());

    if data.len() < 16 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "оборванный заголовок демки",
        ));
    }

    if &data[..8] != b"PBDEMS2\0" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "ожидалась демка Source 2",
        ));
    }

    let mut pos = 16;

    while pos < data.len() {
        let command = demo::read_command(&data, &mut pos)?;
        let payload = command.decode_payload()?;
        // println!(
        //     "type={} tick={} stored={} decoded={}",
        //     command.kind,
        //     command.tick,
        //     command.payload.len(),
        //     payload.len()
        // );


        match command.kind {
            0 => break,
            1 => {
                let header = demo::proto::CDemoFileHeader::decode(payload.as_slice()).map_err(|e| {io::Error::new(io::ErrorKind::InvalidData, e)})?;

                println!("Header: {header:#?}");

                println!("Map: {}", header.map_name.as_deref().unwrap_or("no map name"));
            },

            7 | 8 => {
                let packet =
                    demo::proto::CDemoPacket::decode(payload.as_slice())
                        .map_err(|err| {
                            io::Error::new(io::ErrorKind::InvalidData, err)
                        })?;

                println!(
                    "Пакет: tick={}, сетевых байтов={}",
                    command.tick,
                    packet.data.as_deref().unwrap_or_default().len(),
                );
            }

            _ => {}
        }
    }

    Ok(())
}



