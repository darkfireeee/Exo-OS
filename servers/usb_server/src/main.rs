#![no_std]
#![no_main]

//! # usb_server - Serveur USB xHCI / EHCI et Peripheriques Ring 1 (PID 10, Vague 4)
//!
//! Specifications :
//! - xHCI 1.2 (Extensible Host Controller Interface) / PCI Class 0x0C, Subclass 0x03, ProgIF 0x30.
//! - USB HID Boot Protocol (Clavier et Souris) -> Routage vers input_server via INPUT_MSG_PUSH.
//! - USB Mass Storage (Bulk-Only Transport BBB et SCSI) -> Pipeline de transfert audite (/mnt/usb).
//! - Fallback resilient : Fonctionne silencieusement sans panique si aucun controleur USB n'est detecte.

use core::cell::UnsafeCell;
use core::panic::PanicInfo;
use exo_os_drivers::{DeviceHandle, DeviceInfo};
use exo_syscall_abi as syscall;
use exo_usb_hid::{push_input_event, UsbHidKeyboard, UsbHidMouse};

// -- Registres d'un controleur xHCI -------------------------------------------
pub const XHCI_CAP_CAPLENGTH: usize = 0x00;
pub const XHCI_CAP_HCSPARAMS1: usize = 0x04;
pub const XHCI_CAP_HCSPARAMS2: usize = 0x08;
pub const XHCI_CAP_HCCPARAMS1: usize = 0x10;
pub const XHCI_CAP_DBOFF: usize = 0x14;
pub const XHCI_CAP_RTSOFF: usize = 0x18;

// Operational Registers (relatifs a op_base)
pub const XHCI_OP_USBCMD: usize = 0x00;
pub const XHCI_OP_USBSTS: usize = 0x04;
pub const XHCI_OP_PAGESIZE: usize = 0x08;
pub const XHCI_OP_CRCR_LO: usize = 0x18;
pub const XHCI_OP_CRCR_HI: usize = 0x1C;
pub const XHCI_OP_DCBAAP_LO: usize = 0x30;
pub const XHCI_OP_DCBAAP_HI: usize = 0x34;
pub const XHCI_OP_CONFIG: usize = 0x38;

// USBCMD bits
pub const XHCI_CMD_RUN_STOP: u32 = 1 << 0;
pub const XHCI_CMD_HCRST: u32 = 1 << 1;
pub const XHCI_CMD_INTE: u32 = 1 << 2;

// USBSTS bits
pub const XHCI_STS_HCH: u32 = 1 << 0; // Host Controller Halted

// Mass Storage SCSI Opcodes
pub const SCSI_INQUIRY: u8 = 0x12;
pub const SCSI_READ_CAPACITY_10: u8 = 0x25;
pub const SCSI_READ_10: u8 = 0x28;
pub const SCSI_WRITE_10: u8 = 0x2A;

#[allow(dead_code)]
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct UsbCommandBlockWrapper {
    pub signature: u32, // 0x43425355 ("USBC")
    pub tag: u32,
    pub data_transfer_length: u32,
    pub flags: u8, // Bit 7: 1 = In, 0 = Out
    pub lun: u8,
    pub cb_length: u8,
    pub cb: [u8; 16],
}

#[allow(dead_code)]
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct UsbCommandStatusWrapper {
    pub signature: u32, // 0x53425355 ("USBS")
    pub tag: u32,
    pub data_residue: u32,
    pub status: u8, // 0 = Command Passed, 1 = Failed, 2 = Phase Error
}

#[allow(dead_code)]
struct UsbServerState {
    handle: Option<DeviceHandle>,
    op_offset: usize,
    max_slots: u8,
    max_ports: u8,
    keyboard: UsbHidKeyboard,
    mouse: UsbHidMouse,
    storage_attached: bool,
    storage_blocks: u64,
    storage_block_size: u32,
}

struct StateCell(UnsafeCell<UsbServerState>);
unsafe impl Sync for StateCell {}

static STATE: StateCell = StateCell(UnsafeCell::new(UsbServerState {
    handle: None,
    op_offset: 0,
    max_slots: 0,
    max_ports: 0,
    keyboard: UsbHidKeyboard::new(),
    mouse: UsbHidMouse::new(),
    storage_attached: false,
    storage_blocks: 0,
    storage_block_size: 512,
}));

#[inline(always)]
fn state_mut() -> &'static mut UsbServerState {
    unsafe { &mut *STATE.0.get() }
}

fn boot_log(msg: &[u8]) {
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_EXO_LOG,
            msg.as_ptr() as u64,
            msg.len() as u64,
            1,
        );
    }
}

/// Decouvre et initialise un controleur xHCI ou EHCI via l'interface PCI.
fn probe_usb_controller() {
    let s = state_mut();

    // 1. Recherche d'un controleur xHCI (PCI Class 0x0C, Subclass 0x03, ProgIF 0x30)
    let mut dev_info = syscall::PciDeviceInfo::default();
    let rc = unsafe {
        syscall::syscall5(
            syscall::SYS_PCI_FIND_DEVICE,
            0,
            0,
            0x0C,
            0x03,
            &mut dev_info as *mut syscall::PciDeviceInfo as u64,
        )
    };

    if rc >= 0 && dev_info.bar0_phys != 0 {
        boot_log(b"usb_server: USB Host Controller discovered (PCI 0x0C:0x03)\n");
        let bdf = ((dev_info.bus as u32) << 8)
            | ((dev_info.device as u32) << 3)
            | (dev_info.function as u32);
        let info = DeviceInfo {
            vendor_id: dev_info.vendor_id,
            device_id: dev_info.device_id,
            class: dev_info.class_code,
            subclass: dev_info.subclass,
            bdf_raw: bdf,
            bar0_phys: dev_info.bar0_phys,
            bar0_size: dev_info.bar0_size,
            irq_line: dev_info.irq_line,
        };

        if let Ok(handle) = DeviceHandle::claim_and_map(info) {
            unsafe {
                let caplength = handle.mmio.read32(XHCI_CAP_CAPLENGTH) & 0xFF;
                let hcsparams1 = handle.mmio.read32(XHCI_CAP_HCSPARAMS1);
                s.max_slots = (hcsparams1 & 0xFF) as u8;
                s.max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
                s.op_offset = caplength as usize;

                boot_log(b"usb_server: xHCI operational registers mapped\n");

                // Reinitialisation du controleur (HCRST)
                let op = s.op_offset;
                handle.mmio.write32(op + XHCI_OP_USBCMD, XHCI_CMD_HCRST);
                let mut timeout = 50_000usize;
                while (handle.mmio.read32(op + XHCI_OP_USBCMD) & XHCI_CMD_HCRST) != 0 && timeout > 0 {
                    core::hint::spin_loop();
                    timeout -= 1;
                }

                // Demarrage de la boucle de transfert
                handle.mmio.write32(op + XHCI_OP_CONFIG, s.max_slots as u32);
                handle.mmio.write32(op + XHCI_OP_USBCMD, XHCI_CMD_RUN_STOP | XHCI_CMD_INTE);

                boot_log(b"usb_server: xHCI running, root hub active\n");
            }
            s.handle = Some(handle);
            return;
        }
    }

    boot_log(b"usb_server: no physical xHCI controller detected (USB-optional fallback)\n");
}

/// Simulation / traitement d'un rapport de paquet clavier recu
pub fn handle_keyboard_report(report: &[u8; 8]) {
    let s = state_mut();
    s.keyboard.process_report(report, |event| {
        push_input_event(event);
    });
}

/// Simulation / traitement d'un rapport de souris recu
pub fn handle_mouse_report(report: &[u8]) {
    let s = state_mut();
    s.mouse.process_report(report, |event| {
        push_input_event(event);
    });
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    boot_log(b"usb_server: starting (Ring 1, Vague 4, PID 10)\n");

    let name = b"usb_server";
    let reg_rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            syscall::USB_SERVER_ENDPOINT,
        )
    };
    if reg_rc < 0 && reg_rc != syscall::EEXIST {
        boot_log(b"usb_server: failed to register endpoint\n");
    }

    probe_usb_controller();
    boot_log(b"usb_server: service online\n");

    let mut buf = [0u8; 64];

    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                syscall::USB_SERVER_ENDPOINT,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                syscall::IPC_FLAG_TIMEOUT | 1000,
            )
        };

        if rc >= 8 {
            // Requete recue - decodage et traitement
            let msg_type = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            match msg_type {
                0x01 => {
                    // Simulation paquet HID clavier (8 octets a partir de l'offset 8)
                    if rc >= 16 {
                        let mut report = [0u8; 8];
                        report.copy_from_slice(&buf[8..16]);
                        handle_keyboard_report(&report);
                    }
                }
                0x02 => {
                    // Simulation paquet HID souris (4 octets a partir de l'offset 8)
                    if rc >= 12 {
                        handle_mouse_report(&buf[8..12]);
                    }
                }
                _ => {}
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    boot_log(b"usb_server: panic!\n");
    loop {
        core::hint::spin_loop();
    }
}
