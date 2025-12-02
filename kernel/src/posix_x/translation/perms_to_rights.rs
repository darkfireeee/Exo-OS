//! Permission to Rights Translation
//!
//! Converts POSIX permissions (chmod/chown) to Exo-OS capability rights

// Placeholder Right type (string-based)
pub type Right = alloc::string::String;

/// POSIX permission bits
pub const S_IRUSR: u32 = 0o400; // User read
pub const S_IWUSR: u32 = 0o200; // User write  
pub const S_IXUSR: u32 = 0o100; // User execute
pub const S_IRGRP: u32 = 0o040; // Group read
pub const S_IWGRP: u32 = 0o020; // Group write
pub const S_IXGRP: u32 = 0o010; // Group execute
pub const S_IROTH: u32 = 0o004; // Other read
pub const S_IWOTH: u32 = 0o002; // Other write
pub const S_IXOTH: u32 = 0o001; // Other execute

/// File type bits
pub const S_IFMT: u32 = 0o170000;   // File type mask
pub const S_IFSOCK: u32 = 0o140000; // Socket
pub const S_IFLNK: u32 = 0o120000;  // Symbolic link
pub const S_IFREG: u32 = 0o100000;  // Regular file
pub const S_IFBLK: u32 = 0o060000;  // Block device
pub const S_IFDIR: u32 = 0o040000;  // Directory
pub const S_IFCHR: u32 = 0o020000;  // Character device
pub const S_IFIFO: u32 = 0o010000;  // FIFO

/// Special bits
pub const S_ISUID: u32 = 0o4000; // Set UID
pub const S_ISGID: u32 = 0o2000; // Set GID
pub const S_ISVTX: u32 = 0o1000; // Sticky bit

/// Convert POSIX mode to capability rights
pub fn mode_to_rights(mode: u32) -> alloc::vec::Vec<Right> {
    use alloc::string::String;
    let mut rights = alloc::vec::Vec::new();
    
    // User permissions
    if mode & S_IRUSR != 0 {
        rights.push(String::from("read"));
    }
    if mode & S_IWUSR != 0 {
        rights.push(String::from("write"));
    }
    if mode & S_IXUSR != 0 {
        rights.push(String::from("execute"));
    }
    
    // Group permissions (Exo-OS handles differently)
    if mode & S_IRGRP != 0 {
        rights.push(String::from("share_read"));
    }
    if mode & S_IWGRP != 0 {
        rights.push(String::from("share_write"));
    }
    
    rights
}

/// Convert capability rights to POSIX mode
pub fn rights_to_mode(rights: &[Right]) -> u32 {
    let mut mode: u32 = 0;
    
    for right in rights {
        match right.as_str() {
            "read" => mode |= S_IRUSR | S_IRGRP | S_IROTH,
            "write" => mode |= S_IWUSR | S_IWGRP | S_IWOTH,
            "execute" => mode |= S_IXUSR | S_IXGRP | S_IXOTH,
            "share_read" => mode |= S_IRGRP,
            "share_write" => mode |= S_IWGRP,
            _ => {}
        }
    }
    
    mode
}

/// Extract file type from mode
pub fn get_file_type(mode: u32) -> u32 {
    mode & S_IFMT
}

/// Check if mode represents a directory
pub fn is_directory(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFDIR
}

/// Check if mode represents a regular file
pub fn is_regular_file(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFREG
}

/// Check if mode represents a symbolic link
pub fn is_symlink(mode: u32) -> bool {
    (mode & S_IFMT) == S_IFLNK
}

/// Create default file mode (0644)
pub fn default_file_mode() -> u32 {
    S_IFREG | S_IRUSR | S_IWUSR | S_IRGRP | S_IROTH
}

/// Create default directory mode (0755)
pub fn default_dir_mode() -> u32 {
    S_IFDIR | S_IRUSR | S_IWUSR | S_IXUSR | S_IRGRP | S_IXGRP | S_IROTH | S_IXOTH
}
