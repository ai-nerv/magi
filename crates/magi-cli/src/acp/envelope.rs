use agent_client_protocol_schema::v1::{Error, RequestId};
use serde_json::{Value, json};

pub(super) enum Incoming {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: RequestId,
        outcome: Result<Value, Value>,
    },
}

pub(super) fn parse(value: Value) -> Result<Incoming, ()> {
    let Value::Object(mut object) = value else {
        return Err(());
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(());
    }
    let id = object
        .remove("id")
        .map(|value| {
            if value.as_str().is_some_and(|id| id.len() > 256) {
                return Err(());
            }
            serde_json::from_value::<RequestId>(value).map_err(|_| ())
        })
        .transpose()?;
    let result = object.contains_key("result");
    let error = object.contains_key("error");
    match object.remove("method") {
        None if id.is_some() && result != error && !object.contains_key("params") => {
            let id = id.ok_or(())?;
            let outcome = if result {
                Ok(object.remove("result").ok_or(())?)
            } else {
                let error = object.remove("error").ok_or(())?;
                serde_json::from_value::<Error>(error.clone()).map_err(|_| ())?;
                Err(error)
            };
            Ok(Incoming::Response { id, outcome })
        }
        Some(Value::String(method))
            if !method.is_empty() && method.len() <= 256 && !result && !error =>
        {
            let params = object.remove("params").unwrap_or(Value::Null);
            if !params.is_null() && !params.is_object() && !params.is_array() {
                return Err(());
            }
            Ok(match id {
                Some(id) => Incoming::Request { id, method, params },
                None => Incoming::Notification { method, params },
            })
        }
        _ => Err(()),
    }
}

pub(super) fn failure(id: Option<RequestId>, error: Error) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": error})
}
