use bytes::Buf;

use crate::error::Error;

pub trait TryBuf: Buf {
    fn try_get_bytes(&mut self) -> Result<Vec<u8>, Error>;
    fn try_get_string(&mut self) -> Result<String, Error>;
}

impl<T: Buf> TryBuf for T {
    fn try_get_bytes(&mut self) -> Result<Vec<u8>, Error> {
        let len = self.try_get_u32()? as usize;
        if self.remaining() < len {
            return Err(Error::BadMessage("no remaining for vec".to_owned()));
        }

        Ok(self.copy_to_bytes(len).to_vec())
    }

    fn try_get_string(&mut self) -> Result<String, Error> {
        let bytes = self.try_get_bytes()?;
        Ok(String::from_utf8(bytes).unwrap_or_else(|error| escape_bytes(error.as_bytes())))
    }
}

/// RustyFinder: the first of 256 private-use characters that stand for raw
/// bytes which are not valid UTF-8 (see VENDORED.md).
pub const RAW_BYTE_BASE: u32 = 0x10FF00;

/// RustyFinder: decode `bytes` as UTF-8 without losing anything: each byte
/// that is not part of valid UTF-8 becomes `RAW_BYTE_BASE + byte`.
pub fn escape_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.push_str(chunk.valid());
        for byte in chunk.invalid() {
            out.push(char::from_u32(RAW_BYTE_BASE + u32::from(*byte)).expect("in the private use area"));
        }
    }
    out
}

/// RustyFinder: the bytes `escape_bytes` was given back.
pub fn unescape_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for c in text.chars() {
        let code = c as u32;
        if (RAW_BYTE_BASE..=RAW_BYTE_BASE + 0xFF).contains(&code) {
            out.push((code - RAW_BYTE_BASE) as u8);
        } else {
            let mut utf8 = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
        }
    }
    out
}
