#![no_std]
#![no_main]
#![allow(dead_code, static_mut_refs)]

use core::panic::PanicInfo;

use exo_syscall_abi as syscall;

mod echo;
mod state;

const SERVER_ENDPOINT_ID: u64 = 15;
const NETWORK_ENDPOINT_ID: u64 = 7;
const NET_CTRL_RX_RELEASE: u32 = 0x4F01;
const NET_CTRL_TX_SUBMIT: u32 = 0x4F05;
const NET_CTRL_TX_COMPLETE: u32 = 0x4F06;

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

#[repr(C)]
#[derive(Clone, Copy)]
struct TxSubmitMsg {
    opcode: u32,
    pool_idx: u16,
    len: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxCompleteMsg {
    opcode: u32,
    count: u32,
    pool_idx: [u16; 20],
}

impl state::LoopbackState {
    fn process_request(&mut self, request: &LoopbackRequest) {
        if request.msg_type == NET_CTRL_RX_RELEASE {
            let msg = unsafe {
                core::ptr::read_unaligned(request.payload.as_ptr() as *const RxReleaseMsg)
            };
            self.note_release(msg.count);
        } else if request.msg_type == NET_CTRL_TX_SUBMIT {
            let msg = unsafe {
                core::ptr::read_unaligned(request.payload.as_ptr() as *const TxSubmitMsg)
            };
            if msg.opcode == NET_CTRL_TX_SUBMIT {
                send_single_tx_complete(msg.pool_idx);
            }
        } else {
            self.note_echo();
        }
    }
}

static mut LOOPBACK: state::LoopbackState = state::LoopbackState::new();

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
                LOOPBACK.process_request(&request);
            }
        }
    }
}

fn register_endpoint() {
    register_endpoint_name(b"loopback_driver");
    register_endpoint_name(b"loopback_net");
}

fn register_endpoint_name(name: &[u8]) {
    unsafe {
        let _ = syscall::syscall3(
            syscall::SYS_IPC_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            SERVER_ENDPOINT_ID,
        );
    }
}

fn send_single_tx_complete(pool_idx: u16) {
    let mut complete = TxCompleteMsg {
        opcode: NET_CTRL_TX_COMPLETE,
        count: 1,
        pool_idx: [0; 20],
    };
    complete.pool_idx[0] = pool_idx;
    let payload = unsafe {
        core::slice::from_raw_parts(
            &complete as *const TxCompleteMsg as *const u8,
            core::mem::size_of::<TxCompleteMsg>(),
        )
    };
    send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_TX_COMPLETE, payload);
}

fn send_ctrl(endpoint: u64, msg_type: u32, payload: &[u8]) {
    let pid = unsafe { syscall::syscall0(syscall::SYS_GETPID) }.max(0) as u32;
    let mut msg = LoopbackRequest {
        sender_pid: pid,
        msg_type,
        payload: [0; syscall::IPC_INLINE_PAYLOAD_SIZE],
    };
    let n = payload.len().min(msg.payload.len());
    msg.payload[..n].copy_from_slice(&payload[..n]);
    let _ = unsafe {
        syscall::syscall6(
            syscall::SYS_IPC_SEND,
            endpoint,
            &msg as *const LoopbackRequest as u64,
            core::mem::size_of::<LoopbackRequest>() as u64,
            syscall::IPC_FLAG_INJECT_SRC_PID,
            0,
            0,
        )
    };
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe {
            core::arch::asm!("hlt", options(nostack, nomem));
        }
    }
}
