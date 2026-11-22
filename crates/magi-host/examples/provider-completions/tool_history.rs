use super::*;
use magi_model::{Content, Context, Message, Tool};

fn tool(name: &str) -> Tool {
    Tool {
        name: name.into(),
        description: format!("Use {name}"),
        parameters: json!({"type":"object","properties":{"state":{"const":{"array":[],"object":{},"null":null,"integer":u64::MAX}}}}),
        strict: None,
    }
}

fn names(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|tool| tool["function"]["name"].as_str().expect("tool name"))
        .collect()
}

pub(super) async fn scenario(root: &Path, mind: &str, native: bool, strict: bool) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1", listener.local_addr()?);
    let declaration = json!({"name":"Fixture","api":"openai-completions","base_url":url,
        "auth":{"kind":"api-key","vars":["FIXTURE_KEY"]},
        "compat":{"supports_mid_convo_system_messages":native,"supports_mid_convo_tool_additions":native,"supports_strict_mode":strict},
        "models":[{"id":"m","name":"M","context_window":8192,"max_tokens":128}]});
    let encoded = serde_json::to_string(&declaration.to_string())?;
    std::fs::write(
        root.join("providers.lua"),
        format!(
            r#"for _, id in ipairs({{"openrouter", "ollama"}}) do
            melchior.provider(id, {{name=id,api="openai-completions",auth={{kind="none"}}}})
        end
        melchior.provider("fixture", melchior.json.decode_value({encoded}))"#
        ),
    )?;
    let mut required = tool("b");
    if strict {
        required.strict = Some(magi_model::ToolStrictness::Require);
        required.parameters = json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false});
    }
    let mut change = Message::tool_changes(vec![required.clone()], vec![]);
    change.content.push(Content::Text {
        text: "update α".into(),
        signature: None,
    });
    let mut context = Context {
        system: Some("base".into()),
        tools: vec![tool("a")],
        messages: vec![Message::user("first"), change, Message::user("next")],
    };
    let worker = std::thread::spawn(move || serve(listener, vec![text_stream(), text_stream()]));
    for round in 0..2 {
        let encoded = serde_json::to_vec(&context)?;
        assert_eq!(serde_json::from_slice::<Context>(&encoded)?, context);
        let mut bytes = Vec::new();
        ciborium::into_writer(&context, &mut bytes)?;
        assert_eq!(
            ciborium::from_reader::<Context, _>(bytes.as_slice())?,
            context
        );
        let saved = context.clone();
        let mut deltas = Vec::new();
        let mut retries = 0;
        tokio::time::timeout(
            WAIT,
            magi_host::broker::ask_through(
                mind,
                "fixture/m",
                &context,
                &magi_proto::ask::Wants::default(),
                |delta| deltas.push(delta),
                |_| retries += 1,
            ),
        )
        .await?
        .map_err(|error| error.message)?;
        assert_eq!(retries, 0);
        assert!(
            deltas
                .iter()
                .any(|d| matches!(d, magi_model::Delta::Stop(StopReason::EndTurn)))
        );
        assert_eq!(context, saved);
        if round == 0 {
            context.messages.extend([
                Message::assistant("done"),
                Message::tool_changes(vec![tool("c")], vec!["a".into()]),
                Message::user("after removal"),
                Message::tool_changes(vec![tool("d")], vec![]),
            ]);
        }
    }
    let heard = worker
        .join()
        .map_err(|_| "tool history server panicked")??;
    assert_eq!(heard.len(), 2);
    for (round, body) in heard.iter().enumerate() {
        let wanted = match (native, round) {
            (true, 0) => vec!["a"],
            (false, 0) => vec!["a", "b"],
            (true, _) => vec!["b", "c"],
            (false, _) => vec!["b", "c", "d"],
        };
        assert_eq!(names(&body["tools"]), wanted);
        let messages = body["messages"].as_array().expect("messages");
        let inline: Vec<_> = messages
            .iter()
            .filter(|m| m.get("tools").is_some())
            .collect();
        assert_eq!(inline.len(), usize::from(native));
        if native {
            assert_eq!(
                names(&inline[0]["tools"]),
                if round == 0 { vec!["b"] } else { vec!["d"] }
            );
            assert_eq!(inline[0]["role"], "system");
            assert!(inline[0].get("content").is_none());
            assert_eq!(messages[1]["content"], "first");
        }
        for declarations in
            std::iter::once(&body["tools"]).chain(inline.iter().map(|m| &m["tools"]))
        {
            for declaration in declarations.as_array().expect("declarations") {
                let is_required = strict && declaration["function"]["name"] == "b";
                assert_eq!(
                    declaration["function"]["parameters"],
                    if is_required {
                        required.parameters.clone()
                    } else {
                        tool("schema").parameters
                    }
                );
                if is_required {
                    assert_eq!(declaration["function"]["strict"], true);
                }
            }
        }
        assert!(!body.to_string().contains("tool_changes"));
    }
    println!(
        "PASS tool history native={native} strict={strict}: actual Magi broker and selected Melchior, two loopback requests, lossless JSON/CBOR and immutable replay"
    );
    Ok(())
}
