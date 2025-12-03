//! Parser de commandes shell
//! Supporte: commandes simples, arguments, redirections basiques

#[derive(Debug)]
pub struct Command<'a> {
    pub name: &'a str,
    pub args: [&'a str; 8],
    pub arg_count: usize,
}

impl<'a> Command<'a> {
    pub fn new(name: &'a str) -> Self {
        Command {
            name,
            args: [""; 8],
            arg_count: 0,
        }
    }
    
    pub fn add_arg(&mut self, arg: &'a str) -> Result<(), &'static str> {
        if self.arg_count >= 8 {
            return Err("Trop d'arguments (max 8)");
        }
        self.args[self.arg_count] = arg;
        self.arg_count += 1;
        Ok(())
    }
    
    pub fn get_args(&self) -> &[&'a str] {
        &self.args[..self.arg_count]
    }
}

/// Parse une ligne de commande
pub fn parse_command(line: &str) -> Result<Command, &'static str> {
    let line = line.trim();
    
    if line.is_empty() {
        return Err("Commande vide");
    }
    
    // Split par espaces (simple - pas de quotes pour l'instant)
    let parts: heapless::Vec<&str, 16> = line
        .split_whitespace()
        .collect();
    
    if parts.is_empty() {
        return Err("Commande vide");
    }
    
    let mut cmd = Command::new(parts[0]);
    
    for &part in &parts[1..] {
        cmd.add_arg(part)?;
    }
    
    Ok(cmd)
}

// Vec sans allocation
mod heapless {
    pub struct Vec<T, const N: usize> {
        data: [Option<T>; N],
        len: usize,
    }
    
    impl<T: Copy, const N: usize> Vec<T, N> {
        pub const fn new() -> Self {
            Vec {
                data: [None; N],
                len: 0,
            }
        }
        
        pub fn push(&mut self, value: T) -> Result<(), ()> {
            if self.len >= N {
                return Err(());
            }
            self.data[self.len] = Some(value);
            self.len += 1;
            Ok(())
        }
        
        pub fn is_empty(&self) -> bool {
            self.len == 0
        }
        
        pub fn get(&self, index: usize) -> Option<&T> {
            if index < self.len {
                self.data[index].as_ref()
            } else {
                None
            }
        }
    }
    
    impl<'a, const N: usize> FromIterator<&'a str> for Vec<&'a str, N> {
        fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
            let mut vec = Vec::new();
            for item in iter {
                if vec.push(item).is_err() {
                    break;
                }
            }
            vec
        }
    }
    
    impl<'a, const N: usize> core::ops::Index<usize> for Vec<&'a str, N> {
        type Output = &'a str;
        
        fn index(&self, index: usize) -> &Self::Output {
            self.data[index].as_ref().unwrap()
        }
    }
    
    impl<'a, const N: usize> core::ops::Deref for Vec<&'a str, N> {
        type Target = [&'a str];
        
        fn deref(&self) -> &Self::Target {
            unsafe {
                core::slice::from_raw_parts(
                    self.data.as_ptr() as *const &str,
                    self.len
                )
            }
        }
    }
}
