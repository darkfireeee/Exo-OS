use std::path::Path;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("cat: missing operand");
        std::process::exit(1);
    };
    if let Err(err) = exo_coreutils::cat(Path::new(&path), &mut std::io::stdout()) {
        eprintln!("cat: {err}");
        std::process::exit(1);
    }
}
