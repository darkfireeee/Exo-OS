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

#[test]
fn accepted_socket_endpoints_and_options_work_honestly() {
    const OWNER: u32 = 21;
    const SOL_SOCKET: i32 = 1;
    const SO_REUSEADDR: i32 = 2;
    const SO_TYPE: i32 = 3;
    const SO_ERROR: i32 = 4;
    const IPPROTO_TCP: i32 = 6;
    const TCP_NODELAY: i32 = 1;
    const ENOPROTOOPT: i64 = -92;

    let mut sockets = SocketTable::new();
    let listener = sockets.open(OWNER, SocketKind::Tcp).expect("listener open");
    sockets
        .bind(OWNER, listener.handle, 0x7f00_0001, 8080)
        .expect("listener bind");
    sockets
        .listen(OWNER, listener.handle, 8)
        .expect("listener listen");

    // Accepted child keeps its own generation & updated peer endpoints in place
    let child = sockets
        .open_accepted(OWNER, 0x7f00_0001, 8080, 0x7f00_0001, 51234)
        .expect("accepted child");
    assert!(child.state == SocketState::Connected);
    assert_eq!(child.local_addr, 0x7f00_0001);
    assert_eq!(child.local_port, 8080);
    assert_eq!(child.remote_port, 51234);

    let updated = sockets
        .update_endpoints(OWNER, child.handle, 0x7f00_0001, 8080, 0x7f00_0001, 51235)
        .expect("update endpoints");
    assert_eq!(updated.handle, child.handle);
    assert_eq!(updated.remote_port, 51235);

    // Real option storage + ENOPROTOOPT on unknown options
    let one = 1i32.to_ne_bytes();
    sockets
        .set_option(OWNER, child.handle, IPPROTO_TCP, TCP_NODELAY, &one)
        .expect("set TCP_NODELAY");
    let (_, raw, len) = sockets
        .get_option(OWNER, child.handle, IPPROTO_TCP, TCP_NODELAY)
        .expect("get TCP_NODELAY");
    assert_eq!(len, 4);
    assert_eq!(i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]), 1);

    let (_, raw, len) = sockets
        .get_option(OWNER, child.handle, SOL_SOCKET, SO_TYPE)
        .expect("get SO_TYPE");
    assert_eq!(len, 4);
    assert_eq!(i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]), 1);

    assert_eq!(
        sockets
            .get_option(OWNER, child.handle, SOL_SOCKET, 9999)
            .err(),
        Some(ENOPROTOOPT)
    );

    // Failed connect surfaces via SO_ERROR once and clears on read
    let cli = sockets.open(OWNER, SocketKind::Tcp).expect("cli open");
    sockets
        .connect_at_tick(OWNER, cli.handle, 0x7f00_0001, 9999, 10)
        .expect("connect");
    sockets.fail_tcp_connect_by_handle(cli.handle, 111 /* ECONNREFUSED */);
    let (_, raw, _) = sockets
        .get_option(OWNER, cli.handle, SOL_SOCKET, SO_ERROR)
        .expect("read SO_ERROR");
    assert_eq!(i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]), 111);
    let (_, raw, _) = sockets
        .get_option(OWNER, cli.handle, SOL_SOCKET, SO_ERROR)
        .expect("SO_ERROR cleared");
    assert_eq!(i32::from_ne_bytes([raw[0], raw[1], raw[2], raw[3]]), 0);

    // Active listener cannot be shadowed even with SO_REUSEADDR; once listener closes and only
    // a shutdown accepted socket remains on port 8080, SO_REUSEADDR allows rebinding.
    let s2 = sockets.open(OWNER, SocketKind::Tcp).expect("s2 open");
    assert_eq!(
        sockets.bind(OWNER, s2.handle, 0x7f00_0001, 8080).err(),
        Some(exo_syscall_abi::EADDRINUSE)
    );
    sockets.close(OWNER, listener.handle).expect("close listener");
    sockets
        .shutdown_how(OWNER, child.handle, 2)
        .expect("shutdown child");
    assert_eq!(
        sockets.bind(OWNER, s2.handle, 0x7f00_0001, 8080).err(),
        Some(exo_syscall_abi::EADDRINUSE)
    );
    sockets
        .set_option(OWNER, s2.handle, SOL_SOCKET, SO_REUSEADDR, &one)
        .expect("set SO_REUSEADDR");
    sockets
        .bind(OWNER, s2.handle, 0x7f00_0001, 8080)
        .expect("bind succeeds with SO_REUSEADDR over shutdown socket");
}
