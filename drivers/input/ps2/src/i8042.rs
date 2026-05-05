use crate::InputEvent;

pub const DATA_PORT: u16 = 0x60;
pub const STATUS_PORT: u16 = 0x64;
pub const COMMAND_PORT: u16 = 0x64;

pub const STATUS_OUTPUT_FULL: u8 = 1 << 0;
pub const STATUS_INPUT_FULL: u8 = 1 << 1;

pub const CMD_READ_CONFIG: u8 = 0x20;
pub const CMD_WRITE_CONFIG: u8 = 0x60;
pub const CMD_ENABLE_AUX: u8 = 0xA8;
pub const CMD_ENABLE_KEYBOARD: u8 = 0xAE;
pub const CMD_DISABLE_KEYBOARD: u8 = 0xAD;
pub const CMD_DISABLE_AUX: u8 = 0xA7;

pub const CONFIG_IRQ1: u8 = 1 << 0;
pub const CONFIG_IRQ12: u8 = 1 << 1;
pub const CONFIG_TRANSLATION: u8 = 1 << 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerError {
    Timeout,
}

pub trait PortIo {
    fn read_u8(&mut self, port: u16) -> u8;
    fn write_u8(&mut self, port: u16, value: u8);
}

pub struct I8042<P> {
    io: P,
    spin_limit: usize,
}

impl<P: PortIo> I8042<P> {
    pub const fn new(io: P) -> Self {
        Self {
            io,
            spin_limit: 100_000,
        }
    }

    pub const fn with_spin_limit(io: P, spin_limit: usize) -> Self {
        Self { io, spin_limit }
    }

    pub fn into_inner(self) -> P {
        self.io
    }

    pub fn init(&mut self) -> Result<(), ControllerError> {
        self.write_command(CMD_DISABLE_KEYBOARD)?;
        self.write_command(CMD_DISABLE_AUX)?;
        self.drain_output();
        self.write_command(CMD_READ_CONFIG)?;
        let mut config = self.read_data()?;
        config |= CONFIG_IRQ1 | CONFIG_IRQ12;
        config &= !CONFIG_TRANSLATION;
        self.write_command(CMD_WRITE_CONFIG)?;
        self.write_data(config)?;
        self.write_command(CMD_ENABLE_AUX)?;
        self.write_command(CMD_ENABLE_KEYBOARD)
    }

    pub fn poll_byte(&mut self) -> Option<u8> {
        if self.io.read_u8(STATUS_PORT) & STATUS_OUTPUT_FULL != 0 {
            Some(self.io.read_u8(DATA_PORT))
        } else {
            None
        }
    }

    pub fn write_command(&mut self, cmd: u8) -> Result<(), ControllerError> {
        self.wait_input_empty()?;
        self.io.write_u8(COMMAND_PORT, cmd);
        Ok(())
    }

    pub fn write_data(&mut self, data: u8) -> Result<(), ControllerError> {
        self.wait_input_empty()?;
        self.io.write_u8(DATA_PORT, data);
        Ok(())
    }

    pub fn read_data(&mut self) -> Result<u8, ControllerError> {
        self.wait_output_full()?;
        Ok(self.io.read_u8(DATA_PORT))
    }

    fn wait_input_empty(&mut self) -> Result<(), ControllerError> {
        for _ in 0..self.spin_limit {
            if self.io.read_u8(STATUS_PORT) & STATUS_INPUT_FULL == 0 {
                return Ok(());
            }
        }
        Err(ControllerError::Timeout)
    }

    fn wait_output_full(&mut self) -> Result<(), ControllerError> {
        for _ in 0..self.spin_limit {
            if self.io.read_u8(STATUS_PORT) & STATUS_OUTPUT_FULL != 0 {
                return Ok(());
            }
        }
        Err(ControllerError::Timeout)
    }

    fn drain_output(&mut self) {
        while self.io.read_u8(STATUS_PORT) & STATUS_OUTPUT_FULL != 0 {
            let _ = self.io.read_u8(DATA_PORT);
        }
    }
}

pub trait InputSink {
    fn push_input(&mut self, event: InputEvent);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakePorts {
        status: u8,
        data: u8,
        writes: [u8; 8],
        write_len: usize,
    }

    impl PortIo for FakePorts {
        fn read_u8(&mut self, port: u16) -> u8 {
            match port {
                STATUS_PORT => self.status,
                DATA_PORT => self.data,
                _ => 0,
            }
        }

        fn write_u8(&mut self, _port: u16, value: u8) {
            self.writes[self.write_len] = value;
            self.write_len += 1;
        }
    }

    #[test]
    fn poll_byte_reads_when_output_full() {
        let ports = FakePorts {
            status: STATUS_OUTPUT_FULL,
            data: 0x1c,
            ..FakePorts::default()
        };
        let mut ctl = I8042::new(ports);
        assert_eq!(ctl.poll_byte(), Some(0x1c));
    }

    #[test]
    fn write_command_times_out_when_input_full() {
        let ports = FakePorts {
            status: STATUS_INPUT_FULL,
            ..FakePorts::default()
        };
        let mut ctl = I8042::with_spin_limit(ports, 2);
        assert_eq!(ctl.write_command(0xAE), Err(ControllerError::Timeout));
    }
}
