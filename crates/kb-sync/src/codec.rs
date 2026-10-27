//! Strict little-endian readers and writers for the wire formats. Every failure is
//! [`SyncError::Encoding`] naming the object and the field, never the bytes.

use crate::SyncError;

/// A cursor over a byte string that refuses truncation and trailing bytes.
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    /// The object being read, for error messages.
    what: &'static str,
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, what }
    }

    fn truncated(&self, field: &str) -> SyncError {
        SyncError::Encoding(format!("{}: truncated {field}", self.what))
    }

    /// Bytes not read yet.
    pub(crate) const fn remaining(&self) -> usize {
        self.bytes.len()
    }

    /// The next `len` bytes.
    pub(crate) fn take(&mut self, len: usize, field: &str) -> Result<&'a [u8], SyncError> {
        let Some((head, rest)) = self.bytes.split_at_checked(len) else {
            return Err(self.truncated(field));
        };
        self.bytes = rest;
        Ok(head)
    }

    pub(crate) fn array<const N: usize>(&mut self, field: &str) -> Result<[u8; N], SyncError> {
        let Some((head, rest)) = self.bytes.split_first_chunk::<N>() else {
            return Err(self.truncated(field));
        };
        self.bytes = rest;
        Ok(*head)
    }

    pub(crate) fn u8(&mut self, field: &str) -> Result<u8, SyncError> {
        self.array::<1>(field).map(|[b]| b)
    }

    pub(crate) fn u16(&mut self, field: &str) -> Result<u16, SyncError> {
        self.array(field).map(u16::from_le_bytes)
    }

    pub(crate) fn u32(&mut self, field: &str) -> Result<u32, SyncError> {
        self.array(field).map(u32::from_le_bytes)
    }

    pub(crate) fn u64(&mut self, field: &str) -> Result<u64, SyncError> {
        self.array(field).map(u64::from_le_bytes)
    }

    pub(crate) fn i64(&mut self, field: &str) -> Result<i64, SyncError> {
        self.array(field).map(i64::from_le_bytes)
    }

    /// A `u32 LE` length, then that many bytes.
    pub(crate) fn blob(&mut self, field: &str) -> Result<&'a [u8], SyncError> {
        let len = self.u32(field)?;
        let len = usize::try_from(len).map_err(|_| self.truncated(field))?;
        self.take(len, field)
    }

    /// A `u32 LE` count of items that each take at least `min_item_len` bytes; refuses
    /// counts the remaining input cannot hold, so callers can pre-allocate safely.
    pub(crate) fn count(&mut self, field: &str, min_item_len: usize) -> Result<usize, SyncError> {
        let count = self.u32(field)?;
        self.fit_count(count, field, min_item_len)
    }

    /// Checks a count already read the way [`Self::count`] does.
    pub(crate) fn fit_count(
        &self,
        count: u32,
        field: &str,
        min_item_len: usize,
    ) -> Result<usize, SyncError> {
        let count = usize::try_from(count).map_err(|_| self.truncated(field))?;
        let needed = count.checked_mul(min_item_len.max(1));
        if needed.is_none_or(|needed| needed > self.remaining()) {
            return Err(SyncError::Encoding(format!(
                "{}: {field} of {count} exceeds the input",
                self.what
            )));
        }
        Ok(count)
    }

    /// Succeeds only when every byte was read.
    pub(crate) fn finish(self) -> Result<(), SyncError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(SyncError::Encoding(format!(
                "{}: {} unexpected trailing bytes",
                self.what,
                self.bytes.len()
            )))
        }
    }
}

/// Appends `u32 LE len || bytes`; `None` when the length does not fit in a `u32`.
pub(crate) fn put_blob(out: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    let len = u32::try_from(bytes.len()).ok()?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Some(())
}

/// Appends a `u32 LE` count; `None` when it does not fit.
pub(crate) fn put_count(out: &mut Vec<u8>, count: usize) -> Option<()> {
    let count = u32::try_from(count).ok()?;
    out.extend_from_slice(&count.to_le_bytes());
    Some(())
}
