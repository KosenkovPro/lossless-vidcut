//! Manual test of the desktop shortcut.
//!
//! The test touches the desktop of the current user, so it is ignored by
//! default and only runs when it is asked for:
//!
//! ```text
//! cargo test --test shortcut -- --ignored
//! ```

use lossless_vidcut::shortcut;

#[test]
#[ignore = "changes the desktop of the current user"]
fn creates_and_removes_a_desktop_shortcut() {
    // Remove a shortcut that a previous run may have left behind.
    let _ = shortcut::remove();

    let path = shortcut::create().expect("the shortcut is created");
    assert!(path.exists(), "{} was not written", path.display());
    assert!(shortcut::exists(), "the shortcut was not found");

    shortcut::remove().expect("the shortcut is removed");
    assert!(!shortcut::exists(), "the shortcut was not removed");
}
