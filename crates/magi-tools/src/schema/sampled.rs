use serde_json::Value;

pub fn sampled(
    arguments: &Value,
    schema: &Value,
    strict: Option<magi_model::ToolStrictness>,
) -> Result<Value, String> {
    if strict.is_none() {
        return super::check(arguments, schema);
    }
    let mut restored = arguments.clone();
    restore(&mut restored, schema, 0, &mut 0)?;
    super::check(&restored, schema)
}

fn nullable(schema: &Value) -> bool {
    schema
        .get("type")
        .is_some_and(|v| v == "null" || v.as_array().is_some_and(|v| v.iter().any(|v| v == "null")))
        || schema
            .get("enum")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(Value::is_null))
        || schema.get("const").is_some_and(Value::is_null)
        || schema
            .get("anyOf")
            .and_then(Value::as_array)
            .is_some_and(|v| v.iter().any(nullable))
        || (!schema.is_object()
            || ["type", "enum", "const", "anyOf"]
                .iter()
                .all(|key| schema.get(*key).is_none()))
}

fn restore(
    value: &mut Value,
    schema: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), String> {
    *nodes += 1;
    if depth > 64 || *nodes > 16_384 {
        return Err("strict tool arguments exceed traversal limits".into());
    }
    if let (Some(object), Some(properties)) = (
        value.as_object_mut(),
        schema.get("properties").and_then(Value::as_object),
    ) {
        let required = schema.get("required").and_then(Value::as_array);
        for (name, property) in properties {
            *nodes += 1;
            if *nodes > 16_384 {
                return Err("strict tool arguments exceed traversal limits".into());
            }
            let required =
                required.is_some_and(|names| names.iter().any(|v| v.as_str() == Some(name)));
            if !required && !nullable(property) && object.get(name).is_some_and(Value::is_null) {
                object.remove(name);
            } else if let Some(child) = object.get_mut(name) {
                restore(child, property, depth + 1, nodes)?;
            }
        }
    }
    if let (Some(items), Some(rule)) = (value.as_array_mut(), schema.get("items")) {
        for child in items {
            restore(child, rule, depth + 1, nodes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_model::ToolStrictness;
    use serde_json::json;

    #[test]
    fn optional_nulls_restore_omission_and_defaults_without_mutating_the_transcript() {
        let schema = json!({"type":"object","properties":{
            "optional":{"type":"integer","default":7},
            "nullable":{"type":["string","null"]},
            "required":{"type":"string"},
            "nested":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"}}}}
        },"required":["required"]});
        let args =
            json!({"optional":null,"nullable":null,"required":"yes","nested":[{"name":null}]});
        let before = args.clone();
        for mode in [ToolStrictness::Prefer, ToolStrictness::Require] {
            let restored = sampled(&args, &schema, Some(mode)).expect("strict arguments");
            assert_eq!(
                restored,
                json!({"optional":7,"nullable":null,"required":"yes","nested":[{}]})
            );
            assert_eq!(args, before);
        }
        assert!(sampled(&args, &schema, None).is_err());
        assert!(
            sampled(
                &json!({"required":null}),
                &schema,
                Some(ToolStrictness::Require)
            )
            .is_err()
        );
    }

    #[test]
    fn nullable_unions_unknown_schemas_and_extra_fields_are_not_erased() {
        let schema = json!({"type":"object","properties":{
            "union":{"anyOf":[{"type":"string"},{"type":"null"}]},
            "unknown":{}, "enum":{"enum":[null,"x"]}
        }});
        let args = json!({"union":null,"unknown":null,"enum":null,"extra":null});
        assert_eq!(
            sampled(&args, &schema, Some(ToolStrictness::Prefer)).expect("preserved"),
            args
        );
    }

    #[test]
    fn removing_nulls_does_not_bypass_the_node_limit() {
        let properties: serde_json::Map<String, Value> = (0..16_385)
            .map(|n| (format!("p{n}"), json!({"type":"string"})))
            .collect();
        let arguments: serde_json::Map<String, Value> = properties
            .keys()
            .map(|name| (name.clone(), Value::Null))
            .collect();
        let schema = json!({"type":"object","properties":properties});
        assert!(
            sampled(
                &Value::Object(arguments),
                &schema,
                Some(ToolStrictness::Prefer)
            )
            .expect_err("bounded traversal")
            .contains("traversal limits")
        );
    }
}
