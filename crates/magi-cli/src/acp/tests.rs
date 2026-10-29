#![allow(clippy::expect_used, clippy::indexing_slicing)]

use agent_client_protocol_schema::v1::{InitializeResponse, RequestId};
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};

use super::{envelope, initialize, json as strict_json, transport};

fn request(state: &mut initialize::State, params: Value) -> Value {
    state.request(RequestId::Number(7), "initialize", params)
}

#[test]
fn acp_initialization_uses_official_schema_and_only_supported_capabilities() {
    let mut state = initialize::State::default();
    let reply = request(&mut state, json!({"protocolVersion": 1}));
    let result: InitializeResponse =
        serde_json::from_value(reply["result"].clone()).expect("schema response");
    assert_eq!(result.protocol_version.as_u16(), 1);
    assert_eq!(result.agent_info.expect("agent info").name, "magi");
    assert_eq!(
        result.agent_capabilities,
        agent_client_protocol_schema::v1::AgentCapabilities::default().load_session(true)
    );
    assert!(result.auth_methods.is_empty());
    assert!(state.ready());
    assert_eq!(reply["id"], 7);
    assert_eq!(
        request(&mut state, json!({"protocolVersion": 1}))["error"]["code"],
        -32600
    );
}

#[test]
fn acp_negotiates_supported_version_without_coercion() {
    for version in [0, 1, 2, 65535] {
        assert_eq!(
            request(
                &mut initialize::State::default(),
                json!({"protocolVersion":version})
            )["result"]["protocolVersion"],
            1
        );
    }
    for version in [json!("1"), json!(1.5), json!(-1), json!(65536), Value::Null] {
        let mut state = initialize::State::default();
        assert_eq!(
            request(&mut state, json!({"protocolVersion":version}))["error"]["code"],
            -32602
        );
        assert!(!state.ready());
    }
}

#[test]
fn acp_rejects_malformed_capabilities_instead_of_defaulting() {
    for capabilities in [
        json!(null),
        json!([]),
        json!({"fs":false}),
        json!({"terminal":"yes"}),
        json!({"fs":{"readTextFile":1}}),
        json!({"fs":{"writeTextFile":null}}),
        json!({"auth":{"terminal":[]}}),
        json!({"session":{"compaction":true}}),
        json!({"elicitation":{"form":false}}),
    ] {
        let mut state = initialize::State::default();
        let reply = request(
            &mut state,
            json!({"protocolVersion":1,"clientCapabilities":capabilities}),
        );
        assert_eq!(reply["error"]["code"], -32602);
        assert!(!state.ready());
    }
    for info in [json!({}), json!({"name":"client","version":1}), json!(true)] {
        assert_eq!(
            request(
                &mut initialize::State::default(),
                json!({"protocolVersion":1,"clientInfo":info})
            )["error"]["code"],
            -32602
        );
    }
}

#[test]
fn acp_preserves_valid_capabilities_and_extension_metadata() {
    let params = json!({"protocolVersion":1,"clientCapabilities":{"fs":{"readTextFile":true},"terminal":true,"session":{"compaction":{},"notices":{}},"_extension":{"anything":true}},"_meta":{"private":{"arbitrary":[1,2]}}});
    assert!(
        request(&mut initialize::State::default(), params)
            .get("result")
            .is_some()
    );
}

#[test]
fn acp_rejects_nested_effect_bearing_config_capabilities() {
    let reply = request(
        &mut initialize::State::default(),
        json!({"protocolVersion":1,"clientCapabilities":{"session":{"configOptions":{"boolean":true}}}}),
    );
    assert_eq!(reply["error"]["code"], -32602);
}

#[test]
fn acp_completed_request_ids_can_be_reused_without_a_pending_collision() {
    let mut state = initialize::State::default();
    let id = RequestId::Str("same".into());
    let early = state.request(id.clone(), "session/new", json!({}));
    assert_eq!(early["error"]["code"], -32000);
    let initialized = state.request(id.clone(), "initialize", json!({"protocolVersion":1}));
    assert!(initialized.get("result").is_some());
    let later = state.request(id, "session/new", json!({}));
    assert_eq!(later["error"]["code"], -32601);
    assert_eq!(early["id"], initialized["id"]);
    assert_eq!(early["id"], later["id"]);
}

#[test]
fn acp_duplicate_fields_depth_nodes_utf8_and_trailing_data_are_rejected() {
    for input in [
        br#"{"id":1,"id":2}"#.as_slice(),
        br#"{"params":{"a":1,"\u0061":2}}"#,
        b"\xff",
        b"{} {}",
        b"{",
        b"",
    ] {
        assert!(strict_json::parse(input).is_err());
    }
    let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    assert!(strict_json::parse(deep.as_bytes()).is_err());
    let broad = format!("[{}0]", "0,".repeat(65_536));
    assert!(strict_json::parse(broad.as_bytes()).is_err());
    assert_eq!(
        strict_json::parse(br#"{"a":[],"b":{},"c":null,"d":-1,"e":1.5}"#).expect("valid"),
        json!({"a":[],"b":{},"c":null,"d":-1,"e":1.5})
    );
}

#[test]
fn acp_envelopes_preserve_ids_and_distinguish_notifications() {
    for id in [json!(null), json!(-7), json!(i64::MAX), json!("seven")] {
        let parsed =
            envelope::parse(json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{}}))
                .expect("envelope");
        let envelope::Incoming::Request { id: actual, .. } = parsed else {
            panic!("request");
        };
        assert_eq!(serde_json::to_value(actual).expect("id"), id);
    }
    assert!(matches!(
        envelope::parse(json!({"jsonrpc":"2.0","method":"initialize"})),
        Ok(envelope::Incoming::Notification { .. })
    ));
    for id in [
        json!(true),
        json!(1.5),
        json!(u64::MAX),
        json!("x".repeat(257)),
        json!({}),
    ] {
        assert!(envelope::parse(json!({"jsonrpc":"2.0","id":id,"method":"initialize"})).is_err());
    }
}

#[test]
fn acp_rejects_batches_wrong_version_and_ambiguous_envelopes() {
    for value in [
        json!([]),
        json!(null),
        json!({"method":"x","id":1}),
        json!({"jsonrpc":"1.0","method":"x"}),
        json!({"jsonrpc":"2.0","method":"x","result":1,"id":1}),
        json!({"jsonrpc":"2.0","id":1,"result":1,"error":{}}),
        json!({"jsonrpc":"2.0","method":"x","params":false}),
    ] {
        assert!(envelope::parse(value).is_err());
    }
}

#[tokio::test]
async fn acp_reads_fragmented_unicode_and_pipelined_lines() {
    let (mut peer, input) = tokio::io::duplex(4);
    let sender = tokio::spawn(async move {
        for byte in "{\"text\":\"λ\"}\n{}\r\n".as_bytes() {
            peer.write_all(&[*byte]).await.expect("write");
        }
    });
    let mut reader = BufReader::with_capacity(2, input);
    let first = transport::read(&mut reader)
        .await
        .expect("read")
        .expect("line");
    assert_eq!(
        strict_json::parse(&first).expect("utf8"),
        json!({"text":"λ"})
    );
    assert_eq!(
        transport::read(&mut reader).await.expect("read"),
        Some(b"{}\r".to_vec())
    );
    sender.await.expect("sender");
    assert!(transport::read(&mut reader).await.expect("eof").is_none());
}

#[tokio::test]
async fn acp_frame_limit_accepts_boundary_and_rejects_extra_byte() {
    let mut exact = vec![b' '; magi_ipc::MAX_FRAME_BYTES];
    exact.push(b'\n');
    assert_eq!(
        transport::read(&mut exact.as_slice())
            .await
            .expect("read")
            .expect("line")
            .len(),
        magi_ipc::MAX_FRAME_BYTES
    );
    exact.insert(0, b' ');
    assert!(transport::read(&mut exact.as_slice()).await.is_err());
    assert!(transport::read(&mut b"{}".as_slice()).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn acp_partial_input_and_blocked_output_expire() {
    let (mut peer, input) = tokio::io::duplex(4);
    peer.write_all(b"{").await.expect("partial");
    let mut reader = BufReader::new(input);
    assert!(transport::read(&mut reader).await.is_err());
    let (mut output, _blocked) = tokio::io::duplex(1);
    assert!(
        transport::write(&mut output, &json!({"result":"blocked"}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn acp_oversized_output_emits_no_partial_frame() {
    let (mut output, mut peer) = tokio::io::duplex(8);
    assert!(
        transport::write(&mut output, &"x".repeat(magi_ipc::MAX_FRAME_BYTES))
            .await
            .is_err()
    );
    drop(output);
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    peer.read_to_end(&mut bytes).await.expect("eof");
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn acp_connection_ignores_notifications_and_recovers_from_parse_errors() {
    let input = b"{\"jsonrpc\":\"2.0\",\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\nBAD\n{\"jsonrpc\":\"2.0\",\"id\":\"init\",\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/new\",\"params\":{}}\n";
    let mut output = Vec::new();
    super::serve(input.as_slice(), &mut output)
        .await
        .expect("connection");
    let replies: Vec<Value> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).expect("json"))
        .collect();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[0]["error"]["code"], -32700);
    assert_eq!(replies[1]["id"], "init");
    assert!(replies[1].get("result").is_some());
    assert_eq!(replies[2]["error"]["code"], -32602);
}

#[tokio::test(start_paused = true)]
async fn acp_idle_uninitialized_connection_has_a_deadline() {
    let (_peer, input) = tokio::io::duplex(8);
    assert!(super::serve(input, tokio::io::sink()).await.is_err());
}

#[tokio::test]
async fn acp_unsolicited_responses_close_without_error_loops() {
    let mut output = Vec::new();
    assert!(
        super::serve(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n".as_slice(),
            &mut output
        )
        .await
        .is_err()
    );
    assert!(output.is_empty());
}
