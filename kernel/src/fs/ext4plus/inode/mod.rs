// kernel/src/fs/ext4plus/inode/mod.rs
//
// Sous-module inode EXT4+ : extent tree, xattr, ACL, ops.

pub mod extent;
pub mod xattr;
pub mod acl;
pub mod ops;

pub use extent::{
    Ext4ExtentHeader, Ext4ExtentIdx, Ext4ExtentLeaf, ExtentResult,
    ext4_find_extent, ext4_insert_extent_inline,
    EXT4_EXT_MAGIC, EXTENT_STATS,
};
pub use xattr::{
    XattrNamespace, XattrEntry, XattrCache, XATTR_CACHE, XATTR_STATS,
    parse_xattr_block, xattr_get, xattr_set, xattr_remove, EXT4_XATTR_MAGIC,
};
pub use acl::{
    AclEntry, acl_check, parse_acl, mode_to_acl, acl_to_mode,
    acl_access_from_xattrs, acl_default_from_xattrs, ACL_STATS,
    ACL_USER_OBJ, ACL_USER, ACL_GROUP_OBJ, ACL_GROUP, ACL_MASK, ACL_OTHER,
    ACL_PERM_READ, ACL_PERM_WRITE, ACL_PERM_EXECUTE,
};
pub use ops::{
    Ext4InodeDisk, Ext4InodeOps, INODE_OPS_STATS,
    EXT4_INODE_FLAG_EXTENTS, EXT4_INODE_FLAG_INLINE,
};
