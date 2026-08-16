use crate::db::Database;
use anyhow::{ensure, Result};
use chrono::DateTime;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::path::Path;

/// Parse an XMLTV date string like `20260816022600 +0300` into a unix timestamp.
fn parse_xmltv_date(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // chrono can parse `YYYYMMDDHHMMSS %z`
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y%m%d%H%M%S %z") {
        return Some(dt.timestamp());
    }
    // Some feeds drop the space: `20260816022600+0300`
    if let Ok(dt) = DateTime::parse_from_str(s, "%Y%m%d%H%M%S%z") {
        return Some(dt.timestamp());
    }
    // Some feeds have no tz at all
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d%H%M%S") {
        return Some(ndt.and_utc().timestamp());
    }
    None
}

fn attr<'a>(attrs: quick_xml::events::attributes::Attributes<'a>, key: &str) -> Option<String> {
    for a in attrs.flatten() {
        if a.key.as_ref() == key.as_bytes() {
            return Some(String::from_utf8_lossy(a.value.as_ref()).into_owned());
        }
    }
    None
}

/// Import an XMLTV file into the database, replacing any previous data.
/// `progress` is called with (channels_seen, programmes_seen) periodically.
pub fn import_file(
    db: &Database,
    path: &Path,
    source_url: &str,
    mut progress: impl FnMut(usize, usize),
) -> Result<(usize, usize)> {
    let mut reader = Reader::from_file(path)?;
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut state = State::None;

    let mut chan_id = String::new();
    let mut chan_name = String::new();
    let mut chan_icon: Option<String> = None;
    let mut chan_has_name = false;

    let mut prog_channel = String::new();
    let mut prog_start_ts: i64 = 0;
    let mut prog_stop_ts: i64 = 0;
    let mut prog_start_text = String::new();
    let mut prog_stop_text = String::new();
    let mut prog_title = String::new();
    let mut prog_desc = String::new();
    let mut text_buf = String::new();

    // Keep the previous guide intact if parsing or insertion fails. The old
    // implementation cleared tables before parsing, leaving an empty cache on
    // malformed/truncated downloads.
    let tx = db.conn.unchecked_transaction()?;
    tx.execute_batch(
        "DELETE FROM programmes; DELETE FROM channels; DELETE FROM meta; DROP TABLE IF EXISTS _counts;",
    )?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES('source_url',?1)",
        rusqlite::params![source_url],
    )?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES('imported_at',?1)",
        rusqlite::params![chrono::Utc::now().to_rfc3339()],
    )?;

    let mut n_chan = 0usize;
    let mut n_prog = 0usize;
    let mut completed_tv = false;

    loop {
        let event = reader.read_event_into(&mut buf)?;
        match event {
            Event::Start(e) => match e.name().as_ref() {
                b"channel" => {
                    chan_id = attr(e.attributes(), "id").unwrap_or_default();
                    chan_name.clear();
                    chan_icon = None;
                    chan_has_name = false;
                    state = State::Channel;
                }
                b"programme" => {
                    prog_channel = attr(e.attributes(), "channel").unwrap_or_default();
                    prog_start_text = attr(e.attributes(), "start").unwrap_or_default();
                    prog_stop_text = attr(e.attributes(), "stop").unwrap_or_default();
                    prog_start_ts = attr(e.attributes(), "start_timestamp")
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or_else(|| parse_xmltv_date(&prog_start_text).unwrap_or(0));
                    prog_stop_ts = attr(e.attributes(), "stop_timestamp")
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or_else(|| parse_xmltv_date(&prog_stop_text).unwrap_or(0));
                    prog_title.clear();
                    prog_desc.clear();
                    state = State::Programme;
                }
                b"display-name" if state == State::Channel => {
                    text_buf.clear();
                    state = State::DisplayName;
                }
                b"icon" if state == State::Channel && chan_icon.is_none() => {
                    chan_icon = attr(e.attributes(), "src");
                }
                b"title" if state == State::Programme => {
                    text_buf.clear();
                    state = State::Title;
                }
                b"desc" if state == State::Programme => {
                    text_buf.clear();
                    state = State::Desc;
                }
                _ => {}
            },
            Event::Empty(e)
                if e.name().as_ref() == b"icon"
                    && state == State::Channel
                    && chan_icon.is_none() =>
            {
                chan_icon = attr(e.attributes(), "src");
            }
            Event::Text(t) => match state {
                State::DisplayName | State::Title | State::Desc => {
                    text_buf.push_str(&t.unescape()?);
                }
                _ => {}
            },
            Event::End(e) => match e.name().as_ref() {
                b"tv" => {
                    completed_tv = true;
                    state = State::None;
                }
                b"channel" => {
                    if !chan_id.is_empty() {
                        tx.execute(
                            "INSERT INTO channels(channel_id,display_name,icon) VALUES(?1,?2,?3)
                             ON CONFLICT(channel_id) DO UPDATE SET display_name=excluded.display_name WHERE channels.display_name=''",
                            rusqlite::params![
                                chan_id,
                                if chan_name.is_empty() {
                                    &chan_id
                                } else {
                                    &chan_name
                                },
                                chan_icon,
                            ],
                        )?;
                        n_chan += 1;
                    }
                    state = State::None;
                    if n_chan.is_multiple_of(500) {
                        progress(n_chan, n_prog);
                    }
                }
                b"programme" => {
                    if !prog_channel.is_empty() {
                        tx.execute(
                            "INSERT INTO programmes(channel_id,start_ts,stop_ts,start_text,stop_text,title,desc)
                             VALUES(?1,?2,?3,?4,?5,?6,?7)",
                            rusqlite::params![
                                prog_channel,
                                prog_start_ts,
                                prog_stop_ts,
                                prog_start_text,
                                prog_stop_text,
                                prog_title,
                                prog_desc,
                            ],
                        )?;
                        n_prog += 1;
                    }
                    state = State::None;
                    if n_prog.is_multiple_of(5000) {
                        progress(n_chan, n_prog);
                    }
                }
                b"display-name" if state == State::DisplayName => {
                    if !chan_has_name {
                        chan_name = std::mem::take(&mut text_buf);
                        chan_has_name = true;
                    }
                    state = State::Channel;
                }
                b"title" if state == State::Title => {
                    prog_title = std::mem::take(&mut text_buf);
                    state = State::Programme;
                }
                b"desc" if state == State::Desc => {
                    prog_desc = std::mem::take(&mut text_buf);
                    state = State::Programme;
                }
                _ => {}
            },
            Event::Eof => {
                ensure!(
                    completed_tv,
                    "incomplete XMLTV document: missing closing </tv>"
                );
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    tx.execute_batch(
        "CREATE TABLE _counts AS
             SELECT channel_id, COUNT(*) AS c FROM programmes GROUP BY channel_id;
         CREATE INDEX idx_counts ON _counts(channel_id);",
    )?;
    tx.commit()?;
    progress(n_chan, n_prog);
    Ok((n_chan, n_prog))
}

#[derive(PartialEq, Clone, Copy)]
enum State {
    None,
    Channel,
    DisplayName,
    Programme,
    Title,
    Desc,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_db() -> Database {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("epg-xmltv-db-{}-{n}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Database::open(&path).unwrap()
    }

    fn tmp_xml(content: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("epg-xmltv-{}-{n}.xml", std::process::id()));
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_xmltv_date_with_space_and_offset() {
        let ts = parse_xmltv_date("20260816022600 +0300").unwrap();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-08-16T02:26:00+03:00")
            .unwrap()
            .timestamp();
        assert_eq!(ts, expected);
    }

    #[test]
    fn parse_xmltv_date_without_space_matches_with_space() {
        let no_space = parse_xmltv_date("20260816022600+0300").unwrap();
        let with_space = parse_xmltv_date("20260816022600 +0300").unwrap();
        assert_eq!(no_space, with_space);
    }

    #[test]
    fn parse_xmltv_date_without_timezone_is_treated_as_utc() {
        let ts = parse_xmltv_date("20260816022600").unwrap();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-08-16T02:26:00+00:00")
            .unwrap()
            .timestamp();
        assert_eq!(ts, expected);
    }

    #[test]
    fn parse_xmltv_date_rejects_empty_and_garbage() {
        assert_eq!(parse_xmltv_date(""), None);
        assert_eq!(parse_xmltv_date("   "), None);
        assert_eq!(parse_xmltv_date("not a date"), None);
        // Wrong shape (dashes) is not an XMLTV date.
        assert_eq!(parse_xmltv_date("2026-08-16 02:26:00"), None);
    }

    #[test]
    fn import_file_populates_channels_programmes_and_meta() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <channel id="c1">
    <display-name>Alpha News</display-name>
    <icon src="http://example.com/a.png"/>
  </channel>
  <channel id="c2">
    <display-name>Beta Movies</display-name>
  </channel>
  <programme channel="c1" start="20260816100000 +0000" stop="20260816110000 +0000">
    <title>Morning Bulletin</title>
    <desc>Headlines</desc>
  </programme>
  <programme channel="c1" start="20260816110000 +0000" stop="20260816120000 +0000">
    <title>Midday Show</title>
    <desc>Chat</desc>
  </programme>
  <programme channel="c2" start="20260816100000 +0000" stop="20260816120000 +0000" start_timestamp="1755000000" stop_timestamp="1755007200">
    <title>Film</title>
    <desc>A movie</desc>
  </programme>
</tv>"#;
        let path = tmp_xml(xml);
        let db = tmp_db();
        let (c, p) = import_file(&db, &path, "http://example.com/epg.xml", |_, _| {}).unwrap();
        assert_eq!((c, p), (2, 3));

        let channels = db.load_channels().unwrap();
        assert_eq!(channels.len(), 2);
        // Sorted by name (NOCASE).
        assert_eq!(channels[0].display_name, "Alpha News");
        assert_eq!(channels[0].prog_count, 2);
        assert_eq!(
            channels[0].icon.as_deref(),
            Some("http://example.com/a.png")
        );
        assert_eq!(channels[1].display_name, "Beta Movies");
        assert_eq!(channels[1].prog_count, 1);

        // Meta is recorded.
        assert_eq!(
            db.get_meta("source_url").unwrap().as_deref(),
            Some("http://example.com/epg.xml")
        );
        assert!(db.get_meta("imported_at").unwrap().is_some());

        // Programmes for c1 are ordered by start time.
        let progs = db.programmes_for_channel("c1", 100).unwrap();
        assert_eq!(progs.len(), 2);
        assert_eq!(progs[0].title, "Morning Bulletin");
        assert_eq!(progs[1].title, "Midday Show");
        // Date strings are parsed into timestamps.
        let expected = chrono::DateTime::parse_from_rfc3339("2026-08-16T10:00:00+00:00")
            .unwrap()
            .timestamp();
        assert_eq!(progs[0].start_ts, expected);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_import_preserves_previous_guide() {
        let db = tmp_db();
        let good =
            tmp_xml(r#"<tv><channel id="old"><display-name>Old</display-name></channel></tv>"#);
        import_file(&db, &good, "old-source", |_, _| {}).unwrap();

        let truncated =
            tmp_xml(r#"<tv><channel id="new"><display-name>New</display-name></channel>"#);
        assert!(import_file(&db, &truncated, "new-source", |_, _| {}).is_err());

        let channels = db.load_channels().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].display_name, "Old");
        assert_eq!(
            db.get_meta("source_url").unwrap().as_deref(),
            Some("old-source")
        );
    }

    #[test]
    fn import_file_prefers_timestamp_attributes_over_date_strings() {
        let xml = r#"<?xml version="1.0"?>
<tv>
  <channel id="c2"><display-name>Beta</display-name></channel>
  <programme channel="c2" start="20260816100000 +0000" stop="20260816120000 +0000" start_timestamp="1755000000" stop_timestamp="1755007200">
    <title>Film</title>
  </programme>
</tv>"#;
        let path = tmp_xml(xml);
        let db = tmp_db();
        import_file(&db, &path, "src", |_, _| {}).unwrap();

        let film = db.programmes_for_channel("c2", 100).unwrap();
        // The explicit timestamp attributes win over the date strings.
        assert_eq!(film[0].start_ts, 1755000000);
        assert_eq!(film[0].stop_ts, 1755007200);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_file_falls_back_to_channel_id_when_display_name_missing() {
        let xml = r#"<?xml version="1.0"?>
<tv>
  <channel id="fallback-id"></channel>
  <programme channel="fallback-id" start="20260816100000 +0000" stop="20260816110000 +0000">
    <title>Show</title>
  </programme>
</tv>"#;
        let path = tmp_xml(xml);
        let db = tmp_db();
        let (c, p) = import_file(&db, &path, "src", |_, _| {}).unwrap();
        assert_eq!((c, p), (1, 1));

        let ch = db.load_channels().unwrap();
        assert_eq!(ch[0].display_name, "fallback-id");

        let _ = std::fs::remove_file(&path);
    }
}
