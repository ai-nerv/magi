//! The one check every request passes before it is sent: `L = min(floor(C * SHARE), C - R) - M`,
//! admitting an input within `L` whose `input + R + M` is within `C`. Counted over the payload
//! that will actually go — instructions, declarations and every message. It measures what was
//! chosen and never chooses.

use magi_model::Context;

/// The share of a model's window one request's complete input may occupy.
pub const SHARE: f64 = 0.50;

/// Held back against the difference between an estimate and what a provider counts.
pub const MARGIN: u64 = 0;

/// Whether the boundary refuses what does not fit, or measures it and lets it go. Enforcing since
/// the memory layer plans against this same limit; every request is counted either way.
pub const ENFORCING: bool = true;

/// How this count was arrived at. See [`crate::laying::COUNTING`].
pub const COUNTING: &str = crate::laying::COUNTING;

/// What the boundary saw, as the `:context` view and the cost surface read it. Named rather than
/// derived at each call site, so what is shown and what was done cannot drift apart.
#[must_use]
pub fn reported(admission: Admission, outcome: &str) -> magi_proto::HarnessEvent {
    let (counted, room) = match admission {
        Admission::Admitted { counted, room } | Admission::Over { counted, room } => {
            (counted, Some(room))
        }
        Admission::Blocked(_) => (0, None),
    };
    magi_proto::HarnessEvent::RequestAdmitted {
        capacity: room.map_or(0, |room| room.capacity),
        limit: room.map_or(0, |room| room.limit),
        reply: room.map_or(0, |room| room.reply),
        margin: room.map_or(MARGIN, |room| room.margin),
        counted,
        counting: COUNTING.to_owned(),
        outcome: outcome.to_owned(),
        why: refusal(admission),
    }
}

/// Why nothing can be sent for this model at all, whatever the request holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    UnknownWindow,
    ReplyFillsWindow { window: u64, reply: u64 },
    MarginFillsRoom { room: u64, margin: u64 },
}

/// What one request may occupy: `C`, `H`, `R`, `M` and the `L` they produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Room {
    pub capacity: u64,
    pub ceiling: u64,
    pub reply: u64,
    pub margin: u64,
    pub limit: u64,
}

/// What a complete, counted request came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Admitted { counted: u64, room: Room },
    Over { counted: u64, room: Room },
    Blocked(Blocked),
}

impl Admission {
    #[must_use]
    pub const fn admitted(self) -> bool {
        matches!(self, Self::Admitted { .. })
    }

    /// How far over the limit it was, and zero for anything else.
    #[must_use]
    pub const fn over_by(self) -> u64 {
        match self {
            Self::Over { counted, room } => counted.saturating_sub(room.limit),
            _ => 0,
        }
    }
}

impl Room {
    /// The room a model leaves, or why it leaves none.
    ///
    /// # Errors
    /// [`Blocked`] when the window is unknown or the reservation and margin fill it.
    pub fn of(window: Option<u64>, reply: u64, margin: u64) -> Result<Self, Blocked> {
        let Some(capacity) = window.filter(|window| *window > 0) else {
            return Err(Blocked::UnknownWindow);
        };
        let ceiling = share_of(capacity, SHARE);
        let Some(after_reply) = capacity.checked_sub(reply).filter(|left| *left > 0) else {
            return Err(Blocked::ReplyFillsWindow {
                window: capacity,
                reply,
            });
        };
        let room = ceiling.min(after_reply);
        let Some(limit) = room.checked_sub(margin).filter(|left| *left > 0) else {
            return Err(Blocked::MarginFillsRoom { room, margin });
        };
        Ok(Self {
            capacity,
            ceiling,
            reply,
            margin,
            limit,
        })
    }

    /// Whether a complete input of `count` may be sent: within `L`, and with `R` and `M` still
    /// inside `C`.
    #[must_use]
    pub fn admits(self, count: u64) -> bool {
        let whole = count.saturating_add(self.reply).saturating_add(self.margin);
        count <= self.limit && whole <= self.capacity
    }
}

/// A share of a window, rounded down and never more than the window.
fn share_of(capacity: u64, share: f64) -> u64 {
    let taken = (capacity as f64 * share).floor();
    if taken >= u64::MAX as f64 {
        return capacity;
    }
    (taken as u64).min(capacity)
}

/// What the complete request comes to: instructions, declarations and every message.
#[must_use]
pub fn counted(context: &Context) -> u64 {
    let system = context
        .system
        .as_deref()
        .map_or(0, magi_model::estimate::tokens);
    let tools = serde_json::to_string(&context.tools)
        .map_or(0, |tools| magi_model::estimate::tokens(&tools));
    let body = serde_json::to_string(&context.messages)
        .map_or(0, |body| magi_model::estimate::tokens(&body));
    system.saturating_add(tools).saturating_add(body)
}

/// Measure `context` against what the model leaves, and say whether it may go.
#[must_use]
pub fn admit(context: &Context, window: Option<u64>, reply: u64) -> Admission {
    let room = match Room::of(window, reply, MARGIN) {
        Ok(room) => room,
        Err(why) => return Admission::Blocked(why),
    };
    let counted = counted(context);
    if room.admits(counted) {
        Admission::Admitted { counted, room }
    } else {
        Admission::Over { counted, room }
    }
}

/// What to tell a person when a request was not sent, in their terms rather than the arithmetic's.
#[must_use]
pub fn refusal(admission: Admission) -> String {
    match admission {
        Admission::Admitted { .. } => String::new(),
        Admission::Over { counted, room } => format!(
            "This request came to about {counted} tokens, and {} leaves {} for one \
             ({COUNTING}). Nothing was sent. The memory layer decides what to leave out; if it \
             is not answering, its share of the window cannot be planned.",
            room.capacity, room.limit
        ),
        Admission::Blocked(Blocked::UnknownWindow) => {
            "This model never reported how much it can read, so there is no window to keep a \
             request inside. Nothing was sent."
                .to_owned()
        }
        Admission::Blocked(Blocked::ReplyFillsWindow { window, reply }) => format!(
            "This model reads {window} tokens and is being asked to answer in up to {reply}, \
             which leaves no room for a question. Nothing was sent."
        ),
        Admission::Blocked(Blocked::MarginFillsRoom { room, margin }) => format!(
            "The safety margin of {margin} tokens is wider than the {room} this model leaves. \
             Nothing was sent."
        ),
    }
}

#[cfg(test)]
#[path = "admitting/tests.rs"]
mod tests;
