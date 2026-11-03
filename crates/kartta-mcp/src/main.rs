//! `kartta-mcp [ROOT]`: serves the kartta tools over stdio until the client disconnects.
//!
//! A `ROOT` argument, or `KARTTA_ROOT` when there is none, starts indexing that repository at launch. stdout carries
//! protocol frames alone, and every diagnostic goes to stderr.
#![expect(
    clippy::multiple_crate_versions,
    reason = "the MCP SDK's macros build on syn 3 while serde's derive and the grammars' builds are on syn 2"
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use kartta_mcp::{Config, Server};
use rmcp::ServiceExt;
use rmcp::transport::stdio;

const USAGE: &str = "usage: kartta-mcp [ROOT]

Serves the kartta code graph tools over MCP on stdio. ROOT, or KARTTA_ROOT when no ROOT is given, starts indexing
that repository at launch; without either, the index_repo tool builds one.

Environment:
  KARTTA_ROOT            the repository to index at launch
  KARTTA_CACHE_DIR       a directory builds share per-file data through
  KARTTA_CACHE_ENTRY_TTL_SECS
                         the age in seconds past which a cache entry expires (unset, empty or 0: a week)
  KARTTA_GREP_INDEX      memory, file or mmap: build a trigram index that narrows grep
  KARTTA_GREP_INDEX_DIR  where a file or mmap trigram index lives (default: the temporary directory)
  KARTTA_THREADS         how many threads builds and queries run on (default: every core)
  KARTTA_WATCH           off, 0, false or no: take in edits only through the refresh tool (default: follow the tree)";

enum Launch {
    Serve(Option<PathBuf>),
    Print(String),
}

fn launch(mut arguments: impl Iterator<Item = OsString>) -> Result<Launch, String> {
    let root = match arguments.next() {
        None => std::env::var_os("KARTTA_ROOT").filter(|root| !root.is_empty()),
        Some(flag) if flag == "-h" || flag == "--help" => {
            return Ok(Launch::Print(USAGE.to_owned()));
        }
        Some(flag) if flag == "-V" || flag == "--version" => {
            return Ok(Launch::Print(format!(
                "kartta-mcp {}",
                env!("CARGO_PKG_VERSION")
            )));
        }
        Some(flag) if flag.to_string_lossy().starts_with('-') => {
            return Err(format!(
                "unknown option {}\n\n{USAGE}",
                flag.to_string_lossy()
            ));
        }
        Some(root) => Some(root),
    };
    if let Some(extra) = arguments.next() {
        return Err(format!(
            "unexpected argument {}\n\n{USAGE}",
            extra.to_string_lossy()
        ));
    }
    Ok(Launch::Serve(root.map(PathBuf::from)))
}

#[expect(
    clippy::print_stderr,
    reason = "stdout carries protocol frames, so diagnostics go to stderr"
)]
fn warn(message: &str) {
    eprintln!("kartta-mcp: {message}");
}

async fn warn_unbuilt(launched: impl Future<Output = Option<String>>) {
    if let Some(error) = launched.await {
        warn(&format!("the launch build ended without an index: {error}"));
    }
}

#[expect(
    clippy::print_stdout,
    reason = "help and version are the answer to the command line, not a frame"
)]
fn print(text: &str) {
    println!("{text}");
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let root = match launch(std::env::args_os().skip(1)) {
        Ok(Launch::Serve(root)) => root,
        Ok(Launch::Print(text)) => {
            print(&text);
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            warn(&error);
            return ExitCode::from(2);
        }
    };
    let (config, warnings) = Config::from_env();
    for warning in &warnings {
        warn(warning);
    }

    let server = Server::with_config(config);
    if let Some(root) = root {
        tokio::spawn(warn_unbuilt(server.index_at_launch(root)));
    }
    let service = match server.serve(stdio()).await {
        Ok(service) => service,
        Err(error) => {
            warn(&format!("the handshake failed: {error}"));
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = service.waiting().await {
        warn(&error.to_string());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::USAGE;

    #[test]
    fn the_help_lists_the_variables_the_readme_lists() {
        let readme: Vec<&str> = include_str!("../README.md")
            .lines()
            .filter_map(|line| line.strip_prefix("| `")?.split_once('`'))
            .map(|(name, _)| name)
            .collect();
        let help: Vec<&str> = USAGE
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .filter(|word| word.starts_with("KARTTA_"))
            .collect();

        assert_eq!(help, readme);
    }
}
