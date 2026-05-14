#![no_std]
#![no_main]

use core::panic::PanicInfo;

use exo_syscall_abi as syscall;

const SERVER_ENDPOINT_ID: u64 = 15;
const NET_CTRL_RX_RELEASE: u32 = 0x4F01;

#[repr(C)]
struct LoopbackRequest {
    sender_pid: u32,
    msg_type: u32,
    payload: [u8; syscall::IPC_INLINE_PAYLOAD_SIZE],
}

impl LoopbackRequest {
    const fn zeroed() -> Self {
        Self {
            sender_pid: 0,
            msg_type: 0,
            payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RxReleaseMsg {
    opcode: u32,
    count: u32,
    pool_idx: [u16; 20],
}

const _: () = assert!(core::mem::size_of::<RxReleaseMsg>() == 48);

struct LoopbackState {
    rx_released: u64,
    tx_echoed: u64,
}

impl LoopbackState {
    const fn new() -> Self {
        Self {
            rx_released: 0,
            tx_echoed: 0,
        }
    }

    fn process(&mut self, request: &LoopbackRequest) {
        if request.msg_type == NET_CTRL_RX_RELEASE {
            let msg = unsafe {
                core::ptr::read_unaligned(request.payload.as_ptr() as *const RxReleaseMsg)
            };
            self.rx_released = self.rx_released.saturating_add(msg.count as u64);
        } else {
            self.tx_echoed = self.tx_echoed.saturating_add(1);
        }
    }
}

static mut LOOPBACK: LoopbackState = LoopbackState::new();

#[no_mangle]
pub extern "C" fn _start() -> ! {
    register_endpoint();
    let mut request = LoopbackRequest::zeroed();
    loop {
        let rc = unsafe {
            syscall::syscall4(
                syscall::SYS_IPC_RECV,
                SERVER_ENDPOINT_ID,
                &mut request as *mut LoopbackRequest as u64,
                core::mem::size_of::<LoopbackRequest>() as u64,
                syscall::IPC_FLAG_TIMEOUT | 100,
            )
        };
        if rc > 0 {
            unsafe {
                LOOPBACK.process(&request);
            }
        }
    }
}

fn register_endpoint() {
    let name = b"loopback_net";
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
