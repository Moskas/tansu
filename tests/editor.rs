//! The editor used by `:bulk_rename`. Alone in its own test binary, since it sets `$VISUAL`
//! for the whole process.

use tansu::opener::edit_text;

#[test]
fn edits_text_in_the_configured_editor_else_visual() {
  let tmp = tempfile::tempdir().unwrap();
  // SAFETY: the only test in this binary, so no other thread reads the environment.
  unsafe {
    std::env::set_var("VISUAL", "sed -i s/two/three/");
  }
  let edited = edit_text("one\ntwo\n", tmp.path(), None).unwrap();
  assert_eq!(edited, "one\nthree\n");
  // A blank setting counts as unset.
  let edited = edit_text("one\ntwo\n", tmp.path(), Some("  ")).unwrap();
  assert_eq!(edited, "one\nthree\n");
  // The configured editor comes before `$VISUAL`, with its arguments.
  let edited = edit_text("one\ntwo\n", tmp.path(), Some("sed -i s/one/zero/")).unwrap();
  assert_eq!(edited, "zero\ntwo\n");
  let err = edit_text("one\n", tmp.path(), Some("false")).unwrap_err();
  assert!(err.starts_with("the editor exited with"), "{err}");
}
