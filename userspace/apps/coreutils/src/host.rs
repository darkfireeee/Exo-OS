//! Implémentations compatibles environnement hôte (avec std)

    use std::fs::{self, File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::path::Path;

    pub fn ls(path: &Path, out: &mut dyn Write) -> io::Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            writeln!(out, "{}", entry.file_name().to_string_lossy())?;
        }
        Ok(())
    }

    pub fn mkdir(path: &Path) -> io::Result<()> {
        fs::create_dir(path)
    }

    pub fn rm(path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    pub fn rmdir(path: &Path) -> io::Result<()> {
        fs::remove_dir(path)
    }

    pub fn touch(path: &Path) -> io::Result<()> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map(|_| ())
    }

    pub fn cat(path: &Path, out: &mut dyn Write) -> io::Result<()> {
        let mut file = File::open(path)?;
        let mut buf = [0u8; 4096];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            out.write_all(&buf[..n])?;
        }
    }

    pub fn echo(args: &[String], out: &mut dyn Write) -> io::Result<()> {
        for (idx, arg) in args.iter().enumerate() {
            if idx != 0 {
                write!(out, " ")?;
            }
            write!(out, "{arg}")?;
        }
        writeln!(out)
    }

    pub fn host_main(command: &str) -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let result = match command {
            "cat" => args.first().map(|p| cat(Path::new(p), &mut std::io::stdout())).unwrap_or_else(|| {
                eprintln!("cat: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "echo" => echo(&args, &mut std::io::stdout()),
            "ls" => ls(Path::new(args.first().map(String::as_str).unwrap_or(".")), &mut std::io::stdout()),
            "mkdir" => args.first().map(|p| mkdir(Path::new(p))).unwrap_or_else(|| {
                eprintln!("mkdir: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "rm" => args.first().map(|p| rm(Path::new(p))).unwrap_or_else(|| {
                eprintln!("rm: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "rmdir" => args.first().map(|p| rmdir(Path::new(p))).unwrap_or_else(|| {
                eprintln!("rmdir: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            "touch" => args.first().map(|p| touch(Path::new(p))).unwrap_or_else(|| {
                eprintln!("touch: missing operand");
                Err(io::Error::from(io::ErrorKind::InvalidInput))
            }),
            _ => Ok(()),
        };
        if let Err(err) = result {
            eprintln!("{command}: {err}");
            1
        } else {
            0
        }
    }
