use magi_model::scratch::Scratch;
use magi_tools::{
    Registry,
    supplier::{SuppliedTool, Tooling},
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct Provider {
    pub(super) dir: Scratch,
    pub(super) program: PathBuf,
}

fn utility(name: &str) -> String {
    let path = std::env::split_paths(&std::env::var_os("PATH").expect("fixture PATH"))
        .map(|path| path.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("missing fixture prerequisite: {name}"));
    magi_proto::permit::quoted(
        std::path::absolute(path)
            .expect("absolute utility")
            .to_str()
            .expect("utility path"),
    )
}

impl Provider {
    pub(super) fn new(name: &str) -> Self {
        let dir = Scratch::new("magi-file-delivery", name);
        let program = dir.join("tools-role");
        let cards = ["look", "change", "store", "nested", "badread", "lost", "lost_missing", "reject"].map(|name| {
            let operation = match name { "look" | "badread" => Some("read"), "change" => Some("edit"), "store" | "lost" | "lost_missing" | "reject" => Some("write"), _ => None };
            let mut card = json!({"name":name,"description":"fixture","needs":"read",
                "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}});
            if let Some(operation) = operation { card["file_revision"] = json!({"version":1,"operation":operation}); }
            card
        });
        std::fs::write(
            dir.join("cards"),
            json!({"ok":true,"result":cards}).to_string(),
        )
        .expect("cards");
        std::fs::write(dir.join("version"), "1").expect("native fixture version");
        let root = magi_proto::permit::quoted(dir.to_str().expect("fixture root"));
        let source = format!(
            r#"#!/bin/sh
set -eu
root={root}
jq={jq}
reader={reader}
if [ "$1" = tools ]; then exec "$reader" "$root/cards"; fi
[ "$1" = run ] || exit 2
case " $* " in *" --file-admission "*) printf '%s\n' '{{"event":"file-admission","revision_contract":1}}';; esac
call=$("$reader")
printf '%s\n' "$call" >> "$root/calls"
tool=$(printf '%s' "$call" | "$jq" -r .tool)
path=$(printf '%s' "$call" | "$jq" -r .args.path)
n=$("$reader" "$root/version")
revision=$(printf 'cfr1:%064d' "$n")
key=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
case "$tool" in
  look|badread|nested)
    if [ "$path" = missing ] && [ ! -e "$root/created" ]; then
      recovery=$(printf '%s' "$call" | "$jq" -r .file_recovery)
      if [ "$recovery" = 1 ] && [ ! -e "$root/old-recovery-support" ]; then
        "$jq" -cn --arg key "$key" '{{ok:true,result:[{{said:"absent",revision_contract:1,absence:{{version:1,path_key:$key}}}}]}}'
      else
        printf '%s\n' '{{"ok":true,"result":[{{"said":"missing","failed":true}}]}}'
      fi
      exit 0
    fi
    failed=false
    [ "$tool" != badread ] || failed=true
    "$jq" -cn --arg revision "$revision" --arg key "$key" --argjson failed "$failed" \
      '{{ok:true,result:[{{said:"observed",failed:$failed,revision_contract:1,observations:[{{path_key:$key,revision:$revision,coverage:"snapshot"}}]}}]}}'
    ;;
  lost_missing) printf 'refused: not a typed reply'; exit 0;;
  reject) printf '%s\n' '{{"ok":false,"family":1,"n":0,"result":[],"fault":"refused","error":"fixture safely refused"}}';;
  change|store|lost)
    expected=$(printf '%s' "$call" | "$jq" -r .expected_revision)
    id=$(printf '%s' "$call" | "$jq" -r .mutation_id)
    if [ "$path" = missing ] && [ ! -e "$root/created" ] && [ "$expected" = absent ]; then
      printf created > "$root/created"
    elif [ "$expected" != "$revision" ]; then
      "$jq" -cn --arg id "$id" '{{ok:true,result:[{{said:"stale",failed:true,revision_contract:1,mutation:{{id:$id,outcome:"refused"}}}}]}}'
      exit 0
    fi
    n=$((n+1))
    printf %s "$n" > "$root/version"
    if [ "$tool" = lost ]; then printf 'refused: not a typed reply'; exit 0; fi
    revision=$(printf 'cfr1:%064d' "$n")
    "$jq" -cn --arg id "$id" --arg revision "$revision" --arg key "$key" \
      '{{ok:true,result:[{{said:("committed "+$revision),revision_contract:1,observations:[{{path_key:$key,revision:$revision,coverage:"snapshot"}}],mutation:{{id:$id,outcome:"committed",durability:"synced"}}}}]}}'
    ;;
  *) exit 2;;
esac
"#,
            jq = utility("jq"),
            reader = utility("cat")
        );
        std::fs::write(&program, source).expect("native role fixture");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .expect("executable role");
        Self { dir, program }
    }

    pub(super) fn tools(&self) -> Vec<SuppliedTool> {
        magi_model::scratch::executable_ready(&self.program);
        let tools = SuppliedTool::all(
            &Tooling::of(self.program.to_str().expect("role path")),
            Arc::new(magi_tools::question::Unanswered),
            Arc::new(magi_tools::holding::Screenless),
            Arc::new(magi_tools::holding::Incurious),
            &Default::default(),
        );
        assert_eq!(tools.len(), 8, "fixture discovery must load every tool");
        tools
    }

    pub(super) fn registry(&self) -> Registry {
        let mut registry = Registry::new();
        for tool in self.tools() {
            registry.register(Box::new(tool));
        }
        registry
    }

    pub(super) fn calls(&self) -> Vec<Value> {
        match std::fs::read_to_string(self.dir.join("calls")) {
            Ok(calls) => calls
                .lines()
                .map(|line| serde_json::from_str(line).expect("actual request"))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("request log: {error}"),
        }
    }
}

#[test]
fn discovery_waits_for_fixture_writers_before_loading_the_declared_tools() {
    let provider = Provider::new("held-script-writer");
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(&provider.program)
        .expect("held fixture writer");
    let busy = std::process::Command::new(&provider.program)
        .arg("--probe")
        .status()
        .expect_err("kernel refuses an executable held open for writing");
    assert_eq!(busy.raw_os_error(), Some(26));
    std::thread::scope(|scope| {
        let (started, starting) = std::sync::mpsc::channel();
        let (finished, finishing) = std::sync::mpsc::channel();
        let provider = &provider;
        scope.spawn(move || {
            started.send(()).expect("discovery started");
            finished.send(provider.tools().len()).expect("listed count");
        });
        starting.recv().expect("discovery entered");
        let early = finishing.recv_timeout(std::time::Duration::from_millis(100));
        drop(writer);
        let count = match early {
            Ok(count) => panic!("discovery completed with a writable executable: {count}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => finishing
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("discovery resumes after the writer closes"),
            Err(error) => panic!("discovery channel: {error}"),
        };
        assert_eq!(count, 8, "all fixture cards must be loaded");
        assert!(
            provider.calls().is_empty(),
            "discovery dispatched a request"
        );
    });
}
