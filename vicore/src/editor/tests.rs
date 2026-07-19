//! Behavioral tests for the vi engine. These drive the editor through the same
//! semantic-event API the kernel adapter uses.

#![allow(non_snake_case)]
use super::*;

/// Feed a string of normal/insert characters, one `on_char` per byte.
fn keys(ed: &mut Editor, s: &str) {
    for c in s.chars() {
        ed.on_char(c);
    }
}
fn esc(ed: &mut Editor) {
    ed.on_special(Special::Esc);
}
fn text(ed: &Editor) -> String {
    ed.lines().join("\n")
}

#[test]
fn basic_motion_hjkl() {
    let mut ed = Editor::new("abc\ndef\nghi");
    keys(&mut ed, "ll");
    assert_eq!(ed.cursor(), (0, 2));
    keys(&mut ed, "jj");
    assert_eq!(ed.cursor(), (2, 2));
    keys(&mut ed, "h");
    assert_eq!(ed.cursor(), (2, 1));
    keys(&mut ed, "k");
    assert_eq!(ed.cursor(), (1, 1));
}

#[test]
fn dollar_and_zero_and_caret() {
    let mut ed = Editor::new("  hello world");
    keys(&mut ed, "$");
    assert_eq!(ed.cursor(), (0, 12));
    keys(&mut ed, "0");
    assert_eq!(ed.cursor(), (0, 0));
    keys(&mut ed, "^");
    assert_eq!(ed.cursor(), (0, 2));
}

#[test]
fn word_motions() {
    let mut ed = Editor::new("foo bar.baz qux");
    keys(&mut ed, "w");
    assert_eq!(ed.cursor(), (0, 4)); // bar
    keys(&mut ed, "w");
    assert_eq!(ed.cursor(), (0, 7)); // .
    keys(&mut ed, "w");
    assert_eq!(ed.cursor(), (0, 8)); // baz
    keys(&mut ed, "e");
    assert_eq!(ed.cursor(), (0, 10)); // baz end
    keys(&mut ed, "b");
    assert_eq!(ed.cursor(), (0, 8));
}

#[test]
fn gg_and_G() {
    let mut ed = Editor::new("a\nb\nc\nd");
    keys(&mut ed, "G");
    assert_eq!(ed.cursor().0, 3);
    keys(&mut ed, "gg");
    assert_eq!(ed.cursor().0, 0);
    keys(&mut ed, "2G");
    assert_eq!(ed.cursor().0, 1);
}

#[test]
fn insert_and_escape() {
    let mut ed = Editor::new("bc");
    keys(&mut ed, "i");
    assert_eq!(ed.mode(), Mode::Insert);
    keys(&mut ed, "a");
    esc(&mut ed);
    assert_eq!(text(&ed), "abc");
    assert_eq!(ed.mode(), Mode::Normal);
}

#[test]
fn append_open_line() {
    let mut ed = Editor::new("ab");
    keys(&mut ed, "A!");
    esc(&mut ed);
    assert_eq!(text(&ed), "ab!");
    keys(&mut ed, "ox");
    esc(&mut ed);
    assert_eq!(text(&ed), "ab!\nx");
}

#[test]
fn x_and_dd() {
    let mut ed = Editor::new("hello\nworld");
    keys(&mut ed, "x");
    assert_eq!(ed.lines()[0], "ello");
    keys(&mut ed, "dd");
    assert_eq!(text(&ed), "world");
}

#[test]
fn dw_and_counts() {
    let mut ed = Editor::new("one two three four");
    keys(&mut ed, "dw");
    assert_eq!(ed.lines()[0], "two three four");
    keys(&mut ed, "2dw");
    assert_eq!(ed.lines()[0], "four");
}

#[test]
fn cw_changes_word() {
    let mut ed = Editor::new("foo bar");
    keys(&mut ed, "cwbaz");
    esc(&mut ed);
    assert_eq!(text(&ed), "baz bar");
}

#[test]
fn yy_and_paste() {
    let mut ed = Editor::new("line1\nline2");
    keys(&mut ed, "yyp");
    assert_eq!(text(&ed), "line1\nline1\nline2");
}

#[test]
fn charwise_yank_paste() {
    let mut ed = Editor::new("abcdef");
    keys(&mut ed, "vll"); // select abc
    keys(&mut ed, "y");
    keys(&mut ed, "$p");
    assert_eq!(text(&ed), "abcdefabc");
}

#[test]
fn undo_redo() {
    let mut ed = Editor::new("abc");
    keys(&mut ed, "x");
    assert_eq!(ed.lines()[0], "bc");
    keys(&mut ed, "u");
    assert_eq!(ed.lines()[0], "abc");
    ed.on_ctrl('r');
    assert_eq!(ed.lines()[0], "bc");
}

#[test]
fn undo_coalesces_insert() {
    let mut ed = Editor::new("");
    keys(&mut ed, "i");
    keys(&mut ed, "hello");
    esc(&mut ed);
    assert_eq!(text(&ed), "hello");
    keys(&mut ed, "u");
    assert_eq!(text(&ed), "");
}

#[test]
fn dot_repeats_x() {
    let mut ed = Editor::new("abcdef");
    keys(&mut ed, "x");
    keys(&mut ed, "..");
    assert_eq!(ed.lines()[0], "def");
}

#[test]
fn dot_repeats_insert() {
    let mut ed = Editor::new("X");
    keys(&mut ed, "ia");
    esc(&mut ed);
    assert_eq!(text(&ed), "aX");
    keys(&mut ed, ".");
    assert_eq!(text(&ed), "aaX");
}

#[test]
fn find_char_and_repeat() {
    let mut ed = Editor::new("a.b.c.d");
    keys(&mut ed, "f.");
    assert_eq!(ed.cursor(), (0, 1));
    keys(&mut ed, ";");
    assert_eq!(ed.cursor(), (0, 3));
    keys(&mut ed, ",");
    assert_eq!(ed.cursor(), (0, 1));
}

#[test]
fn search_forward_and_n() {
    let mut ed = Editor::new("apple\nbanana\napricot");
    // /an -> banana
    ed.on_char('/');
    keys(&mut ed, "an");
    ed.on_special(Special::Enter);
    assert_eq!(ed.cursor().0, 1);
}

#[test]
fn ex_write_and_quit_effects() {
    let mut ed = Editor::new("hi");
    keys(&mut ed, "x"); // make dirty
    ed.on_char(':');
    keys(&mut ed, "w");
    ed.on_special(Special::Enter);
    assert_eq!(ed.take_effects(), alloc::vec![Effect::Save(None)]);

    ed.on_char(':');
    keys(&mut ed, "q");
    ed.on_special(Special::Enter);
    // still dirty (host hasn't confirmed save) -> refuses
    assert!(ed.take_effects().is_empty());

    ed.on_char(':');
    keys(&mut ed, "q!");
    ed.on_special(Special::Enter);
    assert_eq!(ed.take_effects(), alloc::vec![Effect::ForceQuit]);
}

#[test]
fn ex_substitute() {
    let mut ed = Editor::new("foo foo foo");
    ed.on_char(':');
    keys(&mut ed, "s/foo/bar/");
    ed.on_special(Special::Enter);
    assert_eq!(ed.lines()[0], "bar foo foo");

    let mut ed2 = Editor::new("foo foo\nfoo bar");
    ed2.on_char(':');
    keys(&mut ed2, "%s/foo/X/g");
    ed2.on_special(Special::Enter);
    assert_eq!(text(&ed2), "X X\nX bar");
}

#[test]
fn goto_line_ex() {
    let mut ed = Editor::new("1\n2\n3\n4\n5");
    ed.on_char(':');
    keys(&mut ed, "3");
    ed.on_special(Special::Enter);
    assert_eq!(ed.cursor().0, 2);
}

#[test]
fn visual_line_delete() {
    let mut ed = Editor::new("a\nb\nc\nd");
    keys(&mut ed, "Vjd");
    assert_eq!(text(&ed), "c\nd");
}

#[test]
fn join_lines() {
    let mut ed = Editor::new("hello\n  world");
    keys(&mut ed, "J");
    assert_eq!(text(&ed), "hello world");
}

#[test]
fn replace_char() {
    let mut ed = Editor::new("cat");
    keys(&mut ed, "rb");
    assert_eq!(ed.lines()[0], "bat");
}

#[test]
fn tilde_toggles_case() {
    let mut ed = Editor::new("abc");
    keys(&mut ed, "~~");
    assert_eq!(ed.lines()[0], "ABc");
}

#[test]
fn marks() {
    let mut ed = Editor::new("l0\nl1\nl2\nl3");
    keys(&mut ed, "jjma"); // mark a at line 2
    keys(&mut ed, "gg");
    keys(&mut ed, "`a");
    assert_eq!(ed.cursor().0, 2);
}

#[test]
fn dollar_then_down_keeps_eol() {
    let mut ed = Editor::new("longline\nx\nanother");
    keys(&mut ed, "$");
    keys(&mut ed, "j");
    // sticky EOL column -> single char line
    assert_eq!(ed.cursor(), (1, 0));
    keys(&mut ed, "j");
    assert_eq!(ed.cursor(), (2, 6));
}
