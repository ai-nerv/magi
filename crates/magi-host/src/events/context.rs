use super::{Correlation, Id, Lineage};
use std::future::Future;

tokio::task_local! {
    static CURRENT: Context;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Context {
    turn: Option<Id>,
    ask: Option<Id>,
}

fn current() -> Context {
    CURRENT.try_with(|value| *value).unwrap_or_default()
}

pub(crate) fn helper<F: Future>(future: F) -> impl Future<Output = F::Output> {
    CURRENT.scope(
        Context {
            turn: current().turn,
            ask: None,
        },
        future,
    )
}

pub(crate) async fn turn<F: Future>(id: Option<Id>, future: F) -> F::Output {
    CURRENT
        .scope(
            Context {
                turn: id,
                ask: None,
            },
            future,
        )
        .await
}

pub(crate) async fn ask<F: Future>(future: F) -> F::Output {
    let context = Context {
        ask: super::identity(),
        ..current()
    };
    CURRENT.scope(context, future).await
}

pub(super) fn fill(mut correlation: Correlation) -> Correlation {
    let context = current();
    correlation.turn_id = correlation.turn_id.or(context.turn);
    if correlation.turn_id == context.turn {
        correlation.ask_id = correlation.ask_id.or(context.ask);
    }
    correlation
}

pub(crate) fn lineage() -> Lineage {
    bind(super::process_lineage())
}

fn bind(mut root: Lineage) -> Lineage {
    if root.run_id.is_some() && root.agent_id.is_some() {
        let context = current();
        root.turn_id = root.turn_id.or(context.turn);
        if root.turn_id == context.turn {
            root.ask_id = root.ask_id.or(context.ask);
        }
    }
    root
}

#[cfg(test)]
mod tests;
