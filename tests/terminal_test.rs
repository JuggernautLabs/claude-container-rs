//! Tests for terminal restoration safety (GS-11).
//!
//! These tests verify that restore_terminal() consolidates all terminal
//! cleanup into a single function, and that it's idempotent.

use git_sandbox::container;

#[test]
fn restore_terminal_shows_cursor() {
    // restore_terminal() should write \x1b[?25h to show cursor.
    // We can't easily capture stdout in a test, but we can verify
    // the function doesn't panic and is callable.
    container::restore_terminal();
}

#[test]
fn restore_terminal_idempotent() {
    // Calling restore_terminal() twice should not panic or break anything.
    container::restore_terminal();
    container::restore_terminal();
    container::restore_terminal();
}

#[test]
fn restore_terminal_after_raw_mode() {
    // If raw mode was never enabled (or was already disabled),
    // restore_terminal() should still work fine.
    // crossterm::disable_raw_mode() is a no-op if not enabled.
    container::restore_terminal();

    // Verify we can still write to stdout/stderr normally
    println!("stdout works after restore");
    eprintln!("stderr works after restore");
}

#[cfg(unix)]
#[test]
fn restore_terminal_restores_opost() {
    // After restore_terminal(), OPOST should be set (if we're on a TTY).
    // In CI / test environments we may not have a TTY, so we skip gracefully.
    container::restore_terminal();

    unsafe {
        let fd = libc::STDERR_FILENO;
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) == 0 {
            // We have a TTY — OPOST should be set
            assert!(
                termios.c_oflag & libc::OPOST != 0,
                "OPOST should be set after restore_terminal()"
            );
        }
        // If tcgetattr fails, we're not on a TTY — skip silently
    }
}

#[cfg(unix)]
#[test]
fn restore_terminal_restores_echo_and_icanon() {
    // After restore_terminal(), ECHO and ICANON should be set (if on a TTY).
    container::restore_terminal();

    unsafe {
        let fd = libc::STDERR_FILENO;
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut termios) == 0 {
            assert!(
                termios.c_lflag & libc::ICANON != 0,
                "ICANON should be set after restore_terminal()"
            );
            assert!(
                termios.c_lflag & libc::ECHO != 0,
                "ECHO should be set after restore_terminal()"
            );
        }
    }
}

/// Verify that restore_terminal is the single public API for terminal cleanup.
/// This is a compile-time check — if the function signature changes, this breaks.
#[test]
fn restore_terminal_is_public() {
    let f: fn() = container::restore_terminal;
    f(); // callable
}
