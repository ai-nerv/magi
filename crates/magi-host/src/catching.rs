//! A future whose panic comes back as an error, so a bug in one turn ends that turn and not the
//! session. Needs `panic = "unwind"`; under `abort` nothing is caught and the process ends.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};

/// `future`, with a panic while it is polled turned into `Err` naming what panicked.
pub(crate) fn caught<F: Future>(future: F) -> Caught<F> {
    Caught(Box::pin(future))
}

pub(crate) struct Caught<F: Future>(Pin<Box<F>>);

impl<F: Future> Future for Caught<F> {
    type Output = Result<F::Output, String>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(Poll::Ready(done)) => Poll::Ready(Ok(done)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(said(payload.as_ref()))),
        }
    }
}

/// What a panic said, when it said it as text.
fn said(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|said| (*said).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic that said nothing".to_owned())
}

#[cfg(test)]
mod tests {
    use super::caught;

    #[tokio::test]
    async fn a_panic_inside_comes_back_as_an_error_and_a_value_as_itself() {
        let failed = caught(async {
            tokio::task::yield_now().await;
            if std::hint::black_box(true) {
                panic!("the bug");
            }
            1
        })
        .await;
        assert_eq!(failed, Err("the bug".to_owned()));
        assert_eq!(caught(async { 7 }).await, Ok(7));
    }
}
