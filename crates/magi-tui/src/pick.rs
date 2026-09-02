//! Choosing the next placeholder.
//!
//! A new one every time the prompt empties, rather than one a session. The line is read once and
//! then it is furniture; a fresh one on every empty box is the difference between a joke and a
//! label. Emptying covers both ways it happens — a prompt submitted, and a prompt deleted back
//! to nothing.

/// A different index from `now`, out of `count`.
///
/// **Different**, not merely random: one in twenty-four rolls repeats, and a placeholder that
/// does not change when you have just watched it change is indistinguishable from one that is
/// stuck. So the roll is over the other twenty-three and then shifted past the current one.
#[must_use]
pub fn another(now: usize, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let step = 1 + roll() % (count - 1);
    (now + step) % count
}

/// A first one, before anything has been typed.
#[must_use]
pub fn first(count: usize) -> usize {
    if count == 0 { 0 } else { roll() % count }
}

/// Something that varies, without a dependency for it.
///
/// The clock's nanoseconds. Not uniform and not unpredictable, and neither matters: this decides
/// which joke you get.
fn roll() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_one_is_never_the_one_you_just_saw() {
        // The whole point. A repeat reads as the box being stuck, not as chance.
        for now in 0..24 {
            for _ in 0..50 {
                assert_ne!(another(now, 24), now, "repeated {now}");
            }
        }
    }

    #[test]
    fn it_stays_inside_the_list() {
        for now in 0..24 {
            assert!(another(now, 24) < 24);
            assert!(first(24) < 24);
        }
    }

    #[test]
    fn a_list_of_one_is_that_one_forever() {
        // There is no other to move to, and refusing to repeat would mean showing nothing.
        assert_eq!(another(0, 1), 0);
        assert_eq!(first(1), 0);
    }

    #[test]
    fn an_empty_list_does_not_divide_by_zero() {
        assert_eq!(another(0, 0), 0);
        assert_eq!(first(0), 0);
    }
}
