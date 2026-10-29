//! Framed calls with optional connection-bound ownership.

use super::{Family, Fault, connected, frame, frame_context, keys, observing, read_reply};
use tokio::io::AsyncWriteExt;

impl Family {
    /// Whether the last exchange retained its transport, without probing peer liveness.
    pub fn has_connection(&self) -> bool {
        self.stream.is_some()
    }

    /// Disable reconnection for resources owned by this exact connection.
    pub fn pin_connection(&mut self) -> Result<(), Fault> {
        self.pinned = true;
        self.check_connection()
    }

    /// Refuse a lost pinned connection, including one abandoned by cancellation.
    pub fn check_connection(&self) -> Result<(), Fault> {
        if self.pinned && self.stream.is_none() {
            return Err(Fault::Failed(
                "the reserved memory connection was lost; automatic reconnection is disabled"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Call under `patience`, refreshing a refused key only on the same live connection.
    pub async fn call_within(
        &mut self,
        verb: &str,
        args: Vec<serde_json::Value>,
        patience: std::time::Duration,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        let reply = self.once(verb, &args, patience).await;
        self.check_connection()?;
        if matches!(&reply, Err(Fault::Refused(why)) if why == keys::REFUSED)
            && let Some(fresh) = keys::published(&self.path)
            && self.key.as_ref().map(keys::Key::as_str) != Some(fresh.as_str())
        {
            self.key = Some(fresh);
            let reply = self.once(verb, &args, patience).await;
            self.check_connection()?;
            return reply;
        }
        reply
    }

    async fn once(
        &mut self,
        verb: &str,
        args: &[serde_json::Value],
        patience: std::time::Duration,
    ) -> Result<Vec<serde_json::Value>, Fault> {
        self.check_connection()?;
        if self.stream.is_none() {
            frame(self.wire, verb, args, self.key.as_ref())?;
            self.stream = Some(connected(&self.path).await?);
        }
        let context = self
            .stream
            .as_ref()
            .and_then(|socket| observing::capture(socket, &self.path));
        let frame = frame_context(self.wire, verb, args, self.key.as_ref(), context)?;
        let mut stream = self
            .stream
            .take()
            .ok_or_else(|| Fault::Unavailable("no memory connection".into()))?;
        tokio::time::timeout(patience, stream.write_all(&frame))
            .await
            .map_err(|_| Fault::Unavailable(format!("sending {verb}: not taken in {patience:?}")))?
            .map_err(|e| Fault::Unavailable(format!("sending {verb}: {e}")))?;
        let reply = tokio::time::timeout(
            patience,
            read_reply(&mut stream, &mut self.scratch, verb, &self.path),
        )
        .await
        .map_err(|_| Fault::Unavailable(format!("{verb}: no answer in {patience:?}")))?;
        if matches!(&reply, Ok(_) | Err(Fault::Refused(_) | Fault::Failed(_))) {
            self.stream = Some(stream);
        }
        reply
    }
}

#[cfg(test)]
mod tests;
