//! End-to-end stdio smoke: spawn the binary and speak raw MCP JSON-RPC.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn initialize_list_and_summary_over_stdio() {
    let dir = std::env::temp_dir().join(format!("daw_mcp_stdio_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let proj = dir.join("song.daw.json");
    let _ = std::fs::remove_file(&proj);

    let mut child = Command::new(env!("CARGO_BIN_EXE_daw-mcp"))
        .arg(&proj)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());

    let mut send = |s: &str| {
        writeln!(stdin, "{s}").unwrap();
        stdin.flush().unwrap();
    };
    let mut recv = || {
        let mut l = String::new();
        out.read_line(&mut l).unwrap();
        l
    };

    send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"0"}}}"#);
    let init = recv();
    assert!(init.contains(r#""id":1"#), "{init}");
    send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
    let tools = recv();
    assert!(
        tools.contains("project_summary") && tools.contains("listen"),
        "{tools}"
    );
    send(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"project_summary","arguments":{}}}"#);
    let sum = recv();
    assert!(sum.contains("Drums"), "{sum}");

    child.kill().ok();
    std::fs::remove_dir_all(&dir).ok();
}
