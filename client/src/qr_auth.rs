//! Discord remote-auth (QR login) akışı.
//! wss://remote-auth-gateway.discord.gg/?v=2  + Origin: https://discord.com
//!
//! hello -> RSA-2048 anahtar çifti -> init(SPKI, base64) -> nonce_proof ->
//! pending_remote_init (fingerprint) -> [QR taranır] -> pending_ticket (kullanıcı
//! bilgisi) -> pending_login (ticket) -> POST /users/@me/remote-auth/login ->
//! encrypted_token -> OAEP decrypt -> token.
//!
//! Tüm kripto PCKS#1 OAEP + SHA-256. Dokümantasyon: docs.discord.food/remote-authentication.

use crate::api::{oaep_decrypt, RestClient};
use crate::events::UiEvent;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rand::rngs::OsRng;
use rsa::pkcs8::spki::EncodePublicKey;
use rsa::RsaPrivateKey;
use sha2::{Digest, Sha256};
use std::sync::mpsc::Sender;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

pub fn spawn_qr_auth(tx: Sender<UiEvent>, ctx: egui::Context) {
    std::thread::spawn(move || {
        if let Err(e) = run(tx.clone(), ctx.clone()) {
            let _ = tx.send(UiEvent::QrStatus(format!("QR hatası: {e}")));
            ctx.request_repaint();
        }
    });
}

fn run(tx: Sender<UiEvent>, ctx: egui::Context) -> Result<(), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(qr_loop(tx, ctx))
}

async fn qr_loop(tx: Sender<UiEvent>, ctx: egui::Context) -> Result<(), String> {
    let url = "wss://remote-auth-gateway.discord.gg/?v=2";
    let mut request = url.into_client_request().map_err(|e| format!("request: {e}"))?;
    request.headers_mut().insert("Origin", tokio_tungstenite::tungstenite::http::HeaderValue::from_static("https://discord.com"));
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("connect: {e}"))?;
    let (mut write, mut read) = ws.split();

    let _ = tx.send(UiEvent::QrStatus("Remote-auth gateway'e bağlandı, anahtar üretiliyor…".into()));
    ctx.request_repaint();

    // RSA-2048 anahtar çifti (her QR oturumunda yeni)
    let mut rng = OsRng;
    let private_key = RsaPrivateKey::new(&mut rng, 2048).map_err(|e| format!("rsa: {e}"))?;
    let public_spki = private_key
        .to_public_key()
        .to_public_key_der()
        .map_err(|e| format!("spki: {e}"))?
        .as_bytes()
        .to_vec();
    let encoded_public_key = base64::engine::general_purpose::STANDARD.encode(&public_spki);
    let fingerprint_bytes = Sha256::digest(&public_spki);
    let fingerprint = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(fingerprint_bytes);

    let mut hb = tokio::time::interval(Duration::from_millis(41250));
    hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // init sadece HELLO geldikten sonra gönderilir
    // (sıra: hello -> init)
    loop {
        tokio::select! {
            msg = read.next() => {
                let Some(msg) = msg else {
                    return Err("gateway kapandı".into());
                };
                match msg.map_err(|e| e.to_string())? {
                    Message::Text(txt) => {
                        let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
                        match v["op"].as_str().unwrap_or("") {
                            "hello" => {
                                let ms = v["heartbeat_interval"].as_u64().unwrap_or(41250);
                                hb = tokio::time::interval(Duration::from_millis(ms));
                                hb.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                                let init = serde_json::json!({ "op": "init", "encoded_public_key": encoded_public_key });
                                write.send(Message::Text(init.to_string())).await.map_err(|e| e.to_string())?;
                                let _ = tx.send(UiEvent::QrStatus("Public key gönderildi, nonce bekleniyor…".into()));
                            }
                            "nonce_proof" => {
                                let enc = v["encrypted_nonce"].as_str().ok_or("encrypted_nonce yok")?.to_string();
                                let nonce = oaep_decrypt(&private_key, &enc)?;
                                let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&nonce);
                                let msg = serde_json::json!({ "op": "nonce_proof", "nonce": proof });
                                write.send(Message::Text(msg.to_string())).await.map_err(|e| e.to_string())?;
                                let _ = tx.send(UiEvent::QrStatus("Nonce proof gönderildi, fingerprint bekleniyor…".into()));
                            }
                            "pending_remote_init" => {
                                let fp = v["fingerprint"].as_str().ok_or("fingerprint yok")?.to_string();
                                if fp != fingerprint {
                                    return Err(format!("fingerprint eşleşmiyor (güvenlik durdu): gateway={fp} yerel={fingerprint}"));
                                }
                                let link = format!("https://discord.com/ra/{fp}");
                                let code = qrcode::QrCode::new(link.as_bytes()).map_err(|e| format!("qr: {e}"))?;
                                let matrix: Vec<u8> = code.to_colors().iter().map(|c| u8::from(*c == qrcode::Color::Dark)).collect();
                                let width = code.width();
                                let _ = tx.send(UiEvent::QrLink { link, matrix, width });
                                let _ = tx.send(UiEvent::QrStatus("QR'ı telefonunda Discord ile tarat: Ayarlar → Hesabım → Cihazları Tara".into()));
                            }
                            "pending_ticket" => {
                                let enc = v["encrypted_user_payload"].as_str().ok_or("encrypted_user_payload yok")?.to_string();
                                let bytes = oaep_decrypt(&private_key, &enc)?;
                                let payload = String::from_utf8_lossy(&bytes).into_owned();
                                // format: id:discriminator:avatar:username
                                let mut parts = payload.splitn(4, ':');
                                let _id = parts.next().unwrap_or("");
                                let _disc = parts.next().unwrap_or("");
                                let _avatar = parts.next().unwrap_or("");
                                let username = parts.next().unwrap_or("?").to_string();
                                let _ = tx.send(UiEvent::QrUser(username));
                                let _ = tx.send(UiEvent::QrStatus("Telefonunda isteği onayla…".into()));
                            }
                            "pending_login" => {
                                let ticket = v["ticket"].as_str().ok_or("ticket yok")?.to_string();
                                let token = RestClient::remote_auth_login(&ticket, &private_key)?;
                                let _ = tx.send(UiEvent::QrToken(token));
                                let _ = tx.send(UiEvent::QrStatus("Giriş yapıldı.".into()));
                                return Ok(());
                            }
                            "cancel" => {
                                return Err("mobil cihaz QR oturumunu iptal etti".into());
                            }
                            "heartbeat_ack" => {}
                            other => {
                                let _ = tx.send(UiEvent::QrStatus(format!("gateway: {other}")));
                            }
                        }
                        ctx.request_repaint();
                    }
                    Message::Ping(p) => { let _ = write.send(Message::Pong(p)).await; }
                    Message::Close(c) => {
                        let code = c.as_ref().map(|c| c.code).map(|c| u16::from(c)).unwrap_or(0);
                        return Err(format!("gateway kapandı (code {code})"));
                    }
                    _ => {}
                }
            }
            _ = hb.tick() => {
                let _ = write.send(Message::Text(r#"{"op":"heartbeat"}"#.to_string())).await;
            }
        }
    }
}
