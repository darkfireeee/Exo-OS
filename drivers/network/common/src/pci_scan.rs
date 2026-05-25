#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PciBdf {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
}

impl PciBdf {
    pub const fn raw(self) -> u32 {
        ((self.bus as u32) << 8) | ((self.device as u32) << 3) | self.function as u32
    }
}

pub struct PciScanner {
    bus: u16,
    device: u8,
    function: u8,
}

impl PciScanner {
    pub const fn new() -> Self {
        Self {
            bus: 0,
            device: 0,
            function: 0,
        }
    }
}

impl Iterator for PciScanner {
    type Item = PciBdf;

    fn next(&mut self) -> Option<Self::Item> {
        if self.bus > 255 {
            return None;
        }
        let item = PciBdf {
            bus: self.bus as u8,
            device: self.device,
            function: self.function,
        };
        self.function += 1;
        if self.function == 8 {
            self.function = 0;
            self.device += 1;
            if self.device == 32 {
                self.device = 0;
                self.bus += 1;
            }
        }
        Some(item)
    }
}
