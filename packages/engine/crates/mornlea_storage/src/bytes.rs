//! Byte primitives shared by the save codecs.
//!
//! Each Go storage domain owns a private copy of these helpers; this module is
//! the single Rust copy. Fixed-width integers are little-endian, matching the
//! on-disk layout of every `save.*` family.

/// Sequential little-endian reader over an immutable record.
///
/// Reads fail closed with [`crate::error::StorageError::Corrupt`] instead of
/// returning partial values, so a truncated record never yields a half-parsed
/// entity.
pub(crate) struct ByteReader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> ByteReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.offset
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| "length overflows the record".to_owned())?;
        if end > self.data.len() {
            return Err(format!(
                "need {length} bytes at offset {}, only {} remain",
                self.offset,
                self.data.len() - self.offset
            ));
        }
        let slice = &self.data[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    pub(crate) fn take_bytes(&mut self, length: usize) -> Result<&'a [u8], String> {
        self.take(length)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        let mut value = [0u8; 8];
        value.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(value))
    }

    pub(crate) fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let slice = self.take(N)?;
        let mut value = [0u8; N];
        value.copy_from_slice(slice);
        Ok(value)
    }
}

/// Append-only little-endian writer used to build canonical save bytes.
pub(crate) struct ByteWriter {
    data: Vec<u8>,
}

impl ByteWriter {
    pub(crate) fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.data.push(value);
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.data.extend_from_slice(&value.to_le_bytes());
    }

    pub(crate) fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        self.data.extend_from_slice(value);
    }

    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn into_vec(self) -> Vec<u8> {
        self.data
    }
}

/// Infallible little-endian writer into a caller-reserved prefix of `dst`.
///
/// Every method assumes the caller already proved `pos + write_len <= dst.len()`.
pub(crate) struct SliceWriter<'a> {
    dst: &'a mut [u8],
    pos: usize,
}

impl<'a> SliceWriter<'a> {
    pub(crate) fn new(dst: &'a mut [u8]) -> Self {
        Self { dst, pos: 0 }
    }

    pub(crate) fn pos(&self) -> usize {
        self.pos
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.dst[self.pos] = value;
        self.pos += 1;
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.dst[self.pos..self.pos + 2].copy_from_slice(&value.to_le_bytes());
        self.pos += 2;
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.dst[self.pos..self.pos + 4].copy_from_slice(&value.to_le_bytes());
        self.pos += 4;
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.dst[self.pos..self.pos + 8].copy_from_slice(&value.to_le_bytes());
        self.pos += 8;
    }

    pub(crate) fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) {
        let end = self.pos + value.len();
        self.dst[self.pos..end].copy_from_slice(value);
        self.pos = end;
    }

    pub(crate) fn zeroes(&mut self, length: usize) {
        let end = self.pos + length;
        self.dst[self.pos..end].fill(0);
        self.pos = end;
    }

    /// Rewrites one little-endian `u32` inside the prefix already written.
    pub(crate) fn patch_u32(&mut self, at: usize, value: u32) {
        self.dst[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
}

/// Reports whether every byte in `data` is zero.
pub(crate) fn is_zero(data: &[u8]) -> bool {
    data.iter().all(|&byte| byte == 0)
}
