#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
use core::panic::PanicInfo;

#[cfg(target_os = "none")]
use exo_ps2_input::{
    i8042::{PortIo, I8042},
    keyboard::Ps2Keyboard,
    InputDevice, InputEvent,
};
#[cfg(target_os = "none")]
use exo_syscall_abi as syscall;

#[cfg(target_os = "none")]
const IRQ_KEYBOARD_LINE: u64 = 1;
#[cfg(target_os = "none")]
const IRQ_KEYBOARD_VECTOR: u64 = 33;
#[cfg(target_os = "none")]
const IRQ_SOURCE_IOAPIC_EDGE: u64 = 0;
#[cfg(target_os = "none")]
const IRQ_ACK_HANDLED: u64 = 0;
#[cfg(target_os = "none")]
const IRQ_RECV_TIMEOUT_MS: u64 = 2;
#[cfg(target_os = "none")]
const IRQ_DRAIN_LIMIT: usize = 64;

#[cfg(target_os = "none")]
#[repr(C)]
#[derive(Clone, Copy)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[cfg(target_os = "none")]
struct SyscallPorts;

#[cfg(target_os = "none")]
impl PortIo for SyscallPorts {
    fn read_u8(&mut self, port: u16) -> u8 {
        let rc = unsafe { syscall::syscall2(syscall::SYS_IOPORT_READ, port as u64, 1) };
        if rc >= 0 {
            rc as u8
        } else {
            0
        }
    }

    fn write_u8(&mut self, port: u16, value: u8) {
        let _ =
            unsafe { syscall::syscall3(syscall::SYS_IOPORT_WRITE, port as u64, value as u64, 1) };
    }
}

#[cfg(target_os = "none")]
fn boot_log(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_WRITE,
            1,
            bytes.as_ptr() as u64,
            bytes.len() as u64,
        );
    }
}

#[cfg(target_os = "none")]
fn exit_failed() -> ! {
    unsafe {
        let _ = syscall::syscall1(syscall::SYS_EXIT, 127);
        let _ = syscall::syscall1(syscall::SYS_EXIT_GROUP, 127);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_os = "none")]
fn sleep_ms(ms: u64) {
    let ts = Timespec {
        tv_sec: (ms / 1000) as i64,
        tv_nsec: ((ms % 1000) * 1_000_000) as i64,
    };
    let _ = unsafe { syscall::syscall2(syscall::SYS_NANOSLEEP, &ts as *const _ as u64, 0) };
}

#[cfg(target_os = "none")]
fn service_endpoint() -> Option<u64> {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) };
    if pid <= 0 {
        None
    } else {
        Some(((pid as u64) << 32) | syscall::PS2_DRIVER_IRQ_CHANNEL)
    }
}

#[cfg(target_os = "none")]
fn register_service_endpoint(endpoint: u64) -> bool {
    let name = b"ps2_driver";
    let rc = unsafe {
        syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            endpoint,
        )
    };
    rc >= 0
}

#[cfg(target_os = "none")]
fn register_keyboard_irq() -> i64 {
    let endpoint_lo = syscall::PS2_DRIVER_IRQ_CHANNEL << 32;
    unsafe {
        syscall::syscall6(
            syscall::SYS_IRQ_REGISTER,
            IRQ_KEYBOARD_LINE,
            endpoint_lo,
            0,
            IRQ_SOURCE_IOAPIC_EDGE,
            0,
            0,
        )
    }
}

#[cfg(target_os = "none")]
fn ack_keyboard_irq(reg_id: u64, wave_gen: u64) {
    let _ = unsafe {
        syscall::syscall5(
            syscall::SYS_IRQ_ACK,
            IRQ_KEYBOARD_VECTOR,
            reg_id,
            0,
            wave_gen,
            IRQ_ACK_HANDLED,
        )
    };
}

#[cfg(target_os = "none")]
fn recv_irq_notification(endpoint: u64, buf: &mut [u8; 9]) -> Option<u64> {
    let rc = unsafe {
        syscall::syscall4(
            syscall::SYS_IPC_RECV,
            endpoint,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            syscall::IPC_FLAG_TIMEOUT | IRQ_RECV_TIMEOUT_MS,
        )
    };
    if rc < 9 || buf[0] as u64 != IRQ_KEYBOARD_VECTOR {
        return None;
    }
    Some(u64::from_le_bytes([
        buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7], buf[8],
    ]))
}

#[cfg(target_os = "none")]
fn modifier_bits(event: &InputEvent) -> u8 {
    let mut bits = 0u8;
    if event.modifiers.shift {
        bits |= syscall::INPUT_MOD_SHIFT;
    }
    if event.modifiers.ctrl {
        bits |= syscall::INPUT_MOD_CTRL;
    }
    if event.modifiers.alt {
        bits |= syscall::INPUT_MOD_ALT;
    }
    if event.modifiers.meta {
        bits |= syscall::INPUT_MOD_META;
    }
    bits
}

#[cfg(target_os = "none")]
fn event_to_wire(event: InputEvent) -> syscall::InputEventWire {
    syscall::InputEventWire {
        device: match event.device {
            InputDevice::Keyboard => syscall::INPUT_DEVICE_KEYBOARD,
            InputDevice::Mouse => syscall::INPUT_DEVICE_MOUSE,
        },
        state: if event.value == 0 {
            syscall::INPUT_KEY_RELEASED
        } else {
            syscall::INPUT_KEY_PRESSED
        },
        code: event.code,
        value: event.value,
        ascii: event.ascii,
        modifiers: modifier_bits(&event),
        _pad: [0; 4],
    }
}

#[cfg(target_os = "none")]
fn push_input_event(event: InputEvent) {
    let req = syscall::InputRequest {
        sender_pid: 0,
        msg_type: syscall::INPUT_MSG_PUSH,
        reply_endpoint: 0,
        event: event_to_wire(event),
    };
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            syscall::INPUT_SERVER_ENDPOINT,
            &req as *const syscall::InputRequest as u64,
            core::mem::size_of::<syscall::InputRequest>() as u64,
            0,
            0,
            0,
        )
    };
}

#[cfg(target_os = "none")]
fn drain_controller(controller: &mut I8042<SyscallPorts>, keyboard: &mut Ps2Keyboard) -> bool {
    let mut produced = false;
    let mut drained = 0usize;
    while drained < IRQ_DRAIN_LIMIT {
        let Some(byte) = controller.poll_byte() else {
            break;
        };
        if let Some(event) = keyboard.feed(byte) {
            push_input_event(event);
            produced = true;
        }
        drained += 1;
    }
    produced
}

#[cfg(target_os = "none")]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    let Some(endpoint) = service_endpoint() else {
        exit_failed();
    };
    if !register_service_endpoint(endpoint) {
        boot_log(b"ps2_driver: register failed\n");
        exit_failed();
    }
    boot_log(b"ps2_driver: registered\n");

    let irq_reg_id = register_keyboard_irq();
    if irq_reg_id < 0 {
        boot_log(b"ps2_driver: irq register failed\n");
    } else {
        boot_log(b"ps2_driver: irq registered\n");
    }

    let mut controller = I8042::new(SyscallPorts);
    let mut keyboard = if controller.init().is_ok() {
        boot_log(b"ps2_driver: i8042 set2 ready\n");
        Ps2Keyboard::new()
    } else {
        boot_log(b"ps2_driver: i8042 init timeout, using translated set1\n");
        Ps2Keyboard::new_set1()
    };

    let mut irq_buf = [0u8; 9];
    loop {
        let wave = recv_irq_notification(endpoint, &mut irq_buf);
        let produced = drain_controller(&mut controller, &mut keyboard);
        if let (Some(wave_gen), true) = (wave, irq_reg_id >= 0) {
            ack_keyboard_irq(irq_reg_id as u64, wave_gen);
        }
        if !produced {
            sleep_ms(1);
        }
    }
}

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
