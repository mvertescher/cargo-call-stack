//! Standalone version of cargo-call-stack

#![deny(warnings)]

use std::{process, path::PathBuf};
use clap::Parser;

use cargo_call_stack::OutputFormat;
use env_logger::{Builder, Env};

/// Generate a call graph and perform whole program stack usage analysis
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Target triple for which the code is compiled
    #[arg(long, value_name = "TRIPLE")]
    target: Option<String>,

    /// Output format
    #[arg(long, default_value = "dot")]
    format: OutputFormat,

    /// Path to the elf file
    #[arg(long, value_name = "ELF_PATH")]
    elf: Option<String>,

    /// Path to the complier-builtins rlib file
    #[arg(long, value_name = "COMPILER_BUILTINS_RLIB_PATH")]
    compiler_builtins_rlib_path: Option<String>,

    /// Path to the complier-builtins ll file
    #[arg(long, value_name = "COMPILER_BUILTINS_LL_PATH")]
    compiler_builtins_ll_path: Option<String>,
}

fn main() -> anyhow::Result<()> {
    match run() {
        Ok(ec) => process::exit(ec),
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1)
        }
    }
}

fn run() -> anyhow::Result<i32> {
    Builder::from_env(Env::default().default_filter_or("warn")).init();
    let args = Args::parse();

    let path = PathBuf::from(args.elf.unwrap());
    let compiler_builtins_rlib_path = args.compiler_builtins_rlib_path.unwrap();
    let compiler_builtins_ll_path = args.compiler_builtins_ll_path.unwrap();
    let target = args.target.unwrap();
    let prefix = String::new();

    cargo_call_stack::analyze(path, compiler_builtins_rlib_path, compiler_builtins_ll_path, &target, prefix, None, args.format)
}
