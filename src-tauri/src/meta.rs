//! Song catalogue search (iTunes, falling back to Deezer) and time-synced
//! lyrics from LRCLIB. All are free public APIs that need no key.

use crate::tools::http;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SongMeta {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds.
    pub duration: f64,
    pub artwork: Option<String>,
    pub year: Option<String>,
    pub explicit: bool,
}

pub async fn search_songs(query: &str) -> Result<Vec<SongMeta>, String> {
    match itunes(query).await {
        Ok(v) if !v.is_empty() => Ok(v),
        first => match deezer(query).await {
            Ok(v) => Ok(v),
            Err(e) => first.and(Err(e)),
        },
    }
}

async fn itunes(query: &str) -> Result<Vec<SongMeta>, String> {
    let json: Value = http()
        .get("https://itunes.apple.com/search")
        .query(&[("term", query), ("entity", "song"), ("media", "music"), ("limit", "40")])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(json["results"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            Some(SongMeta {
                id: format!("itunes:{}", r["trackId"].as_u64()?),
                title: r["trackName"].as_str()?.to_string(),
                artist: r["artistName"].as_str()?.to_string(),
                album: r["collectionName"].as_str().unwrap_or_default().to_string(),
                duration: r["trackTimeMillis"].as_f64().unwrap_or(0.0) / 1000.0,
                artwork: r["artworkUrl100"].as_str().map(|u| u.replace("100x100bb", "600x600bb")),
                year: r["releaseDate"].as_str().map(|d| d.chars().take(4).collect()),
                explicit: r["trackExplicitness"].as_str() == Some("explicit"),
            })
        })
        .collect())
}

async fn deezer(query: &str) -> Result<Vec<SongMeta>, String> {
    let json: Value = http()
        .get("https://api.deezer.com/search")
        .query(&[("q", query), ("limit", "40")])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(json["data"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            Some(SongMeta {
                id: format!("deezer:{}", r["id"].as_u64()?),
                title: r["title"].as_str()?.to_string(),
                artist: r["artist"]["name"].as_str()?.to_string(),
                album: r["album"]["title"].as_str().unwrap_or_default().to_string(),
                duration: r["duration"].as_f64().unwrap_or(0.0),
                artwork: r["album"]["cover_xl"].as_str().map(str::to_string),
                year: None,
                explicit: r["explicit_lyrics"].as_bool().unwrap_or(false),
            })
        })
        .collect())
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    /// LRC text with line (and sometimes word) timestamps.
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
}

/// Strip decorations that stop lyric databases from matching:
/// "Song (feat. X) - Remastered 2011" → "Song".
pub fn clean_title(title: &str) -> String {
    let mut t = title.to_string();
    for sep in [" - ", " – ", " — "] {
        if let Some(i) = t.find(sep) {
            let tail = t[i + sep.len()..].to_lowercase();
            if ["remaster", "live", "version", "edit", "mix", "mono", "stereo", "from", "feat", "single", "acoustic"]
                .iter()
                .any(|k| tail.contains(k))
            {
                t.truncate(i);
            }
        }
    }
    for (open, close) in [('(', ')'), ('[', ']')] {
        while let (Some(a), Some(b)) = (t.find(open), t.find(close)) {
            if b <= a {
                break;
            }
            let inner = t[a + 1..b].to_lowercase();
            if ["feat", "ft.", "with ", "remaster", "live", "version", "edit", "from", "mono", "stereo", "explicit", "clean"]
                .iter()
                .any(|k| inner.contains(k))
            {
                t.replace_range(a..=b, "");
            } else {
                break;
            }
        }
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn primary_artist(artist: &str) -> String {
    let lower = artist.to_lowercase();
    let mut cut = artist.len();
    for sep in [", ", " & ", " feat", " ft.", " x ", " and "] {
        if let Some(i) = lower.find(sep) {
            cut = cut.min(i);
        }
    }
    artist[..cut].trim().to_string()
}

fn to_lyrics(v: &Value) -> Lyrics {
    let s = |k: &str| v[k].as_str().filter(|s| !s.trim().is_empty()).map(str::to_string);
    Lyrics { synced: s("syncedLyrics"), plain: s("plainLyrics"), instrumental: v["instrumental"].as_bool().unwrap_or(false) }
}

pub async fn lyrics(artist: &str, title: &str, album: &str, duration: f64) -> Result<Lyrics, String> {
    let client = http();
    let title_c = clean_title(title);
    let artist_c = primary_artist(artist);

    // Exact signature match first; LRCLIB requires duration within ±2 s.
    if duration > 0.0 {
        for (a, t, al) in [(artist, title, album), (artist_c.as_str(), title_c.as_str(), album)] {
            let resp = client
                .get("https://lrclib.net/api/get")
                .query(&[("artist_name", a), ("track_name", t), ("album_name", al), ("duration", &(duration.round() as i64).to_string())])
                .send()
                .await
                .map_err(|e| e.to_string())?;
            if resp.status().is_success() {
                let v: Value = resp.json().await.map_err(|e| e.to_string())?;
                let l = to_lyrics(&v);
                if l.synced.is_some() || l.instrumental {
                    return Ok(l);
                }
            }
        }
    }

    // Fuzzy search; prefer synced lyrics with the closest duration.
    let mut best: Option<(f64, Lyrics)> = None;
    for query in [
        vec![("track_name", title_c.as_str()), ("artist_name", artist_c.as_str())],
        vec![("q", &format!("{artist_c} {title_c}"))],
    ] {
        let resp = client.get("https://lrclib.net/api/search").query(&query).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            continue;
        }
        let arr: Value = resp.json().await.map_err(|e| e.to_string())?;
        for item in arr.as_array().map(|a| a.as_slice()).unwrap_or_default() {
            let l = to_lyrics(item);
            if l.synced.is_none() && l.plain.is_none() {
                continue;
            }
            let d = item["duration"].as_f64().unwrap_or(0.0);
            let mut cost = if duration > 0.0 { (d - duration).abs() } else { 0.0 };
            if l.synced.is_none() {
                cost += 1000.0;
            }
            if best.as_ref().map(|b| cost < b.0).unwrap_or(true) {
                best = Some((cost, l));
            }
        }
        if best.as_ref().map(|b| b.0 < 5.0).unwrap_or(false) {
            break;
        }
    }
    Ok(best.map(|b| b.1).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_titles() {
        assert_eq!(clean_title("Song (feat. Someone) - Remastered 2011"), "Song");
        assert_eq!(clean_title("Song [Live]"), "Song");
        assert_eq!(clean_title("Love (Is All)"), "Love (Is All)");
        assert_eq!(primary_artist("A & B"), "A");
        assert_eq!(primary_artist("A, B feat. C"), "A");
    }
}
