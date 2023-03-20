//! Fixed-capacity byte queue for serialized console and device buffering.

/// A byte was rejected without changing queued data.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RingError {
    /// No unused slot remains.
    Full,
}

/// A FIFO byte queue with exactly `N` usable slots and no synchronization.
pub struct ByteRing<const N: usize> {
    bytes: [u8; N],
    head: usize,
    tail: usize,
    len: usize,
}

impl<const N: usize> ByteRing<N> {
    /// Construct an empty queue, including when `N == 0`.
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            head: 0,
            tail: 0,
            len: 0,
        }
    }
    /// Maximum number of queued bytes.
    pub const fn capacity(&self) -> usize {
        N
    }
    /// Number of bytes waiting to be read.
    pub fn len(&self) -> usize {
        self.len
    }
    /// Whether there are no readable bytes.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Queue one byte, rejecting overflow without overwriting old data.
    pub fn push(&mut self, byte: u8) -> Result<(), RingError> {
        if self.len == N {
            return Err(RingError::Full);
        }
        self.bytes[self.tail] = byte;
        self.tail = if self.tail + 1 == N { 0 } else { self.tail + 1 };
        self.len += 1;
        Ok(())
    }
    /// Remove the oldest byte, or return `None` without mutation when empty.
    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.bytes[self.head];
        self.head = if self.head + 1 == N { 0 } else { self.head + 1 };
        self.len -= 1;
        Some(byte)
    }
    /// Queue as many bytes as fit and return the accepted prefix length.
    pub fn write(&mut self, bytes: &[u8]) -> usize {
        let mut written = 0;
        for &byte in bytes {
            if self.push(byte).is_err() {
                break;
            }
            written += 1;
        }
        written
    }
    /// Fill a prefix of `output` with queued bytes and return its length.
    pub fn read(&mut self, output: &mut [u8]) -> usize {
        let mut count = 0;
        for slot in output {
            if let Some(byte) = self.pop() {
                *slot = byte;
                count += 1;
            } else {
                break;
            }
        }
        count
    }
    /// Drop queued bytes without promising to securely erase their storage.
    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.len = 0;
    }
}

impl<const N: usize> Default for ByteRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_queue_never_overwrites_and_wrap_preserves_fifo_order() {
        let mut ring = ByteRing::<3>::new();
        assert!(ring.is_empty());
        assert_eq!(ring.pop(), None);
        assert_eq!(ring.write(&[1, 2, 3, 4]), 3);
        assert_eq!(ring.push(5), Err(RingError::Full));
        assert_eq!(ring.pop(), Some(1));
        ring.push(4).unwrap();
        let mut bytes = [0; 4];
        assert_eq!(ring.read(&mut bytes), 3);
        assert_eq!(bytes, [2, 3, 4, 0]);
        assert!(ring.is_empty());
        assert_eq!(ring.capacity(), 3);
    }
    #[test]
    fn zero_and_single_capacity_and_clear_are_defined() {
        let mut zero = ByteRing::<0>::new();
        assert_eq!(zero.push(1), Err(RingError::Full));
        assert_eq!(zero.pop(), None);
        assert_eq!(zero.write(&[1]), 0);
        let mut one = ByteRing::<1>::new();
        for byte in 0..100 {
            one.push(byte).unwrap();
            assert_eq!(one.len(), 1);
            assert_eq!(one.pop(), Some(byte));
        }
        one.push(7).unwrap();
        one.clear();
        assert_eq!(one.len(), 0);
        assert_eq!(one.pop(), None);
    }
}
