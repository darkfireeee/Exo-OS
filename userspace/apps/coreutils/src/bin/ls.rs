use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    if let Err(err) = exo_coreutils::ls(Path::new(&path), &mut std::io::stdout()) {
        eprintln!("ls: {err}");
        std::process::exit(1);
    }
}
