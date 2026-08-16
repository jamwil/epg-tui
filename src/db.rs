use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct Database {
    pub conn: Connection,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ChannelRow {
    pub channel_id: String,
    pub display_name: String,
    pub icon: Option<String>,
    pub prog_count: i64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ProgrammeRow {
    pub rowid: i64,
    pub channel_id: String,
    pub start_ts: i64,
    pub stop_ts: i64,
    pub start_text: String,
    pub stop_text: String,
    pub title: String,
    pub desc: String,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        Self::init_schema(&conn)?;
        Self::rebuild_counts(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT
            );
            CREATE TABLE IF NOT EXISTS channels (
                channel_id   TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                icon         TEXT
            );
            CREATE TABLE IF NOT EXISTS programmes (
                rowid      INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id TEXT NOT NULL,
                start_ts   INTEGER NOT NULL,
                stop_ts    INTEGER NOT NULL,
                start_text TEXT,
                stop_text  TEXT,
                title      TEXT,
                desc       TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_prog_channel ON programmes(channel_id, start_ts);
            CREATE INDEX IF NOT EXISTS idx_prog_start   ON programmes(start_ts);
            "#,
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(Into::into)
    }

    pub fn has_data(&self) -> Result<bool> {
        let exists =
            self.conn
                .query_row("SELECT EXISTS(SELECT 1 FROM channels LIMIT 1)", [], |r| {
                    r.get(0)
                })?;
        Ok(exists)
    }

    /// Wipe and prepare for a fresh import. Production imports do this inside
    /// the same transaction as parsing; this helper is retained for DB tests.
    #[cfg(test)]
    pub fn begin_import(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM programmes; DELETE FROM channels; DELETE FROM meta; DROP TABLE IF EXISTS _counts;",
        )?;
        Ok(())
    }

    /// Recompute the programmes-per-channel counts into a lookup table.
    #[cfg(test)]
    pub fn finalize_import(&self) -> Result<()> {
        Self::rebuild_counts(&self.conn)
    }

    fn rebuild_counts(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "DROP TABLE IF EXISTS _counts;
             CREATE TABLE IF NOT EXISTS _counts AS
                SELECT channel_id, COUNT(*) AS c FROM programmes GROUP BY channel_id;
             CREATE INDEX IF NOT EXISTS idx_counts ON _counts(channel_id);",
        )?;
        Ok(())
    }

    pub fn load_channels(&self) -> Result<Vec<ChannelRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.channel_id, c.display_name, c.icon, COALESCE(cc.c,0)
             FROM channels c LEFT JOIN _counts cc ON cc.channel_id = c.channel_id
             ORDER BY c.display_name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ChannelRow {
                channel_id: r.get(0)?,
                display_name: r.get(1)?,
                icon: r.get(2)?,
                prog_count: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Programmes for a channel, ordered by start time. Limited for safety.
    pub fn programmes_for_channel(
        &self,
        channel_id: &str,
        limit: i64,
    ) -> Result<Vec<ProgrammeRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, channel_id, start_ts, stop_ts, start_text, stop_text, title, desc
             FROM programmes WHERE channel_id=?1 ORDER BY start_ts ASC, rowid LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![channel_id, limit], |r| {
            Ok(ProgrammeRow {
                rowid: r.get(0)?,
                channel_id: r.get(1)?,
                start_ts: r.get(2)?,
                stop_ts: r.get(3)?,
                start_text: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                stop_text: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                title: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                desc: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Find channels whose currently-airing or next programme has a title or
    /// description containing `q` (case-insensitive). Returns
    /// `(channel_id, match_count)`; at most two programmes per channel qualify.
    pub fn search_current_and_next_programmes_by_text(
        &self,
        q: &str,
        now: i64,
    ) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT channel_id, COUNT(*) FROM programmes AS p
             WHERE (
                    p.rowid = (
                        SELECT current.rowid FROM programmes AS current
                        WHERE current.channel_id = p.channel_id
                          AND current.start_ts <= ?2 AND current.stop_ts > ?2
                        ORDER BY current.start_ts DESC, current.rowid DESC LIMIT 1
                    )
                    OR p.rowid = (
                        SELECT next.rowid FROM programmes AS next
                        WHERE next.channel_id = p.channel_id AND next.start_ts > ?2
                        ORDER BY next.start_ts ASC, next.rowid ASC LIMIT 1
                    )
                   )
               AND (
                    lower(p.title) LIKE lower('%' || ?1 || '%')
                    OR lower(p.desc) LIKE lower('%' || ?1 || '%')
               )
             GROUP BY channel_id",
        )?;
        let rows = stmt.query_map(params![q, now], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Currently-airing programme for a channel at time `now`.
    pub fn now_playing(&self, channel_id: &str, now: i64) -> Result<Option<ProgrammeRow>> {
        let p = self.conn.query_row(
            "SELECT rowid, channel_id, start_ts, stop_ts, start_text, stop_text, title, desc
             FROM programmes WHERE channel_id=?1 AND start_ts<=?2 AND stop_ts>?2
             ORDER BY start_ts DESC LIMIT 1",
            params![channel_id, now],
            |r| {
                Ok(ProgrammeRow {
                    rowid: r.get(0)?,
                    channel_id: r.get(1)?,
                    start_ts: r.get(2)?,
                    stop_ts: r.get(3)?,
                    start_text: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    stop_text: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    title: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    desc: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                })
            },
        );
        match p {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Next upcoming programme for a channel after time `now`.
    pub fn next_programme(&self, channel_id: &str, now: i64) -> Result<Option<ProgrammeRow>> {
        let p = self.conn.query_row(
            "SELECT rowid, channel_id, start_ts, stop_ts, start_text, stop_text, title, desc
             FROM programmes WHERE channel_id=?1 AND start_ts>?2
             ORDER BY start_ts ASC LIMIT 1",
            params![channel_id, now],
            |r| {
                Ok(ProgrammeRow {
                    rowid: r.get(0)?,
                    channel_id: r.get(1)?,
                    start_ts: r.get(2)?,
                    stop_ts: r.get(3)?,
                    start_text: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    stop_text: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                    title: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                    desc: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                })
            },
        );
        match p {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

pub fn cache_path() -> Result<std::path::PathBuf> {
    let base = if let Ok(s) = std::env::var("XDG_CACHE_HOME") {
        std::path::PathBuf::from(s)
    } else if let Ok(s) = std::env::var("HOME") {
        std::path::PathBuf::from(s).join(".cache")
    } else {
        return Err(anyhow::anyhow!("no HOME or XDG_CACHE_HOME set"));
    };
    Ok(base.join("epg-viewer").join("cache.db"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db() -> Database {
        // Unique path per call so tests can run in parallel without sharing a file.
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("epg-test-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Database::open(&path).unwrap()
    }

    #[test]
    fn search_current_and_next_programmes_finds_by_title_and_desc() {
        let db = tmp_db();
        db.conn
            .execute(
                "INSERT INTO channels (channel_id, display_name) VALUES ('c1','Movie Hub'), ('c2','News 24')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                 ('c1', -100, 0, 'Archived special', 'past-only keyword'),
                 ('c1', 0, 100, 'Inception', 'A heist in a dream within a dream'),
                 ('c1', 100, 200, 'News at Ten', ' nightly bulletin'),
                 ('c1', 200, 300, 'Later special', 'future-only keyword'),
                 ('c2', 0, 100, 'Morning Show', 'chat and WEATHER today')",
                [],
            )
            .unwrap();
        db.finalize_import().unwrap();

        let by_title = db
            .search_current_and_next_programmes_by_text("inception", 50)
            .unwrap();
        assert_eq!(by_title.len(), 1);
        assert_eq!(by_title[0].0, "c1");
        assert_eq!(by_title[0].1, 1);

        let by_desc = db
            .search_current_and_next_programmes_by_text("weather", 50)
            .unwrap();
        assert_eq!(by_desc.len(), 1);
        assert_eq!(by_desc[0].0, "c2");

        let news = db
            .search_current_and_next_programmes_by_text("news", 50)
            .unwrap();
        let m: std::collections::HashMap<String, i64> = news.into_iter().collect();
        assert_eq!(m.len(), 1, "news should match the title on c1 only");
        assert_eq!(m.get("c1"), Some(&1));

        assert!(db
            .search_current_and_next_programmes_by_text("past-only", 50)
            .unwrap()
            .is_empty());
        assert!(db
            .search_current_and_next_programmes_by_text("future-only", 50)
            .unwrap()
            .is_empty());
        assert!(db
            .search_current_and_next_programmes_by_text("nonexistent", 50)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn load_channels_orders_by_name_and_includes_zero_counts() {
        let db = tmp_db();
        db.conn
            .execute(
                "INSERT INTO channels (channel_id, display_name) VALUES ('c2','zebra'), ('c1','Apple'), ('c3','Mango')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                 ('c2', 0, 60, 'A',''), ('c2', 60, 120, 'B','')",
                [],
            )
            .unwrap();
        db.finalize_import().unwrap();

        let ch = db.load_channels().unwrap();
        let names: Vec<&str> = ch.iter().map(|c| c.display_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Apple", "Mango", "zebra"],
            "should order case-insensitively"
        );

        let counts: std::collections::HashMap<&str, i64> = ch
            .iter()
            .map(|c| (c.channel_id.as_str(), c.prog_count))
            .collect();
        assert_eq!(counts["c2"], 2);
        // Channels with no programmes are still listed with a zero count.
        assert_eq!(counts["c1"], 0);
        assert_eq!(counts["c3"], 0);
    }

    #[test]
    fn programmes_for_channel_orders_by_start_and_respects_limit() {
        let db = tmp_db();
        // Insert deliberately out of start-time order.
        db.conn
            .execute(
                "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                 ('c1', 300, 400, 'third',''),
                 ('c1', 100, 200, 'first',''),
                 ('c1', 200, 300, 'second',''),
                 ('c2', 100, 200, 'other','')",
                [],
            )
            .unwrap();

        let progs = db.programmes_for_channel("c1", 100).unwrap();
        let titles: Vec<&str> = progs.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, vec!["first", "second", "third"]);

        let limited = db.programmes_for_channel("c1", 2).unwrap();
        assert_eq!(limited.len(), 2);
        assert_eq!(limited[0].title, "first");

        // Channels are isolated from one another.
        assert_eq!(db.programmes_for_channel("c2", 100).unwrap().len(), 1);
    }

    #[test]
    fn now_playing_and_next_programme_respect_time_boundaries() {
        let db = tmp_db();
        db.conn
            .execute(
                "INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES
                 ('c1', 0, 100, 'past',''),
                 ('c1', 100, 200, 'airing',''),
                 ('c1', 200, 300, 'future','')",
                [],
            )
            .unwrap();

        // Middle of 'airing'.
        assert_eq!(db.now_playing("c1", 150).unwrap().unwrap().title, "airing");
        assert_eq!(
            db.next_programme("c1", 150).unwrap().unwrap().title,
            "future"
        );

        // Exact handoff: now == stop of 'airing' == start of 'future'.
        assert_eq!(db.now_playing("c1", 200).unwrap().unwrap().title, "future");

        // Before anything starts: nothing now, but a next exists.
        assert!(db.now_playing("c1", -1).unwrap().is_none());
        assert_eq!(db.next_programme("c1", -1).unwrap().unwrap().title, "past");

        // After everything ends: neither now nor next.
        assert!(db.now_playing("c1", 1000).unwrap().is_none());
        assert!(db.next_programme("c1", 1000).unwrap().is_none());
    }

    #[test]
    fn meta_set_get_and_overwrite() {
        let db = tmp_db();
        assert_eq!(db.get_meta("missing").unwrap(), None);
        db.set_meta("source_url", "http://a").unwrap();
        assert_eq!(
            db.get_meta("source_url").unwrap().as_deref(),
            Some("http://a")
        );
        db.set_meta("source_url", "http://b").unwrap();
        assert_eq!(
            db.get_meta("source_url").unwrap().as_deref(),
            Some("http://b")
        );
    }

    #[test]
    fn begin_import_wipes_data_and_finalize_rebuilds_counts() {
        let db = tmp_db();
        db.conn
            .execute(
                "INSERT INTO channels (channel_id, display_name) VALUES ('c1','A')",
                [],
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES ('c1',0,60,'x','')", [])
            .unwrap();
        db.set_meta("source_url", "http://a").unwrap();
        db.finalize_import().unwrap();
        assert_eq!(db.load_channels().unwrap().len(), 1);

        // Wipe everything (this drops the _counts table, so rebuild before querying).
        db.begin_import().unwrap();
        assert_eq!(db.get_meta("source_url").unwrap(), None);
        db.finalize_import().unwrap();
        assert!(db.load_channels().unwrap().is_empty());
        assert!(db.programmes_for_channel("c1", 10).unwrap().is_empty());

        // A fresh import only sees the new rows.
        db.conn
            .execute(
                "INSERT INTO channels (channel_id, display_name) VALUES ('c2','B')",
                [],
            )
            .unwrap();
        db.conn
            .execute("INSERT INTO programmes (channel_id, start_ts, stop_ts, title, desc) VALUES ('c2',0,60,'y','')", [])
            .unwrap();
        db.finalize_import().unwrap();
        let ch = db.load_channels().unwrap();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].channel_id, "c2");
        assert_eq!(ch[0].prog_count, 1);
    }

    #[test]
    fn cache_path_lives_under_epg_viewer_dir() {
        let p = cache_path().unwrap();
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("cache.db"));
        assert_eq!(
            p.parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str()),
            Some("epg-viewer")
        );
    }
}
