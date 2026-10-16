//! Running blocks: one place at a time, pinned, picked and flashed.

use super::*;
use magi_proto::{Cursor, HarnessEvent};

/// A conversation with one call whose tool left `htop` running in its block.
fn running() -> App {
    let mut app = App::new();
    app.apply(HarnessEvent::ToolCallStarted {
        cursor: Cursor(1),
        id: ToolCallId::new("t1"),
        name: "screen".into(),
        args: r#"{"command":"htop","title":"htop"}"#.into(),
    });
    app.apply(HarnessEvent::Surfaced {
        cursor: Cursor(2),
        id: ToolCallId::new("s0"),
        tool: "screen".into(),
        rows: 4,
        about: "system monitor".into(),
        place: Place::Block,
        call: Some(ToolCallId::new("t1")),
        detach: true,
    });
    app.lay_out(60);
    app.live_rows = 0..u16::try_from(app.stack.len()).expect("short");
    app
}

fn s0() -> ToolCallId {
    ToolCallId::new("s0")
}

#[test]
fn a_block_left_running_does_not_take_the_keyboard() {
    let app = running();
    assert!(app.holding().is_none(), "the prompt keeps the keys");
    assert_eq!(app.running().count(), 1);
}

#[test]
fn giving_it_the_keyboard_and_taking_it_back_leaves_it_running() {
    let mut app = running();
    app.focus_on(&s0());
    assert_eq!(app.holding().map(|held| held.id.clone()), Some(s0()));
    app.let_go();
    assert!(app.holding().is_none());
    assert_eq!(app.running().count(), 1, "still running");
}

#[test]
fn its_block_offers_a_pin_and_says_where_it_went() {
    let mut app = running();
    let live = lives(&app.surfaces, None);
    assert_eq!(live[0].pin, Some(false));
    assert_eq!(live[0].away, None);
    app.toggle_pin(&s0());
    let live = lives(&app.surfaces, None);
    assert_eq!(live[0].pin, Some(true));
    assert!(live[0].away.is_some_and(|away| away.contains("pinned")));
}

#[test]
fn a_pinned_block_is_drawn_above_the_prompt_folded_until_opened() {
    let mut app = running();
    app.toggle_pin(&s0());
    let docked = app.docking();
    assert_eq!(docked.len(), 1);
    assert!(!docked[0].1, "folded");
    assert_eq!(
        docked[0].0.away, None,
        "drawn there, not pointing elsewhere"
    );
    app.toggle_docked(&s0());
    assert!(app.docking()[0].1, "open");
    assert_eq!(app.holding().map(|held| held.id.clone()), Some(s0()));
    app.toggle_docked(&s0());
    assert!(!app.docking()[0].1, "folded again");
    assert!(app.holding().is_none(), "and the keys are the prompt's");
}

#[test]
fn unpinning_puts_it_back_in_the_conversation() {
    let mut app = running();
    app.toggle_pin(&s0());
    app.toggle_pin(&s0());
    assert!(app.docking().is_empty());
    assert_eq!(lives(&app.surfaces, None)[0].away, None);
}

#[test]
fn a_block_picked_while_on_screen_flashes_rather_than_moving() {
    let mut app = running();
    app.press_running();
    assert!(app.pane_titled("running"));
    app.pick_running("s0");
    assert!(app.pane.is_none(), "the list closes");
    assert!(app.flash.is_some());
    assert!(!app.surfaces[0].viewing, "drawn in one place only");
    assert!(app.holding().is_none());
}

#[test]
fn a_block_picked_while_off_screen_is_used_in_the_float() {
    let mut app = running();
    app.live_rows = 0..0;
    app.pick_running("s0");
    assert!(app.surfaces[0].viewing);
    assert_eq!(app.holding().map(|held| held.id.clone()), Some(s0()));
    assert!(
        lives(&app.surfaces, app.focus.as_ref())[0]
            .away
            .is_some_and(|away| away.contains("float")),
        "its block says where it went"
    );
    app.let_go();
    assert!(!app.surfaces[0].viewing, "back in its block");
}

#[test]
fn a_flash_lights_and_goes_dark_and_then_ends() {
    let mut app = running();
    app.flash = Some((s0(), std::time::Instant::now()));
    assert_eq!(app.flashing(), Some(s0()), "lit at first");
    app.flash = Some((
        s0(),
        std::time::Instant::now() - std::time::Duration::from_millis(200),
    ));
    assert_eq!(app.flashing(), None, "dark between");
    app.flash = Some((
        s0(),
        std::time::Instant::now() - std::time::Duration::from_secs(1),
    ));
    assert_eq!(app.flashing(), None);
    assert!(app.flash.is_none(), "and over");
}

#[test]
fn closing_it_lets_go_of_the_keyboard() {
    let mut app = running();
    app.focus_on(&s0());
    app.unsurfaced(&s0());
    assert!(app.holding().is_none());
    assert_eq!(app.running().count(), 0);
}

#[test]
fn a_surface_that_holds_its_call_takes_the_keyboard_and_offers_no_pin() {
    let mut app = App::new();
    app.apply(HarnessEvent::Surfaced {
        cursor: Cursor(1),
        id: ToolCallId::new("s1"),
        tool: "dino".into(),
        rows: 4,
        about: "a game".into(),
        place: Place::Block,
        call: Some(ToolCallId::new("t9")),
        detach: false,
    });
    assert_eq!(
        app.holding().map(|held| held.id.clone()),
        Some(ToolCallId::new("s1"))
    );
    assert_eq!(lives(&app.surfaces, None)[0].pin, None);
    assert_eq!(app.running().count(), 0, "it is not left running");
}
