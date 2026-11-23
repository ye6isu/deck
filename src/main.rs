#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use egui::{Color32, Stroke, Rounding, Vec2, Rect, pos2};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

mod theme;
mod dsp;
mod metadata;
mod audio_engine;

use theme::WarmIndustrialTheme;
use metadata::TrackMetadata;
use audio_engine::{AudioEngine, AudioCommand};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FloatingControlType {
    Volume,
    Speed,
    Reverb,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PlaylistItem {
    pub id: usize,
    pub title: String,
    pub artist: String,
    pub duration_str: String,
    pub path: PathBuf,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SavedConfig {
    pub library_folder: Option<PathBuf>,
    pub playlist: Vec<PlaylistItem>,
    pub current_track_path: Option<PathBuf>,
    pub current_progress: f32,
    pub volume: f32,
    pub speed: f32,
    pub reverb: f32,
    pub shuffle: bool,
    #[serde(default)]
    pub playback_order: Vec<PathBuf>,
    #[serde(default)]
    pub appearance: u8,
    #[serde(default)]
    pub accent_color: u8,
    #[serde(default = "default_lyrics_size")]
    pub lyrics_size: f32,
    #[serde(default = "default_lyrics_spacing")]
    pub lyrics_spacing: f32,
    #[serde(default = "default_lyrics_context")]
    pub lyrics_context: u8, // e.g. 2 lines before & 2 lines after (total 5)
    #[serde(default)]
    pub lyrics_align: u8, // 0 = Center, 1 = Left, 2 = Right
}

fn default_lyrics_size() -> f32 { 18.0 }
fn default_lyrics_spacing() -> f32 { 14.0 }
fn default_lyrics_context() -> u8 { 2 }

impl Default for SavedConfig {
    fn default() -> Self {
        Self {
            library_folder: None,
            playlist: Vec::new(),
            current_track_path: None,
            current_progress: 0.0,
            volume: 0.85,
            speed: 1.0,
            reverb: 0.0,
            shuffle: false,
            playback_order: Vec::new(),
            appearance: 0,
            accent_color: 0,
            lyrics_size: 18.0,
            lyrics_spacing: 14.0,
            lyrics_context: 2,
            lyrics_align: 0,
        }
    }
}

pub struct DeckApp {
    search_query: String,
    playlist: Vec<PlaylistItem>,
    selected_idx: Option<usize>,

    // Transport
    is_playing: bool,
    progress: f32,
    pos_str: String,
    dur_str: String,
    volume: f32,

    // DSP
    speed: f32,
    reverb_amount: f32,

    // YouTube-style hover transition (0.0 to 1.0)
    hover_transition: f32,

    // Shuffle & History & Queue
    shuffle_enabled: bool,
    shuffle_order: Vec<usize>,
    shuffle_pos: usize,
    highlighted_idx: Option<usize>,
    queue_open: bool,
    queue_drag: Option<usize>,
    appearance: u8,
    accent_color: u8,
    user_queue: Vec<usize>,

    // Settings
    library_folder: Option<PathBuf>,
    settings_open: bool,
    settings_section: u8,
    library_menu_open: bool,
    library_arrange_open: bool,

    // Artwork & Metadata
    cover_texture: Option<egui::TextureHandle>,
    current_meta: Option<TrackMetadata>,
    metadata_rx: Option<crossbeam_channel::Receiver<(TrackMetadata, Option<egui::ColorImage>)>>,

    // Floating Popups
    speed_open: bool,
    reverb_open: bool,

    // Lyrics view state
    lyrics_open: bool,
    lyrics_scroll_offset: f32, // smoothly animated visual line position (float)
    lyrics_user_offset: f32,   // manual scroll offset in lines relative to active line
    lyrics_user_scroll_timer: f32, // countdown timer (seconds) before snapping back to active line
    lyrics_size: f32,
    lyrics_spacing: f32,
    lyrics_context: u8,
    lyrics_align: u8,

    // Vertical floating preview popup (on hover or scroll of volume, speed, reverb)
    active_floating_control: Option<FloatingControlType>,
    floating_control_timer: f32,

    // Playhead scrubbing state
    is_scrubbing: bool,
    audio_revision: u64,
    audio_error: Option<String>,

    // WASAPI Audio Pipeline
    audio: Option<AudioEngine>,

    // Windows SMTC Controls
    media_controls: Option<souvlaki::MediaControls>,
    media_rx: Option<crossbeam_channel::Receiver<souvlaki::MediaControlEvent>>,
    smtc_inited: bool,
    bg_tx: Option<crossbeam_channel::Sender<crossbeam_channel::Sender<bool>>>,
}

fn duration_seconds(value: &str) -> u32 {
    value.split(':').filter_map(|v| v.parse::<u32>().ok()).fold(0, |total, n| total * 60 + n)
}

#[derive(Clone, Debug)]
pub struct LyricLine {
    pub time_secs: Option<f64>,
    pub text: String,
}

fn parse_timestamp_tag(tag: &str) -> Option<f64> {
    let parts: Vec<&str> = tag.split(':').collect();
    if parts.len() == 2 {
        if let (Ok(m), Ok(s)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
            return Some(m * 60.0 + s);
        }
    }
    None
}

fn strip_lyric_prefixes(s: &str) -> &str {
    let mut cur = s.trim();
    // Strip prefixes like "v1:", "v2:", "p1:", "c1:", "ch:", "v1 ", etc.
    let prefixes = ["v1:", "v2:", "v3:", "v4:", "p1:", "p2:", "c1:", "c2:", "ch:", "v1", "v2"];
    for p in prefixes {
        if cur.to_lowercase().starts_with(p) {
            let rem = &cur[p.len()..];
            cur = rem.trim_start_matches(':').trim();
        }
    }
    cur
}

fn strip_word_timestamps(s: &str) -> String {
    let mut result = String::new();
    let mut in_bracket = false;
    for ch in s.chars() {
        if ch == '<' {
            in_bracket = true;
        } else if ch == '>' {
            in_bracket = false;
        } else if !in_bracket {
            result.push(ch);
        }
    }
    result
}

fn strip_metadata_headers(s: &str) -> bool {
    let lower = s.to_lowercase();
    // Standard ID tags in LRC files: [ti:Title], [ar:Artist], [al:Album], [by:...], [offset:...], [length:...], [re:...], [ve:...]
    for tag in ["[ti:", "[ar:", "[al:", "[by:", "[offset:", "[length:", "[re:", "[ve:"] {
        if lower.starts_with(tag) {
            return true;
        }
    }
    false
}

fn extract_background_vocal_content(s: &str) -> Option<String> {
    let cur = s.trim();
    if cur.to_lowercase().starts_with("[bg:") {
        if let Some(close) = cur.rfind(']') {
            let inner = cur[4..close].trim();
            let clean = if inner.starts_with('(') && inner.ends_with(')') {
                inner[1..inner.len() - 1].trim()
            } else {
                inner
            };
            return Some(format!("({})", clean));
        }
    }
    None
}

pub fn parse_lrc_lines(raw: &str) -> Vec<LyricLine> {
    let mut lines: Vec<LyricLine> = Vec::new();
    for line in raw.lines() {
        let mut trimmed = line.trim();
        if trimmed.is_empty() || strip_metadata_headers(trimmed) {
            continue;
        }

        // Check if the raw line is a standalone background vocal tag: e.g. [bg:<00:06.46>(Rock it)<...>]
        let is_bg_line = trimmed.to_lowercase().starts_with("[bg:");
        if is_bg_line {
            let stripped_words = strip_word_timestamps(trimmed);
            if let Some(bg_text) = extract_background_vocal_content(&stripped_words) {
                if !bg_text.is_empty() {
                    // Append background vocal to previous line if one exists
                    if let Some(prev) = lines.last_mut() {
                        prev.text.push(' ');
                        prev.text.push_str(&bg_text);
                    } else {
                        lines.push(LyricLine {
                            time_secs: None,
                            text: bg_text,
                        });
                    }
                }
                continue;
            }
        }

        // 1. Strip voice/section tag if present at start (e.g. "v1:")
        trimmed = strip_lyric_prefixes(trimmed);

        let mut time_secs = None;
        let mut remaining_text = trimmed;

        // 2. Check for standard line timestamp at beginning: [mm:ss.xx]
        if remaining_text.starts_with('[') {
            if let Some(close_bracket) = remaining_text.find(']') {
                let time_part = &remaining_text[1..close_bracket];
                if let Some(ts) = parse_timestamp_tag(time_part) {
                    time_secs = Some(ts);
                    remaining_text = remaining_text[close_bracket + 1..].trim();
                }
            }
        }

        // Also check if line timestamp is in enhanced format <mm:ss.xxx> at the beginning
        if time_secs.is_none() && remaining_text.starts_with('<') {
            if let Some(close_bracket) = remaining_text.find('>') {
                let time_part = &remaining_text[1..close_bracket];
                if let Some(ts) = parse_timestamp_tag(time_part) {
                    time_secs = Some(ts);
                    remaining_text = remaining_text[close_bracket + 1..].trim();
                }
            }
        }

        // 3. Strip voice prefix again if it was inside the timestamp: e.g. [01:23.45]v1: ...
        remaining_text = strip_lyric_prefixes(remaining_text);

        // 4. Strip any inline word-level timestamps <mm:ss.xxx>
        let clean_text = strip_word_timestamps(remaining_text);
        let final_text = if let Some(bg_text) = extract_background_vocal_content(&clean_text) {
            bg_text
        } else {
            clean_text.trim().to_string()
        };

        if !final_text.is_empty() {
            lines.push(LyricLine {
                time_secs,
                text: final_text,
            });
        }
    }
    lines
}

fn dismiss_popup(ctx: &egui::Context, popup: Rect, anchor: Rect) -> bool {
    ctx.input(|i| i.key_pressed(egui::Key::Escape) || (i.pointer.any_pressed()
        && i.pointer.interact_pos().map_or(false, |p| !popup.contains(p) && !anchor.contains(p))))
}

fn compact_menu_style(ui: &mut egui::Ui) {
    ui.spacing_mut().button_padding = Vec2::new(2.0, 0.0);
    ui.visuals_mut().widgets.active.bg_stroke = Stroke::NONE;
    ui.visuals_mut().widgets.hovered.bg_stroke = Stroke::NONE;
    ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
}

fn submenu_position(parent: Rect, width: f32, screen: Rect) -> egui::Pos2 {
    let x = if parent.right() + 2.0 + width <= screen.right() { parent.right() + 2.0 }
        else { parent.left() - 2.0 - width };
    pos2(x.max(screen.left()), parent.top())
}

fn config_path() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    p.pop();
    p.push("deck_state.json");
    p
}

fn load_saved_state() -> SavedConfig {
    let path = config_path();
    if let Ok(bytes) = std::fs::read(&path) {
        if let Ok(cfg) = serde_json::from_slice::<SavedConfig>(&bytes) {
            return cfg;
        }
    }
    SavedConfig::default()
}

fn save_state(cfg: &SavedConfig) {
    let path = config_path();
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, json);
    }
}

impl Default for DeckApp {
    fn default() -> Self {
        let saved = load_saved_state();
        let (engine, error) = match AudioEngine::new() {
            Ok(engine) => (Some(engine), None),
            Err(error) => (None, Some(error)),
        };
        let mut app = Self::from_saved(saved, engine);
        app.audio_error = error;
        app
    }
}

impl DeckApp {
    fn from_saved(saved: SavedConfig, engine: Option<AudioEngine>) -> Self {
        let mut app = Self {
            search_query: String::new(),
            playlist: saved.playlist,
            selected_idx: None,
            is_playing: false,
            progress: saved.current_progress,
            pos_str: "00:00".into(),
            dur_str: "00:00".into(),
            volume: saved.volume,
            speed: saved.speed,
            reverb_amount: saved.reverb,
            hover_transition: 0.0,
            shuffle_enabled: saved.shuffle,
            shuffle_order: Vec::new(),
            shuffle_pos: 0,
            highlighted_idx: None,
            queue_open: false,
            queue_drag: None,
            appearance: saved.appearance,
            accent_color: saved.accent_color,
            user_queue: Vec::new(),
            library_folder: saved.library_folder,
            settings_open: false,
            settings_section: 0,
            library_menu_open: false,
            library_arrange_open: false,
            cover_texture: None,
            current_meta: None,
            metadata_rx: None,
            speed_open: false,
            reverb_open: false,
            lyrics_open: false,
            lyrics_scroll_offset: 0.0,
            lyrics_user_offset: 0.0,
            lyrics_user_scroll_timer: 0.0,
            lyrics_size: saved.lyrics_size,
            lyrics_spacing: saved.lyrics_spacing,
            lyrics_context: saved.lyrics_context,
            lyrics_align: saved.lyrics_align,
            active_floating_control: None,
            floating_control_timer: 0.0,
            is_scrubbing: false,
            audio_revision: 0,
            audio_error: None,
            audio: engine,
            media_controls: None,
            media_rx: None,
            smtc_inited: false,
            bg_tx: None,
        };

        if let Some(folder) = app.library_folder.clone() {
            app.scan_folder(&folder);
        }

        if let Some(track_path) = saved.current_track_path {
            if let Some(found_idx) = app.playlist.iter().position(|p| p.path == track_path) {
                app.selected_idx = Some(found_idx);
                app.load_artwork(track_path.clone());

                if let Some(audio) = &app.audio {
                    app.audio_revision = audio.send(AudioCommand::Load {
                        path: track_path,
                        autoplay: false,
                        start_progress: app.progress,
                    });
                    audio.send(AudioCommand::SetSpeed(app.speed));
                    audio.send(AudioCommand::SetReverb(app.reverb_amount));
                    audio.send(AudioCommand::SetVolume(app.volume));
                }
            }
        }

        app.shuffle_order = saved.playback_order.iter().filter_map(|path| app.playlist.iter().position(|t| &t.path == path)).collect();
        if app.shuffle_order.len() != app.playlist.len() { app.rebuild_shuffle_order(app.selected_idx); }
        else { app.shuffle_pos = app.selected_idx.and_then(|i| app.shuffle_order.iter().position(|&x| x == i)).unwrap_or(0); }


        app
    }
}

impl DeckApp {
    fn save_current_state(&self) {
        let cur_path = self.selected_idx.and_then(|i| self.playlist.get(i)).map(|t| t.path.clone());
        let cfg = SavedConfig {
            library_folder: self.library_folder.clone(),
            playlist: self.playlist.clone(),
            current_track_path: cur_path,
            current_progress: self.progress,
            volume: self.volume,
            speed: self.speed,
            reverb: self.reverb_amount,
            shuffle: self.shuffle_enabled,
            playback_order: self.shuffle_order.iter().filter_map(|&i| self.playlist.get(i).map(|t| t.path.clone())).collect(),
            appearance: self.appearance,
            accent_color: self.accent_color,
            lyrics_size: self.lyrics_size,
            lyrics_spacing: self.lyrics_spacing,
            lyrics_context: self.lyrics_context,
            lyrics_align: self.lyrics_align,
        };
        save_state(&cfg);
    }

    fn rebuild_shuffle_order(&mut self, current_idx: Option<usize>) {
        let n = self.playlist.len();
        if n == 0 {
            self.shuffle_order.clear();
            self.shuffle_pos = 0;
            return;
        }

        let mut order: Vec<usize> = (0..n).collect();
        let mut rng = rand::thread_rng();
        if self.shuffle_enabled { order.shuffle(&mut rng); }

        if let Some(cur) = current_idx {
            if let Some(pos) = order.iter().position(|&x| x == cur) {
                if self.shuffle_enabled { order.swap(0, pos); } else { order.rotate_left(pos); }
            }
        }

        self.user_queue.clear();
        self.shuffle_order = order;
        self.shuffle_pos = 0;
    }

    fn preferences_ui(&mut self, ctx: &egui::Context) {
        let colors = WarmIndustrialTheme::palette(self.appearance, self.accent_color);
        WarmIndustrialTheme::apply(ctx, colors);
        egui::SidePanel::left("preferences_categories").exact_width(136.0).resizable(false)
            .frame(egui::Frame::none().fill(colors.card).inner_margin(egui::Margin::same(12.0)))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.selectable_value(&mut self.settings_section, 0, "Library");
                ui.selectable_value(&mut self.settings_section, 1, "Appearance");
                ui.selectable_value(&mut self.settings_section, 2, "Lyrics");
            });
        egui::CentralPanel::default().frame(egui::Frame::none().fill(colors.background).inner_margin(egui::Margin::same(20.0)))
            .show(ctx, |ui| {
                if self.settings_section == 0 {
                    ui.heading("Library");
                    ui.add_space(16.0);
                    ui.label("Music folder");
                    let folder = self.library_folder.as_ref().map_or("No folder selected".into(), |p| p.display().to_string());
                    ui.label(egui::RichText::new(folder).color(colors.muted));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Choose folder").clicked() {
                            if let Some(folder) = rfd::FileDialog::new().pick_folder() { self.library_folder = Some(folder.clone()); self.scan_folder(&folder); }
                        }
                        if ui.add_enabled(self.library_folder.is_some(), egui::Button::new("Rescan")).clicked() {
                            if let Some(folder) = self.library_folder.clone() { self.scan_folder(&folder); }
                        }
                    });
                } else if self.settings_section == 1 {
                    ui.heading("Appearance");
                    ui.add_space(16.0);
                    let mut changed = false;
                    egui::ComboBox::from_label("Theme").selected_text(["Warm", "Paper", "Dark"][self.appearance.min(2) as usize]).show_ui(ui, |ui| {
                        for (i, label) in ["Warm", "Paper", "Dark"].iter().enumerate() { changed |= ui.selectable_value(&mut self.appearance, i as u8, *label).changed(); }
                    });
                    ui.add_space(8.0);
                    egui::ComboBox::from_label("Accent color").selected_text(["Orange", "Blue", "Green", "Purple"][self.accent_color.min(3) as usize]).show_ui(ui, |ui| {
                        for (i, label) in ["Orange", "Blue", "Green", "Purple"].iter().enumerate() { changed |= ui.selectable_value(&mut self.accent_color, i as u8, *label).changed(); }
                    });
                    if changed { self.save_current_state(); ctx.request_repaint_of(egui::ViewportId::ROOT); }
                } else {
                    ui.heading("Lyrics");
                    ui.add_space(16.0);
                    let mut changed = false;

                    ui.label(egui::RichText::new("Font size").strong());
                    changed |= ui.add(egui::Slider::new(&mut self.lyrics_size, 12.0..=36.0).text("px")).changed();
                    ui.add_space(8.0);

                    ui.label(egui::RichText::new("Line spacing").strong());
                    changed |= ui.add(egui::Slider::new(&mut self.lyrics_spacing, 4.0..=32.0).text("px")).changed();
                    ui.add_space(8.0);

                    ui.label(egui::RichText::new("Context lines (before / after)").strong());
                    changed |= ui.add(egui::Slider::new(&mut self.lyrics_context, 1..=4).text("lines")).changed();
                    ui.label(egui::RichText::new(format!("Displays {} lines total around current line", (self.lyrics_context as usize * 2) + 1)).size(10.0).color(colors.muted));
                    ui.add_space(8.0);

                    ui.label(egui::RichText::new("Text alignment").strong());
                    egui::ComboBox::from_label("Alignment").selected_text(["Center", "Left", "Right"][self.lyrics_align.min(2) as usize]).show_ui(ui, |ui| {
                        for (i, label) in ["Center", "Left", "Right"].iter().enumerate() {
                            changed |= ui.selectable_value(&mut self.lyrics_align, i as u8, *label).changed();
                        }
                    });

                    if changed { self.save_current_state(); ctx.request_repaint_of(egui::ViewportId::ROOT); }
                }
            });
    }

    fn load_artwork(&mut self, path: PathBuf) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.metadata_rx = Some(rx);
        std::thread::spawn(move || {
            let meta = metadata::extract_metadata(&path);
            let cover = meta.cover_image.as_ref().and_then(|bytes| image::load_from_memory(bytes).ok()).map(|image| {
                let image = image.thumbnail(1024, 1024).to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw())
            });
            let _ = tx.send((meta, cover));
        });
    }

    fn play_track_at_index(&mut self, idx: usize, is_user_direct_pick: bool) {
        if idx >= self.playlist.len() { return; }
        self.selected_idx = Some(idx);
        self.highlighted_idx = Some(idx);
        let item = &self.playlist[idx];
        let path = item.path.clone();
        self.current_meta = Some(TrackMetadata { title: item.title.clone(), artist: item.artist.clone(), ..Default::default() });
        self.cover_texture = None;
        self.is_playing = true;
        self.progress = 0.0;
        self.pos_str = "00:00".into();
        if let Some(audio) = &self.audio {
            self.audio_revision = audio.send(AudioCommand::Load { path: path.clone(), autoplay: true, start_progress: 0.0 });
            audio.send(AudioCommand::SetSpeed(self.speed));
            audio.send(AudioCommand::SetReverb(self.reverb_amount));
            audio.send(AudioCommand::SetVolume(self.volume));
        }
        self.load_artwork(path);
        self.lyrics_scroll_offset = 0.0;
        self.lyrics_user_offset = 0.0;
        self.lyrics_user_scroll_timer = 0.0;
        if is_user_direct_pick { self.rebuild_shuffle_order(Some(idx)); }
        self.save_current_state();
    }

    fn advance_order(&mut self, forward: bool) -> Option<usize> {
        if self.playlist.is_empty() { return None; }
        if self.shuffle_order.len() != self.playlist.len() { self.rebuild_shuffle_order(self.selected_idx); }
        let len = self.shuffle_order.len();
        if self.selected_idx.is_some() {
            self.shuffle_pos = if forward { (self.shuffle_pos + 1) % len } else { (self.shuffle_pos + len - 1) % len };
        }
        let index = self.shuffle_order[self.shuffle_pos];
        self.user_queue.retain(|&i| i != index);
        Some(index)
    }

    fn next_track(&mut self) {
        if let Some(index) = self.advance_order(true) { self.play_track_at_index(index, false); }
    }

    fn prev_track(&mut self) {
        if let Some(index) = self.advance_order(false) { self.play_track_at_index(index, false); }
    }

    fn play_next(&mut self, index: usize) {
        if index >= self.playlist.len() || self.selected_idx == Some(index) { return; }
        if self.shuffle_order.len() != self.playlist.len() { self.rebuild_shuffle_order(self.selected_idx); }
        if let Some(position) = self.shuffle_order.iter().position(|&i| i == index) {
            self.shuffle_order.remove(position);
            if position < self.shuffle_pos { self.shuffle_pos -= 1; }
        }
        let position = if self.selected_idx.is_some() { self.shuffle_pos + 1 } else { 0 };
        self.shuffle_order.insert(position, index);
        self.user_queue.retain(|&i| i != index);
        self.user_queue.insert(0, index);
        self.save_current_state();
    }

    fn move_queue_track(&mut self, index: usize, insertion: usize) {
        if let Some(from) = self.shuffle_order.iter().position(|&item| item == index) {
            let target = insertion.min(self.shuffle_order.len());
            self.shuffle_order.remove(from);
            let target = if target > from { target - 1 } else { target };
            self.shuffle_order.insert(target, index);
            self.shuffle_pos = self.selected_idx.and_then(|i| self.shuffle_order.iter().position(|&x| x == i)).unwrap_or(0);
            self.save_current_state();
        }
    }

    fn arrange_library(&mut self, sort: u8) {
        let current = self.selected_idx.and_then(|i| self.playlist.get(i)).map(|t| t.path.clone());
        let order: Vec<_> = self.shuffle_order.iter().filter_map(|&i| self.playlist.get(i).map(|t| t.path.clone())).collect();
        let queued: Vec<_> = self.user_queue.iter().filter_map(|&i| self.playlist.get(i).map(|t| t.path.clone())).collect();
        self.playlist.sort_by(|a, b| match sort {
            1 => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()).then(a.title.cmp(&b.title)),
            2 => duration_seconds(&a.duration_str).cmp(&duration_seconds(&b.duration_str)),
            _ => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        });
        self.selected_idx = current.and_then(|p| self.playlist.iter().position(|t| t.path == p));
        self.highlighted_idx = None;
        self.shuffle_order = order.iter().filter_map(|p| self.playlist.iter().position(|t| &t.path == p)).collect();
        for i in 0..self.playlist.len() { if !self.shuffle_order.contains(&i) { self.shuffle_order.push(i); } }
        self.shuffle_pos = self.selected_idx.and_then(|i| self.shuffle_order.iter().position(|&x| x == i)).unwrap_or(0);
        self.user_queue = queued.iter().filter_map(|p| self.playlist.iter().position(|t| &t.path == p)).collect();
        for (i, item) in self.playlist.iter_mut().enumerate() { item.id = i + 1; }
        self.save_current_state();
    }

    fn scan_folder(&mut self, folder: &Path) {
        let mut audio_files = Vec::new();
        collect_audio_files(folder, &mut audio_files);

        // Remove any songs in playlist that no longer exist on disk or are not in the scanned library
        let current_track = self.selected_idx.and_then(|i| self.playlist.get(i)).map(|t| t.path.clone());
        let had_tracks = !self.playlist.is_empty();

        self.playlist.retain(|item| item.path.exists() && audio_files.iter().any(|p| p == &item.path));

        for path in audio_files {
            if !self.playlist.iter().any(|item| item.path == path) {
                let meta = metadata::extract_metadata_fast(&path);
                self.playlist.push(PlaylistItem {
                    id: 0,
                    title: meta.title,
                    artist: meta.artist,
                    duration_str: meta.duration_str,
                    path,
                });
            }
        }

        self.arrange_library(0);
        for (i, item) in self.playlist.iter_mut().enumerate() {
            item.id = i + 1;
        }

        // Resync current track, shuffle order and user queue after items removal
        if had_tracks {
            self.selected_idx = current_track.and_then(|p| self.playlist.iter().position(|t| t.path == p));
            self.user_queue.retain(|&idx| idx < self.playlist.len());
            if self.shuffle_order.len() != self.playlist.len() {
                self.rebuild_shuffle_order(self.selected_idx);
            }
        }

        self.save_current_state();
    }

    fn add_paths_recursive(&mut self, paths: &[PathBuf]) {
        // First clean up any tracks in the current playlist that no longer exist on disk
        let current_track = self.selected_idx.and_then(|i| self.playlist.get(i)).map(|t| t.path.clone());
        let had_tracks = !self.playlist.is_empty();
        self.playlist.retain(|item| item.path.exists());

        let mut audio_files = Vec::new();
        for p in paths {
            collect_audio_files(p, &mut audio_files);
        }

        for path in audio_files {
            if !self.playlist.iter().any(|item| item.path == path) {
                let meta = metadata::extract_metadata_fast(&path);
                self.playlist.push(PlaylistItem {
                    id: 0,
                    title: meta.title,
                    artist: meta.artist,
                    duration_str: meta.duration_str,
                    path,
                });
            }
        }

        self.arrange_library(0);
        for (i, item) in self.playlist.iter_mut().enumerate() {
            item.id = i + 1;
        }

        if had_tracks {
            self.selected_idx = current_track.and_then(|p| self.playlist.iter().position(|t| t.path == p));
            self.user_queue.retain(|&idx| idx < self.playlist.len());
            if self.shuffle_order.len() != self.playlist.len() {
                self.rebuild_shuffle_order(self.selected_idx);
            }
        }

        self.save_current_state();
    }
}

fn collect_audio_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                collect_audio_files(&entry.path(), out);
            }
        }
    } else if path.is_file() {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "alac" | "wma" | "aiff" => {
                    out.push(path.to_path_buf());
                }
                _ => {}
            }
        }
    }
}

impl eframe::App for DeckApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.smtc_inited {
            self.smtc_inited = true;
            self.init_media_controls(frame);

            #[cfg(target_os = "windows")]
            {
                use raw_window_handle::HasWindowHandle;
                if let Ok(handle) = frame.window_handle() {
                    if let raw_window_handle::RawWindowHandle::Win32(win32) = handle.as_raw() {
                        let raw_hwnd = win32.hwnd.get() as usize;
                        let (req_tx, req_rx) = crossbeam_channel::bounded::<crossbeam_channel::Sender<bool>>(1);
                        self.bg_tx = Some(req_tx.clone());
                        let ctx_clone = ctx.clone();
                        std::thread::Builder::new()
                            .name("deck-window-restorer".into())
                            .spawn(move || {
                                while let Ok(reply_tx) = req_rx.recv() {
                                    unsafe {
                                        use windows_sys::Win32::UI::WindowsAndMessaging::{
                                            IsIconic, ShowWindowAsync, SW_MINIMIZE, SW_RESTORE
                                        };
                                        let hwnd = raw_hwnd as *mut std::ffi::c_void;
                                        if IsIconic(hwnd) != 0 {
                                            ShowWindowAsync(hwnd, SW_RESTORE);
                                            std::thread::sleep(std::time::Duration::from_millis(15));
                                            ShowWindowAsync(hwnd, SW_MINIMIZE);
                                        }
                                    }
                                    ctx_clone.request_repaint();
                                    let _ = reply_tx.send(true);
                                }
                            })
                            .ok();

                        if let Some(audio) = &self.audio {
                            let wake_req_tx = req_tx;
                            let wake_ctx = ctx.clone();
                            audio.set_wake_callback(move || {
                                wake_ctx.request_repaint();
                                let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
                                let _ = wake_req_tx.send(reply_tx);
                                let _ = reply_rx.recv_timeout(std::time::Duration::from_millis(100));
                            });
                        }
                    }
                }
            }
        }
        self.update_ui(ctx);
    }
}

impl DeckApp {
    fn init_media_controls(&mut self, frame: &eframe::Frame) {
        #[cfg(target_os = "windows")]
        let hwnd = {
            use raw_window_handle::HasWindowHandle;
            match frame.window_handle() {
                Ok(handle) => match handle.as_raw() {
                    raw_window_handle::RawWindowHandle::Win32(win32) => {
                        Some(win32.hwnd.get() as *mut std::ffi::c_void)
                    }
                    _ => None,
                },
                Err(_) => None,
            }
        };

        #[cfg(not(target_os = "windows"))]
        let hwnd = None;

        let config = souvlaki::PlatformConfig {
            dbus_name: "deck_music_player",
            display_name: "DECK",
            hwnd,
        };

        match souvlaki::MediaControls::new(config) {
            Ok(mut controls) => {
                let (tx, rx) = crossbeam_channel::unbounded();
                if let Err(e) = controls.attach(move |event| {
                    let _ = tx.send(event);
                }) {
                    eprintln!("Failed to attach SMTC controls: {:?}", e);
                } else {
                    self.media_controls = Some(controls);
                    self.media_rx = Some(rx);
                }
            }
            Err(e) => {
                eprintln!("Failed to init SMTC media controls: {:?}", e);
            }
        }
    }

    fn update_ui(&mut self, ctx: &egui::Context) {
        let colors = WarmIndustrialTheme::palette(self.appearance, self.accent_color);
        WarmIndustrialTheme::apply(ctx, colors);

        if let Some((meta, image)) = self.metadata_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            let title = meta.title.clone();
            let artist = meta.artist.clone();
            self.current_meta = Some(meta);
            self.cover_texture = image.map(|image| ctx.load_texture("cover", image, Default::default()));
            self.metadata_rx = None;

            if let Some(controls) = &mut self.media_controls {
                let _ = controls.set_metadata(souvlaki::MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artist),
                    album: None,
                    duration: None,
                    cover_url: None,
                });
            }
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Comma)) { self.settings_open = !self.settings_open; }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            if let Some(folder) = self.library_folder.clone() {
                self.scan_folder(&folder);
            }
        }

        // Process Windows System Media Transport Controls (SMTC) events
        let smtc_events: Vec<souvlaki::MediaControlEvent> = if let Some(rx) = &self.media_rx {
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            events
        } else {
            Vec::new()
        };

        for event in smtc_events {
            match event {
                souvlaki::MediaControlEvent::Toggle
                | souvlaki::MediaControlEvent::Play
                | souvlaki::MediaControlEvent::Pause => {
                    if let Some(audio) = &self.audio {
                        audio.send(AudioCommand::Toggle);
                    }
                }
                souvlaki::MediaControlEvent::Next => {
                    self.next_track();
                }
                souvlaki::MediaControlEvent::Previous => {
                    self.prev_track();
                }
                souvlaki::MediaControlEvent::Stop => {
                    if self.is_playing {
                        if let Some(audio) = &self.audio {
                            audio.send(AudioCommand::Toggle);
                        }
                    }
                }
                _ => {}
            }
        }

        // Fallback: Windows Global Multimedia Keyboard Keys Handling (Play/Pause, Next, Prev)
        unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
                GetAsyncKeyState, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PREV_TRACK
            };
            if (GetAsyncKeyState(VK_MEDIA_PLAY_PAUSE as i32) as u16 & 0x0001) != 0 {
                if let Some(audio) = &self.audio {
                    audio.send(AudioCommand::Toggle);
                }
            }
            if (GetAsyncKeyState(VK_MEDIA_NEXT_TRACK as i32) as u16 & 0x0001) != 0 {
                self.next_track();
            }
            if (GetAsyncKeyState(VK_MEDIA_PREV_TRACK as i32) as u16 & 0x0001) != 0 {
                self.prev_track();
            }
        }

        let window_title = match &self.current_meta {
            Some(m) if self.is_playing => format!("{} - {} | DECK", m.title, m.artist),
            Some(m) => format!("{} - {} (Paused) | DECK", m.title, m.artist),
            None => "DECK".to_string(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(window_title));

        let mut track_finished = false;
        if let Some(audio) = &self.audio {
            while let Some(st) = audio.try_recv_status() {
                if st.revision != self.audio_revision { continue; }
                if let Some(error) = st.error { self.audio_error = Some(error); }
                let state_changed = self.is_playing != st.is_playing;
                self.is_playing = st.is_playing;
                if state_changed {
                    if let Some(controls) = &mut self.media_controls {
                        let _ = controls.set_playback(if self.is_playing {
                            souvlaki::MediaPlayback::Playing { progress: None }
                        } else {
                            souvlaki::MediaPlayback::Paused { progress: None }
                        });
                    }
                }
                if !self.is_scrubbing {
                    self.progress = st.progress;
                    let cur_s = st.position_secs as u64;
                    self.pos_str = format!("{:02}:{:02}", cur_s / 60, cur_s % 60);
                }

                let dur_s = st.duration_secs as u64;
                self.dur_str = format!("{:02}:{:02}", dur_s / 60, dur_s % 60);

                if st.finished && !self.is_scrubbing {
                    track_finished = true;
                }
            }
        }
        if track_finished {
            self.next_track();
        }

        ctx.input(|i| {
            if !i.raw.dropped_files.is_empty() {
                let dropped: Vec<PathBuf> = i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect();
                if !dropped.is_empty() {
                    self.add_paths_recursive(&dropped);
                }
            }
        });

        // Power & GPU Saver: Throttle repaints when window is unfocused / in background.
        // Smooth 60 FPS is only needed when DECK is focused and actively animating.
        // In the background or minimized, 10 FPS (100ms) or 4 FPS (250ms) keeps CPU/GPU load at ~0%
        // while guaranteeing track changes and status updates process immediately.
        let is_window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if is_window_focused && (self.is_playing || self.is_scrubbing || (self.hover_transition > 0.001 && self.hover_transition < 0.999) || self.lyrics_open) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16)); // 60 FPS smooth animation lerp when focused
        } else if self.is_playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(100)); // 10 FPS background playback: near 0% GPU/CPU
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(250)); // 4 FPS idle pause
        }

        // ================= CENTRAL PANEL: RESPONSIVE LANDSCAPE / PORTRAIT LAYOUT =================
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let avail_rect = ui.available_rect_before_wrap();
                // A short scroll area can enlarge its layout bounds. Paint only the
                // reserved content area, never an expanded frame over the transport.
                ui.set_clip_rect(ui.clip_rect().intersect(avail_rect));
                ui.painter().rect_filled(avail_rect, Rounding::ZERO, colors.background);
                let is_portrait = avail_rect.height() >= avail_rect.width();
                let (left_rect, art_rect) = content_rects(avail_rect);

                // ---------------- TRACK LIST COLUMN ----------------
                ui.allocate_ui_at_rect(left_rect, |ui| {
                ui.set_clip_rect(ui.clip_rect().intersect(left_rect));
                if left_rect.width() < 24.0 || left_rect.height() < 1.0 { return; }
                // Pill-shaped search bar with tight padding
                let search_height = 24.0;
                let search_margin_x = 8.0;
                let search_margin_top = 8.0;
                let search_margin_bottom = 8.0;
                let search_rect = Rect::from_min_size(
                    pos2(left_rect.min.x + search_margin_x, left_rect.min.y + search_margin_top),
                    Vec2::new((left_rect.width() - (search_margin_x * 2.0) - 32.0).max(0.0), search_height),
                );

                ui.painter().rect_filled(search_rect, Rounding::same(12.0), colors.card);
                ui.painter().rect_stroke(search_rect, Rounding::same(12.0), Stroke::new(1.0, colors.border));

                let text_rect = search_rect.shrink2(Vec2::new(10.0, 2.0));
                ui.allocate_ui_at_rect(text_rect, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("⌕").size(12.0).color(colors.muted));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text("Search library...")
                                .frame(false)
                                .desired_width((text_rect.width() - 25.0).max(0.0))
                        );
                    });
                });

                let menu_rect = Rect::from_min_size(pos2(left_rect.max.x - 32.0, search_rect.min.y), Vec2::splat(24.0));
                let menu_response = ui.interact(menu_rect, ui.id().with("library_menu_button"), egui::Sense::click());
                let icon_color = if menu_response.hovered() || self.library_menu_open { colors.accent } else { colors.text };
                ui.painter().circle_filled(menu_rect.center(), 12.0, colors.card);
                ui.painter().circle_stroke(menu_rect.center(), 12.0, Stroke::new(1.0, colors.border));
                for offset in [-4.0, 0.0, 4.0] {
                    ui.painter().circle_filled(menu_rect.center() + Vec2::new(offset, 0.0), 1.3, icon_color);
                }
                if menu_response.clicked() {
                    self.library_menu_open = !self.library_menu_open;
                    self.library_arrange_open = false;
                }
                menu_response.on_hover_text("Library menu");
                if self.library_menu_open {
                    let margin = ui.spacing().menu_margin.sum().x;
                    let parent_width = 132.0 + margin;
                    let child_width = 88.0 + margin;
                    let screen = ctx.screen_rect().shrink(4.0);
                    let mut x = (menu_rect.max.x - parent_width).clamp(screen.min.x, screen.max.x - parent_width);
                    if x - screen.min.x < child_width + 2.0 && screen.max.x - x - parent_width < child_width + 2.0 { x = screen.min.x; }
                    let popup = egui::Area::new(egui::Id::new("library_menu"))
                        .order(egui::Order::Foreground).movable(false).fixed_pos(pos2(x, menu_rect.max.y + 4.0)).constrain(true)
                        .show(ctx, |ui| {
                            compact_menu_style(ui);
                            egui::Frame::menu(ui.style()).show(ui, |ui| {
                                ui.set_width(132.0);
                                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                                    let arrange = ui.button("Arrange library  >");
                                    if arrange.hovered() || arrange.clicked() { self.library_arrange_open = true; }
                                    let rescan = ui.button("Rescan library");
                                    if rescan.hovered() { self.library_arrange_open = false; }
                                    if rescan.clicked() {
                                        if let Some(folder) = self.library_folder.clone() { self.scan_folder(&folder); }
                                        self.library_menu_open = false;
                                    }
                                    ui.separator();
                                    let preferences = ui.button("Preferences");
                                    if preferences.hovered() { self.library_arrange_open = false; }
                                    if preferences.clicked() { self.settings_open = true; self.library_menu_open = false; }
                                });
                            });
                        });
                    let mut bounds = popup.response.rect;
                    if self.library_arrange_open {
                        let position = submenu_position(bounds, child_width, screen);
                        let submenu = egui::Area::new(egui::Id::new("library_arrange_menu"))
                            .order(egui::Order::Foreground).movable(false).fixed_pos(position).constrain(true)
                            .show(ctx, |ui| {
                                compact_menu_style(ui);
                                egui::Frame::menu(ui.style()).show(ui, |ui| {
                                    ui.set_width(88.0);
                                    ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                                        for (sort, label) in ["Title", "Artist", "Duration"].iter().enumerate() {
                                            if ui.button(*label).clicked() { self.arrange_library(sort as u8); self.library_menu_open = false; }
                                        }
                                    });
                                });
                            });
                        bounds = bounds.union(submenu.response.rect);
                    }
                    if dismiss_popup(ctx, bounds, menu_rect) { self.library_menu_open = false; }
                }

                // Track List: Compact row spacing (exactly 20px per track)
                let list_rect = Rect::from_min_max(
                    pos2(left_rect.min.x, (search_rect.max.y + search_margin_bottom).min(left_rect.max.y)),
                    left_rect.max,
                );

                ui.allocate_ui_at_rect(list_rect, |ui| {
                    if self.playlist.is_empty() {
                        ui.add_space(40.0);
                        ui.vertical_centered(|ui| {
                            ui.label(egui::RichText::new("[ DROP MUSIC FOLDER HERE OR OPEN ⚙ SETTINGS ]").size(11.0).color(colors.muted));
                        });
                    } else {
                        let q = self.search_query.to_lowercase();
                        let filtered: Vec<(usize, String, String)> = self.playlist
                            .iter()
                            .enumerate()
                            .filter(|(_, t)| q.is_empty() || t.title.to_lowercase().contains(&q) || t.artist.to_lowercase().contains(&q))
                            .map(|(i, t)| (i, t.title.clone(), t.artist.clone()))
                            .collect();

                        let mut chosen_idx = None;

                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .min_scrolled_height(0.0)
                            .max_height(list_rect.height().max(0.0))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
                                let row_height = 20.0; // Strictly uniform 20px per track
                                for (idx, title, artist) in filtered {
                                    let is_sel = self.selected_idx == Some(idx) || self.highlighted_idx == Some(idx);
                                    let text_pad = 6.0;

                                    let (row_rect, response) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width() - 8.0, row_height),
                                        egui::Sense::click(),
                                    );
                                    let row_rect = row_rect.translate(Vec2::new(4.0, 0.0));

                                    // Single click selects track without playing; double click plays track
                                    if response.clicked() {
                                        self.highlighted_idx = Some(idx);
                                    }
                                    if response.double_clicked() {
                                        chosen_idx = Some(idx);
                                    }

                                    // Context menu: Right-click to add to Queue
                                    response.context_menu(|ui| {
                                        if ui.button("➕ Play next").clicked() {
                                            self.play_next(idx);
                                            ui.close_menu();
                                        }
                                        if ui.button("▶ Play Now").clicked() {
                                            chosen_idx = Some(idx);
                                            ui.close_menu();
                                        }
                                    });

                                    if is_sel {
                                        ui.painter().rect_filled(row_rect, Rounding::same(2.0), colors.card);
                                        ui.painter().rect_stroke(row_rect, Rounding::same(2.0), Stroke::new(1.0, colors.border));
                                    } else if response.hovered() {
                                        ui.painter().rect_filled(row_rect, Rounding::same(2.0), colors.hover);
                                    }

                                    let font_id = egui::FontId::monospace(10.0);
                                    let artist_font_id = egui::FontId::monospace(9.5);
                                    let title_color = colors.text;
                                    let artist_color = if is_sel { colors.accent } else { colors.muted };

                                    // Single-line text truncation to maintain strictly uniform single rows (duration removed)
                                    let available_text_width = (row_rect.width() - (text_pad * 2.0)).max(60.0);
                                    let title_allowed_width = available_text_width * 0.58;
                                    let artist_allowed_width = available_text_width * 0.40;

                                    // Render Track Title (single line with ellipsis)
                                    let mut display_title = title.clone();
                                    let mut title_galley = ui.painter().layout_no_wrap(display_title.clone(), font_id.clone(), title_color);
                                    if title_galley.size().x > title_allowed_width {
                                        while display_title.len() > 3 && title_galley.size().x > title_allowed_width {
                                            display_title.pop();
                                            title_galley = ui.painter().layout_no_wrap(format!("{}...", display_title), font_id.clone(), title_color);
                                        }
                                    }
                                    ui.painter().galley(
                                        pos2(row_rect.min.x + text_pad, row_rect.center().y - (title_galley.size().y * 0.5)),
                                        title_galley,
                                        title_color,
                                    );

                                    // Render Artist (single line with ellipsis, right aligned)
                                    let mut display_artist = artist.clone();
                                    let mut artist_galley = ui.painter().layout_no_wrap(display_artist.clone(), artist_font_id.clone(), artist_color);
                                    if artist_galley.size().x > artist_allowed_width {
                                        while display_artist.len() > 3 && artist_galley.size().x > artist_allowed_width {
                                            display_artist.pop();
                                            artist_galley = ui.painter().layout_no_wrap(format!("{}...", display_artist), artist_font_id.clone(), artist_color);
                                        }
                                    }
                                    ui.painter().galley(
                                        pos2(row_rect.max.x - text_pad - artist_galley.size().x, row_rect.center().y - (artist_galley.size().y * 0.5)),
                                        artist_galley,
                                        artist_color,
                                    );
                                }
                            });

                        if let Some(idx) = chosen_idx {
                            self.play_track_at_index(idx, true);
                        }
                    }
                });
                });

                // ---------------- ALBUM ARTWORK CONTAINER ----------------
                let right_rect = art_rect;

                // Inset divider line
                if is_portrait {
                    ui.painter().line_segment(
                        [pos2(right_rect.min.x, right_rect.max.y), pos2(right_rect.max.x, right_rect.max.y)],
                        Stroke::new(1.0, colors.border),
                    );
                } else {
                    ui.painter().line_segment(
                        [pos2(right_rect.min.x, right_rect.min.y), pos2(right_rect.min.x, right_rect.max.y)],
                        Stroke::new(1.0, colors.border),
                    );
                }

                let has_lyrics = self.current_meta.as_ref().map_or(false, |m| m.lyrics.is_some());
                if self.lyrics_open && has_lyrics {
                    // Display lyrics in the style of YouTube Music:
                    // 1. Cover artwork underneath (or dark card if no art)
                    // 2. Lightened translucent overlay (gentle darkening)
                    // 3. Proportional font and line sizing strictly tied to track cover dimensions
                    // 4. Smooth continuous position lerp between lines
                    // 5. Scroll interaction with timeout-based snap-back
                    if let Some(tex) = &self.cover_texture {
                        ui.painter().image(
                            tex.id(),
                            right_rect,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    } else {
                        ui.painter().rect_filled(right_rect, Rounding::ZERO, colors.card);
                    }
                    // Reduced darkened overlay: gentle darkening so artwork stays visible
                    ui.painter().rect_filled(right_rect, Rounding::ZERO, Color32::from_black_alpha(120));

                    let lyrics_raw = self.current_meta.as_ref().and_then(|m| m.lyrics.as_ref()).cloned().unwrap_or_default();
                    let parsed_lines = parse_lrc_lines(&lyrics_raw);
                    let track_duration = self.current_meta.as_ref().map_or(0.0, |m| m.duration_secs);

                    // Compute current active line based on playback duration and progress
                    let cur_time = self.progress as f64 * track_duration;
                    let has_timestamps = parsed_lines.iter().any(|l| l.time_secs.is_some());
                    let mut active_line_idx = 0usize;
                    if has_timestamps {
                        for (i, line) in parsed_lines.iter().enumerate() {
                            if let Some(t) = line.time_secs {
                                if t <= cur_time {
                                    active_line_idx = i;
                                }
                            }
                        }
                    }

                    // --- MOUSE WHEEL SCROLLING & AUTO-SNAP TIMER ---
                    let dt = ctx.input(|i| i.stable_dt).max(1.0 / 60.0).min(0.1);
                    // Do not scroll lyrics if cursor is over the bottom transport footer (within 58px of screen bottom)
                    // or if an active floating control popup is open.
                    let pointer_pos = ctx.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()).or_else(|| i.pointer.interact_pos()));
                    let is_in_footer_zone = pointer_pos.map_or(false, |p| p.y >= ctx.screen_rect().max.y - 58.0);
                    let pointer_in_art = pointer_pos.map_or(false, |p| right_rect.contains(p)) && !is_in_footer_zone && self.active_floating_control.is_none();
                    if pointer_in_art {
                        let scroll_delta = ctx.input(|i| {
                            if i.raw_scroll_delta.y.abs() > 0.01 {
                                i.raw_scroll_delta.y
                            } else if i.smooth_scroll_delta.y.abs() > 0.01 {
                                i.smooth_scroll_delta.y
                            } else {
                                i.events.iter().find_map(|e| match e {
                                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                                    _ => None,
                                }).unwrap_or(0.0)
                            }
                        });
                        if scroll_delta.abs() > 0.01 {
                            // Precise line stepping: 1 standard mouse wheel tick (delta ~ 50-100) moves exactly 1 line
                            let lines_to_move = if scroll_delta.abs() >= 10.0 {
                                -scroll_delta.signum() * (scroll_delta.abs() / 50.0).round().max(1.0)
                            } else {
                                -scroll_delta * 0.01
                            };
                            self.lyrics_user_offset += lines_to_move;
                            // Clamp manual scroll within bounds of track lines
                            let max_offset = parsed_lines.len() as f32;
                            if has_timestamps {
                                self.lyrics_user_offset = self.lyrics_user_offset.clamp(-(active_line_idx as f32), (max_offset - 1.0 - active_line_idx as f32).max(0.0));
                                self.lyrics_user_scroll_timer = 2.5; // Snap back after 2.5s of no scroll
                            } else {
                                // For unsynced lyrics: allow free, unbounded navigation without snap-back
                                self.lyrics_user_offset = self.lyrics_user_offset.clamp(0.0, (max_offset - 1.0).max(0.0));
                                self.lyrics_user_scroll_timer = 0.0;
                            }
                        }
                    }

                    if has_timestamps {
                        if self.lyrics_user_scroll_timer > 0.0 {
                            self.lyrics_user_scroll_timer -= dt;
                            if self.lyrics_user_scroll_timer <= 0.0 {
                                self.lyrics_user_scroll_timer = 0.0;
                            }
                        }

                        // Smoothly ease user_offset back to 0.0 when timer expires
                        if self.lyrics_user_scroll_timer <= 0.0 && self.lyrics_user_offset.abs() > 0.001 {
                            self.lyrics_user_offset += (0.0 - self.lyrics_user_offset) * (dt * 8.0).min(1.0);
                            if self.lyrics_user_offset.abs() < 0.005 {
                                self.lyrics_user_offset = 0.0;
                            }
                        }
                    }

                    // Target floating line index to center:
                    // Synced: active_line_idx + manual offset (with 2.5s snap-back)
                    // Unsynced: lyrics_user_offset directly controls position freely
                    let target_center_line = if has_timestamps {
                        (active_line_idx as f32) + self.lyrics_user_offset
                    } else {
                        self.lyrics_user_offset
                    };

                    // Smooth exponential easing for continuous, fluid transitions
                    let scroll_diff = target_center_line - self.lyrics_scroll_offset;
                    if scroll_diff.abs() > 8.0 {
                        self.lyrics_scroll_offset = target_center_line;
                    } else {
                        self.lyrics_scroll_offset += scroll_diff * (dt * 10.0).min(1.0);
                    }

                    // --- SIZING & SPACING LOCKED PROPORTIONALLY TO THE TRACK COVER ---
                    // Baseline reference: a 480px cover yields exact user settings.
                    // When the track cover expands or shrinks, the text & spacing scale in lockstep.
                    let cover_dim = right_rect.width().min(right_rect.height()).max(80.0);
                    let scale_ratio = cover_dim / 480.0;
                    let font_size = (self.lyrics_size * scale_ratio).clamp(8.0, 72.0);
                    let spacing_gap = (self.lyrics_spacing * scale_ratio).clamp(2.0, 64.0);
                    let center_y = right_rect.center().y;
                    let text_max_width = (right_rect.width() - (48.0 * scale_ratio).clamp(16.0, 48.0)).max(60.0);
                    let context_k = self.lyrics_context.clamp(1, 6) as isize;
                    let align_mode = self.lyrics_align; // 0 = Center, 1 = Left, 2 = Right

                    // Measure all lines once to determine accurate heights for wrapping and position stacking
                    struct MeasuredLine {
                        height: f32,
                        galley: std::sync::Arc<egui::Galley>,
                    }

                    let font_id = egui::FontId::proportional(font_size);
                    let measured_all: Vec<MeasuredLine> = parsed_lines.iter().map(|line| {
                        let galley = ui.painter().layout(
                            line.text.clone(),
                            font_id.clone(),
                            Color32::WHITE,
                            text_max_width,
                        );
                        let height = galley.size().y;
                        MeasuredLine { height, galley }
                    }).collect();

                    // Pre-compute the cumulative center Y positions of all lines relative to index 0:
                    // Y(i+1) = Y(i) + (H(i)/2) + spacing_gap + (H(i+1)/2)
                    let num_lines = parsed_lines.len();
                    let mut cumulative_y = vec![0.0f32; num_lines];
                    for i in 1..num_lines {
                        let prev_h = measured_all[i - 1].height;
                        let cur_h = measured_all[i].height;
                        cumulative_y[i] = cumulative_y[i - 1] + (prev_h * 0.5) + spacing_gap + (cur_h * 0.5);
                    }

                    // For the continuous floating scroll offset `self.lyrics_scroll_offset` (which can be fractional):
                    // calculate the current camera reference Y position by interpolating cumulative_y
                    let target_cam_y = if num_lines == 0 {
                        0.0
                    } else {
                        let clamped_offset = self.lyrics_scroll_offset.clamp(0.0, (num_lines - 1) as f32);
                        let base_i = clamped_offset.floor() as usize;
                        let frac = clamped_offset - (base_i as f32);
                        if base_i + 1 < num_lines {
                            cumulative_y[base_i] * (1.0 - frac) + cumulative_y[base_i + 1] * frac
                        } else {
                            cumulative_y[base_i]
                        }
                    };

                    // Visible range determined around current active / scrolled position
                    let view_line_center = self.lyrics_scroll_offset.round() as isize;
                    let min_idx = (view_line_center - context_k - 2).max(0) as usize;
                    let max_idx = (view_line_center + context_k + 2).min((num_lines as isize) - 1).max(0) as usize;

                    let mut seek_to_ratio = None;

                    if !parsed_lines.is_empty() {
                        for idx in min_idx..=max_idx {
                            let item = &measured_all[idx];
                            let line_center_y = center_y + (cumulative_y[idx] - target_cam_y);

                            // Calculate distance in lines or pixels for smooth fading
                            let line_offset = (idx as f32) - self.lyrics_scroll_offset;
                            let dist = line_offset.abs();
                            let max_reach = context_k as f32 + 0.9;
                            let alpha = (1.0 - (dist / max_reach)).clamp(0.0, 1.0).powf(1.4);
                            if alpha < 0.01 { continue; }

                            let is_active = has_timestamps && (idx == active_line_idx) && self.lyrics_user_offset.abs() < 0.5;
                            let g_w = item.galley.size().x;
                            let g_h = item.height;

                            let text_x = match align_mode {
                                1 => right_rect.min.x + 24.0, // Left
                                2 => right_rect.max.x - 24.0 - g_w, // Right
                                _ => right_rect.center().x - (g_w * 0.5), // Center
                            };

                            let line_rect = Rect::from_min_size(pos2(text_x, line_center_y - (g_h * 0.5)), Vec2::new(g_w.max(60.0), g_h.max(20.0)));
                            let resp = ui.interact(line_rect.expand2(Vec2::new(16.0, spacing_gap * 0.4)), ui.id().with(("lyric_line", idx)), egui::Sense::click());
                            if resp.hovered() {
                                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if resp.clicked() {
                                self.lyrics_user_offset = 0.0;
                                self.lyrics_user_scroll_timer = 0.0;
                                let line = &parsed_lines[idx];
                                if let Some(t) = line.time_secs {
                                    if track_duration > 0.0 {
                                        seek_to_ratio = Some((t / track_duration).clamp(0.0, 1.0));
                                    }
                                } else if !parsed_lines.is_empty() {
                                    seek_to_ratio = Some(((idx as f64) / (parsed_lines.len() as f64)).clamp(0.0, 1.0));
                                }
                            }

                            let final_color = if resp.hovered() {
                                Color32::WHITE
                            } else if is_active && line_offset.abs() < 0.4 {
                                Color32::WHITE
                            } else {
                                Color32::from_white_alpha((alpha * 190.0) as u8)
                            };

                            // Draw line galley with active formatting
                            let draw_galley = if final_color == Color32::WHITE {
                                item.galley.clone()
                            } else {
                                ui.painter().layout(parsed_lines[idx].text.clone(), font_id.clone(), final_color, text_max_width)
                            };

                            ui.painter().galley(pos2(text_x, line_center_y - (g_h * 0.5)), draw_galley, final_color);
                        }
                    }

                    if let Some(ratio) = seek_to_ratio {
                        self.progress = ratio as f32;
                        if let Some(audio) = &self.audio {
                            self.audio_revision = audio.send(AudioCommand::Seek(self.progress));
                        }
                        self.save_current_state();
                    }
                } else if let Some(tex) = &self.cover_texture {
                    ui.painter().image(
                        tex.id(),
                        right_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().rect_filled(right_rect, Rounding::ZERO, colors.card);
                    ui.painter().text(
                        right_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "[ NO ARTWORK ]",
                        egui::FontId::monospace(11.0),
                        colors.muted,
                    );
                }
            });
        // ================= FLOATING OVERLAY PLAYER FOOTER =================
        let control_room = ((ctx.screen_rect().width() - 340.0) / 280.0).clamp(0.0, 1.0);
        let transport_height = 54.0;
        let timeline_peek = 4.0; // Show the top of the timeline when unhovered
        let mut queue_anchor = Rect::NOTHING;
        let screen = ctx.screen_rect();

        let mouse_pos = ctx.input(|i| i.pointer.hover_pos().or_else(|| i.pointer.latest_pos()).or_else(|| i.pointer.interact_pos()));

        // Hysteresis hover detection:
        // When collapsed (unhovered), require hovering near the timeline / bottom edge (within 24px of screen bottom).
        // This gives comfortable breathing room from the track list above, preventing premature popups while browsing the playlist.
        // Once expanded (or expanding), the hit region includes the full transport height (54px) so all controls remain interactive.
        // If a floating control popup (volume, speed, reverb) is currently open, keep the player footer locked open.
        let trigger_margin = if self.hover_transition > 0.15 || self.active_floating_control.is_some() || self.queue_open { transport_height + 4.0 } else { 24.0 };
        let is_bottom_hovered = self.active_floating_control.is_some() || self.queue_open || mouse_pos.map_or(false, |p| {
            p.y >= screen.max.y - trigger_margin && p.y <= screen.max.y && p.x >= screen.min.x && p.x <= screen.max.x
        });

        // Smooth hover transition animation (0.0 to 1.0)
        let target_t = if is_bottom_hovered { 1.0 } else { 0.0 };
        let dt = ctx.input(|i| i.stable_dt).max(1.0 / 60.0).min(0.1);
        self.hover_transition = (self.hover_transition + (target_t - self.hover_transition) * (dt * 15.0).min(1.0)).clamp(0.0, 1.0);

        if self.floating_control_timer > 0.0 {
            self.floating_control_timer -= dt;
            if self.floating_control_timer <= 0.0 {
                self.active_floating_control = None;
            }
        }

        // When hover_transition is 0.0, bar_top_y rests at (screen.max.y - timeline_peek),
        // showing the slim glowing timeline right along the bottom edge of the window.
        // When hovered (transition = 1.0), it smoothly rises up to (screen.max.y - transport_height).
        let current_rise = timeline_peek + (transport_height - timeline_peek) * self.hover_transition;
        let bar_top_y = screen.max.y - current_rise;
        let panel_rect = Rect::from_min_max(pos2(screen.min.x, bar_top_y), pos2(screen.max.x, bar_top_y + transport_height));

        egui::Area::new(egui::Id::new("floating_transport_bar"))
            .fixed_pos(panel_rect.min)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_clip_rect(panel_rect);
                ui.painter().rect_filled(panel_rect, Rounding::ZERO, colors.background);
                // ---------------- YOUTUBE-STYLE INTERACTIVE SEEKBAR ON TOP ----------------
                let bar_thickness = 3.0 + (3.0 * self.hover_transition);
                let hit_height = 16.0;
                let seek_y = panel_rect.min.y + bar_thickness * 0.5;

                let seek_hit_rect = Rect::from_min_size(
                    panel_rect.min,
                    Vec2::new(panel_rect.width(), hit_height),
                );

                let seek_visual_rect = Rect::from_center_size(
                    pos2(panel_rect.center().x, seek_y),
                    Vec2::new(panel_rect.width(), bar_thickness),
                );

                let seek_response = ui.interact(seek_hit_rect, ui.id().with("seekbar"), egui::Sense::click_and_drag());

                // Track background: Solid matching color, no white gap
                ui.painter().rect_filled(seek_visual_rect, Rounding::ZERO, colors.track_bg);

                // Elapsed fill: Solid accent orange
                let filled_width = (panel_rect.width() * self.progress.clamp(0.0, 1.0)).max(0.0);
                let progress_rect = Rect::from_min_size(
                    seek_visual_rect.min,
                    Vec2::new(filled_width, bar_thickness),
                );
                ui.painter().rect_filled(progress_rect, Rounding::ZERO, colors.accent);

                // YouTube-style playhead circle: solid accent orange without white outline
                let head_radius = (5.5 * self.hover_transition).max(0.0);
                if head_radius > 0.5 {
                    let playhead_pos = pos2((seek_visual_rect.min.x + filled_width).clamp(panel_rect.min.x + head_radius, panel_rect.max.x - head_radius), seek_visual_rect.center().y);
                    // Let the thumb overlap the artwork edge without adding a blank
                    // strip above the track or clipping the upper half of the circle.
                    ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("seek_thumb")))
                        .with_clip_rect(ctx.screen_rect()).circle_filled(
                        playhead_pos,
                        head_radius,
                        colors.accent,
                    );
                }

                // Dragging previews locally. Commit exactly one seek at release, and
                // reject old telemetry until the playback worker acknowledges its revision.
                if seek_response.drag_started() { self.is_scrubbing = true; }
                if seek_response.clicked() || seek_response.dragged() || seek_response.drag_stopped() {
                    if let Some(pt) = seek_response.interact_pointer_pos() {
                        self.progress = ((pt.x - seek_visual_rect.min.x) / seek_visual_rect.width()).clamp(0.0, 1.0);
                    }
                }
                if seek_response.drag_stopped() || seek_response.clicked() {
                    self.is_scrubbing = false;
                    if let Some(audio) = &self.audio {
                        self.audio_revision = audio.send(AudioCommand::Seek(self.progress));
                    }
                    self.save_current_state();
                }

                // Strictly center all controls vertically between seekbar and bottom edge
                let content_alpha = ((self.hover_transition - 0.2) / 0.8).clamp(0.0, 1.0);
                let slide_offset = (1.0 - self.hover_transition) * 48.0;
                let center_y = panel_rect.min.y + 32.0 + slide_offset;
                let btn_size = 22.0;

                // When unhovered, the controls smoothly slide down into hiding, showing just the timeline.
                // We keep controls interactive only when visible enough.
                if content_alpha > 0.05 {

                // --- 1. LEFT ZONE: [🔀] [⚙] (uniform 12px gap, aligned to exact center_y) ---
                let left_margin = 8.0 + 6.0 * control_room;
                let left_gap = 8.0 + 4.0 * control_room;

                let shuf_rect = Rect::from_center_size(pos2(panel_rect.min.x + left_margin + (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let shuf_resp = ui.allocate_rect(shuf_rect, egui::Sense::click());
                let shuf_color = if self.shuffle_enabled { colors.accent } else if shuf_resp.hovered() { colors.text } else { colors.muted };
                
                // Pixel-perfect vector shuffle icon
                {
                    let p = ui.painter();
                    let stroke = Stroke::new(1.6, shuf_color);
                    let c = shuf_rect.center();
                    // Top crossover arrow: (-6, -4) -> (-1, -4) -> (2, 4) -> (6, 4)
                    p.line_segment([pos2(c.x - 6.0, c.y - 3.5), pos2(c.x - 2.0, c.y - 3.5)], stroke);
                    p.line_segment([pos2(c.x - 2.0, c.y - 3.5), pos2(c.x + 2.0, c.y + 3.5)], stroke);
                    p.line_segment([pos2(c.x + 2.0, c.y + 3.5), pos2(c.x + 6.0, c.y + 3.5)], stroke);
                    // Arrowhead bottom:
                    p.line_segment([pos2(c.x + 3.5, c.y + 1.0), pos2(c.x + 6.0, c.y + 3.5)], stroke);
                    p.line_segment([pos2(c.x + 3.5, c.y + 6.0), pos2(c.x + 6.0, c.y + 3.5)], stroke);

                    // Bottom crossover arrow: (-6, 4) -> (-1, 4) -> (2, -4) -> (6, -4)
                    p.line_segment([pos2(c.x - 6.0, c.y + 3.5), pos2(c.x - 2.0, c.y + 3.5)], stroke);
                    p.line_segment([pos2(c.x - 2.0, c.y + 3.5), pos2(c.x + 2.0, c.y - 3.5)], stroke);
                    p.line_segment([pos2(c.x + 2.0, c.y - 3.5), pos2(c.x + 6.0, c.y - 3.5)], stroke);
                    // Arrowhead top:
                    p.line_segment([pos2(c.x + 3.5, c.y - 6.0), pos2(c.x + 6.0, c.y - 3.5)], stroke);
                    p.line_segment([pos2(c.x + 3.5, c.y - 1.0), pos2(c.x + 6.0, c.y - 3.5)], stroke);
                }

                if shuf_resp.clicked() {
                    self.shuffle_enabled = !self.shuffle_enabled;
                    self.rebuild_shuffle_order(self.selected_idx);
                    self.save_current_state();
                }

                let set_rect = Rect::from_center_size(pos2(shuf_rect.max.x + left_gap + (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let set_resp = ui.allocate_rect(set_rect, egui::Sense::click());
                let set_color = if set_resp.hovered() { colors.text } else { colors.muted };
                
                for offset in [-5.0, 0.0, 5.0] {
                    let c = set_rect.center();
                    ui.painter().line_segment([pos2(c.x - 5.0, c.y + offset), pos2(c.x + 6.0, c.y + offset)], Stroke::new(1.5, set_color));
                    ui.painter().circle_filled(pos2(c.x - 8.0, c.y + offset), 1.0, set_color);
                }
                queue_anchor = set_rect;
                if set_resp.clicked() {
                    self.queue_open = !self.queue_open;
                    self.queue_drag = None;
                    if self.queue_open { self.speed_open = false; self.reverb_open = false; }
                }
                set_resp.on_hover_text("Queue: view the current mix");

                // Toggle for Lyrics beside Queue
                let lyric_rect = Rect::from_center_size(pos2(set_rect.max.x + left_gap + (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let lyric_resp = ui.allocate_rect(lyric_rect, egui::Sense::click());
                let has_lyrics = self.current_meta.as_ref().map_or(false, |m| m.lyrics.is_some());
                let lyric_color = if self.lyrics_open {
                    colors.accent
                } else if lyric_resp.hovered() {
                    colors.text
                } else if has_lyrics {
                    colors.text
                } else {
                    colors.muted
                };

                // Lyrics icon: text quote / note icon
                {
                    let p = ui.painter();
                    let c = lyric_rect.center();
                    // Draw a neat speech bubble / lyrics symbol
                    p.rect_stroke(Rect::from_center_size(pos2(c.x, c.y - 1.0), Vec2::new(14.0, 11.0)), Rounding::same(2.0), Stroke::new(1.4, lyric_color));
                    p.line_segment([pos2(c.x - 3.5, c.y - 2.5), pos2(c.x + 3.5, c.y - 2.5)], Stroke::new(1.2, lyric_color));
                    p.line_segment([pos2(c.x - 3.5, c.y + 0.5), pos2(c.x + 2.0, c.y + 0.5)], Stroke::new(1.2, lyric_color));
                }
                if lyric_resp.clicked() {
                    self.lyrics_open = !self.lyrics_open;
                }
                lyric_resp.on_hover_text(if has_lyrics { "Toggle lyrics" } else { "No lyrics available in track" });

                // --- 2. EXACT CENTER ZONE: [⏮] [▶/⏸] [⏭] (uniform 14px gap, aligned to exact center_y) ---
                let center_x = panel_rect.center().x;
                let center_gap = 8.0 + 6.0 * control_room;

                let play_rect = Rect::from_center_size(pos2(center_x, center_y), Vec2::splat(btn_size));
                let prev_rect = Rect::from_center_size(pos2(play_rect.min.x - center_gap - (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let next_rect = Rect::from_center_size(pos2(play_rect.max.x + center_gap + (btn_size * 0.5), center_y), Vec2::splat(btn_size));

                // Previous
                let prev_resp = ui.allocate_rect(prev_rect, egui::Sense::click());
                let prev_color = if prev_resp.is_pointer_button_down_on() { colors.accent } else if prev_resp.hovered() { colors.text } else { colors.text };
                {
                    let p = ui.painter();
                    let c = prev_rect.center();
                    // Left bar:
                    p.rect_filled(Rect::from_center_size(pos2(c.x - 5.0, c.y), Vec2::new(2.0, 11.0)), Rounding::ZERO, prev_color);
                    // Left triangle:
                    let p1 = pos2(c.x - 3.5, c.y);
                    let p2 = pos2(c.x + 5.0, c.y - 5.5);
                    let p3 = pos2(c.x + 5.0, c.y + 5.5);
                    p.add(egui::Shape::convex_polygon(vec![p1, p2, p3], prev_color, Stroke::NONE));
                }
                if prev_resp.clicked() {
                    self.prev_track();
                }

                // Play / Pause
                let play_resp = ui.allocate_rect(play_rect, egui::Sense::click());
                let play_color = if play_resp.is_pointer_button_down_on() { colors.accent } else if play_resp.hovered() { colors.text } else { colors.text };
                {
                    let p = ui.painter();
                    let c = play_rect.center();
                    if self.is_playing {
                        // Pause: two solid bars
                        p.rect_filled(Rect::from_center_size(pos2(c.x - 3.0, c.y), Vec2::new(3.0, 12.0)), Rounding::ZERO, play_color);
                        p.rect_filled(Rect::from_center_size(pos2(c.x + 3.0, c.y), Vec2::new(3.0, 12.0)), Rounding::ZERO, play_color);
                    } else {
                        // Play: right-pointing triangle centered mathematically
                        let p1 = pos2(c.x - 4.5, c.y - 6.5);
                        let p2 = pos2(c.x - 4.5, c.y + 6.5);
                        let p3 = pos2(c.x + 5.5, c.y);
                        p.add(egui::Shape::convex_polygon(vec![p1, p2, p3], play_color, Stroke::NONE));
                    }
                }
                if play_resp.clicked() {
                    if let Some(audio) = &self.audio {
                        audio.send(AudioCommand::Toggle);
                    }
                }

                // Next
                let next_resp = ui.allocate_rect(next_rect, egui::Sense::click());
                let next_color = if next_resp.is_pointer_button_down_on() { colors.accent } else if next_resp.hovered() { colors.text } else { colors.text };
                {
                    let p = ui.painter();
                    let c = next_rect.center();
                    // Right-pointing triangle:
                    let p1 = pos2(c.x - 5.0, c.y - 5.5);
                    let p2 = pos2(c.x - 5.0, c.y + 5.5);
                    let p3 = pos2(c.x + 3.5, c.y);
                    p.add(egui::Shape::convex_polygon(vec![p1, p2, p3], next_color, Stroke::NONE));
                    // Right bar:
                    p.rect_filled(Rect::from_center_size(pos2(c.x + 5.0, c.y), Vec2::new(2.0, 11.0)), Rounding::ZERO, next_color);
                }
                if next_resp.clicked() {
                    self.next_track();
                }

                // --- 3. RIGHT ZONE: [Volume (Icon)] [Speed (1x)] [Reverb (~)] (uniform 12px gap, aligned to exact center_y) ---
                let right_margin = 8.0 + 6.0 * control_room;
                let right_gap = 8.0 + 6.0 * control_room;

                // Helper closure for non-linear dynamic scroll scaling:
                // Standard Windows wheel delta is ~50.0 to 120.0 per physical tick.
                // Single slow tick (norm ~ 1.0): moves precisely 0.01 (1% volume/reverb, 0.01x speed).
                // Fast spin (norm ~ 3.0 to 5.0+): smoothly accelerates up to 5x faster (0.05 / 5% per tick).
                let dynamic_scroll_step = |raw_delta: f32, base_rate: f32| -> f32 {
                    let mag = raw_delta.abs();
                    let norm = (mag / 50.0).max(1.0);
                    let factor = (norm * norm * 0.5 + 0.5).clamp(1.0, 5.0);
                    raw_delta.signum() * base_rate * factor
                };

                // Reverb Icon (Minimalist "~" wave icon)
                let rev_rect = Rect::from_center_size(pos2(panel_rect.max.x - right_margin - (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let rev_resp = ui.allocate_rect(rev_rect, egui::Sense::click());
                let rev_color = if self.active_floating_control == Some(FloatingControlType::Reverb) || self.reverb_amount > 0.01 { colors.accent } else if rev_resp.hovered() { colors.text } else { colors.muted };
                ui.painter().text(rev_rect.center(), egui::Align2::CENTER_CENTER, "~", egui::FontId::monospace(15.0), rev_color);
                if rev_resp.clicked() {
                    if self.active_floating_control == Some(FloatingControlType::Reverb) {
                        self.active_floating_control = None;
                    } else {
                        self.active_floating_control = Some(FloatingControlType::Reverb);
                        self.floating_control_timer = 0.0;
                    }
                }
                if rev_resp.hovered() {
                    let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
                    if scroll_delta.abs() > 0.01 {
                        let step = dynamic_scroll_step(scroll_delta, 0.01);
                        self.reverb_amount = (self.reverb_amount + step).clamp(0.0, 1.0);
                        if let Some(audio) = &self.audio {
                            audio.send(AudioCommand::SetReverb(self.reverb_amount));
                        }
                        self.save_current_state();
                        self.active_floating_control = Some(FloatingControlType::Reverb);
                        self.floating_control_timer = 1.6;
                    }
                }

                // Speed Icon (Dynamic speed text based on active playback speed)
                let spd_rect = Rect::from_center_size(pos2(rev_rect.min.x - right_gap - (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let spd_resp = ui.allocate_rect(spd_rect, egui::Sense::click());
                let spd_color = if self.active_floating_control == Some(FloatingControlType::Speed) || (self.speed - 1.0).abs() > 0.01 { colors.accent } else if spd_resp.hovered() { colors.text } else { colors.muted };
                let spd_text = if (self.speed - 1.0).abs() < 0.01 {
                    "1x".to_string()
                } else if ((self.speed * 10.0).round() - (self.speed * 10.0)).abs() < 0.01 {
                    format!("{:.1}x", self.speed)
                } else {
                    format!("{:.2}x", self.speed)
                };
                ui.painter().text(spd_rect.center(), egui::Align2::CENTER_CENTER, spd_text, egui::FontId::monospace(10.5), spd_color);
                if spd_resp.clicked() {
                    if self.active_floating_control == Some(FloatingControlType::Speed) {
                        self.active_floating_control = None;
                    } else {
                        self.active_floating_control = Some(FloatingControlType::Speed);
                        self.floating_control_timer = 0.0;
                    }
                }
                if spd_resp.hovered() {
                    let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
                    if scroll_delta.abs() > 0.01 {
                        let step = dynamic_scroll_step(scroll_delta, 0.01);
                        self.speed = (self.speed + step).clamp(0.5, 1.5);
                        if let Some(audio) = &self.audio {
                            audio.send(AudioCommand::SetSpeed(self.speed));
                        }
                        self.save_current_state();
                        self.active_floating_control = Some(FloatingControlType::Speed);
                        self.floating_control_timer = 1.6;
                    }
                }

                // Volume Icon (Crisp vector speaker icon)
                let vol_icon_rect = Rect::from_center_size(pos2(spd_rect.min.x - right_gap - (btn_size * 0.5), center_y), Vec2::splat(btn_size));
                let vol_icon_resp = ui.allocate_rect(vol_icon_rect, egui::Sense::click());
                let vol_color = if vol_icon_resp.hovered() || self.active_floating_control == Some(FloatingControlType::Volume) {
                    colors.accent
                } else if self.volume > 0.001 {
                    colors.text
                } else {
                    colors.muted
                };

                // Draw speaker icon
                {
                    let p = ui.painter();
                    let c = vol_icon_rect.center();
                    // Speaker body
                    let body_rect = Rect::from_center_size(pos2(c.x - 3.5, c.y), Vec2::new(3.0, 5.0));
                    p.rect_filled(body_rect, Rounding::ZERO, vol_color);
                    // Cone polygon
                    let cone = vec![
                        pos2(c.x - 2.0, c.y - 2.5),
                        pos2(c.x + 2.0, c.y - 6.0),
                        pos2(c.x + 2.0, c.y + 6.0),
                        pos2(c.x - 2.0, c.y + 2.5),
                    ];
                    p.add(egui::Shape::convex_polygon(cone, vol_color, Stroke::NONE));

                    if self.volume <= 0.001 {
                        // Muted 'x' mark
                        p.line_segment([pos2(c.x + 4.0, c.y - 3.0), pos2(c.x + 7.5, c.y + 3.0)], Stroke::new(1.3, vol_color));
                        p.line_segment([pos2(c.x + 7.5, c.y - 3.0), pos2(c.x + 4.0, c.y + 3.0)], Stroke::new(1.3, vol_color));
                    } else {
                        // Sound wave arc
                        p.line_segment([pos2(c.x + 4.5, c.y - 3.0), pos2(c.x + 6.0, c.y)], Stroke::new(1.3, vol_color));
                        p.line_segment([pos2(c.x + 6.0, c.y), pos2(c.x + 4.5, c.y + 3.0)], Stroke::new(1.3, vol_color));
                        if self.volume > 0.5 {
                            p.line_segment([pos2(c.x + 7.0, c.y - 5.0), pos2(c.x + 9.0, c.y)], Stroke::new(1.3, vol_color));
                            p.line_segment([pos2(c.x + 9.0, c.y), pos2(c.x + 7.0, c.y + 5.0)], Stroke::new(1.3, vol_color));
                        }
                    }
                }

                if vol_icon_resp.clicked() {
                    // Click toggles floating preview (timer = 0 so it stays open until toggled or dismissed)
                    if self.active_floating_control == Some(FloatingControlType::Volume) {
                        self.active_floating_control = None;
                    } else {
                        self.active_floating_control = Some(FloatingControlType::Volume);
                        self.floating_control_timer = 0.0;
                    }
                }

                // Scroll wheel on volume icon (dynamic acceleration: fine when slow, fast when spun)
                if vol_icon_resp.hovered() {
                    let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
                    if scroll_delta.abs() > 0.01 {
                        let step = dynamic_scroll_step(scroll_delta, 0.01);
                        self.volume = (self.volume + step).clamp(0.0, 1.0);
                        if let Some(audio) = &self.audio {
                            audio.send(AudioCommand::SetVolume(self.volume));
                        }
                        self.save_current_state();
                        self.active_floating_control = Some(FloatingControlType::Volume);
                        self.floating_control_timer = 1.6;
                    }
                }

                // --- VERTICAL FLOATING PREVIEW SLIDER POPUP WITH VALUE BELOW ---
                if let Some(ctrl_type) = self.active_floating_control {
                    let anchor_rect = match ctrl_type {
                        FloatingControlType::Volume => vol_icon_rect,
                        FloatingControlType::Speed => spd_rect,
                        FloatingControlType::Reverb => rev_rect,
                    };
                    let title = match ctrl_type {
                        FloatingControlType::Volume => "VOLUME",
                        FloatingControlType::Speed => "SPEED",
                        FloatingControlType::Reverb => "REVERB",
                    };
                    let popup_w = 46.0;
                    let popup_h = 138.0;
                    let popup_x = (anchor_rect.center().x - popup_w * 0.5).clamp(8.0, panel_rect.max.x - popup_w - 8.0);
                    let popup_pos = pos2(popup_x, panel_rect.min.y - popup_h - 10.0);

                    let popup_resp = egui::Area::new(egui::Id::new("floating_vertical_slider_preview"))
                        .fixed_pos(popup_pos)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            egui::Frame::popup(ui.style())
                                .fill(colors.card)
                                .stroke(Stroke::new(1.0, colors.border))
                                .rounding(Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(6.0, 8.0))
                                .show(ui, |ui| {
                                    ui.set_width(popup_w - 12.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(egui::RichText::new(title).size(8.5).strong().color(colors.muted));
                                        ui.add_space(6.0);

                                        // Vertical slider track: height 72.0, width 14.0 hit box
                                        let track_len = 72.0;
                                        let (s_rect, s_resp) = ui.allocate_exact_size(Vec2::new(16.0, track_len), egui::Sense::click_and_drag());

                                        let current_frac = match ctrl_type {
                                            FloatingControlType::Volume => self.volume.clamp(0.0, 1.0),
                                            FloatingControlType::Speed => ((self.speed - 0.5) / 1.0).clamp(0.0, 1.0),
                                            FloatingControlType::Reverb => self.reverb_amount.clamp(0.0, 1.0),
                                        };

                                        // Drag interaction (bottom is 0.0, top is 1.0)
                                        if s_resp.clicked() || s_resp.dragged() {
                                            self.floating_control_timer = 2.0; // Refresh dismissal timer while interacting
                                            if let Some(pt) = s_resp.interact_pointer_pos() {
                                                let new_frac = (1.0 - (pt.y - s_rect.min.y) / track_len).clamp(0.0, 1.0);
                                                match ctrl_type {
                                                    FloatingControlType::Volume => {
                                                        self.volume = new_frac;
                                                        if let Some(audio) = &self.audio { audio.send(AudioCommand::SetVolume(self.volume)); }
                                                    }
                                                    FloatingControlType::Speed => {
                                                        self.speed = 0.5 + (new_frac * 1.0);
                                                        if let Some(audio) = &self.audio { audio.send(AudioCommand::SetSpeed(self.speed)); }
                                                    }
                                                    FloatingControlType::Reverb => {
                                                        self.reverb_amount = new_frac;
                                                        if let Some(audio) = &self.audio { audio.send(AudioCommand::SetReverb(self.reverb_amount)); }
                                                    }
                                                }
                                                self.save_current_state();
                                            }
                                        }

                                        // Scroll wheel inside popup
                                        if s_resp.hovered() {
                                            let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
                                            if scroll_delta.abs() > 0.01 {
                                                let step = dynamic_scroll_step(scroll_delta, 0.01);
                                                match ctrl_type {
                                                    FloatingControlType::Volume => {
                                                        self.volume = (self.volume + step).clamp(0.0, 1.0);
                                                        if let Some(audio) = &self.audio { audio.send(AudioCommand::SetVolume(self.volume)); }
                                                    }
                                                    FloatingControlType::Speed => {
                                                        self.speed = (self.speed + step).clamp(0.5, 1.5);
                                                        if let Some(audio) = &self.audio { audio.send(AudioCommand::SetSpeed(self.speed)); }
                                                    }
                                                    FloatingControlType::Reverb => {
                                                        self.reverb_amount = (self.reverb_amount + step).clamp(0.0, 1.0);
                                                        if let Some(audio) = &self.audio { audio.send(AudioCommand::SetReverb(self.reverb_amount)); }
                                                    }
                                                }
                                                self.save_current_state();
                                            }
                                        }

                                        // Render vertical track background
                                        let bar_w = 3.0;
                                        let bar_center_x = s_rect.center().x;
                                        let track_rect = Rect::from_center_size(pos2(bar_center_x, s_rect.center().y), Vec2::new(bar_w, track_len));
                                        ui.painter().rect_filled(track_rect, Rounding::same(1.5), colors.track_bg);

                                        // Render active fill from bottom up
                                        let fill_h = track_len * current_frac;
                                        let fill_rect = Rect::from_min_max(
                                            pos2(bar_center_x - bar_w * 0.5, track_rect.max.y - fill_h),
                                            pos2(bar_center_x + bar_w * 0.5, track_rect.max.y),
                                        );
                                        ui.painter().rect_filled(fill_rect, Rounding::same(1.5), colors.accent);

                                        // Thumb circle
                                        let thumb_y = track_rect.max.y - fill_h;
                                        ui.painter().circle_filled(pos2(bar_center_x, thumb_y), 4.5, colors.accent);

                                        ui.add_space(6.0);

                                        // Value display below the vertical slider
                                        let val_text = match ctrl_type {
                                            FloatingControlType::Volume => format!("{:.0}%", self.volume * 100.0),
                                            FloatingControlType::Speed => format!("{:.2}x", self.speed),
                                            FloatingControlType::Reverb => format!("{:.0}%", self.reverb_amount * 100.0),
                                        };
                                        ui.label(egui::RichText::new(val_text).size(9.5).color(colors.text));
                                    });
                                });
                        });

                    if popup_resp.response.hovered() {
                        self.floating_control_timer = 2.0; // Keep alive while mouse inside popup
                    }
                    if dismiss_popup(ctx, popup_resp.response.rect, anchor_rect) {
                        self.active_floating_control = None;
                    }
                }

                } // end if content_alpha > 0.05

            });

        if let Some(message) = self.audio_error.clone() {
            egui::Window::new("Audio playback error").collapsible(false).show(ctx, |ui| {
                ui.label(message);
                if ui.button("Dismiss").clicked() { self.audio_error = None; }
            });
        }

        if self.queue_open {
            let rows: Vec<_> = self.shuffle_order.iter().enumerate().filter_map(|(position, &index)|
                self.playlist.get(index).map(|t| (position, index, t.title.clone(), t.artist.clone()))).collect();
            let mut jump = None;
            let mut promote = None;
            let mut insertion = None;
            let mut queue_context_rect = Rect::NOTHING;
            let popup_width = 320.0f32.min(ctx.screen_rect().width() - 32.0);
            let list_height = (rows.len() as f32 * 30.0).max(28.0).min((ctx.screen_rect().height() * 0.5).min(320.0));
            let popup = egui::Area::new(egui::Id::new("queue_popup"))
                .movable(false)
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(8.0, ctx.screen_rect().max.y - transport_height - 8.0 - list_height - 36.0)).constrain(true)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).fill(colors.card)
                        .stroke(Stroke::new(1.0, colors.border)).rounding(Rounding::same(4.0))
                        .inner_margin(egui::Margin::same(8.0)).show(ui, |ui| {
                            ui.set_width(popup_width);
                            ui.label(egui::RichText::new("QUEUE").size(9.0).strong().color(colors.muted));
                            if rows.is_empty() { ui.label("Queue is empty"); }
                            egui::ScrollArea::vertical().id_source("queue_tracks")
                                .drag_to_scroll(false)
                                .max_height(list_height).min_scrolled_height(list_height)
                                .auto_shrink([false, false]).show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    for (position, index, title, artist) in rows {
                                        let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), egui::Sense::hover());
                                        let handle = Rect::from_min_size(row.min, Vec2::new(22.0, row.height()));
                                        let response = ui.interact(handle, ui.id().with(("queue_drag", index)), egui::Sense::drag());
                                        if response.drag_started() { self.queue_drag = Some(index); }
                                        if response.hovered() { ctx.set_cursor_icon(egui::CursorIcon::Grab); }
                                        let active = self.queue_drag == Some(index);
                                        if active { ui.painter().rect_stroke(row.shrink(1.0), Rounding::same(2.0), Stroke::new(1.0, colors.accent)); }
                                        let handle_color = if active || response.hovered() { colors.accent } else { colors.muted };
                                        for x in [-2.5, 2.5] { for y in [-5.0, 0.0, 5.0] {
                                            ui.painter().circle_filled(handle.center() + Vec2::new(x, y), 1.2, handle_color);
                                        }}
                                        let text_rect = Rect::from_min_max(pos2(handle.max.x + 4.0, row.min.y), row.max);
                                        let marker = if self.selected_idx == Some(index) { ">" } else if self.user_queue.contains(&index) { "+" } else { "" };
                                        let label = ui.allocate_ui_at_rect(text_rect, |ui| {
                                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                                ui.add(egui::Label::new(format!("{} {}. {} - {}", marker, position + 1, title, artist)).truncate(true).sense(egui::Sense::click()))
                                            }).inner
                                        }).inner;
                                        if label.double_clicked() { jump = Some((position, index)); }
                                        if let Some(menu) = label.context_menu(|ui| {
                                            if ui.button("Play next").clicked() { promote = Some(index); ui.close_menu(); }
                                        }) { queue_context_rect = menu.response.rect; }
                                        if self.queue_drag.is_some() {
                                            if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
                                                if row.contains(pointer) && ui.clip_rect().contains(pointer) {
                                                    let after = pointer.y > row.center().y;
                                                    insertion = Some(position + usize::from(after));
                                                    let y = if after { row.max.y } else { row.min.y };
                                                    ui.painter().line_segment([pos2(row.min.x, y), pos2(row.max.x, y)], Stroke::new(2.0, colors.accent));
                                                }
                                            }
                                        }
                                    }
                                    if self.queue_drag.is_some() {
                                        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                                        ctx.request_repaint_after(std::time::Duration::from_millis(16));
                                        if let Some(pointer) = ctx.input(|i| i.pointer.interact_pos()) {
                                            let viewport = ui.clip_rect();
                                            if viewport.contains(pointer) {
                                                let delta = if pointer.y < viewport.top() + 24.0 { 6.0 } else if pointer.y > viewport.bottom() - 24.0 { -6.0 } else { 0.0 };
                                                ui.scroll_with_delta(Vec2::new(0.0, delta));
                                            }
                                        }
                                    }
                                });
                        });
                });
            if ctx.input(|i| i.pointer.any_released()) {
                if let (Some(index), Some(position)) = (self.queue_drag.take(), insertion) { self.move_queue_track(index, position); }
            }
            if dismiss_popup(ctx, popup.response.rect.union(queue_context_rect), queue_anchor) {
                self.queue_open = false;
                self.queue_drag = None;
            }
            if let Some(index) = promote { self.play_next(index); }
            if let Some((position, index)) = jump {
                self.shuffle_pos = position;
                self.play_track_at_index(index, false);
            }
        } else { self.queue_drag = None; }

        if self.settings_open {
            ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("deck_preferences"),
                egui::ViewportBuilder::default().with_title("DECK Preferences")
                    .with_inner_size([560.0, 360.0]).with_min_inner_size([440.0, 280.0])
                    .with_resizable(true), |ctx, class| {
                    if class == egui::ViewportClass::Embedded { return; }
                    self.preferences_ui(ctx);
                    if ctx.input(|i| i.viewport().close_requested()) { self.settings_open = false; }
                });
        }
    }
}

/// A square anchored at the top right naturally covers the landscape list as
/// width falls; once height exceeds width, the list is revealed below it.
fn content_rects(available: Rect) -> (Rect, Rect) {
    let side = available.width().min(available.height()).max(0.0);
    let art = Rect::from_min_size(pos2(available.max.x - side, available.min.y), Vec2::splat(side));
    let list = if available.width() > available.height() {
        Rect::from_min_max(available.min, pos2(art.min.x, available.max.y))
    } else {
        Rect::from_min_max(pos2(available.min.x, art.max.y), available.max)
    };
    (list, art)
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 480.0])
            .with_min_inner_size([340.0, 340.0])
            .with_title("DECK"),
        ..Default::default()
    };

    eframe::run_native(
        "DECK",
        options,
        Box::new(|_cc| Box::new(DeckApp::default())),
    )
}
#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn preferences_categories_select_separate_right_hand_settings() {
        let ctx = egui::Context::default();
        let mut app = DeckApp::from_saved(SavedConfig::default(), None);
        for section in [0, 1, 2] {
            app.settings_section = section;
            let output = ctx.run(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(560.0, 360.0))),
                ..Default::default()
            }, |ctx| app.preferences_ui(ctx));
            let text_positions: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some((text.galley.text(), text.pos.x)),
                _ => None,
            }).collect();
            for category in ["Library", "Appearance", "Lyrics"] {
                assert!(text_positions.iter().any(|(text, x)| *text == category && *x < 136.0));
            }
            let settings = if section == 0 {
                vec!["Music folder", "Choose folder", "Rescan"]
            } else if section == 1 {
                vec!["Theme", "Accent color"]
            } else {
                vec!["Font size", "Line spacing", "Context lines (before / after)", "Text alignment"]
            };
            for setting in settings {
                assert!(text_positions.iter().any(|(text, x)| *text.to_lowercase() == setting.to_lowercase() && *x >= 136.0), "missing setting {setting}");
            }
            let absent = if section == 0 { "Accent color" } else { "Music folder" };
            assert!(!text_positions.iter().any(|(text, _)| *text == absent));
        }
    }

    #[test]
    fn queue_popup_drag_reorders_without_changing_playing_track_and_closes_outside() {
        let ctx = egui::Context::default();
        let mut app = mix_fixture();
        app.queue_open = true;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(720.0, 640.0));
        let frame = |app: &mut DeckApp, events: Vec<egui::Event>| {
            ctx.run(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ctx| app.update_ui(ctx)).shapes
        };
        let mut shapes = Vec::new();
        for _ in 0..4 { shapes = frame(&mut app, vec![]); }
        let row_starts: Vec<_> = shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().contains(". Track") => Some(text.pos.x),
            _ => None,
        }).collect();
        assert_eq!(row_starts.len(), 6);
        assert!(row_starts.iter().all(|x| (x - row_starts[0]).abs() < 0.1), "queue rows must align left");
        let row_y = |label: &str| shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().contains(label) => Some(text.pos.y + text.galley.size().y * 0.5),
            _ => None,
        }).expect("queue row missing");
        let handle_x = shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Circle(circle) if (circle.radius - 1.2).abs() < 0.01 => Some(circle.center.x + 2.5),
            _ => None,
        }).expect("drag handle missing");
        assert!(!shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text)
            if text.galley.text().contains("Next and Previous") || text.galley.text().contains("Double-click a library"))));
        let from = pos2(handle_x, row_y("1. Track 4"));
        let to = pos2(handle_x, row_y("3. Track 6") + 8.0);
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&mut app, vec![egui::Event::PointerMoved(from), press(from, true)]);
        frame(&mut app, vec![egui::Event::PointerMoved(to)]);
        assert_eq!(app.queue_drag, Some(2));
        frame(&mut app, vec![press(to, false)]);
        assert_eq!(app.shuffle_order, [5, 0, 2, 4, 1, 3]);
        assert_eq!(app.selected_idx, Some(2));
        assert_eq!(app.shuffle_pos, 2);
        assert!(app.queue_open);
        frame(&mut app, vec![press(pos2(700.0, 10.0), true)]);
        assert!(!app.queue_open);
        assert!(app.queue_drag.is_none());
    }

    #[test]
    fn library_arrange_opens_beside_parent_with_preferences_last() {
        for (width, height) in [(340.0, 640.0), (420.0, 800.0), (720.0, 480.0)] {
            let ctx = egui::Context::default();
            let mut app = DeckApp::from_saved(SavedConfig::default(), None);
            app.library_menu_open = true;
            app.library_arrange_open = true;
            let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, height));
            let mut shapes = Vec::new();
            for _ in 0..4 {
                shapes = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| app.update_ui(ctx)).shapes;
            }
            let labels = ["Arrange library  >", "Rescan library", "Preferences"];
            let mut previous_bottom = 0.0;
            for label in labels {
                let (text, clip) = shapes.iter().find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == label => Some((text, shape.clip_rect)),
                    _ => None,
                }).unwrap_or_else(|| panic!("missing menu entry: {label}"));
                let bounds = Rect::from_min_size(text.pos, text.galley.size());
                assert!(bounds.top() >= previous_bottom, "menu rows overlap: {label}");
                assert!(screen.contains_rect(bounds), "menu outside window: {label}");
                assert!(clip.contains_rect(bounds), "menu entry clipped: {label}");
                previous_bottom = bounds.bottom();
            }
            let label_bounds = |label: &str| shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => Some(Rect::from_min_size(text.pos, text.galley.size())),
                _ => None,
            }).unwrap();
            let parent = label_bounds("Arrange library  >").union(label_bounds("Preferences"));
            let child = label_bounds("Title").union(label_bounds("Duration"));
            assert!(!parent.intersects(child), "submenu covers parent");
            assert!(child.right() < parent.left() || child.left() > parent.right());
            assert!(screen.contains_rect(child));
        }
    }

    fn mix_fixture() -> DeckApp {
        let mut saved = SavedConfig::default();
        saved.playlist = (0..6).map(|i| PlaylistItem { id: i, title: format!("Track {}", 6-i),
            artist: format!("Artist {i}"), duration_str: format!("0{}:00", i+1), path: PathBuf::from(format!("track-{i}.wav")) }).collect();
        let mut app = DeckApp::from_saved(saved, None);
        app.shuffle_enabled = true;
        app.shuffle_order = vec![2, 5, 0, 4, 1, 3];
        app.selected_idx = Some(2);
        app.shuffle_pos = 0;
        app
    }

    #[test]
    fn next_previous_walk_the_same_mix_without_two_track_toggling() {
        let mut app = mix_fixture();
        let original = app.shuffle_order.clone();
        for expected in [5, 0, 4] {
            let index = app.advance_order(true).unwrap();
            assert_eq!(index, expected);
            app.selected_idx = Some(index);
        }
        for expected in [0, 5, 2] {
            let index = app.advance_order(false).unwrap();
            assert_eq!(index, expected);
            app.selected_idx = Some(index);
        }
        assert_eq!(app.shuffle_order, original);
        app.highlighted_idx = Some(3);
        assert_eq!(app.advance_order(true), Some(5), "selecting a row must not change the playing track");
    }

    #[test]
    fn queue_moves_tracks_without_duplicates_and_previous_retraces_them() {
        let mut app = mix_fixture();
        app.play_next(1);
        assert_eq!(app.shuffle_order, [2, 1, 5, 0, 4, 3]);
        app.play_next(1);
        assert_eq!(app.shuffle_order.len(), 6);
        app.selected_idx = app.advance_order(true);
        assert_eq!(app.selected_idx, Some(1));
        app.selected_idx = app.advance_order(false);
        assert_eq!(app.selected_idx, Some(2));
        app.selected_idx = app.advance_order(true);
        assert_eq!(app.selected_idx, Some(1));
        app.play_next(2); // Move a previously played track to next.
        assert_eq!(app.shuffle_order[app.shuffle_pos], 1);
        assert_eq!(app.advance_order(true), Some(2));
    }

    #[test]
    fn remix_starts_with_chosen_track_and_library_sort_preserves_mix() {
        let mut app = mix_fixture();
        app.selected_idx = Some(4);
        app.rebuild_shuffle_order(Some(4));
        assert_eq!(app.shuffle_pos, 0);
        assert_eq!(app.shuffle_order[0], 4);
        let mut unique = app.shuffle_order.clone(); unique.sort_unstable();
        assert_eq!(unique, (0..6).collect::<Vec<_>>());
        let paths: Vec<_> = app.shuffle_order.iter().map(|&i| app.playlist[i].path.clone()).collect();
        app.arrange_library(1);
        assert_eq!(paths, app.shuffle_order.iter().map(|&i| app.playlist[i].path.clone()).collect::<Vec<_>>());
        assert_eq!(app.playlist[app.selected_idx.unwrap()].path, PathBuf::from("track-4.wav"));
    }

    #[test]
    fn effect_popups_close_on_outside_press_and_escape() {
        let ctx = egui::Context::default();
        let popup = Rect::from_min_max(pos2(10.0, 10.0), pos2(100.0, 100.0));
        let anchor = Rect::from_min_max(pos2(10.0, 110.0), pos2(40.0, 140.0));
        for (point, expected) in [(pos2(50.0, 50.0), false), (pos2(20.0, 120.0), false), (pos2(200.0, 200.0), true)] {
            let _ = ctx.run(egui::RawInput { events: vec![egui::Event::PointerButton {
                pos: point, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default(),
            }], ..Default::default() }, |ctx| assert_eq!(dismiss_popup(ctx, popup, anchor), expected));
            let _ = ctx.run(egui::RawInput { events: vec![egui::Event::PointerButton {
                pos: point, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default(),
            }], ..Default::default() }, |_| {});
        }
        let _ = ctx.run(egui::RawInput { events: vec![egui::Event::Key { key: egui::Key::Escape,
            physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }], ..Default::default() },
            |ctx| assert!(dismiss_popup(ctx, popup, anchor)));
    }

    #[test]
    fn resizing_never_covers_transport_and_seekbar_is_flush() {
        let ctx = egui::Context::default();
        let mut saved = SavedConfig::default();
        saved.playlist = (0..80).map(|id| PlaylistItem {
            id, title: format!("Track {id}"), artist: "Artist".into(),
            duration_str: "03:00".into(), path: PathBuf::new(),
        }).collect();
        let mut app = DeckApp::from_saved(saved, None);
        app.progress = 0.5;
        let sizes: Vec<_> = (700..860).step_by(2).map(|h| (720.0, h as f32))
            .chain((700..860).rev().step_by(2).map(|h| (720.0, h as f32)))
            .chain((340..760).step_by(4).map(|w| (w as f32, 640.0))).collect();
        for (width, height) in sizes {
            let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, height));
            let dock_top = height - 54.0;
            app.hover_transition = 1.0;
            let mut output = egui::FullOutput::default();
            for _ in 0..4 {
                output = ctx.run(egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(pos2(width * 0.5, dock_top + 2.0))],
                    ..Default::default()
                }, |ctx| app.update_ui(ctx));
            }
            let seek_index = output.shapes.iter().position(|shape| matches!(&shape.shape,
                egui::Shape::Rect(rect) if rect.fill == Color32::from_rgb(0xD8, 0xD0, 0xC4)
                    && (rect.rect.width() - width).abs() < 0.1
                    && (rect.rect.min.y - dock_top).abs() < 0.1
            )).expect("seekbar must start exactly at the content/transport boundary");
            let protected_points = [pos2(width * 0.75, dock_top + 1.0), pos2(width * 0.5, dock_top + 32.0)];
            for shape in &output.shapes[seek_index + 1..] {
                if let egui::Shape::Rect(rect) = &shape.shape {
                    if rect.fill == WarmIndustrialTheme::BG_PRIMARY || rect.fill == WarmIndustrialTheme::BG_CARD {
                        for point in protected_points {
                            assert!(!(rect.rect.contains(point) && shape.clip_rect.contains(point)),
                                "content painted over transport at {width}x{height}");
                        }
                    }
                }
            }
            let knob = output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Circle(circle) if circle.fill == WarmIndustrialTheme::ACCENT_ORANGE
                    && (circle.center.x - width * 0.5).abs() < 0.1
                    && circle.center.y < dock_top + 7.0 => Some((shape, circle)),
                _ => None,
            }).expect("scrubber thumb must stay visible");
            let bounds = Rect::from_center_size(knob.1.center, Vec2::splat(knob.1.radius * 2.0));
            assert!(knob.0.clip_rect.contains_rect(bounds), "thumb clipped at {width}x{height}");
        }
    }

    #[test]
    fn artwork_remains_square_and_continuous_through_orientation_change() {
        let mut previous: Option<Rect> = None;
        for height in 300..901 {
            let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(600.0, height as f32));
            let (list, art) = content_rects(available);
            assert_eq!(art.width(), art.height());
            assert!(available.contains_rect(art));
            assert!(available.contains_rect(list));
            if let Some(previous) = previous {
                assert!((art.min.x - previous.min.x).abs() <= 1.0);
                assert!((art.max.y - previous.max.y).abs() <= 1.0);
            }
            previous = Some(art);
        }
        let (list, art) = content_rects(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(600.0, 600.0)));
        assert_eq!(art.size(), Vec2::splat(600.0));
        assert_eq!(list.height(), 0.0);
    }

    #[test]
    fn drag_commits_once_and_old_status_cannot_move_released_playhead() {
        let ctx = egui::Context::default();
        let (audio, requests, status_tx) = AudioEngine::test_engine();
        let mut app = DeckApp::from_saved(SavedConfig::default(), Some(audio));
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(720.0, 480.0));
        let frame = |app: &mut DeckApp, events: Vec<egui::Event>| {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(screen), events, ..Default::default() }, |ctx| app.update_ui(ctx));
        };
        // Wake and expand the floating transport footer to test seeking
        for _ in 0..20 {
            frame(&mut app, vec![egui::Event::PointerMoved(pos2(100.0, 480.0 - 10.0))]);
        }
        let y = 480.0 - 54.0 + 7.0;
        let button = |x, pressed| egui::Event::PointerButton { pos: pos2(x, y), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        for _ in 0..5 { frame(&mut app, vec![egui::Event::PointerMoved(pos2(100.0, y))]); }
        frame(&mut app, vec![button(100.0, true)]);
        for x in [200.0, 500.0, 300.0, 600.0] {
            frame(&mut app, vec![egui::Event::PointerMoved(pos2(x, y))]);
        }
        assert_eq!(requests.len(), 0, "dragging must not queue seeks");
        frame(&mut app, vec![button(600.0, false)]);
        let request = requests.try_recv().expect("release must seek");
        assert!(matches!(request.command, AudioCommand::Seek(p) if (p - 600.0 / 720.0).abs() < 0.001));
        assert!(requests.is_empty());
        let released = app.progress;
        status_tx.send(audio_engine::AudioStatus { revision: 0, progress: 0.1, ..Default::default() }).unwrap();
        frame(&mut app, vec![]);
        assert_eq!(app.progress, released);
        status_tx.send(audio_engine::AudioStatus { revision: request.revision, progress: released, ..Default::default() }).unwrap();
        frame(&mut app, vec![]);
        assert_eq!(app.progress, released);
    }

    #[test]
    fn transport_controls_remain_visible_and_separate_at_supported_sizes() {
        for (width, height) in [(340.0, 360.0), (340.0, 640.0), (519.0, 700.0), (520.0, 360.0), (550.0, 600.0), (580.0, 600.0), (619.0, 600.0), (720.0, 480.0)] {
            let ctx = egui::Context::default();
            let mut app = DeckApp::from_saved(SavedConfig::default(), None);
            let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, height));
            let dock_top = height - 54.0;
            let mut shapes = Vec::new();
            // Hover near bottom edge so the floating footer smoothly expands
            for _ in 0..20 {
                let output = ctx.run(egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(pos2(width * 0.5, height - 10.0))],
                    ..Default::default()
                }, |ctx| app.update_ui(ctx));
                shapes = output.shapes;
            }
            let mut volume_bounds = None;
            let mut transport_bounds = Vec::new();
            for clipped in shapes {
                match &clipped.shape {
                    egui::Shape::Rect(rect) if rect.fill == WarmIndustrialTheme::palette(0, 0).text && rect.rect.center().y > dock_top && rect.rect.center().x > width * 0.6 => {
                        let bounds = rect.rect;
                        assert!(clipped.clip_rect.contains_rect(bounds), "volume clipped at {width}x{height}");
                        assert!(screen.contains_rect(bounds));
                        volume_bounds = Some(bounds);
                    }
                    egui::Shape::Path(path) if path.closed && path.points.len() == 3 && path.points.iter().all(|p| p.y > dock_top) => {
                        let bounds = Rect::from_points(&path.points);
                        assert!(clipped.clip_rect.contains_rect(bounds), "transport clipped at {width}x{height}");
                        assert!(screen.contains_rect(bounds));
                        transport_bounds.push(bounds);
                    }
                    _ => {}
                }
            }
            let volume = volume_bounds.expect("volume control missing");
            assert_eq!(transport_bounds.len(), 3, "previous, play and next must be visible");
            for bounds in transport_bounds {
                assert!(!bounds.expand(8.0).intersects(volume.expand(8.0)), "controls overlap at {width}x{height}");
            }
        }
    }

    #[test]
    fn test_lyrics_parsing_and_toggle() {
        let lrc_sample = "[ti:Never Say Goodbye]\n[ar:Bon Jovi]\n[al:Slippery When Wet]\n[00:01.00]First line\n[00:05.50]v1: Second line\n[bg:<00:06.46>(Rock it)<00:07.476>]\nPlain third line\nv1:<02:50.856><02:50.856>say\nv1:<03:17.952>Ooh, <03:19.136><03:19.136>and";
        let parsed = parse_lrc_lines(lrc_sample);
        assert_eq!(parsed.len(), 5);
        assert_eq!(parsed[0].time_secs, Some(1.0));
        assert_eq!(parsed[0].text, "First line");
        assert_eq!(parsed[1].time_secs, Some(5.5));
        assert_eq!(parsed[1].text, "Second line (Rock it)");
        assert_eq!(parsed[2].time_secs, None);
        assert_eq!(parsed[2].text, "Plain third line");
        assert_eq!(parsed[3].text, "say");
        assert_eq!(parsed[4].text, "Ooh, and");
    }

    #[test]
    fn test_floating_preview_auto_removal() {
        let ctx = egui::Context::default();
        let mut app = DeckApp::from_saved(SavedConfig::default(), None);
        app.hover_transition = 1.0;
        app.active_floating_control = Some(FloatingControlType::Volume);
        app.floating_control_timer = 0.01;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(720.0, 480.0));
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(screen),
            events: vec![egui::Event::PointerMoved(pos2(100.0, 100.0))],
            ..Default::default()
        }, |ctx| app.update_ui(ctx));
        assert!(app.active_floating_control.is_none(), "floating control must be removed automatically after timer expires");
    }

    #[test]
    fn test_unsynced_lyrics_scroll_no_snap_back() {
        let ctx = egui::Context::default();
        let mut app = DeckApp::from_saved(SavedConfig::default(), None);
        app.lyrics_open = true;
        app.current_meta = Some(TrackMetadata {
            title: "Song Without Timestamps".into(),
            artist: "Artist".into(),
            duration_secs: 180.0,
            lyrics: Some("Line 1\nLine 2\nLine 3\nLine 4\nLine 5\nLine 6\nLine 7\nLine 8\nLine 9\nLine 10".into()),
            ..Default::default()
        });
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(720.0, 480.0));
        
        // Frame 1: Initial display
        let _ = ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| app.update_ui(ctx));
        assert_eq!(app.lyrics_user_offset, 0.0);
        assert_eq!(app.lyrics_user_scroll_timer, 0.0);

        // Frame 2: Mouse wheel scroll over lyrics area (right half of screen)
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(screen),
            events: vec![
                egui::Event::PointerMoved(pos2(500.0, 200.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Line,
                    delta: egui::vec2(0.0, -100.0),
                    modifiers: Default::default(),
                },
            ],
            ..Default::default()
        }, |ctx| app.update_ui(ctx));

        assert!(app.lyrics_user_offset > 0.0, "User scroll offset must advance on wheel scroll");
        assert_eq!(app.lyrics_user_scroll_timer, 0.0, "Unsynced lyrics must not set snap-back timer");
        let scrolled_pos = app.lyrics_user_offset;

        // Frame 3: Elapse 5 seconds without scrolling
        let _ = ctx.run(egui::RawInput {
            screen_rect: Some(screen),
            predicted_dt: 5.0,
            events: vec![egui::Event::PointerMoved(pos2(500.0, 200.0))],
            ..Default::default()
        }, |ctx| app.update_ui(ctx));

        // Must still stay at scrolled_pos and NOT snap back to 0.0
        assert_eq!(app.lyrics_user_offset, scrolled_pos, "Unsynced lyrics position must remain exactly where scrolled");
    }
}
