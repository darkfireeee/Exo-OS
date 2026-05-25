#[allow(dead_code)]
#[path = "../src/socket_table.rs"]
mod socket_table;

use socket_table::{SocketKind, SocketState, SocketTable};

#[test]
fn tagged_socket_handles_keep_generation_separate_from_connect_lookup() {
    const OWNER: u32 = 13;

    let mut sockets = SocketTable::new();
    let opened = sockets.open(OWNER, SocketKind::Tcp).expect("socket open");
    let connected = sockets
        .connect(OWNER, opened.handle, 0x0a00_0202, 80)
        .expect("socket connect");
    assert!(connected.state == SocketState::Connecting);
    let established = sockets
        .complete_tcp_connect(OWNER, opened.handle)
        .expect("tcp handshake complete");
    assert!(established.state == SocketState::Connected);

    sockets
        .close(OWNER, opened.handle)
        .expect("socket close preserves first generation");
    let reopened = sockets.open(OWNER, SocketKind::Tcp).expect("socket reopen");

    assert_ne!(opened.handle, reopened.handle);
    assert_eq!(
        sockets.connect(OWNER, opened.handle, 0x0a00_0202, 80).err(),
        Some(exo_syscall_abi::EBADF)
    );
}

#[test]
fn raw_icmp_socket_connects_without_a_transport_port() {
    const OWNER: u32 = 13;

    let mut sockets = SocketTable::new();
    let opened = sockets.open(OWNER, SocketKind::Raw).expect("raw open");
    let connected = sockets
        .connect(OWNER, opened.handle, 0x0a00_0202, 0)
        .expect("raw icmp connect");

    assert!(connected.state == SocketState::Connected);
    assert_eq!(connected.remote_port, 0);
    assert_ne!(connected.local_port, 0);
}

#[test]
fn closed_raw_icmp_socket_releases_its_identifier_binding() {
    const OWNER: u32 = 13;
    const ICMP_IDENT: u16 = 0x4558;

    let mut sockets = SocketTable::new();
    let first = sockets.open(OWNER, SocketKind::Raw).expect("raw open");
    sockets
        .bind(OWNER, first.handle, 0, ICMP_IDENT)
        .expect("raw bind");
    sockets.close(OWNER, first.handle).expect("raw close");

    let second = sockets.open(OWNER, SocketKind::Raw).expect("raw reopen");
    sockets
        .bind(OWNER, second.handle, 0, ICMP_IDENT)
        .expect("identifier can be rebound after close");
}
