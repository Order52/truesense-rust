//! The dialogs: the question, the text prompt and the help; and the messages the model ignores.

use super::*;
use crate::tui::model::Modal;

fn prompt_text(h: &Harness) -> Option<String> {
    match &h.model.modal {
        Some(Modal::Prompt { buf, .. }) => Some(buf.clone()),
        _ => None,
    }
}

fn open_prompt(h: &mut Harness) {
    h.goto(3);
    h.sel("Custom colour", 0);
    h.key(Key::Enter);
}

fn ask(h: &mut Harness) -> RowRef {
    h.goto(2);
    let row = h.sel("GPU mode", 0);
    h.edit(row, Value::text("auto"));
    row
}

fn focus(h: &Harness) -> usize {
    match &h.model.modal {
        Some(Modal::Confirm { focus, .. }) => *focus,
        other => panic!("no question: {other:?}"),
    }
}

#[test]
fn the_confirm_dialog_defaults_to_no_and_enter_alone_never_says_yes() {
    let mut h = Harness::new(100, 30);
    ask(&mut h);
    assert_eq!(focus(&h), 1);
    h.key(Key::Enter);
    assert!(h.model.modal.is_none());
    assert_eq!(h.rig.sim.snapshot().gpu, 2);
    assert_eq!(h.model.msg.text, "Cancelled: nothing was changed");
    assert_eq!(h.model.yes_answers(), 0);
}

#[test]
fn the_confirm_dialog_yes_needs_an_explicit_choice() {
    for key in [Key::Char('y'), Key::Char('Y')] {
        let mut h = Harness::new(100, 30);
        ask(&mut h);
        h.key(key);
        assert_eq!(h.rig.sim.snapshot().gpu, 3, "{key:?}");
        assert_eq!(h.model.yes_answers(), 1);
    }
    for key in [Key::Char('n'), Key::Char('N'), Key::Esc, Key::Char('q')] {
        let mut h = Harness::new(100, 30);
        ask(&mut h);
        h.key(key);
        assert_eq!(h.rig.sim.snapshot().gpu, 2, "{key:?}");
        assert!(h.model.modal.is_none() && !h.model.quit());
    }
    let mut h = Harness::new(100, 30);
    ask(&mut h);
    h.key(Key::Left);
    h.key(Key::Enter);
    assert_eq!(h.rig.sim.snapshot().gpu, 3, "left to Yes, then Enter");
    let mut h = Harness::new(100, 30);
    ask(&mut h);
    h.key(Key::Char('x'));
    assert!(h.model.modal.is_some(), "other keys do nothing");
}

#[test]
fn the_confirm_focus_toggles_with_every_move_key() {
    let mut h = Harness::new(100, 30);
    ask(&mut h);
    for key in [Key::Left, Key::Right, Key::Char('h'), Key::Char('l'), Key::Tab, Key::BackTab] {
        let before = focus(&h);
        h.key(key);
        assert_eq!(focus(&h), 1 - before, "{key:?}");
    }
}

#[test]
fn the_confirm_dialog_is_drawn_with_both_buttons_and_the_marked_one() {
    let mut h = Harness::new(100, 30);
    ask(&mut h);
    let buf = h.draw();
    let text = buffer_text(&buf);
    assert!(
        text.contains("[ Yes ]") && text.contains("[ No ]") && text.contains("Switch the GPU mode to auto"),
        "{text}"
    );
    // The marked button is the pill: No at first.
    let (y, line) = text.lines().enumerate().find(|(_, l)| l.contains("[ No ]")).expect("the buttons");
    let no_x = line[..line.find("[ No ]").expect("No")].chars().count();
    let yes_x = line[..line.find("[ Yes ]").expect("Yes")].chars().count();
    let pill = Palette::new(true).style(St::Pill);
    assert_eq!(buf[(no_x as u16, y as u16)].bg, pill.bg.expect("a background"));
    assert_ne!(buf[(yes_x as u16, y as u16)].bg, pill.bg.expect("a background"));
}

#[test]
fn the_prompts_first_key_replaces_the_suggestion_and_backspace_edits_it() {
    let mut h = Harness::new(100, 30);
    open_prompt(&mut h);
    assert!(prompt_text(&h).is_some_and(|t| t.len() == 6), "the current colour is suggested");
    h.keys("f");
    assert_eq!(prompt_text(&h).as_deref(), Some("f"));
    h.key(Key::Backspace);
    h.key(Key::Backspace);
    assert_eq!(prompt_text(&h).as_deref(), Some(""));
    h.key(Key::Esc);
    assert!(h.model.modal.is_none());
    open_prompt(&mut h);
    h.key(Key::Backspace);
    let kept = prompt_text(&h).expect("a prompt");
    assert_eq!(kept.len(), 5, "backspace edits the suggestion instead of replacing it");
    h.keys("a");
    assert_eq!(prompt_text(&h), Some(format!("{kept}a")));
}

#[test]
fn the_prompt_is_limited_and_takes_printable_ascii_only() {
    let mut h = Harness::new(100, 30);
    open_prompt(&mut h);
    h.keys(&"x".repeat(40));
    assert_eq!(prompt_text(&h).map(|t| t.len()), Some(24));
    h.key(Key::Esc);
    open_prompt(&mut h);
    h.keys("ä€\u{7f}\u{1}");
    h.key(Key::Tab);
    h.key(Key::Left);
    assert_eq!(
        prompt_text(&h).map(|t| t.len()),
        Some(6),
        "nothing but printable ASCII is typed: the suggestion is still there"
    );
}

#[test]
fn a_colour_with_a_hash_and_capitals_is_accepted() {
    let mut h = Harness::new(100, 30);
    open_prompt(&mut h);
    h.keys("#FF8800");
    h.key(Key::Enter);
    h.elapse();
    let colour = h
        .backend
        .ops()
        .iter()
        .rev()
        .find_map(|o| if let Op::Keyboard(l) = o { Some(l.color.clone()) } else { None });
    assert_eq!(colour.as_deref(), Some("ff8800"));
}

#[test]
fn the_help_closes_on_any_key_or_click() {
    let mut h = Harness::new(100, 30);
    h.keys("?");
    assert!(h.text().contains("Keys"));
    h.keys("x");
    assert!(!h.text().contains("Keys"), "any key closes it");
    h.keys("?");
    h.input(Input::Press { x: 3, y: 3 });
    assert!(h.model.modal.is_none(), "any click closes it");
}

#[test]
fn every_dialog_fits_the_smallest_screen() {
    for (w, hh) in [(44, 12), (50, 14), (100, 30)] {
        let mut h = Harness::new(w, hh);
        h.keys("?");
        let text = h.text();
        assert!(text.contains("Keys") && widest(&text) <= usize::from(w), "{w}x{hh}");
        h.keys("x");
        ask(&mut h);
        let text = h.text();
        assert!(text.contains("[ Yes ]") || hh < 14, "{w}x{hh}:\n{text}");
        h.key(Key::Char('n'));
        open_prompt(&mut h);
        let text = h.text();
        assert!(text.contains("_"), "{w}x{hh}:\n{text}");
        h.key(Key::Esc);
    }
}

#[test]
fn only_one_question_is_asked_at_a_time() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    let gpu = h.sel("GPU mode", 0);
    h.model.edit(gpu, Value::text("auto"), false); // chosen, waiting for its pause
    h.goto(1);
    let mode = h.sel("Mode", 0);
    h.edit(mode, Value::text("off")); // asks at once
    assert!(matches!(&h.model.modal, Some(Modal::Confirm { text, .. }) if text.starts_with("Stop BOTH")));
    h.elapse(); // the GPU question is due now, but one question is on screen
    assert!(
        matches!(&h.model.modal, Some(Modal::Confirm { text, .. }) if text.starts_with("Stop BOTH")),
        "the first question stays"
    );
    assert_eq!(h.state(gpu), RowState::Pending, "the second change waits instead of replacing it");
    h.key(Key::Char('n'));
    h.elapse();
    assert!(
        matches!(&h.model.modal, Some(Modal::Confirm { text, .. }) if text.starts_with("Switch the GPU mode")),
        "now the second one is asked"
    );
    h.key(Key::Char('n'));
    assert!(h.model.modal.is_none() && h.backend.ops().is_empty(), "nothing was sent");
}

#[test]
fn results_of_writes_nobody_knows_about_are_ignored() {
    let mut h = Harness::new(100, 30);
    h.feed(Msg::Written { ticket: 4242, result: Ok(Done::Unit) });
    h.feed(Msg::Written { ticket: 4243, result: Err("boom".into()) });
    h.feed(Msg::Dropped(4244));
    assert!(h.model.msg.text.is_empty() && h.model.log.is_empty());
}

#[test]
fn the_cycle_key_does_nothing_while_the_power_mode_is_unknown() {
    let mut h = Harness::new(100, 30);
    h.model.data = crate::tui::data::Data::default();
    h.keys("m");
    assert!(h.model.pending.is_empty());
}

#[test]
fn the_loop_wakes_sooner_while_a_change_waits_or_a_drag_is_on() {
    let mut h = Harness::new(100, 30);
    assert!(!h.model.needs_fast_tick());
    h.keys("l");
    assert!(h.model.needs_fast_tick());
    h.elapse();
    assert!(!h.model.needs_fast_tick());
}
