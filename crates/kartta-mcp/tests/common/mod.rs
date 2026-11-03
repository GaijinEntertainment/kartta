use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The `kartta-mcp` binary, driven over its stdio as an MCP client drives it.
pub(crate) struct Client {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

/// What one tool call answered: the text an agent reads, and whether it is a tool error.
#[derive(Debug)]
pub(crate) struct Answer {
    pub(crate) text: String,
    pub(crate) is_error: bool,
}

impl Client {
    /// Starts the server with `arguments`, and completes the MCP handshake.
    pub(crate) fn start(arguments: &[&Path]) -> Self {
        Self::launch(Self::command(arguments, "on"))
    }

    /// Starts the server with `arguments` and `KARTTA_WATCH=off`, and completes the MCP handshake.
    pub(crate) fn unwatched(arguments: &[&Path]) -> Self {
        Self::launch(Self::command(arguments, "off"))
    }

    /// Starts an unwatched server that keeps a file-backed trigram index in `dir`, and completes the MCP handshake.
    pub(crate) fn grep_indexed_in(dir: &Path) -> Self {
        let mut command = Self::command(&[], "off");
        command
            .env("KARTTA_GREP_INDEX", "file")
            .env("KARTTA_GREP_INDEX_DIR", dir);
        Self::launch(command)
    }

    /// Starts the server with `arguments` and its stderr piped, completes the MCP handshake, and returns each line the
    /// server writes to stderr as it arrives.
    pub(crate) fn with_stderr(arguments: &[&Path]) -> (Self, mpsc::Receiver<String>) {
        let mut command = Self::command(arguments, "on");
        command.stderr(Stdio::piped());
        let mut client = Self::launch(command);
        let stderr = client.child.stderr.take().expect("stderr is piped");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            BufReader::new(stderr)
                .lines()
                .map_while(Result::ok)
                .try_for_each(|line| sender.send(line))
        });
        (client, lines)
    }

    fn command(arguments: &[&Path], watch: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kartta-mcp"));
        command
            .args(arguments)
            .env_remove("KARTTA_ROOT")
            .env_remove("KARTTA_GREP_INDEX")
            .env_remove("KARTTA_CACHE_DIR")
            .env("KARTTA_WATCH", watch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        command
    }

    fn launch(mut command: Command) -> Self {
        let mut child = command.spawn().expect("the server starts");
        let input = child.stdin.take().expect("stdin is piped");
        let output = BufReader::new(child.stdout.take().expect("stdout is piped"));
        let mut client = Self {
            child,
            input,
            output,
            next_id: 1,
        };
        let initialized = client.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "kartta-mcp-tests", "version": "0"}
            }),
        );
        assert_eq!(initialized["serverInfo"]["name"], "kartta-mcp");
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    /// Sends a request and returns its result, failing on a protocol error.
    pub(crate) fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            let read = self.output.read_line(&mut line).expect("stdout reads");
            assert!(
                read > 0,
                "the server closed stdout before answering {method}"
            );
            let message: Value =
                serde_json::from_str(&line).expect("every stdout line is a JSON-RPC frame");
            if message["id"] == id {
                assert!(message["error"].is_null(), "{method} failed: {message}");
                return message["result"].clone();
            }
        }
    }

    /// Calls a tool with `arguments`.
    pub(crate) fn call(&mut self, tool: &str, arguments: &Value) -> Answer {
        let result = self.request("tools/call", &json!({"name": tool, "arguments": arguments}));
        Answer {
            text: result["content"][0]["text"]
                .as_str()
                .expect("the answer is text")
                .to_owned(),
            is_error: result["isError"].as_bool().unwrap_or(false),
        }
    }

    /// Calls a tool and returns its text, failing when it answers a tool error.
    pub(crate) fn answer(&mut self, tool: &str, arguments: &Value) -> String {
        let answer = self.call(tool, arguments);
        assert!(
            !answer.is_error,
            "{tool} answered an error: {}",
            answer.text
        );
        answer.text
    }

    /// Indexes `root` and returns the answer.
    pub(crate) fn index(&mut self, root: &Path) -> String {
        self.answer("index_repo", &json!({"root": root}))
    }

    /// Polls `index_stats` until the index is no longer building.
    pub(crate) fn wait_for_index(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let answer = self.call("index_stats", &json!({}));
            if !answer.text.contains("status=\"building\"") {
                return answer.text;
            }
            assert!(
                Instant::now() < deadline,
                "the index is still building: {}",
                answer.text
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn send(&mut self, message: &Value) {
        writeln!(self.input, "{message}").expect("stdin writes");
        self.input.flush().expect("stdin flushes");
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Writes each `(path, text)` under a new temporary directory.
pub(crate) fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tree = tempfile::tempdir().expect("a temporary directory");
    for (path, text) in files {
        let path = tree.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a file has a parent"))
            .expect("the directory is created");
        std::fs::write(path, text).expect("the file is written");
    }
    tree
}

/// A small Go module: a package with a type, its constructor and a method, and a command that calls both.
pub(crate) fn go_module() -> tempfile::TempDir {
    tree(&[
        ("go.mod", "module example.com/app\n\ngo 1.22\n"),
        (
            "pkg/service.go",
            "package pkg\n\n// Service greets.\ntype Service struct{}\n\n// New builds a Service.\n\
             func New() *Service {\n\treturn &Service{}\n}\n\n// Greet greets name.\n\
             func (s *Service) Greet(name string) string {\n\
             \treturn \"hi \" + name\n}\n",
        ),
        (
            "cmd/app/main.go",
            "package main\n\nimport \"example.com/app/pkg\"\n\nfunc main() {\n\ts := pkg.New()\n\trun(s)\n}\n\n\
             func run(s *pkg.Service) {\n\ts.Greet(\"x\")\n}\n",
        ),
        ("README.md", "# app\n"),
    ])
}

/// `text` with the fixture's root spelled `<root>`, so an answer can be pinned whatever the temporary path is.
pub(crate) fn rooted(text: &str, root: &Path) -> String {
    let shown = root.display().to_string();
    text.replace(&shown.replace('\\', "\\\\"), "<root>")
        .replace(&shown, "<root>")
}
