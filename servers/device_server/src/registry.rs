use exo_syscall_abi as syscall;

const MAX_PCI_DEVICES: usize = 128;

#[derive(Clone, Copy)]
struct DeviceRecord {
    active: bool,
    phys_base: u64,
    size: u64,
    bdf_raw: u32,
    parent_bdf_raw: u32,
    vendor_device: u32,
    class_code: u32,
    owner_pid: u32,
    flags: u32,
}

impl DeviceRecord {
    const fn empty() -> Self {
        Self {
            active: false,
            phys_base: 0,
            size: 0,
            bdf_raw: 0,
            parent_bdf_raw: 0,
            vendor_device: 0,
            class_code: 0,
            owner_pid: 0,
            flags: 0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct DeviceSnapshot {
    pub phys_base: u64,
    pub size: u64,
    pub bdf_raw: u32,
    pub parent_bdf_raw: u32,
    pub vendor_device: u32,
    pub class_code: u32,
    pub owner_pid: u32,
    pub flags: u32,
}

pub struct PciRegistry {
    devices: [DeviceRecord; MAX_PCI_DEVICES],
}

impl PciRegistry {
    pub const fn new() -> Self {
        Self {
            devices: [DeviceRecord::empty(); MAX_PCI_DEVICES],
        }
    }

    pub fn register_device(
        &mut self,
        phys_base: u64,
        size: u64,
        bdf_raw: u32,
        parent_bdf_raw: u32,
        vendor_device: u32,
        class_code: u32,
        flags: u32,
    ) -> Result<DeviceSnapshot, i64> {
        if size == 0 {
            return Err(syscall::EINVAL);
        }

        if let Some(idx) = self.devices.iter().position(|device| device.active && device.bdf_raw == bdf_raw) {
            self.devices[idx] = DeviceRecord {
                active: true,
                phys_base,
                size,
                bdf_raw,
                parent_bdf_raw,
                vendor_device,
                class_code,
                owner_pid: self.devices[idx].owner_pid,
                flags,
            };
            return Ok(self.snapshot(idx));
        }

        let Some(idx) = self.devices.iter().position(|device| !device.active) else {
            return Err(syscall::ENOSPC);
        };

        self.devices[idx] = DeviceRecord {
            active: true,
            phys_base,
            size,
            bdf_raw,
            parent_bdf_raw,
            vendor_device,
            class_code,
            owner_pid: 0,
            flags,
        };
        Ok(self.snapshot(idx))
    }

    pub fn assign_owner(&mut self, owner_pid: u32, bdf_raw: u32) -> Result<DeviceSnapshot, i64> {
        let Some(idx) = self.devices.iter().position(|device| device.active && device.bdf_raw == bdf_raw) else {
            return Err(syscall::ENOENT);
        };
        if self.devices[idx].owner_pid != 0 {
            return Err(syscall::EBUSY);
        }
        self.devices[idx].owner_pid = owner_pid;
        Ok(self.snapshot(idx))
    }

    pub fn release_owner(&mut self, owner_pid: u32) -> Option<DeviceSnapshot> {
        let idx = self.devices.iter().position(|device| device.active && device.owner_pid == owner_pid)?;
        self.devices[idx].owner_pid = 0;
        Some(self.snapshot(idx))
    }

    pub fn snapshot_by_bdf(&self, bdf_raw: u32) -> Option<DeviceSnapshot> {
        let idx = self.devices.iter().position(|device| device.active && device.bdf_raw == bdf_raw)?;
        Some(self.snapshot(idx))
    }

    pub fn bdf_of_owner(&self, owner_pid: u32) -> Option<u32> {
        self.devices
            .iter()
            .find(|device| device.active && device.owner_pid == owner_pid)
            .map(|device| device.bdf_raw)
    }

    fn snapshot(&self, idx: usize) -> DeviceSnapshot {
        let device = self.devices[idx];
        DeviceSnapshot {
            phys_base: device.phys_base,
            size: device.size,
            bdf_raw: device.bdf_raw,
            parent_bdf_raw: device.parent_bdf_raw,
            vendor_device: device.vendor_device,
            class_code: device.class_code,
            owner_pid: device.owner_pid,
            flags: device.flags,
        }
    }
}
