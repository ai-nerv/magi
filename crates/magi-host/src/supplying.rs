//! What a turn is told, from everything that has something to say, for any number of suppliers.
//! Three rules: every [`Block`] carries a citation and cannot be built without one; nothing is
//! dropped silently, so [`Packed::dropped`] reports what did not fit; and a cost that cannot be
//! derived is `None` rather than zero, which would otherwise pack it first every time.

use magi_model::{Content, Message, Role};

/// Roughly four characters to the token, which is what the rest of the host estimates with.
pub(crate) const PER_TOKEN: usize = 4;

/// One piece of context somebody offered, and where it came from. Built through [`Block::new`],
/// which is the only place the citation can be enforced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    text: String,
    citation: String,
    /// What it costs, or `None` when that is not derivable — which is not the same as free.
    cost: Option<usize>,
    /// Whether this is offered as current truth or merely as something on record.
    asserted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Uncitable {
    #[error("a context block with no text")]
    Empty,
    #[error("a context block from nowhere: every block says where it came from")]
    Uncited,
}

impl Block {
    /// One block, from somewhere.
    ///
    /// # Errors
    /// When the text is empty, or the citation is.
    pub fn new(text: impl Into<String>, citation: impl Into<String>) -> Result<Self, Uncitable> {
        let text = text.into();
        let citation = citation.into();
        if text.trim().is_empty() {
            return Err(Uncitable::Empty);
        }
        if citation.trim().is_empty() {
            return Err(Uncitable::Uncited);
        }
        Ok(Self {
            text,
            citation,
            cost: None,
            asserted: false,
        })
    }

    /// Say what it costs, when the supplier knows.
    #[must_use]
    pub fn costing(mut self, tokens: usize) -> Self {
        self.cost = Some(tokens);
        self
    }

    /// Offer it as current truth rather than as something merely on record.
    #[must_use]
    pub fn asserted(mut self, asserted: bool) -> Self {
        self.asserted = asserted;
        self
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn citation(&self) -> &str {
        &self.citation
    }

    #[must_use]
    pub fn is_asserted(&self) -> bool {
        self.asserted
    }

    /// What writing this line actually costs, in characters: the supplier's own number when it
    /// gave one, and the rendered length otherwise.
    fn charged(&self) -> usize {
        self.cost
            .map_or_else(|| self.rendered().len(), |tokens| tokens * PER_TOKEN)
    }

    fn rendered(&self) -> String {
        format!("- {}\n", self.text.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub from: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub from: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Packed {
    /// The message to put in front of the turn, or `None` when nothing fit.
    pub message: Option<Message>,
    /// Everything that did not. Never silently empty — see the module docs.
    pub dropped: Vec<Dropped>,
}

impl Packed {
    #[must_use]
    fn nothing() -> Self {
        Self::default()
    }
}

/// What the model is told before the block, so it can tell offered context from conversation.
const PREFACE: &str = "What this project knows. This is not part of the conversation:";

/// The line that separates what is current from what is merely on record.
pub(crate) const HEDGE: &str = "Also on record, but not current enough to rely on \
                     — check before acting on any of it:";

/// How much of a turn's window may be spent on everything the suppliers offered. A tenth in total
/// rather than each: a second supplier must not double what a turn spends on context.
pub(crate) const SHARE: usize = 10;

/// Pack what was offered into one message, and say what did not fit. Each supplier gets an equal
/// slice of the window's tenth, and what one leaves unspent passes to whoever comes after, in the
/// order it was offered.
#[must_use]
pub fn pack(offers: &[Offer], window: usize) -> Packed {
    let total = window.saturating_mul(SHARE) / 100 * PER_TOKEN;
    if total == 0 || offers.is_empty() {
        return Packed::nothing();
    }

    let mut out = format!("{PREFACE}\n");
    let mut spent = out.len();
    if spent >= total {
        return Packed::nothing();
    }

    // Recomputed as it goes rather than divided once, so what one leaves unspent is available.
    let mut dropped = Vec::new();
    let mut wrote = false;
    let mut hedged: Vec<(&str, &Block)> = Vec::new();

    for (index, offer) in offers.iter().enumerate() {
        let remaining = offers.len() - index;
        let slice = (total - spent) / remaining;
        let ceiling = spent + slice;

        let mut said = false;
        for block in &offer.blocks {
            // What is merely on record waits until every supplier's current truth is written.
            if !block.is_asserted() {
                hedged.push((&offer.from, block));
                continue;
            }
            let heading = format!("{}:\n", offer.from);
            let cost = block.charged() + if said { 0 } else { heading.len() };
            if spent + cost > ceiling {
                dropped.push(Dropped {
                    from: offer.from.clone(),
                    text: block.text().to_owned(),
                });
                continue;
            }
            if !said {
                out.push_str(&heading);
                spent += heading.len();
                said = true;
            }
            out.push_str(&block.rendered());
            spent += block.charged();
            wrote = true;
        }
    }

    // Then whatever was hedged, under one heading, out of whatever is left.
    let mut under = false;
    for (from, block) in hedged {
        let heading = format!("{HEDGE}\n");
        let cost = block.charged() + if under { 0 } else { heading.len() };
        if spent + cost > total {
            dropped.push(Dropped {
                from: from.to_owned(),
                text: block.text().to_owned(),
            });
            continue;
        }
        if !under {
            out.push_str(&heading);
            spent += heading.len();
            under = true;
        }
        out.push_str(&format!("- {} ({from})\n", block.text().trim()));
        spent += block.charged();
        wrote = true;
    }

    Packed {
        message: wrote.then(|| Message {
            role: Role::User,
            content: vec![Content::Text {
                text: out.trim_end().to_owned(),
                signature: None,
            }],
            stop_reason: None,
            usage: None,
            error: None,
        }),
        dropped,
    }
}

#[cfg(test)]
#[path = "supplying/packing.rs"]
mod packing;
