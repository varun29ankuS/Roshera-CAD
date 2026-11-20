//! Lifecycle of stdio MCP extension processes behind the ACP HTTP transport.
//!
//! Every session that carries a stdio `mcpServers` entry owns one real child
//! process. These tests drive `create_acp_router` exactly the way an HTTP
//! client does (POST `initialize`, SSE GET for responses, DELETE to close) and
//! check, by PID against the OS process list, that those children are bounded
//! by the session cap and are gone once their connection is closed.
//!
//! The stdio MCP server is this test binary itself, re-executed in a helper
//! mode (see [`maybe_run_probe_mcp_server`]). The helper records its PID in a
//! per-test directory and, after its stdin closes, deliberately keeps running
//! for a while: it models a server with live handles that does not exit on
//! EOF, so a passing test proves goose *killed* the child rather than merely
//! closing its pipes.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use futures::StreamExt;
use goose::acp::server_factory::{AcpServer, AcpServerFactoryConfig};
use goose::acp::transport::create_acp_router;
use goose::agents::GoosePlatform;
use serde_json::{json, Value};
use serial_test::serial;
use tokio::sync::mpsc;
use tower::ServiceExt;

const PROBE_PID_DIR_ENV: &str = "GOOSE_LIFECYCLE_PROBE_PID_DIR";
const PROBE_EXTENSION_NAME: &str = "lifecycle_probe";
const PROBE_TOOL_NAME: &str = "probe_ping";
/// How long the helper lingers after stdin EOF before exiting on its own.
/// Long enough that only a kill can make it disappear inside a test's
/// deadline; short enough that a leaked helper cleans itself up.
const PROBE_LINGER_AFTER_EOF: Duration = Duration::from_secs(120);
const PROCESS_EXIT_DEADLINE: Duration = Duration::from_secs(15);
const RESPONSE_DEADLINE: Duration = Duration::from_secs(60);
const SESSION_CAP: usize = 2;

/// Helper mode: when re-executed with [`PROBE_PID_DIR_ENV`] set, this binary
/// is a minimal stdio MCP server (initialize + tools/list) instead of a test
/// runner. Runs before `main`, so the test harness never starts.
#[ctor::ctor]
fn maybe_run_probe_mcp_server() {
    let Some(pid_dir) = std::env::var_os(PROBE_PID_DIR_ENV) else {
        return;
    };
    let pid = std::process::id();
    let pid_path = PathBuf::from(pid_dir).join(format!("{pid}.pid"));
    if std::fs::write(&pid_path, pid.to_string()).is_err() {
        std::process::exit(2);
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            // Notifications (e.g. notifications/initialized) need no reply.
            continue;
        };
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let reply = match method {
            "initialize" => {
                let protocol_version = message
                    .pointer("/params/protocolVersion")
                    .cloned()
                    .unwrap_or_else(|| json!("2025-03-26"));
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": protocol_version,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "lifecycle-probe", "version": "1.0.0" }
                    }
                })
            }
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": PROBE_TOOL_NAME,
                        "description": "lifecycle probe",
                        "inputSchema": { "type": "object", "properties": {} }
                    }]
                }
            }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            _ => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": "method not found" }
            }),
        };
        if writeln!(stdout, "{reply}").is_err() || stdout.flush().is_err() {
            break;
        }
    }

    std::thread::sleep(PROBE_LINGER_AFTER_EOF);
    std::process::exit(0);
}

static GOOSE_ROOT: LazyLock<tempfile::TempDir> = LazyLock::new(|| {
    let root = tempfile::tempdir().unwrap();
    std::env::set_var("GOOSE_PATH_ROOT", root.path());
    std::env::set_var("GOOSE_DISABLE_KEYRING", "1");
    std::env::set_var("OPENAI_API_KEY", "lifecycle-test-key");
    let config_dir = goose::config::paths::Paths::config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(goose::config::paths::Paths::in_state_dir("logs")).unwrap();
    // OPENAI_HOST points at the discard port: no test here ever prompts, and
    // nothing may leave the machine if a code path tries.
    std::fs::write(
        config_dir.join(goose::config::base::CONFIG_YAML_NAME),
        "GOOSE_PROVIDER: openai\nGOOSE_MODEL: gpt-4o\nGOOSE_DISABLE_KEYRING: true\n\
         OPENAI_HOST: http://127.0.0.1:9\n",
    )
    .unwrap();
    root
});

/// The PIDs of every probe child a test spawned. Dropping it force-kills all
/// of them, so a failing assertion never leaves a process behind.
struct ProbeProcesses {
    dir: tempfile::TempDir,
}

impl ProbeProcesses {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn dir(&self) -> &Path {
        self.dir.path()
    }

    fn pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = std::fs::read_dir(self.dir.path())
            .unwrap()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.strip_suffix(".pid")?.parse().ok()
            })
            .collect();
        pids.sort_unstable();
        pids
    }

    fn alive(&self) -> Vec<u32> {
        self.pids()
            .into_iter()
            .filter(|pid| process_alive(*pid))
            .collect()
    }

    /// Polls until at most `max_alive` probes are alive, returning the live
    /// set at the deadline. The kill on drop is asynchronous (rmcp spawns it
    /// onto the runtime), hence a deadline rather than an instant check.
    async fn wait_until_at_most(&self, max_alive: usize) -> Vec<u32> {
        let deadline = Instant::now() + PROCESS_EXIT_DEADLINE;
        loop {
            let alive = self.alive();
            if alive.len() <= max_alive || Instant::now() >= deadline {
                return alive;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

impl Drop for ProbeProcesses {
    fn drop(&mut self) {
        for pid in self.pids() {
            kill_process(pid);
        }
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let Ok(output) = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(unix)]
fn kill_process(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status();
}

fn acp_router(data_root: &Path, max_active_sessions: Option<usize>) -> Router {
    LazyLock::force(&GOOSE_ROOT);
    let server = Arc::new(AcpServer::new(AcpServerFactoryConfig {
        builtins: vec![],
        data_dir: data_root.join("data"),
        config_dir: data_root.join("config"),
        goose_platform: GoosePlatform::GooseCli,
        additional_source_roots: Vec::new(),
        enable_scheduler: false,
        max_active_sessions,
    }));
    create_acp_router(server)
}

fn probe_mcp_server(probes: &ProbeProcesses) -> Value {
    let exe = std::env::current_exe().unwrap();
    json!({
        "name": PROBE_EXTENSION_NAME,
        "command": exe.to_string_lossy(),
        "args": [],
        "env": [{ "name": PROBE_PID_DIR_ENV, "value": probes.dir().to_string_lossy() }],
    })
}

/// One ACP connection driven over HTTP, the way a browser drives it.
struct HttpAcpConnection {
    router: Router,
    connection_id: String,
    next_id: u64,
}

impl HttpAcpConnection {
    async fn open(router: &Router) -> Self {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": { "protocolVersion": 1, "clientCapabilities": {} }
        });
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/acp")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "initialize must succeed");
        let connection_id = response
            .headers()
            .get("acp-connection-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .expect("initialize response must carry Acp-Connection-Id");
        Self {
            router: router.clone(),
            connection_id,
            next_id: 1,
        }
    }

    /// Opens an SSE GET (the connection stream, or one session's stream) and
    /// forwards every JSON `data:` frame into a channel. Aborting the returned
    /// task drops the stream, exactly as a browser dropping the GET does.
    async fn subscribe(
        &self,
        session_id: Option<&str>,
    ) -> (mpsc::UnboundedReceiver<Value>, tokio::task::JoinHandle<()>) {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri("/acp")
            .header("accept", "text/event-stream")
            .header("acp-connection-id", &self.connection_id);
        if let Some(session_id) = session_id {
            builder = builder.header("acp-session-id", session_id);
        }
        let response = self
            .router
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "SSE GET must open");
        let (tx, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut stream = response.into_body().into_data_stream();
            let mut buffer = String::new();
            while let Some(Ok(chunk)) = stream.next().await {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(end) = buffer.find("\n\n") {
                    let frame: String = buffer.drain(..end + 2).collect();
                    for line in frame.lines() {
                        let Some(data) = line.strip_prefix("data:") else {
                            continue;
                        };
                        if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                            if tx.send(value).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        (rx, task)
    }

    async fn post(&self, session_id: Option<&str>, body: Value) -> StatusCode {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/acp")
            .header("content-type", "application/json")
            .header("acp-connection-id", &self.connection_id);
        if let Some(session_id) = session_id {
            builder = builder.header("acp-session-id", session_id);
        }
        self.router
            .clone()
            .oneshot(builder.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// Sends one JSON-RPC request and waits for its response on the matching
    /// SSE stream (session-scoped requests answer on the session stream).
    async fn request(&mut self, session_id: Option<&str>, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let (mut events, reader) = self.subscribe(session_id).await;
        let status = self
            .post(
                session_id,
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::ACCEPTED,
            "{method} POST must be accepted"
        );
        let response = tokio::time::timeout(RESPONSE_DEADLINE, async {
            while let Some(event) = events.recv().await {
                if event.get("id").and_then(Value::as_u64) == Some(id) {
                    return event;
                }
            }
            panic!("SSE stream ended before the {method} response arrived");
        })
        .await
        .unwrap_or_else(|_| panic!("no {method} response within {RESPONSE_DEADLINE:?}"));
        reader.abort();
        if let Some(error) = response.get("error") {
            panic!("{method} failed: {error}");
        }
        response["result"].clone()
    }

    async fn new_session(&mut self, cwd: &Path, probes: &ProbeProcesses) -> String {
        let result = self
            .request(
                None,
                "session/new",
                json!({
                    "cwd": cwd.to_string_lossy(),
                    "mcpServers": [probe_mcp_server(probes)],
                }),
            )
            .await;
        result["sessionId"]
            .as_str()
            .expect("session/new must return a sessionId")
            .to_string()
    }

    async fn tool_names(&mut self, session_id: &str) -> Vec<String> {
        let result = self
            .request(
                Some(session_id),
                "_goose/unstable/tools/list",
                json!({ "sessionId": session_id }),
            )
            .await;
        result["tools"]
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    async fn delete(&self) -> StatusCode {
        self.router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/acp")
                    .header("acp-connection-id", &self.connection_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }
}

/// Closing a connection with DELETE must take down the extension processes
/// of every session it opened — not leave them parented to a dead connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn delete_kills_every_extension_process_the_connection_spawned() {
    let data_root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let probes = ProbeProcesses::new();
    let router = acp_router(data_root.path(), Some(SESSION_CAP));

    let mut connection = HttpAcpConnection::open(&router).await;
    for _ in 0..SESSION_CAP {
        connection.new_session(cwd.path(), &probes).await;
    }
    let spawned = probes.pids();
    assert_eq!(
        spawned.len(),
        SESSION_CAP,
        "each session must have spawned exactly one probe: {spawned:?}"
    );
    assert_eq!(
        probes.alive(),
        spawned,
        "every probe must be alive before close"
    );

    assert_eq!(connection.delete().await, StatusCode::ACCEPTED);

    let alive = probes.wait_until_at_most(0).await;
    assert!(
        alive.is_empty(),
        "DELETE left {} of {} extension processes alive: {alive:?}",
        alive.len(),
        spawned.len()
    );
}

/// The session cap bounds live extension processes on one connection: an
/// LRU-evicted session's extension process is killed, not merely forgotten.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn session_cap_bounds_live_extension_processes_on_one_connection() {
    let data_root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let probes = ProbeProcesses::new();
    let router = acp_router(data_root.path(), Some(SESSION_CAP));

    let sessions_opened = SESSION_CAP + 3;
    let mut connection = HttpAcpConnection::open(&router).await;
    for _ in 0..sessions_opened {
        connection.new_session(cwd.path(), &probes).await;
    }
    assert_eq!(
        probes.pids().len(),
        sessions_opened,
        "each session must have spawned its own probe"
    );

    let alive = probes.wait_until_at_most(SESSION_CAP).await;
    let delete_status = connection.delete().await;
    assert!(
        alive.len() <= SESSION_CAP,
        "{} sessions opened with a cap of {SESSION_CAP}, but {} extension \
         processes are still alive: {alive:?}",
        sessions_opened,
        alive.len()
    );
    assert_eq!(delete_status, StatusCode::ACCEPTED);
}

/// What keeps a small cap safe: a session evicted from the cache is reloaded
/// from the session store on its next use, with its extension re-spawned and
/// its tools reachable again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn evicted_session_reloads_with_its_extension_respawned() {
    let data_root = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let probes = ProbeProcesses::new();
    let router = acp_router(data_root.path(), Some(1));

    let mut connection = HttpAcpConnection::open(&router).await;
    let first = connection.new_session(cwd.path(), &probes).await;
    let expected_tool = format!("{PROBE_EXTENSION_NAME}__{PROBE_TOOL_NAME}");
    assert!(
        connection.tool_names(&first).await.contains(&expected_tool),
        "the fresh session must expose the probe tool"
    );
    let first_pids = probes.pids();

    // A second session under a cap of one evicts the first.
    connection.new_session(cwd.path(), &probes).await;
    let alive = probes.wait_until_at_most(1).await;
    assert_eq!(
        alive.len(),
        1,
        "cap 1: only the newest session's probe may stay alive: {alive:?}"
    );
    assert!(
        first_pids.iter().all(|pid| !alive.contains(pid)),
        "the evicted session's probe must be the one killed"
    );

    // Using the evicted session reloads it and re-spawns its extension.
    let tools = connection.tool_names(&first).await;
    assert!(
        tools.contains(&expected_tool),
        "the reloaded session must expose the probe tool again: {tools:?}"
    );
    let respawned: Vec<u32> = probes
        .pids()
        .into_iter()
        .filter(|pid| !first_pids.contains(pid))
        .collect();
    assert_eq!(
        respawned.len(),
        2,
        "the second session's probe plus the reloaded first session's probe: {respawned:?}"
    );

    assert_eq!(connection.delete().await, StatusCode::ACCEPTED);
    let alive = probes.wait_until_at_most(0).await;
    assert!(alive.is_empty(), "DELETE must leave no probes: {alive:?}");
}
