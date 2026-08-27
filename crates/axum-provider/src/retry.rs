//! Why a request failed, and whether trying again can help.
//!
//! An enum built from status codes and typed bodies. Never from matching provider prose: Pi
//! classifies with ~35 regex alternates and had to re-prefix Bedrock's errors so they would
//! match, which couples every adapter to a regex in another package through nothing but
//! convention.

use std::time::Duration;

/// The kind of failure, which decides whether and when to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryClass {
    /// The connection failed before an answer arrived.
    Transport,
    /// The provider is busy. Retry with backoff.
    Overload,
    /// Rate limited. Retry after the window.
    Throttle,
    /// Credentials are missing or rejected.
    Auth,
    /// The request was malformed, or asked for something unavailable.
    Invalid,
    /// The context window overflowed. Compact and retry.
    Overflow,
    /// Unrecognised.
    Unknown,
}

impl RetryClass {
    /// Classify an HTTP status.
    ///
    /// 400 stays [`Invalid`](Self::Invalid) rather than being sniffed for an overflow message:
    /// the caller knows the token count it sent and can decide that far more reliably than a
    /// substring can.
    #[must_use]
    pub const fn of_status(status: u16) -> Self {
        match status {
            401 | 403 => Self::Auth,
            408 | 409 => Self::Transport,
            429 => Self::Throttle,
            500 | 502 | 503 | 504 | 529 => Self::Overload,
            400 | 404 | 422 => Self::Invalid,
            _ => Self::Unknown,
        }
    }

    /// The class of a failure, from its status *and* what the provider said about it.
    ///
    /// The message is not decoration here. A context-window overflow arrives as an ordinary
    /// 400: the status says only that the request was rejected, and nothing but the body
    /// distinguishes "your request is malformed" — which retrying cannot fix — from "your
    /// request was too long", which compacting can. Classified on status alone, `Overflow` is
    /// a variant nothing ever produces and the conversation simply stops.
    ///
    /// Matched on phrases rather than on a per-vendor error code because the codes disagree
    /// and the phrases do not: every one of them says the length was the problem.
    #[must_use]
    pub fn of(status: u16, message: &str) -> Self {
        let class = Self::of_status(status);
        if matches!(class, Self::Invalid | Self::Unknown) && mentions_length(message) {
            return Self::Overflow;
        }
        class
    }

    /// Whether retrying this can succeed.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::Transport | Self::Overload | Self::Throttle)
    }
}

/// Whether a provider's complaint is about how much was sent.
fn mentions_length(message: &str) -> bool {
    const PHRASES: &[&str] = &[
        "context length",
        "context_length_exceeded",
        "maximum context",
        "context window",
        "too many tokens",
        "prompt is too long",
        "input is too long",
        "reduce the length",
        "exceeds the maximum",
    ];
    let message = message.to_ascii_lowercase();
    PHRASES.iter().any(|phrase| message.contains(phrase))
}

/// The first delay, before any growth.
pub(crate) const BASE: Duration = Duration::from_secs(10);

/// The most any single wait will be.
const CEILING: Duration = Duration::from_secs(600);

/// How long to wait before attempt `attempt`, counting from 1.
///
/// Fibonacci from ten seconds, jittered by a hash of the request rather than by a random
/// number, so a retry schedule reproduces exactly in a test. Tau does the same, and it is the
/// difference between a backoff you can assert on and one you can only observe.
#[must_use]
pub fn backoff(attempt: u32, seed: u64) -> Duration {
    backoff_from(BASE, attempt, seed)
}

/// The same, from a stated first delay.
///
/// The base is a parameter so a caller can hold the policy rather than inherit it. Tests are
/// the caller that needs this: four attempts at the real base is a minute of a test suite
/// spent proving arithmetic that has its own tests already.
#[must_use]
pub fn backoff_from(base_delay: Duration, attempt: u32, seed: u64) -> Duration {
    // 1, 2, 3, 5, 8 — from (1, 2) rather than (1, 1), so consecutive attempts never share a
    // multiplier and each wait is strictly longer than the last.
    let (mut a, mut b) = (1_u64, 2_u64);
    for _ in 1..attempt.max(1) {
        (a, b) = (b, a.saturating_add(b));
    }
    let base = base_delay.saturating_mul(u32::try_from(a).unwrap_or(u32::MAX));
    let capped = base.min(CEILING);

    // Jitter spreads a thundering herd across the last sixth of the window; the ceiling stays
    // a ceiling because the jitter is subtracted, never added.
    let span = capped.as_millis() as u64 / 6;
    let offset = if span == 0 { 0 } else { seed % span };
    capped.saturating_sub(Duration::from_millis(offset))
}

/// A stable seed for one request's retry schedule.
#[must_use]
pub fn seed(request_id: &str, attempt: u32) -> u64 {
    // FNV-1a: not cryptographic, just a stable spread that does not need a dependency.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in request_id.bytes().chain(attempt.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_map_to_classes() {
        assert_eq!(RetryClass::of_status(429), RetryClass::Throttle);
        assert_eq!(RetryClass::of_status(529), RetryClass::Overload);
        assert_eq!(RetryClass::of_status(401), RetryClass::Auth);
        assert_eq!(RetryClass::of_status(400), RetryClass::Invalid);
    }

    #[test]
    fn only_transient_classes_retry() {
        assert!(RetryClass::Transport.is_retryable());
        assert!(RetryClass::Overload.is_retryable());
        assert!(RetryClass::Throttle.is_retryable());
        assert!(!RetryClass::Auth.is_retryable());
        assert!(!RetryClass::Invalid.is_retryable());
        assert!(!RetryClass::Overflow.is_retryable());
    }

    #[test]
    fn a_four_hundred_about_length_is_an_overflow() {
        // The whole reason `Overflow` was a variant nothing produced: providers send an
        // ordinary 400 and put the actual problem in the body.
        for said in [
            "This model's maximum context length is 8192 tokens",
            "context_length_exceeded",
            "prompt is too long: 250000 tokens > 200000 maximum",
            "Please reduce the length of the messages",
        ] {
            assert_eq!(RetryClass::of(400, said), RetryClass::Overflow, "{said}");
        }
    }

    #[test]
    fn an_ordinary_four_hundred_is_still_invalid() {
        // Compacting would not help, and retrying a malformed request forever is worse than
        // reporting it.
        assert_eq!(
            RetryClass::of(400, "unknown field `temperatur`"),
            RetryClass::Invalid
        );
    }

    #[test]
    fn a_status_that_speaks_for_itself_is_not_second_guessed() {
        // A 401 mentioning a context length is still an auth failure; compacting a request
        // nobody is allowed to make achieves nothing.
        assert_eq!(
            RetryClass::of(401, "context length exceeded"),
            RetryClass::Auth
        );
        assert_eq!(RetryClass::of(429, "too many tokens"), RetryClass::Throttle);
    }

    #[test]
    fn every_attempt_waits_longer_than_the_last() {
        // Across seeds, not one: jitter must never let a later attempt fire sooner.
        for request in ["a", "b", "c", "d"] {
            for attempt in 1..9 {
                let earlier = backoff(attempt, seed(request, attempt));
                let later = backoff(attempt + 1, seed(request, attempt + 1));
                assert!(
                    later > earlier,
                    "{request} attempt {attempt}: {later:?} <= {earlier:?}"
                );
            }
        }
    }

    #[test]
    fn backoff_is_capped() {
        assert!(backoff(50, seed("r", 50)) <= CEILING);
    }

    #[test]
    fn backoff_reproduces_for_the_same_request() {
        let a = backoff(3, seed("request-1", 3));
        let b = backoff(3, seed("request-1", 3));
        assert_eq!(a, b, "a retry schedule must be assertable");
    }

    #[test]
    fn different_requests_get_different_jitter() {
        let a = backoff(3, seed("request-1", 3));
        let b = backoff(3, seed("request-2", 3));
        assert_ne!(a, b, "a herd must not retry in lockstep");
    }

    #[test]
    fn jitter_never_pushes_a_wait_past_the_ceiling() {
        for attempt in 1..20 {
            for request in ["a", "b", "c"] {
                let wait = backoff(attempt, seed(request, attempt));
                assert!(wait <= CEILING, "attempt {attempt} waited {wait:?}");
            }
        }
    }

    #[test]
    fn the_first_wait_is_about_the_base() {
        let wait = backoff(1, seed("r", 1));
        assert!(wait <= BASE && wait > BASE.mul_f64(0.8), "{wait:?}");
    }
}
