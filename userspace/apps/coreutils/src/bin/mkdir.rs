use std::path::Path;

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("mkdir: missing operand");
        std::process::exit(1);
    };
    if let Err(err) = exo_coreutils::mkdir(Path::new(&path)) {
        eprintln!("mkdir: {err}");
        std::process::exit(1);
    }
}
