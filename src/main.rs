use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Write};
use std::path::PathBuf;

mod app;
mod db;
mod fetch;
mod ui;
mod xmltv;

use app::{App, InputMode, Mode, SortMode};

#[derive(Parser, Debug)]
#[command(
    name = "epg",
    version,
    about = "A TUI EPG (XMLTV) viewer with SQLite caching and vim-style bindings"
)]
struct Cli {
    /// XMLTV source URL. Defaults to the cached source, if available.
    #[arg(long, short = 'u', env = "EPG_URL")]
    url: Option<String>,

    /// Read an XMLTV file instead of fetching. Does not populate the URL source meta.
    #[arg(long, short = 'f')]
    file: Option<PathBuf>,

    /// Force a re-download + re-import on startup before launching.
    #[arg(long, short = 'r')]
    refresh: bool,

    /// Path to the SQLite cache database.
    #[arg(long, env = "EPG_DB")]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Download and import the source, no TUI.
    Refresh {
        #[arg(long, short = 'u', env = "EPG_URL")]
        url: Option<String>,
        #[arg(long, short = 'f')]
        file: Option<PathBuf>,
    },
    /// Print the configured cache path.
    CachePath,
    /// Show metadata about the cached data.
    Info,
    /// Run a headless render self-test (no TTY needed).
    SelfTest,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db.clone().unwrap_or_else(|| db::cache_path().unwrap());
    let database = db::Database::open(&db_path)?;
    let url = cli
        .url
        .clone()
        .or_else(default_url)
        .or(database.get_meta("source_url")?);

    match &cli.command {
        Some(Cmd::Refresh { url: sub_url, file }) => {
            let url = sub_url.clone().or_else(default_url).or(url.clone());
            if let Some(f) = file {
                let (c, p) = xmltv::import_file(&database, f, "", |_, _| {})?;
                println!("Imported {c} channels, {p} programmes from {:?}", f);
            } else if let Some(u) = url {
                run_refresh(&database, &u)?;
            } else {
                return Err(anyhow!("no URL or file provided for refresh"));
            }
            Ok(())
        }
        Some(Cmd::CachePath) => {
            println!("{}", db_path.display());
            Ok(())
        }
        Some(Cmd::Info) => {
            if let Some(u) = database.get_meta("source_url")?.filter(|u| !u.is_empty()) {
                println!("source: {}", u);
            } else {
                println!("source: local file or not configured");
            }
            if let Some(t) = database.get_meta("imported_at")? {
                println!("imported: {}", t);
            }
            let ch = database.load_channels()?;
            let total: i64 = ch.iter().map(|c| c.prog_count).sum();
            println!("channels: {}", ch.len());
            println!("programmes: {}", total);
            Ok(())
        }
        Some(Cmd::SelfTest) => {
            // Need a populated DB. If empty, import from file/URL.
            if !database.has_data()? {
                if let Some(f) = &cli.file {
                    xmltv::import_file(&database, f, "", |_, _| {})?;
                } else if cli.refresh {
                    let u = url
                        .as_deref()
                        .ok_or_else(|| anyhow!("no source URL configured"))?;
                    let file = fetch::download_to_temp(u, |_, _| {})?;
                    xmltv::import_file(&database, file.path(), u, |_, _| {})?;
                }
            }
            let app_source = database.get_meta("source_url")?.unwrap_or_default();
            let mut app = App::new(database, app_source)?;
            app.reload_channels()?;
            self_test(&mut app)?;
            Ok(())
        }
        None => {
            // Interactive TUI path.
            if let Some(f) = &cli.file {
                let (c, p) = xmltv::import_file(&database, f, "", |_, _| {})?;
                eprintln!("Imported {c} channels, {p} programmes");
            } else if cli.refresh || cli.url.is_some() || !database.has_data()? {
                let u = url.as_deref().ok_or_else(|| {
                    anyhow!(
                        "no data in cache and no source provided. \
                         Supply one via -u <url>, EPG_URL=<url>, or -f <file>."
                    )
                })?;
                print!("Fetching {} …", u);
                let _ = io::stdout().flush();
                let file = fetch::download_to_temp(u, |_, _| {})?;
                println!(" done ({} bytes)", file.as_file().metadata()?.len());
                let (c, p) = xmltv::import_file(&database, file.path(), u, |_, _| {})?;
                eprintln!("Imported {c} channels, {p} programmes");
            }

            let app_source = database.get_meta("source_url")?.unwrap_or_default();
            let mut app = App::new(database, app_source)?;
            // First import path: build default view.
            app.reload_channels()?;
            run_tui(&mut app)?;
            Ok(())
        }
    }
}

fn default_url() -> Option<String> {
    // No credentials are bundled. Supply a source via EPG_URL / -u <url> / -f <file>.
    std::env::var("EPG_URL").ok()
}

fn run_refresh(db: &db::Database, url: &str) -> Result<()> {
    print!("Fetching {} …", url);
    let _ = io::stdout().flush();
    let file = fetch::download_to_temp(url, |_, _| {})?;
    println!(" done ({} bytes)", file.as_file().metadata()?.len());
    let (c, p) = xmltv::import_file(db, file.path(), url, |ch, pg| {
        eprint!("\r  channels: {ch}  programmes: {pg}   ");
        let _ = io::stderr().flush();
    })?;
    eprintln!();
    println!("Imported {c} channels, {p} programmes.");
    Ok(())
}

fn run_tui(app: &mut App) -> Result<()> {
    struct TerminalCleanup;
    impl Drop for TerminalCleanup {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }

    enable_raw_mode()?;
    let cleanup = TerminalCleanup;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run_loop(&mut terminal, app);
    let _ = terminal.show_cursor();
    drop(terminal);
    drop(cleanup);
    result
}

fn run_loop<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut last_tick = std::time::Instant::now();
    loop {
        if app.quit {
            break;
        }
        terminal.draw(|f| ui::draw(f, app))?;

        // Poll events with a 2s tick to refresh the clock / now-airing markers.
        let timeout = std::time::Duration::from_millis(2000).saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    handle_key(app, key);
                }
            }
        }
        if last_tick.elapsed() >= std::time::Duration::from_secs(2) {
            app.refresh_now();
            // Clear now/next cache so airing markers update.
            app.now_cache.clear();
            last_tick = std::time::Instant::now();
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Search input mode takes all keys.
    if app.input == InputMode::Search {
        app.pending_g = false;
        match key.code {
            KeyCode::Esc => app.cancel_search(),
            KeyCode::Enter => {
                app.confirm_search();
                if app.mode == Mode::Programmes {
                    app.recompute_prog_view();
                }
            }
            KeyCode::Backspace => {
                if app.query.is_empty() {
                    app.cancel_search();
                } else {
                    app.query.pop();
                    app.update_search_preview();
                }
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                app.query.push(c);
                app.update_search_preview();
            }
            _ => {}
        }
        return;
    }

    // Global keys.
    match key.code {
        KeyCode::Char('?') => {
            app.pending_g = false;
            if app.mode == Mode::Help {
                app.mode = app.prev_mode;
            } else {
                app.prev_mode = app.mode;
                app.mode = Mode::Help;
            }
            return;
        }
        KeyCode::Char('q') => {
            app.quit = true;
            return;
        }
        KeyCode::Esc => {
            // Esc is the universal "back / dismiss" key: it closes the help,
            // backs out of the detail view, clears an active search/filter,
            // and otherwise returns to the previous screen.
            app.pending_g = false;
            match app.mode {
                Mode::Help => app.mode = app.prev_mode,
                Mode::Detail => app.close_detail(),
                Mode::Programmes => {
                    if !app.query.is_empty() || app.filter_only {
                        app.cancel_search();
                        app.recompute_prog_view();
                    } else if let Err(e) = app.close_programmes() {
                        app.status = format!("error: {e}");
                    }
                }
                Mode::Channels => {
                    if !app.query.is_empty() || app.filter_only {
                        app.cancel_search();
                    }
                }
            }
            return;
        }
        _ => {}
    }

    match app.mode {
        Mode::Channels => handle_channels_key(app, key),
        Mode::Programmes => handle_prog_key(app, key),
        Mode::Detail => handle_detail_key(app, key),
        Mode::Help => {}
    }
}

fn handle_channels_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.move_sel(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_sel(-1),
        KeyCode::Char('G') => app.goto(app.view.len().saturating_sub(1)),
        KeyCode::Char('g') => {
            if app.pending_g {
                app.goto(0);
                app.pending_g = false;
            } else {
                app.pending_g = true;
            }
        }

        KeyCode::Char('J') => app.move_sel(5),
        KeyCode::Char('K') => app.move_sel(-5),
        KeyCode::Char('H') => app.goto(app.offset),
        KeyCode::Char('M') => app.goto(app.offset + app.viewport / 2),
        KeyCode::Char('L') => app.goto((app.offset + app.viewport).saturating_sub(1)),
        KeyCode::PageDown if ctrl => app.page(1, true),
        KeyCode::PageUp if ctrl => app.page(-1, true),
        KeyCode::PageDown => app.page(1, false),
        KeyCode::PageUp => app.page(-1, false),
        KeyCode::Char('d') if ctrl => app.page(1, true),
        KeyCode::Char('u') if ctrl => app.page(-1, true),
        KeyCode::Char('f') if ctrl => app.page(1, false),
        KeyCode::Char('b') if ctrl => app.page(-1, false),
        KeyCode::Char('f') => {
            app.toggle_filter();
            app.recompute_view();
        }
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('n') => app.next_match(true),
        KeyCode::Char('N') => app.next_match(false),
        KeyCode::Char('s') => {
            app.sort = match app.sort {
                SortMode::Name => SortMode::Count,
                SortMode::Count => SortMode::Name,
            };
            app.status = format!(
                "sort: {}",
                match app.sort {
                    SortMode::Name => "name",
                    SortMode::Count => "count",
                }
            );
            app.apply_sort();
            app.recompute_view();
        }
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
            if let Err(e) = app.open_channel() {
                app.status = format!("error: {e}");
            }
        }
        KeyCode::Char('r') => {
            app.status = "refreshing… (see stderr)".into();
            if let Err(e) = do_refresh(app) {
                app.status = format!("refresh failed: {e}");
            } else {
                app.status = "refresh complete".into();
            }
        }
        _ => {}
    }
    if key.code != KeyCode::Char('g') {
        app.pending_g = false;
    }
}

fn handle_prog_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.move_prog(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_prog(-1),
        KeyCode::Char('G') => app.prog_goto(app.prog_view.len().saturating_sub(1)),
        KeyCode::Char('g') => {
            if app.pending_g {
                app.prog_goto(0);
                app.pending_g = false;
            } else {
                app.pending_g = true;
            }
        }
        KeyCode::Char('t') => app.jump_to_now_programme(),
        KeyCode::Char('J') => app.move_prog(5),
        KeyCode::Char('K') => app.move_prog(-5),
        KeyCode::Char('H') => app.prog_goto(app.prog_offset),
        KeyCode::Char('M') => app.prog_goto(app.prog_offset + app.viewport / 2),
        KeyCode::Char('L') => app.prog_goto((app.prog_offset + app.viewport).saturating_sub(1)),
        KeyCode::PageDown if ctrl => app.prog_page(1, true),
        KeyCode::PageUp if ctrl => app.prog_page(-1, true),
        KeyCode::PageDown => app.prog_page(1, false),
        KeyCode::PageUp => app.prog_page(-1, false),
        KeyCode::Char('d') if ctrl => app.prog_page(1, true),
        KeyCode::Char('u') if ctrl => app.prog_page(-1, true),
        KeyCode::Char('f') if ctrl => app.prog_page(1, false),
        KeyCode::Char('b') if ctrl => app.prog_page(-1, false),
        KeyCode::Char('f') => {
            app.toggle_filter();
            app.recompute_prog_view();
        }
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('n') => {
            if app.prog_matches.is_empty() {
                if !app.query.is_empty() {
                    app.status = format!("No matches for \"{}\"", app.query);
                }
            } else {
                app.prog_match_pos = (app.prog_match_pos + 1) % app.prog_matches.len();
                app.prog_goto(app.prog_matches[app.prog_match_pos]);
                app.status = format!(
                    "match {}/{}",
                    app.prog_match_pos + 1,
                    app.prog_matches.len()
                );
            }
        }
        KeyCode::Char('N') => {
            if app.prog_matches.is_empty() {
                if !app.query.is_empty() {
                    app.status = format!("No matches for \"{}\"", app.query);
                }
            } else {
                app.prog_match_pos =
                    (app.prog_match_pos + app.prog_matches.len() - 1) % app.prog_matches.len();
                app.prog_goto(app.prog_matches[app.prog_match_pos]);
                app.status = format!(
                    "match {}/{}",
                    app.prog_match_pos + 1,
                    app.prog_matches.len()
                );
            }
        }
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.open_detail(),
        KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left if !ctrl => {
            if let Err(e) = app.close_programmes() {
                app.status = format!("error: {e}");
            }
        }
        KeyCode::Char('r') => {
            if let Err(e) = do_refresh(app) {
                app.status = format!("refresh failed: {e}");
            } else {
                app.status = "refresh complete".into();
            }
        }
        _ => {}
    }
    if key.code != KeyCode::Char('g') {
        app.pending_g = false;
    }
}

fn handle_detail_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.detail_scroll = app.detail_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.detail_scroll = app.detail_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') if ctrl => app.detail_scroll += 10,
        KeyCode::Char('u') if ctrl => {
            app.detail_scroll = app.detail_scroll.saturating_sub(10);
        }
        KeyCode::Char('f') if ctrl => app.detail_scroll += 20,
        KeyCode::Char('b') if ctrl => {
            app.detail_scroll = app.detail_scroll.saturating_sub(20);
        }
        KeyCode::PageDown => app.detail_scroll += 20,
        KeyCode::PageUp => app.detail_scroll = app.detail_scroll.saturating_sub(20),
        KeyCode::Char('G') => {
            // Scroll to bottom; max_scroll is applied during draw.
            app.detail_scroll = usize::MAX / 2;
        }
        KeyCode::Char('g') => app.detail_scroll = 0,
        KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left => app.close_detail(),
        _ => {}
    }
}

fn self_test(app: &mut App) -> Result<()> {
    use ratatui::backend::TestBackend;
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend)?;
    // Render the channels view.
    terminal.draw(|f| ui::draw(f, app))?;
    app.status.clear();
    // Open the first channel with programmes and render the schedule.
    for i in 0..app.view.len().min(50) {
        app.selected = i;
        app.open_channel()?;
        if !app.programmes.is_empty() {
            break;
        }
        app.mode = Mode::Channels;
    }
    terminal.draw(|f| ui::draw(f, app))?;
    // Open detail of the first programme.
    if !app.prog_view.is_empty() {
        app.open_detail();
        terminal.draw(|f| ui::draw(f, app))?;
    }
    // Exercise search.
    app.start_search();
    app.query.push('e');
    app.recompute_view();
    terminal.draw(|f| ui::draw(f, app))?;
    app.confirm_search();
    terminal.draw(|f| ui::draw(f, app))?;
    // Help screen.
    app.mode = Mode::Help;
    terminal.draw(|f| ui::draw(f, app))?;
    println!(
        "self-test OK: channels={} (view={}), programmes={} (view={})",
        app.channels.len(),
        app.view.len(),
        app.programmes.len(),
        app.prog_view.len()
    );
    Ok(())
}

fn do_refresh(app: &mut App) -> Result<()> {
    let url = app.source_url.clone();
    if url.is_empty() {
        return Err(anyhow!("no source URL configured"));
    }
    let file = fetch::download_to_temp(&url, |_, _| {})?;
    let (c, p) = xmltv::import_file(&app.db, file.path(), &url, |_, _| {})?;
    app.reload_channels()?;
    app.now_cache.clear();
    app.status = format!("refreshed: {c} ch / {p} prog");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db() -> db::Database {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("epg-main-test-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        db::Database::open(&path).unwrap()
    }

    fn app_with_channels(names: &[&str]) -> App {
        let d = tmp_db();
        for (i, n) in names.iter().enumerate() {
            d.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES (?1, ?2)",
                    rusqlite::params![format!("c{i}"), n],
                )
                .unwrap();
        }
        d.finalize_import().unwrap();
        App::new(d, String::new()).unwrap()
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn code(k: KeyCode) -> KeyEvent {
        KeyEvent::new(k, KeyModifiers::NONE)
    }

    #[test]
    fn g_then_g_goes_to_top_and_capital_g_to_bottom() {
        let mut app = app_with_channels(&["a", "b", "c", "d", "e"]);
        handle_key(&mut app, key('G'));
        assert_eq!(app.selected, 4, "G jumps to the bottom");
        handle_key(&mut app, key('g'));
        assert!(app.pending_g, "first g arms the pending-g state");
        handle_key(&mut app, key('g'));
        assert_eq!(app.selected, 0, "second g jumps to the top");
        assert!(!app.pending_g);
    }

    #[test]
    fn pending_g_is_cleared_by_other_keys() {
        let mut app = app_with_channels(&["a", "b", "c", "d", "e"]);
        app.goto(2);
        handle_key(&mut app, key('g'));
        assert!(app.pending_g);
        handle_key(&mut app, key('j'));
        assert_eq!(
            app.selected, 3,
            "j just moves down; the pending g is discarded"
        );
        assert!(!app.pending_g);
    }

    #[test]
    fn slash_typing_and_enter_confirms_search() {
        let mut app = app_with_channels(&["Sky News", "Sky Sports", "Comedy"]);
        handle_key(&mut app, key('/'));
        assert_eq!(app.input, InputMode::Search);
        for c in "sky".chars() {
            handle_key(&mut app, key(c));
        }
        assert_eq!(app.query, "sky");
        handle_key(&mut app, code(KeyCode::Enter));
        assert_eq!(app.input, InputMode::Normal);
        assert_eq!(app.matches.len(), 2);
    }

    #[test]
    fn esc_cancels_search_input_mode() {
        let mut app = app_with_channels(&["a", "b"]);
        handle_key(&mut app, key('/'));
        handle_key(&mut app, key('x'));
        handle_key(&mut app, code(KeyCode::Esc));
        assert_eq!(app.input, InputMode::Normal);
        assert!(app.query.is_empty());
    }

    #[test]
    fn esc_with_active_search_clears_it_before_backing_out() {
        let mut app = app_with_channels(&["Sky News", "Other"]);
        handle_key(&mut app, key('/'));
        for c in "sky".chars() {
            handle_key(&mut app, key(c));
        }
        handle_key(&mut app, code(KeyCode::Enter));
        assert!(!app.query.is_empty());
        handle_key(&mut app, code(KeyCode::Esc));
        assert!(app.query.is_empty(), "Esc dismisses the active search");
        assert_eq!(app.mode, Mode::Channels, "and stays on the channels view");
    }

    #[test]
    fn f_toggles_filter_mode() {
        let mut app = app_with_channels(&["Sky News", "Sky Sports", "Comedy"]);
        handle_key(&mut app, key('/'));
        for c in "sky".chars() {
            handle_key(&mut app, key(c));
        }
        handle_key(&mut app, code(KeyCode::Enter));
        assert_eq!(app.view.len(), 3);
        handle_key(&mut app, key('f'));
        assert!(app.filter_only);
        assert_eq!(app.view.len(), 2);
    }

    #[test]
    fn s_cycles_sort_mode() {
        let mut app = app_with_channels(&["a", "b"]);
        assert_eq!(app.sort, SortMode::Name);
        handle_key(&mut app, key('s'));
        assert_eq!(app.sort, SortMode::Count);
        handle_key(&mut app, key('s'));
        assert_eq!(app.sort, SortMode::Name);
    }

    #[test]
    fn q_sets_quit_flag() {
        let mut app = app_with_channels(&["a"]);
        handle_key(&mut app, key('q'));
        assert!(app.quit);
    }
}
