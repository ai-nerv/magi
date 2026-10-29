use super::{Cancel, Catalog};
use crate::skills::{Client, Diagnostic, Roots};
use magi_proto::{Cursor, HarnessEvent, workflow::Summary};

pub(super) const MOST: usize = 8 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum Response {
    Admitted(Vec<String>),
    Listed(Box<HarnessEvent>),
}

impl Response {
    pub(crate) fn events(self, cursor: Cursor) -> Vec<HarnessEvent> {
        match self {
            Self::Admitted(warnings) => warnings
                .into_iter()
                .map(|text| HarnessEvent::Noticed { cursor, text })
                .collect(),
            Self::Listed(event) => vec![*event],
        }
    }
}

pub(super) async fn discover(
    catalog: &Catalog,
    cancel: &Cancel,
) -> Result<(Client, crate::skills::Catalog), String> {
    let env = |name: &str| {
        catalog
            .environ
            .get(name)
            .map(std::ffi::OsString::from)
            .or_else(|| std::env::var_os(name))
            .filter(|value| !value.is_empty())
    };
    let config = env("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| env("HOME").map(|home| std::path::PathBuf::from(home).join(".config")))
        .ok_or("workflow discovery needs XDG_CONFIG_HOME or HOME")?;
    let roots = Roots::new(&catalog.cwd, catalog.workflow_workspace.as_deref(), &config)
        .map_err(|why| why.to_string())?;
    let client = Client::new(catalog.tooling.clone(), catalog.environ.clone());
    let selected = crate::skills::Catalog::discover(&client, &roots, cancel)
        .await
        .map_err(|why| why.to_string())?;
    Ok((client, selected))
}

pub(super) fn warnings(catalog: &crate::skills::Catalog) -> Vec<String> {
    catalog
        .diagnostics()
        .iter()
        .map(|diagnostic| match diagnostic {
            Diagnostic::Duplicate {
                name,
                selected,
                shadowed,
            } => format!(
                "Workflow {name}: using {:?}; duplicate in {:?} ignored",
                selected.class, shadowed.class
            ),
            Diagnostic::InvalidNames { source, count } => format!(
                "Workflow catalog {:?}: {count} invalid names ignored",
                source.class
            ),
        })
        .collect()
}

pub(super) fn response(catalog: &crate::skills::Catalog) -> Result<Response, String> {
    let event = HarnessEvent::WorkflowsListed {
        entries: catalog
            .entries()
            .map(|entry| Summary {
                name: entry.name.clone(),
                description: entry
                    .metadata
                    .as_ref()
                    .map(|metadata| metadata.description.clone())
                    .unwrap_or_default(),
                source: entry.source.class,
                error: entry.metadata.as_ref().err().map(ToString::to_string),
            })
            .collect(),
        warnings: warnings(catalog),
        error: None,
    };
    if serde_json::to_vec(&event)
        .map_err(|why| why.to_string())?
        .len()
        > MOST
    {
        return Err("workflow catalog response exceeds its byte limit".into());
    }
    Ok(Response::Listed(Box::new(event)))
}
