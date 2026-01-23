//! Lock-free ring buffer for capturing command output.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A fixed-size ring buffer for capturing output.
///
/// Single producer, multiple consumer. The producer never blocks;
/// old data is overwritten when the buffer is full.
pub struct RingBuffer {
    /// The buffer storage.
    data: Box<[u8]>,

    /// Write position (monotonically increasing).
    write_pos: AtomicUsize,

    /// Total bytes written (for overflow detection).
    total_written: AtomicUsize,

    /// Capacity of the buffer.
    capacity: usize,
}

impl RingBuffer {
    /// Create a new ring buffer with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![0u8; capacity].into_boxed_slice(),
            write_pos: AtomicUsize::new(0),
            total_written: AtomicUsize::new(0),
            capacity,
        }
    }

    /// Write data to the buffer. Overwrites old data if full.
    pub fn write(&self, data: &[u8]) {
        let len = data.len();
        if len == 0 {
            return;
        }

        // Get current write position
        let pos = self.write_pos.load(Ordering::Acquire);

        // Write data, wrapping around if necessary
        for (i, &byte) in data.iter().enumerate() {
            let idx = (pos + i) % self.capacity;
            // Safety: We're the only writer, and readers handle torn reads
            unsafe {
                let ptr = self.data.as_ptr() as *mut u8;
                ptr.add(idx).write(byte);
            }
        }

        // Update positions
        self.write_pos.store(pos + len, Ordering::Release);
        self.total_written.fetch_add(len, Ordering::Release);
    }

    /// Read all available data from the buffer.
    pub fn read_all(&self) -> Vec<u8> {
        let total = self.total_written.load(Ordering::Acquire);
        let pos = self.write_pos.load(Ordering::Acquire);

        if total == 0 {
            return Vec::new();
        }

        let len = total.min(self.capacity);
        let start = if total > self.capacity {
            pos % self.capacity
        } else {
            0
        };

        let mut result = Vec::with_capacity(len);

        // Read data in order
        for i in 0..len {
            let idx = (start + i) % self.capacity;
            result.push(self.data[idx]);
        }

        result
    }

    /// Read the last N bytes from the buffer.
    pub fn read_tail(&self, n: usize) -> Vec<u8> {
        let total = self.total_written.load(Ordering::Acquire);
        let pos = self.write_pos.load(Ordering::Acquire);

        if total == 0 {
            return Vec::new();
        }

        let available = total.min(self.capacity);
        let to_read = n.min(available);
        let start = (pos + self.capacity - to_read) % self.capacity;

        let mut result = Vec::with_capacity(to_read);

        for i in 0..to_read {
            let idx = (start + i) % self.capacity;
            result.push(self.data[idx]);
        }

        result
    }

    /// Get the total number of bytes written (may exceed capacity).
    pub fn total_written(&self) -> usize {
        self.total_written.load(Ordering::Acquire)
    }

    /// Check if data has been overwritten (buffer overflow).
    pub fn has_overflow(&self) -> bool {
        self.total_written.load(Ordering::Acquire) > self.capacity
    }

    /// Get the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_write_read() {
        let buf = RingBuffer::new(1024);
        buf.write(b"hello world");

        let data = buf.read_all();
        assert_eq!(data, b"hello world");
    }

    #[test]
    fn test_overflow() {
        let buf = RingBuffer::new(10);
        buf.write(b"hello world!"); // 12 bytes into 10-byte buffer

        assert!(buf.has_overflow());
        let data = buf.read_all();
        assert_eq!(data.len(), 10);
    }

    #[test]
    fn test_read_tail() {
        let buf = RingBuffer::new(1024);
        buf.write(b"hello world");

        let tail = buf.read_tail(5);
        assert_eq!(tail, b"world");
    }
}
