use crate::app::{duration_mins, fmt_range, fmt_time, App, InputMode, Mode, SortMode};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);
    draw_titlebar(f, app, chunks[0]);
    match app.mode {
        Mode::Channels => draw_channels(f, app, chunks[1]),
        Mode::Programmes => draw_programmes(f, app, chunks[1]),
        Mode::Detail => draw_detail(f, app, chunks[1]),
        Mode::Help => draw_help(f, app, chunks[1]),
    }
    draw_statusbar(f, app, chunks[2]);
}

fn mode_name(app: &App) -> &'static str {
    match app.mode {
        Mode::Channels => "CHANNELS",
        Mode::Programmes => "PROGRAMMES",
        Mode::Detail => "DETAIL",
        Mode::Help => "HELP",
    }
}

fn draw_titlebar(f: &mut Frame, app: &App, area: Rect) {
    let sort = match app.sort {
        SortMode::Name => "name",
        SortMode::Count => "count",
    };
    let title = Line::from(vec![
        Span::styled(
            " EPG ",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .bg(Color::Cyan)
                .fg(Color::Black),
        ),
        Span::raw(" "),
        Span::styled(
            format!(
                "{} channels  {} programmes",
                app.channels.len(),
                app.channels.iter().map(|c| c.prog_count).sum::<i64>()
            ),
            Style::default().fg(Color::Gray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{}] ", mode_name(app)),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(format!("sort:{} ", sort)),
        if app.query.is_empty() {
            Span::raw("")
        } else {
            // The active search query stays visible after confirming.
            Span::styled(
                format!("/{} ", app.query),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        },
        if app.filter_only {
            Span::styled("FILTER ", Style::default().fg(Color::Magenta))
        } else {
            Span::raw("")
        },
        Span::raw(format!("now: {}", fmt_time(app.now))),
    ]);
    let p = Paragraph::new(title).style(Style::default().bg(Color::DarkGray));
    f.render_widget(p, area);
}

fn draw_channels(f: &mut Frame, app: &mut App, area: Rect) {
    let inner = area;
    let h = inner.height as usize;
    app.set_viewport(h.saturating_sub(2)); // header + margin

    let header = Row::new(vec![
        Cell::from(Span::styled(
            "Channel",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "#",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Now",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Next",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ])
    .height(1)
    .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    let name_w = Constraint::Min(20);
    let cnt_w = Constraint::Length(7);

    let visible = app.view.len();
    let start = app.offset.min(visible.saturating_sub(1));
    let end = (start + h.saturating_sub(2)).min(visible);
    let end = end.max(start);

    let mut rows: Vec<Row> = Vec::new();
    let selected_view_idx = if app.view.is_empty() {
        usize::MAX
    } else {
        app.selected
    };
    let searching = !app.query.is_empty();
    let query_lc = app.query.to_lowercase();
    for vi in start..end {
        let ci = app.view[vi];
        let ch = app.channels[ci].clone();
        let (cur, nxt) = app.now_next(&ch.channel_id);
        let is_sel = vi == selected_view_idx;

        let name_spans: Vec<Span> = if searching {
            let prog_match = app.prog_match.get(&ch.channel_id).copied().unwrap_or(0);
            // Matches in the name are highlighted inline; the badge counts
            // matching programmes on this channel.
            let mut spans =
                highlight_spans(&truncate(&ch.display_name, 42), &query_lc, Style::default());
            if prog_match > 0 {
                spans.push(Span::styled(
                    format!(" \u{27eb}{}\u{27ed}", prog_match),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            spans
        } else {
            vec![Span::raw(truncate(&ch.display_name, 48))]
        };

        let now_cell = match &cur {
            Some(p) => Span::styled(
                truncate(&p.title, 40).to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            None => Span::styled("—", Style::default().fg(Color::DarkGray)),
        };
        let next_cell = match &nxt {
            Some(p) => Span::styled(
                truncate(&p.title, 40).to_string(),
                Style::default().fg(Color::Gray),
            ),
            None => Span::styled("·", Style::default().fg(Color::DarkGray)),
        };

        let row = Row::new(vec![
            Cell::from(Line::from(name_spans)),
            Cell::from(format!("{}", ch.prog_count)),
            Cell::from(now_cell),
            Cell::from(next_cell),
        ]);
        let row = if is_sel {
            row.style(
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            row
        };
        rows.push(row);
    }

    let table = Table::new(
        rows,
        [
            name_w,
            cnt_w,
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::TOP).title(""));
    f.render_widget(table, inner);
}

fn draw_programmes(f: &mut Frame, app: &mut App, area: Rect) {
    let h = area.height as usize;
    app.set_viewport(h.saturating_sub(2));

    let header = Row::new(vec![
        Cell::from(Span::styled(
            "Start",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Stop",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Min",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Cell::from(Span::styled(
            "Title",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ])
    .height(1)
    .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    let visible = app.prog_view.len();
    let start = app.prog_offset.min(visible.saturating_sub(1));
    let end = (start + h.saturating_sub(2)).min(visible);
    let end = end.max(start);
    let sel = if app.prog_view.is_empty() {
        usize::MAX
    } else {
        app.prog_selected
    };

    let now = app.now;
    let query_lc = app.query.to_lowercase();
    let mut rows = Vec::new();
    for vi in start..end {
        let pi = app.prog_view[vi];
        let p = app.programmes[pi].clone();
        let airing = p.start_ts <= now && p.stop_ts > now;
        let base_style = if airing {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else if p.stop_ts <= now {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };
        let is_sel = vi == sel;
        let row = Row::new(vec![
            Cell::from(fmt_time(p.start_ts)),
            Cell::from(fmt_time(p.stop_ts)),
            Cell::from(format!("{}", duration_mins(p.start_ts, p.stop_ts))),
            Cell::from(Line::from(highlight_spans(&p.title, &query_lc, base_style))),
        ]);
        let row = if is_sel {
            row.style(
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            row
        };
        rows.push(row);
    }

    let title = match &app.cur_channel {
        Some(ch) => format!(
            " {} \u{2014} {} programmes ",
            ch.display_name,
            app.programmes.len()
        ),
        None => " Programmes ".to_string(),
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::TOP).title(title));
    f.render_widget(table, area);
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let Some(p) = &app.detail else {
        return;
    };
    let inner = Block::default().borders(Borders::ALL).title(" Programme ");
    let inner_area = inner.inner(area);
    f.render_widget(inner, area);

    let h = inner_area.height as usize;
    let query_lc = app.query.to_lowercase();
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(highlight_spans(
        &p.title,
        &query_lc,
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    )));
    lines.push(Line::from(""));
    let channel_label = app
        .cur_channel
        .as_ref()
        .map(|c| c.display_name.as_str())
        .unwrap_or(&p.channel_id);
    lines.push(Line::from(vec![
        Span::styled("Channel: ", Style::default().fg(Color::Yellow)),
        Span::raw(channel_label.to_string()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("When:    ", Style::default().fg(Color::Yellow)),
        Span::raw(fmt_range(p.start_ts, p.stop_ts)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Duration: ", Style::default().fg(Color::Yellow)),
        Span::raw(format!("{} min", duration_mins(p.start_ts, p.stop_ts))),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Description",
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Gray),
    )));
    lines.push(Line::from(""));
    let desc = if p.desc.is_empty() {
        "(no description)"
    } else {
        &p.desc
    };
    // Wrap to the actual panel width rather than a hardcoded column.
    let wrap_w = (inner_area.width as usize).saturating_sub(2).max(20);
    for w in textwrap_lines(desc, wrap_w) {
        lines.push(Line::from(highlight_spans(&w, &query_lc, Style::default())));
    }

    let total = lines.len();
    let max_scroll = total.saturating_sub(h);
    if app.detail_scroll > max_scroll {
        app.detail_scroll = max_scroll;
    }
    let p_widget = Paragraph::new(lines).scroll((app.detail_scroll as u16, 0));
    f.render_widget(p_widget, inner_area);
}

fn draw_help(f: &mut Frame, _app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help — press ? or Esc to close ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let help = vec![
        ("j / k", "move down / up (detail: scroll)"),
        (
            "l / Enter",
            "drill in: channels \u{2192} schedule \u{2192} detail",
        ),
        ("Esc", "back: dismiss search/filter, else previous screen"),
        ("h / Backspace", "straight back to the previous screen"),
        ("g g", "go to top"),
        ("G", "go to bottom"),
        ("t", "schedule: jump to the programme airing now/next"),
        ("Ctrl-d / Ctrl-u", "half page down / up"),
        ("Ctrl-f / Ctrl-b / PgDn / PgUp", "full page down / up"),
        ("H / M / L", "top / middle / bottom of viewport"),
        ("/", "search (Enter to confirm, Esc to cancel)"),
        ("n / N", "next / previous search match"),
        ("f", "toggle filter (show only matches)"),
        ("s", "channels: cycle sort (name / programme count)"),
        ("r", "refresh data (re-download + re-import)"),
        ("?", "toggle this help"),
        ("q", "quit"),
    ];
    let lines: Vec<Line> = std::iter::once(Line::from(Span::styled(
        "Keybindings",
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    )))
    .chain(help.iter().map(|(k, d)| {
        Line::from(vec![
            Span::styled(format!("  {:<18}", k), Style::default().fg(Color::Green)),
            Span::raw(d.to_string()),
        ])
    }))
    .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn draw_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let top = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let status_area = top[0];
    let hint_area = top[1];

    match app.input {
        InputMode::Search => {
            let line = Line::from(vec![
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(app.query.as_str()),
                Span::styled("▏", Style::default().fg(Color::Gray)),
            ]);
            f.render_widget(
                Paragraph::new(line).style(Style::default().bg(Color::Black)),
                status_area,
            );
        }
        _ => {
            let pos = if app.mode == Mode::Channels {
                if app.view.is_empty() {
                    0
                } else {
                    app.selected + 1
                }
            } else if app.mode == Mode::Programmes {
                if app.prog_view.is_empty() {
                    0
                } else {
                    app.prog_selected + 1
                }
            } else {
                0
            };
            let total = if app.mode == Mode::Channels {
                app.view.len()
            } else if app.mode == Mode::Programmes {
                app.prog_view.len()
            } else {
                0
            };
            let status = if app.status.is_empty() {
                format!(
                    " {}  {}/{}  {:>3}% ",
                    mode_name(app),
                    pos,
                    total,
                    pct(pos, total)
                )
            } else {
                format!(" {}  {}/{} — {} ", mode_name(app), pos, total, app.status)
            };
            f.render_widget(
                Paragraph::new(Line::from(status))
                    .style(Style::default().bg(Color::DarkGray).fg(Color::White))
                    .alignment(Alignment::Left),
                status_area,
            );
        }
    }

    let hint = match (app.mode, app.input) {
        (_, InputMode::Search) => "Enter confirm • Esc cancel".to_string(),
        (Mode::Channels, _) => "j/k move • Enter/l open • / search • n/N next • f filter • s sort • r refresh • ? help • q quit".to_string(),
        (Mode::Programmes, _) => "j/k move • Enter/l detail • t now • / search • n/N next • f filter • Esc back • q quit".to_string(),
        (Mode::Detail, _) => "j/k scroll • g/G top/bottom • Esc back • q quit".to_string(),
        (Mode::Help, _) => "Esc / ? close".to_string(),
    };
    f.render_widget(
        Paragraph::new(Line::from(hint)).style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

fn pct(pos: usize, total: usize) -> usize {
    (pos * 100).checked_div(total).unwrap_or(0)
}

/// Style used to highlight search matches in lists and the detail view.
fn match_style() -> Style {
    Style::default()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

/// Split `text` into spans, highlighting case-insensitive occurrences of
/// `query_lower` (which must already be lowercased) with the match style.
fn highlight_spans(text: &str, query_lower: &str, base: Style) -> Vec<Span<'static>> {
    if query_lower.is_empty() {
        return vec![Span::styled(text.to_string(), base)];
    }
    let qlen = query_lower.chars().count();
    let mut spans = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while i < text.len() {
        if starts_with_ignore_case(&text[i..], query_lower) {
            if plain_start < i {
                spans.push(Span::styled(text[plain_start..i].to_string(), base));
            }
            // Advance by the query's char count, not its byte length: the
            // matched text may differ in case (e.g. 'A' vs 'a'), and slicing
            // must stay on char boundaries.
            let end = text[i..]
                .char_indices()
                .nth(qlen)
                .map(|(b, _)| i + b)
                .unwrap_or(text.len());
            spans.push(Span::styled(text[i..end].to_string(), match_style()));
            i = end;
            plain_start = end;
        } else {
            let c = text[i..].chars().next().unwrap();
            i += c.len_utf8();
        }
    }
    if plain_start < text.len() {
        spans.push(Span::styled(text[plain_start..].to_string(), base));
    }
    spans
}

/// True if `s` starts with `query_lower`, comparing char-by-char ignoring
/// case. `query_lower` must already be lowercase.
fn starts_with_ignore_case(s: &str, query_lower: &str) -> bool {
    let mut sc = s.chars();
    for qc in query_lower.chars() {
        let Some(c) = sc.next() else { return false };
        if !c.to_lowercase().eq(std::iter::once(qc)) {
            return false;
        }
    }
    true
}

fn truncate(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    // Add an ellipsis if we actually cut something and there's room for it.
    if out.len() < s.len() && w < max {
        out.push('…');
    }
    out
}

fn textwrap_lines(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        let mut w = 0usize;
        for word in para.split_whitespace() {
            let ww = unicode_width::UnicodeWidthStr::width(word);
            let extra = if line.is_empty() { 0 } else { 1 };
            if w + extra + ww > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                w = 0;
            }
            if !line.is_empty() {
                line.push(' ');
                w += 1;
            }
            line.push_str(word);
            w += ww;
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlighted_parts(text: &str, query: &str) -> Vec<String> {
        highlight_spans(text, &query.to_lowercase(), Style::default())
            .into_iter()
            .filter(|s| s.style.bg == Some(Color::Yellow))
            .map(|s| s.content.into_owned())
            .collect()
    }

    #[test]
    fn highlight_is_case_insensitive() {
        assert_eq!(highlighted_parts("The Morning News", "news"), vec!["News"]);
        assert_eq!(highlighted_parts("NEWS at Ten", "news"), vec!["NEWS"]);
        assert_eq!(
            highlighted_parts("news and more news", "news"),
            vec!["news", "news"]
        );
    }

    #[test]
    fn highlight_handles_multibyte_and_no_match() {
        assert_eq!(
            highlighted_parts("Télévision Française", "FRAN"),
            vec!["Fran"]
        );
        assert!(highlighted_parts("nothing here", "xyz").is_empty());
        // Empty query: single unstyled span, no highlight.
        assert!(highlighted_parts("anything", "").is_empty());
        // Match on a multibyte char with a case difference (2-byte 'É' vs 'é').
        assert_eq!(highlighted_parts("Émission", "émi"), vec!["Émi"]);
    }

    #[test]
    fn highlight_covers_adjacent_and_full_matches() {
        assert_eq!(highlighted_parts("aaaa", "aa"), vec!["aa", "aa"]);
        assert_eq!(highlighted_parts("abc", "abc"), vec!["abc"]);
    }

    #[test]
    fn titlebar_displays_active_query() {
        let path = std::env::temp_dir().join(format!("epg-ui-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = crate::db::Database::open(&path).unwrap();
        let mut app = crate::app::App::new(db, String::new()).unwrap();
        app.query = "news".to_string();

        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();

        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            text.contains("/news"),
            "titlebar should show the active search query"
        );
        let _ = std::fs::remove_file(&path);
    }

    fn test_db() -> crate::db::Database {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("epg-ui-test-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        crate::db::Database::open(&path).unwrap()
    }

    fn buffer_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    // ---------- truncate ----------

    #[test]
    fn truncate_leaves_short_strings_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello", "exact fit is unchanged");
    }

    #[test]
    fn truncate_respects_max_width() {
        let s = truncate("hello world", 8);
        assert!(s.starts_with("hello"));
        assert!(unicode_width::UnicodeWidthStr::width(s.as_str()) <= 8);
    }

    #[test]
    fn truncate_adds_ellipsis_when_room_remains() {
        // The wide char '世' (2 cols) doesn't fit in the one remaining column,
        // leaving room for the ellipsis to be appended.
        let s = truncate("abcd世efgh", 5);
        assert_eq!(s, "abcd\u{2026}");
    }

    #[test]
    fn truncate_counts_wide_chars_as_double_width() {
        // '世' and '界' are each two columns wide.
        let s = truncate("世界世界", 5);
        assert!(unicode_width::UnicodeWidthStr::width(s.as_str()) <= 5);
        assert!(s.ends_with('\u{2026}'));
    }

    // ---------- textwrap_lines ----------

    #[test]
    fn textwrap_wraps_at_width() {
        let lines = textwrap_lines("the quick brown fox jumps over the lazy dog", 10);
        assert!(lines.len() > 1, "long text should wrap");
        for l in &lines {
            assert!(
                unicode_width::UnicodeWidthStr::width(l.as_str()) <= 10,
                "line {l:?} exceeds width"
            );
        }
    }

    #[test]
    fn textwrap_preserves_paragraph_breaks() {
        let lines = textwrap_lines("para one\n\npara two", 80);
        assert_eq!(lines, vec!["para one", "", "para two"]);
    }

    #[test]
    fn textwrap_keeps_overlong_word_on_its_own_line() {
        let lines = textwrap_lines("a supercalifragilistic word", 8);
        assert!(lines.iter().any(|l| l == "supercalifragilistic"));
    }

    #[test]
    fn textwrap_empty_input_yields_single_empty_line() {
        let lines = textwrap_lines("", 10);
        assert_eq!(lines, vec![String::new()]);
    }

    // ---------- misc helpers ----------

    #[test]
    fn pct_computes_percentage_and_handles_zero_total() {
        assert_eq!(pct(1, 4), 25);
        assert_eq!(pct(3, 4), 75);
        assert_eq!(pct(0, 0), 0, "zero total must not panic");
        assert_eq!(pct(5, 0), 0);
    }

    #[test]
    fn starts_with_ignore_case_matches_prefixes() {
        assert!(starts_with_ignore_case("Hello World", "hello"));
        assert!(starts_with_ignore_case("NEWS", "news"));
        assert!(
            !starts_with_ignore_case("News", "newspaper"),
            "query longer than text"
        );
        assert!(!starts_with_ignore_case("abc", "abd"));
        assert!(
            starts_with_ignore_case("Émission", "émi"),
            "multibyte case-insensitive"
        );
    }

    // ---------- rendering ----------

    #[test]
    fn programmes_view_shows_channel_name_and_programme_title() {
        let db = test_db();
        let mut app = crate::app::App::new(db, String::new()).unwrap();
        app.now = 1_000_000;
        app.mode = Mode::Programmes;
        app.cur_channel = Some(crate::db::ChannelRow {
            channel_id: "c1".into(),
            display_name: "Test Channel".into(),
            icon: None,
            prog_count: 1,
        });
        app.programmes = vec![crate::db::ProgrammeRow {
            rowid: 1,
            channel_id: "c1".into(),
            start_ts: app.now - 100,
            stop_ts: app.now + 100,
            start_text: String::new(),
            stop_text: String::new(),
            title: "Live Show".into(),
            desc: "desc".into(),
        }];
        app.prog_view = vec![0];
        app.prog_selected = 0;

        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Test Channel"));
        assert!(text.contains("Live Show"));
    }

    #[test]
    fn detail_view_shows_title_channel_and_description_words() {
        let db = test_db();
        let mut app = crate::app::App::new(db, String::new()).unwrap();
        app.now = 1_000_000;
        app.mode = Mode::Detail;
        app.cur_channel = Some(crate::db::ChannelRow {
            channel_id: "c1".into(),
            display_name: "My Channel".into(),
            icon: None,
            prog_count: 1,
        });
        app.detail = Some(crate::db::ProgrammeRow {
            rowid: 1,
            channel_id: "c1".into(),
            start_ts: app.now - 100,
            stop_ts: app.now + 3500,
            start_text: String::new(),
            stop_text: String::new(),
            title: "Big Film".into(),
            desc: "alpha beta gamma delta".into(),
        });
        app.detail_scroll = 0;

        let backend = ratatui::backend::TestBackend::new(60, 25);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = buffer_text(&terminal);
        assert!(text.contains("Big Film"));
        assert!(text.contains("My Channel"));
        for word in ["alpha", "beta", "gamma", "delta"] {
            assert!(text.contains(word), "missing description word {word:?}");
        }
    }
}
