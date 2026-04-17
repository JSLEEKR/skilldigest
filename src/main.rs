//! skilldigest CLI binary.

use std::process;

use clap::Parser;
use skilldigest::cli::{self, Cli};

fn main() {
    let cli = Cli::parse();
    match cli::run(cli) {
        Ok(code) => process::exit(code.as_i32()),
        Err(e) => {
            eprintln!("skilldigest: {e}");
            process::exit(e.exit_code().as_i32());
        }
    }
}
