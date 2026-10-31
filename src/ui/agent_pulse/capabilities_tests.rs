use super::{geometry::agent_stage_layout, modal::agent_table_modal_area, render_canvas};
use crate::app::{Action, App};
use crate::catalog::Catalog;
use crate::model::agent::{AgentCapabilities, AgentDetails, AgentId, AgentSnapshot, AgentStatus};
use crate::settings::Settings;
use crate::theme::{Theme, ThemeName};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    widgets::{Paragraph, Widget},
};
use std::time::Instant;

fn app(capabilities: Option<AgentCapabilities>, now: Instant) -> App {
    let mut app = App::new(Settings::default(), Catalog::curated());
    if let Some(capabilities) = capabilities {
        app.configure_agent_capabilities(capabilities);
    }
    app.apply(Action::AgentSnapshot {
        agents: vec![AgentSnapshot {
            id: AgentId::herdr("workspace", "pane"),
            details: AgentDetails::default(),
            status: AgentStatus::Idle,
        }],
        now,
    });
    app.apply(Action::ToggleAgentOverlay);
    app.apply(Action::SelectAgent(AgentId::herdr("workspace", "pane")));
    app
}

fn render(app: &App, area: Rect, now: Instant) -> Buffer {
    let mut buffer = Buffer::empty(area);
    render_canvas(
        app,
        &Theme::for_name(ThemeName::Minimal),
        false,
        now,
        area,
        &mut buffer,
    );
    buffer
}

fn assert_footer(buffer: &Buffer, area: Rect, text: &str) {
    let theme = Theme::for_name(ThemeName::Minimal);
    let mut expected = Buffer::empty(area);
    Paragraph::new(text)
        .alignment(Alignment::Center)
        .style(Style::default().fg(theme.muted))
        .render(area, &mut expected);
    for x in area.x..area.right() {
        assert_eq!(
            buffer.cell((x, area.y)).unwrap().symbol(),
            expected.cell((x, area.y)).unwrap().symbol()
        );
    }
}

#[test]
fn capability_hints_preserve_stage_and_table_at_normal_and_narrow_widths() {
    let now = Instant::now();
    for width in [50, 100] {
        let area = Rect::new(0, 0, width, 30);
        for table_open in [false, true] {
            let mut default = app(None, now);
            let mut herdr = app(Some(AgentCapabilities::HERDR), now);
            let mut read_only = app(Some(AgentCapabilities::READ_ONLY), now);
            if table_open {
                for app in [&mut default, &mut herdr, &mut read_only] {
                    app.apply(Action::OpenAgentDetails);
                    assert!(app.is_agent_details_open());
                }
            }
            let default_buffer = render(&default, area, now);
            let herdr_buffer = render(&herdr, area, now);
            let read_only_buffer = render(&read_only, area, now);
            assert_eq!(default_buffer, herdr_buffer, "default Herdr output changed");

            let stage = agent_stage_layout(area);
            let stage_herdr = if table_open {
                "Tab/↑↓ select · Enter/Esc close · a close"
            } else {
                "Tab/↑↓/click select · Enter table · O open pane · Space play · a/Esc close"
            };
            let stage_read_only = if table_open {
                stage_herdr
            } else {
                "Tab/↑↓/click select · Enter table · Space play · a/Esc close"
            };
            assert_footer(&herdr_buffer, stage.footer, stage_herdr);
            assert_footer(&read_only_buffer, stage.footer, stage_read_only);

            let modal_footer = if table_open {
                let modal = agent_table_modal_area(stage.field, 1);
                let footer = Rect::new(modal.x + 1, modal.y + modal.height - 2, modal.width - 2, 1);
                assert_footer(
                    &herdr_buffer,
                    footer,
                    "O open pane · r rename · Enter/Esc close",
                );
                assert_footer(&read_only_buffer, footer, "Enter/Esc close");
                Some(footer)
            } else {
                None
            };
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if (y == stage.footer.y && x >= stage.footer.x && x < stage.footer.right())
                        || modal_footer.is_some_and(|footer| {
                            y == footer.y && x >= footer.x && x < footer.right()
                        })
                    {
                        continue;
                    }
                    assert_eq!(
                        herdr_buffer.cell((x, y)),
                        read_only_buffer.cell((x, y)),
                        "unexpected change at ({x}, {y})"
                    );
                }
            }
        }
    }
}

#[test]
fn each_table_hint_follows_its_own_capability() {
    let now = Instant::now();
    for width in [50, 100] {
        let area = Rect::new(0, 0, width, 30);
        for (capabilities, expected) in [
            (
                AgentCapabilities {
                    focus: true,
                    rename: false,
                },
                "O open pane · Enter/Esc close",
            ),
            (
                AgentCapabilities {
                    focus: false,
                    rename: true,
                },
                "r rename · Enter/Esc close",
            ),
        ] {
            let mut tested = app(Some(capabilities), now);
            let stage = agent_stage_layout(area);
            let closed_buffer = render(&tested, area, now);
            let stage_hint = if capabilities.focus {
                "Tab/↑↓/click select · Enter table · O open pane · Space play · a/Esc close"
            } else {
                "Tab/↑↓/click select · Enter table · Space play · a/Esc close"
            };
            assert_footer(&closed_buffer, stage.footer, stage_hint);
            let herdr_closed = render(&app(Some(AgentCapabilities::HERDR), now), area, now);
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if y == stage.footer.y && x >= stage.footer.x && x < stage.footer.right() {
                        continue;
                    }
                    assert_eq!(
                        closed_buffer.cell((x, y)),
                        herdr_closed.cell((x, y)),
                        "unexpected change at ({x}, {y})"
                    );
                }
            }
            tested.apply(Action::OpenAgentDetails);
            assert!(tested.is_agent_details_open());
            let buffer = render(&tested, area, now);
            let modal = agent_table_modal_area(agent_stage_layout(area).field, 1);
            let footer = Rect::new(modal.x + 1, modal.y + modal.height - 2, modal.width - 2, 1);
            assert_footer(&buffer, footer, expected);
            assert_footer(
                &buffer,
                stage.footer,
                "Tab/↑↓ select · Enter/Esc close · a close",
            );
            let mut herdr = app(Some(AgentCapabilities::HERDR), now);
            herdr.apply(Action::OpenAgentDetails);
            let herdr_buffer = render(&herdr, area, now);
            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    if y == footer.y && x >= footer.x && x < footer.right() {
                        continue;
                    }
                    assert_eq!(
                        buffer.cell((x, y)),
                        herdr_buffer.cell((x, y)),
                        "unexpected change at ({x}, {y})"
                    );
                }
            }
        }
    }
}
