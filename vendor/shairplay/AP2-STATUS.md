# AirPlay 2 — Implementation Status & Research

## Complete

| Feature | Stream Type | Details |
|---------|-------------|---------|
| mDNS discovery | — | `_airplay._tcp` + `_raop._tcp`, `et=0,3,5` |
| SRP-6a transient pairing | — | PIN 3939, automatic, no persistence |
| Normal HomeKit pairing | — | Configurable PIN, PairingStore key persistence |
| Encrypted RTSP transport | — | ChaCha20-Poly1305, HKDF-SHA512 key derivation |
| FairPlay handshake | — | Full fp-setup M1/M2 |
| PTP timing | — | Sync/Follow_Up/Announce selection + Delay_Req/Resp path delay; RTCP time announces feed playout anchors (no NQPTP) |
| Buffered audio | 103 | AAC (symphonia) **and ALAC** (crate decoder) decode, format negotiated per RTP SSRC, per-packet ChaCha20 decrypt |
| Multichannel | 103 | 5.1/7.1 AAC → stereo mixdown (ITU-R BS.775) |
| Resampling | 103 | rubato StreamResampler, any rate → output rate |
| Timed playout buffer | 103 | Pause/resume/flush, stale frame discard, 64-bit wrap-aware RTP timeline |
| Metadata forwarding | — | Volume, artwork, progress, DMAP track info |
| SETMAGICCOOKIE | — | `POST /SETMAGICCOOKIE` and bare RTSP method acknowledged (iOS/macOS 27) |
| RTCP control channel | 103/130 | Per-connection UDP control port listened to; time announce parsed |
| Group peers | — | SETPEERS/SETPEERSX values recorded (diagnostics; no routing/logic) |
| Event channel | — | Bidirectional encrypted TCP; `updateInfo` sent on connect. Ongoing outbound event reporting **unwired** (see Open) |
| Realtime audio | 96 | ALAC decode, ChaCha20 decrypt, immediate delivery |
| **Video (screen mirroring)** | **110** | **AES-128-CTR decrypt, H.264 decode, working on iOS 18** |
| Unified output | — | Always F32LE interleaved PCM to app |


## AP2 Protocol Completeness Update (2026-11)

This vendored copy additionally implements the following, targeting iOS/macOS
27 senders. Items marked *best effort* may need real-device verification.

### Buffered ALAC (type 103)

* The SETUP's `audioFormat` bits are parsed with `AlacFormat::from_audio_format`
  and `spf` is read when present; both are handed to the buffered processor.
* Each buffered TCP connection selects its decoder from the packet SSRC:
  `0x0000FACE` (ALAC 44.1/16) and `0x15000000` (ALAC 48/24) use the crate's
  ALAC decoder; the AAC SSRCs keep the symphonia path. Apple may switch formats
  mid-stream; the decoder and resampler are rebuilt on format change.
* ALAC streaming parameters: standard `ALACSpecificConfig` (frameLength =
  SETUP `spf` or 4096, `pb=40`, `mb=10`, `kb=14`, `maxRun=255`; sample
  rate/bit depth/channels from the SSRC, refined by `audioFormat`).
* Decoder construction failures call `AudioHandler::on_error` with
  `CodecError::UnsupportedFormat`; no silent fallback to the previous decoder.
* `SourceAudioFormat` reports ALAC/AAC, bit depth, sample rate, and channels to
  the host so now-playing quality data is accurate.

### Unknown SSRC / SSRC_NONE (AirPlay 2, OS 27)

Buffered-audio packets with an unrecognized SSRC (including `SSRC_NONE = 0`,
used by ACK/flush control packets) are skipped before decryption/decoding. They
do not update the current format, are never fed to the previous decoder, and do
not establish the playout anchor. Only the first *valid* audio packet anchors
playback. This mirrors shairport-sync's OS-27 fix (unknown SSRC packets must
not stall a stream whose first block has an unrecognized format).

### SETMAGICCOOKIE

Acknowledged for both shapes seen in the wild: `POST /SETMAGICCOOKIE` (iOS/macOS
27) and a bare `SETMAGICCOOKIE *` RTSP method. The binary-plist body is parsed
for debug logging only; the cookie is opaque and playback does not depend on
it. `OPTIONS` advertises `SETMAGICCOOKIE` and `SETPEERSX`.

### RTCP control channel

The `controlPort` returned in the type 103 SETUP response is now kept open for
the connection's lifetime and read by a tokio task (`src/raop/rtcp.rs`):

* Type 215 (`TIME_ANNOUNCE_PTP`, 28 bytes) and type 212
  (`TIME_ANNOUNCE_NTP`, 20/32 bytes) are parsed, including compound packets and
  padding; Sender/Receiver Reports and retransmit packets are drained.
* The latest PTP announce is stored in `RaopShared::rtcp_announce`. When the
  playout has no authoritative `SETRATEANCHORTIME` anchor yet, the announce is
  converted to a local deadline through `PtpClock` and used as a fallback
  anchor. NTP announces and matches without a synchronised PTP master are
  ignored, so the existing local fallback is preserved.
* The listener is owned by a `Drop` guard: RTSP teardown, stream replacement,
  or a failed connection closes the socket and aborts the task (no leaks).
* When `SETRATEANCHORTIME`'s `networkTimeTimelineID` does not match the PTP
  sink's master id, the clock identity advertised by the RTCP announce is used
  as a fallback timeline id.

### Long-running playback (64-bit RTP timeline)

RTP timestamps were previously stored as `u32` and compared with signed
differences, so playback misbehaved past 2^31 frames (~13.5 h at 44.1 kHz) and
on timestamp wraparound. The buffered playout now extends timestamps onto a
monotonic 64-bit timeline (wrap-aware, reorder-safe, with a bounded rebase when
the sender restarts its timeline for a seek/new track). Buffer keys, anchors,
playout targets and flush ranges all use the extended timeline.

Decoded PCM is capped at ~20 s per stream to bound memory if playout stops
consuming it.

### PTP (best effort, no NQPTP)

`src/net/ptp.rs` now also:

* selects a master from Announce packets (priority1/priority2/stepsRemoved/
  grandmaster id), resetting pending Syncs and offset smoothing when the master
  changes, and ignores Follow_Up messages from non-selected masters;
* measures path delay with rate-limited Delay_Req probes and Delay_Resp
  round-trip estimates (one-way = RTT/2, implausible >100 ms dropped), and adds
  the smoothed delay to Follow_Up offsets;
* keeps Sync/Follow_Up offset smoothing and fallback behaviour unchanged when no
  announce/delay information is available.

**Limitations:** this is not NQPTP. Without a shared PTP daemon there is no
cross-process clock discipline, so multi-room playback with other receivers is
best effort, not guaranteed sample-accurate. The Delay_Req target is the source
address of the last Sync; a master that does not answer keeps the pre-existing
offset-only behaviour. No iOS/macOS 27 hardware was available to validate the
27-specific paths (SETMAGICCOOKIE, buffered ALAC from Apple Music, RTCP timing)
end to end.

### Receiver classification (audio-only vs TV/video)

Apple senders classify a receiver from `GET /info` (and the event channel's
`updateInfo`) as well as mDNS. An audio-only receiver therefore answers all of
those with the pure audio feature set (no video/photo/screen/HLS bits) and the
`AirPort10,115` model; `receiver_features_for_pairing_with(.., audio_only=true)`
builds that set directly instead of clearing bits from the video profile. 1.4.0
advertised the mDNS audio profile correctly but still returned `AppleTV6,2` plus
video/HLS bits from `/info`, which made iOS/macOS offer screen mirroring. This
is covered by unit tests in `net::features`, `net::mdns` and
`handlers_ap2::info_profile`.

### Lyrics in AirPlay metadata (no verifiable field)

AirPlay's DMAP metadata carries `minm` (title), `asar`, `asal`, `asgn`, `astm`,
`astn` and `asdk`; neither this crate, shairport-sync, nor the openairplay
`dxxp.py` tag table defines a lyrics field. Changing `title` lengths in captured
logs therefore cannot be treated as lyrics. To detect vendor extensions (which
may appear with iOS/macOS 27) unknown DMAP tags are now reported (tag + payload
length only, never content) as a rate-limited event log, so the next exported
log can confirm or rule out a lyrics tag.

### Security hardening

* ALAC decoder: frame length bounded (≤16384 samples), bit depth restricted to
  16/24, rice parameters validated, declared per-frame sample counts checked
  against the configured maximum, channel layouts validated, output buffers
  sized-checked, and all decoder arithmetic uses wrapping/i64 math so malformed
  input cannot panic or index out of bounds. A deterministic fuzz-style test
  decodes 2000 random frames/configs.
* RTCP parsing tolerates truncated/malformed/padded datagrams (fuzz-tested).
* Buffered packet lengths are bounded by the on-wire `u16` length prefix; RTSP
  header/body sizes keep the existing `MAX_HEADER_BYTES`/`MAX_BODY_BYTES`
  limits.

## Open / Unwired — scaffolding present, not connected

Implemented building blocks that are **not** wired into the runtime path. Kept
deliberately (behind `#[allow(dead_code)]`) as the foundation for these features —
do **not** delete them as "dead code"; they are unfinished AP2 capabilities.

- **PTP timing → multi-room precision** — Improve further only with a real
  NQPTP-style shared clock. This copy now selects a master from Announce,
  compensates measured Delay_Req/Resp path delay, feeds Sync/Follow_Up offsets
  into `PtpClock`, maps `SETRATEANCHORTIME` onto buffered-audio local playout
  time, and uses RTCP time announces as a fallback anchor. If PTP is
  unavailable, buffered audio retains its best-effort local fallback. Realtime
  stream (type 96) PTP scheduling is still open, and without a shared clock
  there is no claim of sample-accurate multi-room sync.

- **Outbound event reporting** — the encrypted event channel is established and the
  initial `updateInfo` is pushed at SETUP, but the receiver never sends events
  afterward. `EventSender::send` (held in `RaopConnection::event_sender`,
  `src/raop/event_channel.rs`) is the API for this; wire calls to it on receiver-side
  state changes (volume / now-playing / progress) for fuller AP2 event reporting.

> Related already-wired gap closed in 0.6.0: `AudioHandler::on_error` was defined but
> never called — now wired. The two items above are the remaining "built but unwired"
> AP2 capabilities.

## Video — Working (iOS 18)

**Screen mirroring is working.** iPhone screen successfully mirrored and
displayed in a window. The full pipeline is proven:

1. FairPlay key exchange (fp-setup M1/M2)
2. AES-128-CTR video decryption
3. H.264 decode (openh264) + display (minifb)

### Key Discovery: UxPlay Feature Set

The breakthrough was using UxPlay's exact feature bitmask:

```
Features = 0x527FFEE6 (bit 27 "legacy pairing" OFF)
```

With this feature set:
- iPhone **skips pair-setup and pair-verify entirely**
- iPhone sends `ekey` (72 bytes, FairPlay-encrypted) directly in SETUP
- No ECDH hash needed — raw FairPlay-decrypted key goes to Stage 3
- No AP2 bits (40, 41, 46, 48) — pure legacy protocol for video

### Video Key Derivation (Working)

Two-step process (no ECDH hash when bit 27 is off):

```
Step 1: aeskey = playfair_decrypt(keymsg_164, ekey_72)              → 16 bytes
Step 2: key    = SHA-512("AirPlayStreamKey{id}" || aeskey)[0..16]   → 16 bytes
        iv     = SHA-512("AirPlayStreamIV{id}"  || aeskey)[0..16]   → 16 bytes
```

Where `{id}` is the `streamConnectionID` from the type 110 SETUP, formatted
as unsigned decimal (`PRIu64`).

### Critical Bug Found: Truncated FairPlay Tables

The Rust port of `playfair_decrypt` had **6 truncated lookup tables** from the
original C-to-Rust conversion. Rust zero-filled the missing bytes, causing
`playfair_decrypt` to produce wrong keys silently:

| Table | Declared | Actual | Missing |
|-------|----------|--------|---------|
| TABLE_S1 | 10240 | 9600 | 640 |
| TABLE_S2 | 36864 | 34560 | 2304 |
| TABLE_S3 | 4096 | 3840 | 256 |
| TABLE_S4 | 36864 | 34560 | 2304 |
| TABLE_S9 | 1024 | 256 | 768 |
| TABLE_S10 | 4096 | 3840 | 256 |
| STATIC_SOURCE_2 | 47 | 46 | 1 |

Tables regenerated from original C source (`playfair/omg_hax.c`).

**⚠️ This bug invalidated ALL previous video decryption research.** Every
"tested approach" that failed was tested with a broken `playfair_decrypt`.
The previous conclusion that "iOS 18 doesn't send ekey" was wrong — it does
send ekey, but only with UxPlay's feature set (no AP2 bits).

### Current FairPlay Status

FairPlay decryption is implemented in safe Rust. The generated lookup tables are
checked in as Rust constants and are covered by test vectors.

### Previous Research — Needs Re-verification

Earlier video-key research was conducted with the broken `playfair_decrypt`
implementation and may no longer be accurate:

- "iOS 18 doesn't send ekey for screen mirroring" — **WRONG.** It does, with
  UxPlay features (`0x527FFEE6`).
- "ECDH hash is required for video key" — **WRONG for legacy mode.** With bit 27
  off, no pairing occurs and the raw FairPlay key is used directly.
- "All 13+ key derivation variants failed" — **All tested with wrong FairPlay key.**
  Need to re-test with AP2 features if AP2+video is desired.
- "Screen mirroring audio (type 96 usingScreen) has no shk" — Needs re-testing
  with UxPlay features.

### Current Limitations

- **No AP2 audio with video** — UxPlay features disable AP2 buffered audio
- **openh264 decode errors** — Software decoder struggles at 30fps in debug mode
- **No screen mirroring audio** — Type 96 `usingScreen` audio not yet wired up

### Stream Type 120 — Video Relay (Not Implemented)

Sent by YouTube, Apple Music (music videos), and other video apps. The SETUP
contains only `{"type": Integer(120)}` with no additional fields. Likely HLS
video relay where the app sends a video URL for the receiver to fetch directly.

### TODO
- [ ] Test AP2+video hybrid features (can we have both AP2 audio and video?)
- [ ] Wire up screen mirroring audio (type 96 `usingScreen`)
- [ ] Improve H.264 decode stability (release build, error recovery)
- [ ] Re-test previous key derivation approaches with correct FairPlay key

### HLS with AP2 Audio

HLS video playback (`/play`, `/playback-info`) is pure HTTP — it relays an
m3u8 URL to the application and doesn't use the RTP audio pipeline. The
`hls` feature now retains the AP2 buffered-audio feature profile and advertises
the HLS capability without enabling the legacy screen-mirroring feature mask.
The application handles `/play`, `/rate`, `/scrub`, `/stop`, and
`/playback-info`; the sender can therefore control the Windows media pipeline
without replacing the known-good AP2 audio configuration.

## Known Issues (Resolved)

### RC Connection Delay (~10s) — FULLY RESOLVED ✅

Previously, the iPhone opened a "Remote Control Only" RTSP connection ~10 seconds before starting the audio connection.

This has been **fully resolved** by:
1. Deriving a **stable and deterministic Pairing Identifier (`pi`)** from the receiver's MAC address (advertised consistently in mDNS).
2. Correctly completing the GET `/info` response plist payload to return `"pi"`, `"name"`, `"macAddress"`, and `"deviceID"`.
3. Binding and returning a proper `eventPort` in the `isRemoteControlOnly` SETUP response plist, and spawning the encrypted event channel handler.

With these changes, the trust relationship is established instantly, and audio streaming begins in under 50ms without any delay!

## AP2 Remote Control — Research

Third-party AP2 receivers cannot send playback commands (play/pause/skip) to
the iPhone. AP1 DACP remote control is fully implemented and works.

See previous sections for full remote control research (unchanged).

## Test Coverage

`cargo test --features ap2` currently runs 278 tests (207 library, 23 integration, 46 unit-test-vectors, 2 doctests), including fuzz-style ALAC/RTCP robustness tests and buffered ALAC end-to-end decode tests. 17 C-verified vectors from the pair_ap reference implementation:
- TLV codec, HKDF-SHA512, ChaCha20 transport framing
- ADTS framing, audio packet decryption, server keypair
- Anchor time calculation, channel mixdown, SSRC mapping
- Full M1→M4 SRP integration test over real TCP
- Video cipher streaming AES-CTR partial block tests
- FairPlay cross-validation test against C `playfair_decrypt` output

## References

- [AirPlay 2 Internals — Features](https://emanuelecozzi.net/docs/airplay2/features/)
- [AirPlay 2 Internals — RTSP](https://emanuelecozzi.net/docs/airplay2/rtsp/)
- [Unofficial AirPlay Specification](https://openairplay.github.io/airplay-spec/)
- [UxPlay](https://github.com/FDH2/UxPlay) — working screen mirroring reference
- [pair_ap](https://github.com/ejurgensen/pair_ap)
- [shairport-sync](https://github.com/mikebrady/shairport-sync)
