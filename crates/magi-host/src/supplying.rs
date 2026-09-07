//! What a turn is told, from everything that has something to say.
//!
//! **One supplier is a function call; two is a plane.** Memory reached a turn through one path
//! with one supplier named in Rust: [`crate::turn`] asked balthasar and handed what came back
//! straight to a renderer. That is right until something else has context to offer — a code
//! index, a git history, a project's own notes — and then the questions that were never asked
//! all arrive at once. Which of them gets the budget? Which said this? What did not fit?
//!
//! This module answers those three for any number of suppliers, and balthasar becomes *a*
//! supplier rather than *the* one.
//!
//! # Three rules, taken from a system that has more than one supplier already
//!
//! **Every block says where it came from.** A [`Block`] cannot be built without a citation — not
//! "should have one", cannot. With one supplier the question never arose, because everything in
//! the block came from the same place and the frame around it said so. With two, a model reading
//! a claim has no way to weigh it without knowing whether a memory layer asserted it or an
//! indexer computed it, and a person debugging has no way to find where it came from.
//!
//! **Nothing is dropped silently.** Packing to a budget means something does not fit, and the
//! caller is told what: [`Packed::dropped`] is the report. The renderer this replaces broke out
//! of its loop when the budget ran out and said nothing to anybody — so a memory layer whose
//! answers were all slightly too long looked exactly like one that found nothing.
//!
//! **A cost that cannot be derived is absent, not zero.** A block's cost is an `Option`, and
//! `None` means "nobody could work this out", which is a different claim from "free". Treating
//! the second as the first is how a block with unknown cost gets packed first and every time.
//!
//! # What is deliberately not here
//!
//! **There is no `Supplier` trait.** There is one implementor, and a trait with one implementor
//! is a shape asserted rather than discovered — the mistake this project has written down about
//! somebody else's plugin port. What is here is the *data* two suppliers would have to agree on,
//! which is the part that makes the second one cheap. The trait arrives with it.

use magi_model::{Content, Message, Role};

/// Roughly four characters to the token, which is what the rest of the host estimates with.
pub(crate) const PER_TOKEN: usize = 4;

/// One piece of context somebody offered, and where it came from.
///
/// Built through [`Block::new`], because the citation is the field that cannot be optional and a
/// public constructor is the only place that can be enforced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// What the model reads.
    text: String,
    /// Where it came from, in words a person would recognise.
    citation: String,
    /// What it costs, or `None` when that is not derivable — which is not the same as free.
    cost: Option<usize>,
    /// Whether this is offered as current truth or merely as something on record.
    asserted: bool,
}

/// Why a block could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Uncitable {
    /// A block with nothing in it costs tokens to say nothing.
    #[error("a context block with no text")]
    Empty,
    /// The rule this type exists for.
    #[error("a context block from nowhere: every block says where it came from")]
    Uncited,
}

impl Block {
    /// One block, from somewhere.
    ///
    /// # Errors
    /// When the text is empty, or the citation is. A block that cannot say where it came from is
    /// refused at construction rather than rendered without provenance — with two suppliers in
    /// the same message, an uncited line is one the reader cannot weigh and the author cannot
    /// find again.
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

    /// What the model reads.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Where it came from.
    #[must_use]
    pub fn citation(&self) -> &str {
        &self.citation
    }

    /// Whether it is offered as current truth.
    #[must_use]
    pub fn is_asserted(&self) -> bool {
        self.asserted
    }

    /// What writing this line actually costs, in characters.
    ///
    /// The supplier's own number when it gave one, and the rendered length otherwise. A supplier
    /// that cannot cost its own block does not get it packed for free — it gets it measured.
    fn charged(&self) -> usize {
        self.cost
            .map_or_else(|| self.rendered().len(), |tokens| tokens * PER_TOKEN)
    }

    /// The line as it goes into the message.
    fn rendered(&self) -> String {
        format!("- {}\n", self.text.trim())
    }
}

/// What one supplier offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// What it is called, and what its blocks are cited to.
    pub from: String,
    /// What it has to say, in the order it would like it read.
    pub blocks: Vec<Block>,
}

/// A block that did not fit, and whose it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    /// Which supplier offered it.
    pub from: String,
    /// What it said, so a reader of the log can tell whether it mattered.
    pub text: String,
}

/// What was packed, and what was left out.
#[derive(Debug, Clone, Default)]
pub struct Packed {
    /// The message to put in front of the turn, or `None` when nothing fit.
    pub message: Option<Message>,
    /// Everything that did not. Never silently empty — see the module docs.
    pub dropped: Vec<Dropped>,
}

impl Packed {
    /// Nothing offered, nothing dropped.
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

/// How much of a turn's window may be spent on everything the suppliers offered.
///
/// A tenth, and a tenth in total rather than each: the share is a property of the conversation,
/// not of how many things happen to have something to say. A second supplier must not double
/// what a turn spends on context it did not ask for.
pub(crate) const SHARE: usize = 10;

/// Pack what was offered into one message, and say what did not fit.
///
/// **The budget is shared, and unspent budget passes on.** Each supplier is allotted an equal
/// slice of the window's tenth; a supplier that offers little leaves the rest to whoever comes
/// after, in the order it was offered. Equal-then-passing is balthasar's own rule for sections
/// and it is the right one here for the same reason: a fixed share per supplier wastes the room a
/// quiet one was allotted, and no share at all lets the first one crowd out every other.
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

    // Each supplier's slice, of what is left after the frame. Recomputed as it goes rather than
    // divided once, so what one leaves unspent is actually available to the next.
    let mut dropped = Vec::new();
    let mut wrote = false;
    let mut hedged: Vec<(&str, &Block)> = Vec::new();

    for (index, offer) in offers.iter().enumerate() {
        let remaining = offers.len() - index;
        let slice = (total - spent) / remaining;
        let ceiling = spent + slice;

        let mut said = false;
        for block in &offer.blocks {
            // What is merely on record waits until every supplier's current truth has been
            // written: a hedged line from the first supplier must not cost the second one its
            // asserted ones.
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
