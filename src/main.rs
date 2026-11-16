use flate2::{read::ZlibDecoder, Decompress, FlushDecompress};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    env,
    fs::{File, OpenOptions},
    io::{Read, Write},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};

struct State {
    seq: Option<u64>,
    shared_seq: Arc<Mutex<Option<u64>>>,
    shared_iv: Arc<Mutex<Option<Duration>>>,
    out: mpsc::UnboundedSender<Message>,
    file: File,
    identify: String,
    identified: bool,
}

fn trunc(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "..."
    }
}

impl State {
    fn handle(&mut self, txt: &str) -> Result<(), Box<dyn std::error::Error>> {
        let v: Value = match serde_json::from_str(txt) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        let op = v["op"].as_i64().unwrap_or(-1);
        if let Some(s) = v["s"].as_u64() {
            self.seq = Some(s);
        }
        let t = v["t"].as_str().unwrap_or("").to_string();

        let rec = json!({
            "ts": chrono::Utc::now().to_rfc3339(),
            "dir": "IN",
            "op": op,
            "t": t,
            "raw": v
        });
        self.file.write_all(rec.to_string().as_bytes())?;
        self.file.write_all(b"\n")?;
        println!("IN  op={:>2} t={:<28} {}", op, t, trunc(txt, 160));

        match op {
            10 => {
                let ms = v["d"]["heartbeat_interval"].as_u64().unwrap_or(45000);
                *self.shared_iv.lock().unwrap() = Some(Duration::from_millis(ms));
                println!("    hello: heartbeat_interval={}ms", ms);
                if !self.identified {
                    self.out.send(Message::Text(self.identify.clone()))?;
                    self.identified = true;
                    println!("    -> IDENTIFY sent");
                }
            }
            1 => {
                self.out
                    .send(Message::Text(json!({"op": 1, "d": self.seq}).to_string()))?;
                println!("    heartbeat requested -> sent");
            }
            7 => println!("    RECONNECT requested"),
            9 => println!(
                "    INVALID SESSION resumable={}",
                v["d"].as_bool().unwrap_or(false)
            ),
            0 if t == "READY" => println!(
                "    READY session={} user={}#{}",
                v["d"]["session_id"].as_str().unwrap_or("?"),
                v["d"]["user"]["username"].as_str().unwrap_or("?"),
                v["d"]["user"]["discriminator"].as_str().unwrap_or("?")
            ),
            _ => {}
        }
        *self.shared_seq.lock().unwrap() = self.seq;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let url = env::var("GW_URL")
        .unwrap_or_else(|_| "wss://gateway.discord.gg/?v=10&encoding=json".into());
    let token = env::var("DISCORD_TOKEN")?;
    let zlib_stream = url.contains("compress=zlib-stream");

    let mut identify: Value = match std::fs::read_to_string("identify.json") {
        Ok(s) => serde_json::from_str(&s)?,
        Err(_) => json!({
            "op": 2,
            "d": {
                "token": token,
                "intents": 2147483647u64,
                "properties": {
                    "os": "linux",
                    "browser": "Discord Client",
                    "device": "",
                    "system_locale": "en-US",
                    "browser_user_agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
                    "browser_version": "121.0.0.0",
                    "os_version": "6.6.0",
                    "release_channel": "stable",
                    "client_build_number": 330000,
                    "client_event_source": null
                },
                "presence": null
            }
        }),
    };
    if let Some(d) = identify.get_mut("d") {
        if d.get("token").is_none() {
            d["token"] = json!(token);
        }
        if zlib_stream {
            d["compress"] = json!(false);
        }
    }
    let identify_str = serde_json::to_string(&identify)?;

    let (ws, _) = connect_async(url.as_str()).await?;
    let (mut write, mut read) = ws.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();

    let shared_seq = Arc::new(Mutex::new(None::<u64>));
    let shared_iv = Arc::new(Mutex::new(None::<Duration>));

    let hb_seq = shared_seq.clone();
    let hb_iv = shared_iv.clone();
    let hb_tx = out_tx.clone();
    tokio::spawn(async move {
        loop {
            let iv = { *hb_iv.lock().unwrap() };
            if iv.is_some() {
                break;
            }
            sleep(Duration::from_millis(30)).await;
        }
        let jitter = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
            % 1000;
        let first = hb_iv
            .lock()
            .unwrap()
            .unwrap_or(Duration::from_secs(45));
        sleep(first.mul_f64(jitter as f64 / 1000.0)).await;
        loop {
            let s = { *hb_seq.lock().unwrap() };
            let payload = json!({"op": 1, "d": s}).to_string();
            if hb_tx.send(Message::Text(payload)).is_err() {
                break;
            }
            let iv = hb_iv
                .lock()
                .unwrap()
                .unwrap_or(Duration::from_secs(45));
            sleep(iv).await;
        }
    });

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("frames.jsonl")?;
    let mut state = State {
        seq: None,
        shared_seq: shared_seq.clone(),
        shared_iv: shared_iv.clone(),
        out: out_tx.clone(),
        file,
        identify: identify_str,
        identified: false,
    };

    let mut decompress = Decompress::new(true);
    let mut zbuf: Vec<u8> = Vec::new();

    println!("connected: {}", url);
    println!("logging to frames.jsonl");

    loop {
        tokio::select! {
            incoming = read.next() => {
                let Some(msg) = incoming else { break; };
                match msg? {
                    Message::Text(txt) => state.handle(&txt)?,
                    Message::Binary(bin) => {
                        if zlib_stream {
                            zbuf.extend_from_slice(&bin);
                            if zbuf.len() >= 4 && &zbuf[zbuf.len() - 4..] == b"\x00\x00\xff\xff" {
                                let mut out = Vec::new();
                                let before = decompress.total_in();
                                decompress.decompress_vec(&zbuf, &mut out, FlushDecompress::Sync)?;
                                let consumed = (decompress.total_in() - before) as usize;
                                zbuf.drain(..consumed.min(zbuf.len()));
                                if !out.is_empty() {
                                    state.handle(&String::from_utf8_lossy(&out))?;
                                }
                            }
                        } else {
                            let mut s = String::new();
                            if ZlibDecoder::new(&bin[..]).read_to_string(&mut s).is_ok() && !s.is_empty() {
                                state.handle(&s)?;
                            } else {
                                state.handle(&String::from_utf8_lossy(&bin))?;
                            }
                        }
                    }
                    Message::Ping(p) => { out_tx.send(Message::Pong(p))?; }
                    Message::Close(c) => { println!("[CLOSE] {:?}", c); break; }
                    _ => {}
                }
            }
            outgoing = out_rx.recv() => {
                match outgoing {
                    Some(m) => {
                        let label = match &m {
                            Message::Text(t) => trunc(t, 160),
                            other => format!("{:?}", other),
                        };
                        println!("OUT {}", label);
                        write.send(m).await?;
                    }
                    None => break,
                }
            }
        }
    }
    Ok(())
}
