//! Keys, mouse and dialogs against the simulated laptop: the behaviour of the Python edition's
//! `tests/test_tui.py` case by case, plus the safety cases of the Go edition's rebuilt screen.

use ratatui::style::Color;

use super::*;
use crate::tui::backend::{FanAction, FanReq};
use crate::tui::data::{FanMode, FanState, Mode};
use crate::tui::model::{Modal, Zone};
use crate::tui::row::{Id, slider_index};

fn stops(h: &Harness) -> Vec<Op> {
    h.backend.ops().into_iter().filter(Op::stops_a_fan).collect()
}

fn set_fan(h: &mut Harness, mode: FanMode, cpu: i64, gpu: i64) {
    let mut fast = h.backend.fast();
    fast.fan = Ok(FanState { mode, cpu_duty: cpu, gpu_duty: gpu });
    h.feed(Msg::Fast(Box::new(fast)));
}

fn confirm_focus(h: &Harness) -> Option<usize> {
    match &h.model.modal {
        Some(Modal::Confirm { focus, .. }) => Some(*focus),
        _ => None,
    }
}

fn bar_hit(h: &mut Harness, r: RowRef) -> crate::tui::model::Hit {
    h.draw();
    h.model.frame.hits.iter().copied().find(|x| x.row == r && x.zone == Zone::Bar).expect("a slider bar")
}

#[test]
fn a_terminal_that_is_too_small_says_so() {
    let mut h = Harness::new(30, 8);
    let text = h.text();
    assert!(text.contains("44x12") && text.contains("30x8"), "{text}");
}

#[test]
fn tab_digits_and_shift_tab_change_page() {
    let mut h = Harness::new(100, 30);
    for (key, want) in
        [(Key::Tab, 1), (Key::Char('5'), 4), (Key::BackTab, 3), (Key::Char('1'), 0), (Key::BackTab, 6)]
    {
        h.key(key);
        assert_eq!(h.model.page, want, "after {key:?}");
    }
    h.key(Key::Char('8'));
    assert_eq!(h.model.page, 6, "there is no page 8");
}

#[test]
fn only_q_quits_escape_closes_a_dialog_or_undoes_a_change() {
    let mut h = Harness::new(100, 30);
    h.keys("q");
    assert!(h.model.quit());
    let mut h = Harness::new(100, 30);
    h.key(Key::Esc);
    assert!(!h.model.quit(), "a stray Esc must not quit");
    h.goto(1);
    let mode = h.sel("Mode", 0);
    h.edit(mode, Value::text("off"));
    assert!(confirm_focus(&h).is_some());
    h.key(Key::Esc);
    assert!(h.model.modal.is_none() && !h.model.quit(), "Esc closes the question and does not quit");
    h.input(Input::Quit);
    assert!(h.model.quit(), "Ctrl-C quits");
}

#[test]
fn a_change_is_shown_at_once_written_after_a_pause_and_enter_writes_it_now() {
    let mut h = Harness::new(100, 30);
    let row = (0, 0);
    h.keys("l");
    assert_eq!(h.backend.count("PowerMode"), 0, "the pause lets a run of key presses become one write");
    assert_eq!(h.value(row), Some(Value::text("quiet")), "but it is shown at once");
    assert_eq!(h.state(row), RowState::Pending);
    h.key(Key::Enter);
    assert_eq!(h.backend.count("PowerMode"), 1);
    assert_eq!(h.sim().2, 0, "the firmware has the quiet mode");
    assert!(h.model.msg.text.contains("Power mode"), "{:?}", h.model.msg.text);
    assert_eq!(h.state(row), RowState::Idle);
    h.keys("lh");
    h.elapse();
    assert_eq!(h.sim().2, 0, "a change taken back by the next key press ends where it began");
}

#[test]
fn the_power_mode_is_held_thirty_minutes() {
    let mut h = Harness::new(100, 30);
    h.keys("l");
    h.key(Key::Enter);
    assert_eq!(h.backend.ops().last(), Some(&Op::PowerMode { mode: "quiet".into(), hold_minutes: 30.0 }));
}

#[test]
fn m_steps_through_the_power_modes_and_quick_presses_are_one_write() {
    let mut h = Harness::new(100, 30);
    h.keys("mmm");
    assert_eq!(h.backend.count("PowerMode"), 0, "written before the pause");
    h.elapse();
    let writes: Vec<Op> = h.backend.ops().into_iter().filter(|o| matches!(o, Op::PowerMode { .. })).collect();
    assert_eq!(
        writes,
        vec![Op::PowerMode { mode: "performance".into(), hold_minutes: 30.0 }],
        "eco, quiet, balanced, performance"
    );
    assert_eq!(Mode::ALL[3].name(), "performance");
}

#[test]
fn escape_takes_back_a_change_that_was_not_sent() {
    let mut h = Harness::new(100, 30);
    h.keys("ll");
    h.key(Key::Esc);
    h.elapse();
    assert_eq!(h.backend.count("PowerMode"), 0);
    assert_eq!(h.value((0, 0)), Some(Value::text("eco")));
    assert_eq!(h.state((0, 0)), RowState::Idle);
    assert!(!h.model.needs_fast_tick());
    assert_eq!(h.message().0, "Change undone: nothing was sent");
}

#[test]
fn a_toggle_flips_at_once_and_reads_back() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    h.sel("Battery limit (80 %)", 0);
    h.keys(" ");
    assert_eq!(h.rig.sim.snapshot().battery[0], 1);
    assert!(h.model.msg.text.contains("Battery limit"), "{:?}", h.model.msg.text);
    assert_eq!(h.model.data.bool(ReadKey::Limit), Some(true), "read again after the write");
}

#[test]
fn the_fan_slider_writes_once_when_the_arrow_keys_pause() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    h.keys("j"); // Mode -> CPU fan
    h.keys("lll");
    assert_eq!(h.model.pending.len(), 1, "queued, not written on every key");
    assert_eq!(h.backend.count("Fan("), 0);
    h.elapse();
    assert_eq!(h.backend.count("Fan("), 1);
    assert_eq!(h.sim().0, [45, 30]);
}

#[test]
fn enter_on_a_slider_that_was_not_changed_writes_nothing() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    h.keys("j");
    let before = h.backend.ops().len();
    h.key(Key::Enter);
    h.keys(" ");
    assert!(h.model.modal.is_none());
    assert_eq!(h.backend.ops().len(), before);
    assert!(h.model.pending.is_empty());
    assert!(h.model.msg.text.starts_with("Use"), "it says how to change the value instead");
}

#[test]
fn enter_on_a_different_section_never_stops_a_fan() {
    for service in [false, true] {
        let mut h = if service { Harness::with_service(100, 30) } else { Harness::new(100, 30) };
        h.goto(1);
        h.keys("jhhhh"); // the CPU fan slider is at the bottom, not sent
        h.key(Key::Tab); // away to another page: the unsent stop must not follow
        h.key(Key::Enter);
        h.keys("jjj");
        h.key(Key::Enter);
        h.elapse();
        assert!(stops(&h).is_empty(), "service={service}: {:?}", h.backend.ops());
        assert_eq!(h.sim().0, [30, 30]);
    }
}

#[test]
fn a_slider_with_an_unknown_reading_starts_in_the_middle_never_at_off() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    let row = h.sel("CPU fan", 0);
    h.model.data = crate::tui::data::Data::default();
    let mut fast = h.backend.fast();
    fast.fan = Err("no reading".into());
    h.feed(Msg::Fast(Box::new(fast)));
    assert_eq!(h.value(row), None);
    h.keys("h");
    assert_eq!(h.state(row), RowState::Pending);
    assert!(h.value(row).and_then(|v| v.as_int()).is_some_and(|n| n >= 20), "{:?}", h.value(row));
    assert!(h.model.modal.is_none());
    h.key(Key::Esc);
    h.keys("l");
    assert!(h.value(row).and_then(|v| v.as_int()).is_some_and(|n| n >= 20));
}

#[test]
fn a_fan_that_runs_at_ten_percent_is_not_shown_as_off() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    let row = h.sel("CPU fan", 0);
    let index = |h: &Harness| {
        let r = h.model.row(row);
        slider_index(&r.values, r.exact0, h.value(row).as_ref()).map(|i| r.values[i])
    };
    set_fan(&mut h, FanMode::Manual, 10, 40);
    assert_eq!(index(&h), Some(20), "10 % shows as 20 %");
    set_fan(&mut h, FanMode::Manual, 0, 40);
    assert_eq!(index(&h), Some(0), "0 shows as off");
}

#[test]
fn stopping_a_fan_asks_defaults_to_no_and_is_refused_without_the_service() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    h.keys("j");
    h.keys("hhhhhhhh"); // down to the bottom stop
    assert!(h.model.modal.is_none() && h.sim().0[0] != 0, "moving there does not ask or write");
    h.key(Key::Enter);
    assert_eq!(confirm_focus(&h), Some(1), "a question with No marked");
    h.key(Key::Enter);
    assert!(h.model.modal.is_none() && h.sim().0 == [30, 30]);
    assert!(h.model.msg.text.contains("Cancelled"));
    let row = h.sel("CPU fan", 0);
    assert_eq!(h.value(row), Some(Value::Int(30)), "the slider shows what the fan does");
    h.keys("hhhhhhhh");
    h.key(Key::Enter);
    h.key(Key::Char('y'));
    let (text, st) = h.message();
    assert_eq!(h.sim().0, [30, 30]);
    assert_eq!(st, St::Bad);
    assert!(text.contains("service"), "a stop needs the service: {text}");
}

#[test]
fn the_stop_question_can_be_answered_with_the_arrows_and_enter_but_enter_alone_never_says_yes() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    let row = h.sel("Mode", 0);
    h.edit(row, Value::text("off"));
    assert_eq!(confirm_focus(&h), Some(1));
    h.keys("h");
    assert_eq!(confirm_focus(&h), Some(0), "h moves to Yes");
    h.keys("l");
    assert_eq!(confirm_focus(&h), Some(1), "l moves to No");
    h.key(Key::Enter);
    assert!(h.model.modal.is_none());
    assert_eq!(h.state(row), RowState::Idle);
    assert_eq!(h.backend.count("Fan("), 0, "Enter on No must not write");
    h.edit(row, Value::text("off"));
    h.keys("h");
    h.key(Key::Enter);
    assert_eq!(h.message().1, St::Bad, "Yes was chosen: the write was tried and refused (no service)");
    assert_eq!(h.backend.count("Fan("), 1);
    assert_eq!(h.model.yes_answers(), 1);
}

#[test]
fn a_question_that_is_not_answered_is_a_no_when_you_quit() {
    let mut h = Harness::with_service(100, 30);
    h.goto(1);
    h.keys("j");
    h.keys("hhhhhhhh");
    h.flush();
    assert_eq!(h.backend.count("Fan("), 0, "a stop was written on quit: {:?}", h.backend.ops());
    assert!(h.model.pending.is_empty());
    // And a question that is open (asked, not answered) is dropped too.
    let mut h = Harness::with_service(100, 30);
    h.goto(1);
    let mode = h.sel("Mode", 0);
    h.edit(mode, Value::text("off"));
    assert!(confirm_focus(&h).is_some());
    h.flush();
    assert_eq!(h.backend.count("Fan("), 0, "an open question is a No");
    assert!(h.model.modal.is_none());
    assert_eq!(h.state(mode), RowState::Idle);
}

#[test]
fn with_the_service_a_confirmed_stop_reaches_the_daemon_with_the_confirmation() {
    let mut h = Harness::with_service(100, 30);
    h.goto(1);
    let mode = h.sel("Mode", 0);
    h.edit(mode, Value::text("off"));
    h.keys("h");
    h.key(Key::Enter);
    assert_eq!(h.message(), ("Fans off".to_owned(), St::Good));
    let args = h.rig.daemon.last_args("fan").expect("the fan command");
    assert_eq!(args.get("mode"), Some(&serde_json::json!("off")));
    assert_eq!(args.get("confirm"), Some(&serde_json::json!(true)));
    assert_eq!(stops(&h).len(), 1);
    assert_eq!(h.model.yes_answers(), 1);
}

#[test]
fn a_backend_never_gets_an_unconfirmed_stop_even_if_the_screen_asked_for_one() {
    let h = Harness::with_service(100, 30);
    let req = FanReq { action: FanAction::Manual, cpu: 0, gpu: 40, confirm: false };
    let err = h.backend.execute(&Op::Fan(req)).unwrap_err();
    assert!(err.contains("confirmation"), "{err}");
    let off = FanReq::mode(FanAction::Off);
    assert!(h.backend.execute(&Op::Fan(off)).is_err());
    assert!(h.rig.daemon.last_args("fan").is_none(), "nothing reached the daemon");
}

#[test]
fn a_hint_belongs_to_its_row_and_is_gone_when_you_move() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    h.keys("j");
    h.key(Key::Enter);
    assert!(h.model.msg.text.contains("Use") && h.text().contains("applied by itself"));
    h.keys("j");
    assert!(!h.text().contains("applied by itself"), "the hint does not follow you to another row");
    h.keys("k");
    h.key(Key::Enter);
    h.key(Key::Tab);
    assert!(!h.text().contains("applied by itself"), "nor to another page");
}

#[test]
fn choosing_manual_on_the_fan_mode_row_works_and_keeps_both_fans_running() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    let mode = h.sel("Mode", 0);
    h.edit(mode, Value::text("manual"));
    assert_eq!(h.message().1, St::Good, "{:?}", h.message());
    let (duty, fan, _) = h.sim();
    assert_eq!(fan, "Manual");
    assert!(duty[0].min(duty[1]) >= 20, "{duty:?}");
}

#[test]
fn choosing_manual_never_carries_a_stop_even_when_a_fan_is_stopped() {
    let mut h = Harness::with_service(100, 30);
    h.goto(1);
    set_fan(&mut h, FanMode::Manual, 0, 40);
    let mode = h.sel("Mode", 0);
    h.edit(mode, Value::text("manual"));
    let last = h.backend.ops().into_iter().rev().find(|o| matches!(o, Op::Fan(_))).expect("a fan write");
    assert_eq!(last, Op::Fan(FanReq { action: FanAction::Manual, cpu: 40, gpu: 40, confirm: false }));
}

#[test]
fn moving_one_fan_slider_never_stops_the_other_fan() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    set_fan(&mut h, FanMode::Auto, 0, 0); // an odd reading must not become a stop
    let cpu = h.sel("CPU fan", 0);
    h.edit(cpu, Value::Int(60));
    let (duty, _, _) = h.sim();
    assert_eq!(duty[0], 60);
    assert!(duty[1] >= 20, "{duty:?}");
    assert!(stops(&h).is_empty());
}

#[test]
fn a_fan_that_is_stopped_stays_stopped_and_moving_the_other_one_needs_the_confirmation() {
    let mut h = Harness::with_service(100, 30);
    h.goto(1);
    set_fan(&mut h, FanMode::Manual, 0, 40);
    let gpu = h.sel("GPU fan", 0);
    h.edit(gpu, Value::Int(60));
    let (text, st) = h.message();
    assert_eq!(st, St::Bad);
    assert!(text.contains("confirmation"), "the status line says what happened: {text}");
    assert!(h.rig.daemon.last_args("fan").is_none());
}

#[test]
fn a_custom_colour_is_typed_in_a_dialog_and_checked() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    h.sel("Custom colour", 0);
    h.key(Key::Enter);
    assert!(matches!(h.model.modal, Some(Modal::Prompt { .. })));
    h.keys("ff8800");
    h.key(Key::Enter);
    h.elapse();
    let color = |h: &Harness| match h.backend.ops().iter().rev().find(|o| matches!(o, Op::Keyboard(_))) {
        Some(Op::Keyboard(l)) => l.color.clone(),
        _ => String::new(),
    };
    assert_eq!(color(&h), "ff8800");
    h.key(Key::Enter);
    h.keys("nothex");
    h.key(Key::Enter);
    assert_eq!(h.message().1, St::Bad);
    assert_eq!(color(&h), "ff8800", "a bad colour is refused");
}

#[test]
fn a_value_that_could_not_be_written_is_not_left_on_screen() {
    let mut h = Harness::new(100, 30);
    h.backend.fail_on("Overdrive", "firmware said no");
    h.goto(2);
    let row = h.sel("Panel overdrive", 0);
    let was = h.value(row);
    h.keys(" ");
    let (text, st) = h.message();
    assert_eq!(st, St::Bad);
    assert!(text.contains("firmware said no"), "{text}");
    assert_eq!(h.value(row), was);
    assert_eq!(h.state(row), RowState::Idle);
}

#[test]
fn a_failed_light_write_forgets_the_remembered_colour() {
    let mut h = Harness::new(100, 30);
    h.backend.fail_on("Keyboard", "no");
    h.goto(3);
    let colour = h.sel("Colour", 0);
    h.edit(colour, Value::text("ff0000"));
    assert_eq!(h.message().1, St::Bad);
    assert!(h.model.kb.color.is_none(), "the colour that failed is still remembered");
}

#[test]
fn a_button_marks_its_own_row_busy_and_not_the_row_you_changed_last() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    let other = h.sel("Boot sound", 0);
    h.keys(" ");
    assert_eq!(h.state(other), RowState::Idle);
    h.goto(5);
    let button = h.sel("gaming", 0);
    let effects = h.model.update(h.now, Msg::Input(Input::Key(Key::Enter)));
    assert_eq!(h.state(button), RowState::Writing, "marked while its write is on the way");
    assert_eq!(h.state(other), RowState::Idle);
    let ticket = effects
        .iter()
        .find_map(|e| if let Effect::Write(w) = e { Some(w.ticket) } else { None })
        .expect("a write");
    h.feed(Msg::Written { ticket, result: Ok(Done::Unit) });
    assert_eq!(h.state(button), RowState::Idle);
    assert_eq!(h.message(), ("Profile gaming applied".to_owned(), St::Good));
}

#[test]
fn a_button_that_is_still_running_is_not_started_twice() {
    let mut h = Harness::new(100, 30);
    h.goto(5);
    let button = h.sel("gaming", 0);
    h.model.pages[button.0].rows[button.1].state = RowState::Writing;
    h.key(Key::Enter);
    assert_eq!(h.backend.count("Profile"), 0);
    assert!(h.model.msg.text.contains("Still working"));
}

#[test]
fn a_failed_action_is_reported_not_raised() {
    let mut h = Harness::new(100, 30);
    h.backend.fail_on("Profile", "boom");
    h.goto(5);
    h.sel("gaming", 0);
    h.key(Key::Enter);
    let (text, st) = h.message();
    assert_eq!(st, St::Bad);
    assert!(text.contains("boom"), "{text}");
}

#[test]
fn quitting_sends_a_change_made_a_moment_ago() {
    let mut h = Harness::new(100, 30);
    h.keys("l");
    h.flush();
    assert_eq!(h.sim().2, 0, "the change was lost");
}

#[test]
fn the_bottom_line_shows_the_keys_of_the_selected_row() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    h.sel("CPU fan", 0);
    assert!(h.last_line().contains("adjust"), "slider keys");
    h.goto(3);
    h.sel("Effect", 0);
    assert!(h.last_line().contains("choose"), "choice keys");
    h.goto(2);
    h.sel("Boot sound", 0);
    assert!(h.last_line().contains("switch"), "toggle keys");
    h.goto(5);
    h.sel("gaming", 0);
    assert!(h.last_line().contains("run"), "button keys");
}

#[test]
fn esc_appears_in_the_bottom_line_only_while_a_change_waits() {
    let mut h = Harness::new(100, 30);
    assert!(!h.last_line().contains("Esc"));
    h.keys("l");
    assert!(h.last_line().contains("Esc undo"), "{:?}", h.last_line());
}

#[test]
fn the_pending_marker_is_drawn_last() {
    let mut h = Harness::new(100, 30);
    h.keys("l");
    let line = h.text().lines().nth(2).unwrap_or_default().to_owned();
    assert_eq!(line.chars().nth(100 - 3), Some('↵'), "{line:?}");
    h.key(Key::Enter);
    assert!(!h.text().lines().nth(2).unwrap_or_default().contains('↵'), "marker left after the write");
}

#[test]
fn clicking_yes_or_no_in_the_question_answers_it() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    let row = h.sel("GPU mode", 0);
    h.edit(row, Value::text("auto"));
    let text = h.text();
    assert!(text.contains("Switch the GPU mode") && text.contains("[ Yes ]"), "{text}");
    assert!(h.click("[ No ]", None) && h.model.modal.is_none());
    assert_eq!(h.rig.sim.snapshot().gpu, 2, "No must cancel");
    h.edit(row, Value::text("auto"));
    assert!(h.click("[ Yes ]", None));
    assert_eq!(h.rig.sim.snapshot().gpu, 3, "Yes must write");
}

#[test]
fn dragging_a_slider_follows_the_pointer_and_writes_when_released() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    let row = h.sel("Brightness", 0);
    let bar = bar_hit(&mut h, row);
    let values = h.model.row(row).values.clone();
    h.input(Input::Press { x: bar.x0, y: bar.y });
    assert_eq!(h.value(row), Some(Value::Int(values[0])), "a press at the left end");
    let before = h.backend.count("Keyboard");
    let mut x = bar.x0;
    while x < bar.x0 + bar.extra {
        h.input(Input::Drag { x, y: bar.y });
        x += 3;
    }
    h.input(Input::Drag { x: bar.x0 + bar.extra + 10, y: bar.y });
    assert_eq!(h.value(row), Some(Value::Int(*values.last().unwrap())), "past the end is the end");
    assert_eq!(h.backend.count("Keyboard"), before, "a drag must not write on every step");
    h.input(Input::Release);
    assert_eq!(h.backend.count("Keyboard") - before, 1);
    assert_eq!(h.api().keyboard_brightness().unwrap(), 50);
    assert!(h.model.drag.is_none());
}

#[test]
fn a_key_press_ends_a_drag() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    let row = h.sel("Brightness", 0);
    let bar = bar_hit(&mut h, row);
    h.input(Input::Press { x: bar.x0, y: bar.y });
    assert!(h.model.drag.is_some());
    h.keys("j");
    assert!(h.model.drag.is_none());
}

#[test]
fn clicking_a_tab_changes_page() {
    let mut h = Harness::new(100, 30);
    assert!(h.click("Fans", Some(1)));
    assert_eq!(h.model.page, 1);
}

#[test]
fn clicking_an_option_selects_it() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    assert!(h.click("Turbo", None));
    assert_eq!(h.sim().2, 5);
}

#[test]
fn clicking_a_toggle_flips_it() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    let row = h.sel("Boot sound", 0);
    h.draw();
    let hit = h
        .model
        .frame
        .hits
        .iter()
        .copied()
        .find(|x| x.row == row && x.zone == Zone::Toggle)
        .expect("a toggle");
    h.input(Input::Press { x: hit.x0 + 1, y: hit.y });
    assert_eq!(h.rig.sim.snapshot().boot, 1);
}

#[test]
fn clicking_a_slider_sets_that_position_and_the_wheel_moves_the_selection() {
    let mut h = Harness::new(100, 30);
    h.goto(1);
    h.draw();
    let bar = h.model.frame.hits.iter().copied().find(|x| x.zone == Zone::Bar).expect("a slider");
    h.input(Input::Press { x: bar.x1 - 1, y: bar.y });
    let r = h.model.cursor();
    let row = h.model.row(r);
    assert_eq!(slider_index(&row.values, row.exact0, h.value(r).as_ref()), Some(row.values.len() - 1));
    let sel = h.model.current().sel;
    h.input(Input::Wheel(1));
    assert_ne!(h.model.current().sel, sel, "the wheel did not move the selection");
}

#[test]
fn clicks_while_a_dialog_is_open_do_not_reach_the_page() {
    let mut h = Harness::new(100, 30);
    h.goto(2);
    let row = h.sel("GPU mode", 0);
    h.edit(row, Value::text("auto"));
    h.click("Turbo", None);
    assert_eq!(h.backend.count("PowerMode"), 0, "the click reached the page");
    assert!(h.model.modal.is_some());
}

#[test]
fn a_click_on_an_option_that_no_longer_exists_is_ignored() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    h.model.data.apply_slow(SlowReading {
        key: ReadKey::SavedLightings,
        value: Ok(crate::tui::data::SlowValue::List(vec!["a".into(), "b".into(), "c".into()])),
    });
    h.sel("Saved lighting", 0);
    h.draw(); // the frame has three options
    h.model.data.apply_slow(SlowReading {
        key: ReadKey::SavedLightings,
        value: Ok(crate::tui::data::SlowValue::List(vec![])),
    });
    let hits: Vec<_> =
        h.model.frame.hits.iter().copied().filter(|x| matches!(x.zone, Zone::Option(_))).collect();
    assert!(hits.len() >= 3);
    for hit in hits {
        h.input(Input::Press { x: hit.x0, y: hit.y });
    }
    assert_eq!(h.backend.count("SavedLighting"), 0, "a stale frame writes nothing");
}

#[test]
fn selection_skips_rows_that_cannot_be_used_and_wraps() {
    let mut h = Harness::new(100, 30); // no service: the automatic mode row is not selectable
    assert_eq!(h.model.current().sel, 0);
    h.keys("j");
    let label = h.model.row(h.model.cursor()).label.clone();
    assert_eq!(label, "Fans", "down skipped to the Fans row");
    h.keys("j");
    assert_eq!(h.model.current().sel, 0, "down from the last row wraps");
    h.key(Key::End);
    assert_eq!(h.model.current().sel, 2);
    h.key(Key::Home);
    assert_eq!(h.model.current().sel, 0);
    h.goto(2);
    h.key(Key::PageDown);
    h.key(Key::PageUp);
    assert!(h.model.selectable(h.model.cursor()), "paging landed on a row that cannot be used");
}

#[test]
fn r_reads_everything_again_and_forgets_remembered_light_changes() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    h.model.kb.color = Some("123456".into());
    h.keys("r");
    assert!(h.model.kb.color.is_none());
    assert_eq!(h.model.msg.text, "Reading everything again…");
}

#[test]
fn ctrl_l_asks_for_a_full_repaint() {
    let mut h = Harness::new(100, 30);
    h.key(Key::Redraw);
    assert!(h.model.take_repaint());
    assert!(!h.model.take_repaint());
}

#[test]
fn light_changes_are_remembered_only_while_something_is_on_its_way() {
    let mut h = Harness::new(100, 30);
    h.goto(3);
    h.model.kb.color = Some("123456".into());
    h.keys("l"); // a change is pending on the Lighting page (Effect row)
    h.key(Key::Tab);
    assert_eq!(h.model.kb.color.as_deref(), Some("123456"), "a change is waiting: nothing is forgotten");
    h.key(Key::Esc);
    h.key(Key::Tab);
    assert!(h.model.kb.color.is_none(), "idle: show what the hardware has");
}

#[test]
fn a_chosen_value_is_a_choice_of_the_user_only() {
    let mut h = Harness::new(100, 30);
    for page in 0..7 {
        h.goto(page);
        h.key(Key::Enter);
        h.key(Key::Char(' '));
    }
    let writes: Vec<Op> = h.backend.ops();
    // Enter and Space only ever run a button or flip a toggle; no choice or slider was chosen.
    assert!(
        writes
            .iter()
            .all(|o| !matches!(o, Op::Fan(_) | Op::PowerMode { .. } | Op::UsbCharge(_) | Op::GpuMode(_))),
        "{writes:?}"
    );
}

#[test]
fn the_dashboard_row_ids_are_what_the_cycle_key_assumes() {
    let h = Harness::new(100, 30);
    assert_eq!(h.model.row((0, 0)).id, Id::PowerMode, "the m key steps the first row of the first page");
}

#[test]
fn the_red_of_a_hot_temperature_is_drawn_as_bad() {
    let mut h = Harness::new(100, 40);
    h.rig.sim.set_sensors([90, 50, 50], [3000, 3000]);
    h.fast();
    let buf = h.draw();
    let text = buffer_text(&buf);
    let (y, line) = text.lines().enumerate().find(|(_, l)| l.contains("90 °C")).expect("the CPU temperature");
    let x = line.chars().position(|c| c == '9').expect("a 9");
    assert_eq!(buf[(x as u16, y as u16)].fg, Color::Red);
}
