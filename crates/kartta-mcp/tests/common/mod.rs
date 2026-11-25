use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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
    /// The root a call names when its arguments name none: the launch root, then that of each `index_root` that
    /// answered an index.
    root: Option<PathBuf>,
    /// The instructions the server's `initialize` answer carries.
    pub(crate) instructions: String,
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
        Self::launch(Self::command(arguments, "on")).rooted_at(arguments)
    }

    /// Starts the server with `arguments` and `KARTTA_WATCH=off`, and completes the MCP handshake.
    pub(crate) fn unwatched(arguments: &[&Path]) -> Self {
        Self::launch(Self::command(arguments, "off")).rooted_at(arguments)
    }

    /// Starts the server in `dir` with `arguments`, and completes the MCP handshake.
    pub(crate) fn start_in(dir: &Path, arguments: &[&Path]) -> Self {
        let mut command = Self::command(arguments, "on");
        command.current_dir(dir);
        let mut client = Self::launch(command);
        client.root = arguments.first().map(|root| dir.join(root));
        client
    }

    /// The client, its calls naming the root the server's `arguments` launch it with, as an absolute path.
    fn rooted_at(mut self, arguments: &[&Path]) -> Self {
        self.root = arguments
            .first()
            .map(|root| std::path::absolute(root).expect("the launch root resolves"));
        self
    }

    /// Starts an unwatched server that keeps a file-backed trigram index in `dir`, and completes the MCP handshake.
    pub(crate) fn grep_indexed_in(dir: &Path) -> Self {
        let mut command = Self::command(&[], "off");
        command
            .env("KARTTA_GREP_INDEX", "file")
            .env("KARTTA_GREP_INDEX_DIR", dir);
        Self::launch(command)
    }

    /// Starts an unwatched server that sets neither the cache nor the content index, with `home` as the user's home and
    /// local application data directory, and completes the MCP handshake.
    pub(crate) fn defaulted_in(home: &Path) -> Self {
        let mut command = Self::command(&[], "off");
        command
            .env_remove("KARTTA_CACHE_DIR")
            .env_remove("KARTTA_GREP_INDEX")
            .env_remove("KARTTA_GREP_INDEX_DIR")
            .env_remove("XDG_CACHE_HOME")
            .env("HOME", home)
            .env("LOCALAPPDATA", home);
        Self::launch(command)
    }

    /// Starts the server with `arguments` and its stderr piped, completes the MCP handshake, and returns each line the
    /// server writes to stderr as it arrives.
    pub(crate) fn with_stderr(arguments: &[&Path]) -> (Self, mpsc::Receiver<String>) {
        let mut command = Self::command(arguments, "on");
        command.stderr(Stdio::piped());
        let mut client = Self::launch(command).rooted_at(arguments);
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
            .env("KARTTA_GREP_INDEX", "")
            .env("KARTTA_CACHE_DIR", "")
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
            root: None,
            instructions: String::new(),
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
        initialized["instructions"]
            .as_str()
            .expect("the server sends instructions")
            .clone_into(&mut client.instructions);
        client.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        client
    }

    /// Sends a request and returns its result, failing on a protocol error.
    pub(crate) fn request(&mut self, method: &str, params: &Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self.frame(method);
            if message["id"] == id {
                assert!(message["error"].is_null(), "{method} failed: {message}");
                return message["result"].clone();
            }
        }
    }

    /// Sends every call before it reads an answer, so the server answers them side by side, and returns their answers
    /// in the order of `calls`. Each call's arguments go as they are.
    pub(crate) fn calls_at_once(&mut self, calls: &[(&str, Value)]) -> Vec<Answer> {
        let first = self.next_id;
        for (tool, arguments) in calls {
            let call = json!({"name": tool, "arguments": arguments});
            self.send(&json!({"jsonrpc": "2.0", "id": self.next_id, "method": "tools/call", "params": call}));
            self.next_id += 1;
        }
        let mut answers: Vec<Option<Answer>> = calls.iter().map(|_| None).collect();
        while answers.iter().any(Option::is_none) {
            let message = self.frame("tools/call");
            let at = message["id"].as_u64().and_then(|id| id.checked_sub(first));
            if let Some(answer) = at.and_then(|at| answers.get_mut(usize::try_from(at).ok()?)) {
                assert!(message["error"].is_null(), "tools/call failed: {message}");
                *answer = Some(Answer::of(&message["result"]));
            }
        }
        answers.into_iter().flatten().collect()
    }

    /// The next frame the server writes, failing when it closes stdout before answering `method`.
    fn frame(&mut self, method: &str) -> Value {
        let mut line = String::new();
        let read = self.output.read_line(&mut line).expect("stdout reads");
        assert!(
            read > 0,
            "the server closed stdout before answering {method}"
        );
        serde_json::from_str(&line).expect("every stdout line is a JSON-RPC frame")
    }

    /// Calls a tool with `arguments`, and with the client's root as `root` when they name none and the tool is not
    /// `index_root`. An `index_root` that answers an index makes its root the client's.
    pub(crate) fn call(&mut self, tool: &str, arguments: &Value) -> Answer {
        let mut arguments = arguments.clone();
        if let (Some(root), Value::Object(named)) = (&self.root, &mut arguments)
            && tool != "index_root"
        {
            named.entry("root").or_insert_with(|| json!(root));
        }
        let answer = self.call_as_sent(tool, &arguments);
        if tool == "index_root" && answer.text.starts_with("<index root=") {
            self.root = arguments["root"].as_str().map(PathBuf::from);
        }
        answer
    }

    /// Calls a tool with `arguments` as they are.
    pub(crate) fn call_as_sent(&mut self, tool: &str, arguments: &Value) -> Answer {
        Answer::of(&self.request("tools/call", &json!({"name": tool, "arguments": arguments})))
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
        self.answer("index_root", &json!({"root": root}))
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

impl Answer {
    /// What the `tools/call` result `result` answered.
    fn of(result: &Value) -> Self {
        Self {
            text: result["content"][0]["text"]
                .as_str()
                .expect("the answer is text")
                .to_owned(),
            is_error: result["isError"].as_bool().unwrap_or(false),
        }
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
