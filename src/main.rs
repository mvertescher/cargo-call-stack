#![deny(warnings)]

use core::str;
use std::{
    env,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{self, Command, Stdio},
    time::SystemTime,
};

use anyhow::bail;
use cargo_project::{Artifact, Profile, Project};
use clap::{Parser};
use env_logger::{Builder, Env};
use filetime::FileTime;
use walkdir::WalkDir;

use cargo_call_stack::OutputFormat;

mod wrapper;

/// Generate a call graph and perform whole program stack usage analysis
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Target triple for which the code is compiled
    #[arg(long, value_name = "TRIPLE")]
    target: Option<String>,

    /// Build only the specified binary
    #[arg(long, value_name = "BIN")]
    bin: Option<String>,

    /// Build only the specified example
    #[arg(long, value_name = "NAME")]
    example: Option<String>,

    /// Space-separated list of features to activate
    #[arg(long, value_name = "FEATURES")]
    features: Option<String>,

    /// Activate all available features
    #[arg(long)]
    all_features: bool,

    /// Use verbose output
    #[arg(short, long)]
    verbose: bool,

    /// Output format
    #[arg(long, default_value = "dot")]
    format: OutputFormat,

    /// consider only the call graph that starts from this node
    start: Option<String>,
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

#[allow(deprecated)]
fn run() -> anyhow::Result<i32> {
    if env::var_os("CARGO_CALL_STACK_RUSTC_WRAPPER").is_some() {
        return wrapper::wrapper();
    }

    Builder::from_env(Env::default().default_filter_or("warn")).init();

    let args = Args::parse();
    let profile = Profile::Release;

    let file = match (&args.example, &args.bin) {
        (Some(f), None) => f,
        (None, Some(f)) => f,
        _ => bail!("Please specify either --example <NAME> or --bin <NAME>."),
    };

    let meta = rustc_version::version_meta()?;
    let host = meta.host;
    let cwd = env::current_dir()?;
    let project = Project::query(cwd)?;
    let target_flag = args.target.as_deref();
    let target = project.target().or(target_flag).unwrap_or(&host);

    let mut is_no_std = false;
    {
        let output = Command::new("rustc")
            .args(&["--print=cfg", "--target", target])
            .output()?;
        for line in str::from_utf8(&output.stdout)?.lines() {
            if let Some(value) = line.strip_prefix("target_os=") {
                if value == "\"none\"" {
                    is_no_std = true;
                }
            }
        }
    };

    let mut cargo = Command::new("cargo");
    cargo.arg("rustc");

    // NOTE we do *not* use `project.target()` here because Cargo will figure things out on
    // its own (i.e. it will search and parse .cargo/config, etc.)
    if let Some(target) = target_flag {
        cargo.args(&["--target", target]);
    }

    if args.all_features {
        cargo.arg("--all-features");
    } else if let Some(features) = &args.features {
        cargo.args(&["--features", features]);
    }

    if args.example.is_some() {
        cargo.args(&["--example", file]);
    }

    if args.bin.is_some() {
        cargo.args(&["--bin", file]);
    }

    if profile.is_release() {
        cargo.arg("--release");
    }

    let build_std = if is_no_std {
        "-Zbuild-std=core,alloc,compiler_builtins"
    } else {
        "-Zbuild-std"
    };

    cargo.args(&[
        build_std,
        "--color=always",
        "--",
        // .ll file
        "--emit=llvm-ir,obj",
        // needed to produce a single .ll file
        "-C",
        "embed-bitcode=yes",
        "-C",
        "lto=fat",
    ]);

    cargo.env("CARGO_CALL_STACK_RUSTC_WRAPPER", "1");
    cargo.env("RUSTC_WRAPPER", env::current_exe()?);
    cargo.stderr(Stdio::piped());

    // "touch" some source file to trigger a rebuild
    let root = project.toml().parent().expect("UNREACHABLE");
    let now = FileTime::from_system_time(SystemTime::now());
    if !filetime::set_file_times(root.join("src/main.rs"), now, now).is_ok() {
        if !filetime::set_file_times(root.join("src/lib.rs"), now, now).is_ok() {
            // look for some rust source file and "touch" it
            let src = root.join("src");
            let haystack = if src.exists() { &src } else { root };

            for entry in WalkDir::new(haystack) {
                let entry = entry?;
                let path = entry.path();

                if path.extension().map(|ext| ext == "rs").unwrap_or(false) {
                    filetime::set_file_times(path, now, now)?;
                    break;
                }
            }
        }
    }

    if args.verbose {
        eprintln!("{:?}", cargo);
    }

    let mut child = cargo.spawn()?;
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let mut compiler_builtins_rlib_path = None;
    let mut compiler_builtins_ll_path = None;
    for line in stderr.lines() {
        let line = line?;
        if line.starts_with(wrapper::COMPILER_BUILTINS_RLIB_PATH_MARKER) {
            let path = &line[wrapper::COMPILER_BUILTINS_RLIB_PATH_MARKER.len()..];
            compiler_builtins_rlib_path = Some(path.to_string());
        } else if line.starts_with(wrapper::COMPILER_BUILTINS_LL_PATH_MARKER) {
            let path = &line[wrapper::COMPILER_BUILTINS_LL_PATH_MARKER.len()..];
            compiler_builtins_ll_path = Some(path.to_string());
        } else {
            eprintln!("{}", line);
        }
    }

    let status = child.wait()?;

    if !status.success() {
        return Ok(status.code().unwrap_or(1));
    }

    let compiler_builtins_rlib_path =
        compiler_builtins_rlib_path.expect("`compiler_builtins` was not linked");
    let compiler_builtins_ll_path =
        compiler_builtins_ll_path.expect("`compiler_builtins` LLVM IR unavailable");

    let path: PathBuf = if args.example.is_some() {
        project.path(Artifact::Example(file), profile, target_flag, &host)?
    } else {
        project.path(Artifact::Bin(file), profile, target_flag, &host)?
    };

    let prefix = format!("{}-", file.replace('-', "_"));
    let target = project.target().or(target_flag).unwrap_or(&host);

    cargo_call_stack::analyze(path, compiler_builtins_rlib_path, compiler_builtins_ll_path, target, prefix, args.start, args.format)
}
