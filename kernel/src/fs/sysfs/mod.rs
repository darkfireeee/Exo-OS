//! SysFS - System Filesystem
//! 
//! Expose kernel subsystems and device hierarchy.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// SysFS attribute
pub struct SysAttr {
    pub name: String,
    pub value: String,
}

/// SysFS directory
pub struct SysDir {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
    pub subdirs: BTreeMap<String, SysDir>,
}

impl SysDir {
    pub fn new(name: &str) -> Self {
        Self {
            name: String::from(name),
            attrs: BTreeMap::new(),
            subdirs: BTreeMap::new(),
        }
    }
    
    pub fn add_attr(&mut self, name: &str, value: &str) {
        self.attrs.insert(String::from(name), String::from(value));
    }
    
    pub fn add_subdir(&mut self, name: &str) -> &mut SysDir {
        self.subdirs.insert(
            String::from(name),
            SysDir::new(name)
        );
        self.subdirs.get_mut(name).unwrap()
    }
}

/// SysFS root
pub struct SysFs {
    root: SysDir,
}

impl SysFs {
    pub fn new() -> Self {
        let mut root = SysDir::new("/");
        
        // Create standard structure
        let devices = root.add_subdir("devices");
        devices.add_attr("count", "0");
        
        let block = root.add_subdir("block");
        block.add_attr("count", "0");
        
        let class = root.add_subdir("class");
        class.add_attr("count", "0");
        
        Self { root }
    }
    
    pub fn read_attr(&self, path: &str) -> Result<String, &'static str> {
        // TODO: Parse path and lookup attribute
        Ok(String::from("0"))
    }
}
