use std::collections::BTreeSet;

use agent_client_protocol_schema::v1::{
    Error, RequestId, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};
use anyhow::{Context, Result, bail};
use magi_proto::{ModelChoice, ModelInfo, UiCommand};
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{envelope, intake, transport};

const MODELS: usize = 4096;
const BYTES: usize = 1024 * 1024;
const NAME: usize = 512;

pub(super) struct Settings {
    current: Option<String>,
    choices: Vec<SessionConfigSelectOption>,
    next: u64,
    pending: Option<Pending>,
}

struct Pending {
    id: RequestId,
    model: String,
    deadline: Instant,
    _lease: intake::Item,
}

pub(super) fn parse(value: Value) -> Result<(String, String), Error> {
    let invalid = Error::invalid_params;
    let object = value.as_object().ok_or_else(invalid)?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "sessionId" | "configId" | "value" | "type" | "_meta"
        )
    }) || object.get("type").is_some_and(|kind| kind != "select")
        || serde_json::to_vec(&value).map_err(|_| invalid())?.len() > 16 * 1024
    {
        return Err(invalid());
    }
    let request: SetSessionConfigOptionRequest =
        serde_json::from_value(value).map_err(|_| invalid())?;
    let model = request.value.as_value_id().ok_or_else(invalid)?.to_string();
    let session = request.session_id.to_string();
    if request.config_id.to_string() != "model"
        || super::history::validate_id(&session).is_err()
        || !name_valid(&model)
    {
        return Err(invalid());
    }
    Ok((session, model))
}

fn name_valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= NAME && !name.chars().any(char::is_control)
}

impl Settings {
    pub(super) fn new(model: Option<ModelInfo>, choices: Vec<ModelChoice>) -> Result<Self> {
        let mut settings = Self {
            current: None,
            choices: Vec::new(),
            next: 0,
            pending: None,
        };
        settings.catalog(choices)?;
        settings.model(model)?;
        Ok(settings)
    }

    pub(super) fn options(&self) -> Vec<SessionConfigOption> {
        let Some(current) = &self.current else {
            return Vec::new();
        };
        let mut choices = self.choices.clone();
        if !choices
            .iter()
            .any(|choice| choice.value.to_string() == *current)
        {
            choices.push(SessionConfigSelectOption::new(
                current.clone(),
                current.clone(),
            ));
        }
        vec![
            SessionConfigOption::select("model", "Model", current.clone(), choices)
                .category(SessionConfigOptionCategory::Model),
        ]
    }

    pub(super) fn model(&mut self, model: Option<ModelInfo>) -> Result<()> {
        if model.as_ref().is_some_and(|model| !name_valid(&model.name)) {
            bail!("invalid native model name");
        }
        self.current = model.map(|model| model.name);
        self.bounded()
    }

    pub(super) fn catalog(&mut self, choices: Vec<ModelChoice>) -> Result<()> {
        if choices.len() > MODELS {
            bail!("native model catalog exceeds ACP limit");
        }
        let mut names = BTreeSet::new();
        let mut options = Vec::with_capacity(choices.len());
        for choice in choices {
            if !name_valid(&choice.name)
                || choice.requirement.len() > 4096
                || !names.insert(choice.name.clone())
            {
                bail!("invalid native model catalog");
            }
            let description = (!choice.requirement.is_empty()).then_some(choice.requirement);
            options.push(
                SessionConfigSelectOption::new(choice.name.clone(), choice.name)
                    .description(description),
            );
        }
        if serde_json::to_vec(&options)?.len() > BYTES - 4096 {
            bail!("native model catalog exceeds ACP byte limit");
        }
        self.choices = options;
        self.bounded()
    }

    fn bounded(&self) -> Result<()> {
        if serde_json::to_vec(&self.options())?.len() > BYTES {
            bail!("native configuration exceeds ACP byte limit");
        }
        Ok(())
    }

    pub(super) fn notification(&self, session: &str) -> Value {
        json!({"jsonrpc":"2.0", "method":"session/update", "params":{
            "sessionId":session, "update":{
                "sessionUpdate":"config_option_update", "configOptions":self.options()
            }
        }})
    }

    pub(super) fn busy(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.deadline)
    }

    pub(super) fn select(
        &mut self,
        id: RequestId,
        model: String,
        lease: &mut Option<intake::Item>,
    ) -> Result<UiCommand, Error> {
        if self.busy() {
            return Err(Error::new(
                -32000,
                "Session has a pending configuration change",
            ));
        }
        if self.current.is_none()
            || !(self.current.as_ref() == Some(&model)
                || self
                    .choices
                    .iter()
                    .any(|choice| choice.value.to_string() == model))
        {
            return Err(Error::invalid_params());
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| Error::new(-32000, "Configuration identity exhausted"))?;
        self.pending = Some(Pending {
            id,
            model: model.clone(),
            deadline: Instant::now() + transport::DEADLINE,
            _lease: lease.take().ok_or_else(Error::internal_error)?,
        });
        Ok(UiCommand::SelectModel {
            request: self.next,
            name: model,
        })
    }

    pub(super) fn response(&self, request: u64, error: Option<String>) -> Result<Value> {
        let pending = self
            .pending
            .as_ref()
            .context("unsolicited native model receipt")?;
        if request != self.next {
            bail!("uncorrelated native model receipt");
        }
        if Instant::now() >= pending.deadline {
            bail!("native model receipt arrived after its deadline");
        }
        if error.is_some() {
            return Ok(envelope::failure(
                Some(pending.id.clone()),
                Error::new(-32000, "Native model selection refused"),
            ));
        }
        if self.current.as_ref() != Some(&pending.model) {
            bail!("native model receipt disagrees with selected model");
        }
        Ok(json!({"jsonrpc":"2.0", "id":pending.id,
            "result":SetSessionConfigOptionResponse::new(self.options())}))
    }

    pub(super) fn release(&mut self) {
        self.pending = None;
    }

    pub(super) fn lease(&mut self) -> Result<&mut intake::Item> {
        Ok(&mut self
            .pending
            .as_mut()
            .context("missing model request lease")?
            ._lease)
    }
}

#[cfg(test)]
mod tests;
