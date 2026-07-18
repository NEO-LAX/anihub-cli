//! Search-specific overlays.

use super::*;

pub(super) fn render_sort_popup(f: &mut Frame, app: &AppState) {
    let Some(selected) = app.search_ordering.popup else {
        return;
    };
    let actions = [
        ("Enter", "Застосувати / ↕", color_highlight()),
        ("Esc", "", color_dim()),
    ];
    let area = centered_fixed(f.area(), dialog_width_for(54, &actions), 10);
    let block = dialog_block(" Сортування пошуку ", color_highlight(), color_highlight());
    f.render_widget(Clear, area);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let items = SearchSort::ALL
        .iter()
        .map(|sort| {
            let active = *sort == app.search_ordering.sort;
            let reversed = active && app.search_ordering.reversed;
            let marker = if active { "✓" } else { " " };
            ListItem::new(format!(
                "{marker} {} · {}",
                sort.label(),
                sort.order_label(reversed)
            ))
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .highlight_symbol(">> ")
        .highlight_style(selection_style());
    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(selected));
    f.render_stateful_widget(list, rows[0], &mut state);
    f.render_widget(
        Paragraph::new(action_footer_line(&actions)).alignment(Alignment::Center),
        rows[1],
    );
}
