use crate::{Capabilities, ThinkingLevel};

impl ThinkingLevel {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    #[must_use]
    pub fn available(reasoning: bool, capabilities: Option<&Capabilities>) -> Vec<Self> {
        let levels = capabilities.and_then(|caps| caps.thinking_levels.as_ref());
        Self::ALL
            .into_iter()
            .filter(|level| {
                (reasoning || *level == Self::Off)
                    && levels.map_or(*level != Self::XHigh, |levels| {
                        levels.iter().any(|name| name == level.as_str())
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_controls_do_not_infer_xhigh_and_explicit_empty_means_none() {
        assert_eq!(ThinkingLevel::available(true, None).len(), 6);
        assert_eq!(ThinkingLevel::available(false, None), [ThinkingLevel::Off]);
        let caps = Capabilities {
            thinking: Some(vec!["xhigh".into()]),
            ..Default::default()
        };
        assert!(!ThinkingLevel::available(true, Some(&caps)).contains(&ThinkingLevel::XHigh));
        let caps = Capabilities {
            thinking_levels: Some(Vec::new()),
            ..Default::default()
        };
        assert!(ThinkingLevel::available(true, Some(&caps)).is_empty());
    }

    #[test]
    fn declared_controls_are_canonical_bounded_and_ignore_future_names() {
        let caps = Capabilities {
            thinking_levels: Some(vec![
                "xhigh".into(),
                "future".into(),
                "low".into(),
                "xhigh".into(),
            ]),
            ..Default::default()
        };
        assert_eq!(
            ThinkingLevel::available(true, Some(&caps)),
            [ThinkingLevel::Low, ThinkingLevel::XHigh]
        );
        assert!(ThinkingLevel::available(false, Some(&caps)).is_empty());
        for level in ThinkingLevel::ALL {
            let wire = serde_json::to_value(level).expect("level");
            assert_eq!(wire, level.as_str());
            assert_eq!(
                serde_json::from_value::<ThinkingLevel>(wire).expect("roundtrip"),
                level
            );
        }
        assert!(
            ThinkingLevel::High < ThinkingLevel::XHigh && ThinkingLevel::XHigh < ThinkingLevel::Max
        );
    }
}
