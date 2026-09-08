//! Applying one harness event to the state.
//!
//! Split out under THE RULE; the state it applies to is next door. One function, because it is
//! one decision repeated per event kind — and because it is the single funnel every event passes
//! through, which is what makes the trace a complete record rather than a sample.

use super::*;

impl App {
    /// Fold one harness event into the state.
    pub fn apply(&mut self, event: HarnessEvent) {
        self.cursor = self.cursor.max(event.cursor());
        // **Every event, before anything decides what to do with it.** The trace is a view of
        // what happened, so it has to be written where everything arrives rather than in the
        // arms that happen to be interesting -- an arm added later would otherwise be missing
        // from the timeline and nothing would say so.
        self.timeline.note(&event);
        match event {
            HarnessEvent::SessionSnapshot {
                cursor: _,
                entries,
                status,
                model,
                choices,
                thinking,
                ..
            } => {
                let unconfigured = model.is_none();
                self.model = model;
                self.model_reasons = self.model.as_ref().is_some_and(|chosen| {
                    choices.iter().any(|c| c.name == chosen.name && c.reasoning)
                });
                if !thinking.is_empty() {
                    self.thinking = thinking;
                }
                self.choices = choices;
                let empty = entries.is_empty();
                self.entries = entries;
                self.set_status(status);
                // Now that the daemon's entries have replaced ours, anything the UI knew at
                // startup can be added without the snapshot eating it.
                if let Some(text) = self.pending_notice.take() {
                    self.show_notice(text);
                }
                // Said once, on a session that has not started yet. A fresh install points at
                // a model whose key nobody has set, and the whole of what it told you was
                // `no-model` in a corner of the footer — true, and no help at all.
                if unconfigured && empty && !self.choices.is_empty() {
                    let said = self.no_model.clone().unwrap_or_else(|| {
                        "No model is configured. Type `:model` to choose one.".to_owned()
                    });
                    self.show_notice(said);
                }
            }
            HarnessEvent::UserMessage { id, text, .. } => {
                // No aside. It is context for the model and the transcript never shows it, so
                // there is nothing here for one to be.
                self.entries.push(Entry::User {
                    id,
                    text,
                    aside: String::new(),
                });
            }
            // Drawn from the session's own stream rather than appended when it landed on the
            // socket. The UI is where a message arrives and the session is where it *is*: an
            // entry the UI kept for itself was one the model never saw, so an instance could be
            // asked a question and sit there until somebody typed at it.
            HarnessEvent::MessageArrived {
                who,
                kin,
                sort,
                text,
                ..
            } => {
                self.entries.push(Entry::From {
                    who,
                    kin,
                    sort,
                    text,
                });
            }
            // Beginning a message that is already on screen means beginning it *again*: an
            // attempt streamed half an answer, failed, and the retry starts from nothing. So it
            // empties the one that is there rather than pushing a second — which is what a
            // retry mid-answer used to leave behind, two copies of the same half-message.
            HarnessEvent::AssistantStarted { id, .. } => {
                if let Some(Entry::Assistant {
                    text,
                    thinking,
                    stop_reason,
                    error,
                    ..
                }) = self.assistant_mut(&id)
                {
                    text.clear();
                    thinking.clear();
                    *stop_reason = None;
                    *error = None;
                } else {
                    self.entries.push(Entry::Assistant {
                        id,
                        text: String::new(),
                        thinking: String::new(),
                        stop_reason: None,
                        error: None,
                        signatures: magi_proto::Signatures::default(),
                        usage: magi_proto::Usage::default(),
                    });
                }
            }
            HarnessEvent::AssistantDelta {
                id, text, thinking, ..
            } => {
                if let Some(Entry::Assistant {
                    text: body,
                    thinking: reasoning,
                    ..
                }) = self.assistant_mut(&id)
                {
                    body.push_str(&text);
                    reasoning.push_str(&thinking);
                }
            }
            HarnessEvent::AssistantEnded {
                id,
                stop_reason,
                error,
                usage,
                ..
            } => {
                if let Some(Entry::Assistant {
                    stop_reason: stop,
                    error: err,
                    usage: cost,
                    ..
                }) = self.assistant_mut(&id)
                {
                    *stop = Some(stop_reason);
                    *err = error;
                    *cost = usage;
                }
            }
            // Said in the transcript, once the conversation has started. Which model answered
            // is part of the record, and a switch that changes only two dim words in the
            // footer leaves no mark on the place a reader actually reads.
            // The turn is blocked until this is answered, so it takes the screen: a picker
            // opened over whatever else was there, with the narrowest answer under the cursor.
            HarnessEvent::PermissionAsked {
                id,
                tool,
                action,
                offers,
                ..
            } => {
                let choices = offers
                    .iter()
                    .map(|scope| magi_tui::picker::Choice {
                        value: scope.label(&action),
                        detail: String::new(),
                        ready: true,
                    })
                    .chain(std::iter::once(magi_tui::picker::Choice {
                        value: "no".to_owned(),
                        detail: "refuse, and tell the model".to_owned(),
                        ready: true,
                    }))
                    .collect();
                // The call on its own rows, not in the title. A long command clipped into a
                // heading is clipped in the middle of the very thing being decided about.
                let about = magi_tui::wrap::hard(action.subject(), 60);
                self.overlay = Some(
                    magi_tui::picker::Picker::new(
                        format!("{tool} wants to {}", action.verb()),
                        choices,
                        None,
                    )
                    .about(about)
                    .into(),
                );
                self.asking_about = action;
                self.picking = Some(Picking::Permission { id, offers });
            }
            // The general question, drawn with the same picker a permission is — see `asked`.
            HarnessEvent::Asked {
                id,
                tool,
                question,
                options,
                detail,
                ..
            } => self.asked(id, &tool, &question, options, detail),
            // Rows a tool asked for. Nothing here reads what goes in them — see `surfacing`.
            HarnessEvent::Surfaced {
                id,
                tool,
                rows,
                about,
                ..
            } => self.surfaced(id, tool, rows, about),
            HarnessEvent::Drew { id, lines, cursor } => self.drew(&id, lines, cursor),
            HarnessEvent::Unsurfaced { id, .. } => self.unsurfaced(&id),
            // A permission answered on a surface. Remembered here because a session lends what it
            // holds to a child, and this one was decided on the tool thread without passing
            // through the loop that usually notices.
            HarnessEvent::Granted { grant, .. } => self.was_granted(grant),
            HarnessEvent::ModelChanged { model, .. } => {
                let before = self.model.as_ref().map(|m| m.name.clone());
                let after = model.as_ref().map(|m| m.name.clone());
                if self.started()
                    && before != after
                    && let Some(name) = after
                {
                    self.show_notice(format!("Model is now `{name}`."));
                }
                self.model = model;
            }
            // Not a transcript entry: the request was understood and declined, which is a
            // fact about what the UI asked rather than about the conversation.
            HarnessEvent::Refused { message, .. } => self.show_notice(message),
            // A rule marks the boundary between what is still sent and what is not. On a view
            // with nothing above it there is no boundary to mark, only a line saying nothing
            // is sent from here -- which is every empty session.
            HarnessEvent::Branched { id, keeps, .. } => {
                if self.started() {
                    self.entries.push(Entry::Branch { id, keeps });
                }
            }
            HarnessEvent::Compacted {
                id,
                summary,
                replaces,
                ..
            } => self.entries.push(Entry::Compaction {
                id,
                summary,
                replaces,
            }),
            HarnessEvent::ToolCallStarted { id, name, args, .. } => {
                self.entries.push(Entry::Tool {
                    id,
                    name,
                    args,
                    result: None,
                    thought_signature: None,
                });
            }
            HarnessEvent::ToolCallEnded { id, result, .. } => {
                if let Some(Entry::Tool { result: slot, .. }) = self.tool_mut(&id) {
                    *slot = Some(result);
                }
            }
            HarnessEvent::StatusChanged { status, .. } => self.set_status(status),
            HarnessEvent::Error { class, message, .. } => {
                self.entries.push(Entry::Assistant {
                    id: MessageId::new("error"),
                    text: String::new(),
                    thinking: String::new(),
                    stop_reason: Some(magi_proto::StopReason::Error),
                    error: Some(format!("{class:?}: {message}")),
                    signatures: magi_proto::Signatures::default(),
                    usage: magi_proto::Usage::default(),
                });
            }
        }
    }
}
