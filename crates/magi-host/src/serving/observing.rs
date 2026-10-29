use crate::events::{self, ClientOutcome, Correlation, Fact, Id};

fn emit(id: Option<Id>, outcome: ClientOutcome) {
    if let Some(id) = id {
        events::emit(|| {
            (
                Correlation {
                    request_id: Some(id),
                    ..Correlation::default()
                },
                Fact::Client { outcome },
            )
        });
    }
}

pub(crate) fn refused() {
    emit(events::identity(), ClientOutcome::Refused);
}

pub(crate) struct Client {
    id: Option<Id>,
}

impl Client {
    pub(crate) fn connected() -> Self {
        let id = events::identity();
        emit(id, ClientOutcome::Connected);
        Self { id }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        emit(self.id, ClientOutcome::Disconnected);
    }
}

#[cfg(test)]
mod tests;
