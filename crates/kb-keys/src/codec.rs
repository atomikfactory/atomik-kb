//! The binary encoding shared by the device log and the keyring: little-endian
//! integers, fixed-size arrays written as is, `u32` length prefixes for byte strings.
//!
//! Decoding is strict so that the encoding is canonical: a decoder accepts a byte
//! string only if encoding the decoded value gives back exactly those bytes (booleans
//! are `0` or `1`, no trailing bytes). Signatures and hashes are computed over the
//! encoding, so two encodings of one value would be two different signed messages.

/// Why bytes do not decode. Static text only: never input bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DecodeError(pub(crate) &'static str);

/// Reads values front to back from a byte slice.
pub(crate) struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Starts at the first byte.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    /// The next `len` bytes.
    pub(crate) fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        match self.rest.split_at_checked(len) {
            Some((head, tail)) => {
                self.rest = tail;
                Ok(head)
            }
            None => Err(DecodeError("truncated")),
        }
    }

    /// The next `N` bytes as an array.
    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    /// One byte.
    pub(crate) fn u8(&mut self) -> Result<u8, DecodeError> {
        let [byte] = self.array::<1>()?;
        Ok(byte)
    }

    /// A little-endian `u16`.
    pub(crate) fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// A little-endian `u32`.
    pub(crate) fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    /// A little-endian `u64`.
    pub(crate) fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    /// A little-endian `i64`.
    pub(crate) fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    /// A boolean byte: `0` or `1`, nothing else.
    pub(crate) fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError("invalid boolean")),
        }
    }

    /// A byte string with a `u32` length prefix.
    pub(crate) fn bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = usize::try_from(self.u32()?).map_err(|_| DecodeError("length out of range"))?;
        self.take(len)
    }

    /// Succeeds when every byte was consumed.
    pub(crate) fn finish(self) -> Result<(), DecodeError> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(DecodeError("trailing bytes"))
        }
    }
}

/// Appends a byte string with its `u32` length prefix.
///
/// Byte strings in these formats are tens of bytes (encrypted device names). One
/// longer than `u32::MAX` bytes is cut at that length so the output stays decodable;
/// signatures and hashes cover exactly what is written.
pub(crate) fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    let (len, body) = match u32::try_from(bytes.len()) {
        Ok(len) => (len, bytes),
        Err(_) => {
            let max = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
            (u32::MAX, bytes.get(..max).unwrap_or(bytes))
        }
    };
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(body);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian_values_in_order() {
        let mut bytes = vec![7u8];
        bytes.extend_from_slice(&0x0102u16.to_le_bytes());
        bytes.extend_from_slice(&0x0304_0506u32.to_le_bytes());
        bytes.extend_from_slice(&0x0708_090a_0b0c_0d0eu64.to_le_bytes());
        bytes.extend_from_slice(&(-2i64).to_le_bytes());
        bytes.push(1);
        put_bytes(&mut bytes, b"abc");
        bytes.extend_from_slice(&[9, 9]);

        let mut r = Reader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 0x0102);
        assert_eq!(r.u32().unwrap(), 0x0304_0506);
        assert_eq!(r.u64().unwrap(), 0x0708_090a_0b0c_0d0e);
        assert_eq!(r.i64().unwrap(), -2);
        assert!(r.bool().unwrap());
        assert_eq!(r.bytes().unwrap(), b"abc");
        assert_eq!(r.array::<2>().unwrap(), [9, 9]);
        r.finish().unwrap();
    }

    #[test]
    fn length_prefix_is_u32_le() {
        let mut out = Vec::new();
        put_bytes(&mut out, b"xy");
        assert_eq!(out, [2, 0, 0, 0, b'x', b'y']);
        let mut empty = Vec::new();
        put_bytes(&mut empty, b"");
        assert_eq!(empty, [0, 0, 0, 0]);
    }

    #[test]
    fn strictness() {
        assert_eq!(Reader::new(&[]).u8(), Err(DecodeError("truncated")));
        assert_eq!(Reader::new(&[1, 2, 3]).u32(), Err(DecodeError("truncated")));
        assert_eq!(
            Reader::new(&[2]).bool(),
            Err(DecodeError("invalid boolean"))
        );
        // A length prefix pointing past the end.
        assert_eq!(
            Reader::new(&[5, 0, 0, 0, 1, 2]).bytes(),
            Err(DecodeError("truncated"))
        );
        // A huge length prefix does not allocate or panic.
        assert_eq!(
            Reader::new(&[0xff, 0xff, 0xff, 0xff]).bytes(),
            Err(DecodeError("truncated"))
        );
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert_eq!(r.finish(), Err(DecodeError("trailing bytes")));
    }
}
