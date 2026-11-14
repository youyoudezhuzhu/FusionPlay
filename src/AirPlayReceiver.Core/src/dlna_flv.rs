//! Minimal FLV (Flash Video) audio demuxer for DLNA playback.
//!
//! Several Chinese music apps (most notably NetEase Cloud Music) announce
//! audio-only DLNA streams as `video/flv` with an `object.item.videoItem`
//! UPnP class. The payload is a normal FLV container carrying AAC audio, so
//! the DLNA player must be able to turn it into something the AAC decoder can
//! consume.
//!
//! [`FlvAacAdtsStream`] wraps an FLV byte stream (HTTP or file) and exposes an
//! ADTS byte stream: FLV tags are parsed, the AAC AudioSpecificConfig is
//! decoded into sample rate / channel configuration and every raw AAC frame
//! is prefixed with a synthesized ADTS header. The rest of the audio pipeline
//! can then treat it like a plain AAC stream.
//!
//! Only AAC audio (FLV SoundFormat 10) is supported; MP3-in-FLV is rare for
//! this use case and returns an actionable error. HE-AAC (SBR/PS) is detected
//! and rejected explicitly instead of being decoded at the wrong sample rate.

use std::io::{self, Read, Seek, SeekFrom};

use symphonia::core::io::MediaSource;

/// FLV file signature (`FLV` + version byte).
pub fn is_flv(header: &[u8]) -> bool {
    header.len() >= 4 && &header[0..3] == b"FLV"
}

/// AAC configuration extracted from the FLV AAC sequence header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlvAudioInfo {
    /// Sample rate in Hz (from the AudioSpecificConfig).
    pub sample_rate: u32,
    /// Channel count (from the AudioSpecificConfig; 1 or 2 in practice).
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

/// A [`MediaSource`] that replays an already-consumed prefix before delegating
/// to the original source. Used for container sniffing without seek support.
pub struct PrefixedSource {
    prefix: Vec<u8>,
    offset: usize,
    inner: Box<dyn MediaSource>,
}

impl PrefixedSource {
    pub fn new(prefix: Vec<u8>, inner: Box<dyn MediaSource>) -> Self {
        Self {
            prefix,
            offset: 0,
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
            return Ok(count);
        }
        self.inner.read(buf)
    }
}

impl Seek for PrefixedSource {
    fn seek(&mut self, _from: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "streaming source cannot seek",
        ))
    }
}

impl MediaSource for PrefixedSource {
    fn is_seekable(&self) -> bool {
        false
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner
            .byte_len()
            .map(|length| length.saturating_add(self.prefix.len() as u64))
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

/// FLV audio tag framing wrapper producing an ADTS stream.
///
/// Call [`Self::info`] once to parse the FLV header and the AAC sequence header;
/// afterwards the type implements [`Read`] yielding ADTS frames.
pub struct FlvAacAdtsStream<R: Read> {
    inner: R,
    header: Option<(u8, u8, u8, u8)>, // version, flags, data_offset bytes consumed
    info: Option<FlvAudioInfo>,
    out: Vec<u8>,
    out_offset: usize,
    eof: bool,
}

impl<R: Read> FlvAacAdtsStream<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            header: None,
            info: None,
            out: Vec::new(),
            out_offset: 0,
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

    /// Parse the FLV header and the first AAC sequence header, returning the
    /// stream's sample rate / channel configuration.
    pub fn info(&mut self) -> io::Result<FlvAudioInfo> {
        if let Some(info) = self.info {
            return Ok(info);
        }
        if self.header.is_none() {
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
            self.header = Some((header[3], header[4], 0, 0));
        }

        // Scan tags until the AAC sequence header (SoundFormat 10, packet type 0)
        // or until we know the audio is not AAC.
        loop {
            let tag = match self.read_tag()? {
                Some(tag) => tag,
                None => return Err(invalid_data("FLV 流中没有找到 AAC 音频轨")),
            };
            let crate::dlna_flv::FlvTagBody::Audio { sound_format, payload } = tag.body else {
                continue;
            };
            if sound_format == 2 {
                return Err(unsupported("FLV 使用 MP3 音频，暂不支持（请使用 AAC 编码的 FLV）"));
            }
            if sound_format != 10 {
                return Err(unsupported(format!(
                    "FLV 音频格式 {sound_format} 不受支持（仅支持 AAC）"
                )));
            }
            let Some((packet_type, data)) = payload.split_first() else {
                return Err(invalid_data("FLV AAC 音频标签为空"));
            };
            match *packet_type {
                0 => {
                    let info = parse_audio_specific_config(data)?;
                    self.info = Some(info);
                    return Ok(info);
                }
                1 => {
                    // Raw AAC frame before the sequence header: tolerate it by
                    // buffering it once the config arrives (rare but harmless).
                    // Without a config we cannot synthesize ADTS headers.
                    return Err(invalid_data("FLV AAC 数据出现在配置之前"));
                }
                other => {
                    return Err(invalid_data(format!(
                        "未知的 FLV AAC 包类型 {other}"
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

    /// Append one raw AAC frame to the ADTS output buffer.
    fn push_aac_frame(&mut self, info: FlvAudioInfo, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() {
            return Ok(());
        }
        let adts = build_adts_header(info, payload.len())?;
        self.out.extend_from_slice(&adts);
        self.out.extend_from_slice(payload);
        Ok(())
    }
}

impl<R: Read> Read for FlvAacAdtsStream<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.out_offset < self.out.len() {
                let available = &self.out[self.out_offset..];
                let count = available.len().min(buf.len());
                buf[..count].copy_from_slice(&available[..count]);
                self.out_offset += count;
                return Ok(count);
            }
            self.out.clear();
            self.out_offset = 0;
            if self.eof {
                return Ok(0);
            }
            let info = match self.info {
                Some(info) => info,
                None => self.info()?,
            };
            let Some(tag) = self.read_tag()? else {
                self.eof = true;
                return Ok(0);
            };
            match tag.body {
                FlvTagBody::Skip => continue,
                FlvTagBody::Audio { sound_format, payload } => {
                    if sound_format != 10 {
                        return Err(unsupported(format!(
                            "FLV 音频格式 {sound_format} 不受支持（仅支持 AAC）"
                        )));
                    }
                    let Some((packet_type, data)) = payload.split_first() else {
                        continue;
                    };
                    match *packet_type {
                        // Sequence headers are rare after the first one; ignore.
                        0 => continue,
                        1 => {
                            self.push_aac_frame(info, data)?;
                            if !self.out.is_empty() {
                                continue;
                            }
                        }
                        _ => continue,
                    }
                }
            }
        }
    }
}

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
        sample_rate,
        channels,
    })
}

fn build_adts_header(info: FlvAudioInfo, payload_len: usize) -> io::Result<[u8; 7]> {
    let frequency_index = AAC_SAMPLE_RATES
        .iter()
        .position(|rate| *rate == info.sample_rate)
        .ok_or_else(|| unsupported("AAC 采样率不在 ADTS 表内"))? as u8;
    let total_len = payload_len + 7;
    if total_len > 0x1FFF {
        return Err(invalid_data("AAC 帧过大"));
    }
    let profile = 1u8; // AAC-LC
    let channel = info.channels & 0x07;
    let mut header = [0u8; 7];
    header[0] = 0xFF;
    header[1] = 0xF1; // MPEG-4, layer 0, no CRC
    header[2] = ((profile & 0x03) << 6) | ((frequency_index & 0x0F) << 2) | ((channel >> 2) & 0x01);
    header[3] = ((channel & 0x03) << 6) | ((total_len >> 11) as u8 & 0x03);
    header[4] = ((total_len >> 3) & 0xFF) as u8;
    header[5] = (((total_len & 0x07) as u8) << 5) | 0x1F;
    header[6] = 0xFC;
    Ok(header)
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

    fn flv_with_audio(asc: &[u8], frames: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        // FLV header: "FLV", version 1, flags = audio only, data offset 9.
        bytes.extend_from_slice(b"FLV\x01\x04\0\0\0\x09");
        bytes.extend_from_slice(&[0, 0, 0, 0]); // PreviousTagSize0
        let mut push_tag = |tag_type: u8, payload: &[u8]| {
            bytes.push(tag_type);
            let len = payload.len() as u32;
            bytes.extend_from_slice(&[(len >> 16) as u8, (len >> 8) as u8, len as u8]);
            bytes.extend_from_slice(&[0, 0, 0, 0]); // timestamp
            bytes.extend_from_slice(&[0, 0, 0]); // stream id
            bytes.extend_from_slice(payload);
            let total = payload.len() as u32 + 11;
            bytes.extend_from_slice(&total.to_be_bytes()); // PreviousTagSize
        };
        // A metadata/script tag must be skipped.
        push_tag(18, b"onMetaData");
        let mut sequence = vec![0xAF, 0x00];
        sequence.extend_from_slice(asc);
        push_tag(8, &sequence);
        for frame in frames {
            let mut payload = vec![0xAF, 0x01];
            payload.extend_from_slice(frame);
            push_tag(8, &payload);
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
    fn extracts_aac_config_and_converts_frames_to_adts() {
        // AAC-LC, 44.1 kHz, stereo.
        let flv = flv_with_audio(&[0x12, 0x10], &[&[1, 2, 3, 4], &[5, 6, 7]]);
        let mut stream = FlvAacAdtsStream::new(Cursor::new(flv));
        let info = stream.info().expect("config");
        assert_eq!(
            info,
            FlvAudioInfo {
                sample_rate: 44_100,
                channels: 2
            }
        );
        let mut adts = Vec::new();
        std::io::Read::read_to_end(&mut stream, &mut adts).expect("adts");
        // Two frames: 7-byte header + payload each = 11 + 10 bytes.
        assert_eq!(adts.len(), 21);
        assert_eq!(&adts[..2], &[0xFF, 0xF1]);
        let first_len = ((usize::from(adts[3] & 0x03)) << 11)
            | (usize::from(adts[4]) << 3)
            | (usize::from(adts[5]) >> 5);
        assert_eq!(first_len, 11);
        assert_eq!(&adts[7..11], &[1, 2, 3, 4]);
        let second_len = ((usize::from(adts[14] & 0x03)) << 11)
            | (usize::from(adts[15]) << 3)
            | (usize::from(adts[16]) >> 5);
        assert_eq!(second_len, 10);
        assert_eq!(&adts[18..21], &[5, 6, 7]);
    }

    #[test]
    fn rejects_he_aac_and_mp3_flv() {
        // HE-AAC (SBR) AudioSpecificConfig: AOT 5, 44.1 kHz, stereo.
        let flv = flv_with_audio(&[0x2A, 0x10], &[]);
        let mut stream = FlvAacAdtsStream::new(Cursor::new(flv));
        let error = stream.info().expect_err("HE-AAC must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("HE-AAC"));

        let mut mp3_flv = Vec::new();
        mp3_flv.extend_from_slice(b"FLV\x01\x04\0\0\0\x09");
        mp3_flv.extend_from_slice(&[0, 0, 0, 0]);
        mp3_flv.extend_from_slice(&[8, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0x2F, 0, 0, 0, 0]);
        let mut stream = FlvAacAdtsStream::new(Cursor::new(mp3_flv));
        let error = stream.info().expect_err("MP3 must be rejected");
        assert!(error.to_string().contains("MP3"));
    }

    #[test]
    fn rejects_non_flv_stream() {
        let mut stream = FlvAacAdtsStream::new(Cursor::new(b"not an flv stream".to_vec()));
        assert!(stream.info().is_err());
    }

    #[test]
    fn prefixed_source_replays_prefix_then_inner() {
        let inner: Box<dyn MediaSource> = Box::new(Cursor::new(b"world".to_vec()));
        let mut source = PrefixedSource::new(b"hello ".to_vec(), inner);
        let mut out = String::new();
        std::io::Read::read_to_string(&mut source, &mut out).expect("read");
        assert_eq!(out, "hello world");
        assert!(!source.is_seekable());
    }

    #[test]
    fn aac_config_parses_mono_and_48k() {
        // AAC-LC, 48 kHz (index 3), mono.
        let flv = flv_with_audio(&[0x11, 0x88], &[&[9, 9]]);
        let mut stream = FlvAacAdtsStream::new(Cursor::new(flv));
        let info = stream.info().expect("config");
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.channels, 1);
    }
}
