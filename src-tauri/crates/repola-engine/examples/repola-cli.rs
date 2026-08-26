use std::process::ExitCode;

use repola_engine::worktree::{scan, ScanRequest};

fn main() -> ExitCode {
    let repository_paths = std::env::args().skip(1).collect::<Vec<_>>();
    if repository_paths.is_empty() {
        eprintln!("Pass at least one exact Git repository path.");
        return ExitCode::FAILURE;
    }
    match scan(ScanRequest { repository_paths }) {
        Ok(result) => match serde_json::to_string_pretty(&result) {
            Ok(json) => {
                println!("{json}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Could not serialize scan result: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("Scan failed: {error}");
            ExitCode::FAILURE
        }
    }
}
