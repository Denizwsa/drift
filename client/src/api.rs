#![allow(dead_code)]
use crate::models::{Channel, CreateMessage, Guild, Message, User};
use std::time::Duration;

const API_BASE: &str = "https://discord.com/api/v10";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

pub enum LoginResult {
    Token(String),
    Mfa { ticket: String },
}

#[derive(Clone)]
pub struct RestClient {
    http: reqwest::blocking::Client,
    pub token: String,
}

impl RestClient {
    pub fn new(token: String) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(UA)
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        Ok(Self { http, token })
    }

    fn auth(&self, rb: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        rb.header("Authorization", &self.token)
            .header("Content-Type", "application/json")
    }

    fn err_body(resp: reqwest::blocking::Response, status: u16) -> String {
        let body = resp.text().unwrap_or_default();
        if body.contains("captcha") {
            return format!("captcha gerekli (Discord yeni istemcilere zorluk çıkartıyor). Token ile giriş yap.\n{body}");
        }
        format!("http {status}: {}", trunc(&body, 300))
    }

    fn retry<F>(&self, build: F) -> Result<reqwest::blocking::Response, String>
    where
        F: Fn(&reqwest::blocking::Client) -> reqwest::blocking::RequestBuilder,
    {
        let mut delay_ms = 60u64;
        for attempt in 0..4 {
            match build(&self.http).send() {
                Ok(r) => return Ok(r),
                Err(e) => {
                    if attempt < 3 {
                        std::thread::sleep(Duration::from_millis(delay_ms));
                        delay_ms *= 3;
                        continue;
                    }
                    return Err(format!("ağ hatası ({attempt} deneme): {e}"));
                }
            }
        }
        unreachable!()
    }

    pub fn login_with_email(email: &str, password: &str) -> Result<LoginResult, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(UA)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = http
            .post(format!("{API_BASE}/auth/login"))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "login": email, "password": password }))
            .send()
            .map_err(|e| e.to_string())?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        let body: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        if let Some(token) = body.get("token").and_then(|t| t.as_str()) {
            return Ok(LoginResult::Token(token.to_string()));
        }
        if body.get("mfa").and_then(|m| m.as_bool()) == Some(true) {
            if let Some(ticket) = body.get("ticket").and_then(|t| t.as_str()) {
                return Ok(LoginResult::Mfa { ticket: ticket.to_string() });
            }
        }
        Err(format!("beklenmeyen login yanıtı: {}", trunc(&body.to_string(), 200)))
    }

    pub fn mfa_totp(ticket: &str, code: &str) -> Result<String, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(UA)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = http
            .post(format!("{API_BASE}/auth/mfa/totp"))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "ticket": ticket, "code": code }))
            .send()
            .map_err(|e| e.to_string())?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        let body: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        body.get("token")
            .and_then(|t| t.as_str())
            .map(String::from)
            .ok_or_else(|| format!("beklenmeyen mfa yanıtı: {}", trunc(&body.to_string(), 200)))
    }

    pub fn remote_auth_login(ticket: &str, private_key: &rsa::RsaPrivateKey) -> Result<String, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent(UA)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = http
            .post(format!("{API_BASE}/users/@me/remote-auth/login"))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "ticket": ticket }))
            .send()
            .map_err(|e| e.to_string())?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        let body: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        let enc = body.get("encrypted_token").and_then(|t| t.as_str()).ok_or("encrypted_token yok")?;
        let bytes = oaep_decrypt(private_key, enc)?;
        String::from_utf8(bytes).map_err(|e| format!("token utf8: {e}"))
    }

    pub fn me(&self) -> Result<User, String> {
        let resp = self.retry(|h| h.get(format!("{API_BASE}/users/@me")).header("Authorization", &self.token))?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        resp.json::<User>().map_err(|e| e.to_string())
    }

    pub fn guilds(&self) -> Result<Vec<Guild>, String> {
        let resp = self.retry(|h| h.get(format!("{API_BASE}/users/@me/guilds")).header("Authorization", &self.token))?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        resp.json::<Vec<Guild>>().map_err(|e| e.to_string())
    }

    pub fn channels(&self, guild_id: &str) -> Result<Vec<Channel>, String> {
        let resp = self.retry(|h| h.get(format!("{API_BASE}/guilds/{guild_id}/channels")).header("Authorization", &self.token))?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        let mut v = resp.json::<Vec<Channel>>().map_err(|e| e.to_string())?;
        v.sort_by_key(|c| c.position);
        Ok(v)
    }

    pub fn dm_channels(&self) -> Result<Vec<Channel>, String> {
        let resp = self.retry(|h| h.get(format!("{API_BASE}/users/@me/channels")).header("Authorization", &self.token))?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        resp.json::<Vec<Channel>>().map_err(|e| e.to_string())
    }

    pub fn messages(&self, channel_id: &str, limit: u8) -> Result<Vec<Message>, String> {
        let resp = self.retry(|h| h.get(format!("{API_BASE}/channels/{channel_id}/messages?limit={limit}")).header("Authorization", &self.token))?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        let mut v = resp.json::<Vec<Message>>().map_err(|e| e.to_string())?;
        v.reverse(); // api returns newest first; display oldest first
        Ok(v)
    }

    pub fn send_message(&self, channel_id: &str, content: &str) -> Result<Message, String> {
        let body = CreateMessage { content };
        let resp = self.retry(|h| {
            h.post(format!("{API_BASE}/channels/{channel_id}/messages"))
                .header("Authorization", &self.token)
                .json(&body)
        })?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        resp.json::<Message>().map_err(|e| e.to_string())
    }

    pub fn send_with_attachments(&self, channel_id: &str, content: &str, files: &[String]) -> Result<Message, String> {
        let token = self.token.clone();
        let files_owned: Vec<String> = files.to_vec();
        let resp = self.retry(|h| {
            let mut form = reqwest::blocking::multipart::Form::new();
            if !content.is_empty() {
                form = form.text("payload_json", serde_json::json!({ "content": content }).to_string());
            } else {
                form = form.text("payload_json", "{}".to_string());
            }
            for (i, path) in files_owned.iter().enumerate() {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("file{i}"));
                let part = reqwest::blocking::multipart::Part::file(path)
                    .map(|p| p.file_name(name));
                if let Ok(part) = part {
                    form = form.part(format!("files[{i}]"), part);
                }
            }
            h.post(format!("{API_BASE}/channels/{channel_id}/messages"))
                .header("Authorization", &token)
                .multipart(form)
        })?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        resp.json::<Message>().map_err(|e| e.to_string())
    }

    pub fn open_dm(&self, user_id: &str) -> Result<Channel, String> {
        let resp = self.retry(|h| {
            h.post(format!("{API_BASE}/users/@me/channels"))
                .header("Authorization", &self.token)
                .json(&serde_json::json!({ "recipient_id": user_id }))
        })?;
        let st = resp.status().as_u16();
        if !resp.status().is_success() {
            return Err(Self::err_body(resp, st));
        }
        resp.json::<Channel>().map_err(|e| e.to_string())
    }

    pub fn icon_image(&self, guild_id: &str, icon: &str) -> Result<Vec<u8>, String> {
        let url = format!("https://cdn.discordapp.com/icons/{guild_id}/{icon}.png?size=64");
        let resp = self.http.get(&url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("icon http {}", resp.status().as_u16()));
        }
        resp.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
    }

    pub fn avatar_image(&self, user_id: &str, avatar: &str) -> Result<Vec<u8>, String> {
        let url = format!("https://cdn.discordapp.com/avatars/{user_id}/{avatar}.png?size=64");
        let resp = self.http.get(&url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("avatar http {}", resp.status().as_u16()));
        }
        resp.bytes().map(|b| b.to_vec()).map_err(|e| e.to_string())
    }
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "..."
    }
}

pub fn oaep_decrypt(private_key: &rsa::RsaPrivateKey, b64_ciphertext: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    use rsa::Oaep;
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_ciphertext)
        .map_err(|e| format!("base64: {e}"))?;
    private_key
        .decrypt(Oaep::new::<sha2::Sha256>(), &key_bytes)
        .map_err(|e| format!("oaep decrypt: {e}"))
}
