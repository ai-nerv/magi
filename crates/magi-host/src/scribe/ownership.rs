//! Optional generic writer negotiation, bound to one transport and transcript.

use super::{Scribe, rows};
use magi_ipc::family::Fault;
use serde_json::json;

pub(super) enum Writer {
    Unchecked,
    Legacy,
    Pending,
    Owned(String),
}

#[derive(serde::Deserialize)]
struct Status {
    revision: u32,
    supported: bool,
    owned: bool,
    lifetime: String,
    scope: String,
}

#[derive(serde::Deserialize)]
struct Reserved {
    revision: u32,
    owned: bool,
}

fn one<T: serde::de::DeserializeOwned>(values: Vec<serde_json::Value>) -> Result<T, Fault> {
    let [value] = values.as_slice() else {
        return Err(Fault::Failed(
            "writer control must return exactly one object".into(),
        ));
    };
    serde_json::from_value(value.clone())
        .map_err(|why| Fault::Failed(format!("invalid writer control: {why}")))
}

impl Scribe {
    pub(super) fn check_writer(&self) -> Result<(), Fault> {
        match &self.writer {
            Writer::Pending => Err(Fault::Failed(
                "writer acquisition did not complete; discard this connection".into(),
            )),
            Writer::Owned(key) => {
                if key != &self.transcript {
                    return Err(Fault::Failed(
                        "the writer belongs to another transcript".into(),
                    ));
                }
                self.family
                    .as_ref()
                    .ok_or_else(|| Fault::Failed("the writer connection was lost".into()))?
                    .check_connection()
            }
            _ => Ok(()),
        }
    }

    /// Reserve this transcript before replay or serving; require support for persistent loading.
    pub async fn reserve_writer(&mut self, required: bool) -> Result<bool, Fault> {
        self.managed_writer = true;
        self.check_writer()?;
        if matches!(self.writer, Writer::Owned(_)) {
            return Ok(true);
        }
        if matches!(self.writer, Writer::Unchecked) {
            let advertised = self.connect().await?.call("verbs", Vec::new()).await?;
            let names: Vec<_> = advertised
                .iter()
                .flat_map(rows)
                .filter_map(|row| {
                    row.get("name")
                        .or_else(|| row.get("verb"))
                        .and_then(|name| name.as_str())
                })
                .collect();
            let status = names.contains(&"writer_status");
            let writer = names.contains(&"writer");
            if status != writer {
                return Err(Fault::Failed(
                    "memory advertises an incomplete writer extension".into(),
                ));
            }
            if !writer {
                self.writer = Writer::Legacy;
            } else {
                let key = self.transcript.clone();
                let status: Status = one(self
                    .connect()
                    .await?
                    .call("writer_status", vec![json!(key), json!({"revision":1})])
                    .await?)?;
                if status.revision != 1
                    || status.lifetime != "connection"
                    || status.scope != "store-transcript"
                    || status.owned
                {
                    return Err(Fault::Failed("incompatible writer capability".into()));
                }
                if !status.supported {
                    self.writer = Writer::Legacy;
                } else {
                    self.writer = Writer::Pending;
                    let family = self
                        .family
                        .as_mut()
                        .ok_or_else(|| Fault::Failed("no writer connection".into()))?;
                    family.pin_connection()?;
                    let reserved: Reserved = one(family
                        .call_within(
                            "writer",
                            vec![json!(key), json!({"revision":1,"operation":"reserve"})],
                            magi_ipc::family::DURABLE,
                        )
                        .await?)?;
                    if reserved.revision != 1 || !reserved.owned {
                        return Err(Fault::Failed(
                            "memory did not confirm writer ownership".into(),
                        ));
                    }
                    self.writer = Writer::Owned(key);
                    return Ok(true);
                }
            }
        }
        if required {
            return Err(Fault::Refused(
                "persistent loading requires store-scoped writer reservations from the memory role"
                    .into(),
            ));
        }
        Ok(false)
    }

    pub(crate) fn matches_recording(
        &self,
        session: &magi_proto::SessionId,
        transcript: &str,
    ) -> bool {
        self.session == session.as_str() && self.transcript == transcript
    }
}

#[cfg(test)]
mod tests;
