use crate::error::DecodeError;

type DecoderResult<T> = std::result::Result<T, DecodeError>;

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    pub(crate) fn take(&mut self, length: usize) -> DecoderResult<&'a [u8]> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(DecodeError::LengthOverflow)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(DecodeError::Truncated)?;
        self.cursor = end;
        Ok(value)
    }

    pub(crate) fn read_u8(&mut self) -> DecoderResult<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn read_u16(&mut self) -> DecoderResult<u16> {
        let mut bytes = [0; 2];
        bytes.copy_from_slice(self.take(2)?);
        Ok(u16::from_be_bytes(bytes))
    }

    pub(crate) fn read_u32(&mut self) -> DecoderResult<u32> {
        let mut bytes = [0; 4];
        bytes.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(bytes))
    }

    pub(crate) fn read_u64(&mut self) -> DecoderResult<u64> {
        let mut bytes = [0; 8];
        bytes.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(bytes))
    }

    pub(crate) fn position(&self) -> usize {
        self.cursor
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.cursor == self.bytes.len()
    }

    pub(crate) fn finish(&self) -> DecoderResult<()> {
        if self.is_empty() {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes)
        }
    }

    pub(crate) fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}
