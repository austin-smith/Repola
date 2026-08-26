use std::io::{stdin, stdout};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    match (arguments.next().as_deref(), arguments.next()) {
        (Some("--stdio"), None) => {
            let mut input = stdin().lock();
            let mut output = stdout();
            if let Err(error) = repola_engine::protocol::serve(&mut input, &mut output) {
                eprintln!("Repola agent protocol failed: {error}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        (Some("--version"), None) => {
            println!("repola-agent {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("Usage: repola-agent --stdio | --version");
            ExitCode::from(2)
        }
    }
}
