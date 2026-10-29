#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaitError {
    Cancelled,
    Expired,
    Unavailable,
}

impl From<std::io::Error> for WaitError {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => Self::Expired,
            _ => Self::Unavailable,
        }
    }
}

impl From<magi_ipc::IpcError> for WaitError {
    fn from(error: magi_ipc::IpcError) -> Self {
        match error {
            magi_ipc::IpcError::Io(error) => error.into(),
            _ => Self::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_timeout_is_not_an_explicit_denial() {
        for kind in [std::io::ErrorKind::TimedOut, std::io::ErrorKind::WouldBlock] {
            assert_eq!(
                WaitError::from(std::io::Error::from(kind)),
                WaitError::Expired
            );
        }
        for error in [
            magi_ipc::IpcError::Malformed,
            magi_ipc::IpcError::Disconnected,
        ] {
            assert_eq!(WaitError::from(error), WaitError::Unavailable);
        }
    }
}
