// Port of legacy `src/providers/song-info.ts` (Pear desktop, now Windharp).
// Builds a SongInfo from the renderer-provided GetPlayerResponse payload.
// Rust->webview notifications use the Tauri event "pear-ipc-event" with
// payload {"channel": "...", "args": [...]}; the channel string is the
// ORIGINAL (non-normalized) `peard:*` name.

use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, State};

use crate::config::{apply_config_change, SharedStore};

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SongInfo {
    pub title: String,
    pub alternative_title: String,
    pub artist: String,
    pub artist_url: String,
    pub views: i64,
    pub upload_date: String,
    pub image_src: String,
    /// Always serialized as null in this port (image payload dropped).
    pub image: Option<String>,
    pub is_paused: Option<bool>,
    pub song_duration: f64,
    pub elapsed_seconds: f64,
    pub url: String,
    pub album: Option<String>,
    pub video_id: String,
    pub playlist_id: String,
    pub media_type: MediaType,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MediaType {
    #[serde(rename = "AUDIO")]
    Audio,
    #[serde(rename = "ORIGINAL_MUSIC_VIDEO")]
    OriginalMusicVideo,
    #[serde(rename = "USER_GENERATED_CONTENT")]
    UserGenerated,
    #[serde(rename = "PODCAST_EPISODE")]
    PodcastEpisode,
    #[serde(rename = "OTHER_VIDEO")]
    OtherVideo,
}

/// Cached last-known song info; managed by lib.rs as `SharedSong`.
#[derive(Debug, Default)]
pub struct SongState {
    pub current: Option<SongInfo>,
}

pub type SharedSong = Mutex<SongState>;

// ---------------------------------------------------------------------------
// JSON scalar helpers (JS `String(x)` / `Number(x)` semantics)
// ---------------------------------------------------------------------------

/// JS `String(x)`: strings pass through, numbers/booleans stringify,
/// everything else yields "".
fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// JS `Number(x)`: number -> as-is, numeric string -> parsed, else None.
fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn value_to_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

/// Tiny query-param parser: returns the value of `key` from the query string
/// of `url` (percent-decoded, like legacy URL.searchParams.get), or "" when
/// absent.
fn query_param(url: &str, key: &str) -> String {
    let query = url.split('?').nth(1).unwrap_or("");
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next().unwrap_or("") == key {
            return percent_decode(it.next().unwrap_or(""));
        }
    }
    String::new()
}

/// Percent-decode a query component (legacy URL.searchParams.get decodes
/// %XX escapes; playlist ids like `RDMM%3A...` must be decoded or every
/// comparison against raw ids fails).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            // Decode from the BYTE slice — slicing `s` itself could split a
            // multi-byte UTF-8 char and panic.
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        // '+' means space in query-string encoding.
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Name/title cleanup
// ---------------------------------------------------------------------------

/// Is the inner content of a trailing bracket group a known decoration?
///
/// NOTE: this whole cleanup is a deliberate simplification of the legacy
/// regex chain in src/providers/song-info.ts (which ran an ordered list of
/// `/i` regexes). Instead, `strip_decoration_suffixes` repeatedly inspects
/// the LAST `(...)`/`[...]` group at the end of the string (bounded passes)
/// and strips it when the inner matches this vocabulary, plus the plain
/// "- topic" / "vevo" suffixes. Verified examples:
///   "Artist - Topic"        -> "Artist"
///   "ArtistVEVO"            -> "Artist"
///   "Song (Official Video)" -> "Song"
///   "Song [Live]"           -> "Song"
///   "Song (HD Audio)"       -> "Song"
fn is_decoration_inner(inner: &str) -> bool {
    let inner = inner.trim();
    if inner.is_empty() {
        return false;
    }
    let lower = inner.to_ascii_lowercase();
    if lower.starts_with("official")
        || lower.starts_with("lyric")
        || lower.starts_with("visualizer")
        || lower.starts_with("audio")
        || lower.starts_with("performance video")
        || lower.starts_with("clip official")
        || lower.starts_with("video version")
        || lower == "live"
        || lower == "4k"
        || lower == "4k upgrade"
    {
        return true;
    }
    // "(HD)" / "(HQ)" alone, or "(HD|HQ) audio".
    if lower == "hd" || lower == "hq" {
        return true;
    }
    if let Some(prefix) = lower.strip_suffix(" audio") {
        if prefix == "hd" || prefix == "hq" {
            return true;
        }
    }
    false
}

/// If `s` ends with `suffix` (ASCII case-insensitive, using the pre-computed
/// lowercase `lower`), return `s` with the suffix and any trailing
/// whitespace before it removed.
fn strip_ci_suffix(s: &str, lower: &str, suffix: &str) -> Option<String> {
    if let Some(stripped) = lower.strip_suffix(suffix) {
        // `lower` is the ASCII-lowercase of `s`, so byte indices align.
        Some(s[..stripped.len()].trim_end().to_string())
    } else {
        None
    }
}

/// One cleanup pass over `s`; returns the next value (unchanged when stable).
fn cleanup_step(s: &str) -> String {
    let trimmed = s.trim_end();
    let lower = trimmed.to_ascii_lowercase();

    // Plain suffixes: legacy `\s*- topic$` and `\s*vevo$` (case-insensitive).
    if let Some(rest) = strip_ci_suffix(trimmed, &lower, "- topic") {
        return rest;
    }
    if let Some(rest) = strip_ci_suffix(trimmed, &lower, "vevo") {
        return rest;
    }

    // Trailing bracket group whose inner is a known decoration.
    if let Some(idx) = trimmed.rfind(|c| c == '(' || c == '[') {
        let well_formed = match trimmed.as_bytes()[idx] {
            b'(' => trimmed.ends_with(')'),
            b'[' => trimmed.ends_with(']'),
            _ => false,
        };
        if well_formed && trimmed.len() >= idx + 2 {
            let inner = &trimmed[idx + 1..trimmed.len() - 1];
            if is_decoration_inner(inner) {
                return trimmed[..idx].trim_end().to_string();
            }
        }
    }

    trimmed.to_string()
}

/// Repeatedly strip decoration suffixes until stable (bounded, max 8 passes).
fn strip_decoration_suffixes(input: &str) -> String {
    let mut current = input.to_string();
    for _ in 0..8 {
        let next = cleanup_step(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current.trim_end().to_string()
}

/// Port of the legacy `cleanups` regex chain applied to names/titles.
fn cleanup_name(input: String) -> String {
    strip_decoration_suffixes(&input)
}

// ---------------------------------------------------------------------------
// Parse GetPlayerResponse -> SongInfo
// ---------------------------------------------------------------------------

fn default_song_info() -> SongInfo {
    SongInfo {
        title: String::new(),
        alternative_title: String::new(),
        artist: String::new(),
        artist_url: String::new(),
        views: 0,
        upload_date: String::new(),
        image_src: String::new(),
        image: None,
        is_paused: None,
        song_duration: 0.0,
        elapsed_seconds: 0.0,
        url: String::new(),
        album: None,
        video_id: String::new(),
        playlist_id: String::new(),
        media_type: MediaType::OtherVideo,
        tags: Vec::new(),
    }
}

fn parse_media_type(music_video_type: &str) -> MediaType {
    // Legacy TS switch over the raw musicVideoType value; substring checks
    // cover observed variants (e.g. "MUSIC_VIDEO_TYPE_UGC_MUSIC_VIDEO").
    if music_video_type == "MUSIC_VIDEO_TYPE_ATV" {
        MediaType::Audio
    } else if music_video_type == "MUSIC_VIDEO_TYPE_OMV" {
        MediaType::OriginalMusicVideo
    } else if music_video_type.contains("UGC") {
        MediaType::UserGenerated
    } else if music_video_type.contains("PODCAST") {
        MediaType::PodcastEpisode
    } else {
        MediaType::OtherVideo
    }
}

pub fn parse_player_response(
    player_response: &Value,
    use_podcast_participant_as_artist: bool,
) -> SongInfo {
    let mut info = default_song_info();

    let micro = player_response
        .get("microformat")
        .and_then(|m| m.get("microformatDataRenderer"))
        .filter(|m| m.is_object());

    if let Some(micro) = micro {
        info.upload_date = micro
            .get("uploadDate")
            .map(value_to_string)
            .unwrap_or_default();

        let url_canonical = micro
            .get("urlCanonical")
            .map(value_to_string)
            .unwrap_or_default();
        // `url` field: first '&'-segment of the canonical URL (legacy strips
        // tracking params appended with '&').
        info.url = url_canonical.split('&').next().unwrap_or("").to_string();
        // playlistId: raw 'list' query param of the FULL canonical URL
        // (before the &-split), no percent-decoding (legacy parity).
        info.playlist_id = query_param(&url_canonical, "list");

        // artistUrl: channel page from pageOwnerDetails.externalChannelId.
        let channel_id = micro
            .get("pageOwnerDetails")
            .and_then(|p| p.get("externalChannelId"))
            .map(value_to_string)
            .unwrap_or_default();
        if !channel_id.is_empty() {
            info.artist_url = format!("https://music.youtube.com/channel/{channel_id}");
        }

        // alternativeTitle: first linkAlternates entry with a truthy title.
        if let Some(alts) = micro.get("linkAlternates").and_then(|v| v.as_array()) {
            for alt in alts {
                if let Some(title) = alt.get("title").and_then(|t| t.as_str()) {
                    if !title.is_empty() {
                        info.alternative_title = title.trim().to_string();
                        break;
                    }
                }
            }
        }

        // tags: only meaningful when the value is an array; keep the string
        // entries, ignore anything else.
        if let Some(Value::Array(items)) = micro.get("tags") {
            info.tags = items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect();
        }
    }

    let video_details = player_response
        .get("videoDetails")
        .filter(|v| v.is_object());

    if let Some(vd) = video_details {
        info.title = cleanup_name(vd.get("title").map(value_to_string).unwrap_or_default());
        info.artist = cleanup_name(vd.get("author").map(value_to_string).unwrap_or_default());

        // Number() semantics: viewCount usually arrives as a digit string.
        info.views = vd.get("viewCount").and_then(value_to_i64).unwrap_or(0);
        info.song_duration = vd
            .get("lengthSeconds")
            .and_then(value_to_f64)
            .unwrap_or(0.0);

        // F-9 — legacy song-info.ts:114-115 maps videoDetails.elapsedSeconds
        // and isPaused directly; payload values win over the page pre-inject.
        info.elapsed_seconds = vd
            .get("elapsedSeconds")
            .and_then(value_to_f64)
            .unwrap_or(0.0);
        if let Some(paused) = vd.get("isPaused").and_then(Value::as_bool) {
            info.is_paused = Some(paused);
        }

        info.video_id = vd.get("videoId").map(value_to_string).unwrap_or_default();

        // album: pre-injected into the GetPlayerResponse payload by the
        // renderer page pre-inject (legacy page.ts), so it lives in
        // videoDetails here.
        info.album = vd.get("album").and_then(|a| a.as_str()).map(str::to_string);

        // mediaType from musicVideoType (default OTHER_VIDEO).
        let music_video_type = vd
            .get("musicVideoType")
            .map(value_to_string)
            .unwrap_or_default();
        info.media_type = parse_media_type(&music_video_type);

        // F-4 — port of song-info.ts:119-152 artist-swap HACKs:
        //  1. PODCAST_EPISODE: when the option is DISABLED (legacy reads it
        //     inverted: `!config.get('options.usePodcastParticipantAsArtist')`),
        //     the podcast "participant" (videoDetails.author) is NOT the
        //     artist — swap to the channel owner's name.
        //  2. OTHER_VIDEO (default branch): same swap when the option is
        //     disabled AND the responseContext 'ipcc' param != '0' (absent
        //     param defaults to '1' → swap applies).
        // Owner name = microformat.microformatDataRenderer.pageOwnerDetails.name;
        // when microformat is absent we keep the already-parsed author (legacy
        // would throw on the un-chained access; graceful fallback in port).
        let owner_name = micro
            .and_then(|m| m.get("pageOwnerDetails"))
            .and_then(|p| p.get("name"))
            .map(value_to_string)
            .map(|n| cleanup_name(n))
            .filter(|n| !n.is_empty());

        match info.media_type {
            MediaType::PodcastEpisode => {
                if !use_podcast_participant_as_artist {
                    if let Some(owner) = &owner_name {
                        info.artist = owner.clone();
                    }
                }
            }
            MediaType::OtherVideo => {
                let ipcc = player_response
                    .get("responseContext")
                    .and_then(|rc| rc.get("serviceTrackingParams"))
                    .and_then(|s| s.as_array())
                    .and_then(|a| a.first())
                    .and_then(|first| first.get("params"))
                    .and_then(|p| p.as_array())
                    .and_then(|params| {
                        params
                            .iter()
                            .find(|it| it.get("key").and_then(Value::as_str) == Some("ipcc"))
                    })
                    .and_then(|it| it.get("value"))
                    .map(value_to_string)
                    .unwrap_or_else(|| "1".to_string()); // absent → '1' (legacy ?? '1')
                if !use_podcast_participant_as_artist && ipcc != "0" {
                    if let Some(owner) = &owner_name {
                        info.artist = owner.clone();
                    }
                }
            }
            _ => {}
        }

        // imageSrc: last thumbnail url, '?'-stripped.
        let thumbs = vd
            .get("thumbnail")
            .and_then(|t| t.get("thumbnails"))
            .and_then(|t| t.as_array());
        if let Some(thumbs) = thumbs {
            if let Some(last) = thumbs.last() {
                let url = last.get("url").map(value_to_string).unwrap_or_default();
                info.image_src = url.split('?').next().unwrap_or("").to_string();
            }
        }
    }

    // image: always None (image payload dropped in the port).
    // isPaused stays None here; the video-src command sets Some(false)
    // (the page pre-injects isPaused false) and play-or-paused updates later.
    info
}

// ---------------------------------------------------------------------------
// Emission helper
// ---------------------------------------------------------------------------

fn emit_ipc_event(app: &AppHandle, channel: &str, args: Value) -> Result<(), String> {
    app.emit("pear-ipc-event", json!({ "channel": channel, "args": args }))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Renderer sent a new GetPlayerResponse (video src changed): rebuild the
/// song info, cache it, persist the canonical URL and notify consumers.
#[tauri::command]
pub fn peard_video_src_changed(
    app: AppHandle,
    state: State<'_, SharedStore>,
    song: State<'_, SharedSong>,
    player_response: Value,
) -> Result<(), String> {
    // Config option (legacy options-level key), default false.
    let use_podcast = {
        let store = state
            .lock()
            .map_err(|_| "store lock poisoned".to_string())?;
        store
            .get_bool("options.usePodcastParticipantAsArtist")
            .unwrap_or(false)
    };

    let mut info = parse_player_response(&player_response, use_podcast);
    // F-9 — parse_player_response now maps videoDetails.isPaused / elapsedSec
    // (legacy song-info.ts:114-115); these are only FALLBACKS for when the
    // payload omitted them (the renderer page pre-injects isPaused=false).
    if info.is_paused.is_none() {
        info.is_paused = Some(false);
    }

    // Persist the FULL canonical URL as config 'url' (resumeOnStart reads
    // it). Legacy config.set emitted config-changed snapshots, so use
    // apply_config_change for parity; 'restart_if_configured' is false.
    // When urlCanonical is missing/empty, record nothing and proceed.
    let full_canonical = player_response
        .get("microformat")
        .and_then(|m| m.get("microformatDataRenderer"))
        .and_then(|m| m.get("urlCanonical"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(canonical) = full_canonical {
        let mut store = state
            .lock()
            .map_err(|_| "store lock poisoned".to_string())?;
        apply_config_change(&app, &mut store, "url", Value::String(canonical), false);
    }

    // Cache before emitting.
    {
        let mut song_state = song
            .lock()
            .map_err(|_| "song lock poisoned".to_string())?;
        song_state.current = Some(info.clone());
    }

    emit_ipc_event(&app, "peard:update-song-info", json!([info]))
}

/// Play/pause state changed: merge into the cached song info and re-emit on
/// the original channel so consumers (synced-lyrics, song-info-front) get the
/// updated songInfo.
#[tauri::command]
pub fn peard_play_or_paused(
    app: AppHandle,
    song: State<'_, SharedSong>,
    _store: State<'_, SharedStore>,
    is_paused: Option<bool>,
    elapsed_seconds: Option<f64>,
) -> Result<(), String> {
    let merged = {
        let mut song_state = song
            .lock()
            .map_err(|_| "song lock poisoned".to_string())?;
        let mut info = song_state
            .current
            .clone()
            .unwrap_or_else(default_song_info);
        if let Some(paused) = is_paused {
            info.is_paused = Some(paused);
        }
        if let Some(elapsed) = elapsed_seconds {
            info.elapsed_seconds = elapsed;
        }
        song_state.current = Some(info.clone());
        info
    };

    // Keep the tray in sync; the tray module is lead-wired, ignore failures.
    let _ = crate::tray::update_tray(&app, is_paused);

    emit_ipc_event(&app, "peard:play-or-paused", json!([merged]))
}

/// Elapsed-time tick: only updates the cached elapsedSeconds. Legacy
/// time-changed was inert for song info (elapsed tracking only, no emit).
#[tauri::command]
pub fn peard_time_changed(
    _app: AppHandle,
    song: State<'_, SharedSong>,
    seconds: Option<f64>,
) -> Result<(), String> {
    let mut song_state = song
        .lock()
        .map_err(|_| "song lock poisoned".to_string())?;
    if let (Some(info), Some(sec)) = (song_state.current.as_mut(), seconds) {
        info.elapsed_seconds = sec;
    }
    Ok(())
}
