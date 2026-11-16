# Reverse Engineering Notes — Discord Desktop → Native Rust

What we learned, where things live, and what the client actually is.

## Architecture of the official client

The Linux desktop client (`~/.config/discord/app-<version>/`) is a **thin Electron shell**:

- `resources/app.asar` (~3 MB) — bootstrap/updater only. Bundles `@sentry`, `@electron/remote`-style modules, and the splash screen (`splash/index.html`).
- `modules/discord_desktop_core-1/discord_desktop_core/core.asar` (~1.6 MB) — the main-process shell, spellcheck, RPC, native bindings.
- `modules/discord_voice-1/discord_voice/discord_voice.node` — native voice pipeline (Opus + audio).
- `modules/discord_zstd-1/discord_zstd.node` — zstd compression for the gateway.
- Native media/crypto: `discord_krisp`, `discord_utils`, `discord_erlpack`, `discord_zstd`, `discord_voice`.

The **actual UI logic is not in these files**. At runtime the shell fetches the web bundle from Discord's CDN:

- `https://discord.com/assets/web.<hash>.js` (12 MB, uncompressed in the disk cache)
- Disk cache: `~/.config/discord/Cache/Cache_Data/<key>` — format `"1/0/" + url + "?cdt=1" + body`. The `?cdt=1` marker is where the URL key ends and the body begins.

Extracting it:

```bash
# pull the cached bundle, carve from the "?cdt=1" marker (strip the \x00 padding)
```

The bundle is webpack/rspack output full of module IDs and mangled names, but every string, opcode and payload shape is readable.

## Gateway protocol (main)

`wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream`

- Payload: `{ "op": N, "d": ..., "s": seq?, "t": EVENT? }`
- HELLO (op 10) → heartbeat interval (default 45 s); heartbeats with jitter (`heartbeat_interval * rand[0,1]`), `op: 1, d: last_seq`.
- IDENTIFY (op 2) for **user** sessions includes `token`, full `intents` bitmask, and — critically — `properties`: `os`, `browser`, `device`, `system_locale`, `browser_user_agent`, `browser_version`, `os_version`, `release_channel`, `client_build_number`, `client_event_source`, plus `installation_id`, `gateway_connect_reasons`, `is_fast_connect: false`, `presence`.
- READY (op 0) → `session_id`, `resume_gateway_url`, guilds (+`data_mode: partial|full_sync` per guild), `auth_endpoint`.
- Compression: `zlib-stream` — shared inflate context per connection, sync-flush boundary `00 00 ff ff`.
- Close codes: 4004 (bad token), 4014 (privileged intent), 4000 (invalid version), 1000 (clean).

## Voice protocol

1. UI dispatches op4 `VOICE_STATE_UPDATE` → gateway emits `VOICE_SERVER_UPDATE` (`endpoint`, `token`, `guild_id`) and `VOICE_STATE_UPDATE` (`session_id`).
2. Open voice WS to `wss://<endpoint>/?v=8` (Origin: `https://discord.com`).
3. OP 0 Identify `{ server_id, user_id, session_id, token }`.
4. OP 2 Ready → `ssrc`, `ip`, `port`, `modes` (`aead_xchacha20_poly1305_rtpsize`, fallback `xsalsa20_poly1305`); then **UDP IP discovery** — 74-byte request: `type=0x0001, len=0x0046, ssrc=big-endian, 64 zero bytes`; response gives external IP/port.
5. OP 1 Select Protocol → OP 4 Session Description (`secret_key` 32 bytes).
6. Media: 12-byte RTP header (`0x80 0x78` + seq + ts + ssrc, big-endian), Opus payload, `aead_xchacha20_poly1305_rtpsize` (AAD = header, nonce = 4-byte BE counter + 20 zero bytes, 4-byte counter appended to the packet). IP-discovery format confirmed by discord-api-docs issue #2814 (74-byte request, not the documented 70).

DAVE (payload MLS E2EE) is layered on top of SRTP for modern calls; implementing it means an OpenMLS-derived state machine plus the DAVE transforms — `davey` (Rust) exists but Drift does not implement it yet.

## Go Live / screen share

Voice WS commands: `STREAM_CREATE` op 18, `STREAM_DELETE` op 19, `STREAM_WATCH` op 20, `STREAM_PING` op 21, `STREAM_SET_PAUSED` op 22, `REQUEST_GUILD_*` 23+.

Main gateway dispatches: `STREAM_CREATE`, `STREAM_SERVER_UPDATE` (`stream_key` → `{ guild_id, channel_id, owner_id }`), `STREAM_UPDATE`, `STREAM_DELETE`.

`GET /api/v10/streams/{stream_key}/preview` returns `{ "url": <cdn image> }` — Drift polls it ~every 2 s for the inline "watch". Real video requires WebRTC/ICE/DTLS-SRTP (or Discord's own SFU path) + VP9/VP8/H264 decode + **DAVE**. That is the largest remaining work item.

## Email + TOTP login (official web flow)

```
POST /api/v10/auth/login
{ "login": email, "password": pass }
→ 200 { token }                     # no 2FA
→ 200 { mfa: true, ticket }         # TOTP required
POST /api/v10/auth/mfa/totp  { ticket, code }  → { token }
```

Discord may return a captcha block for new devices; QR is the reliable path there.

## QR remote auth (official desktop flow)

`wss://remote-auth-gateway.discord.gg/?v=2`, `Origin: https://discord.com`

1. HELLO (interval ~41.25 s, timeout ~10 min).
2. Client → RSA-2048 keypair; `init { encoded_public_key: base64(SPKI DER) }`.
3. `nonce_proof` ← base64(encrypted nonce), OAEP-decrypt with SHA-256, send base64url(nonce) as `nonce_proof`.
4. `pending_remote_init { fingerprint }` — must equal `base64url(SHA-256(SPKI))`.
5. QR encodes `https://discord.com/ra/<fingerprint>`. User scans with the phone → `pending_ticket` (encrypted `user_payload: "id:disc:avatar:username"`) → `pending_login { ticket }`.
6. `POST /api/v10/users/@me/remote-auth/login { ticket }` → `{ encrypted_token }` → OAEP-decrypt → user token.

## Tooling

- [REA](https://github.com/morluto/rea) (set up as an OpenCode Skill + MCP server for this investigation) — `analyze_javascript_application`, module views, feature tracing; CLI fallback `rea analyze-javascript-application --json`.
- `asar` (npm) for extraction; `grep`/python for string archaeology; FFmpeg for media probing/playback.

Prefer **shipped-artifact evidence over repository lore**; treat everything above as versioned against build `1.0.169` (`sentryRelease: discord-desktop-2fa3c2c144...`).
