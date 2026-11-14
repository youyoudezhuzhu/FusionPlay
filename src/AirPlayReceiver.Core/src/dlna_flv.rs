//! Minimal FLV (Flash Video) audio demuxer for DLNA playback.
//!
//! Several Chinese music apps (most notably NetEase Cloud Music) announce
//! audio-only DLNA streams as `video/flv` with an `object.item.videoItem`
//! UPnP class. The payload is a normal FLV container carrying either AAC or
//! MP3 audio, so the DLNA player must be able to turn it into something the
//! platform codec decoders can consume.
//!
//! [`FlvAudioStream`] wraps an FLV byte stream (HTTP or file) and exposes the
//! individual codec frames:
//! - **AAC** (FLV SoundFormat 10): the AudioSpecificConfig is decoded into
//!   sample rate / channel configuration and every raw AAC frame is returned
//!   as-is (callers feed it to the platform AAC decoder).
//! - **MP3** (FLV SoundFormat 2): the tag payloads are re-assembled into
//!   complete MPEG audio frames by parsing the `0xFFEx` frame header and
//!   computing the frame length; callers feed each frame to the MP3 decoder.
//!
//! HE-AAC (SBR/PS) is detected and rejected explicitly instead of being
//! decoded at the wrong sample rate. Other FLV sound formats are rejected with
//! an actionable error.

use std::io::{self, Read, Seek, SeekFrom};

use symphonia::core::io::MediaSource;

/// FLV file signature (`FLV` + version byte).
pub fn is_flv(header: &[u8]) -> bool {
    header.len() >= 4 && &header[0..3] == b"FLV"
}

/// Audio codec carried inside the FLV container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlvAudioCodec {
    /// FLV SoundFormat 10: AAC (ADTS/raw AAC frames).
    Aac,
    /// FLV SoundFormat 2: MPEG audio (layers I/II/III; layer III in practice).
    Mp3,
}

impl FlvAudioCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aac => "AAC",
            Self::Mp3 => "MP3",
        }
    }
}

/// Codec configuration extracted from the FLV audio tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvAudioInfo {
    pub codec: FlvAudioCodec,
    /// Sample rate in Hz (from the AAC AudioSpecificConfig or the first MP3
    /// frame header).
    pub sample_rate: u32,
    /// Channel count (1 or 2 in practice).
    pub channels: u8,
}

impl FlvAudioInfo {
    /// Symphonia channel layout matching [`Self::channels`].
    pub fn channel_layout(self) -> Option<symphonia::core::audio::Channels> {
        use symphonia::core::audio::{Channels, Position};
        match self.channels {
            1 => Some(Channels::from(Position::FRONT_CENTER)),
            2 => Some(Channels::from(Position::FRONT_LEFT | Position::FRONT_RIGHT)),
            _ => None,
        }
    }
}

/// One decoded-container audio frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlvAudioFrame {
    /// Raw codec payload: one AAC raw frame (no ADTS header) or one complete
    /// MPEG audio frame including its 4-byte header.
    pub data: Vec<u8>,
    /// PCM samples represented by this frame (AAC-LC: 1024; MP3: 384/576/1152).
    pub samples: u32,
    /// Sample rate of this frame.
    pub sample_rate: u32,
    /// Channel count of this frame.
    pub channels: u8,
}

/// Parsed MPEG audio frame header (`0xFFEx` sync word).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp3FrameHeader {
    /// Total frame length in bytes, including the 4-byte header.
    pub frame_len: usize,
    pub sample_rate: u32,
    pub channels: u8,
    /// PCM samples per frame (Layer I: 384, Layer II: 1152, Layer III: 1152
    /// for MPEG-1 and 576 for MPEG-2/2.5).
    pub samples: u32,
    /// `true` for MPEG-1, `false` for MPEG-2 / MPEG-2.5.
    pub mpeg1: bool,
    /// MPEG layer (1, 2 or 3).
    pub layer: u8,
}

impl Mp3FrameHeader {
    /// Parse a 4-byte MPEG audio frame header. Returns `None` when `bytes` is
    /// too short, the sync word is missing, or the header fields are reserved.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        let header = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if header[0] != 0xFF || header[1] & 0xE0 != 0xE0 {
            return None;
        }
        let version_bits = (header[1] >> 3) & 0x03;
        let layer_bits = (header[1] >> 1) & 0x03;
        let bitrate_index = usize::from((header[2] >> 4) & 0x0F);
        let sample_rate_index = usize::from((header[2] >> 2) & 0x03);
        let padding = usize::from((header[2] >> 1) & 0x01);
        let channel_mode = (header[3] >> 6) & 0x03;
        // Reserved version/layer/bitrate/sample-rate values are not decodable.
        if version_bits == 1 || layer_bits == 0 {
            return None;
        }
        if bitrate_index == 0 || bitrate_index == 0x0F || sample_rate_index == 3 {
            return None;
        }
        // Version index: 0 = MPEG-1, 1 = MPEG-2, 2 = MPEG-2.5.
        let version_index = match version_bits {
            3 => 0usize, // MPEG-1
            2 => 1usize, // MPEG-2
            0 => 2usize, // MPEG-2.5
            _ => return None,
        };
        // Layer index: 0 = Layer I, 1 = Layer II, 2 = Layer III.
        let layer_index = match layer_bits {
            3 => 0usize,
            2 => 1usize,
            1 => 2usize,
            _ => return None,
        };
        let sample_rate = SAMPLE_RATES[version_index][sample_rate_index];
        let bitrate_kbps = if version_index == 0 {
            BITRATES_MPEG1[layer_index][bitrate_index]
        } else {
            BITRATES_MPEG2[layer_index][bitrate_index]
        };
        let bitrate = u32::from(bitrate_kbps) * 1_000;
        if bitrate == 0 {
            return None;
        }
        let layer = match layer_index {
            0 => 1u8,
            1 => 2u8,
            _ => 3u8,
        };
        let frame_len = match layer {
            // Layer I: 4-byte slots.
            1 => (12 * bitrate / sample_rate + padding as u32) * 4,
            // Layer III at MPEG-2/2.5 halves the samples per frame.
            3 if version_index != 0 => 72 * bitrate / sample_rate + padding as u32,
            _ => 144 * bitrate / sample_rate + padding as u32,
        } as usize;
        if !(4..=8 * 1024).contains(&frame_len) {
            return None;
        }
        let samples = match layer {
            1 => 384,
            2 => 1_152,
            _ if version_index == 0 => 1_152,
            _ => 576,
        };
        let channels = if channel_mode == 3 { 1 } else { 2 };
        Some(Self {
            frame_len,
            sample_rate,
            channels,
            samples,
            mpeg1: version_index == 0,
            layer,
        })
    }
}

/// Bitrate tables in kbit/s, indexed `[layer][bitrate_index]`.
const BITRATES_MPEG1: [[u16; 15]; 3] = [
    // Layer I
    [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
    // Layer II
    [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
    // Layer III
    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
];

/// MPEG-2/2.5 bitrate tables in kbit/s, indexed `[layer][bitrate_index]`.
const BITRATES_MPEG2: [[u16; 15]; 3] = [
    // Layer I
    [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
    // Layer II
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    // Layer III
    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
];

/// Sample rates in Hz, indexed `[version][sample_rate_index]`.
const SAMPLE_RATES: [[u32; 3]; 3] = [
    [44_100, 48_000, 32_000], // MPEG-1
    [22_050, 24_000, 16_000], // MPEG-2
    [11_025, 12_000, 8_000],  // MPEG-2.5
];

/// A [`MediaSource`] that replays an already-consumed prefix before delegating
/// to the original source. Used for container sniffing.
///
/// Seeking is forwarded to the inner source (offset by the prefix length), so a
/// seekable local file (e.g. an MP4/M4A whose `moov` atom sits at the end) stays
/// fully seekable through the sniffing wrapper. Non-seekable HTTP bodies keep
/// reporting `is_seekable() == false`.
pub struct PrefixedSource {
    prefix: Vec<u8>,
    /// Bytes of `prefix` already served.
    offset: usize,
    /// Absolute stream position (prefix + inner), used to resolve `SeekFrom`.
    position: u64,
    inner: Box<dyn MediaSource>,
}

impl PrefixedSource {
    pub fn new(prefix: Vec<u8>, inner: Box<dyn MediaSource>) -> Self {
        Self {
            prefix,
            offset: 0,
            position: 0,
            inner,
        }
    }
}

impl Read for PrefixedSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.offset < self.prefix.len() {
            let available = &self.prefix[self.offset..];
            let count = available.len().min(buf.len());
            buf[..count].copy_from_slice(&available[..count]);
            self.offset += count;
            self.position += count as u64;
            return Ok(count);
        }
        let count = self.inner.read(buf)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for PrefixedSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let prefix_len = self.prefix.len() as u64;
        let target = match from {
            SeekFrom::Start(position) => position,
            SeekFrom::Current(delta) => self
                .position
                .checked_add_signed(delta)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?,
            SeekFrom::End(delta) => {
                let length = self
                    .inner
                    .byte_len()
                    .map(|length| length.saturating_add(prefix_len))
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::Unsupported, "stream length is unknown")
                    })? as i64;
                length
                    .checked_add(delta)
                    .filter(|position| *position >= 0)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek overflow"))?
                    as u64
            }
        };
        // The inner source was already advanced by `prefix_len` bytes when the
        // prefix was sniffed, so wrapper position `p` is raw position
        // `max(p, prefix_len)`: seeking inside the prefix keeps the inner source
        // parked at its boundary instead of replaying bytes twice.
        self.inner.seek(SeekFrom::Start(target.max(prefix_len)))?;
        self.offset = target.min(prefix_len) as usize;
        self.position = target;
        Ok(target)
    }
}

impl MediaSource for PrefixedSource {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        // The prefix is the head of the inner stream, not extra bytes: the
        // inner source reports the full length including the sniffed bytes.
        self.inner.byte_len()
    }
}

/// AAC object types rejected with a clear error (HE-AAC / HE-AACv2 need SBR/PS).
const AOT_SBR: u8 = 5;
const AOT_PS: u8 = 29;

/// AAC sample-rate table indexed by the AudioSpecificConfig frequency index.
const AAC_SAMPLE_RATES: [u32; 13] = [
    96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000,
    7_350,
];

/// FLV audio tag framing wrapper producing codec frames.
///
/// Call [`Self::info`] once to parse the FLV header and the codec configuration
/// (AAC AudioSpecificConfig or first MP3 frame header); afterwards
/// [`Self::next_frame`] yields one codec frame at a time.
pub struct FlvAudioStream<R: Read> {
    inner: R,
    header_read: bool,
    info: Option<FlvAudioInfo>,
    state: Option<CodecState>,
    /// First MP3 frame consumed while probing [`Self::info`].
    pending_frame: Option<FlvAudioFrame>,
    eof: bool,
}

enum CodecState {
    Aac,
    Mp3(Mp3FrameSplitter),
}

impl<R: Read> FlvAudioStream<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            header_read: false,
            info: None,
            state: None,
            pending_frame: None,
            eof: false,
        }
    }

    fn read_exact_or_eof(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut filled = 0;
        while filled < buf.len() {
            match self.inner.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(count) => filled += count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(filled)
    }

    /// Parse the FLV file header (once).
    fn ensure_header(&mut self) -> io::Result<()> {
        if self.header_read {
            return Ok(());
        }
        let mut header = [0u8; 9];
        let read = self.read_exact_or_eof(&mut header)?;
        if read < 9 || !is_flv(&header) {
            return Err(invalid_data("不是有效的 FLV 流"));
        }
        let data_offset = u32::from_be_bytes([header[5], header[6], header[7], header[8]]);
        if data_offset as usize > 9 {
            let skip = data_offset as usize - 9;
            let mut scratch = vec![0u8; skip];
            let read = self.read_exact_or_eof(&mut scratch)?;
            if read < skip {
                return Err(invalid_data("FLV 头被截断"));
            }
        }
        // PreviousTagSize0 after the FLV header.
        let mut previous = [0u8; 4];
        let read = self.read_exact_or_eof(&mut previous)?;
        if read < 4 {
            return Err(invalid_data("FLV 头缺少 PreviousTagSize0"));
        }
        self.header_read = true;
        Ok(())
    }

    /// Parse the FLV header and the codec configuration, returning the stream's
    /// codec / sample rate / channel configuration.
    pub fn info(&mut self) -> io::Result<FlvAudioInfo> {
        if let Some(info) = self.info {
            return Ok(info);
        }
        self.ensure_header()?;

        // MP3 payload fragments seen before the first complete frame header.
        let mut mp3 = Mp3FrameSplitter::new();
        loop {
            let tag = match self.read_tag()? {
                Some(tag) => tag,
                None => return Err(invalid_data("FLV 流中没有找到音频轨")),
            };
            let FlvTagBody::Audio {
                sound_format,
                payload,
            } = tag.body
            else {
                continue;
            };
            match sound_format {
                // FLV SoundFormat 10: AAC. The first payload byte is the packet
                // type (0 = AudioSpecificConfig, 1 = raw frame).
                10 => {
                    let Some((packet_type, data)) = payload.split_first() else {
                        return Err(invalid_data("FLV AAC 音频标签为空"));
                    };
                    match *packet_type {
                        0 => {
                            let info = parse_audio_specific_config(data)?;
                            self.info = Some(info);
                            self.state = Some(CodecState::Aac);
                            return Ok(info);
                        }
                        1 => {
                            return Err(invalid_data("FLV AAC 数据出现在配置之前"));
                        }
                        other => {
                            return Err(invalid_data(format!(
                                "未知的 FLV AAC 包类型 {other}"
                            )));
                        }
                    }
                }
                // FLV SoundFormat 2: MP3. The payload is raw MPEG audio data
                // (one or more frames, possibly split across tags).
                2 => {
                    mp3.push(&payload);
                    if let Some((data, header)) = mp3.next_frame() {
                        let info = FlvAudioInfo {
                            codec: FlvAudioCodec::Mp3,
                            sample_rate: header.sample_rate,
                            channels: header.channels,
                        };
                        self.pending_frame = Some(FlvAudioFrame {
                            data,
                            samples: header.samples,
                            sample_rate: header.sample_rate,
                            channels: header.channels,
                        });
                        self.info = Some(info);
                        self.state = Some(CodecState::Mp3(mp3));
                        return Ok(info);
                    }
                }
                other => {
                    return Err(unsupported(format!(
                        "FLV 音频格式 {other} 不受支持（仅支持 AAC 与 MP3）"
                    )));
                }
            }
        }
    }

    /// Read the next codec frame, or `None` at end of stream.
    pub fn next_frame(&mut self) -> io::Result<Option<FlvAudioFrame>> {
        if self.info.is_none() {
            self.info()?;
        }
        if let Some(frame) = self.pending_frame.take() {
            return Ok(Some(frame));
        }
        let codec = self.info.map(|info| info.codec);
        loop {
            if self.eof {
                return Ok(None);
            }
            // Drain the MP3 frame splitter before pulling more FLV tags.
            if codec == Some(FlvAudioCodec::Mp3)
                && let Some(CodecState::Mp3(splitter)) = self.state.as_mut()
                && let Some((data, header)) = splitter.next_frame()
            {
                return Ok(Some(FlvAudioFrame {
                    data,
                    samples: header.samples,
                    sample_rate: header.sample_rate,
                    channels: header.channels,
                }));
            }

            let Some(tag) = self.read_tag()? else {
                self.eof = true;
                return Ok(None);
            };
            let FlvTagBody::Audio {
                sound_format,
                payload,
            } = tag.body
            else {
                continue;
            };
            match (codec, sound_format) {
                (Some(FlvAudioCodec::Aac), 10) => {
                    let Some((packet_type, data)) = payload.split_first() else {
                        continue;
                    };
                    if *packet_type == 1 {
                        let info = self.info.expect("info is initialised above");
                        return Ok(Some(FlvAudioFrame {
                            data: data.to_vec(),
                            samples: 1024,
                            sample_rate: info.sample_rate,
                            channels: info.channels,
                        }));
                    }
                    // Repeated/unknown sequence headers are ignored.
                }
                (Some(FlvAudioCodec::Mp3), 2) => {
                    if let Some(CodecState::Mp3(splitter)) = self.state.as_mut() {
                        splitter.push(&payload);
                    }
                }
                (_, other) => {
                    return Err(unsupported(format!(
                        "FLV 音频格式 {other} 不受支持（仅支持 AAC 与 MP3）"
                    )));
                }
            }
        }
    }

    /// Read the next FLV tag, or `None` at end of stream.
    fn read_tag(&mut self) -> io::Result<Option<FlvTag>> {
        if self.eof {
            return Ok(None);
        }
        let mut header = [0u8; 11];
        let read = self.read_exact_or_eof(&mut header)?;
        if read == 0 {
            self.eof = true;
            return Ok(None);
        }
        if read < 11 {
            return Err(invalid_data("FLV 标签头被截断"));
        }
        let tag_type = header[0];
        let data_size = (usize::from(header[1]) << 16)
            | (usize::from(header[2]) << 8)
            | usize::from(header[3]);
        let timestamp = (u32::from(header[4]) << 16)
            | (u32::from(header[5]) << 8)
            | u32::from(header[6])
            | (u32::from(header[7]) << 24);
        let mut body = vec![0u8; data_size];
        let read = self.read_exact_or_eof(&mut body)?;
        if read < data_size {
            return Err(invalid_data("FLV 标签内容被截断"));
        }
        // PreviousTagSize follows every tag; ignore it.
        let mut previous = [0u8; 4];
        let _ = self.read_exact_or_eof(&mut previous)?;
        let body = match tag_type {
            8 => {
                let Some((sound_format, data)) = body.split_first() else {
                    return Ok(Some(FlvTag {
                        timestamp,
                        body: FlvTagBody::Skip,
                    }));
                };
                FlvTagBody::Audio {
                    sound_format: *sound_format >> 4,
                    payload: data.to_vec(),
                }
            }
            _ => FlvTagBody::Skip,
        };
        Ok(Some(FlvTag { timestamp, body }))
    }
}

/// Backwards-compatible alias; the stream now carries AAC **or** MP3 frames.
pub type FlvAacAdtsStream<R> = FlvAudioStream<R>;

/// FLV audio tag payload (after the first byte).
enum FlvTagBody {
    Skip,
    Audio { sound_format: u8, payload: Vec<u8> },
}

struct FlvTag {
    #[allow(dead_code)]
    timestamp: u32,
    body: FlvTagBody,
}

/// Re-assembles complete MPEG audio frames from arbitrary FLV tag fragments.
struct Mp3FrameSplitter {
    buffer: Vec<u8>,
}

impl Mp3FrameSplitter {
    fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    fn push(&mut self, data: &[u8]) {
        self.buffer.extend_from_slice(data);
    }

    /// Pop the next complete frame. Bytes before the first valid sync word are
    /// discarded (e.g. a stray ID3 tag or garbage).
    fn next_frame(&mut self) -> Option<(Vec<u8>, Mp3FrameHeader)> {
        // Find the first position with a parseable frame header.
        let mut header = None;
        for index in 0..self.buffer.len().saturating_sub(3) {
            if self.buffer[index] == 0xFF
                && self.buffer[index + 1] & 0xE0 == 0xE0
                && let Some(parsed) = Mp3FrameHeader::parse(&self.buffer[index..])
            {
                header = Some((index, parsed));
                break;
            }
        }
        let Some((start, header)) = header else {
            // Keep the last 3 bytes: they may be the beginning of a sync
            // word split across tag boundaries.
            let keep = self.buffer.len().min(3);
            let drain = self.buffer.len() - keep;
            self.buffer.drain(..drain);
            return None;
        };
        if start > 0 {
            self.buffer.drain(..start);
        }
        if self.buffer.len() < header.frame_len {
            return None;
        }
        let frame: Vec<u8> = self.buffer.drain(..header.frame_len).collect();
        Some((frame, header))
    }
}

fn parse_audio_specific_config(data: &[u8]) -> io::Result<FlvAudioInfo> {
    if data.is_empty() {
        return Err(invalid_data("AAC AudioSpecificConfig 为空"));
    }
    let mut reader = BitReader::new(data);
    let object_type = reader.read_bits(5)? as u8;
    let object_type = match object_type {
        31 => 32 + reader.read_bits(6)? as u8,
        other => other,
    };
    let frequency_index = reader.read_bits(4)? as u8;
    let sample_rate = match frequency_index {
        0..=12 => AAC_SAMPLE_RATES[usize::from(frequency_index)],
        13 | 14 => return Err(unsupported("AAC 保留采样率索引")),
        15 => return Err(unsupported("AAC 显式采样率（24 位）暂不支持")),
        _ => return Err(invalid_data("AAC 采样率索引非法")),
    };
    let channel_config = reader.read_bits(4)? as u8;
    if object_type == AOT_SBR || object_type == AOT_PS {
        return Err(unsupported("FLV 使用 HE-AAC（SBR/PS），暂不支持"));
    }
    let channels = match channel_config {
        0 => {
            // Program config element: not parsed, assume the common stereo case.
            2
        }
        1..=6 => channel_config,
        _ => 2,
    };
    Ok(FlvAudioInfo {
        codec: FlvAudioCodec::Aac,
        sample_rate,
        channels,
    })
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn unsupported(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message.into())
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read_bits(&mut self, count: u8) -> io::Result<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let byte = self.bit / 8;
            let offset = 7 - (self.bit % 8);
            let data = self
                .data
                .get(byte)
                .ok_or_else(|| invalid_data("AAC 配置数据被截断"))?;
            value = (value << 1) | u32::from((data >> offset) & 1);
            self.bit += 1;
        }
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn flv_header() -> Vec<u8> {
        let mut bytes = Vec::new();
        // FLV header: "FLV", version 1, flags = audio only, data offset 9.
        bytes.extend_from_slice(b"FLV\x01\x04\0\0\0\x09");
        bytes.extend_from_slice(&[0, 0, 0, 0]); // PreviousTagSize0
        bytes
    }

    fn push_tag(bytes: &mut Vec<u8>, tag_type: u8, payload: &[u8]) {
        bytes.push(tag_type);
        let len = payload.len() as u32;
        bytes.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
        bytes.extend_from_slice(&[0, 0, 0, 0]); // timestamp
        bytes.extend_from_slice(&[0, 0, 0]); // stream id
        bytes.extend_from_slice(payload);
        let total = payload.len() as u32 + 11;
        bytes.extend_from_slice(&total.to_be_bytes()); // PreviousTagSize
    }

    fn flv_with_aac(asc: &[u8], frames: &[&[u8]]) -> Vec<u8> {
        let mut bytes = flv_header();
        // A metadata/script tag must be skipped.
        push_tag(&mut bytes, 18, b"onMetaData");
        let mut sequence = vec![0xAF, 0x00];
        sequence.extend_from_slice(asc);
        push_tag(&mut bytes, 8, &sequence);
        for frame in frames {
            let mut payload = vec![0xAF, 0x01];
            payload.extend_from_slice(frame);
            push_tag(&mut bytes, 8, &payload);
        }
        bytes
    }

    /// A valid MPEG-1 Layer III, 128 kbit/s, 44.1 kHz, stereo frame
    /// (the header `FF FB 90 64` yields 417 bytes per frame).
    fn mp3_frame() -> Vec<u8> {
        let mut frame = vec![0u8; 417];
        frame[0] = 0xFF;
        frame[1] = 0xFB;
        frame[2] = 0x90;
        frame[3] = 0x64;
        frame
    }

    fn flv_with_mp3(frames: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = flv_header();
        push_tag(&mut bytes, 18, b"onMetaData");
        for frame in frames {
            // SoundFormat 2 (MP3), rate 3, size 1, stereo -> 0x2F.
            let mut payload = vec![0x2F];
            payload.extend_from_slice(frame);
            push_tag(&mut bytes, 8, &payload);
        }
        bytes
    }

    #[test]
    fn flv_signature_detection() {
        assert!(is_flv(b"FLV\x01"));
        assert!(!is_flv(b"ID3"));
        assert!(!is_flv(b"FL"));
    }

    #[test]
    fn extracts_aac_config_and_frames() {
        // AAC-LC, 44.1 kHz, stereo.
        let flv = flv_with_aac(&[0x12, 0x10], &[&[1, 2, 3, 4], &[5, 6, 7]]);
        let mut stream = FlvAudioStream::new(Cursor::new(flv));
        let info = stream.info().expect("config");
        assert_eq!(
            info,
            FlvAudioInfo {
                codec: FlvAudioCodec::Aac,
                sample_rate: 44_100,
                channels: 2,
            }
        );
        let first = stream.next_frame().expect("frame").expect("first frame");
        assert_eq!(first.data, vec![1, 2, 3, 4]);
        assert_eq!(first.samples, 1024);
        assert_eq!(first.sample_rate, 44_100);
        assert_eq!(first.channels, 2);
        let second = stream.next_frame().expect("frame").expect("second frame");
        assert_eq!(second.data, vec![5, 6, 7]);
        assert!(stream.next_frame().expect("eof").is_none());
    }

    #[test]
    fn parses_mp3_frame_headers() {
        let frame = mp3_frame();
        let header = Mp3FrameHeader::parse(&frame).expect("valid header");
        assert_eq!(header.frame_len, 417);
        assert_eq!(header.sample_rate, 44_100);
        assert_eq!(header.channels, 2);
        assert_eq!(header.samples, 1152);
        assert!(header.mpeg1);
        assert_eq!(header.layer, 3);

        // Reserved layer and bad bitrate must be rejected.
        assert!(Mp3FrameHeader::parse(&[0xFF, 0xF9, 0x90, 0x64]).is_none());
        assert!(Mp3FrameHeader::parse(&[0xFF, 0xFB, 0xF0, 0x64]).is_none());
        assert!(Mp3FrameHeader::parse(&[0xFF, 0xFB]).is_none());
    }

    #[test]
    fn demuxes_mp3_frames_from_flv() {
        let frames = vec![mp3_frame(), mp3_frame()];
        let flv = flv_with_mp3(&frames);
        let mut stream = FlvAudioStream::new(Cursor::new(flv));
        let info = stream.info().expect("mp3 config");
        assert_eq!(info.codec, FlvAudioCodec::Mp3);
        assert_eq!(info.sample_rate, 44_100);
        assert_eq!(info.channels, 2);
        for expected in frames {
            let frame = stream.next_frame().expect("frame").expect("frame");
            assert_eq!(frame.data, expected);
            assert_eq!(frame.samples, 1152);
            assert_eq!(frame.sample_rate, 44_100);
        }
        assert!(stream.next_frame().expect("eof").is_none());
    }

    #[test]
    fn reassembles_mp3_frames_split_across_flv_tags() {
        let frame = mp3_frame();
        let mut bytes = flv_header();
        // One frame split into two audio tags (100 + 317 bytes after the
        // SoundFormat byte).
        for part in [&frame[..100], &frame[100..]] {
            let mut payload = vec![0x2F];
            payload.extend_from_slice(part);
            push_tag(&mut bytes, 8, &payload);
        }
        let mut stream = FlvAudioStream::new(Cursor::new(bytes));
        assert_eq!(stream.info().expect("info").codec, FlvAudioCodec::Mp3);
        let frame = stream.next_frame().expect("frame").expect("frame");
        assert_eq!(frame.data.len(), 417);
        assert!(stream.next_frame().expect("eof").is_none());
    }

    #[test]
    fn skips_garbage_before_mp3_sync() {
        let frame = mp3_frame();
        let mut payload = vec![0x2F];
        payload.extend_from_slice(b"ID3\x03garbage");
        payload.extend_from_slice(&frame);
        let mut bytes = flv_header();
        push_tag(&mut bytes, 8, &payload);
        let mut stream = FlvAudioStream::new(Cursor::new(bytes));
        assert_eq!(stream.info().expect("info").codec, FlvAudioCodec::Mp3);
        assert_eq!(stream.next_frame().expect("frame").expect("frame").data, frame);
    }

    #[test]
    fn rejects_he_aac_and_unknown_sound_formats() {
        // HE-AAC (SBR) AudioSpecificConfig: AOT 5, 44.1 kHz, stereo.
        let flv = flv_with_aac(&[0x2A, 0x10], &[]);
        let mut stream = FlvAudioStream::new(Cursor::new(flv));
        let error = stream.info().expect_err("HE-AAC must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("HE-AAC"));

        // FLV SoundFormat 5 (Nellymoser) is not supported.
        let mut bytes = flv_header();
        push_tag(&mut bytes, 8, &[0x50, 0, 0, 0]);
        let mut stream = FlvAudioStream::new(Cursor::new(bytes));
        let error = stream.info().expect_err("unknown sound format must be rejected");
        assert!(error.to_string().contains("不受支持"));
    }

    #[test]
    fn rejects_non_flv_stream() {
        let mut stream = FlvAudioStream::new(Cursor::new(b"not an flv stream".to_vec()));
        assert!(stream.info().is_err());
    }

    #[test]
    fn prefixed_source_replays_prefix_then_inner() {
        let inner: Box<dyn MediaSource> = Box::new(Cursor::new(b"world".to_vec()));
        let mut source = PrefixedSource::new(b"hello ".to_vec(), inner);
        let mut out = String::new();
        std::io::Read::read_to_string(&mut source, &mut out).expect("read");
        assert_eq!(out, "hello world");
        // A seekable inner source stays seekable through the wrapper (MP4/M4A
        // needs this: its `moov` atom can sit at the end of the file).
        assert!(source.is_seekable());
    }

    #[test]
    fn prefixed_source_seek_does_not_replay_bytes_twice() {
        // Model the real flow: sniff the first 6 bytes of the file, then wrap
        // the remaining cursor.
        let full = b"hello world".to_vec();
        let mut cursor = Cursor::new(full);
        let mut prefix = vec![0u8; 6];
        std::io::Read::read_exact(&mut cursor, &mut prefix).expect("prefix");
        let mut source = PrefixedSource::new(prefix, Box::new(cursor));

        let mut head = [0u8; 3];
        std::io::Read::read_exact(&mut source, &mut head).expect("head");
        assert_eq!(&head, b"hel");
        source.seek(SeekFrom::Start(0)).expect("seek start");
        let mut out = String::new();
        std::io::Read::read_to_string(&mut source, &mut out).expect("read");
        assert_eq!(out, "hello world", "seek must not duplicate sniffs bytes");

        source.seek(SeekFrom::Start(8)).expect("seek inside inner");
        let mut tail = String::new();
        std::io::Read::read_to_string(&mut source, &mut tail).expect("read tail");
        assert_eq!(tail, "rld");
        assert_eq!(source.byte_len(), Some(11));
    }

    #[test]
    fn aac_config_parses_mono_and_48k() {
        // AAC-LC, 48 kHz (index 3), mono.
        let flv = flv_with_aac(&[0x11, 0x88], &[&[9, 9]]);
        let mut stream = FlvAudioStream::new(Cursor::new(flv));
        let info = stream.info().expect("config");
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 1);
    }
}
