use anyhow::{anyhow, Context, Result};
use std::io::{Read, Write};
use tempfile::NamedTempFile;

/// Download `url` to a temp file, returning the path. Calls `progress` with
/// bytes downloaded so far and total length (if known).
pub fn download_to_temp(
    url: &str,
    mut progress: impl FnMut(usize, Option<usize>),
) -> Result<NamedTempFile> {
    let agent = ureq::AgentBuilder::new()
        .timeout_read(std::time::Duration::from_secs(120))
        .timeout_connect(std::time::Duration::from_secs(30))
        .build();

    let resp = agent
        .get(url)
        .call()
        .map_err(|e| anyhow!("HTTP request failed: {e}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<usize>().ok());

    let mut out = NamedTempFile::new().context("create temp file")?;

    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut read = 0usize;
    loop {
        let n = reader.read(&mut buf).context("reading download stream")?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        read += n;
        progress(read, total);
    }
    out.flush()?;
    Ok(out)
}
