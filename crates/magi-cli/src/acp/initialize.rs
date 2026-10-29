use agent_client_protocol_schema::ProtocolVersion;
use agent_client_protocol_schema::v1::{
    AgentCapabilities, Error, Implementation, InitializeRequest, InitializeResponse, RequestId,
};
use serde_json::{Value, json};

use super::envelope::failure;

#[derive(Default)]
pub(super) struct State {
    client: Option<InitializeRequest>,
}

impl State {
    pub(super) fn ready(&self) -> bool {
        self.client.is_some()
    }

    pub(super) fn display(&self) -> super::display::Policy {
        super::display::Policy {
            compaction: self.client.as_ref().is_some_and(|client| {
                client
                    .client_capabilities
                    .session
                    .as_ref()
                    .is_some_and(|session| session.compaction.is_some())
            }),
        }
    }

    pub(super) fn request(&mut self, id: RequestId, method: &str, params: Value) -> Value {
        if method != "initialize" {
            let error = if self.ready() {
                Error::method_not_found()
            } else {
                Error::new(-32000, "Initialize first")
            };
            return failure(Some(id), error);
        }
        if self.ready() {
            return failure(Some(id), Error::invalid_request());
        }
        let Ok(client) = strict(params) else {
            return failure(Some(id), Error::invalid_params());
        };
        let response = InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(AgentCapabilities::default().load_session(true))
            .agent_info(Implementation::new("magi", env!("CARGO_PKG_VERSION")));
        self.client = Some(client);
        json!({"jsonrpc": "2.0", "id": id, "result": response})
    }
}

fn strict(value: Value) -> Result<InitializeRequest, ()> {
    if !value.is_object() {
        return Err(());
    }
    object(&value, "_meta", true)?;
    if let Some(info) = object(&value, "clientInfo", true)? {
        for key in ["name", "version"] {
            if !info.get(key).is_some_and(Value::is_string) {
                return Err(());
            }
        }
        if info
            .get("title")
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Err(());
        }
    }
    if let Some(caps) = object(&value, "clientCapabilities", false)? {
        boolean(caps, "terminal")?;
        if let Some(fs) = object(caps, "fs", false)? {
            boolean(fs, "readTextFile")?;
            boolean(fs, "writeTextFile")?;
        }
        if let Some(auth) = object(caps, "auth", false)? {
            boolean(auth, "terminal")?;
        }
        if let Some(session) = object(caps, "session", true)? {
            for key in ["compaction", "configOptions", "notices"] {
                object(session, key, true)?;
            }
            if let Some(config) = object(session, "configOptions", true)? {
                object(config, "boolean", true)?;
            }
        }
        if let Some(elicitation) = object(caps, "elicitation", true)? {
            for key in ["form", "url"] {
                object(elicitation, key, true)?;
            }
        }
    }
    serde_json::from_value(value).map_err(|_| ())
}

fn object<'a>(parent: &'a Value, key: &str, nullable: bool) -> Result<Option<&'a Value>, ()> {
    match parent.get(key) {
        None => Ok(None),
        Some(Value::Null) if nullable => Ok(None),
        Some(value) if value.is_object() => Ok(Some(value)),
        _ => Err(()),
    }
}

fn boolean(parent: &Value, key: &str) -> Result<(), ()> {
    if parent.get(key).is_none_or(Value::is_boolean) {
        Ok(())
    } else {
        Err(())
    }
}
