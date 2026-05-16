use exo_syscall_abi as abi;

#[test]
fn syscall_contract_standard_exofs_and_core_numbers() {
    assert_eq!(abi::SYS_READ, 0);
    assert_eq!(abi::SYS_WRITE, 1);
    assert_eq!(abi::SYS_OPEN, 2);
    assert_eq!(abi::SYS_GETPID, 39);
    assert_eq!(abi::SYS_SYNC_FILE_RANGE, 277);
    assert_eq!(abi::SYS_EPOLL_CREATE1, 291);
    assert_eq!(abi::SYS_DUP3, 292);
    assert_eq!(abi::SYS_PIPE2, 293);
    assert_eq!(abi::SYS_GETCPU, 309);
    assert_eq!(abi::SYS_RENAMEAT2, 316);
    assert_eq!(abi::SYS_GETRANDOM, 318);
    assert_eq!(abi::SYS_COPY_FILE_RANGE, 326);
    assert_eq!(abi::SYS_STATX, 332);
    assert_eq!(abi::SYS_OPENAT2, 437);

    assert_eq!(abi::SYS_EXO_IPC_SEND, 300);
    assert_eq!(abi::SYS_EXO_MEM_SHARE, 310);
    assert_eq!(abi::SYS_EXO_MEM_MAP_PID, 312);
    assert_eq!(abi::SYS_EXO_MEM_MPROTECT_PID, 314);
    assert_eq!(abi::SYS_EXO_CAP_CHECK, 323);
    assert_eq!(abi::SYS_EXO_LOG, 350);
    assert_eq!(abi::SYS_EXO_PROCESS_LIST, 351);
    assert_eq!(abi::SYS_EXO_PHOENIX_STATE_SET, 352);

    assert_eq!(abi::SYS_EXOFS_FIRST, 500);
    assert_eq!(abi::SYS_EXOFS_PATH_RESOLVE, 500);
    assert_eq!(abi::SYS_EXOFS_OBJECT_OPEN, 501);
    assert_eq!(abi::SYS_EXOFS_OBJECT_READ, 502);
    assert_eq!(abi::SYS_EXOFS_OBJECT_WRITE, 503);
    assert_eq!(abi::SYS_EXOFS_EPOCH_COMMIT, 518);
    assert_eq!(abi::SYS_EXOFS_OPEN_BY_PATH, 519);
    assert_eq!(abi::SYS_EXOFS_READDIR, 520);
    assert_eq!(abi::SYS_EXOFS_LAST, 520);
    assert_eq!(abi::SYS_EXOFS_COUNT, 21);

    assert_eq!(abi::SYS_IRQ_REGISTER, 530);
    assert_eq!(abi::SYS_PCI_SET_TOPOLOGY, 546);

    assert_eq!(abi::SYS_IPC_REGISTER, abi::SYS_EXO_IPC_CREATE);
    assert_eq!(abi::SYS_PROC_CLONE, abi::SYS_FORK);
    assert_eq!(abi::SYS_PROC_EXEC, abi::SYS_EXECVE);
}

#[test]
fn syscall_contract_standard_exofs_layouts_and_rights() {
    assert_eq!(core::mem::size_of::<abi::ExofsPathResolveResult>(), 104);
    assert_eq!(core::mem::size_of::<abi::ExofsOpenArgs>(), 48);
    assert_eq!(core::mem::size_of::<abi::ExoProcessInfo>(), 48);

    let mut resolved = abi::ExofsPathResolveResult::default();
    resolved.blob_id[..8].copy_from_slice(&0x0123_4567_89AB_CDEFu64.to_le_bytes());
    assert_eq!(resolved.blob_id_low64(), 0x0123_4567_89AB_CDEF);

    assert_ne!(abi::EXOFS_RIGHT_READ & abi::EXOFS_RIGHT_READ_ONLY, 0);
    assert_ne!(abi::EXOFS_RIGHT_LIST & abi::EXOFS_RIGHT_READ_ONLY, 0);
    assert_ne!(abi::EXOFS_RIGHT_WRITE & abi::EXOFS_RIGHT_READ_WRITE, 0);
    assert_eq!(abi::EXOFS_RIGHT_ALL, 0x0000_FFFF);
}
