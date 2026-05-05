pub trait ConsoleSink {
    fn write_byte(&mut self, byte: u8);

    fn write_all(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_byte(byte);
        }
    }
}

pub fn write_crlf_normalized<S: ConsoleSink>(sink: &mut S, data: &[u8]) {
    for &byte in data {
        if byte == b'\n' {
            sink.write_byte(b'\r');
        }
        sink.write_byte(byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct VecSink {
        data: std::vec::Vec<u8>,
    }

    impl ConsoleSink for VecSink {
        fn write_byte(&mut self, byte: u8) {
            self.data.push(byte);
        }
    }

    #[test]
    fn normalizes_newline() {
        let mut sink = VecSink::default();
        write_crlf_normalized(&mut sink, b"a\n");
        assert_eq!(sink.data, b"a\r\n");
    }
}
