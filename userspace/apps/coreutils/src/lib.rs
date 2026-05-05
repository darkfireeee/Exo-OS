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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmpdir() -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        path.push(format!("exo-coreutils-{nonce}"));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn touch_cat_ls_rm_roundtrip() {
        let dir = tmpdir();
        let file = dir.join("a");
        touch(&file).unwrap();
        fs::write(&file, b"hi").unwrap();
        let mut cat_out = Vec::new();
        cat(&file, &mut cat_out).unwrap();
        assert_eq!(cat_out, b"hi");
        let mut ls_out = Vec::new();
        ls(&dir, &mut ls_out).unwrap();
        assert_eq!(String::from_utf8(ls_out).unwrap(), "a\n");
        rm(&file).unwrap();
        fs::remove_dir(&dir).unwrap();
    }
}
