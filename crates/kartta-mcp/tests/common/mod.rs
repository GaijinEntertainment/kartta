use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_kartta-mcp"))
            .args(arguments)
            .env_remove("KARTTA_ROOT")
            .env_remove("KARTTA_GREP_INDEX")
            .env_remove("KARTTA_CACHE_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the server starts");
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
