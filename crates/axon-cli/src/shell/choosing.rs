//! Which shell the peer runs, and how a command is quoted into it.
//!
//! Split out under THE RULE; the session next door is what these are about.

#[cfg(test)]
mod which_shell_tests {
    use crate::shell::{name_of, shell_command_from};

    #[test]
    fn a_shell_that_does_not_exist_is_not_used() {
        // `$SHELL` records a login shell, which is not always a shell this machine has — a
        // home directory carried between machines is the usual way that happens.
        assert_eq!(shell_command_from(Some("/no/such/shell"), None), "sh");
    }

    #[test]
    fn the_users_own_shell_wins_over_sh() {
        assert_eq!(shell_command_from(Some("/bin/sh"), None), "/bin/sh");
    }

    #[test]
    fn an_override_beats_the_login_shell() {
        // `$AXON_SHELL` exists so a session can differ from the login shell without changing it.
        assert_eq!(
            shell_command_from(Some("/bin/sh"), Some("/bin/sh")),
            "/bin/sh"
        );
    }

    #[test]
    fn nothing_set_is_sh() {
        assert_eq!(shell_command_from(None, None), "sh");
    }

    #[test]
    fn the_name_is_what_the_model_is_told_it_is_running() {
        assert_eq!(name_of("/usr/bin/oslo"), "oslo");
        assert_eq!(name_of("sh"), "sh");
    }
}

#[cfg(test)]
mod escape_tests {
    use crate::shell::strip_escapes;

    #[test]
    fn colour_is_removed_and_the_text_kept() {
        assert_eq!(strip_escapes("\u{1b}[01;32mhello\u{1b}[00m"), "hello");
    }

    #[test]
    fn shell_integration_codes_are_removed() {
        // A model handed `]3008;start=…` reads it as data.
        let noisy = "\u{1b}]3008;start=abc;cwd=/tmp\u{1b}\\hello";
        assert_eq!(strip_escapes(noisy), "hello");
    }

    #[test]
    fn an_osc_ending_in_bell_is_removed() {
        assert_eq!(strip_escapes("\u{1b}]0;a title\u{7}text"), "text");
    }

    #[test]
    fn plain_text_is_untouched() {
        assert_eq!(strip_escapes("just output"), "just output");
    }

    #[test]
    fn a_lone_escape_does_not_eat_the_line() {
        assert_eq!(strip_escapes("a\u{1b}b"), "a");
    }
}
