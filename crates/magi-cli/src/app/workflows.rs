use super::App;
use magi_proto::{UiCommand, workflow::Summary};

impl App {
    pub fn workflow_request(&mut self) -> Option<UiCommand> {
        if self.view_only
            || self.workflows_requested
            || self.editor.cursor().0 != 0
            || !self
                .editor
                .lines()
                .first()
                .is_some_and(|line| line.starts_with("/skill "))
        {
            return None;
        }
        Some(self.refresh_workflows())
    }

    pub fn refresh_workflows(&mut self) -> UiCommand {
        self.clear_workflows();
        self.workflows_requested = true;
        UiCommand::RefreshWorkflows
    }

    pub(super) fn clear_workflows(&mut self) {
        self.workflows.clear();
        self.workflows_requested = false;
        if matches!(&self.overlay, Some(magi_tui::overlay::Overlay::Completion(found)) if found.kind == magi_tui::complete::Kind::Skill)
        {
            self.overlay = None;
        }
    }

    pub(super) fn listed_workflows(
        &mut self,
        entries: Vec<Summary>,
        warnings: Vec<String>,
        error: Option<String>,
    ) {
        let invalid = entries.iter().filter(|entry| entry.error.is_some()).count();
        if error.is_none() && entries.is_empty() {
            self.show_notice("No workflows found in the configured roots.".into());
        } else if invalid > 0 {
            self.show_notice(format!(
                "Workflow catalog: {invalid} invalid entries; /skill <name> reports details."
            ));
        }
        self.workflows = if error.is_some() { Vec::new() } else { entries };
        self.workflows_requested = true;
        for warning in warnings.into_iter().chain(error) {
            self.show_notice(warning);
        }
        let (row, col) = self.editor.cursor();
        if row != 0
            || self
                .overlay
                .as_ref()
                .is_some_and(magi_tui::overlay::Overlay::is_picker)
        {
            return;
        }
        if let Some(line) = self.editor.lines().first()
            && line.starts_with("/skill ")
        {
            self.overlay = magi_tui::complete::resolve_workflows(
                line,
                col,
                &|_| Vec::new(),
                &|_| Vec::new(),
                &self.workflows,
            )
            .map(Into::into);
        }
    }
}

#[cfg(test)]
mod tests;
