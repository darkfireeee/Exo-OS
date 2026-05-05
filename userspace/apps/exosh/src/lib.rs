pub mod parser;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Builtin {
    Cd,
    Pwd,
    Exit,
}

impl Builtin {
    pub fn from_command(cmd: &str) -> Option<Self> {
        match cmd {
            "cd" => Some(Self::Cd),
            "pwd" => Some(Self::Pwd),
            "exit" => Some(Self::Exit),
            _ => None,
        }
    }
}
