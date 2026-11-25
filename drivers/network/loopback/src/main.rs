#![no_std]
#![no_main]
#![allow(dead_code, static_mut_refs)]


use exo_syscall_abi as syscall;

mod echo;
pub mod grant_loop;
mod state;

use syscall::{
    DriverCtrlMsg as LoopbackRequest, DriverInitMsg, MacReplyMsg, RxPacketRef, RxReadyMsg,
    RxReleaseMsg, TxCompleteMsg, TxSubmitMsg, NET_CTRL_DRIVER_INIT, NET_CTRL_MAC_QUERY,
    NET_CTRL_MAC_REPLY, NET_CTRL_RX_READY, NET_CTRL_RX_RELEASE, NET_CTRL_TX_COMPLETE,
    NET_CTRL_TX_SUBMIT,
};

const SERVER_ENDPOINT_ID: u64 = 15;
const NETWORK_ENDPOINT_ID: u64 = 7;
const LOOPBACK_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];

static mut RX_POOL_COUNT: u32 = 256;
static mut NEXT_RX_IDX: u16 = 0;

impl state::LoopbackState {
    fn process_request(&mut self, request: &LoopbackRequest) {
        if request.msg_type == NET_CTRL_DRIVER_INIT {
            let msg = unsafe {
                core::ptr::read_unaligned(request.payload.as_ptr() as *const DriverInitMsg)
            };
            if msg.opcode == NET_CTRL_DRIVER_INIT && msg.pool_count > 0 {
                unsafe {
                    RX_POOL_COUNT = msg.pool_count;
                    NEXT_RX_IDX = 0;
                }
            }
            send_mac_reply();
        } else if request.msg_type == NET_CTRL_MAC_QUERY {
            send_mac_reply();
        } else if request.msg_type == NET_CTRL_RX_RELEASE {
            let msg = unsafe {
                core::ptr::read_unaligned(request.payload.as_ptr() as *const RxReleaseMsg)
            };
            self.note_release(msg.count);
        } else if request.msg_type == NET_CTRL_TX_SUBMIT {
            let msg = unsafe {
                core::ptr::read_unaligned(request.payload.as_ptr() as *const TxSubmitMsg)
            };
            if msg.opcode == NET_CTRL_TX_SUBMIT {
                // Loopback echo accounting & RX ready notification before TX completion.
                self.note_echo();
                let rx_idx = unsafe {
                    let cur = NEXT_RX_IDX;
                    let max = RX_POOL_COUNT.max(1) as u16;
                    NEXT_RX_IDX = cur.wrapping_add(1) % max;
                    cur
                };
                if msg.len > 0 {
                    send_single_rx_ready(rx_idx, msg.len);
                }
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

fn send_mac_reply() {
    let reply = MacReplyMsg {
        opcode: NET_CTRL_MAC_REPLY,
        mac: LOOPBACK_MAC,
        _pad: [0; 2],
    };
    let payload = unsafe {
        core::slice::from_raw_parts(
            &reply as *const MacReplyMsg as *const u8,
            core::mem::size_of::<MacReplyMsg>(),
        )
    };
    send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_MAC_REPLY, payload);
}

fn send_single_rx_ready(pool_idx: u16, len: u16) {
    let mut ready = RxReadyMsg {
        opcode: NET_CTRL_RX_READY,
        count: 1,
        entries: [RxPacketRef { pool_idx: 0, len: 0 }; 20],
    };
    ready.entries[0] = RxPacketRef { pool_idx, len };
    let payload = unsafe {
        core::slice::from_raw_parts(
            &ready as *const RxReadyMsg as *const u8,
            core::mem::size_of::<RxReadyMsg>(),
        )
    };
    send_ctrl(NETWORK_ENDPOINT_ID, NET_CTRL_RX_READY, payload);
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

exo_syscall_abi::ring1_nostd_panic_handler!(b"");
