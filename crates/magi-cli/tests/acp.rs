#![allow(clippy::expect_used, clippy::indexing_slicing)]

use std::{process::Stdio, time::Duration};

use magi_model::scratch::Scratch;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn run(input: &[u8], flags: &[&str]) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    let root = Scratch::new("acp", "stdio");
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_magi"));
    let mut child = command
        .arg("acp")
        .args(flags)
        .env_clear()
        .env("HOME", &*root)
        .env("PATH", &*root)
        .env("XDG_CONFIG_HOME", &*root)
        .env("XDG_STATE_HOME", &*root)
        .env("XDG_CACHE_HOME", &*root)
        .env("XDG_RUNTIME_DIR", &*root)
        .current_dir(&*root)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ACP binary");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout").take(65_537);
    let mut stderr = child.stderr.take().expect("stderr").take(65_537);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        stdin.write_all(input).await?;
        drop(stdin);
        tokio::try_join!(
            child.wait(),
            stdout.read_to_end(&mut out),
            stderr.read_to_end(&mut err)
        )
    })
    .await;
    let status = match result {
        Ok(Ok((status, _, _))) => status,
        other => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!("ACP pipe fixture failed: {other:?}");
        }
    };
    assert!(
        out.len() <= 65_536 && err.len() <= 65_536,
        "bounded fixture output"
    );
    assert!(
        std::fs::read_dir(&*root)
            .expect("fixture directory")
            .next()
            .is_none(),
        "protocol startup must not create native state"
    );
    (status, out, err)
}

#[tokio::test]
async fn real_stdio_initializes_without_config_siblings_or_stdout_noise() {
    let (status, out, err) = run(b"{\"jsonrpc\":\"2.0\",\"id\":\"editor\",\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n", &[]).await;
    assert!(status.success(), "{}", String::from_utf8_lossy(&err));
    let reply: Value = serde_json::from_slice(&out).expect("one protocol response only");
    assert_eq!(reply["id"], "editor");
    assert_eq!(reply["result"]["protocolVersion"], 1);
    assert_eq!(reply["result"]["agentInfo"]["name"], "magi");
    assert!(out.ends_with(b"\n"));
    assert!(String::from_utf8_lossy(&err).contains("experimental"));
}

#[tokio::test]
async fn real_stdio_disconnect_reaps_after_partial_frame() {
    let (status, out, err) = run(b"{\"jsonrpc\":", &[]).await;
    assert!(!status.success());
    assert!(out.is_empty());
    assert!(String::from_utf8_lossy(&err).contains("unterminated ACP frame"));
}

#[tokio::test]
async fn real_stdio_errors_are_protocol_frames_without_input_echo() {
    let (status, out, _err) = run(
        b"secret-malformed-input\n{\"jsonrpc\":\"2.0\",\"id\":null,\"method\":\"session/new\"}\n",
        &[],
    )
    .await;
    assert!(status.success());
    let lines: Vec<Value> = out
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("protocol error"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["id"], Value::Null);
    assert_eq!(lines[1]["error"]["code"], -32000);
    assert!(!String::from_utf8_lossy(&out).contains("secret-malformed-input"));
}

#[tokio::test]
async fn real_stdio_rejects_conflicting_native_flags_before_startup() {
    for flags in [
        &["--resume"][..],
        &["--cbor"][..],
        &["--socket", "/not-owned"][..],
    ] {
        let (status, out, err) = run(b"", flags).await;
        assert!(!status.success());
        assert!(out.is_empty());
        assert!(String::from_utf8_lossy(&err).contains("does not accept native"));
    }
}

#[tokio::test]
async fn real_stdio_notification_is_not_a_request_or_initialization() {
    let messages = [
        json!({"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":1}}),
        json!({"jsonrpc":"2.0","method":"session/new","id":8,"params":{}}),
    ];
    let input = messages
        .into_iter()
        .map(|m| format!("{m}\n"))
        .collect::<String>();
    let (status, out, _) = run(input.as_bytes(), &[]).await;
    assert!(status.success());
    let reply: Value = serde_json::from_slice(&out).expect("one reply");
    assert_eq!(reply["id"], 8);
    assert_eq!(reply["error"]["code"], -32000);
}

async fn socket_stdio(input: &[u8]) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let root = Scratch::new("acp", "socket-stdio");
    let (writer, child_input) = UnixStream::pair().expect("input socketpair");
    let (reader, child_output) = UnixStream::pair().expect("output socketpair");
    writer.set_nonblocking(true).expect("nonblocking input");
    reader.set_nonblocking(true).expect("nonblocking output");
    let mut input_socket = tokio::net::UnixStream::from_std(writer).expect("input");
    let output_socket = tokio::net::UnixStream::from_std(reader).expect("output");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_magi"))
        .arg("acp")
        .env_clear()
        .env("HOME", &*root)
        .env("PATH", &*root)
        .current_dir(&*root)
        .kill_on_drop(true)
        .stdin(Stdio::from(OwnedFd::from(child_input)))
        .stdout(Stdio::from(OwnedFd::from(child_output)))
        .stderr(Stdio::piped())
        .spawn()
        .expect("socket-backed ACP");
    let mut output_socket = output_socket.take(65_537);
    let mut errors = child.stderr.take().expect("stderr").take(65_537);
    let mut out = Vec::new();
    let mut err = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        for byte in input {
            input_socket.write_all(&[*byte]).await?;
        }
        drop(input_socket);
        tokio::try_join!(
            child.wait(),
            output_socket.read_to_end(&mut out),
            errors.read_to_end(&mut err)
        )
    })
    .await;
    let status = match result {
        Ok(Ok((status, _, _))) => status,
        other => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            panic!("socket fixture: {other:?}");
        }
    };
    assert!(out.len() <= 65_536 && err.len() <= 65_536);
    assert!(std::fs::read_dir(&*root).expect("fixture").next().is_none());
    (status, out, err)
}

#[tokio::test]
async fn real_socket_stdio_initializes_with_fragmented_input_and_clean_eof() {
    let request = json!({"jsonrpc":"2.0","id":"socket-editor","method":"initialize","params":{"protocolVersion":1}});
    let (status, out, err) = socket_stdio(format!("{request}\n").as_bytes()).await;
    assert!(status.success(), "{}", String::from_utf8_lossy(&err));
    let response: Value = serde_json::from_slice(&out).expect("protocol response");
    assert_eq!(response["id"], "socket-editor");
    assert_eq!(response["result"]["protocolVersion"], 1);
    assert!(out.ends_with(b"\n"));
}

#[tokio::test]
async fn real_socket_stdio_partial_input_disconnect_is_not_a_success() {
    let (status, out, err) = socket_stdio(b"{\"jsonrpc\":").await;
    assert!(!status.success());
    assert!(out.is_empty());
    assert!(String::from_utf8_lossy(&err).contains("unterminated ACP frame"));
}
