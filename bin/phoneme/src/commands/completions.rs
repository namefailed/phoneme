//! `phoneme completions <SHELL>` — print a shell-completion script to stdout.
//!
//! Pure local generation: this is the one subcommand that never loads config
//! or touches the daemon. It hands clap's own [`Command`](clap::Command) tree
//! to `clap_complete::generate`, so the emitted script always matches the
//! current CLI surface (every subcommand and flag in `args`). Pipe the output
//! into your shell's completion directory — see the cli_reference for the
//! per-shell install one-liners.

use crate::args::{Cli, CompletionsArgs};
use clap::CommandFactory;
use std::process::ExitCode;

pub fn run(args: CompletionsArgs) -> ExitCode {
    let mut cmd = Cli::command();
    clap_complete::generate(args.shell, &mut cmd, "phoneme", &mut std::io::stdout());
    ExitCode::SUCCESS
}
