//! Single-reader/single-writer, exclusively owned bounded byte pipe.

use crate::{
    drivers::ring::ByteRing,
    syscall::{ConsoleIo, Errno},
};

/// Fixed-capacity byte stream with one logical read and write endpoint.
/// It is not synchronized; the caller owns serialization and wakeup delivery.
pub struct Pipe<const N: usize> {
    bytes: ByteRing<N>,
    reader_open: bool,
    writer_open: bool,
}
impl<const N: usize> Pipe<N> {
    /// Construct an empty stream with both endpoints open.
    pub const fn new() -> Self {
        Self {
            bytes: ByteRing::new(),
            reader_open: true,
            writer_open: true,
        }
    }
    /// Number of queued bytes, excluding any bytes already consumed.
    pub fn queued_bytes(&self) -> usize {
        self.bytes.len()
    }
    /// Close the writer; queued data remains readable before EOF.
    pub fn close_writer(&mut self) {
        self.writer_open = false;
    }
    /// Close the reader and discard queued data; future nonempty writes fail.
    pub fn close_reader(&mut self) {
        self.reader_open = false;
        self.bytes.clear();
    }
    /// Write a partial nonblocking prefix; a full queue reports `Again`.
    pub fn write(&mut self, input: &[u8]) -> Result<usize, Errno> {
        if !self.writer_open {
            return Err(Errno::BadFd);
        }
        if input.is_empty() {
            return Ok(0);
        }
        if !self.reader_open {
            return Err(Errno::Pipe);
        }
        let count = self.bytes.write(input);
        if count == 0 {
            Err(Errno::Again)
        } else {
            Ok(count)
        }
    }
    /// Read a partial prefix; zero means drained EOF, not temporary emptiness.
    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, Errno> {
        if !self.reader_open {
            return Err(Errno::BadFd);
        }
        if output.is_empty() {
            return Ok(0);
        }
        let count = self.bytes.read(output);
        if count == 0 && self.writer_open {
            Err(Errno::Again)
        } else {
            Ok(count)
        }
    }
}
impl<const N: usize> Default for Pipe<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> ConsoleIo for Pipe<N> {
    fn write(&mut self, input: &[u8]) -> Result<usize, Errno> {
        Pipe::write(self, input)
    }
    fn read(&mut self, output: &mut [u8]) -> Result<usize, Errno> {
        Pipe::read(self, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syscall::Errno;
    #[test]
    fn pipe_drains_before_eof_and_never_overwrites_a_full_queue() {
        let mut pipe = Pipe::<3>::new();
        let mut output = [0; 4];
        assert_eq!(pipe.read(&mut output), Err(Errno::Again));
        assert_eq!(pipe.write(&[1, 2, 3, 4]), Ok(3));
        assert_eq!(pipe.write(&[4]), Err(Errno::Again));
        assert_eq!(pipe.read(&mut output[..2]), Ok(2));
        assert_eq!(output[..2], [1, 2]);
        assert_eq!(pipe.write(&[4, 5]), Ok(2));
        pipe.close_writer();
        assert_eq!(pipe.read(&mut output), Ok(3));
        assert_eq!(output[..3], [3, 4, 5]);
        assert_eq!(pipe.read(&mut output), Ok(0));
        assert_eq!(pipe.write(&[6]), Err(Errno::BadFd));
    }
    #[test]
    fn closed_reader_discards_queued_data_and_rejects_future_writes() {
        let mut pipe = Pipe::<1>::new();
        pipe.write(&[1]).unwrap();
        pipe.close_reader();
        assert_eq!(pipe.queued_bytes(), 0);
        assert_eq!(pipe.write(&[2]), Err(Errno::Pipe));
        assert_eq!(pipe.read(&mut [0]), Err(Errno::BadFd));
        let mut empty = Pipe::<0>::new();
        assert_eq!(empty.write(&[1]), Err(Errno::Again));
        assert_eq!(empty.write(&[]), Ok(0));
        assert_eq!(empty.read(&mut []), Ok(0));
    }
}
