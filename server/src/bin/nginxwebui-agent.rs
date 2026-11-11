fn main() {
    if let Err(err) = nginxwebui::agent::run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
