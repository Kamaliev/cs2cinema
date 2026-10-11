use std::process::ExitCode;

fn main() -> ExitCode {
    server::cli::run_cli(std::env::args().skip(1).collect(), &|k| {
        std::env::var(k).ok()
    })
}
