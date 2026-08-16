use crate::db::{ChannelRow, Database, ProgrammeRow};
use anyhow::Result;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Channels,
    Programmes,
    Detail,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Name,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Search,
}

pub struct App {
    pub db: Database,
    pub mode: Mode,
    pub prev_mode: Mode,
    pub input: InputMode,

    pub channels: Vec<ChannelRow>,
    pub view: Vec<usize>,
    pub selected: usize,
    pub offset: usize,

    pub query: String,
    pub matches: Vec<usize>,
    pub match_pos: usize,
    pub filter_only: bool,
    /// channel_id -> matching current/next programme count for the current query.
    pub prog_match: HashMap<String, i64>,

    pub programmes: Vec<ProgrammeRow>,
    pub prog_view: Vec<usize>,
    pub prog_selected: usize,
    pub prog_offset: usize,
    pub prog_matches: Vec<usize>,
    pub prog_match_pos: usize,

    pub detail: Option<ProgrammeRow>,
    pub detail_scroll: usize,
    /// Channel whose schedule is currently open (for titles/labels).
    pub cur_channel: Option<ChannelRow>,

    pub now_cache: HashMap<String, (Option<ProgrammeRow>, Option<ProgrammeRow>)>,
    pub now: i64,

    pub sort: SortMode,
    pub source_url: String,
    pub status: String,
    pub quit: bool,
    pub pending_g: bool,

    pub viewport: usize,
}

impl App {
    pub fn new(db: Database, source_url: String) -> Result<Self> {
        let channels = db.load_channels()?;
        let view: Vec<usize> = (0..channels.len()).collect();
        Ok(Self {
            db,
            mode: Mode::Channels,
            prev_mode: Mode::Channels,
            input: InputMode::Normal,
            channels,
            view,
            selected: 0,
            offset: 0,
            query: String::new(),
            matches: Vec::new(),
            match_pos: 0,
            filter_only: false,
            prog_match: HashMap::new(),
            programmes: Vec::new(),
            prog_view: Vec::new(),
            prog_selected: 0,
            prog_offset: 0,
            prog_matches: Vec::new(),
            prog_match_pos: 0,
            detail: None,
            detail_scroll: 0,
            cur_channel: None,
            now_cache: HashMap::new(),
            now: chrono::Utc::now().timestamp(),
            sort: SortMode::Name,
            source_url,
            status: String::new(),
            quit: false,
            pending_g: false,
            viewport: 40,
        })
    }

    pub fn refresh_now(&mut self) {
        self.now = chrono::Utc::now().timestamp();
    }

    pub fn set_viewport(&mut self, h: usize) {
        self.viewport = h.max(1);
        self.clamp_scroll();
        self.clamp_prog_scroll();
    }

    // ---------- Channels ----------

    pub fn reload_channels(&mut self) -> Result<()> {
        self.channels = self.db.load_channels()?;
        self.apply_sort();
        self.recompute_view();
        Ok(())
    }

    pub fn apply_sort(&mut self) {
        match self.sort {
            SortMode::Name => self.channels.sort_by(|a, b| {
                a.display_name
                    .to_lowercase()
                    .cmp(&b.display_name.to_lowercase())
            }),
            SortMode::Count => self.channels.sort_by(|a, b| {
                b.prog_count.cmp(&a.prog_count).then(
                    a.display_name
                        .to_lowercase()
                        .cmp(&b.display_name.to_lowercase()),
                )
            }),
        }
    }

    pub fn recompute_view(&mut self) {
        if self.filter_only && !self.query.is_empty() {
            let q = self.query.to_lowercase();
            self.view = self
                .channels
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.display_name.to_lowercase().contains(&q)
                        || self.prog_match.contains_key(&c.channel_id)
                })
                .map(|(i, _)| i)
                .collect();
        } else {
            self.view = (0..self.channels.len()).collect();
        }
        self.compute_matches();
        if self.selected >= self.view.len() {
            self.selected = self.view.len().saturating_sub(1);
        }
        self.clamp_scroll();
    }

    fn compute_matches(&mut self) {
        self.matches.clear();
        if self.query.is_empty() {
            return;
        }
        let q = self.query.to_lowercase();
        for (vi, ci) in self.view.iter().enumerate() {
            let c = &self.channels[*ci];
            if c.display_name.to_lowercase().contains(&q)
                || self.prog_match.contains_key(&c.channel_id)
            {
                self.matches.push(vi);
            }
        }
    }

    pub fn clamp_scroll(&mut self) {
        let h = self.viewport;
        if self.view.is_empty() {
            self.offset = 0;
            self.selected = 0;
            return;
        }
        if self.selected >= self.view.len() {
            self.selected = self.view.len() - 1;
        }
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + h {
            self.offset = self.selected - h + 1;
        }
        let max_off = self.view.len().saturating_sub(h);
        if self.offset > max_off {
            self.offset = max_off;
        }
    }

    pub fn move_sel(&mut self, delta: isize) {
        if self.view.is_empty() {
            return;
        }
        let new = (self.selected as isize + delta).max(0) as usize;
        let new = new.min(self.view.len() - 1);
        self.selected = new;
        self.clamp_scroll();
    }

    pub fn page(&mut self, pages: isize, half: bool) {
        let h = self.viewport as isize;
        let step = if half { h / 2 } else { h };
        let step = step.max(1);
        self.move_sel(step * pages);
    }

    pub fn goto(&mut self, idx: usize) {
        self.selected = idx.min(self.view.len().saturating_sub(1));
        self.clamp_scroll();
    }

    // ---------- Search ----------

    /// Refresh matching current/next programmes for the current query. Only
    /// meaningful in channel mode (searches across all channels).
    pub fn refresh_prog_match(&mut self) -> Result<()> {
        if self.query.trim().is_empty() {
            self.prog_match.clear();
            return Ok(());
        }
        let pairs = self
            .db
            .search_current_and_next_programmes_by_text(&self.query, self.now)?;
        self.prog_match = pairs.into_iter().collect();
        Ok(())
    }

    pub fn start_search(&mut self) {
        self.input = InputMode::Search;
        // Edit the active query instead of discarding it, so repeated searches
        // can be refined without retyping the whole term.
        self.status.clear();
    }

    pub fn update_search_preview(&mut self) {
        if self.mode == Mode::Programmes {
            self.recompute_prog_view();
        } else {
            // Current/next programme matches are intentionally deferred until
            // Enter; querying SQLite on every keypress is expensive on large guides.
            self.recompute_view();
        }
    }

    pub fn cancel_search(&mut self) {
        self.input = InputMode::Normal;
        self.query.clear();
        self.matches.clear();
        self.prog_match.clear();
        self.filter_only = false;
        self.recompute_view();
        self.status.clear();
    }

    pub fn confirm_search(&mut self) {
        self.input = InputMode::Normal;
        // Channel-mode search spans only current/next programme titles and descriptions.
        if self.mode != Mode::Programmes {
            if let Err(e) = self.refresh_prog_match() {
                self.status = format!("search error: {e}");
                self.prog_match.clear();
            }
        }
        self.recompute_view();
        if !self.matches.is_empty() {
            self.match_pos = 0;
            self.goto(self.matches[0]);
            if self.prog_match.is_empty() {
                self.status = format!("{} matches", self.matches.len());
            } else {
                self.status = format!(
                    "{} matches • {} channels with matching programmes",
                    self.matches.len(),
                    self.prog_match.len()
                );
            }
        } else if !self.query.is_empty() {
            self.status = format!("No matches for \"{}\"", self.query);
        }
    }

    pub fn next_match(&mut self, forward: bool) {
        if self.matches.is_empty() {
            if !self.query.is_empty() {
                self.status = format!("No matches for \"{}\"", self.query);
            }
            return;
        }
        if forward {
            self.match_pos = (self.match_pos + 1) % self.matches.len();
        } else {
            self.match_pos = (self.match_pos + self.matches.len() - 1) % self.matches.len();
        }
        self.goto(self.matches[self.match_pos]);
        self.status = format!("match {}/{}", self.match_pos + 1, self.matches.len());
    }

    pub fn toggle_filter(&mut self) {
        if self.query.is_empty() {
            self.status = "No search query to filter by".into();
            return;
        }
        self.filter_only = !self.filter_only;
        self.recompute_view();
        if self.filter_only {
            self.status = format!("Filter ON: {} channels", self.view.len());
        } else {
            self.status = "Filter OFF".into();
        }
    }

    // ---------- Programmes ----------

    pub fn open_channel(&mut self) -> Result<()> {
        if self.view.is_empty() || self.selected >= self.view.len() {
            return Ok(());
        }
        let ci = self.view[self.selected];
        let ch = self.channels[ci].clone();
        self.programmes = self.db.programmes_for_channel(&ch.channel_id, 100_000)?;
        self.cur_channel = Some(ch);
        // Applies any active query/filter and recomputes match positions.
        self.recompute_prog_view();
        self.jump_to_now_programme();
        self.mode = Mode::Programmes;
        Ok(())
    }

    /// Select the programme currently airing, else the next upcoming.
    /// `prog_selected` is an index into `prog_view` (the filtered view), so
    /// the lookup must walk the view rather than the raw programme list.
    pub fn jump_to_now_programme(&mut self) {
        let now = self.now;
        let mut best: Option<usize> = None;
        for (vi, &pi) in self.prog_view.iter().enumerate() {
            let p = &self.programmes[pi];
            if p.start_ts <= now && p.stop_ts > now {
                best = Some(vi);
                break;
            }
        }
        if best.is_none() {
            for (vi, &pi) in self.prog_view.iter().enumerate() {
                if self.programmes[pi].start_ts > now {
                    best = Some(vi);
                    break;
                }
            }
        }
        if let Some(vi) = best {
            self.prog_selected = vi;
            self.clamp_prog_scroll();
        }
    }

    pub fn recompute_prog_view(&mut self) {
        if self.filter_only && !self.query.is_empty() {
            let q = self.query.to_lowercase();
            self.prog_view = self
                .programmes
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.title.to_lowercase().contains(&q) || p.desc.to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect();
        } else {
            self.prog_view = (0..self.programmes.len()).collect();
        }
        self.compute_prog_matches();
        if self.prog_selected >= self.prog_view.len() {
            self.prog_selected = self.prog_view.len().saturating_sub(1);
        }
        self.clamp_prog_scroll();
    }

    fn compute_prog_matches(&mut self) {
        self.prog_matches.clear();
        if self.query.is_empty() {
            return;
        }
        let q = self.query.to_lowercase();
        for (vi, pi) in self.prog_view.iter().enumerate() {
            let p = &self.programmes[*pi];
            if p.title.to_lowercase().contains(&q) || p.desc.to_lowercase().contains(&q) {
                self.prog_matches.push(vi);
            }
        }
    }

    fn clamp_prog_scroll(&mut self) {
        let h = self.viewport;
        if self.prog_view.is_empty() {
            self.prog_offset = 0;
            self.prog_selected = 0;
            return;
        }
        if self.prog_selected >= self.prog_view.len() {
            self.prog_selected = self.prog_view.len() - 1;
        }
        if self.prog_selected < self.prog_offset {
            self.prog_offset = self.prog_selected;
        } else if self.prog_selected >= self.prog_offset + h {
            self.prog_offset = self.prog_selected - h + 1;
        }
        let max_off = self.prog_view.len().saturating_sub(h);
        if self.prog_offset > max_off {
            self.prog_offset = max_off;
        }
    }

    pub fn move_prog(&mut self, delta: isize) {
        if self.prog_view.is_empty() {
            return;
        }
        let new = (self.prog_selected as isize + delta).max(0) as usize;
        self.prog_selected = new.min(self.prog_view.len() - 1);
        self.clamp_prog_scroll();
    }

    pub fn prog_page(&mut self, pages: isize, half: bool) {
        let h = self.viewport as isize;
        let step = (if half { h / 2 } else { h }).max(1);
        self.move_prog(step * pages);
    }

    pub fn prog_goto(&mut self, idx: usize) {
        self.prog_selected = idx.min(self.prog_view.len().saturating_sub(1));
        self.clamp_prog_scroll();
    }

    pub fn open_detail(&mut self) {
        if self.prog_view.is_empty() || self.prog_selected >= self.prog_view.len() {
            return;
        }
        let pi = self.prog_view[self.prog_selected];
        self.detail = Some(self.programmes[pi].clone());
        self.detail_scroll = 0;
        self.mode = Mode::Detail;
    }

    /// Return from the schedule view to the channel list. A query typed while
    /// browsing one channel's schedule carries over to a cross-channel
    /// programme search.
    pub fn close_programmes(&mut self) -> Result<()> {
        self.mode = Mode::Channels;
        self.programmes.clear();
        self.prog_view.clear();
        self.prog_matches.clear();
        self.cur_channel = None;
        if !self.query.is_empty() {
            self.refresh_prog_match()?;
            self.recompute_view();
        }
        Ok(())
    }

    /// Return from the detail view to the schedule.
    pub fn close_detail(&mut self) {
        self.mode = Mode::Programmes;
        self.detail = None;
    }

    // ---------- Now/Next cache ----------

    pub fn now_next(&mut self, channel_id: &str) -> (Option<ProgrammeRow>, Option<ProgrammeRow>) {
        if let Some(v) = self.now_cache.get(channel_id) {
            return v.clone();
        }
        let now = self.now;
        let cur = self.db.now_playing(channel_id, now).ok().flatten();
        let nxt = self.db.next_programme(channel_id, now).ok().flatten();
        self.now_cache
            .insert(channel_id.to_string(), (cur.clone(), nxt.clone()));
        (cur, nxt)
    }
}

pub fn fmt_time(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_opt(ts, 0) {
        chrono::LocalResult::Single(dt) => dt.format("%m-%d %H:%M").to_string(),
        _ => String::new(),
    }
}

pub fn fmt_range(start: i64, stop: i64) -> String {
    format!("{} – {}", fmt_time(start), fmt_time(stop))
}

pub fn duration_mins(start: i64, stop: i64) -> i64 {
    ((stop - start) / 60).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db() -> Database {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("epg-app-test-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Database::open(&path).unwrap()
    }

    fn empty_app() -> App {
        App::new(tmp_db(), String::new()).unwrap()
    }

    /// Seed `n` channels named "Channel 000".. and return an App over them.
    fn app_with_channels(n: usize) -> App {
        let db = tmp_db();
        for i in 0..n {
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES (?1, ?2)",
                    rusqlite::params![format!("c{i:03}"), format!("Channel {i:03}")],
                )
                .unwrap();
        }
        db.finalize_import().unwrap();
        let mut app = App::new(db, String::new()).unwrap();
        app.set_viewport(10);
        app
    }

    fn chan(id: &str, name: &str, count: i64) -> ChannelRow {
        ChannelRow {
            channel_id: id.into(),
            display_name: name.into(),
            icon: None,
            prog_count: count,
        }
    }

    fn prog(rowid: i64, start: i64, stop: i64, title: &str) -> ProgrammeRow {
        ProgrammeRow {
            rowid,
            channel_id: "c1".into(),
            start_ts: start,
            stop_ts: stop,
            start_text: String::new(),
            stop_text: String::new(),
            title: title.into(),
            desc: String::new(),
        }
    }

    fn selected_prog_title(app: &App) -> &str {
        &app.programmes[app.prog_view[app.prog_selected]].title
    }

    // ---------- navigation ----------

    #[test]
    fn move_sel_clamps_at_boundaries() {
        let mut app = app_with_channels(5);
        app.move_sel(-1);
        assert_eq!(app.selected, 0, "moving up from the top stays at 0");
        app.goto(4);
        app.move_sel(10);
        assert_eq!(
            app.selected, 4,
            "moving down past the end clamps to the last row"
        );
    }

    #[test]
    fn goto_clamps_to_last_index() {
        let mut app = app_with_channels(5);
        app.goto(999);
        assert_eq!(app.selected, 4);
    }

    #[test]
    fn move_sel_scrolls_window_to_follow_selection() {
        let mut app = app_with_channels(50);
        app.set_viewport(10);
        for _ in 0..15 {
            app.move_sel(1);
        }
        assert_eq!(app.selected, 15);
        // The scroll window keeps the selection visible.
        assert!(app.offset <= app.selected);
        assert!(app.selected < app.offset + app.viewport);
        app.goto(0);
        assert_eq!(app.offset, 0, "jumping to the top resets the scroll offset");
    }

    #[test]
    fn page_moves_by_full_and_half_viewport() {
        let mut app = app_with_channels(100);
        app.set_viewport(10);
        app.page(1, false);
        assert_eq!(app.selected, 10, "full page down");
        app.page(1, true);
        assert_eq!(app.selected, 15, "half page down");
        app.page(-1, false);
        assert_eq!(app.selected, 5, "full page up");
        app.page(-10, false);
        assert_eq!(app.selected, 0, "paging past the top clamps to 0");
    }

    // ---------- sorting ----------

    #[test]
    fn sort_by_name_is_case_insensitive() {
        let mut app = empty_app();
        app.channels = vec![
            chan("c1", "banana", 0),
            chan("c2", "Apple", 0),
            chan("c3", "cherry", 0),
        ];
        app.sort = SortMode::Name;
        app.apply_sort();
        let names: Vec<&str> = app
            .channels
            .iter()
            .map(|c| c.display_name.as_str())
            .collect();
        assert_eq!(names, vec!["Apple", "banana", "cherry"]);
    }

    #[test]
    fn sort_by_count_descending_then_name() {
        let mut app = empty_app();
        app.channels = vec![
            chan("c1", "low", 1),
            chan("c2", "high", 10),
            chan("c3", "mid-b", 5),
            chan("c4", "mid-a", 5),
        ];
        app.sort = SortMode::Count;
        app.apply_sort();
        let names: Vec<&str> = app
            .channels
            .iter()
            .map(|c| c.display_name.as_str())
            .collect();
        assert_eq!(names, vec!["high", "mid-a", "mid-b", "low"]);
    }

    // ---------- search & filter (channels) ----------

    #[test]
    fn confirm_search_matches_channel_names_case_insensitively() {
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES
                     ('c1','Sky News'), ('c2','Sky Sports'), ('c3','Comedy Central')",
                    [],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.start_search();
        app.query = "sky".into();
        app.confirm_search();
        assert_eq!(app.matches.len(), 2);
        let ci = app.view[app.selected];
        assert!(app.channels[ci].display_name.to_lowercase().contains("sky"));
        assert!(app.status.contains("2 matches"));
    }

    #[test]
    fn confirm_search_finds_channels_via_current_or_next_programme_text() {
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES ('c1','Movies'), ('c2','Docs')",
                    [],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                     ('c2', 0, 60, 'Planet Earth', 'nature documentary about oceans')",
                    [],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.now = 30;
        app.start_search();
        app.query = "oceans".into();
        app.confirm_search();
        assert!(
            app.prog_match.contains_key("c2"),
            "c2 has a matching current programme"
        );
        let names: Vec<String> = app
            .matches
            .iter()
            .map(|&vi| app.channels[app.view[vi]].display_name.clone())
            .collect();
        assert_eq!(names, vec!["Docs"]);
    }

    #[test]
    fn next_match_wraps_around_in_both_directions() {
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES
                     ('c1','News 1'), ('c2','News 2'), ('c3','News 3')",
                    [],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.start_search();
        app.query = "news".into();
        app.confirm_search();
        assert_eq!(app.matches.len(), 3);
        assert_eq!(app.match_pos, 0);
        app.next_match(true);
        assert_eq!(app.match_pos, 1);
        app.next_match(true);
        assert_eq!(app.match_pos, 2);
        app.next_match(true);
        assert_eq!(app.match_pos, 0, "forward wraps to the first match");
        app.next_match(false);
        assert_eq!(app.match_pos, 2, "backward wraps to the last match");
    }

    #[test]
    fn toggle_filter_narrows_and_restores_view() {
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES
                     ('c1','Sky News'), ('c2','Sky Sports'), ('c3','Comedy Central')",
                    [],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.start_search();
        app.query = "sky".into();
        app.confirm_search();
        assert_eq!(app.view.len(), 3, "filter off shows all channels");
        app.toggle_filter();
        assert!(app.filter_only);
        assert_eq!(app.view.len(), 2, "filter on shows only matches");
        app.toggle_filter();
        assert!(!app.filter_only);
        assert_eq!(app.view.len(), 3, "toggling off restores the full view");
    }

    #[test]
    fn toggle_filter_without_query_sets_status() {
        let mut app = empty_app();
        app.toggle_filter();
        assert!(!app.filter_only);
        assert_eq!(app.status, "No search query to filter by");
    }

    #[test]
    fn cancel_search_clears_query_and_restores_view() {
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES ('c1','Sky News'), ('c2','Other')",
                    [],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.start_search();
        app.query = "sky".into();
        app.confirm_search();
        app.filter_only = true;
        app.recompute_view();
        assert_eq!(app.view.len(), 1);
        app.cancel_search();
        assert_eq!(app.input, InputMode::Normal);
        assert!(app.query.is_empty());
        assert!(!app.filter_only);
        assert_eq!(app.view.len(), 2, "cancelling restores the unfiltered view");
    }

    // ---------- programmes ----------

    #[test]
    fn jump_to_now_prefers_currently_airing() {
        let mut app = empty_app();
        app.now = 1000;
        app.programmes = vec![
            prog(1, 100, 500, "past"),
            prog(2, 500, 1500, "airing"),
            prog(3, 1500, 2500, "next"),
        ];
        app.prog_view = (0..3).collect();
        app.jump_to_now_programme();
        assert_eq!(selected_prog_title(&app), "airing");
    }

    #[test]
    fn jump_to_now_falls_back_to_next_upcoming() {
        let mut app = empty_app();
        app.now = 1000;
        app.programmes = vec![
            prog(1, 100, 500, "past"),
            prog(2, 1500, 2500, "next"),
            prog(3, 2600, 3000, "later"),
        ];
        app.prog_view = (0..3).collect();
        app.jump_to_now_programme();
        assert_eq!(selected_prog_title(&app), "next");
    }

    #[test]
    fn recompute_prog_view_filters_by_title_or_desc() {
        let mut app = empty_app();
        let mut p1 = prog(1, 0, 60, "Morning News");
        p1.desc = "daily headlines".into();
        let p2 = prog(2, 60, 120, "Cartoon");
        let mut p3 = prog(3, 120, 180, "Documentary");
        p3.desc = "all about the news industry".into();
        app.programmes = vec![p1, p2, p3];
        app.query = "news".into();
        app.filter_only = true;
        app.recompute_prog_view();
        let titles: Vec<&str> = app
            .prog_view
            .iter()
            .map(|&pi| app.programmes[pi].title.as_str())
            .collect();
        assert_eq!(titles, vec!["Morning News", "Documentary"]);
    }

    #[test]
    fn open_channel_loads_programmes_switches_mode_and_jumps_to_now() {
        let now = 1_000_000i64;
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES ('c1','Alpha')",
                    [],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                     ('c1', ?1, ?2, 'Earlier', ''),
                     ('c1', ?3, ?4, 'Airing Now', ''),
                     ('c1', ?5, ?6, 'Later', '')",
                    rusqlite::params![
                        now - 7200,
                        now - 3600,
                        now - 1800,
                        now + 1800,
                        now + 1800,
                        now + 3600
                    ],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.now = now;
        app.open_channel().unwrap();
        assert_eq!(app.mode, Mode::Programmes);
        assert_eq!(app.programmes.len(), 3);
        assert_eq!(selected_prog_title(&app), "Airing Now");
        assert_eq!(app.cur_channel.as_ref().unwrap().channel_id, "c1");
    }

    #[test]
    fn open_detail_then_close_returns_to_programmes() {
        let mut app = empty_app();
        app.programmes = vec![prog(1, 0, 60, "Show A"), prog(2, 60, 120, "Show B")];
        app.prog_view = (0..2).collect();
        app.prog_selected = 1;
        app.mode = Mode::Programmes;
        app.detail_scroll = 5;
        app.open_detail();
        assert_eq!(app.mode, Mode::Detail);
        assert_eq!(app.detail.as_ref().unwrap().title, "Show B");
        assert_eq!(app.detail_scroll, 0, "opening detail resets the scroll");
        app.close_detail();
        assert_eq!(app.mode, Mode::Programmes);
        assert!(app.detail.is_none());
    }

    #[test]
    fn close_programmes_returns_to_channels_and_refreshes_search() {
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES ('c1','Alpha'), ('c2','Beta')",
                    [],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                     ('c1', 0, 60, 'Oceans Deep', 'underwater')",
                    [],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        // Simulate browsing a channel's schedule with an active query.
        app.now = 30;
        app.query = "oceans".into();
        app.mode = Mode::Programmes;
        app.programmes = vec![prog(1, 0, 60, "Oceans Deep")];
        app.prog_view = vec![0];
        app.close_programmes().unwrap();
        assert_eq!(app.mode, Mode::Channels);
        assert!(app.programmes.is_empty());
        assert!(app.prog_view.is_empty());
        // The query carries over to a cross-channel current/next programme search.
        assert!(app.prog_match.contains_key("c1"));
    }

    // ---------- now/next cache ----------

    #[test]
    fn now_next_returns_airing_and_next_and_uses_cache() {
        let now = 1_000_000i64;
        let mut app = {
            let db = tmp_db();
            db.conn
                .execute(
                    "INSERT INTO channels (channel_id, display_name) VALUES ('c1','Alpha')",
                    [],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                     ('c1', ?1, ?2, 'Now Show', ''),
                     ('c1', ?3, ?4, 'Next Show', '')",
                    rusqlite::params![now - 100, now + 100, now + 100, now + 200],
                )
                .unwrap();
            db.finalize_import().unwrap();
            App::new(db, String::new()).unwrap()
        };
        app.now = now;
        let (cur, nxt) = app.now_next("c1");
        assert_eq!(cur.unwrap().title, "Now Show");
        assert_eq!(nxt.unwrap().title, "Next Show");
        assert!(app.now_cache.contains_key("c1"), "result is cached");
        // A second call is served from the cache with identical data.
        let (cur2, nxt2) = app.now_next("c1");
        assert_eq!(cur2.unwrap().title, "Now Show");
        assert_eq!(nxt2.unwrap().title, "Next Show");
    }

    // ---------- formatting helpers ----------

    #[test]
    fn duration_mins_computes_and_clamps_negative() {
        assert_eq!(duration_mins(0, 3600), 60);
        assert_eq!(duration_mins(1000, 1000 + 90 * 60), 90);
        assert_eq!(
            duration_mins(2000, 1000),
            0,
            "negative durations clamp to 0"
        );
    }

    #[test]
    fn fmt_time_has_mmdd_hhmm_shape() {
        let s = fmt_time(0);
        assert_eq!(s.len(), 11);
        assert!(s.contains('-'));
        assert!(s.contains(':'));
    }

    #[test]
    fn fmt_range_joins_start_and_stop() {
        let s = fmt_range(0, 3600);
        assert!(s.contains('\u{2013}'), "range uses an en dash separator");
    }
}
