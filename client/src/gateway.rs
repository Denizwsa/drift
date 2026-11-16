#![allow(dead_code)]
use crate::events::UiEvent;
use crate::models::{GatewayIdentify, IdentifyData, IdentifyProperties, INTENTS};
use flate2::Decompress;
use flate2::FlushDecompress;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::mpsc::Sender;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub fn spawn_gateway(token: String, tx: Sender<UiEvent>, ctx: egui::Context, mut voice_cmds: Option<tokio::sync::mpsc::UnboundedReceiver<String>>) {
    std::thread::spawn(move || {
        if let Err(e) = run(token, tx.clone(), ctx.clone(), voice_cmds.take()) {
            let _ = tx.send(UiEvent::GatewayState(format!("gateway error: {e}")));
            ctx.request_repaint();
        }
    });
}

fn run(token: String, tx: Sender<UiEvent>, ctx: egui::Context, voice_cmds: Option<tokio::sync::mpsc::UnboundedReceiver<String>>) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(gateway_loop(token, tx, ctx, voice_cmds))
}

async fn gateway_loop(token: String, tx: Sender<UiEvent>, ctx: egui::Context, mut voice_cmds: Option<tokio::sync::mpsc::UnboundedReceiver<String>>) -> Result<(), String> {
    let url = "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream";
    let (ws, _) = connect_async(url).await.map_err(|e| e.to_string())?;
    let (mut write, mut read) = ws.split();

    let identify = serde_json::to_string(&GatewayIdentify {
        op: 2,
        d: IdentifyData {
            token,
            intents: INTENTS,
            properties: IdentifyProperties {
                os: "linux".into(),
                browser: "Discord Rust Client".into(),
                device: "Discord Rust Client".into(),
                system_locale: std::env::var("LANG").unwrap_or_else(|_| "en-US".into()).split('.').next().unwrap_or("en-US").replace('_', "-"),
                browser_user_agent: "DiscordRust/0.1".into(),
                browser_version: "0.1.0".into(),
                os_version: "6.6.0".into(),
                release_channel: "stable".into(),
                client_build_number: 330000,
                client_event_source: None,
            },
        },
    })
    .map(|s| {
        // official clients identify with a presence block; omitting it leaves us offline until op3
        let mut v: serde_json::Value = serde_json::from_str(&s).unwrap_or_default();
        v["d"]["presence"] = serde_json::json!({
            "status": "online",
            "since": null,
            "activities": [],
            "afk": false
        });
        v.to_string()
    })
    .map_err(|e| e.to_string())?;

    let _ = tx.send(UiEvent::GatewayState("Bağlandı (gateway)".into()));
    ctx.request_repaint();

    let mut decompressor = Decompress::new(true);
    let mut zbuf: Vec<u8> = Vec::new();
    let mut seq: Option<u64> = None;
    let mut session_id: Option<String> = None;
    let mut resume_gateway_url: Option<String> = None;
    let mut heartbeat_interval = Duration::from_millis(45000);

    write.send(Message::Text(identify)).await.map_err(|e| e.to_string())?;

    let mut hb = tokio::time::interval(heartbeat_interval);
    hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // first tick fires immediately; we wait for HELLO instead for the real interval
    let mut hello_seen = false;
    let mut voice = VoiceCapture::default();

    loop {
        tokio::select! {
            msg = read.next() => {
                let Some(msg) = msg else {
                    let _ = tx.send(UiEvent::GatewayState("Bağlantı kapandı".into()));
                    ctx.request_repaint();
                    return Ok(());
                };
                match msg.map_err(|e| e.to_string())? {
                    Message::Binary(bin) => {
                        zbuf.extend_from_slice(&bin);
                        if zbuf.len() >= 4 && &zbuf[zbuf.len() - 4..] == b"\x00\x00\xff\xff" {
                            let mut out = Vec::new();
                            let before = decompressor.total_in();
                            decompressor.decompress_vec(&zbuf, &mut out, FlushDecompress::Sync).map_err(|e| e.to_string())?;
                            let consumed = (decompressor.total_in() - before) as usize;
                            zbuf.drain(..consumed.min(zbuf.len()));
                            if !out.is_empty() {
                                dispatch_frame(&String::from_utf8_lossy(&out), &mut heartbeat_interval, &mut hello_seen, &mut seq, &mut session_id, &mut resume_gateway_url, &tx, &ctx, &mut voice);
                                if hello_seen {
                                    hb = tokio::time::interval(heartbeat_interval);
                                    hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                    hb.tick().await; // arm next tick
                                }
                            }
                        }
                    }
                    Message::Text(txt) => {
                        dispatch_frame(&txt, &mut heartbeat_interval, &mut hello_seen, &mut seq, &mut session_id, &mut resume_gateway_url, &tx, &ctx, &mut voice);
                        if hello_seen {
                            hb = tokio::time::interval(heartbeat_interval);
                            hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                            hb.tick().await;
                        }
                    }
                    Message::Ping(p) => { let _ = write.send(Message::Pong(p)).await; }
                    Message::Close(c) => {
                        let _ = tx.send(UiEvent::GatewayState(format!("Kapandı: {:?} session={}", c, session_id.as_deref().unwrap_or("-"))));
                        ctx.request_repaint();
                        return Ok(());
                    }
                    _ => {}
                }
            }
            _ = hb.tick(), if hello_seen => {
                let payload = json!({"op": 1, "d": seq}).to_string();
                if let Err(e) = write.send(Message::Text(payload)).await {
                    let _ = tx.send(UiEvent::GatewayState(format!("heartbeat failed: {e}")));
                    ctx.request_repaint();
                    return Err(e.to_string());
                }
            }
            maybe_cmd = async {
                match &mut voice_cmds {
                    Some(r) => r.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                match maybe_cmd {
                    Some(c) => {
                        if let Err(e) = write.send(Message::Text(c)).await {
                            let _ = tx.send(UiEvent::GatewayState(format!("voice komut gönderilemedi: {e}")));
                            return Err(e.to_string());
                        }
                    }
                    None => {
                        voice_cmds = None;
                        let _ = tx.send(UiEvent::GatewayState("voice komut kanalı kapandı".into()));
                    }
                }
            }
        }
    }
}

#[derive(Default, Clone)]
struct VoiceCapture {
    user_id: Option<String>,
    endpoint: Option<String>,
    token: Option<String>,
    guild_id: Option<String>,
    session_id: Option<String>,
    channel_id: Option<String>,
    started: bool,
}

fn try_start_voice(voice: &mut VoiceCapture, tx: &Sender<UiEvent>, ctx: &egui::Context) {
    if voice.started {
        return;
    }
    if let (Some(endpoint), Some(token), Some(session_id), Some(user_id)) =
        (voice.endpoint.clone(), voice.token.clone(), voice.session_id.clone(), voice.user_id.clone())
    {
        voice.started = true;
        let cfg = crate::voice::VoiceServer {
            server_id: voice.guild_id.clone().unwrap_or_default(),
            user_id,
            session_id,
            endpoint,
            token,
        };
        crate::voice::spawn_voice(cfg, tx.clone(), ctx.clone());
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch_frame(
    txt: &str,
    heartbeat_interval: &mut Duration,
    hello_seen: &mut bool,
    seq: &mut Option<u64>,
    session_id: &mut Option<String>,
    resume_gateway_url: &mut Option<String>,
    tx: &Sender<UiEvent>,
    ctx: &egui::Context,
    voice: &mut VoiceCapture,
) {
    let v: Value = match serde_json::from_str(txt) {
        Ok(v) => v,
        Err(_) => return,
    };
    let op = v["op"].as_i64().unwrap_or(-1);
    if let Some(s) = v["s"].as_u64() {
        *seq = Some(s);
    }
    let t = v["t"].as_str().unwrap_or("").to_string();

    match op {
        10 => {
            let ms = v["d"]["heartbeat_interval"].as_u64().unwrap_or(45000);
            *heartbeat_interval = Duration::from_millis(ms);
            *hello_seen = true;
            // real clients: first heartbeat after HELLO waits jitter * interval
            let _ = tx.send(UiEvent::GatewayState(format!("HELLO alındı ({ms}ms)")));
        }
        11 => {}
        7 => {
            let _ = tx.send(UiEvent::GatewayState("Sunucu reconnect istedi (op 7)".into()));
        }
        9 => {
            let _ = tx.send(UiEvent::GatewayState(format!("INVALID SESSION resumable={}", v["d"].as_bool().unwrap_or(false))));
        }
        0 => match t.as_str() {
            "READY" => {
                *session_id = v["d"]["session_id"].as_str().map(String::from);
                *resume_gateway_url = v["d"]["resume_gateway_url"].as_str().map(String::from);
                voice.user_id = v["d"]["user"]["id"].as_str().map(String::from);
                if let Some(uid) = &voice.user_id {
                    let _ = tx.send(UiEvent::GatewayState(format!("READY — user={uid}")));
                }
            }
            "MESSAGE_CREATE" => {
                if let Ok(msg) = serde_json::from_value::<crate::models::Message>(v["d"].clone()) {
                    let channel_id = msg.channel_id.clone();
                    let _ = tx.send(UiEvent::LiveMessage { channel_id, message: msg });
                }
            }
            "STREAM_CREATE" => {
                let key = v["d"]["stream_key"].as_str().unwrap_or("").to_string();
                let parts: Vec<&str> = key.split(':').collect();
                let (channel_id, owner_id) = match parts.len() {
                    4 => (parts[2].to_string(), parts[3].to_string()),
                    3 => (parts[1].to_string(), parts[2].to_string()),
                    _ => (String::new(), String::new()),
                };
                if !channel_id.is_empty() {
                    let _ = tx.send(UiEvent::StreamStarted { stream_key: key, channel_id, owner_id });
                }
            }
            "STREAM_DELETE" => {
                if let Some(key) = v["d"]["stream_key"].as_str() {
                    let _ = tx.send(UiEvent::StreamEnded { stream_key: key.to_string() });
                }
            }
            "VOICE_STATE_UPDATE" => {
                if let Some(uid) = voice.user_id.as_deref() {
                    if v["d"]["user_id"].as_str() == Some(uid) {
                        voice.guild_id = v["d"]["guild_id"].as_str().map(String::from);
                        let channel_id = v["d"]["channel_id"].as_str().map(String::from);
                        voice.channel_id = channel_id.clone();
                        if let Some(sid) = v["d"]["session_id"].as_str() {
                            voice.session_id = Some(sid.to_string());
                        }
                        if channel_id.is_none() {
                            // sesten çıkıldı: sunucuyu sıfırla
                            voice.endpoint = None;
                            voice.token = None;
                            voice.started = false;
                        }
                        try_start_voice(voice, tx, ctx);
                    }
                }
            }
            "VOICE_SERVER_UPDATE" => {
                voice.endpoint = v["d"]["endpoint"].as_str().map(String::from);
                voice.token = v["d"]["token"].as_str().map(String::from);
                if let Some(g) = v["d"]["guild_id"].as_str() {
                    voice.guild_id = Some(g.to_string());
                }
                try_start_voice(voice, tx, ctx);
            }
            _ => {}
        },
        _ => {}
    }
    ctx.request_repaint();
}

pub fn save_token(token: &str) -> std::io::Result<std::path::PathBuf> {
    let dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let dir = dir.join("discord-rust");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("token");
    std::fs::write(&path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

pub fn load_token() -> Option<String> {
    let dir = dirs::config_dir()?;
    let path = dir.join("discord-rust").join("token");
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn clear_token() -> std::io::Result<()> {
    let dir = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let path = dir.join("discord-rust").join("token");
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
