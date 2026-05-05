use exosh::parser;
use std::env;
use std::io::{self, Write};
use std::process::Command;

fn main() {
    let mut cwd = env::current_dir().unwrap_or_else(|_| "/".into());
    loop {
        print!("exo:{}$ ", cwd.display());
        let _ = io::stdout().flush();
        let mut line = String::new();
        if io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let parsed = match parser::parse(&line) {
            Ok(parsed) => parsed,
            Err(parser::ParseError::Empty) => continue,
            Err(err) => {
                eprintln!("exosh: parse error: {err:?}");
                continue;
            }
        };
        match parsed.argv[0].as_str() {
            "cd" => {
                let target = parsed.argv.get(1).map(String::as_str).unwrap_or("/");
                if let Err(err) = env::set_current_dir(target) {
                    eprintln!("cd: {err}");
                } else {
                    cwd = env::current_dir().unwrap_or_else(|_| "/".into());
                }
            }
            "pwd" => println!("{}", cwd.display()),
            "exit" => break,
            cmd => {
                let status = Command::new(cmd).args(&parsed.argv[1..]).status();
                match status {
                    Ok(status) if status.success() => {}
                    Ok(status) => eprintln!("{cmd}: exit {status}"),
                    Err(err) => eprintln!("{cmd}: {err}"),
                }
            }
        }
    }
}
