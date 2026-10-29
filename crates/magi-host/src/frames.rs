//! Session-local, fixed-size observations of provider-neutral request frames.

use magi_model::Context;
use magi_proto::caching::Invalidation;
use magi_proto::laying::{Layout, Slot};
use serde::Serialize;
use sha2::{Digest, Sha256};

type Fingerprint = [u8; 32];

pub(crate) struct Request<'a> {
    pub mind: &'a str,
    pub model: &'a str,
    pub wants: &'a magi_proto::ask::Wants,
}

#[derive(Default)]
pub(crate) struct Frames {
    previous: Option<Frame>,
    resumed: bool,
    provider: Option<magi_model::RequestFrame>,
    provider_seen: bool,
}

struct Frame {
    tools: Fingerprint,
    model: Fingerprint,
    rules: Fingerprint,
    options: Fingerprint,
    summary: Fingerprint,
    messages: Fingerprint,
    count: usize,
    summarised: bool,
}

struct Sink(Sha256);

impl std::io::Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn fingerprint(value: &impl Serialize) -> Option<Fingerprint> {
    let mut sink = Sink(Sha256::new());
    serde_json::to_writer(&mut sink, value).ok()?;
    Some(sink.0.finalize().into())
}

fn texts<'a>(values: impl Iterator<Item = &'a str>) -> Option<Fingerprint> {
    let mut sink = Sink(Sha256::new());
    for value in values {
        serde_json::to_writer(&mut sink, value).ok()?;
        sink.0.update(b"\n");
    }
    Some(sink.0.finalize().into())
}

impl Frame {
    fn of(
        context: &Context,
        backend: Request<'_>,
        layout: Option<&Layout>,
        entries: &[magi_proto::Entry],
    ) -> Option<Self> {
        let slots = layout.map_or(&[][..], |layout| layout.slots.as_slice());
        let rules = slots.iter().filter_map(|slot| match slot {
            Slot::Rules { text, .. } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        });
        let summaries = slots.iter().filter_map(|slot| match slot {
            Slot::Summary { text, .. } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        });
        let legacy = entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                magi_proto::Entry::Compaction { summary, .. } => Some(summary.as_str()),
                _ => None,
            })
            .filter(|_| {
                !slots
                    .iter()
                    .any(|slot| matches!(slot, Slot::Summary { .. }))
            });
        let summarised = summaries.clone().next().is_some() || legacy.is_some();
        Some(Self {
            tools: fingerprint(&context.tools)?,
            model: fingerprint(&(&backend.mind, &backend.model))?,
            rules: fingerprint(&(fingerprint(&context.system)?, texts(rules)?))?,
            options: fingerprint(&backend.wants)?,
            summary: texts(summaries.chain(legacy))?,
            messages: fingerprint(&context.messages)?,
            count: context.messages.len(),
            summarised,
        })
    }
}

impl Frames {
    pub(crate) fn resumed() -> Self {
        Self {
            previous: None,
            resumed: true,
            ..Self::default()
        }
    }

    pub(crate) fn observe(
        &mut self,
        context: &Context,
        backend: Request<'_>,
        layout: Option<&Layout>,
        entries: &[magi_proto::Entry],
    ) -> Vec<Invalidation> {
        use Invalidation::{LayoutChanged, LayoutReduced, ObservationUnavailable};
        self.provider_seen = false;
        let mut reasons = Vec::new();
        if std::mem::take(&mut self.resumed) {
            reasons.push(Invalidation::ResumeIncompatible);
        }
        let Some(next) = Frame::of(context, backend, layout, entries) else {
            self.previous = None;
            reasons.push(ObservationUnavailable);
            return reasons;
        };
        if let Some(last) = &self.previous {
            for (changed, reason) in [
                (next.tools != last.tools, Invalidation::ToolsChanged),
                (next.model != last.model, Invalidation::ModelChanged),
                (next.rules != last.rules, Invalidation::RulesChanged),
                (next.options != last.options, Invalidation::OptionsChanged),
                (
                    next.summarised && next.summary != last.summary,
                    Invalidation::SummaryActivated,
                ),
            ] {
                if changed {
                    reasons.push(reason);
                }
            }
            let prefix = context
                .messages
                .get(..last.count)
                .and_then(|messages| fingerprint(&messages));
            if next.count < last.count {
                reasons.push(LayoutReduced);
            } else if let Some(prefix) = prefix {
                if prefix != last.messages {
                    reasons.push(LayoutChanged);
                }
            } else {
                reasons.push(ObservationUnavailable);
            }
        }
        self.previous = Some(next);
        reasons
    }

    pub(crate) fn provider(&mut self, frame: magi_model::RequestFrame) -> Vec<Invalidation> {
        self.provider_seen = true;
        if frame.version != 1 {
            self.provider = None;
            return vec![Invalidation::ObservationUnavailable];
        }
        let mut reasons = Vec::new();
        if let Some(last) = self.provider {
            if last.dialect != frame.dialect {
                reasons.push(Invalidation::DialectChanged);
            }
            if last.policy != frame.policy {
                reasons.push(Invalidation::ProviderPolicyChanged);
            }
        }
        if frame.omitted_images != 0 {
            reasons.push(Invalidation::ImagesAdapted);
        }
        self.provider = Some(frame);
        reasons
    }

    pub(crate) fn next_attempt(&mut self) {
        self.provider_seen = false;
    }

    pub(crate) fn finish_provider(&mut self) -> Option<Vec<Invalidation>> {
        if self.provider_seen {
            return None;
        }
        self.provider = None;
        Some(vec![Invalidation::ObservationUnavailable])
    }
}

#[cfg(test)]
mod tests;
