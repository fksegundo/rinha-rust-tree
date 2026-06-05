use crate::{PACKED_DIMS, SCALE};

pub struct IndexWriter {
    buf: Vec<u8>,
}

impl IndexWriter {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    pub fn write_header(
        &mut self,
        reference_count: i32,
        amount_cut_count: i16,
        dow_cut_count: i16,
        cuts: &[i16],
    ) -> Result<(), String> {
        self.buf.extend_from_slice(b"RNSPCST3");
        self.write_i32(SCALE as i32)?;
        self.write_i32(PACKED_DIMS as i32)?;
        self.write_i32(reference_count)?;
        self.write_i32(0)?;
        self.write_i32(0)?;
        self.write_i32(0)?;
        self.write_i16(amount_cut_count)?;
        self.write_i16(dow_cut_count)?;
        for &c in cuts {
            self.write_i16(c)?;
        }
        Ok(())
    }

    pub fn write_partition_count(&mut self, count: i32) -> Result<(), String> {
        let offset = 8 + 4 + 4 + 4;
        self.buf[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    pub fn write_node_count(&mut self, count: i32) -> Result<(), String> {
        let offset = 8 + 4 + 4 + 4 + 4;
        self.buf[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    pub fn write_block_count(&mut self, count: i32) -> Result<(), String> {
        let offset = 8 + 4 + 4 + 4 + 4 + 4;
        self.buf[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
        Ok(())
    }

    pub fn write_partition_entry(
        &mut self,
        key: u32,
        root: usize,
        len: usize,
        min: [i16; PACKED_DIMS],
        max: [i16; PACKED_DIMS],
    ) -> Result<(), String> {
        self.write_u32(key)?;
        self.write_i32(root as i32)?;
        self.write_i32(0)?;
        self.write_i32(len as i32)?;
        for &v in &min {
            self.write_i16(v)?;
        }
        for &v in &max {
            self.write_i16(v)?;
        }
        Ok(())
    }

    pub fn write_node_entry(
        &mut self,
        left: i32,
        right: i32,
        start: usize,
        len: usize,
        min: [i16; PACKED_DIMS],
        max: [i16; PACKED_DIMS],
    ) -> Result<(), String> {
        self.write_i32(left)?;
        self.write_i32(right)?;
        self.write_i32(start as i32)?;
        self.write_i32(len as i32)?;
        for &v in &min {
            self.write_i16(v)?;
        }
        for &v in &max {
            self.write_i16(v)?;
        }
        Ok(())
    }

    pub fn write_i16(&mut self, v: i16) -> Result<(), String> {
        self.buf.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    pub fn write_u8(&mut self, v: u8) -> Result<(), String> {
        self.buf.push(v);
        Ok(())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    fn write_u32(&mut self, v: u32) -> Result<(), String> {
        self.buf.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }

    fn write_i32(&mut self, v: i32) -> Result<(), String> {
        self.buf.extend_from_slice(&v.to_le_bytes());
        Ok(())
    }
}
