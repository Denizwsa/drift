#![allow(dead_code)]
use crate::api::RestClient;
use crate::models::{Channel, Guild, Message, User};
use std::time::Duration;

pub enum UiEvent {
    Connected(String),
    Disconnected(String),
    GatewayState(String),
    Guilds(Vec<Guild>),
    GuildIcon { id: String, bytes: Vec<u8> },
    Channels { guild_id: String, channels: Vec<Channel> },
    DmChannels(Vec<Channel>),
    Messages { channel_id: String, messages: Vec<Message> },
    MessageSent { channel_id: String, message: Message },
    LiveMessage { channel_id: String, message: Message },
    UserLoaded(User),
    ApiError(String),
    VoiceStateUpdate { guild_id: Option<String>, channel_id: Option<String> },
    EmailLoginOk(String),
    EmailLoginErr(String),
    EmailLoginMfa(String),
    QrStatus(String),
    QrLink { link: String, matrix: Vec<u8>, width: usize },
    QrUser(String),
    QrToken(String),
    GifReady { url: String, frames: Vec<GifFrame> },
    GifFailed { url: String },
    StreamStarted { stream_key: String, channel_id: String, owner_id: String },
    StreamEnded { stream_key: String },
    StreamPreview { stream_key: String, image_bytes: Vec<u8> },
    StreamPreviewFailed { stream_key: String },
    AvatarLoaded { user_id: String, bytes: Vec<u8> },
    VideoFrame { id: String, width: u32, height: u32, rgba: Vec<u8> },
    VideoDone { id: String },
    DmOpened { channel: Channel },
    GuildData { guild_id: String, members: Vec<crate::models::GuildMember>, roles: Vec<crate::models::Role>, emojis: Vec<crate::models::Emoji> },
    TypingStart { channel_id: String, user_id: String },
    SpeakingLocal { speaking: bool },
    SpeakingRemote { ssrc: u32, speaking: bool },
    VoiceClients { ids: Vec<String> },
    VoiceSsrcMap { user_id: String, ssrc: u32 },
    ReactionEvent { channel_id: String, message_id: String, user_id: String, emoji_name: String, emoji_id: Option<String>, add: bool },
    GifResults { results: Vec<crate::api::GifItem>, query: String },
    GifSearchFailed { error: String },
    EmojiTexture { id: String, bytes: Vec<u8> },
}

#[derive(Clone)]
pub struct GifFrame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub delay_ms: u32,
}

pub fn gif_fetch(url: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<GifFrame>, String> {
            const MAX_BYTES: u64 = 12 * 1024 * 1024;
            let http = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) DiscordRust/0.1")
                .build()
                .map_err(|e| e.to_string())?;
            if let Some(len) = http.get(&url).send().ok().and_then(|r| r.content_length()) {
                if len > MAX_BYTES {
                    return Err(format!("gif çok büyük ({len} bayt)"));
                }
            }
            let bytes = fetch_bytes(&url)?;
            // GIF değilse statik resmin ilk karesini kullan (png/jpg/webp) — GIF decoder'da patlar
            if !bytes.starts_with(b"GIF8") {
                match image::load_from_memory(&bytes) {
                    Ok(img) => {
                        let rgba = img.into_rgba8();
                        let w = rgba.width();
                        let h = rgba.height();
                        return Ok(vec![GifFrame { rgba: rgba.into_raw(), width: w, height: h, delay_ms: 3000 }]);
                    }
                    Err(e) => return Err(format!("resim decode: {e}")),
                }
            }
            use image::AnimationDecoder;
            let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
                .map_err(|e| format!("gif decoder: {e}"))?;
            let frames = decoder
                .into_frames()
                .collect_frames()
                .map_err(|e| format!("gif frames: {e}"))?;
            let out = frames
                .into_iter()
                .take(250)
                .map(|f| {
                    let (num, den) = f.delay().numer_denom_ms();
                    let delay_ms = if den == 0 { 100 } else { (num / den).max(10) };
                    let rgba = f.into_buffer();
                    let width = rgba.width();
                    let height = rgba.height();
                    GifFrame { rgba: rgba.into_raw(), width, height, delay_ms }
                })
                .collect::<Vec<_>>();
            if out.is_empty() {
                return Err("boş gif".into());
            }
            Ok(out)
        })();
        match result {
            Ok(frames) => { let _ = tx.send(UiEvent::GifReady { url, frames }); }
            Err(e) => { let _ = tx.send(UiEvent::GifFailed { url }); let _ = e; }
        }
        ctx.request_repaint();
    });
}

pub fn stream_preview_fetch(token: String, stream_key: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<u8>, String> {
            let http = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(15))
                .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
                .build()
                .map_err(|e| e.to_string())?;
            let meta = http
                .get(format!("https://discord.com/api/v10/streams/{}/preview", stream_key))
                .header("Authorization", &token)
                .send()
                .map_err(|e| e.to_string())?;
            let st = meta.status().as_u16();
            if !meta.status().is_success() {
                return Err(format!("preview http {st}: {}", meta.status().canonical_reason().unwrap_or("?")));
            }
            let body: serde_json::Value = meta.json().map_err(|e| e.to_string())?;
            let url = body.get("url").and_then(|u| u.as_str()).ok_or_else(|| "preview url yok".to_string())?;
            let img = http.get(url).send().map_err(|e| e.to_string())?;
            if !img.status().is_success() {
                return Err(format!("preview image http {}", img.status().as_u16()));
            }
            img.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
        })();
        match result {
            Ok(bytes) => { let _ = tx.send(UiEvent::StreamPreview { stream_key, image_bytes: bytes }); }
            Err(e) => {
                eprintln!("stream preview: {e}");
                let _ = tx.send(UiEvent::StreamPreviewFailed { stream_key });
            }
        }
        ctx.request_repaint();
    });
}

pub fn avatar_fetch(token: String, user_id: String, avatar: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let bytes = match RestClient::new(token) {
            Ok(api) => api.avatar_image(&user_id, &avatar).ok(),
            Err(_) => None,
        };
        match bytes {
            Some(b) => { let _ = tx.send(UiEvent::AvatarLoaded { user_id, bytes: b }); }
            None => {}
        }
        ctx.request_repaint();
    });
}

pub fn email_login(email: String, password: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match crate::api::RestClient::login_with_email(&email, &password) {
            Ok(crate::api::LoginResult::Token(t)) => { let _ = tx.send(UiEvent::EmailLoginOk(t)); }
            Ok(crate::api::LoginResult::Mfa { ticket }) => { let _ = tx.send(UiEvent::EmailLoginMfa(ticket)); }
            Err(e) => { let _ = tx.send(UiEvent::EmailLoginErr(e)); }
        }
        ctx.request_repaint();
    });
}

pub fn mfa_totp_login(ticket: String, code: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match crate::api::RestClient::mfa_totp(&ticket, &code) {
            Ok(t) => { let _ = tx.send(UiEvent::EmailLoginOk(t)); }
            Err(e) => { let _ = tx.send(UiEvent::EmailLoginErr(e)); }
        }
        ctx.request_repaint();
    });
}

pub fn qr_start(tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    crate::qr_auth::spawn_qr_auth(tx, ctx);
}

pub fn load_guilds(client: RestClient, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match client.guilds() {
            Ok(g) => {
                let icons: Vec<(String, String)> = g
                    .iter()
                    .filter_map(|g| g.icon.as_ref().map(|i| (g.id.clone(), i.clone())))
                    .collect();
                let _ = tx.send(UiEvent::Guilds(g));
                for (id, icon) in icons {
                    match client.icon_image(&id, &icon) {
                        Ok(bytes) => {
                            let _ = tx.send(UiEvent::GuildIcon { id, bytes });
                        }
                        Err(e) => { eprintln!("icon failed: {e}"); }
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ApiError(format!("guilds: {e}")));
            }
        }
        ctx.request_repaint();
    });
}

pub fn load_channels(client: RestClient, guild_id: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match client.channels(&guild_id) {
            Ok(channels) => {
                let _ = tx.send(UiEvent::Channels { guild_id, channels });
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ApiError(format!("channels: {e}")));
            }
        }
        ctx.request_repaint();
    });
}

pub fn load_messages(client: RestClient, channel_id: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match client.messages(&channel_id, 50) {
            Ok(messages) => {
                let _ = tx.send(UiEvent::Messages { channel_id, messages });
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ApiError(format!("messages: {e}")));
            }
        }
        ctx.request_repaint();
    });
}

pub fn send_message(client: RestClient, channel_id: String, content: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match client.send_message(&channel_id, &content) {
            Ok(message) => {
                let _ = tx.send(UiEvent::MessageSent { channel_id, message });
            }
            Err(e) => {
                let _ = tx.send(UiEvent::ApiError(format!("send: {e}")));
            }
        }
        ctx.request_repaint();
    });
}

pub fn send_message_with_files(token: String, channel_id: String, content: String, files: Vec<String>, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        match RestClient::new(token).and_then(|api| api.send_with_attachments(&channel_id, &content, &files)) {
            Ok(message) => { let _ = tx.send(UiEvent::MessageSent { channel_id, message }); }
            Err(e) => { let _ = tx.send(UiEvent::ApiError(format!("yükleme hatası: {e}"))); }
        }
        ctx.request_repaint();
    });
}

pub fn video_play(token: String, id: String, url: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        let _ = token;
        // dims via ffprobe
        let probe = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", &url])
            .output();
        let (w, h) = match probe {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                let mut it = s.trim().split(',');
                let w: u32 = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(480);
                let h: u32 = it.next().and_then(|v| v.trim().parse().ok()).unwrap_or(270);
                (w, h)
            }
            Err(_) => (480, 270),
        };
        let scale = if w > 480 { format!("scale=480:-2") } else { format!("scale={w}:-2") };
        let mut child = match std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-i", &url, "-vf", &scale, "-r", "15", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(UiEvent::ApiError(format!("ffmpeg başlatılamadı: {e}")));
                ctx.request_repaint();
                return;
            }
        };
        use std::io::Read;
        let out_w = 480u32.min(w) & !1;
        let out_h = ((h as u64 * out_w as u64) / w.max(1) as u64) as u32 & !1;
        let frame_len = (out_w * out_h * 4) as usize;
        let mut buf = vec![0u8; frame_len];
        if let Some(mut stdout) = child.stdout.take() {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match stdout.read_exact(&mut buf) {
                    Ok(()) => {
                        let _ = tx.send(UiEvent::VideoFrame { id: id.clone(), width: out_w, height: out_h, rgba: buf.clone() });
                    }
                    Err(_) => break,
                }
                std::thread::sleep(Duration::from_millis(60));
            }
        }
        let _ = child.kill();
        let _ = tx.send(UiEvent::VideoDone { id });
        ctx.request_repaint();
    });
}

fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    let mut delay = 60u64;
    for attempt in 0..3 {
        match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
            .build()
            .and_then(|h| h.get(url).send())
        {
            Ok(resp) => {
                if !resp.status().is_success() {
                    return Err(format!("http {}", resp.status().as_u16()));
                }
                return resp.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string());
            }
            Err(e) => {
                if attempt < 2 { std::thread::sleep(Duration::from_millis(delay)); delay *= 3; continue; }
                return Err(format!("ağ hatası: {e}"));
            }
        }
    }
    unreachable!()
}

pub fn guild_data_fetch(token: String, guild_id: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let api = match RestClient::new(token) {
            Ok(a) => a,
            Err(e) => { let _ = tx.send(UiEvent::ApiError(format!("üyeler: {e}"))); return; }
        };
        let members = api.members(&guild_id).unwrap_or_else(|e| { eprintln!("members: {e}"); Vec::new() });
        let roles = api.roles(&guild_id).unwrap_or_else(|e| { eprintln!("roles: {e}"); Vec::new() });
        let emojis = api.emojis(&guild_id).unwrap_or_else(|e| { eprintln!("emojis: {e}"); Vec::new() });
        let _ = tx.send(UiEvent::GuildData { guild_id, members, roles, emojis });
        ctx.request_repaint();
    });
}

pub fn gif_search(token: String, query: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = RestClient::new(token).and_then(|api| api.gif_search(&query, 24));
        match result {
            Ok(results) => { let _ = tx.send(UiEvent::GifResults { results, query }); }
            Err(e) => { let _ = tx.send(UiEvent::GifSearchFailed { error: e }); }
        }
        ctx.request_repaint();
    });
}

pub fn emoji_fetch(id: String, animated: bool, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let url = if animated {
            format!("https://cdn.discordapp.com/emojis/{id}.gif?size=64")
        } else {
            format!("https://cdn.discordapp.com/emojis/{id}.png?size=64")
        };
        match fetch_bytes(&url) {
            Ok(bytes) => { let _ = tx.send(UiEvent::EmojiTexture { id, bytes }); }
            Err(e) => { eprintln!("emoji fetch: {e}"); }
        }
        ctx.request_repaint();
    });
}

pub fn trending_gifs(token: String, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = RestClient::new(token).and_then(|api| api.trending_gifs(24));
        match result {
            Ok(results) => { let _ = tx.send(UiEvent::GifResults { results, query: String::new() }); }
            Err(e) => { let _ = tx.send(UiEvent::GifSearchFailed { error: e }); }
        }
        ctx.request_repaint();
    });
}
