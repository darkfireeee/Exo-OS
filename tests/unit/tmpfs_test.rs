//! Tests unitaires pour tmpfs

#[cfg(test)]
mod tests {
    use crate::fs::vfs::inode::{Inode, InodeType};
    use crate::fs::vfs::tmpfs::{TmpFs, TmpfsInode};

    #[test]
    fn test_tmpfs_create() {
        let tmpfs = TmpFs::new();
        let root = tmpfs.get_inode(1).expect("Root inode should exist");
        assert_eq!(root.read().ino(), 1);
        assert_eq!(root.read().inode_type(), InodeType::Directory);
    }

    #[test]
    fn test_tmpfs_create_file() {
        let tmpfs = TmpFs::new();
        let file_inode = tmpfs.create_inode(InodeType::File);

        let inode = file_inode.read();
        assert_eq!(inode.inode_type(), InodeType::File);
        assert_eq!(inode.size(), 0);
    }

    #[test]
    fn test_tmpfs_read_write() {
        let tmpfs = TmpFs::new();
        let file_inode = tmpfs.create_inode(InodeType::File);

        let data = b"Hello, tmpfs!";
        let mut inode = file_inode.write();

        // Write
        let written = inode.write_at(0, data).expect("Write should succeed");
        assert_eq!(written, data.len());
        assert_eq!(inode.size(), data.len() as u64);

        // Read
        let mut buf = vec![0u8; data.len()];
        let read = inode.read_at(0, &mut buf).expect("Read should succeed");
        assert_eq!(read, data.len());
        assert_eq!(&buf, data);
    }

    #[test]
    fn test_tmpfs_directory_ops() {
        let tmpfs = TmpFs::new();
        let root = tmpfs.get_inode(1).expect("Root should exist");

        // Create entry
        let new_ino = root
            .write()
            .create("test.txt", InodeType::File)
            .expect("Create should succeed");

        // Lookup
        let found_ino = root
            .read()
            .lookup("test.txt")
            .expect("Lookup should succeed");
        assert_eq!(found_ino, new_ino);

        // List
        let entries = root.read().list().expect("List should succeed");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], "test.txt");

        // Remove
        root.write()
            .remove("test.txt")
            .expect("Remove should succeed");
        let entries = root.read().list().expect("List should succeed");
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_tmpfs_zero_copy() {
        let tmpfs = TmpFs::new();
        let file_inode = tmpfs.create_inode(InodeType::File);

        // Write large data
        let data = vec![0x42u8; 4096];
        let mut inode = file_inode.write();
        let written = inode.write_at(0, &data).expect("Write should succeed");
        assert_eq!(written, 4096);

        // Read should be zero-copy
        let mut buf = vec![0u8; 4096];
        let read = inode.read_at(0, &mut buf).expect("Read should succeed");
        assert_eq!(read, 4096);
        assert_eq!(buf, data);
    }
}
