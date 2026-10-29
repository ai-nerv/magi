use super::{Client, HarnessEvent};
use std::time::Duration;

impl Client {
    pub(super) async fn began(&mut self) {
        let event = tokio::time::timeout(Duration::from_secs(5), self.next())
            .await
            .expect("start boundary deadline");
        assert!(
            matches!(
                event,
                HarnessEvent::TurnStarted {
                    owner: 1,
                    request: None
                }
            ),
            "{event:?}"
        );
    }

    pub(super) async fn settled(&mut self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let HarnessEvent::TurnFinished { owner } = self.next().await {
                    assert_eq!(owner, 1);
                    break;
                }
            }
        })
        .await
        .expect("settlement deadline");
    }
}
