//! BootInfo userspace pour `init_server`.
//!
//! Le kernel doit mapper cette structure en lecture seule dans la VMA de PID 1
//! et passer son adresse virtuelle en premier argument de `_start()`.

use core::mem::size_of;

pub const BOOT_INFO_MAGIC: u64 = 0x424F_4F54_5F49_4E46;
pub const BOOT_INFO_VERSION: u32 = 1;
const IPC_BROKER_CAP_TYPE_ID: u16 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectId(pub [u8; 32]);

impl ObjectId {
    #[inline]
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|&byte| byte == 0)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CapToken {
    pub generation: u64,
    pub object_id: ObjectId,
    pub rights: u32,
    pub type_id: u16,
    pub _pad: [u8; 2],
}

impl CapToken {
    #[inline]
    pub fn validate_ipc_broker(&self) -> bool {
        self.generation != 0
            && self.type_id == IPC_BROKER_CAP_TYPE_ID
            && self._pad == [0u8; 2]
            && !self.object_id.is_zero()
    }
}

const _: () = assert!(size_of::<CapToken>() == 48);

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u32,
    pub _pad: [u8; 4],
    pub ipc_broker_cap: CapToken,
    pub ssr_phys_addr: u64,
    pub nr_cpus: u32,
    pub _pad2: [u8; 4],
    pub memory_bitmap_phys: u64,
    pub memory_bitmap_size: u64,
    pub kernel_heap_start: u64,
    pub kernel_heap_end: u64,
    pub reserved: [u64; 16],
}

impl BootInfo {
    #[inline]
    pub fn validate(&self) -> bool {
        if self.magic != BOOT_INFO_MAGIC || self.version != BOOT_INFO_VERSION {
            return false;
        }
        if self._pad != [0u8; 4] || self._pad2 != [0u8; 4] {
            return false;
        }
        if self.reserved.iter().any(|&value| value != 0) {
            return false;
        }
        if self.nr_cpus == 0 {
            return false;
        }
        if self.memory_bitmap_phys == 0 || self.memory_bitmap_size == 0 {
            return false;
        }
        if self.kernel_heap_start == 0 || self.kernel_heap_start >= self.kernel_heap_end {
            return false;
        }
        self.ipc_broker_cap.validate_ipc_broker()
    }

    #[inline]
    pub unsafe fn from_virt(boot_info_virt: usize) -> Option<&'static Self> {
        if boot_info_virt == 0 {
            return None;
        }

        // SAFETY: l'appelant garantit que `boot_info_virt` pointe une page
        // `BootInfo` mappée en lecture seule dans l'espace d'adressage de PID 1.
        Some(&*(boot_info_virt as *const Self))
    }
}
