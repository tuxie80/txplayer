use std::path::Path;

#[derive(Debug, Clone)]
pub struct LyricLine {
    /// Timestamp in seconds (None for plain unsynchronized lyrics).
    pub time_secs: Option<f64>,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct TrackLyrics {
    pub lines: Vec<LyricLine>,
    pub synced: bool,
    pub source_label: String,
}

impl TrackLyrics {
    /// Returns the index of the active lyric line for the current playback position.
    pub fn active_index(&self, position_secs: f64) -> Option<usize> {
        if !self.synced || self.lines.is_empty() {
            return None;
        }
        let mut active = None;
        for (i, line) in self.lines.iter().enumerate() {
            if let Some(t) = line.time_secs {
                if t <= position_secs + 0.08 {
                    active = Some(i);
                } else {
                    break;
                }
            }
        }
        active
    }
}

/// Loads lyrics for a track: checks sidecar `.lrc` / `.txt` next to the audio file first,
/// then falls back to embedded lyrics tags if present.
pub fn load_lyrics_for_track(audio_path: &Path, embedded: Option<&str>) -> Option<TrackLyrics> {
    // 1) Sidecar `.lrc` (exact stem match)
    let lrc_path = audio_path.with_extension("lrc");
    if lrc_path.is_file()
        && let Ok(bytes) = std::fs::read(&lrc_path)
    {
        let text = String::from_utf8_lossy(&bytes);
        let parsed = parse_lrc_or_plain(&text, "Sidecar .lrc");
        if !parsed.lines.is_empty() {
            return Some(parsed);
        }
    }

    // 2) Embedded audio metadata lyrics
    if let Some(raw) = embedded
        && !raw.trim().is_empty()
    {
        let parsed = parse_lrc_or_plain(raw, "Embedded Tag");
        if !parsed.lines.is_empty() {
            return Some(parsed);
        }
    }

    None
}

/// Parses either timestamped LRC (`[mm:ss.xx]`, supports multiple timestamps per line and `[offset:ms]`)
/// or plain unsynchronized lyrics.
pub fn parse_lrc_or_plain(input: &str, source_label: &str) -> TrackLyrics {
    let text = input.trim_start_matches('\u{feff}');
    let mut offset_secs = 0.0_f64;
    let mut timed: Vec<LyricLine> = Vec::new();
    let mut plain: Vec<LyricLine> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        // Check [offset:...] metadata tag
        if let Some(rest) = line.strip_prefix("[offset:")
            && let Some(end) = rest.find(']')
            && let Ok(ms) = rest[..end].trim().parse::<f64>()
        {
            offset_secs = ms / 1000.0;
            continue;
        }

        // Parse leading [mm:ss.xx] timestamps
        let mut cursor = line;
        let mut times: Vec<f64> = Vec::new();
        while cursor.starts_with('[') {
            let Some(close) = cursor.find(']') else {
                break;
            };
            let inside = &cursor[1..close];
            if let Some(t) = parse_lrc_timestamp(inside) {
                times.push(t);
                cursor = &cursor[close + 1..];
            } else {
                // Skip non-time LRC metadata tags like [ar:...], [ti:...], [al:...]
                if inside.contains(':')
                    && inside
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
                {
                    cursor = &cursor[close + 1..];
                    continue;
                }
                break;
            }
        }

        let lyric_text = cursor.trim().to_string();
        if !times.is_empty() {
            for t in times {
                timed.push(LyricLine {
                    time_secs: Some((t + offset_secs).max(0.0)),
                    text: if lyric_text.is_empty() {
                        "♪".to_string()
                    } else {
                        lyric_text.clone()
                    },
                });
            }
        } else if !lyric_text.is_empty() {
            plain.push(LyricLine {
                time_secs: None,
                text: lyric_text,
            });
        }
    }

    if !timed.is_empty() {
        timed.sort_by(|a, b| {
            a.time_secs
                .unwrap_or(0.0)
                .total_cmp(&b.time_secs.unwrap_or(0.0))
        });
        TrackLyrics {
            lines: timed,
            synced: true,
            source_label: source_label.to_string(),
        }
    } else {
        TrackLyrics {
            lines: plain,
            synced: false,
            source_label: source_label.to_string(),
        }
    }
}

fn parse_lrc_timestamp(s: &str) -> Option<f64> {
    let (m_str, rest) = s.trim().split_once(':')?;
    let mins: f64 = m_str.trim().parse().ok()?;
    let secs: f64 = rest.trim().parse().ok()?;
    if mins < 0.0 || secs < 0.0 {
        return None;
    }
    Some(mins * 60.0 + secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synced_lrc_and_tracks_active_line() {
        let lrc = "[ar:Artist]\n[00:05.50]First line\n[00:12.00][00:30.25]Chorus line\n";
        let parsed = parse_lrc_or_plain(lrc, "test");
        assert!(parsed.synced);
        assert_eq!(parsed.lines.len(), 3);
        assert_eq!(parsed.active_index(4.0), None);
        assert_eq!(parsed.active_index(6.0), Some(0));
        assert_eq!(parsed.active_index(12.5), Some(1));
        assert_eq!(parsed.active_index(31.0), Some(2));
        assert_eq!(parsed.lines[2].text, "Chorus line");
    }
}
