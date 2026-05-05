fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(err) = exo_coreutils::echo(&args, &mut std::io::stdout()) {
        eprintln!("echo: {err}");
        std::process::exit(1);
    }
}
