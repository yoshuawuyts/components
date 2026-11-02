//! Pdfdiff command: compare the text of two PDF files, like `diff -u`.
//!
//! ```text
//! pdfdiff [-U <n>] [-w] [--label <old>] [--label <new>] <old.pdf> <new.pdf>
//! ```
//!
//! Prints a unified diff to stdout. Exits with `0` when the text is
//! identical, `1` when it differs, and `2` on error, matching `diff`.
#![allow(
    unsafe_code,
    missing_docs,
    clippy::missing_docs_in_private_items,
    reason = "wit-bindgen generates unsafe FFI glue and undocumented items"
)]
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "this is a command line tool"
)]

wit_bindgen::generate!({
    world: "yoshuawuyts:pdfdiff-cli/cli",
    path: ["../pdfdiff/wit", "wit"],
    generate_all,
    additional_derives: [PartialEq, Eq],
});

use std::process::ExitCode;
use yoshuawuyts::pdfdiff::diff::compare;
use yoshuawuyts::pdfdiff::types::Options;

const USAGE: &str = "\
Usage: pdfdiff [OPTIONS] <OLD.pdf> <NEW.pdf>

Compare the text of two PDF files and print a unified diff.

Options:
  -U, --unified <n>        Show <n> lines of context (default: 3)
  -w, --ignore-all-space   Ignore whitespace and blank-line changes
      --label <label>      Use <label> instead of the file name in the
                           header; pass twice to label both files
  -h, --help               Print this help

Exit status is 0 if the text is identical, 1 if it differs, and 2 on error.
Files are read through preopened directories, so run with e.g. `--dir .`.";

/// Parsed command line arguments.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    old: String,
    new: String,
    options: Options,
}

/// What the command should do.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Diff(Args),
    Help,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut context_lines = 3;
    let mut ignore_whitespace = false;
    let mut labels = Vec::new();
    let mut files = Vec::new();
    let mut args = args.into_iter();
    let mut only_files = false;

    while let Some(arg) = args.next() {
        if only_files || arg == "-" || !arg.starts_with('-') {
            files.push(arg);
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if arg.starts_with("--") => {
                (flag.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("missing value for `{name}`"))
        };
        match flag.as_str() {
            "--" => only_files = true,
            "-h" | "--help" => return Ok(Command::Help),
            "-w" | "--ignore-all-space" => ignore_whitespace = true,
            "-U" | "--unified" => {
                let n = value(&flag)?;
                context_lines = n
                    .parse()
                    .map_err(|_| format!("invalid context length `{n}`"))?;
            }
            "--label" => labels.push(value(&flag)?),
            _ if flag.starts_with("-U") => {
                let n = flag.trim_start_matches("-U");
                context_lines = n
                    .parse()
                    .map_err(|_| format!("invalid context length `{n}`"))?;
            }
            _ => return Err(format!("unknown option `{arg}`")),
        }
    }

    let [old, new]: [String; 2] = files
        .try_into()
        .map_err(|files: Vec<String>| format!("expected 2 files, got {}", files.len()))?;
    if labels.len() > 2 {
        return Err("`--label` may be passed at most twice".to_string());
    }
    let mut labels = labels.into_iter();
    let old_label = labels.next().unwrap_or_else(|| old.clone());
    let new_label = labels.next().unwrap_or_else(|| new.clone());
    Ok(Command::Diff(Args {
        old,
        new,
        options: Options {
            context_lines,
            old_label,
            new_label,
            ignore_whitespace,
        },
    }))
}

fn read(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| {
        let hint = if e.kind() == std::io::ErrorKind::NotFound {
            " (is its directory preopened with `--dir`?)"
        } else {
            ""
        };
        format!("{path}: {e}{hint}")
    })
}

fn run(args: &Args) -> Result<bool, String> {
    let old = read(&args.old)?;
    let new = read(&args.new)?;
    let diff = compare(&old, &new, &args.options)?;
    print!("{diff}");
    Ok(!diff.is_empty())
}

/// Drop the program name from `argv`, if there is one.
///
/// `wasmtime run` passes the path of the `.wasm` file as `argv[0]`, while
/// `component run` passes only the guest arguments.
fn guest_args(mut args: Vec<String>) -> Vec<String> {
    if args.first().is_some_and(|arg| arg.ends_with(".wasm")) {
        args.remove(0);
    }
    args
}

fn main() -> ExitCode {
    match parse(guest_args(std::env::args().collect())) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Ok(Command::Diff(args)) => match run(&args) {
            Ok(false) => ExitCode::SUCCESS,
            Ok(true) => ExitCode::from(1),
            Err(e) => {
                eprintln!("pdfdiff: {e}");
                ExitCode::from(2)
            }
        },
        Err(e) => {
            eprintln!("pdfdiff: {e}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]
    use super::{guest_args, parse, Args, Command, Options};

    fn args(list: &[&str]) -> Result<Command, String> {
        parse(list.iter().map(|s| (*s).to_string()))
    }

    fn diff(old: &str, new: &str, context_lines: u32, ignore: bool) -> Command {
        Command::Diff(Args {
            old: old.to_string(),
            new: new.to_string(),
            options: Options {
                context_lines,
                old_label: old.to_string(),
                new_label: new.to_string(),
                ignore_whitespace: ignore,
            },
        })
    }

    #[test]
    fn defaults() {
        assert_eq!(
            args(&["a.pdf", "b.pdf"]).unwrap(),
            diff("a.pdf", "b.pdf", 3, false)
        );
    }

    #[test]
    fn context_forms() {
        for form in [
            &["-U", "1"][..],
            &["-U1"],
            &["--unified", "1"],
            &["--unified=1"],
        ] {
            let mut list = form.to_vec();
            list.extend(["a", "b"]);
            assert_eq!(args(&list).unwrap(), diff("a", "b", 1, false), "{form:?}");
        }
    }

    #[test]
    fn ignore_whitespace() {
        assert_eq!(args(&["-w", "a", "b"]).unwrap(), diff("a", "b", 3, true));
    }

    #[test]
    fn labels() {
        let Command::Diff(parsed) = args(&["--label", "x", "--label=y", "a", "b"]).unwrap() else {
            panic!("expected a diff command");
        };
        assert_eq!(parsed.options.old_label, "x");
        assert_eq!(parsed.options.new_label, "y");
    }

    #[test]
    fn errors() {
        assert!(args(&["a"]).is_err());
        assert!(args(&["a", "b", "c"]).is_err());
        assert!(args(&["--bogus", "a", "b"]).is_err());
        assert!(args(&["-U", "x", "a", "b"]).is_err());
        assert!(args(&["-U"]).is_err());
    }

    #[test]
    fn program_name_is_dropped() {
        let list = |l: &[&str]| l.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            guest_args(list(&["x/pdfdiff.wasm", "a", "b"])),
            list(&["a", "b"])
        );
        assert_eq!(guest_args(list(&["a", "b"])), list(&["a", "b"]));
    }

    #[test]
    fn help_and_double_dash() {
        assert_eq!(args(&["-h"]).unwrap(), Command::Help);
        assert_eq!(
            args(&["--", "-a", "-b"]).unwrap(),
            diff("-a", "-b", 3, false)
        );
    }
}
