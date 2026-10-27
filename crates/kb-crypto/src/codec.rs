//! A bounds-checked reader for the crate's fixed binary layouts (envelopes,
//! passphrase blobs). Every read either returns exactly the requested bytes or a
//! [`CryptoError::Encoding`] naming the field; nothing here can panic.

use crate::CryptoError;

/// Reads fields front to back from a byte slice.
pub(crate) struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    /// A reader positioned at the start of `bytes`.
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { rest: bytes }
    }

    /// The next `len` bytes.
    pub(crate) fn take(&mut self, len: usize, what: &str) -> Result<&'a [u8], CryptoError> {
        let (head, tail) = self
            .rest
            .split_at_checked(len)
            .ok_or_else(|| truncated(what))?;
        self.rest = tail;
        Ok(head)
    }

    /// The next `N` bytes as an array.
    pub(crate) fn array<const N: usize>(&mut self, what: &str) -> Result<[u8; N], CryptoError> {
        let (head, tail) = self
            .rest
            .split_first_chunk::<N>()
            .ok_or_else(|| truncated(what))?;
        self.rest = tail;
        Ok(*head)
    }

    /// One byte.
    pub(crate) fn u8(&mut self, what: &str) -> Result<u8, CryptoError> {
        let [byte] = self.array::<1>(what)?;
        Ok(byte)
    }

    /// A little-endian `u32`.
    pub(crate) fn u32_le(&mut self, what: &str) -> Result<u32, CryptoError> {
        Ok(u32::from_le_bytes(self.array::<4>(what)?))
    }

    /// A byte string prefixed with its length as a little-endian `u32`.
    pub(crate) fn len_prefixed(&mut self, what: &str) -> Result<&'a [u8], CryptoError> {
        let len = self.u32_le(what)?;
        let len = usize::try_from(len).map_err(|_| truncated(what))?;
        self.take(len, what)
    }

    /// Everything not read yet.
    pub(crate) const fn rest(&self) -> &'a [u8] {
        self.rest
    }
}

fn truncated(what: &str) -> CryptoError {
    CryptoError::Encoding(format!("truncated {what}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_fields_in_order_and_reports_truncation() {
        let bytes = [7u8, 2, 0, 0, 0, 0xaa, 0xbb, 1, 2, 3];
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.u8("version").unwrap(), 7);
        assert_eq!(reader.len_prefixed("header").unwrap(), &[0xaa, 0xbb]);
        assert_eq!(reader.array::<2>("pair").unwrap(), [1, 2]);
        assert_eq!(reader.rest(), &[3]);
        assert_eq!(
            reader.u32_le("length"),
            Err(CryptoError::Encoding("truncated length".to_owned()))
        );
        assert_eq!(reader.rest(), &[3], "a failed read consumes nothing");
        assert_eq!(reader.take(1, "tail").unwrap(), &[3]);
        assert!(reader.rest().is_empty());
        assert!(reader.u8("more").is_err());
    }

    #[test]
    fn a_length_beyond_the_input_is_truncation() {
        let bytes = [5u8, 0, 0, 0, 1, 2];
        assert_eq!(
            Reader::new(&bytes).len_prefixed("header"),
            Err(CryptoError::Encoding("truncated header".to_owned()))
        );
        let huge = [0xff_u8, 0xff, 0xff, 0xff];
        assert!(Reader::new(&huge).len_prefixed("header").is_err());
    }
}
