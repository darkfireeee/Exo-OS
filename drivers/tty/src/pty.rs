pub const PTY_BUF_SIZE: usize = 4096;

#[derive(Clone)]
pub struct RingBuffer {
    buf: [u8; PTY_BUF_SIZE],
    head: usize,
    tail: usize,
    len: usize,
}

impl Default for RingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl RingBuffer {
    pub const fn new() -> Self {
        Self {
            buf: [0; PTY_BUF_SIZE],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn available_read(&self) -> usize {
        self.len
    }

    pub fn available_write(&self) -> usize {
        PTY_BUF_SIZE - self.len
    }

    pub fn push(&mut self, byte: u8) -> bool {
        if self.len == PTY_BUF_SIZE {
            return false;
        }
        self.buf[self.tail] = byte;
        self.tail = (self.tail + 1) % PTY_BUF_SIZE;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.buf[self.head];
        self.head = (self.head + 1) % PTY_BUF_SIZE;
        self.len -= 1;
        Some(byte)
    }

    pub fn write(&mut self, data: &[u8]) -> usize {
        let mut n = 0usize;
        for &byte in data {
            if !self.push(byte) {
                break;
            }
            n += 1;
        }
        n
    }

    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let mut n = 0usize;
        while n < out.len() {
            match self.pop() {
                Some(byte) => {
                    out[n] = byte;
                    n += 1;
                }
                None => break,
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_ring_buffer() {
        let mut rb = RingBuffer::new();
        assert_eq!(rb.write(b"abc"), 3);
        let mut out = [0u8; 2];
        assert_eq!(rb.read(&mut out), 2);
        assert_eq!(&out, b"ab");
        assert_eq!(rb.pop(), Some(b'c'));
    }
}
