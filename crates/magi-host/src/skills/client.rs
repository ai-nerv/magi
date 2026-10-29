use super::{
    Error,
    wire::{Operation, Payload, Request},
};
use crate::cancel::Cancel;
use magi_tools::supplier::Tooling;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod process;
mod supervised;

const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct Client {
    tooling: Arc<Tooling>,
    environ: Arc<BTreeMap<String, String>>,
}

impl Client {
    pub fn new(tooling: Tooling, environ: BTreeMap<String, String>) -> Self {
        Self {
            tooling: Arc::new(tooling),
            environ: Arc::new(environ),
        }
    }

    pub(super) async fn request(
        &self,
        root: &Path,
        operation: Operation,
        cancel: &Cancel,
    ) -> Result<Payload, Error> {
        let deadline = Instant::now() + PATIENCE;
        self.request_verified(
            root,
            operation,
            cancel,
            deadline,
            magi_tools::supplier::verified,
        )
        .await
    }

    async fn request_verified(
        &self,
        root: &Path,
        operation: Operation,
        cancel: &Cancel,
        deadline: Instant,
        verify: impl FnOnce(&Tooling) -> Result<String, String> + Send + 'static,
    ) -> Result<Payload, Error> {
        supervised::check(cancel, deadline)?;
        let request = Request {
            version: 1,
            root: root.into(),
            operation,
        };
        let input = request.encode()?;
        let client = self.clone();
        let runtime = tokio::runtime::Handle::current();
        supervised::run(cancel, deadline, move |cancel| {
            let program = verify(&client.tooling).map_err(|_| Error::Pin)?;
            supervised::check(&cancel, deadline)?;
            let command = client.command(program);
            let output = runtime.block_on(process::exchange(command, &input, &cancel, deadline))?;
            supervised::check(&cancel, deadline)?;
            let payload = request.decode(&output)?;
            supervised::check(&cancel, deadline)?;
            Ok(payload)
        })
        .await
    }

    fn command(&self, program: String) -> std::process::Command {
        let mut command = std::process::Command::new(program);
        command.args(["workflows", "--json"]).current_dir("/");
        magi_tools::environ::apply(&mut command, &self.environ);
        magi_tools::environ::without_capability(&mut command);
        for name in self
            .environ
            .keys()
            .filter(|name| magi_tools::environ::carries_capability(name))
        {
            command.env_remove(name);
        }
        command
            .env_remove("CASPER_WORKFLOW_WORKER")
            .env_remove("CASPER_WORKFLOW_BINDING");
        command
    }
}
