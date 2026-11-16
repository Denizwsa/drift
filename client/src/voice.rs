//! Discord ses yolu — pycord/discord.py referanslarıyla doğrulanmış protokol.
//!
//! Akış: main gateway'de op4 VOICE_STATE_UPDATE -> dispatch VOICE_SERVER_UPDATE
//! (endpoint, token, guild_id) + VOICE_STATE_UPDATE (session_id) -> voice WS
//! (wss://<endpoint>?v=8) -> OP 0 Identify -> OP 2 Ready -> UDP IP discovery
//! (74 bayt) -> OP 1 Select Protocol -> OP 4 Session Description (secret_key)
//! -> OP 5 Speaking -> Opus + RTP + AEAD şifreleme, cpal mikrofon/hoparlör.
//!
//! RTP: 12 bayt başlık (0x80 0x78 + seq/ts/ssrc, BE). Opus'u
//! aead_xchacha20_poly1305_rtpsize ile şifrele (aad = başlık, nonce = 4 bayt BE
//! sayaç + 20 bayt sıfır, pakete 4 bayt sayaç eki). Fallback
//! xsalsa20_poly1305: nonce[:12] = başlık.
//!
//! DAVE (payload E2EE) bazı Discord çağrı türlerinde zorunlu. Bu modülde YOK:
//! DAVE gerektiren oturumda ses taşınmaz. Transport SRTP her durumda çalışır.

use crate::events::UiEvent;
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Channels};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use futures_util::{SinkExt, StreamExt};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

pub const OP_IDENTIFY: u8 = 0;
pub const OP_SELECT_PROTOCOL: u8 = 1;
pub const OP_READY: u8 = 2;
pub const OP_HEARTBEAT: u8 = 3;
pub const OP_SESSION_DESCRIPTION: u8 = 4;
pub const OP_SPEAKING: u8 = 5;
pub const OP_HELLO: u8 = 8;

const RTP_VERSION_FLAGS: u8 = 0x80;
const RTP_PAYLOAD_TYPE_OPUS: u8 = 0x78;
const SAMPLE_RATE: audiopus::SampleRate = audiopus::SampleRate::Hz48000;
const FRAME_SAMPLES: usize = 960; // 20 ms @ 48 kHz
const DETECT_LEN: usize = 74;
const AEAD_ABYTES: usize = 16;
const XSALSA_ABYTES: usize = 16;

pub struct VoiceServer {
    pub server_id: String,
    pub user_id: String,
    pub session_id: String,
    pub endpoint: String,
    pub token: String,
    pub video: bool,
}

pub fn spawn_voice(cfg: VoiceServer, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        if let Err(e) = run(cfg, tx.clone(), ctx.clone()) {
            let _ = tx.send(UiEvent::ApiError(format!("ses: {e}")));
            ctx.request_repaint();
        }
    });
}

fn run(cfg: VoiceServer, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(voice_loop(cfg, tx, ctx))
}

struct MediaState {
    ssrc: u32,
    secret_key: Option<Vec<u8>>,
    mode: Option<String>,
    seq: u16,
    timestamp: u32,
    encrypt_counter: u32,
}

impl MediaState {
    fn new() -> Self {
        Self { ssrc: 0, secret_key: None, mode: None, seq: 0, timestamp: 0, encrypt_counter: 0 }
    }
}

fn sodium_init_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        unsafe { let _ = libsodium_sys::sodium_init(); };
    });
}

fn rtp_encrypt(
    mode: &str,
    secret_key: &[u8],
    header: &[u8; 12],
    payload: &[u8],
    counter: u32,
) -> Result<Vec<u8>, String> {
    sodium_init_once();
    match mode {
        "aead_xchacha20_poly1305_rtpsize" => {
            let mut nonce = [0u8; 24];
            nonce[..4].copy_from_slice(&counter.to_be_bytes());
            let mut ct = vec![0u8; payload.len() + AEAD_ABYTES];
            let mut clen: libc::c_ulonglong = 0;
            let kr = unsafe { libsodium_sys::crypto_aead_xchacha20poly1305_ietf_encrypt(
                ct.as_mut_ptr(),
                &mut clen,
                payload.as_ptr(),
                payload.len() as libc::c_ulonglong,
                header.as_ptr(),
                header.len() as libc::c_ulonglong,
                std::ptr::null(),
                nonce.as_ptr(),
                secret_key.as_ptr(),
            ) };
            if kr != 0 {
                return Err("aead encrypt başarısız".into());
            }
            ct.truncate(clen as usize);
            let mut packet = Vec::with_capacity(12 + ct.len() + 4);
            packet.extend_from_slice(header);
            packet.extend_from_slice(&ct);
            packet.extend_from_slice(&counter.to_be_bytes());
            Ok(packet)
        }
        "xsalsa20_poly1305" => {
            let mut nonce = [0u8; 24];
            nonce[..12].copy_from_slice(header);
            let mut ct = vec![0u8; payload.len() + XSALSA_ABYTES];
            let kr = unsafe { libsodium_sys::crypto_secretbox_easy(
                ct.as_mut_ptr(),
                payload.as_ptr(),
                payload.len() as libc::c_ulonglong,
                nonce.as_ptr(),
                secret_key.as_ptr(),
            ) };
            if kr != 0 {
                return Err("secretbox encrypt başarısız".into());
            }
            let mut packet = Vec::with_capacity(12 + ct.len());
            packet.extend_from_slice(header);
            packet.extend_from_slice(&ct);
            Ok(packet)
        }
        other => Err(format!("bilinmeyen rtp mode: {other}")),
    }
}

fn rtp_decrypt(mode: &str, secret_key: &[u8], packet: &[u8]) -> Result<Vec<u8>, String> {
    sodium_init_once();
    if packet.len() < 12 + AEAD_ABYTES {
        return Err("paket kısa".into());
    }
    let header = &packet[..12];
    let payload = &packet[12..];
    match mode {
        "aead_xchacha20_poly1305_rtpsize" => {
            if payload.len() < 4 + AEAD_ABYTES {
                return Err("aead paket kısa".into());
            }
            let mut nonce = [0u8; 24];
            nonce[..4].copy_from_slice(&payload[payload.len() - 4..]);
            let ct = &payload[..payload.len() - 4];
            let mut plain = vec![0u8; ct.len() - AEAD_ABYTES];
            let mut mlen: libc::c_ulonglong = 0;
            let kr = unsafe { libsodium_sys::crypto_aead_xchacha20poly1305_ietf_decrypt(
                plain.as_mut_ptr(),
                &mut mlen,
                std::ptr::null_mut(),
                ct.as_ptr(),
                ct.len() as libc::c_ulonglong,
                header.as_ptr(),
                header.len() as libc::c_ulonglong,
                nonce.as_ptr(),
                secret_key.as_ptr(),
            ) };
            if kr != 0 {
                return Err("aead decrypt başarısız".into());
            }
            plain.truncate(mlen as usize);
            Ok(plain)
        }
        "xsalsa20_poly1305" => {
            let mut nonce = [0u8; 24];
            nonce[..12].copy_from_slice(header);
            let mut plain = vec![0u8; payload.len() - XSALSA_ABYTES];
            let kr = unsafe { libsodium_sys::crypto_secretbox_open_easy(
                plain.as_mut_ptr(),
                payload.as_ptr(),
                payload.len() as libc::c_ulonglong,
                nonce.as_ptr(),
                secret_key.as_ptr(),
            ) };
            if kr != 0 {
                return Err("secretbox open başarısız".into());
            }
            Ok(plain)
        }
        other => Err(format!("bilinmeyen rtp mode: {other}")),
    }
}

fn rtp_header(seq: u16, timestamp: u32, ssrc: u32) -> [u8; 12] {
    let mut h = [0u8; 12];
    h[0] = RTP_VERSION_FLAGS;
    h[1] = RTP_PAYLOAD_TYPE_OPUS;
    h[2..4].copy_from_slice(&seq.to_be_bytes());
    h[4..8].copy_from_slice(&timestamp.to_be_bytes());
    h[8..12].copy_from_slice(&ssrc.to_be_bytes());
    h
}

struct CaptureStream {
    _stream: cpal::Stream,
}

fn start_capture(pcm_tx: mpsc::UnboundedSender<Vec<i16>>) -> Result<CaptureStream, String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or_else(|| "ses giriş aygıtı bulunamadı".to_string())?;
    let supported = device.default_input_config().map_err(|e| format!("giriş yapılandırması: {e}"))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let err_fn = |e| eprintln!("mikrofon error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                let mut frame = Vec::with_capacity(FRAME_SAMPLES);
                for chunk in data.chunks(channels) {
                    let mixed: f32 = chunk.iter().copied().sum::<f32>() / channels as f32;
                    frame.push((mixed.clamp(-1.0, 1.0) * 32767.0) as i16);
                    if frame.len() >= FRAME_SAMPLES {
                        let _ = pcm_tx.send(std::mem::take(&mut frame));
                    }
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                let mut frame = Vec::with_capacity(FRAME_SAMPLES);
                for chunk in data.chunks(channels) {
                    let mixed: i64 = chunk.iter().map(|v| *v as i64).sum::<i64>() / channels as i64;
                    frame.push(mixed.clamp(-32768, 32767) as i16);
                    if frame.len() >= FRAME_SAMPLES {
                        let _ = pcm_tx.send(std::mem::take(&mut frame));
                    }
                }
            },
            err_fn,
            None,
        ),
        other => return Err(format!("desteklenmeyen örnek formatı: {other:?}")),
    }
    .map_err(|e| format!("giriş stream kurulamadı: {e}"))?;
    stream.play().map_err(|e| format!("giriş başlatılamadı: {e}"))?;
    Ok(CaptureStream { _stream: stream })
}

pub struct PlaybackStream {
    _stream: cpal::Stream,
    pub buffer: Arc<Mutex<VecDeque<i16>>>,
}

fn start_playback() -> Result<PlaybackStream, String> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or_else(|| "ses çıkış aygıtı bulunamadı".to_string())?;
    let supported = device.default_output_config().map_err(|e| format!("çıkış yapılandırması: {e}"))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels as usize;
    let buffer: Arc<Mutex<VecDeque<i16>>> = Arc::new(Mutex::new(VecDeque::with_capacity(FRAME_SAMPLES * 16)));
    let err_fn = |e| eprintln!("hoparlör error: {e}");

    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let buf = buffer.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    for chunk in data.chunks_mut(channels) {
                        let sample = buf.lock().ok().and_then(|mut b| b.pop_front());
                        let v = sample.unwrap_or(0) as f32 / 32768.0;
                        for s in chunk.iter_mut() {
                            *s = v;
                        }
                    }
                },
                err_fn,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let buf = buffer.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [i16], _| {
                    for chunk in data.chunks_mut(channels) {
                        let sample = buf.lock().ok().and_then(|mut b| b.pop_front());
                        let v = sample.unwrap_or(0);
                        for s in chunk.iter_mut() {
                            *s = v;
                        }
                    }
                },
                err_fn,
                None,
            )
        }
        other => return Err(format!("desteklenmeyen çıkış örnek formatı: {other:?}")),
    }
    .map_err(|e| format!("çıkış stream kurulamadı: {e}"))?;
    stream.play().map_err(|e| format!("çıkış başlatılamadı: {e}"))?;
    Ok(PlaybackStream { _stream: stream, buffer })
}

async fn voice_loop(cfg: VoiceServer, tx: std::sync::mpsc::Sender<UiEvent>, ctx: egui::Context) -> Result<(), String> {
    let url = format!("wss://{}/?v=8", cfg.endpoint);
    let mut request = url.into_client_request().map_err(|e| format!("request: {e}"))?;
    request.headers_mut().insert("Origin", tokio_tungstenite::tungstenite::http::HeaderValue::from_static("https://discord.com"));
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("voice ws: {e}"))?;
    let (mut write, mut read) = ws.split();

    let _ = tx.send(UiEvent::ApiError(format!("ses: {}'e bağlandı", cfg.endpoint)));
    ctx.request_repaint();

    let identify = if cfg.video {
        serde_json::json!({
            "op": OP_IDENTIFY,
            "d": {
                "server_id": cfg.server_id,
                "user_id": cfg.user_id,
                "session_id": cfg.session_id,
                "token": cfg.token,
                "video": true
            }
        })
    } else {
        serde_json::json!({
            "op": OP_IDENTIFY,
            "d": {
                "server_id": cfg.server_id,
                "user_id": cfg.user_id,
                "session_id": cfg.session_id,
                "token": cfg.token
            }
        })
    };
    write.send(Message::Text(identify.to_string())).await.map_err(|e| e.to_string())?;

    // mikrofon -> tokio mpsc kanalı
    let (pcm_tx, mut pcm_rx) = mpsc::unbounded_channel::<Vec<i16>>();
    let capture = start_capture(pcm_tx).map_err(|e| format!("mikrofon: {e}"))?;
    let playback = start_playback().map_err(|e| format!("hoparlör: {e}"))?;
    let speaker = playback.buffer.clone();

    let udp = tokio::net::UdpSocket::bind("0.0.0.0:0").await.map_err(|e| format!("udp bind: {e}"))?;

    let mut media = MediaState::new();

    let mut encoder = Encoder::new(SAMPLE_RATE, Channels::Mono, Application::Voip)
        .map_err(|e| format!("opus enc: {e}"))?;
    encoder.set_bitrate(audiopus::Bitrate::BitsPerSecond(64_000)).map_err(|e| e.to_string())?;

    let mut hb = tokio::time::interval(Duration::from_millis(41250));
    hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut hb_armed = false;
    let mut heartbeat_interval = Duration::from_millis(41250);
    let mut heartbeat_nonce: u64 = 0;

    let mut opus_out = vec![0u8; 1500];
    let mut opus_in = vec![0i16; FRAME_SAMPLES];
    let mut decoders: HashMap<u32, Decoder> = HashMap::new();
    let mut recv_buf = vec![0u8; 2048];
    let mut local_speaking = false;

    loop {
        tokio::select! {
            msg = read.next() => {
                let Some(msg) = msg else {
                    return Err("voice ws kapandı".into());
                };
                match msg.map_err(|e| e.to_string())? {
                    Message::Text(txt) => {
                        let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
                        let op = v["op"].as_u64().unwrap_or(255) as u8;
                        match op {
                            OP_HELLO => {
                                heartbeat_interval = Duration::from_millis(v["d"]["heartbeat_interval"].as_u64().unwrap_or(41250));
                                hb = tokio::time::interval(heartbeat_interval);
                                hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                hb_armed = true;
                                let _ = tx.send(UiEvent::ApiError(format!("ses: HELLO — heartbeat {}ms", heartbeat_interval.as_millis())));
                                ctx.request_repaint();
                            }
                            OP_READY => {
                                media.ssrc = v["d"]["ssrc"].as_u64().unwrap_or(0) as u32;
                                let ip = v["d"]["ip"].as_str().unwrap_or("").to_string();
                                let port = v["d"]["port"].as_u64().unwrap_or(0) as u16;
                                let modes: Vec<String> = v["d"]["modes"].as_array().cloned().unwrap_or_default()
                                    .into_iter().filter_map(|m| m.as_str().map(String::from)).collect();
                                let mode = if modes.iter().any(|m| m == "aead_xchacha20_poly1305_rtpsize") {
                                    "aead_xchacha20_poly1305_rtpsize".to_string()
                                } else if modes.iter().any(|m| m == "xsalsa20_poly1305") {
                                    "xsalsa20_poly1305".to_string()
                                } else {
                                    return Err(format!("sunucunun desteklemediği rtp mode: {modes:?}"));
                                };
                                udp.connect(format!("{ip}:{port}")).await.map_err(|e| format!("udp connect: {e}"))?;
                                let mut probe = [0u8; DETECT_LEN];
                                probe[0..2].copy_from_slice(&1u16.to_be_bytes());
                                probe[2..4].copy_from_slice(&70u16.to_be_bytes());
                                probe[4..8].copy_from_slice(&media.ssrc.to_be_bytes());
                                udp.send(&probe).await.map_err(|e| format!("ip probe: {e}"))?;
                                let mut buf = [0u8; DETECT_LEN];
                                let n = tokio::time::timeout(Duration::from_secs(5), udp.recv(&mut buf))
                                    .await
                                    .map_err(|_| "ip probe zaman aşımı".to_string())?
                                    .map_err(|e| format!("ip probe recv: {e}"))?;
                                if n != DETECT_LEN || buf[1] != 0x02 {
                                    return Err(format!("ip discovery yanıtı bozuk ({n} bayt)"));
                                }
                                let ext_port = u16::from_be_bytes([buf[n - 2], buf[n - 1]]);
                                let addr_bytes = &buf[8..n - 2];
                                let addr_end = addr_bytes.iter().position(|b| *b == 0).unwrap_or(addr_bytes.len());
                                let address = String::from_utf8_lossy(&addr_bytes[..addr_end]).into_owned();
                                media.mode = Some(mode.clone());
                                let select = serde_json::json!({
                                    "op": OP_SELECT_PROTOCOL,
                                    "d": {
                                        "protocol": "udp",
                                        "data": { "address": address, "port": ext_port, "mode": mode }
                                    }
                                });
                                write.send(Message::Text(select.to_string())).await.map_err(|e| e.to_string())?;
                                let _ = tx.send(UiEvent::ApiError(format!("ses: ip={address}:{ext_port} mode={mode}")));
                                ctx.request_repaint();
                            }
                            OP_SESSION_DESCRIPTION => {
                                let mode = v["d"]["mode"].as_str().unwrap_or("").to_string();
                                let secret: Vec<u8> = v["d"]["secret_key"]
                                    .as_array()
                                    .map(|a| a.iter().filter_map(|x| x.as_u64().map(|b| b as u8)).collect())
                                    .unwrap_or_default();
                                media.secret_key = Some(secret);
                                if !mode.is_empty() {
                                    media.mode = Some(mode.clone());
                                }
                                let speaking = serde_json::json!({
                                    "op": OP_SPEAKING,
                                    "d": { "speaking": 1, "delay": 0, "ssrc": media.ssrc }
                                });
                                write.send(Message::Text(speaking.to_string())).await.map_err(|e| e.to_string())?;
                                let _ = tx.send(UiEvent::ApiError(format!("ses: session açık — mode={} ssrc={}", media.mode.as_deref().unwrap_or("?"), media.ssrc)));
                                ctx.request_repaint();
                            }
                            11 => {
                                if let Ok(arr) = serde_json::from_value::<Vec<serde_json::Value>>(v["d"]["user_ids"].clone()) {
                                    let _ = tx.send(UiEvent::VoiceClients { ids: arr.iter().filter_map(|x| x.as_str().map(String::from)).collect() });
                                }
                            }
                            17 => {
                                if let (Some(uid), Some(ssrc)) = (v["d"]["user_id"].as_str(), v["d"]["ssrc"].as_u64()) {
                                    let _ = tx.send(UiEvent::VoiceSsrcMap { user_id: uid.to_string(), ssrc: ssrc as u32 });
                                }
                            }
                            _ => {}
                        }
                    }
                    Message::Ping(p) => { let _ = write.send(Message::Pong(p)).await; }
                    Message::Close(c) => {
                        let code = c.as_ref().map(|c| u16::from(c.code)).unwrap_or(0);
                        return Err(format!("voice ws kapandı (code {code})"));
                    }
                    _ => {}
                }
            }
            maybe_frame = pcm_rx.recv() => {
                let Some(frame) = maybe_frame else { break; };
                let sum: f64 = frame.iter().map(|s| (*s as f64) * (*s as f64)).sum();
                let rms = (sum / frame.len() as f64).sqrt() as f32;
                let now_speaking = rms > 800.0;
                if now_speaking != local_speaking {
                    local_speaking = now_speaking;
                    let _ = tx.send(UiEvent::SpeakingLocal { speaking: local_speaking });
                    let _ = write.send(Message::Text(serde_json::json!({
                        "op": OP_SPEAKING,
                        "d": { "speaking": if local_speaking { 1 } else { 0 }, "delay": 0, "ssrc": media.ssrc }
                    }).to_string())).await;
                }
                if let (Some(secret), Some(mode)) = (media.secret_key.clone(), media.mode.clone()) {
                    match encoder.encode(&frame, &mut opus_out) {
                        Ok(n) => {
                            media.seq = media.seq.wrapping_add(1);
                            media.timestamp = media.timestamp.wrapping_add(FRAME_SAMPLES as u32);
                            let header = rtp_header(media.seq, media.timestamp, media.ssrc);
                            match rtp_encrypt(&mode, &secret, &header, &opus_out[..n], media.encrypt_counter) {
                                Ok(bytes) => {
                                    media.encrypt_counter = media.encrypt_counter.wrapping_add(1);
                                    let _ = udp.send(&bytes).await;
                                }
                                Err(e) => { let _ = tx.send(UiEvent::ApiError(format!("ses encrypt: {e}"))); }
                            }
                        }
                        Err(e) => { let _ = tx.send(UiEvent::ApiError(format!("opus encode: {e}"))); }
                    }
                }
            }
            recv_result = udp.recv(&mut recv_buf) => {
                if let Ok(n) = recv_result {
                    let data = &recv_buf[..n];
                    if data.len() >= 28 && data[0] == RTP_VERSION_FLAGS {
                        if let (Some(secret), Some(mode)) = (media.secret_key.clone(), media.mode.clone()) {
                            let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
                            match rtp_decrypt(&mode, &secret, data) {
                                Ok(opus_payload) => {
                                    let decoder = decoders.entry(ssrc)
                                        .or_insert_with(|| Decoder::new(SAMPLE_RATE, Channels::Mono).expect("opus decoder"));
                                    let packet = audiopus::packet::Packet::try_from(opus_payload.as_slice()).ok();
                                    let signals = audiopus::MutSignals::try_from(&mut opus_in[..]).expect("mutsignals");
                                    match decoder.decode(packet, signals, false) {
                                        Ok(n) => {
                                            let rms_sq: f64 = opus_in[..n].iter().map(|s| { let v = *s as f64; v * v }).sum();
                                            if rms_sq.sqrt() / (n.max(1) as f64) > 250.0 {
                                                let _ = tx.send(UiEvent::SpeakingRemote { ssrc, speaking: true });
                                            }
                                            let mut sp = speaker.lock().unwrap();
                                            for s in &opus_in[..n] {
                                                sp.push_back(*s);
                                            }
                                            if sp.len() > FRAME_SAMPLES * 8 {
                                                let drop = sp.len() - FRAME_SAMPLES * 8;
                                                sp.drain(..drop);
                                            }
                                        }
                                        Err(e) => { let _ = tx.send(UiEvent::ApiError(format!("opus decode: {e}"))); }
                                    }
                                }
                                Err(_) => {} // RTCP vb. — atla
                            }
                        }
                    }
                }
            }
            _ = hb.tick(), if hb_armed => {
                heartbeat_nonce = heartbeat_nonce.wrapping_add(1);
                let hb_msg = serde_json::json!({ "op": OP_HEARTBEAT, "d": heartbeat_nonce });
                if let Err(e) = write.send(Message::Text(hb_msg.to_string())).await {
                    return Err(format!("voice heartbeat: {e}"));
                }
            }
        }
    }

    drop(capture);
    drop(playback);
    Ok(())
}
