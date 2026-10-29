use super::{turns::*, *};

fn jq() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").expect("tools PATH"))
        .map(|directory| directory.join("jq"))
        .find(|path| path.is_file())
        .and_then(|path| std::fs::canonicalize(path).ok())
        .expect("selected jq")
}

pub(super) fn model(name: &str, suffix: &str, arguments: Value) -> magi_testkit::Mind {
    magi_testkit::Mind::scripted(
        name,
        &format!(
            r#"
if [ $((n % 2)) -eq 0 ]; then
  tool=$('{jq}' -r '[.context.tools[]? | select(.name|startswith("mcp__")) | select(.name|endswith("__{suffix}"))] | if length == 1 then .[0].name else "" end' "$here/ask.$n")
  if [ -n "$tool" ]; then
    '{jq}' -cn --arg name "$tool" --argjson args '{arguments}' '{{event:"tool_call_start",id:"mcp1",name:$name}},{{event:"tool_call_args",args:($args|tojson)}},{{event:"stop",reason:"tool_use"}}'
  else
    printf '%s\n' '{{"event":"text","text":"no session MCP"}}' '{{"event":"stop","reason":"end_turn"}}'
  fi
else
  printf '%s\n' '{{"event":"text","text":"after MCP"}}' '{{"event":"stop","reason":"end_turn"}}'
fi
"#,
            jq = jq().display(),
            arguments = arguments.to_string().replace('\'', "'\\''")
        ),
    )
}

pub(super) fn server() -> String {
    format!(
        r#"
while IFS= read -r line; do
  method=$(printf '%s' "$line" | '{jq}' -r '.method // ""')
  id=$(printf '%s' "$line" | '{jq}' -c '.id // null')
  case "$method" in
    initialize)
      version=$(printf '%s' "$line" | '{jq}' -r '.params.protocolVersion')
      '{jq}' -cn --argjson id "$id" --arg version "$version" '{{jsonrpc:"2.0",id:$id,result:{{protocolVersion:$version,capabilities:{{tools:{{}}}},serverInfo:{{name:"ACP fixture",version:"1"}}}}}}' ;;
    tools/list)
      '{jq}' -cn --argjson id "$id" '{{jsonrpc:"2.0",id:$id,result:{{tools:[{{name:"echo",description:"Echo literal fixture data",inputSchema:{{type:"object",properties:{{}},additionalProperties:false}}}}]}}}}' ;;
    tools/call)
      printf 'called\n' >> called
      '{jq}' -cn --argjson id "$id" --arg value "$1:$ACP_FIXTURE_TOKEN" '{{jsonrpc:"2.0",id:$id,result:{{content:[{{type:"text",text:$value}}],isError:false}}}}' ;;
  esac
done
"#,
        jq = jq().display()
    )
}

pub(super) async fn result(fixture: &mut Fixture, session: &Value, allow: bool) -> (String, usize) {
    let mut output = String::new();
    let mut approvals = 0;
    loop {
        let frame = fixture.read().await;
        if frame["method"] == "session/request_permission" {
            assert_eq!(frame["params"]["sessionId"], *session);
            let options = frame["params"]["options"].as_array().expect("options");
            let kind = if allow { "allow_once" } else { "reject_once" };
            let option = options
                .iter()
                .find(|option| option["kind"] == kind)
                .expect("permission kind");
            fixture
                .send(json!({"jsonrpc":"2.0", "id":frame["id"], "result":{
                    "outcome":{"outcome":"selected","optionId":option["optionId"]}
                }}))
                .await;
            approvals += 1;
        } else if frame.get("id").is_some() {
            assert_eq!(frame["id"], 3);
            assert_eq!(frame["result"]["stopReason"], "end_turn", "{frame}");
            return (output, approvals);
        } else {
            assert_eq!(frame["params"]["sessionId"], *session);
            output.push_str(&frame.to_string());
        }
    }
}

async fn generic(allow: bool) {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = model(
        if allow {
            "acp-mcp-allow"
        } else {
            "acp-mcp-deny"
        },
        "echo",
        json!({}),
    );
    synthetic(&fixture, &mind);
    let installed = std::fs::read(fixture.root.join("c/magi/init.lua")).expect("config before");
    let token = "literal $(touch escaped) ' \" fixture-token";
    let response = fixture.call(2, "session/new", json!({"cwd":fixture.root.join("a"),"mcpServers":[{
        "name":"Editor tools λ", "command":"/bin/sh", "args":["-c",server(),"fixture","literal argument"],
        "env":[{"name":"ACP_FIXTURE_TOKEN","value":token}]
    }]})).await;
    assert!(response.get("result").is_some(), "{response}");
    let session = response["result"]["sessionId"].clone();
    prompt(&mut fixture, 3, &session, "call the session MCP echo").await;
    let (output, approvals) = result(&mut fixture, &session, allow).await;
    assert_eq!(approvals, 1, "{output}; asks={}", mind.heard());
    assert_eq!(mind.asked(), 2);
    let called = fixture.root.join("a/called");
    if allow {
        assert!(output.contains("fixture-token"), "{output}");
        assert!(mind.asks()[1].contains("literal argument:"));
        assert!(mind.asks()[1].contains("fixture-token"));
        assert_eq!(
            std::fs::read_to_string(&called).expect("actual MCP call"),
            "called\n"
        );
    } else {
        assert!(!output.contains("fixture-token"));
        assert!(!called.exists());
    }
    assert!(!fixture.root.join("a/escaped").exists());
    let isolated = fixture
        .call(
            2,
            "session/new",
            json!({"cwd":fixture.root.join("b"),"mcpServers":[]}),
        )
        .await;
    let isolated = isolated["result"]["sessionId"].clone();
    prompt(&mut fixture, 3, &isolated, "no MCP inherited").await;
    let (output, approvals) = result(&mut fixture, &isolated, false).await;
    assert_eq!(approvals, 0);
    assert!(output.contains("no session MCP"));
    assert!(!mind.asks()[2].contains("mcp__"));
    assert_eq!(
        std::fs::read(fixture.root.join("c/magi/init.lua")).expect("config after"),
        installed
    );
    let _retained = fixture.close().await;
    assert!(!called.exists() || allow);
}

#[tokio::test]
async fn native_acp_stdio_mcp_invokes_with_literal_configuration_and_no_session_leakage() {
    generic(true).await;
}

#[tokio::test]
async fn native_acp_stdio_mcp_denial_prevents_the_server_action() {
    generic(false).await;
}

#[tokio::test]
async fn native_acp_nested_native_read_mcp_preserves_the_real_result() {
    let Some(mut fixture) = Fixture::start().await else {
        return;
    };
    let mind = model("acp-native-mcp-read", "read", json!({"path":"note.txt"}));
    synthetic(&fixture, &mind);
    std::fs::write(fixture.root.join("a/note.txt"), "nested-native-MCP-result").expect("resource");
    std::fs::copy(
        fixture.root.join("c/casper/tools.lua"),
        fixture.root.join("a/server-tools.lua"),
    )
    .expect("nested tool config");
    let casper = std::fs::read_link(fixture.root.join("bin/casper")).expect("selected Casper");
    let response = fixture.call(2, "session/new", json!({"cwd":fixture.root.join("a"),"mcpServers":[{
        "name":"Native reads", "command":casper,"args":["mcp","--tools=read"],"env":[
            {"name":"CASPER_JAIL","value":"1"},
            {"name":"CASPER_CONFIGURE","value":json!({"load":fixture.root.join("a/server-tools.lua")}).to_string()},
            {"name":"XDG_CONFIG_HOME","value":fixture.root.join("a/server-config")}
        ]
    }]})).await;
    assert!(response.get("result").is_some(), "{response}");
    let session = response["result"]["sessionId"].clone();
    prompt(&mut fixture, 3, &session, "read through session MCP").await;
    let (output, approvals) = result(&mut fixture, &session, true).await;
    assert_eq!(approvals, 1, "{output}; asks={}", mind.heard());
    assert!(output.contains("nested-native-MCP-result"), "{output}");
    assert_eq!(mind.asked(), 2);
    assert!(mind.asks()[1].contains("nested-native-MCP-result"));
    fixture.close().await;
}
