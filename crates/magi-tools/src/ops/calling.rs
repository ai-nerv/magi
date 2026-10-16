//! Ops for one tool call: everything goes to the session's own, and the call is known by id.

use super::Ops;
use std::path::{Path, PathBuf};

/// `ops`, for the one tool call `id`: everything it does is `ops`'s, and it knows whose it is.
pub struct Calling<'a> {
    pub ops: &'a dyn Ops,
    pub id: magi_proto::ToolCallId,
}

impl Ops for Calling<'_> {
    fn cwd(&self) -> PathBuf {
        self.ops.cwd()
    }

    fn resolved(&self, path: &Path) -> PathBuf {
        self.ops.resolved(path)
    }

    fn read(&self, path: &Path) -> Result<String, String> {
        self.ops.read(path)
    }

    fn write(&self, path: &Path, contents: &str) -> Result<(), String> {
        self.ops.write(path, contents)
    }

    fn allow(&self, tool: &str, action: &magi_proto::permit::Action) -> Result<(), String> {
        self.ops.allow(tool, action)
    }

    fn take_on(&self, grants: Vec<magi_proto::permit::Grant>) {
        self.ops.take_on(grants);
    }

    fn held(&self) -> Vec<magi_proto::permit::Grant> {
        self.ops.held()
    }

    fn noticed(&self) -> Vec<crate::watching::Noted> {
        self.ops.noticed()
    }

    fn jail(&self) -> Option<String> {
        self.ops.jail()
    }

    fn call(&self) -> Option<magi_proto::ToolCallId> {
        Some(self.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ops that refuse everything and name a jail, so a wrapper that dropped either would show.
    struct Strict;

    impl Ops for Strict {
        fn cwd(&self) -> PathBuf {
            PathBuf::from("/work")
        }
        fn read(&self, _: &Path) -> Result<String, String> {
            Err("read refused".into())
        }
        fn write(&self, _: &Path, _: &str) -> Result<(), String> {
            Err("write refused".into())
        }
        fn allow(&self, _: &str, _: &magi_proto::permit::Action) -> Result<(), String> {
            Err("not allowed".into())
        }
        fn jail(&self) -> Option<String> {
            Some("profile".into())
        }
    }

    #[test]
    fn a_call_is_gated_and_jailed_as_the_session_is() {
        let calling = Calling {
            ops: &Strict,
            id: magi_proto::ToolCallId::new("c1"),
        };
        let action = magi_proto::permit::Action::Read { path: "/".into() };
        assert_eq!(calling.allow("read", &action), Err("not allowed".into()));
        assert_eq!(calling.jail().as_deref(), Some("profile"));
        assert_eq!(calling.cwd(), PathBuf::from("/work"));
        assert!(calling.write(Path::new("a"), "x").is_err());
    }

    #[test]
    fn a_call_knows_whose_it_is() {
        let calling = Calling {
            ops: &Strict,
            id: magi_proto::ToolCallId::new("c1"),
        };
        assert_eq!(calling.call(), Some(magi_proto::ToolCallId::new("c1")));
        assert_eq!(Strict.call(), None);
    }
}
