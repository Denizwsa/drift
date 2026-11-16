#![allow(dead_code)]
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub discriminator: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub public_flags: Option<u64>,
}

impl User {
    pub fn display_name(&self) -> String {
        self.global_name.clone().unwrap_or_else(|| self.username.clone())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub owner: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ChannelRecipient {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Channel {
    pub id: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub name: String,
    #[serde(rename = "type")]
    pub kind: u8,
    #[serde(default)]
    pub position: i64,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub recipients: Vec<ChannelRecipient>,
    #[serde(default)]
    pub last_message_id: Option<String>,
}

fn null_as_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

impl Channel {
    pub const TEXT: u8 = 0;
    pub const DM: u8 = 1;
    pub const GROUP_DM: u8 = 3;
    pub const VOICE: u8 = 2;
    pub const CATEGORY: u8 = 4;

    pub fn is_dm(&self) -> bool {
        matches!(self.kind, Self::DM | Self::GROUP_DM)
    }

    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        if !self.recipients.is_empty() {
            let names: Vec<String> = self
                .recipients
                .iter()
                .map(|r| r.global_name.clone().unwrap_or_else(|| r.username.clone()))
                .collect();
            return names.join(", ");
        }
        "DM".to_string()
    }
    pub const NEWS: u8 = 5;
    pub const THREAD_NEWS: u8 = 10;
    pub const THREAD_PUBLIC: u8 = 11;
    pub const THREAD_PRIVATE: u8 = 12;
    pub const FORUM: u8 = 15;
    pub const MEDIA: u8 = 16;

    pub fn is_text_like(&self) -> bool {
        matches!(self.kind, Self::TEXT | Self::NEWS | Self::THREAD_NEWS | Self::THREAD_PUBLIC | Self::THREAD_PRIVATE | Self::FORUM | Self::MEDIA)
    }

    pub fn is_voice(&self) -> bool {
        matches!(self.kind, Self::VOICE)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct MessageAuthor {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub public_flags: Option<u64>,
}

impl MessageAuthor {
    pub fn display_name(&self) -> String {
        self.global_name.clone().unwrap_or_else(|| self.username.clone())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    pub author: MessageAuthor,
    #[serde(default)]
    pub attachments: Vec<serde_json::Value>,
    #[serde(default)]
    pub bot: bool,
}

impl Message {
    pub fn short_time(&self) -> String {
        chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_else(|_| self.timestamp.clone())
    }

    /// Eklerden gerçekten gösterilebilecek görsel URL'ler (png/jpg/jpeg/webp/gif).
    pub fn attachment_image_urls(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.attachments {
            let url = a.get("url").and_then(|v| v.as_str()).map(str::to_string);
            let filename = a.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let ct = a.get("content_type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            if let Some(u) = url {
                if ct.starts_with("image/") || [".png", ".jpg", ".jpeg", ".webp", ".bmp", ".gif"].iter().any(|e| filename.ends_with(e)) {
                    out.push(u);
                }
            }
        }
        out
    }

    /// Mesajda geçen gerçekten oynatılabilir GIF'ler (ekler + tenor/media linkleri).
    pub fn gif_urls(&self) -> Vec<String> {
        let mut out = Vec::new();
        for a in &self.attachments {
            let url = a.get("url").and_then(|v| v.as_str()).map(str::to_string);
            let filename = a.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let content_type = a.get("content_type").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            if let Some(u) = url {
                if filename.ends_with(".gif") || content_type == "image/gif" {
                    out.push(u);
                }
            }
        }
        for raw in self.content.split_whitespace() {
            let w = raw.trim_matches(|c| matches!(c, '<' | '>' | '(' | ')' | ',' | '.' | '!' | '*' | '"' | '\''));
            if w.len() < 16 || !w.starts_with("http") {
                continue;
            }
            let path_part = w.split('?').next().unwrap_or(w).split('#').next().unwrap_or(w);
            if path_part.to_lowercase().ends_with(".gif") {
                out.push(w.to_string());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn short_date(&self) -> String {
        chrono::DateTime::parse_from_rfc3339(&self.timestamp)
            .map(|t| t.format("%d.%m %H:%M").to_string())
            .unwrap_or_else(|_| self.timestamp.clone())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CreateMessage<'a> {
    pub content: &'a str,
}

#[derive(Clone, Debug, Serialize)]
pub struct GatewayIdentify {
    pub op: u8,
    pub d: IdentifyData,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentifyData {
    pub token: String,
    pub intents: u64,
    pub properties: IdentifyProperties,
}

#[derive(Clone, Debug, Serialize)]
pub struct IdentifyProperties {
    pub os: String,
    pub browser: String,
    pub device: String,
    pub system_locale: String,
    pub browser_user_agent: String,
    pub browser_version: String,
    pub os_version: String,
    pub release_channel: String,
    pub client_build_number: u64,
    pub client_event_source: Option<String>,
}

pub const INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 12) | (1 << 15);
