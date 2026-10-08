/// Побитовый читатель (LSB-first), как в сетевых сообщениях Source 2.
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn bits_left(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    pub fn read_bits(&mut self, n: usize) -> Option<u32> {
        debug_assert!(n <= 32);
        if n > self.bits_left() {
            return None;
        }
        let mut value = 0u64;
        let mut got = 0;
        while got < n {
            let byte = self.data[self.pos / 8] as u64;
            let offset = self.pos % 8;
            let take = (8 - offset).min(n - got);
            let chunk = (byte >> offset) & ((1 << take) - 1);
            value |= chunk << got;
            got += take;
            self.pos += take;
        }
        Some(value as u32)
    }

    /// `ubitvar`: 6 бит, старшие два бита выбирают сколько добавить.
    pub fn read_ubit_var(&mut self) -> Option<u32> {
        let ret = self.read_bits(6)?;
        let extra = match ret & 0x30 {
            0x10 => 4,
            0x20 => 8,
            0x30 => 28,
            _ => return Some(ret),
        };
        Some((ret & 15) | (self.read_bits(extra)? << 4))
    }

    pub fn read_var_u32(&mut self) -> Option<u32> {
        let mut value = 0u32;
        for shift in (0..35).step_by(7) {
            let byte = self.read_bits(8)?;
            value |= (byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }

    pub fn read_bytes(&mut self, n: usize) -> Option<Vec<u8>> {
        if n * 8 > self.bits_left() {
            return None;
        }
        if self.pos % 8 == 0 {
            let start = self.pos / 8;
            self.pos += n * 8;
            return Some(self.data[start..start + n].to_vec());
        }
        (0..n).map(|_| self.read_bits(8).map(|b| b as u8)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_bits_lsb_first() {
        let mut r = BitReader::new(&[0b1010_1101, 0b0000_0011]);
        assert_eq!(r.read_bits(3), Some(0b101));
        assert_eq!(r.read_bits(7), Some(117));
        assert_eq!(r.bits_left(), 6);
    }

    #[test]
    fn ubit_var_extended() {
        // 6 бит: 0b01_0101 -> префикс 0x10 (+4 бита), младшие 4 = 5; следующие 4 бита = 0b1010
        let mut w = Vec::new();
        let bits: u32 = 0b01_0101 | (0b1010 << 6);
        w.extend_from_slice(&bits.to_le_bytes()[..2]);
        let mut r = BitReader::new(&w);
        assert_eq!(r.read_ubit_var(), Some(5 | (0b1010 << 4)));
    }

    #[test]
    fn unaligned_bytes() {
        let mut r = BitReader::new(&[0xAB, 0xCD, 0x0E]);
        r.read_bits(4).unwrap();
        assert_eq!(r.read_bytes(2), Some(vec![0xDA, 0xEC]));
    }
}
