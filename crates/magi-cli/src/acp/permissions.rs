use std::{collections::VecDeque, time::Duration};

use agent_client_protocol_schema::v1::{
    PermissionOption, PermissionOptionKind, RequestId, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ToolCallUpdate,
};
use anyhow::{Context, Result, bail};
use magi_proto::{
    ToolCallId, UiCommand,
    permit::{Action, Decision, Lifetime, Scope},
};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::events::Identity;

const PENDING: usize = 16;
const RETIRED: usize = 64;
const PATIENCE: Duration = Duration::from_secs(240);

#[derive(Default)]
pub(super) struct Permissions {
    pending: Vec<Pending>,
    retired: VecDeque<RequestId>,
}

struct Pending {
    request: RequestId,
    identity: Identity,
    approval: ToolCallId,
    call: String,
    choices: Vec<(String, Decision)>,
    deadline: Instant,
}

pub(super) struct Resolution {
    pub identity: Identity,
    pub command: UiCommand,
    pub frame: Value,
}

impl Pending {
    fn resolve(self, decision: Decision) -> Resolution {
        let status = if decision == Decision::Deny {
            "failed"
        } else {
            "completed"
        };
        Resolution {
            frame: update(
                &self.identity.external,
                json!({
                    "sessionUpdate":"tool_call_update", "toolCallId":self.call, "status":status
                }),
            ),
            identity: self.identity,
            command: UiCommand::Permit {
                id: self.approval,
                decision,
            },
        }
    }
}

impl Permissions {
    pub(super) fn open(
        &mut self,
        identity: &Identity,
        approval: ToolCallId,
        tool: String,
        action: Action,
        offers: Vec<Scope>,
        advice: Option<magi_proto::judging::Advice>,
    ) -> Result<Vec<Value>> {
        if self.pending.len() >= PENDING
            || offers.len() > 16
            || tool.len() > 256
            || approval.to_string().len() > 256
            || serde_json::to_vec(&(&action, &offers, &advice))?.len() > 64 * 1024
        {
            bail!("native permission bounds exceeded");
        }
        if self
            .pending
            .iter()
            .any(|pending| pending.identity == *identity && pending.approval == approval)
        {
            bail!("duplicate native approval");
        }
        let request = RequestId::Str(format!("magi-permission-{}", magi_ipc::random_hex(16)?));
        if self
            .pending
            .iter()
            .any(|pending| pending.request == request)
            || self.retired.contains(&request)
        {
            bail!("permission request identity collision");
        }
        let call = format!("magi-approval-{approval}");
        let mut choices = vec![("deny".to_owned(), Decision::Deny)];
        let mut options = vec![PermissionOption::new(
            "deny",
            "Refuse this operation",
            PermissionOptionKind::RejectOnce,
        )];
        for (index, scope) in offers.into_iter().enumerate() {
            let once = scope == Scope::Once;
            let id = format!("allow-{index}");
            let label = if once {
                "Allow this action once".to_owned()
            } else {
                format!("Allow for this session only: {}", scope.label(&action))
            };
            options.push(PermissionOption::new(
                id.clone(),
                label,
                if once {
                    PermissionOptionKind::AllowOnce
                } else {
                    PermissionOptionKind::AllowAlways
                },
            ));
            choices.push((
                id,
                Decision::Allow {
                    scope,
                    lifetime: Lifetime::Session,
                },
            ));
        }
        let description = format!("{tool}: approve {} {}", action.verb(), action.subject());
        let details = json!({"action":action, "advice":advice});
        let tool_call: ToolCallUpdate = serde_json::from_value(json!({
            "toolCallId":call, "title":description, "status":"pending", "rawInput":details
        }))?;
        let params = RequestPermissionRequest::new(identity.external.clone(), tool_call, options);
        let frames = vec![
            update(
                &identity.external,
                json!({
                    "sessionUpdate":"tool_call", "toolCallId":call, "title":description,
                    "status":"pending", "rawInput":details
                }),
            ),
            json!({"jsonrpc":"2.0", "id":request, "method":"session/request_permission", "params":params}),
        ];
        self.pending.push(Pending {
            request,
            identity: identity.clone(),
            approval,
            call,
            choices,
            deadline: Instant::now() + PATIENCE,
        });
        Ok(frames)
    }

    pub(super) fn answer(
        &mut self,
        id: &RequestId,
        outcome: &std::result::Result<Value, Value>,
    ) -> Result<Option<Resolution>> {
        if let Some(index) = self.retired.iter().position(|retired| retired == id) {
            self.retired.remove(index);
            return Ok(None);
        }
        let index = self
            .pending
            .iter()
            .position(|pending| &pending.request == id)
            .context("unsolicited ACP response")?;
        let pending = self
            .pending
            .get(index)
            .context("missing pending permission")?;
        if pending.deadline <= Instant::now() {
            return Ok(Some(self.pending.remove(index).resolve(Decision::Deny)));
        }
        let decision = match outcome {
            Err(_) => Decision::Deny,
            Ok(value) => selected(value)?.map_or(Ok(Decision::Deny), |selected| {
                pending
                    .choices
                    .iter()
                    .find(|(id, _)| *id == selected)
                    .map(|(_, decision)| decision.clone())
                    .context("unoffered permission choice")
            })?,
        };
        Ok(Some(self.pending.remove(index).resolve(decision)))
    }

    pub(super) fn retire(&mut self, identity: &Identity) -> Vec<Resolution> {
        self.remove(|pending| pending.identity == *identity)
    }

    pub(super) fn expire(&mut self) -> Vec<Resolution> {
        let now = Instant::now();
        self.remove(|pending| pending.deadline <= now)
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|pending| pending.deadline).min()
    }

    fn remove(&mut self, matches: impl Fn(&Pending) -> bool) -> Vec<Resolution> {
        let mut removed = Vec::new();
        let mut index = 0;
        while index < self.pending.len() {
            if self.pending.get(index).is_some_and(&matches) {
                let pending = self.pending.remove(index);
                if self.retired.len() == RETIRED {
                    self.retired.pop_front();
                }
                self.retired.push_back(pending.request.clone());
                removed.push(pending.resolve(Decision::Deny));
            } else {
                index += 1;
            }
        }
        removed
    }
}

fn selected(value: &Value) -> Result<Option<String>> {
    if serde_json::to_vec(value)?.len() > 8192 {
        bail!("permission response too large");
    }
    let object = value
        .as_object()
        .context("permission response is not an object")?;
    if object
        .keys()
        .any(|key| !matches!(key.as_str(), "outcome" | "_meta"))
    {
        bail!("unknown permission response fields");
    }
    let outcome = object
        .get("outcome")
        .and_then(Value::as_object)
        .context("missing permission outcome")?;
    let allowed: &[&str] = match outcome.get("outcome").and_then(Value::as_str) {
        Some("cancelled") => &["outcome"],
        Some("selected") => &["outcome", "optionId", "_meta"],
        _ => bail!("unknown permission outcome"),
    };
    if outcome.keys().any(|key| !allowed.contains(&key.as_str())) {
        bail!("unknown permission outcome fields");
    }
    match serde_json::from_value::<RequestPermissionResponse>(value.clone())?.outcome {
        RequestPermissionOutcome::Cancelled => Ok(None),
        RequestPermissionOutcome::Selected(selected) => Ok(Some(selected.option_id.to_string())),
        _ => bail!("unsupported permission outcome"),
    }
}

fn update(session: &str, update: Value) -> Value {
    json!({"jsonrpc":"2.0", "method":"session/update", "params":{"sessionId":session, "update":update}})
}

#[cfg(test)]
mod tests;
