use std::io::Read;

const ANNOUNCEMENT_BYTES: usize = 1024;
pub(super) const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

fn line(from: &mut impl Read) -> Result<serde_json::Value, String> {
    let mut bytes = Vec::new();
    for _ in 0..ANNOUNCEMENT_BYTES {
        let mut byte = [0_u8];
        from.read_exact(&mut byte)
            .map_err(|_| "file admission announcement is missing or incomplete".to_owned())?;
        if byte == [b'\n'] {
            return serde_json::from_slice(&bytes)
                .map_err(|_| "file admission announcement is not a typed event".to_owned());
        }
        bytes.extend_from_slice(&byte);
    }
    Err("file admission announcement exceeds its bound".into())
}

pub(super) fn command(from: &mut impl Read) -> Result<(), String> {
    let event = line(from)?;
    if event.get("event").and_then(serde_json::Value::as_str) == Some("file-admission")
        && event
            .get("revision_contract")
            .and_then(serde_json::Value::as_u64)
            == Some(1)
    {
        Ok(())
    } else {
        Err("the actual tool process did not admit native file revision version1".into())
    }
}

#[cfg(test)]
pub(super) fn listening(from: &mut impl Read, path: &std::path::Path) -> Result<bool, String> {
    listening_context(from, path).map(|(files, _)| files)
}

pub(super) fn listening_context(
    from: &mut impl Read,
    path: &std::path::Path,
) -> Result<(bool, bool), String> {
    let event = line(from)?;
    if event.get("event").and_then(serde_json::Value::as_str) != Some("listening")
        || event
            .get("at")
            .is_some_and(|at| at.as_str() != path.to_str())
    {
        return Err("the tools daemon did not announce its selected socket".into());
    }
    let files = event
        .get("file_admission")
        .and_then(serde_json::Value::as_u64)
        == Some(1);
    let events = event
        .get("request_context")
        .and_then(serde_json::Value::as_u64)
        == Some(1);
    Ok((files, events))
}

pub(super) fn result(
    mut value: serde_json::Value,
    call: &magi_proto::tooling::Call,
) -> Result<magi_proto::tooling::Ran, String> {
    if !call.revision.requested()
        && let Some(object) = value.as_object_mut()
    {
        for field in ["revision_contract", "observations", "mutation", "absence"] {
            object.remove(field);
        }
    }
    serde_json::from_value(value).map_err(|why| why.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_actual_supported_listening_context_is_enabled() {
        for capability in [
            serde_json::Value::Null,
            serde_json::json!(0),
            serde_json::json!(1),
            serde_json::json!(2),
            serde_json::json!("1"),
            serde_json::json!(true),
        ] {
            let body = format!(
                "{}\n",
                serde_json::json!({"event":"listening","file_admission":1,"request_context":capability})
            );
            assert_eq!(
                listening_context(
                    &mut body.as_bytes(),
                    std::path::Path::new("/fixture/socket")
                )
                .expect("announcement"),
                (true, capability == 1)
            );
        }
    }

    #[test]
    fn native_file_events_are_bounded_typed_and_leave_the_reply_unconsumed() {
        for invalid in [
            b"listening\n".to_vec(),
            b"{\"event\":\"not-listening\"}\n".to_vec(),
            b"{\"event\":\"file-admission\",\"revision_contract\":null}\n".to_vec(),
            b"{\"event\":\"file-admission\",\"revision_contract\":2}\n".to_vec(),
            b"{\"event\":\"file-admission\",\"revision_contract\":1}".to_vec(),
            vec![b'x'; ANNOUNCEMENT_BYTES + 1],
        ] {
            assert!(command(&mut invalid.as_slice()).is_err());
        }
        let mut input = b"{\"event\":\"file-admission\",\"revision_contract\":1}\nreply".as_slice();
        command(&mut input).expect("supported actual process");
        assert_eq!(input, b"reply");
        let path = std::path::Path::new("/fixture/socket");
        assert!(!listening(&mut b"{\"event\":\"listening\"}\n".as_slice(), path).expect("legacy"));
        assert!(
            listening(
                &mut b"{\"event\":\"listening\",\"at\":\"/wrong\",\"file_admission\":1}\n"
                    .as_slice(),
                path
            )
            .is_err()
        );
        assert!(
            listening(
                &mut b"{\"event\":\"listening\",\"at\":\"/fixture/socket\",\"file_admission\":1}\n"
                    .as_slice(),
                path
            )
            .expect("native")
        );
        assert!(
            !listening(
                &mut b"{\"event\":\"listening\",\"file_admission\":2}\n".as_slice(),
                path
            )
            .expect("unsupported native version")
        );
    }

    #[test]
    fn native_machine_metadata_survives_supplier_and_both_registry_paths_but_not_legacy_or_lua_claims()
     {
        use crate::{Cancel, Ops, Output, Registry, Tool, Uncancelled};
        use serde_json::json;
        let call = serde_json::from_value(json!({"tool":"look","args":{},
            "file_revision":{"version":1,"operation":"read"}}))
        .expect("native read");
        let value = json!({"said":"native bytes","brief":"read source","revision_contract":1,
            "observations":[{"path_key":"opaque","revision":format!("cfr1:{}","a".repeat(64)),"coverage":"snapshot"}]});
        let output = super::super::finished(result(value.clone(), &call).expect("native result"));
        let expected = output.files.clone();
        assert_eq!(
            expected
                .as_ref()
                .expect("native metadata")
                .observations
                .len(),
            1
        );
        let legacy =
            serde_json::from_value(json!({"tool":"legacy","args":{}})).expect("legacy call");
        let mut forged = value;
        forged["observations"] = json!("unrelated malformed legacy extension");
        forged["absence"] = json!("unrelated malformed legacy extension");
        assert!(
            result(forged, &legacy)
                .expect("legacy extension ignored")
                .files
                .observations
                .is_empty()
        );
        let encoded = serde_json::to_value(&output).expect("output shape");
        assert!(encoded.get("files").is_none() && encoded.get("observations").is_none());
        let lua: Output = serde_json::from_value(json!({"content":"claim","is_error":false,
            "files":{"revision_contract":1,"observations":[{"path_key":"fake"}]},
            "observations":[{"path_key":"fake"}]}))
        .expect("ordinary callback shape");
        assert!(lua.files.is_none());

        struct Native(Output);
        impl Tool for Native {
            fn name(&self) -> &str {
                "look"
            }
            fn description(&self) -> &str {
                "metadata fixture"
            }
            fn parameters(&self) -> serde_json::Value {
                json!({"type":"object"})
            }
            fn run(&self, _: &serde_json::Value, _: &dyn Ops, _: &dyn Cancel) -> Output {
                self.0.clone()
            }
        }
        let dir = magi_model::scratch::Scratch::new("magi-files", "metadata");
        let ops = crate::ops::Real::new(dir.to_path_buf());
        let mut registry = Registry::new();
        registry.register(Box::new(Native(output)));
        let direct = registry.call("look", &json!({}), &ops, &Uncancelled);
        assert_eq!(direct.files, expected);
        assert_eq!(direct.content, "native bytes");
        assert_eq!(direct.hints.brief.as_deref(), Some("read source"));
        let prepared = registry.prepare("look", "{}", &ops);
        let collected = registry.finish(prepared, &ops, &Uncancelled);
        assert_eq!(collected.files, expected);
        assert_eq!(collected.content, "native bytes");
    }
}

#[cfg(test)]
mod processes;
