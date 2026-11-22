use super::super::*;
use magi_proto::UiCommand;
use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

#[test]
fn captions_render_and_refresh_without_changing_selected_routes() {
    let mut app = App::new();
    app.choices = serde_json::from_value(serde_json::json!([
        {"name":"meta/muse-spark-1.3-contributor","display_name":"training-enabled: Muse Spark 1.3 Contributor",
         "context_window":1048576,"requirement":""},
        {"name":"meta/muse-spark-1.3","context_window":1048576,"requirement":""}
    ])).expect("choices");
    app.open_model_picker();
    let overlay = app.overlay.as_ref().expect("picker");
    let rows = overlay.render(80);
    assert_eq!(usize::from(overlay.height()), rows.len());
    let mut terminal = Terminal::new(TestBackend::new(80, 8)).expect("terminal");
    terminal
        .draw(|frame| frame.render_widget(Paragraph::new(rows), frame.area()))
        .expect("render");
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("training-enabled: Muse Spark 1.3 Contributor"));
    assert_eq!(
        overlay
            .list()
            .expect("list")
            .current()
            .expect("choice")
            .value,
        "meta/muse-spark-1.3-contributor"
    );
    let picker = app
        .overlay
        .as_mut()
        .expect("picker")
        .picker()
        .expect("list");
    picker.next();
    assert!(
        !magi_tui::picker::render(picker, 80)
            .iter()
            .any(|line| line.to_string().contains("training-enabled"))
    );
    picker.previous();
    let mut choices = app.choices.clone();
    choices[0].display_name = Some("updated label".into());
    app.apply(HarnessEvent::ModelsRefreshed {
        choices,
        warning: None,
    });
    assert!(
        app.overlay
            .as_ref()
            .expect("picker")
            .render(80)
            .iter()
            .any(|line| line.to_string().contains("updated label"))
    );
    assert!(
        matches!(app.chose("meta/muse-spark-1.3-contributor".into()),
        Some(UiCommand::SetModel { name }) if name == "meta/muse-spark-1.3-contributor")
    );
}
