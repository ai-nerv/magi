//! Session-wide catalog refresh, independent of foreground turns.

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub(crate) struct Models {
    pub catalog: RwLock<crate::catalog::Catalog>,
    fetching: Mutex<()>,
}

impl Models {
    pub fn new(catalog: crate::catalog::Catalog) -> Self {
        Self {
            catalog: RwLock::new(catalog),
            fetching: Mutex::new(()),
        }
    }

    pub async fn refresh(&self, session: &Arc<Mutex<crate::Session>>) {
        let Ok(_fetching) = self.fetching.try_lock() else {
            return;
        };
        let program = self.catalog.read().await.mind.clone();
        let result = crate::broker::refresh_cards(&program).await;
        let mut catalog = self.catalog.write().await;
        let warning = match result {
            Ok((mut cards, failed)) => {
                for provider in &failed {
                    cards.retain(|card| &card.provider != provider);
                    cards.extend(
                        catalog
                            .cards
                            .iter()
                            .filter(|card| &card.provider == provider)
                            .cloned(),
                    );
                }
                catalog.cards = cards;
                (!failed.is_empty()).then(|| {
                    format!(
                        "Model refresh failed for {}; keeping their previous models.",
                        failed.join(", ")
                    )
                })
            }
            Err(message) => Some(message),
        };
        let choices = catalog.choices();
        let mut held = session.lock().await;
        held.set_choices(choices.clone());
        let _ = held
            .publisher()
            .send(magi_proto::HarnessEvent::ModelsRefreshed { choices, warning });
    }
}

#[cfg(test)]
#[path = "refreshing/tests.rs"]
mod tests;
