mod api;
mod events;
mod gateway;
mod models;
mod qr_auth;
mod voice;

use api::RestClient;
use eframe::egui;
use egui::{Color32, ScrollArea, RichText, TextEdit};
use events::*;
use gateway::{clear_token, load_token, save_token, spawn_gateway};
use models::{Channel, Guild, Message, User};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use egui::{Frame, Stroke};

const RAIL_BG: Color32 = Color32::from_rgb(0x1e, 0x1f, 0x22);
const CHANNELS_BG: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x31);
const CHAT_BG: Color32 = Color32::from_rgb(0x31, 0x33, 0x38);
const USER_PANEL_BG: Color32 = Color32::from_rgb(0x23, 0x24, 0x28);
const BLURPLE: Color32 = Color32::from_rgb(0x58, 0x65, 0xf2);
const TEXT: Color32 = Color32::from_rgb(0xdb, 0xde, 0xe1);
const TEXT_MUTE: Color32 = Color32::from_rgb(0x94, 0x9b, 0xa4);
const ROW_HOVER: Color32 = Color32::from_rgb(0x35, 0x37, 0x3c);
const ROW_SELECTED: Color32 = Color32::from_rgb(0x40, 0x42, 0x49);
const GREEN: Color32 = Color32::from_rgb(0x23, 0xa5, 0x5a);
const YELLOW: Color32 = Color32::from_rgb(0xf0, 0xb2, 0x32);
const RED: Color32 = Color32::from_rgb(0xf2, 0x3f, 0x43);
const GRAY: Color32 = Color32::from_rgb(0x80, 0x84, 0x8e);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Online,
    Idle,
    Dnd,
    Invisible,
}

impl Presence {
    fn wire(self) -> &'static str {
        match self {
            Presence::Online => "online",
            Presence::Idle => "idle",
            Presence::Dnd => "dnd",
            Presence::Invisible => "invisible",
        }
    }
    fn color(self) -> Color32 {
        match self {
            Presence::Online => GREEN,
            Presence::Idle => YELLOW,
            Presence::Dnd => RED,
            Presence::Invisible => GRAY,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Presence::Online => "Çevrimiçi",
            Presence::Idle => "Boşta",
            Presence::Dnd => "Rahatsız Etmeyin",
            Presence::Invisible => "Görünmez",
        }
    }
}

fn author_color(id: &str) -> Color32 {
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(0x58, 0x65, 0xf2),
        Color32::from_rgb(0xeb, 0x45, 0x9e),
        Color32::from_rgb(0x57, 0xf2, 0x87),
        Color32::from_rgb(0xf0, 0xb2, 0x32),
        Color32::from_rgb(0x23, 0xa5, 0x5a),
        Color32::from_rgb(0x00, 0xb0, 0xf4),
        Color32::from_rgb(0x9c, 0x84, 0xfc),
        Color32::from_rgb(0xfe, 0x53, 0x73),
    ];
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    PALETTE[(h.finish() % PALETTE.len() as u64) as usize]
}

fn ping_wav() -> Vec<u8> {
    let sr: u32 = 44_100;
    let n = (sr * 350 / 1000) as usize;
    let mut samples = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / sr as f32;
        let env = (t * 18.0).min(1.0) * ((0.35 - t).max(0.0) * 9.0).min(1.0);
        let s = (t * 659.26 * 2.0 * std::f32::consts::PI).sin() * 0.6 * env;
        samples.push((s * 32767.0) as i16);
    }
    let data_bytes = (n * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_bytes as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    w.extend_from_slice(b"WAVE");
    w.extend_from_slice(b"fmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&44_100u32.to_le_bytes());
    w.extend_from_slice(&88_200u32.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_bytes.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}


fn draw_media(ui: &mut egui::Ui, gifs: &mut HashMap<String, GifState>, url: &str, tx: &std::sync::mpsc::Sender<UiEvent>, ctx: &egui::Context, max_w: f32) {
    if !gifs.contains_key(url) {
        gifs.insert(url.to_string(), GifState::Loading);
        events::gif_fetch(url.to_string(), tx.clone(), ctx.clone());
    }
    match gifs.get(url) {
        Some(GifState::Loading) => {
            ui.label(RichText::new("🎞 yükleniyor…").small().color(TEXT_MUTE));
        }
        Some(GifState::Failed) => {
            ui.label(RichText::new("🎞 medya yüklenemedi").small().color(TEXT_MUTE));
        }
        Some(GifState::Ready(entry)) => {
            let elapsed = (entry.started.elapsed().as_millis() as u32) % entry.total_ms.max(1);
            let mut acc = 0u32;
            let mut idx = 0usize;
            for (i, (_, d)) in entry.frames.iter().enumerate() {
                acc += *d;
                if elapsed < acc { idx = i; break; }
            }
            if let Some((tex, _)) = entry.frames.get(idx) {
                let w = tex.size()[0] as f32;
                let h = tex.size()[1] as f32;
                let scale = (max_w / w).min(1.0);
                ui.add(egui::Image::new(tex).max_width(w * scale).max_height(h * scale));
            }
        }
        None => {}
    }
}
fn sys_lang() -> &'static str {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if v.to_lowercase().starts_with("tr") {
                return "tr";
            }
        }
    }
    "en"
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([720.0, 480.0])
            .with_title("Discord (Rust)"),
        ..Default::default()
    };

    eframe::run_native(
        "discord-rust",
        options,
        Box::new(|_cc| Ok(Box::new(App::new()))),
    )
}

struct App {
    tx: std::sync::mpsc::Sender<UiEvent>,
    rx: Receiver<UiEvent>,
    token: Option<String>,
    token_input: String,
    user: Option<User>,
    guilds: Vec<Guild>,
    guild_icons: HashMap<String, egui::TextureHandle>,
    selected_guild: Option<String>,
    channels: Vec<Channel>,
    dm_channels: Vec<Channel>,
    selected_channel: Option<String>,
    selected_is_dm: bool,
    messages: Vec<Message>,
    message_input: String,
    status: String,
    show_settings: bool,
    zoom: f32,
    dark: bool,
    font_size: f32,
    auth_mode: AuthMode,
    email: String,
    password: String,
    mfa_ticket: Option<String>,
    mfa_code: String,
    qr_link: Option<String>,
    qr_matrix: Option<Vec<u8>>,
    qr_width: usize,
    qr_status: String,
    qr_user: Option<String>,
    gifs: HashMap<String, GifState>,
    voice_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    voice_connected: Option<String>,
    voice_guild: Option<String>,
    streams: HashMap<String, StreamEntry>,
    watching_channel: Option<String>,
    video_textures: HashMap<String, egui::TextureHandle>,
    video_stops: HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pending_files: Vec<std::path::PathBuf>,
    show_members: bool,
    profile: Option<ProfileTarget>,
    presence: Presence,
    search: String,
    sound_on: bool,
    self_mute: bool,
    self_deafen: bool,
    author_avatars: HashMap<String, egui::TextureHandle>,
    avatar_pending: std::collections::HashSet<String>,
    output_stream: Option<rodio::OutputStream>,
    output_handle: Option<rodio::OutputStreamHandle>,
    settings_tab: SettingsTab,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Gorunum,
    Bildirimler,
    Durum,
    Hesap,
}

#[derive(Clone)]
struct ProfileTarget {
    id: String,
    username: String,
    display_name: String,
    avatar: Option<String>,
    public_flags: u64,
}

fn badges(flags: u64) -> Vec<&'static str> {
    const FLAGS: &[(u64, &str)] = &[
        (1 << 0, "Discord Staff"),
        (1 << 1, "Partnered Server Owner"),
        (1 << 2, "HypeSquad Events"),
        (1 << 3, "Bug Hunter Lv1"),
        (1 << 6, "HypeSquad Bravery"),
        (1 << 7, "HypeSquad Brilliance"),
        (1 << 8, "HypeSquad Balance"),
        (1 << 9, "Early Supporter"),
        (1 << 14, "Bug Hunter Lv2"),
        (1 << 17, "Early Verified Bot Dev"),
        (1 << 18, "Moderator Program Alumni"),
    ];
    FLAGS.iter().filter(|(bit, _)| flags & bit != 0).map(|(_, n)| *n).collect()
}

struct StreamEntry {
    stream_key: String,
    owner_id: String,
    preview: Option<egui::TextureHandle>,
    last_poll: Option<std::time::Instant>,
    inflight: bool,
    failed: bool,
}

struct GifEntry {
    frames: Vec<(egui::TextureHandle, u32)>,
    total_ms: u32,
    started: std::time::Instant,
}

enum GifState {
    Loading,
    Failed,
    Ready(GifEntry),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthMode {
    Email,
    Qr,
    Token,
}

impl App {
    fn new() -> Self {
        let (tx, rx) = channel::<UiEvent>();
        let token = load_token();
        let audio = rodio::OutputStream::try_default().ok();
        let (out_stream, out_handle) = match audio {
            Some((s, h)) => (Some(s), Some(h)),
            None => (None, None),
        };
        let mut app = Self {
            tx,
            rx,
            token: token.clone(),
            token_input: String::new(),
            user: None,
            guilds: Vec::new(),
            guild_icons: HashMap::new(),
            selected_guild: Some("@me".to_string()),
            channels: Vec::new(),
            dm_channels: Vec::new(),
            selected_channel: None,
            selected_is_dm: true,
            messages: Vec::new(),
            message_input: String::new(),
            status: "Bağlantı bekleniyor…".to_string(),
            show_settings: false,
            zoom: 1.0,
            dark: true,
            font_size: 14.0,
            auth_mode: AuthMode::Email,
            email: String::new(),
            password: String::new(),
            mfa_ticket: None,
            mfa_code: String::new(),
            qr_link: None,
            qr_matrix: None,
            qr_width: 0,
            qr_status: String::new(),
            qr_user: None,
            gifs: HashMap::new(),
            voice_tx: None,
            voice_connected: None,
            voice_guild: None,
            streams: HashMap::new(),
            watching_channel: None,
            video_textures: HashMap::new(),
            video_stops: HashMap::new(),
            pending_files: Vec::new(),
            show_members: true,
            profile: None,
            presence: Presence::Online,
            search: String::new(),
            sound_on: true,
            self_mute: false,
            self_deafen: false,
            author_avatars: HashMap::new(),
            avatar_pending: std::collections::HashSet::new(),
            output_stream: out_stream,
            output_handle: out_handle,
            settings_tab: SettingsTab::Gorunum,
        };
        if let Some(tok) = token {
            if let Ok(api) = RestClient::new(tok.clone()) {
                let tx_user = app.tx.clone();
                let api_me = api.clone();
                std::thread::spawn(move || {
                    match api_me.me() {
                        Ok(u) => { let _ = tx_user.send(UiEvent::UserLoaded(u)); }
                        Err(e) => { let _ = tx_user.send(UiEvent::ApiError(format!("me: {e}"))); }
                    }
                });
                events::load_guilds(api.clone(), app.tx.clone(), egui::Context::default());
                let dm_tx = app.tx.clone();
                let dm_api = api.clone();
                std::thread::spawn(move || {
                    match dm_api.dm_channels() {
                        Ok(mut dm) => {
                            dm.sort_by(|a, b| {
                                let ka = a.last_message_id.as_deref().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
                                let kb = b.last_message_id.as_deref().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
                                kb.cmp(&ka)
                            });
                            let _ = dm_tx.send(UiEvent::DmChannels(dm));
                        }
                        Err(e) => { let _ = dm_tx.send(UiEvent::ApiError(format!("dms: {e}"))); }
                    }
                });
                app.spawn_gateway_session(tok, &egui::Context::default());
            }
        }
        app
    }

    fn start_session(&mut self, ctx: &egui::Context, token: String) {
        self.token = Some(token.clone());
        self.status = "Bağlanıyor…".into();
        let api = match RestClient::new(token.clone()) {
            Ok(a) => a,
            Err(e) => {
                self.status = format!("http client hatası: {e}");
                return;
            }
        };
        // load current user + guilds + gateway, all concurrent
        let tx_user = self.tx.clone();
        let ctx_user = ctx.clone();
        let api_user = api.clone();
        std::thread::spawn(move || {
            match api_user.me() {
                Ok(u) => {
                    let _ = tx_user.send(UiEvent::UserLoaded(u));
                }
                Err(e) => {
                    let _ = tx_user.send(UiEvent::ApiError(format!("me: {e}")));
                }
            }
            ctx_user.request_repaint();
        });

        events::load_guilds(api.clone(), self.tx.clone(), ctx.clone());
        let dm_tx = self.tx.clone();
        let dm_ctx = ctx.clone();
        let dm_api = api.clone();
        std::thread::spawn(move || {
            match dm_api.dm_channels() {
                Ok(mut dm) => {
                    dm.sort_by(|a, b| {
                        let ka = a.last_message_id.as_deref().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
                        let kb = b.last_message_id.as_deref().and_then(|x| x.parse::<u128>().ok()).unwrap_or(0);
                        kb.cmp(&ka)
                    });
                    let _ = dm_tx.send(UiEvent::DmChannels(dm));
                }
                Err(e) => {
                    let _ = dm_tx.send(UiEvent::ApiError(format!("dms: {e}")));
                }
            }
            dm_ctx.request_repaint();
        });
        self.spawn_gateway_session(token, ctx);
    }

    fn ping(&self) {
        if let Some(h) = &self.output_handle {
            if let Ok(sink) = rodio::Sink::try_new(h) {
                if let Ok(dec) = rodio::Decoder::new(std::io::Cursor::new(ping_wav())) {
                    sink.append(dec);
                    sink.detach();
                }
            }
        }
    }

    fn set_presence(&mut self, p: Presence) {
        self.presence = p;
        let payload = serde_json::json!({
            "op": 3,
            "d": { "since": null, "activities": [], "status": p.wire(), "afk": false }
        });
        if let Some(s) = &self.voice_tx {
            let _ = s.send(payload.to_string());
        }
        self.status = format!("Durum güncellendi: {}", p.label());
    }

    fn spawn_gateway_session(&mut self, token: String, ctx: &egui::Context) {
        let (vtx, vrx) = tokio::sync::mpsc::unbounded_channel();
        self.voice_tx = Some(vtx);
        spawn_gateway(token, self.tx.clone(), ctx.clone(), Some(vrx));
    }

    fn toggle_voice(&mut self, guild_id: Option<String>, channel_id: &str) {
        let leaving = self.voice_connected.as_deref() == Some(channel_id);
        let gid = match &guild_id {
            Some(g) if g.as_str() != "@me" => serde_json::Value::String(g.clone()),
            _ => serde_json::Value::Null,
        };
        let payload = if leaving {
            serde_json::json!({ "op": 4, "d": { "guild_id": gid, "channel_id": null, "self_mute": false, "self_deaf": false } })
        } else {
            serde_json::json!({ "op": 4, "d": { "guild_id": gid, "channel_id": channel_id, "self_mute": false, "self_deaf": false } })
        };
        match &self.voice_tx {
            Some(s) => {
                if let Err(e) = s.send(payload.to_string()) {
                    self.status = format!("ses komutu gönderilemedi: {e}");
                    return;
                }
            }
            None => {
                self.status = "gateway kapalı — ses komutu işlenemez".into();
                return;
            }
        }
        if leaving {
            self.voice_connected = None;
            self.voice_guild = None;
            self.status = "Ses: çıkıldı".into();
        } else {
            self.voice_connected = Some(channel_id.to_string());
            self.voice_guild = guild_id.clone();
            self.status = "Ses: katılım isteği gönderildi — voice handshake başlıyor…".into();
        }
    }

    fn load_messages_for(&mut self, ctx: &egui::Context, channel_id: String) {
        if let Some(tok) = &self.token {
            if let Ok(api) = RestClient::new(tok.clone()) {
                events::load_messages(api, channel_id, self.tx.clone(), ctx.clone());
            }
        }
    }

    fn load_channels_for(&mut self, ctx: &egui::Context, guild_id: String) {
        if let Some(tok) = &self.token {
            if let Ok(api) = RestClient::new(tok.clone()) {
                events::load_channels(api, guild_id, self.tx.clone(), ctx.clone());
            }
        }
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.rx.try_recv() {
            match ev {
                UiEvent::Connected(s) => self.status = s,
                UiEvent::Disconnected(s) => self.status = s,
                UiEvent::GatewayState(s) => self.status = s,
                UiEvent::Guilds(g) => {
                    self.guilds = g;
                    self.status = format!("{} sunucu yüklendi", self.guilds.len());
                }
                UiEvent::DmChannels(dm) => {
                    self.dm_channels = dm.clone();
                    if self.selected_is_dm {
                        self.channels = self.dm_channels.clone();
                    }
                    if let Some(tok) = self.token.clone() {
                        for c in &dm {
                            for r in &c.recipients {
                                if self.author_avatars.contains_key(&r.id) || self.avatar_pending.contains(&r.id) {
                                    continue;
                                }
                                if let Some(av) = r.avatar.clone() {
                                    self.avatar_pending.insert(r.id.clone());
                                    events::avatar_fetch(tok.clone(), r.id.clone(), av, self.tx.clone(), ctx.clone());
                                }
                            }
                        }
                    }
                }
                UiEvent::UserLoaded(u) => {
                    self.user = Some(u.clone());
                    self.status = format!("{} olarak giriş yapıldı", u.display_name());
                    if let Some(tok) = self.token.clone() {
                        if let Some(av) = self.user.as_ref().and_then(|x| x.avatar.clone()) {
                            events::avatar_fetch(tok, self.user.as_ref().unwrap().id.clone(), av, self.tx.clone(), ctx.clone());
                        }
                    }
                }
                UiEvent::Channels { guild_id: _, channels } => {
                    self.channels = channels;
                }
                UiEvent::Messages { channel_id, messages } => {
                    if Some(&channel_id) == self.selected_channel.as_ref() {
                        self.messages = messages;
                    }
                    for m in &self.messages {
                        let id = m.author.id.clone();
                        if self.author_avatars.contains_key(&id) || !self.avatar_pending.insert(id.clone()) {
                            continue;
                        }
                        if let Some(av) = m.author.avatar.clone() {
                            if let Some(tok) = self.token.clone() {
                                events::avatar_fetch(tok, id, av, self.tx.clone(), ctx.clone());
                            }
                        }
                    }
                }
                UiEvent::VideoFrame { id, width, height, rgba } => {
                    let tex = ctx.load_texture(
                        format!("video-{id}"),
                        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba),
                        Default::default(),
                    );
                    self.video_textures.insert(id, tex);
                }
                UiEvent::VideoDone { id } => {
                    self.video_stops.remove(&id);
                    self.video_textures.remove(&id);
                }
                UiEvent::DmOpened { channel } => {
                    self.dm_channels.push(channel.clone());
                    self.selected_guild = Some("@me".into());
                    self.selected_is_dm = true;
                    self.channels = self.dm_channels.clone();
                    let cid = channel.id.clone();
                    self.selected_channel = Some(cid.clone());
                    self.messages.clear();
                    self.load_messages_for(&ctx, cid);
                }
                UiEvent::AvatarLoaded { user_id, bytes } => {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgba = img.into_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let tex = ctx.load_texture(
                            format!("avatar-{user_id}"),
                            egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                            Default::default(),
                        );
                        self.author_avatars.insert(user_id.clone(), tex);
                    }
                    self.avatar_pending.remove(&user_id);
                }
                UiEvent::MessageSent { channel_id, message } => {
                    if Some(&channel_id) == self.selected_channel.as_ref() {
                        self.messages.push(message);
                    }
                }
                UiEvent::LiveMessage { channel_id, message } => {
                    let from_me = self.user.as_ref().map(|u| u.id == message.author.id).unwrap_or(false);
                    if Some(&channel_id) == self.selected_channel.as_ref()
                        && !self.messages.iter().any(|m| m.id == message.id)
                    {
                        self.messages.push(message.clone());
                    }
                    if self.sound_on && !from_me {
                        self.ping();
                    }
                }
                UiEvent::GuildIcon { id, bytes } => {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let rgba = img.into_rgba8();
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        let tex = ctx.load_texture(
                            format!("guild-icon-{id}"),
                            egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                            Default::default(),
                        );
                        self.guild_icons.insert(id, tex);
                    }
                }
                UiEvent::ApiError(e) => {
                    self.status = e;
                }
                UiEvent::StreamStarted { stream_key, channel_id, owner_id } => {
                    self.streams.insert(
                        channel_id,
                        StreamEntry { stream_key, owner_id, preview: None, last_poll: None, inflight: false, failed: false },
                    );
                    self.status = "📺 Canlı yayın başladı".into();
                }
                UiEvent::StreamEnded { stream_key } => {
                    let gone: Vec<String> = self
                        .streams
                        .iter()
                        .filter(|(_, e)| e.stream_key == stream_key)
                        .map(|(c, _)| c.clone())
                        .collect();
                    for c in gone {
                        self.streams.remove(&c);
                        if self.watching_channel.as_deref() == Some(c.as_str()) {
                            self.watching_channel = None;
                        }
                    }
                    self.status = "📺 Yayın sona erdi".into();
                }
                UiEvent::StreamPreview { stream_key, image_bytes } => {
                    if let Some(entry) = self.streams.iter_mut().find(|(_, e)| e.stream_key == stream_key).map(|(_, e)| e) {
                        entry.inflight = false;
                        entry.failed = false;
                        entry.last_poll = Some(std::time::Instant::now());
                        if let Ok(img) = image::load_from_memory(&image_bytes) {
                            let rgba = img.into_rgba8();
                            let size = [rgba.width() as usize, rgba.height() as usize];
                            entry.preview = Some(ctx.load_texture(
                                format!("preview-{stream_key}"),
                                egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                                Default::default(),
                            ));
                        }
                    }
                }
                UiEvent::StreamPreviewFailed { stream_key } => {
                    if let Some(entry) = self.streams.iter_mut().find(|(_, e)| e.stream_key == stream_key).map(|(_, e)| e) {
                        entry.inflight = false;
                        entry.failed = true;
                        entry.last_poll = Some(std::time::Instant::now());
                    }
                }
                UiEvent::VoiceStateUpdate { guild_id, .. } => {
                    self.status = match guild_id {
                        Some(_) => "Ses: voice gateway'e bağlanıldı".into(),
                        None => "Ses: durum güncellendi".into(),
                    };
                }
                UiEvent::EmailLoginOk(token) => {
                    let _ = save_token(&token);
                    self.start_session(&ctx, token);
                }
                UiEvent::EmailLoginMfa(ticket) => {
                    self.mfa_ticket = Some(ticket);
                    self.status = "2FA (TOTP) kodu gerekiyor.".into();
                }
                UiEvent::EmailLoginErr(e) => {
                    self.status = e;
                }
                UiEvent::QrStatus(s) => {
                    self.qr_status = s;
                }
                UiEvent::QrLink { link, matrix, width } => {
                    self.qr_link = Some(link);
                    self.qr_matrix = Some(matrix);
                    self.qr_width = width;
                    self.qr_user = None;
                }
                UiEvent::QrUser(u) => {
                    self.qr_user = Some(u);
                }
                UiEvent::QrToken(token) => {
                    let _ = save_token(&token);
                    self.start_session(&ctx, token);
                }
                UiEvent::GifReady { url, frames } => {
                    let texs: Vec<(egui::TextureHandle, u32)> = frames
                        .iter()
                        .enumerate()
                        .map(|(i, f)| {
                            let tex = ctx.load_texture(
                                format!("gif-{url}-{i}"),
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [f.width as usize, f.height as usize],
                                    &f.rgba,
                                ),
                                Default::default(),
                            );
                            (tex, f.delay_ms)
                        })
                        .collect();
                    let total_ms: u32 = texs.iter().map(|(_, d)| *d).sum::<u32>().max(1);
                    self.gifs.insert(url, GifState::Ready(GifEntry { frames: texs, total_ms, started: std::time::Instant::now() }));
                }
                UiEvent::GifFailed { url } => {
                    self.gifs.insert(url, GifState::Failed);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.drain_events(&ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
        // drag-and-drop anywhere on the window = attach
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        for f in dropped {
            let p = f.path().to_path_buf();
            if p.exists() && !self.pending_files.contains(&p) && self.pending_files.len() < 10 {
                self.pending_files.push(p);
            }
        }

        if self.dark {
            let mut v = egui::Visuals::dark();
            v.panel_fill = CHAT_BG;
            v.window_fill = CHANNELS_BG;
            v.window_stroke = Stroke::new(1.0, RAIL_BG);
            v.selection.bg_fill = ROW_SELECTED;
            v.selection.stroke = Stroke::new(1.0, TEXT);
            v.widgets.noninteractive.weak_bg_fill = CHANNELS_BG;
            v.widgets.inactive.weak_bg_fill = CHAT_BG;
            v.widgets.hovered.weak_bg_fill = ROW_HOVER;
            v.widgets.hovered.bg_fill = ROW_HOVER;
            v.widgets.active.bg_fill = BLURPLE;
            v.widgets.open.bg_fill = ROW_SELECTED;
            ctx.set_visuals(v);
        } else {
            ctx.set_visuals(egui::Visuals::light());
        }
        ctx.set_zoom_factor(self.zoom);

        // --- Login screen
        if let Some(cid) = self.watching_channel.clone() {
            let due = self
                .streams
                .get(&cid)
                .map(|e| !e.inflight && e.last_poll.map(|t| t.elapsed() >= Duration::from_secs(2)).unwrap_or(true))
                .unwrap_or(false);
            if due {
                if let Some(entry) = self.streams.get_mut(&cid) {
                    entry.inflight = true;
                    let sk = entry.stream_key.clone();
                    if let Some(tok) = self.token.clone() {
                        events::stream_preview_fetch(tok, sk, self.tx.clone(), ctx.clone());
                    } else {
                        entry.inflight = false;
                        entry.failed = true;
                    }
                }
            }
        }

        if self.token.is_none() {
            let ctx = ui.ctx().clone();
            egui::CentralPanel::default().show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(36.0);
                    let (logo_rect, _) = ui.allocate_exact_size(egui::vec2(72.0, 72.0), egui::Sense::hover());
                    let painter = ui.painter_at(logo_rect);
                    painter.rect_filled(logo_rect, 16.0, BLURPLE);
                    painter.text(logo_rect.center(), egui::Align2::CENTER_CENTER, "D", egui::FontId::proportional(40.0), Color32::WHITE);
                    ui.add_space(10.0);
                    ui.heading(RichText::new("Discord'a Hoş Geldin!").size(22.0).strong().color(Color32::WHITE));
                    ui.label(RichText::new("Hesabına gir. Üç yol var:").color(TEXT_MUTE));

                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.auth_mode, AuthMode::Email, "✉ Email + Şifre");
                        ui.selectable_value(&mut self.auth_mode, AuthMode::Qr, "📱 QR ile giriş");
                        ui.selectable_value(&mut self.auth_mode, AuthMode::Token, "🔑 Token");
                    });
                    ui.separator();
                    ui.add_space(8.0);

                    match self.auth_mode {
                        AuthMode::Email => {
                            ui.label("Discord hesabının email'i ve şifresi:");
                            ui.add_space(6.0);
                            ui.add(TextEdit::singleline(&mut self.email).hint_text("email@example.com").desired_width(360.0));
                            ui.add_space(6.0);
                            let pass = ui.add(TextEdit::singleline(&mut self.password).password(true).hint_text("şifre").desired_width(360.0));
                            ui.add_space(6.0);
                            if ui.add(egui::Button::new(RichText::new("Giriş Yap").color(Color32::WHITE)).fill(BLURPLE).min_size(egui::vec2(120.0, 36.0))).clicked()
                                || (pass.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                            {
                                let email = self.email.trim().to_string();
                                let password = self.password.clone();
                                if !email.is_empty() && !password.is_empty() {
                                    self.status = "Giriş yapılıyor…".into();
                                    events::email_login(email, password, self.tx.clone(), ctx.clone());
                                }
                            }
                            if let Some(_ticket) = self.mfa_ticket.clone() {
                                ui.add_space(12.0);
                                ui.label(RichText::new("İki adımlı doğrulama gerekli. Authenticator uygulamandaki 6 haneli kod:").small().weak());
                                ui.add(TextEdit::singleline(&mut self.mfa_code).hint_text("000000").desired_width(180.0));
                                if ui.button("Onayla").clicked() {
                                    let ticket = self.mfa_ticket.clone().unwrap();
                                    let code = self.mfa_code.trim().to_string();
                                    if !code.is_empty() {
                                        self.status = "2FA onaylanıyor…".into();
                                        events::mfa_totp_login(ticket, code, self.tx.clone(), ctx.clone());
                                    }
                                }
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new("Discord yeni cihazda CAPTCHA isteyebilir — o durumda QR veya token yolunu kullan.").small().weak());
                        }
                        AuthMode::Qr => {
                            if self.qr_matrix.is_none() {
                                ui.label("Telefonundaki Discord'u tara, bilgisayarda giriş yapılsın.");
                                ui.add_space(10.0);
                                if ui.add(egui::Button::new(RichText::new("QR Oturumu Başlat").color(Color32::WHITE)).fill(BLURPLE).min_size(egui::vec2(160.0, 36.0))).clicked() {
                                    self.qr_status.clear();
                                    self.qr_user = None;
                                    events::qr_start(self.tx.clone(), ctx.clone());
                                }
                            } else {
                                // QR matrisini çiz
                                let cell = 8.0f32;
                                let side = self.qr_width as f32 * cell;
                                let padding = 14.0f32;
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(side + 2.0 * padding, side + 2.0 * padding),
                                    egui::Sense::hover(),
                                );
                                let painter = ui.painter_at(rect);
                                painter.rect_filled(rect, 8.0, Color32::WHITE);
                                if let Some(matrix) = &self.qr_matrix {
                                    for (i, dark) in matrix.iter().enumerate() {
                                        if *dark == 1 {
                                            let x = (i % self.qr_width) as f32 * cell + rect.min.x + padding;
                                            let y = (i / self.qr_width) as f32 * cell + rect.min.y + padding;
                                            painter.rect_filled(
                                                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(cell, cell)),
                                                0.0,
                                                Color32::from_rgb(25, 25, 28),
                                            );
                                        }
                                    }
                                }
                                ui.add_space(8.0);
                                if let Some(u) = &self.qr_user {
                                    ui.label(RichText::new(format!("Telefonundaki cihaz: {u} — isteği onayla")).strong());
                                }
                                if let Some(link) = &self.qr_link {
                                    ui.label(RichText::new(link.as_str()).small().weak());
                                }
                                ui.add_space(6.0);
                                if ui.button("Yenile / iptal").clicked() {
                                    self.qr_matrix = None;
                                    self.qr_link = None;
                                    self.qr_user = None;
                                    self.qr_status.clear();
                                }
                            }
                            ui.add_space(8.0);
                            ui.label(RichText::new(&self.qr_status).small());
                        }
                        AuthMode::Token => {
                            ui.label("User token ile giriş (DevTools → Network → @me → authorization başlığı):");
                            ui.add_space(6.0);
                            let resp = ui.add(
                                TextEdit::singleline(&mut self.token_input)
                                    .password(true)
                                    .hint_text("token…")
                                    .desired_width(360.0),
                            );
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                let t = self.token_input.trim().to_string();
                                if !t.is_empty() {
                                    let _ = save_token(&t);
                                    self.start_session(&ctx, t);
                                }
                            }
                            ui.add_space(8.0);
                            if ui.add(egui::Button::new(RichText::new("Bağlan").color(Color32::WHITE)).fill(BLURPLE).min_size(egui::vec2(120.0, 36.0))).clicked() {
                                let t = self.token_input.trim().to_string();
                                if !t.is_empty() {
                                    let _ = save_token(&t);
                                    self.start_session(&ctx, t);
                                }
                            }
                        }
                    }
                    ui.add_space(16.0);
                    ui.label(RichText::new(&self.status).small().weak());
                });
            });
            return;
        }

        // --- TOP BAR
        egui::Panel::top("top")
            .frame(Frame::new().fill(CHANNELS_BG).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    // Discord logo: blurple rounded square
                    let (logo_rect, _) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
                    let painter = ui.painter_at(logo_rect);
                    painter.rect_filled(logo_rect, 6.0, BLURPLE);
                    painter.text(logo_rect.center(), egui::Align2::CENTER_CENTER, "D", egui::FontId::proportional(15.0), Color32::WHITE);
                    ui.label(RichText::new("Discord (Rust)").strong().color(TEXT));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("⚙").on_hover_text("Ayarlar").clicked() {
                            self.show_settings = !self.show_settings;
                        }
                        let status = self.status.to_lowercase();
                        let errish = status.contains("http") || status.contains("captcha") || status.contains("error");
                        let color = if errish { RED } else { TEXT_MUTE };
                        ui.label(RichText::new(&self.status).small().color(color));
                    });
                });
            });

        if self.show_settings {
            let mut settings_open = true;
            egui::Window::new("⚙ Ayarlar")
                .open(&mut settings_open)
                .collapsible(false)
                .default_size([620.0, 430.0])
                .frame(Frame::new().fill(CHANNELS_BG).inner_margin(14).corner_radius(10.0).stroke(Stroke::new(1.0, RAIL_BG)))
                .show(&ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_min_width(150.0);
                        for (tab, label) in [
                            (SettingsTab::Gorunum, "🎨 Görünüm"),
                            (SettingsTab::Bildirimler, "🔔 Bildirimler"),
                            (SettingsTab::Durum, "🟢 Durum"),
                            (SettingsTab::Hesap, "👤 Hesap"),
                        ] {
                            let selected = self.settings_tab == tab;
                            let text = RichText::new(label).size(13.5).color(if selected { Color32::WHITE } else { TEXT_MUTE });
                            if ui.add(egui::Button::new(text).fill(if selected { BLURPLE } else { Color32::TRANSPARENT }).min_size(egui::vec2(150.0, 34.0))).clicked() {
                                self.settings_tab = tab;
                            }
                            ui.add_space(2.0);
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| match self.settings_tab {
                        SettingsTab::Gorunum => {
                            ui.heading("Görünüm");
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Yazı boyutu");
                                ui.add(egui::Slider::new(&mut self.font_size, 11.0..=22.0).suffix("px"));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Zoom");
                                ui.add(egui::Slider::new(&mut self.zoom, 0.75..=1.6).suffix("x"));
                            });
                            ui.checkbox(&mut self.dark, "Karanlık mod");
                            ui.add_space(10.0);
                            ui.label(RichText::new("Paletin tamamı Discord değerlerine ayarlı: rail #1e1f22, kenar çubukları #2b2d31, mesaj alanı #313338.").small().color(TEXT_MUTE));
                        }
                        SettingsTab::Bildirimler => {
                            ui.heading("Bildirimler");
                            ui.add_space(6.0);
                            ui.checkbox(&mut self.sound_on, "Yeni mesajda ping sesi çal");
                            ui.add_space(6.0);
                            ui.label(RichText::new(if self.output_handle.is_some() { "Ses çıkış aygıtı: bağlı" } else { "Ses çıkış aygıtı bulunamadı — ping sessiz kalır" }).small().color(TEXT_MUTE));
                        }
                        SettingsTab::Durum => {
                            ui.heading("Durum");
                            ui.add_space(6.0);
                            for (p, label) in [
                                (Presence::Online, "● Çevrimiçi"),
                                (Presence::Idle, "🌙 Boşta"),
                                (Presence::Dnd, "⛔ Rahatsız Etmeyin"),
                                (Presence::Invisible, "⚫ Görünmez"),
                            ] {
                                let selected = self.presence == p;
                                if ui.selectable_label(selected, RichText::new(label).size(15.0)).clicked() {
                                    self.set_presence(p);
                                }
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new("Durum gateway op 3 ile sunucuya bildirilir (presence_update).").small().color(TEXT_MUTE));
                        }
                        SettingsTab::Hesap => {
                            ui.heading("Hesap");
                            ui.add_space(6.0);
                            if let Some(u) = &self.user {
                                ui.label(RichText::new(u.display_name()).strong().color(TEXT));
                                ui.label(RichText::new(format!("id: {}", u.id)).small().color(TEXT_MUTE));
                                ui.label(RichText::new(format!("kullanıcı adı: {}", u.username)).small().color(TEXT_MUTE));
                            } else {
                                ui.label(RichText::new("Kullanıcı bilgisi yüklenmedi.").color(TEXT_MUTE));
                            }
                            ui.add_space(12.0);
                            if ui.add(egui::Button::new(RichText::new("Token'ı temizle ve çıkış yap").color(Color32::WHITE)).fill(RED).min_size(egui::vec2(220.0, 38.0))).clicked() {
                                let _ = clear_token();
                                self.token = None;
                                self.user = None;
                                self.guilds.clear();
                                self.channels.clear();
                                self.dm_channels.clear();
                                self.messages.clear();
                                self.selected_channel = None;
                                self.show_settings = false;
                                self.status = "Çıkış yapıldı.".into();
                            }
                        }
                    });
                });
            });
            if !settings_open {
                self.show_settings = false;
            }
        }


        // --- PROFILE POPUP
        if let Some(target) = self.profile.clone() {
            egui::Window::new("")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .fixed_size(egui::vec2(320.0, 380.0))
                .frame(Frame::new().fill(CHANNELS_BG).inner_margin(18.0).corner_radius(14.0).stroke(Stroke::new(1.0, RAIL_BG)))
                .title_bar(false)
                .show(&ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                            if ui.button("✕").clicked() { self.profile = None; }
                        });
                    });
                    ui.vertical_centered(|ui| {
                        let size = 96.0;
                        match self.author_avatars.get(&target.id) {
                            Some(tex) => { ui.add(egui::Image::new(tex).max_width(size).max_height(size)); }
                            None => {
                                let initial = target.display_name.chars().next().unwrap_or('?').to_uppercase().to_string();
                                ui.label(RichText::new(initial).size(54.0).strong().color(Color32::WHITE).background_color(author_color(&target.id)));
                            }
                        }
                        ui.add_space(10.0);
                        ui.heading(RichText::new(&target.display_name).color(Color32::WHITE));
                        ui.label(RichText::new(format!("@{}", target.username)).color(TEXT_MUTE));
                        ui.add_space(10.0);
                        let bs = badges(target.public_flags);
                        if !bs.is_empty() {
                            ui.label(RichText::new("Rozetler").small().strong().color(TEXT_MUTE));
                            ui.horizontal_wrapped(|ui| {
                                for b in bs {
                                    ui.label(RichText::new(b).small().color(TEXT).background_color(ROW_SELECTED));
                                }
                            });
                            ui.add_space(6.0);
                        }
                        if ui.add(egui::Button::new(RichText::new("📨 Direkt Mesaj Gönder").color(Color32::WHITE)).fill(BLURPLE).min_size(egui::vec2(220.0, 38.0))).clicked() {
                            let id_td = target.id.clone();
                            // already an open DM? select it; else create it
                            if let Some(ch) = self.dm_channels.iter().find(|c| c.recipients.iter().any(|r| r.id == id_td)).cloned() {
                                self.selected_guild = Some("@me".into());
                                self.selected_is_dm = true;
                                self.channels = self.dm_channels.clone();
                                self.selected_channel = Some(ch.id.clone());
                                self.messages.clear();
                                self.load_messages_for(&ctx, ch.id);
                                self.profile = None;
                            } else if let Some(tok) = self.token.clone() {
                                let tx = self.tx.clone();
                                let ctx2 = ctx.clone();
                                std::thread::spawn(move || {
                                    match RestClient::new(tok).and_then(|api| api.open_dm(&id_td)) {
                                        Ok(channel) => { let _ = tx.send(UiEvent::DmOpened { channel }); }
                                        Err(e) => { let _ = tx.send(UiEvent::ApiError(format!("dm açılamadı: {e}"))); }
                                    }
                                    ctx2.request_repaint();
                                });
                                self.profile = None;
                            }
                        }
                    });
                });
        }
        // --- GUILD RAIL (leftmost)
        egui::Panel::left("guild-rail")
            .frame(Frame::new().fill(RAIL_BG))
            .resizable(false)
            .exact_size(72.0)
            .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                // Discord logo button
                let logo = egui::Button::new(RichText::new("D").size(20.0).strong().color(Color32::WHITE))
                    .fill(BLURPLE)
                    .min_size(egui::vec2(44.0, 44.0));
                ui.add(logo).on_hover_text("Discord (Rust)");
                ui.add_space(8.0);
                // Home (DMs)
                let home_selected = self.selected_guild.as_deref() == Some("@me");
                let home = egui::Button::new(RichText::new("💬").size(20.0))
                    .fill(if home_selected { BLURPLE } else { ROW_SELECTED })
                    .min_size(egui::vec2(44.0, 44.0));
                if ui.add(home).on_hover_text("Direk Mesajlar").clicked() {
                    self.selected_guild = Some("@me".into());
                    self.selected_is_dm = true;
                    self.channels = self.dm_channels.clone();
                    self.selected_channel = None;
                    self.messages.clear();
                }
                ui.add_space(6.0);
                ui.separator();
                ScrollArea::vertical().show(ui, |ui| {
                    for g in &self.guilds.clone() {
                        let selected = self.selected_guild.as_deref() == Some(g.id.as_str());
                        let fill = if selected { BLURPLE } else { ROW_SELECTED };
                        let btn = if let Some(tex) = self.guild_icons.get(&g.id) {
                            egui::Button::image(egui::Image::new(tex).max_height(44.0).max_width(44.0))
                                .fill(fill)
                                .min_size(egui::vec2(44.0, 44.0))
                        } else {
                            egui::Button::new(RichText::new(&g.name.chars().take(2).collect::<String>().to_uppercase()).size(16.0).strong())
                                .fill(fill)
                                .min_size(egui::vec2(44.0, 44.0))
                        };
                        if ui.add(btn).on_hover_text(&g.name).clicked() {
                            self.selected_guild = Some(g.id.clone());
                            self.selected_is_dm = false;
                            self.selected_channel = None;
                            self.messages.clear();
                            self.load_channels_for(&ctx, g.id.clone());
                        }
                        ui.add_space(4.0);
                    }
                });
            });
        });

        // --- CHANNEL LIST (second column)
        egui::Panel::left("channel-list")
            .frame(Frame::new().fill(CHANNELS_BG))
            .resizable(true)
            .default_size(248.0)
            .min_size(170.0)
            .show(ui, |ui| {
            let title = if self.selected_is_dm {
                "Direk Mesajlar".to_string()
            } else {
                self.guilds.iter().find(|g| Some(&g.id) == self.selected_guild.as_ref()).map(|g| g.name.clone()).unwrap_or_else(|| "Sunucu".into())
            };
            ui.heading(RichText::new(title).size(15.0).strong().color(TEXT));
            ui.separator();
            let vh = ui.available_height();
            ScrollArea::vertical().id_salt("ch-list").max_height((vh - 62.0).max(0.0)).show(ui, |ui| {
                for c in self.channels.clone() {
                    if c.kind == Channel::CATEGORY {
                        ui.add_space(8.0);
                        ui.label(RichText::new(c.display_name().to_uppercase()).small().strong().color(TEXT_MUTE));
                        continue;
                    }
                    let selected = self.selected_channel.as_deref() == Some(c.id.as_str());
                    let icon = if c.is_voice() { "🔊" } else { "#" };
                    ui.horizontal(|ui| {
                        // DM/group rows show the peer's avatar instead of an icon
                        if c.is_dm() {
                            if let Some(r) = c.recipients.first() {
                                match self.author_avatars.get(&r.id) {
                                    Some(tex) => {
                                        ui.add(egui::Image::new(tex).max_width(30.0).max_height(30.0));
                                    }
                                    None => {
                                        let initial = r.username.chars().next().unwrap_or('?').to_uppercase().collect::<String>();
                                        ui.label(RichText::new(initial).size(13.0).strong().color(Color32::WHITE).background_color(author_color(&r.id)));
                                    }
                                }
                            }
                        }
                        let label = if c.is_dm() { c.display_name() } else { format!("{} {}", icon, c.display_name()) };
                        let resp = ui.selectable_label(
                            selected,
                            RichText::new(label)
                                .size(self.font_size)
                                .color(if selected { Color32::WHITE } else { TEXT_MUTE }),
                        );
                        if self.streams.contains_key(&c.id) {
                            let watched = self.watching_channel.as_deref() == Some(c.id.as_str());
                            if ui.small_button(if watched { "📺■" } else { "📺" }).clicked() {
                                self.watching_channel = if watched { None } else { Some(c.id.clone()) };
                                self.status = if watched { "📺 izleme durduruldu".into() } else { "📺 yayın önizlemesi açıldı".into() };
                            }
                        }
                        if resp.clicked() {
                            self.selected_channel = Some(c.id.clone());
                            self.messages.clear();
                            self.load_messages_for(&ctx, c.id.clone());
                            if c.is_voice() {
                                self.toggle_voice(self.selected_guild.clone(), &c.id);
                            }
                        }
                    });
                }
                if self.channels.is_empty() {
                    ui.label(RichText::new("Kanallar yükleniyor ya da boş…").small().color(TEXT_MUTE));
                }
            });
            ui.separator();
            // --- USER PANEL (bottom)
            Frame::new().fill(USER_PANEL_BG).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(u) = &self.user {
                        let name = u.display_name();
                        let avatar_resp = match self.author_avatars.get(&u.id) {
                            Some(tex) => ui.add(egui::Image::new(tex).max_width(38.0).max_height(38.0)),
                            None => {
                                let initial = name.chars().next().unwrap_or('?').to_uppercase().collect::<String>();
                                ui.add(egui::Label::new(RichText::new(initial).size(18.0).strong().color(Color32::WHITE)))
                            }
                        };
                        // status dot at bottom-right of avatar
                        let painter = ui.painter_at(avatar_resp.rect);
                        let dot_center = egui::pos2(avatar_resp.rect.max.x - 7.0, avatar_resp.rect.max.y - 7.0);
                        painter.circle_filled(dot_center, 5.5, RAIL_BG);
                        painter.circle_filled(dot_center, 4.5, self.presence.color());
                        ui.vertical(|ui| {
                            ui.label(RichText::new(name).size(13.0).strong().color(Color32::WHITE));
                            ui.label(RichText::new(self.presence.label()).small().color(self.presence.color()));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("⚙").on_hover_text("Ayarlar").clicked() {
                                self.show_settings = !self.show_settings;
                            }
                            let deaf_btn = if self.self_deafen { "🎧❌" } else { "🎧" };
                            if ui.button(deaf_btn).on_hover_text("Kulaklık sustur / aç").clicked() {
                                self.self_deafen = !self.self_deafen;
                            }
                            let mic_btn = if self.self_mute { "🔇" } else { "🎙" };
                            if ui.button(mic_btn).on_hover_text("Mikrofon sustur / aç").clicked() {
                                self.self_mute = !self.self_mute;
                                if let Some(s) = &self.voice_tx {
                                    if let Some(ch) = &self.voice_connected {
                                        let gid = self.voice_guild.clone().map(serde_json::Value::String).unwrap_or(serde_json::Value::Null);
                                        let payload = serde_json::json!({
                                            "op": 4,
                                            "d": { "guild_id": gid, "channel_id": ch, "self_mute": self.self_mute, "self_deaf": self.self_deafen }
                                        });
                                        let _ = s.send(payload.to_string());
                                    }
                                }
                            }
                        });
                    }
                });
            });
        });

        // --- GROUP DM MEMBER PANEL
        let sel_id = self.selected_channel.clone();
        let group_channel = self.channels.iter().find(|c| Some(&c.id) == sel_id.as_ref() && c.kind == 3).cloned();
        if let (true, Some(group)) = (self.show_members, group_channel) {
            egui::Panel::right("group-members")
                .frame(Frame::new().fill(CHANNELS_BG))
                .exact_size(190.0)
                .show(ui, |ui| {
                    ui.heading(RichText::new(format!("Üyeler — {}", group.display_name())).small().strong().color(TEXT));
                    ui.separator();
                    ScrollArea::vertical().show(ui, |ui| {
                        for r in &group.recipients {
                            ui.horizontal(|ui| {
                                match self.author_avatars.get(&r.id) {
                                    Some(tex) => { ui.add(egui::Image::new(tex).max_width(34.0).max_height(34.0)); }
                                    None => {
                                        let initial = r.username.chars().next().unwrap_or('?').to_uppercase().collect::<String>();
                                        ui.label(RichText::new(initial).size(13.0).strong().color(Color32::WHITE).background_color(author_color(&r.id)));
                                    }
                                }
                                ui.label(RichText::new(r.global_name.clone().unwrap_or_else(|| r.username.clone())).size(13.0).color(TEXT));
                            });
                        }
                    });
                });
        }

        // --- MESSAGE AREA
        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(cid) = self.watching_channel.clone() {
                if let Some(entry) = self.streams.get(&cid) {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("📺 Canlı yayın — sahip: {}", entry.owner_id)).strong());
                        if ui.button("Durdur").clicked() {
                            self.watching_channel = None;
                        }
                    });
                    if let Some(tex) = &entry.preview {
                        ui.add(egui::Image::new(tex).max_height(240.0));
                    } else if entry.failed {
                        ui.label(RichText::new("önizleme alınamadı (yetki yok ya da yayın kapalı)").small().color(Color32::LIGHT_RED));
                    } else {
                        ui.label(RichText::new("önizleme yükleniyor…").small().weak());
                    }
                    ui.separator();
                }
            }
            let sel = self.selected_channel.clone();
            let is_group = self.channels.iter().find(|c| Some(&c.id) == sel.as_ref()).map(|c| c.kind == 3).unwrap_or(false);
            ui.horizontal(|ui| {
                match &sel {
                    Some(_) => {
                        ui.label(RichText::new(if self.selected_is_dm { "💬" } else { "#" }).heading());
                        ui.label(RichText::new(self.channels.iter().find(|c| Some(&c.id) == sel.as_ref()).map(|c| c.display_name()).unwrap_or_else(|| "?".into())).heading());
                    }
                    None => {
                        ui.label(RichText::new("Bir kanal seç").color(TEXT_MUTE));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if is_group && ui.button("👥 Üyeler").clicked() {
                        self.show_members = !self.show_members;
                    }
                    ui.add(TextEdit::singleline(&mut self.search).hint_text("🔍 Mesajlarda ara…").desired_width(180.0));
                    let searching = !self.search.trim().is_empty();
                    let count = if searching {
                        self.messages.iter().filter(|m| m.content.to_lowercase().contains(&self.search.trim().to_lowercase())).count()
                    } else {
                        self.messages.len()
                    };
                    ui.label(RichText::new(format!("{} mesaj", count)).small().color(TEXT_MUTE));
                });
            });
            ui.separator();

            let input_height = 40.0;
            let avail = ui.available_size();
            let searching = !self.search.trim().is_empty();
            let search_l = self.search.trim().to_lowercase();
            let filtered: Vec<&Message> = self
                .messages
                .iter()
                .filter(|m| search_l.is_empty() || m.content.to_lowercase().contains(&search_l))
                .collect();
            ScrollArea::vertical()
                .stick_to_bottom(!searching)
                .max_height((avail.y - input_height - 12.0).max(0.0))
                .show(ui, |ui| {
                    for m in &filtered {
                        ui.horizontal(|ui| {
                            let ac = author_color(&m.author.id);
                            match self.author_avatars.get(&m.author.id) {
                                Some(tex) => {
                                    ui.add(egui::Image::new(tex).max_width(40.0).max_height(40.0));
                                }
                                None => {
                                    let initial = m.author.username.chars().next().unwrap_or('?').to_uppercase().collect::<String>();
                                    ui.label(RichText::new(initial).size(20.0).strong().color(Color32::WHITE).background_color(ac));
                                }
                            }
                            ui.vertical(|ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let name_resp = ui.add(egui::Label::new(RichText::new(m.author.display_name()).strong().color(ac)).sense(egui::Sense::click()));
                                    if name_resp.clicked() {
                                        self.profile = Some(ProfileTarget {
                                            id: m.author.id.clone(),
                                            username: m.author.username.clone(),
                                            display_name: m.author.display_name(),
                                            avatar: m.author.avatar.clone(),
                                            public_flags: m.author.public_flags.unwrap_or(0),
                                        });
                                    }
                                    if let Some(av) = &m.author.avatar {
                                        if !self.author_avatars.contains_key(&m.author.id) && !self.avatar_pending.contains(&m.author.id) {
                                            self.avatar_pending.insert(m.author.id.clone());
                                            if let Some(tok) = self.token.clone() {
                                                events::avatar_fetch(tok, m.author.id.clone(), av.clone(), self.tx.clone(), ctx.clone());
                                            }
                                        }
                                    }
                                    ui.label(RichText::new(m.short_date()).small().color(TEXT_MUTE));
                                    if m.bot {
                                        ui.label(RichText::new("BOT").small().background_color(BLURPLE).color(Color32::WHITE));
                                    }
                                    if m.edited_timestamp.is_some() {
                                        ui.label(RichText::new("düzenlendi").small().color(TEXT_MUTE));
                                    }
                                });
                                ui.label(RichText::new(&m.content).size(self.font_size).color(TEXT));
                                for url in m.attachment_image_urls() {
                                    draw_media(ui, &mut self.gifs, &url, &self.tx, &ctx, 360.0);
                                }
                                for url in m.gif_urls() {
                                    draw_media(ui, &mut self.gifs, &url, &self.tx, &ctx, 360.0);
                                }
                                for att in &m.attachments {
                                    let is_video = att.get("content_type").and_then(|v| v.as_str()).map(|s| s.starts_with("video/")).unwrap_or(false)
                                        || att.get("filename").and_then(|v| v.as_str()).map(|f| [".mp4", ".webm", ".mov", ".mkv", ".avi"].iter().any(|e| f.to_lowercase().ends_with(e))).unwrap_or(false);
                                    if !is_video { continue; }
                                    let aid = att.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let url = att.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    let fname = att.get("filename").and_then(|v| v.as_str()).unwrap_or("video").to_string();
                                    ui.horizontal(|ui| {
                                        let is_playing = self.video_textures.contains_key(&aid);
                                        if ui.button(if is_playing { "⏹ Durdur" } else { "▶ Oynat" }).clicked() {
                                            if is_playing {
                                                if let Some(stop) = self.video_stops.remove(&aid) {
                                                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                                                }
                                                self.video_textures.remove(&aid);
                                            } else if let Some(tok) = &self.token {
                                                let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                                                self.video_stops.insert(aid.clone(), stop.clone());
                                                events::video_play(tok.clone(), aid.clone(), url.clone(), self.tx.clone(), ctx.clone(), stop);
                                            }
                                        }
                                        ui.label(RichText::new(fname).small().color(TEXT_MUTE));
                                    });
                                    if let Some(tex) = self.video_textures.get(&aid) {
                                        ui.add(egui::Image::new(tex).max_width(360.0));
                                    }
                                }
                                if !m.attachments.is_empty() {
                                    ui.label(RichText::new(format!("📎 {} ek", m.attachments.len())).small().color(TEXT_MUTE));
                                }
                                ui.add_space(4.0);
                            });
                        });
                        ui.add_space(4.0);
                    }
                });

            if !self.pending_files.is_empty() {
                ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for f in self.pending_files.clone() {
                            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dosya".into());
                            let size_kb = std::fs::metadata(&f).map(|m| m.len() / 1024).unwrap_or(0);
                            let resp = ui.button(RichText::new(format!("📎 {} ({}KB) ✕", name, size_kb)).size(12.0));
                            if resp.clicked() {
                                self.pending_files.retain(|p| p != &f);
                            }
                        }
                    });
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                let resp = ui.add(
                    TextEdit::singleline(&mut self.message_input)
                        .hint_text("Mesaj yaz…")
                        .desired_width(ui.available_width() - 70.0),
                );
                let send = ui.add(egui::Button::new(RichText::new("Gönder").color(Color32::WHITE)).fill(BLURPLE)).clicked()
                    || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                if send {
                    let content = self.message_input.trim().to_string();
                    if !content.is_empty() || !self.pending_files.is_empty() {
                        if let Some(cid) = &sel {
                            if let Some(tok) = &self.token {
                                let files: Vec<String> = self.pending_files.iter().map(|p| p.to_string_lossy().into_owned()).collect();
                                if files.is_empty() {
                                    if let Ok(api) = RestClient::new(tok.clone()) {
                                        events::send_message(api, cid.clone(), content, self.tx.clone(), ctx.clone());
                                    }
                                } else {
                                    events::send_message_with_files(tok.clone(), cid.clone(), content, files, self.tx.clone(), ctx.clone());
                                    self.pending_files.clear();
                                }
                                self.message_input.clear();
                            }
                        }
                    }
                }
            });
        });
    }
}
