//! Token counts and what they cost.

use serde::{Deserialize, Serialize};

/// Tokens consumed by one request. Cache reads and writes are counted apart from ordinary input
/// because they are priced apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    #[must_use]
    pub const fn prompt_tokens(self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    /// What fraction of the prompt came from cache, or `None` when nothing was sent.
    #[must_use]
    pub fn cache_hit_rate(self) -> Option<f64> {
        let prompt = self.prompt_tokens();
        (prompt > 0).then(|| self.cache_read as f64 / prompt as f64 * 100.0)
    }

    pub const fn add(&mut self, other: Self) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    #[must_use]
    pub fn price(self, cost: Cost) -> f64 {
        let per_million = |tokens: u64, rate: f64| tokens as f64 / 1_000_000.0 * rate;
        per_million(self.input, cost.input)
            + per_million(self.output, cost.output)
            + per_million(self.cache_read, cost.cache_read)
            + per_million(self.cache_write, cost.cache_write)
    }
}

/// Dollars per million tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
    /// Prompt tokens served from cache, usually far cheaper than `input`. Defaults to zero rather
    /// than to `input`: only the catalog knows whether a provider prices caching separately.
    #[serde(default)]
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_tokens_count_cache_as_well_as_input() {
        let usage = Usage {
            input: 100,
            output: 50,
            cache_read: 900,
            cache_write: 10,
        };
        assert_eq!(usage.prompt_tokens(), 1010);
    }

    #[test]
    fn the_cache_hit_rate_is_a_percentage_of_the_prompt() {
        let usage = Usage {
            input: 250,
            cache_read: 750,
            ..Usage::default()
        };
        assert_eq!(usage.cache_hit_rate(), Some(75.0));
    }

    #[test]
    fn an_empty_prompt_has_no_hit_rate_rather_than_zero() {
        assert_eq!(Usage::default().cache_hit_rate(), None);
    }

    #[test]
    fn totals_accumulate() {
        let mut total = Usage::default();
        total.add(Usage {
            input: 10,
            output: 5,
            ..Usage::default()
        });
        total.add(Usage {
            input: 1,
            output: 2,
            ..Usage::default()
        });
        assert_eq!(total.input, 11);
        assert_eq!(total.output, 7);
    }

    #[test]
    fn price_is_per_million_tokens() {
        let usage = Usage {
            input: 1_000_000,
            output: 500_000,
            ..Usage::default()
        };
        let cost = Cost {
            input: 3.0,
            output: 15.0,
            ..Cost::default()
        };
        assert!(
            (usage.price(cost) - 10.5).abs() < 1e-9,
            "{}",
            usage.price(cost)
        );
    }
}
