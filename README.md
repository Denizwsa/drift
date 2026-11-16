# 🪽 Drift

**Discord in pure Rust. No Electron. No 600 MB of Chromium tax.**

Drift is a native Discord client written from scratch in Rust + egui. It talks to the real Discord API, renders the real Discord UI with the real Discord palette, and sips memory while your RAM stops screaming.

It started because the official client on a modest PC felt like it. So we reverse-engineered the client protocol, wrote it down, and built the thing native.

> Status: **alpha.** Text, DMs, groups, servers, messages, reactions (no, not yet), live-stream previews, inline media, voice (untested), QR + email + 2FA login. Missing: reactions, pins, threads, server settings, screen-share video, DAVE E2EE. See [Roadmap](#-roadmap) and [Reverse Engineering Notes](docs/REVERSE-ENGINEERING.md).

---

## ✨ Why Drift

| | Official Discord | Drift |
|---|---|---|
| Runtime | Electron + Chromium | Native Rust (egui) |
| Memory footprint | ~600 MB+ | ~30–80 MB typical |
| Startup | Seconds of splash | Instant |
| Login | official flows | Email+password, QR, token, TOTP 2FA |
| Protocol | official gateway/REST | same, reverse-engineered |

## 🚀 Features

- **Login three ways** — email + password (with TOTP 2FA), QR (Discord remote-auth protocol, mobile scan), or your own user token.
- **Servers & DMs** — guilds rail, channel list with categories/threads/voice channels, DM recency ordering, DM/group avatars, group member panel.
- **Messages** — 50-message history, live `MESSAGE_CREATE` over the gateway, edit markers, attachments, inline images, animated GIFs, inline video playback (silent), file upload (drag & drop anywhere).
- **Media** — images render inline, GIFs play, videos decode through FFmpeg at 15 fps.
- **Voice** — voice gateway handshake, UDP IP discovery, Opus encode/decode, `aead_xchacha20_poly1305_rtpsize` SRTP. **Untested against a live call; DAVE E2EE is not implemented.**
- **Live streams** — Go Live detection via `STREAM_CREATE`/`STREAM_DELETE`, inline preview of the screen share every ~2 s. Full video is on the roadmap; DAVE makes it hard.
- **Presence** — online/idle/dnd/invisible, pushed with gateway op 3 and on identify.
- **Notifications** — synthetic 659 Hz ping, toggleable.
- **Settings** — Görünüm (zoom/font/dark), Bildirimler, Durum, Hesap. System language detected (`tr` / `en`).

## 🏗️ Build

```bash
# FFmpeg + libopus + libsodium + ALSA dev headers required for full build
sudo apt install ffmpeg libopus-dev libsodium-dev libasound2-dev pkg-config cmake

cd client
cargo build --release
./target/release/discord-rust
```

## 🔐 Login

1. **Email + Şifre** — your Discord credentials. Discord may answer a new-device login with a captcha; use QR or token then.
2. **QR** — generates a real Discord remote-auth session; scan it with Discord on your phone (**Settings → My Account → Scan Devices**) and you're signed in. This is the exact protocol the official desktop client uses.
3. **Token** — DevTools → Network → `@me` → `Authorization` header.

Token is stored at `~/.config/discord-rust/token` with mode `0600`.

## 🔬 Reverse engineering

Drift is also an RE project. See **[docs/REVERSE-ENGINEERING.md](docs/REVERSE-ENGINEERING.md)** for:

- How the desktop client actually works (thin Electron shell + a 12 MB remote JS bundle, pulled from the disk cache).
- The gateway protocol (`v10`, zlib-stream transport, opcodes, heartbeat jitter, resume).
- Identify payload shape for user clients (`installation_id`, `is_fast_connect`, `gateway_connect_reasons`).
- Voice protocol (op4 → `VOICE_SERVER_UPDATE` → voice WS → `session_description` → SRTP aead).
- Go Live / screen share flow (`STREAM_WATCH` op 20, `rtc_server_id`, DAVE/E2EE boundary).
- Toolchain: [REA](https://github.com/morluto/rea) (MCP reverse-engineering agent plugin, OpenCode integration), `asar`, Ghidra-less JS static analysis.

## ⚖️ Legal

Drift is **not affiliated with Discord**. Discord is a trademark of Discord Inc. Drift is an independent client built from publicly observable protocol behavior, for interoperability/research. Using a selfbot/custom client violates Discord's ToS; use your own account at your own risk. No Discord assets, binaries, or bundle code are included in this repository.

## 🗺️ Roadmap

- Reactions, pins, thread support, markdown-heavy rendering
- Server-side message search
- Screen-share **video** (WebRTC + DAVE E2EE; the hard part)
- Voice **incoming audio** (Opus decode wired, untested; DAVE missing)
- Read states / unread badges
- Better i18n (currently `tr`/`en`, system locale)

## 📄 License

MIT — see [LICENSE](LICENSE).
---
<div align="center">

[![Buy me a coffee](https://img.buymeacoffee.com/button-api/?text=Buy%20me%20a%20coffee&emoji=%E2%98%95&slug=denizkwsa&button_colour=FFDD00&font_colour=000000&font_family=Cookie&outline_colour=000000&coffee_colour=ffffff)](https://buymeacoffee.com/denizkwsa)

</div>
