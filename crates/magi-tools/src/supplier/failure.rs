#[derive(Debug)]
pub(super) struct Failure {
    reason: String,
    refused: bool,
}

impl Failure {
    pub(super) fn after(sent: bool, reason: impl Into<String>) -> Self {
        if sent {
            Self::unknown(reason)
        } else {
            Self::refused(reason)
        }
    }
    pub(super) fn refused(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            refused: true,
        }
    }
    pub(super) fn unknown(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            refused: false,
        }
    }
    pub(super) fn no_effect(&self) -> bool {
        self.refused
    }
    pub(super) fn prefixed(self, prefix: &str) -> Self {
        Self {
            reason: format!("{prefix}{}", self.reason),
            refused: self.refused,
        }
    }
    pub(super) fn into_string(self) -> String {
        self.reason
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::ops::Deref for Failure {
    type Target = str;
    fn deref(&self) -> &str {
        &self.reason
    }
}
