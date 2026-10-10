use anyhow::Result;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct AudioMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_number: u32,
    pub year: u32,
    pub duration: f64,
    pub artwork: Option<Vec<u8>>,
}

pub struct MetadataReader;

impl MetadataReader {
    pub fn new() -> Self {
        Self
    }

    pub fn read(&self, path: &Path) -> Result<AudioMetadata> {
        let mut meta = AudioMetadata::default();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let fallback = match ext.as_str() {
            "mp3" => self.read_id3(path).unwrap_or_default(),
            "m4a" | "mp4" | "aac" => self.read_mp4(path).unwrap_or_default(),
            "flac" => self.read_flac(path).unwrap_or_default(),
            "ogg" | "opus" | "oga" => self.read_vorbis(path).unwrap_or_default(),
            _ => AudioMetadata::default(),
        };

        if meta.title.is_empty() {
            meta.title = fallback.title;
        }
        if meta.artist.is_empty() {
            meta.artist = fallback.artist;
        }
        if meta.album.is_empty() {
            meta.album = fallback.album;
        }
        if meta.track_number == 0 {
            meta.track_number = fallback.track_number;
        }
        if meta.year == 0 {
            meta.year = fallback.year;
        }
        if meta.duration <= 0.0 {
            meta.duration = fallback.duration;
        }
        if meta.artwork.is_none() {
            meta.artwork = fallback.artwork;
        }

        if meta.artwork.is_none() {
            meta.artwork = Self::folder_artwork(path);
        }

        Ok(meta)
    }

    fn read_id3(&self, path: &Path) -> Result<AudioMetadata> {
        let mut file = File::open(path)?;
        let mut header = [0u8; 10];
        file.read_exact(&mut header)?;
        if &header[0..3] != b"ID3" {
            return Ok(AudioMetadata::default());
        }
        let version = header[3];
        if version != 3 && version != 4 {
            return Ok(AudioMetadata::default());
        }
        let flags = header[5];
        let size = Self::syncsafe(&header[6..10]);
        let mut meta = AudioMetadata::default();
        let mut offset: u64 = 10;

        if flags & 0x40 != 0 {
            file.seek(SeekFrom::Start(10))?;
            let mut ext = [0u8; 4];
            file.read_exact(&mut ext)?;
            let ext_size = if version == 4 {
                Self::syncsafe(&ext)
            } else {
                u32::from_be_bytes(ext) as usize
            };
            offset += ext_size as u64 + 4;
        }

        let end = 10 + size as u64;
        let mut frame_header = [0u8; 10];

        while offset + 10 <= end {
            file.seek(SeekFrom::Start(offset))?;
            if file.read(&mut frame_header)? < 10 || frame_header[0] == 0 {
                break;
            }
            let id = &frame_header[0..4];
            let frame_size = if version == 4 {
                Self::syncsafe(&frame_header[4..8])
            } else {
                u32::from_be_bytes([
                    frame_header[4],
                    frame_header[5],
                    frame_header[6],
                    frame_header[7],
                ]) as usize
            };
            if frame_size == 0 || frame_size > 4_000_000 {
                offset += 10;
                continue;
            }
            let mut data = vec![0u8; frame_size];
            file.seek(SeekFrom::Start(offset + 10))?;
            file.read_exact(&mut data)?;

            match id {
                b"TIT2" => meta.title = Self::id3_text(&data),
                b"TPE1" => meta.artist = Self::id3_text(&data),
                b"TALB" => meta.album = Self::id3_text(&data),
                b"TRCK" => {
                    meta.track_number = Self::id3_text(&data)
                        .split('/')
                        .next()
                        .unwrap_or("0")
                        .trim()
                        .parse()
                        .unwrap_or(0)
                }
                b"TYER" | b"TDRC" => {
                    meta.year = Self::id3_text(&data)
                        .chars()
                        .take(4)
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0)
                }
                b"TDOR" | b"TORY" if meta.year == 0 => {
                    meta.year = Self::id3_text(&data)
                        .chars()
                        .take(4)
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0)
                }
                b"APIC" if meta.artwork.is_none() => {
                    meta.artwork = Self::apic_data(&data);
                }
                _ => {}
            }
            offset += 10 + frame_size as u64;
        }
        if meta.duration <= 0.0 {
            // audio_start is the position after the ID3 tag
            let audio_start = 10 + size as u64;
            if flags & 0x40 != 0 {
                // Extended header already accounted for in offset
                // But we need to add it to audio_start
                // For simplicity, just use offset which is already past the tag
            }
            meta.duration = Self::mp3_duration(path, audio_start);
        }
        Ok(meta)
    }

    /// MP3 duration without a decoder: Xing/Info header when present,
    /// otherwise CBR estimate from the first frame header.
    fn mp3_duration(path: &Path, audio_start: u64) -> f64 {
        const BR_MPEG1: [u32; 16] = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
        ];
        const BR_MPEG2: [u32; 16] = [
            0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
        ];
        const RATES: [[u32; 3]; 3] = [
            [44100, 48000, 32000],
            [22050, 24000, 16000],
            [11025, 12000, 8000],
        ];

        let mut file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return 0.0,
        };
        let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let mut buf = vec![0u8; 8192];
        if file.seek(SeekFrom::Start(audio_start)).is_err() {
            return 0.0;
        }
        let n = file.read(&mut buf).unwrap_or(0);
        if n < 16 {
            return 0.0;
        }
        let buf = &buf[..n];

        let mut i = 0;
        while i + 4 < buf.len() {
            if buf[i] == 0xFF && buf[i + 1] & 0xE0 == 0xE0 {
                let ver_bits = (buf[i + 1] >> 3) & 3;
                let layer = (buf[i + 1] >> 1) & 3;
                let br_idx = (buf[i + 2] >> 4) as usize;
                let rate_idx = ((buf[i + 2] >> 2) & 3) as usize;
                let padding = ((buf[i + 2] >> 1) & 1) as u32;
                let mono = buf[i + 3] >> 6 == 3;
                // version: 0 = MPEG2.5, 2 = MPEG2, 3 = MPEG1; layer 1 = III
                let (vi, mpeg1) = match ver_bits {
                    3 => (0, true),
                    2 => (1, false),
                    0 => (2, false),
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                if layer != 1 || br_idx == 0 || br_idx == 15 || rate_idx == 3 {
                    i += 1;
                    continue;
                }
                let bitrate = if mpeg1 {
                    BR_MPEG1[br_idx]
                } else {
                    BR_MPEG2[br_idx]
                } * 1000;
                let rate = RATES[vi][rate_idx];
                if bitrate == 0 || rate == 0 {
                    return 0.0;
                }
                let spf = if mpeg1 { 1152 } else { 576 };
                // Xing/Info header after side info
                let side = if mpeg1 {
                    if mono {
                        17
                    } else {
                        32
                    }
                } else if mono {
                    9
                } else {
                    17
                };
                let x = i + 4 + side;
                if x + 12 <= buf.len()
                    && (buf[x..].starts_with(b"Xing") || buf[x..].starts_with(b"Info"))
                {
                    let flags = u32::from_be_bytes(buf[x + 4..x + 8].try_into().unwrap());
                    if flags & 1 != 0 {
                        let frames = u32::from_be_bytes(buf[x + 8..x + 12].try_into().unwrap());
                        return frames as f64 * spf as f64 / rate as f64;
                    }
                }
                let frame_len = ((if mpeg1 { 144 } else { 72 } * bitrate / rate) + padding) as u64;
                if frame_len > 0 && file_len > audio_start {
                    let frames = (file_len - audio_start) / frame_len;
                    return frames as f64 * spf as f64 / rate as f64;
                }
                return 0.0;
            }
            i += 1;
        }
        0.0
    }

    fn read_mp4(&self, path: &Path) -> Result<AudioMetadata> {
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        let mut meta = AudioMetadata::default();

        let read_header = |file: &mut File, pos: u64| -> Result<Option<(u32, u64, u64)>> {
            if pos + 8 > len {
                return Ok(None);
            }
            file.seek(SeekFrom::Start(pos))?;
            let mut h = [0u8; 16];
            if file.read(&mut h)? < 8 {
                return Ok(None);
            }
            let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]);
            let typ = u32::from_be_bytes([h[4], h[5], h[6], h[7]]);
            let (start, end) = match size {
                0 => (pos + 8, len),
                1 => {
                    file.read_exact(&mut h[8..16])?;
                    let large = u64::from_be_bytes(h[8..16].try_into().unwrap());
                    (pos + 16, pos + large)
                }
                2..=7 => return Ok(None),
                _ => (pos + 8, pos + size as u64),
            };
            Ok(Some((typ, start, end.min(len))))
        };

        type ReadHeader<'a> = dyn Fn(&mut File, u64) -> Result<Option<(u32, u64, u64)>> + 'a;

        fn find(
            file: &mut File,
            len: u64,
            read_header: &ReadHeader<'_>,
            start: u64,
            end: u64,
            target: u32,
        ) -> Result<Option<(u64, u64)>> {
            let mut pos = start;
            while pos + 8 <= end && pos < len {
                if let Some((t, s, e)) = read_header(file, pos)? {
                    if t == target {
                        return Ok(Some((s, e)));
                    }
                    if e <= pos {
                        break;
                    }
                    pos = e;
                } else {
                    break;
                }
            }
            Ok(None)
        }

        let moov = find(
            &mut file,
            len,
            &read_header,
            0,
            len,
            u32::from_be_bytes(*b"moov"),
        )?;
        let Some((moov_s, moov_e)) = moov else {
            return Ok(meta);
        };

        // Duration from mvhd
        if let Some((mvhd_s, _)) = find(
            &mut file,
            len,
            &read_header,
            moov_s,
            moov_e,
            u32::from_be_bytes(*b"mvhd"),
        )? {
            file.seek(SeekFrom::Start(mvhd_s))?;
            let mut v = [0u8; 4];
            file.read_exact(&mut v)?;
            let version = v[0];
            if version == 1 {
                file.seek(SeekFrom::Start(mvhd_s + 20))?;
                let mut buf = [0u8; 12];
                file.read_exact(&mut buf)?;
                let scale = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                let dur = u64::from_be_bytes(buf[4..12].try_into().unwrap());
                if scale > 0 {
                    meta.duration = dur as f64 / scale as f64;
                }
            } else {
                file.seek(SeekFrom::Start(mvhd_s + 12))?;
                let mut buf = [0u8; 8];
                file.read_exact(&mut buf)?;
                let scale = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
                let dur = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
                if scale > 0 {
                    meta.duration = dur as f64 / scale as f64;
                }
            }
        }

        let udta = find(
            &mut file,
            len,
            &read_header,
            moov_s,
            moov_e,
            u32::from_be_bytes(*b"udta"),
        )?;
        let Some((udta_s, udta_e)) = udta else {
            return Ok(meta);
        };
        let meta_box = find(
            &mut file,
            len,
            &read_header,
            udta_s,
            udta_e,
            u32::from_be_bytes(*b"meta"),
        )?;
        let Some((meta_s, meta_e)) = meta_box else {
            return Ok(meta);
        };
        let ilst = find(
            &mut file,
            len,
            &read_header,
            meta_s + 4,
            meta_e,
            u32::from_be_bytes(*b"ilst"),
        )?;
        let Some((ilst_s, ilst_e)) = ilst else {
            return Ok(meta);
        };

        let mut pos = ilst_s;
        while pos + 8 <= ilst_e {
            let Some((typ, s, e)) = read_header(&mut file, pos)? else {
                break;
            };
            // item content is one or more data boxes; first data box payload after 8 bytes header
            if e > s + 8 {
                // walk inner boxes for 'data'
                let mut inner = s;
                let mut text_val = String::new();
                let mut num_val: Option<u32> = None;
                let mut covr: Option<Vec<u8>> = None;
                while inner + 8 <= e {
                    let Some((it, is, ie)) = read_header(&mut file, inner)? else {
                        break;
                    };
                    if it == u32::from_be_bytes(*b"data") && ie > is + 8 {
                        let n = (ie - is - 8) as usize;
                        let mut buf = vec![0u8; n.min(1_000_000)];
                        file.seek(SeekFrom::Start(is + 8))?;
                        file.read_exact(&mut buf)?;
                        if typ == u32::from_be_bytes(*b"covr") {
                            covr = Some(buf);
                        } else if typ == u32::from_be_bytes(*b"trkn") {
                            if buf.len() >= 4 {
                                num_val = Some(u16::from_be_bytes([buf[2], buf[3]]) as u32);
                            }
                        } else {
                            text_val = String::from_utf8_lossy(&buf).trim_matches('\0').to_string();
                        }
                        break;
                    }
                    if ie <= inner {
                        break;
                    }
                    inner = ie;
                }
                match typ {
                    t if t == u32::from_be_bytes(*b"\xa9nam") => meta.title = text_val,
                    t if t == u32::from_be_bytes(*b"\xa9ART") => meta.artist = text_val,
                    t if t == u32::from_be_bytes(*b"\xa9alb") => meta.album = text_val,
                    t if t == u32::from_be_bytes(*b"\xa9day") => {
                        meta.year = text_val
                            .chars()
                            .take(4)
                            .collect::<String>()
                            .parse()
                            .unwrap_or(0)
                    }
                    t if t == u32::from_be_bytes(*b"trkn") => {
                        if let Some(n) = num_val {
                            meta.track_number = n;
                        }
                    }
                    t if t == u32::from_be_bytes(*b"covr") => meta.artwork = covr,
                    _ => {}
                }
            }
            if e <= pos {
                break;
            }
            pos = e;
        }
        Ok(meta)
    }

    fn read_flac(&self, path: &Path) -> Result<AudioMetadata> {
        let mut file = File::open(path)?;
        let mut magic = [0u8; 4];
        file.read_exact(&mut magic)?;
        if &magic != b"fLaC" {
            return Ok(AudioMetadata::default());
        }
        let mut meta = AudioMetadata::default();
        let mut hdr = [0u8; 4];
        loop {
            file.read_exact(&mut hdr)?;
            let last = hdr[0] & 0x80 != 0;
            let typ = hdr[0] & 0x7F;
            let len = ((hdr[1] as usize) << 16) | ((hdr[2] as usize) << 8) | hdr[3] as usize;
            if len > 10_000_000 {
                break;
            }
            let mut block = vec![0u8; len];
            file.read_exact(&mut block)?;
            if typ == 0 && block.len() >= 18 {
                // STREAMINFO bytes 10..18: 20-bit rate, 3-bit ch, 5-bit bps, 36-bit total.
                let q = u64::from_be_bytes(block[10..18].try_into().unwrap());
                let sample_rate = (q >> 44) as u32;
                let total = q & 0x0F_FFFF_FFFF;
                if sample_rate > 0 {
                    meta.duration = total as f64 / sample_rate as f64;
                }
            } else if typ == 4 {
                let mut pos = 0;
                if block.len() < 4 {
                    continue;
                }
                let vendor_len =
                    u32::from_le_bytes(block[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4 + vendor_len;
                if pos + 4 > block.len() {
                    continue;
                }
                let n = u32::from_le_bytes(block[pos..pos + 4].try_into().unwrap()) as usize;
                pos += 4;
                for _ in 0..n {
                    if pos + 4 > block.len() {
                        break;
                    }
                    let clen = u32::from_le_bytes(block[pos..pos + 4].try_into().unwrap()) as usize;
                    pos += 4;
                    if pos + clen > block.len() {
                        break;
                    }
                    let comment = String::from_utf8_lossy(&block[pos..pos + clen]);
                    pos += clen;
                    if let Some((k, v)) = comment.split_once('=') {
                        match k.to_uppercase().as_str() {
                            "TITLE" => meta.title = v.to_string(),
                            "ARTIST" => meta.artist = v.to_string(),
                            "ALBUM" => meta.album = v.to_string(),
                            "TRACKNUMBER" => {
                                meta.track_number = v
                                    .split('/')
                                    .next()
                                    .unwrap_or("0")
                                    .trim()
                                    .parse()
                                    .unwrap_or(0)
                            }
                            "DATE" | "YEAR" => {
                                meta.year =
                                    v.chars().take(4).collect::<String>().parse().unwrap_or(0)
                            }
                            _ => {}
                        }
                    }
                }
            } else if typ == 6 && meta.artwork.is_none() {
                meta.artwork = Self::flac_picture(&block);
            }
            if last {
                break;
            }
        }
        Ok(meta)
    }

    fn read_vorbis(&self, path: &Path) -> Result<AudioMetadata> {
        let mut file = File::open(path)?;
        let mut meta = AudioMetadata::default();
        let mut packet = Vec::new();
        for _ in 0..64 {
            let mut ph = [0u8; 27];
            if file.read(&mut ph)? != 27 || &ph[0..4] != b"OggS" {
                break;
            }
            let segs = ph[26] as usize;
            let mut table = vec![0u8; segs];
            file.read_exact(&mut table)?;
            for &s in &table {
                let mut buf = vec![0u8; s as usize];
                file.read_exact(&mut buf)?;
                packet.extend(buf);
                if s < 255 {
                    let prefix = if packet.starts_with(b"\x03vorbis") {
                        7
                    } else if packet.starts_with(b"OpusTags") {
                        8
                    } else {
                        packet.clear();
                        continue;
                    };
                    let Some(vendor_bytes) = packet.get(prefix..prefix + 4) else {
                        packet.clear();
                        continue;
                    };
                    let vendor_len = u32::from_le_bytes(vendor_bytes.try_into().unwrap()) as usize;
                    let mut pos = prefix + 4 + vendor_len;
                    let count = packet
                        .get(pos..pos + 4)
                        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                        .unwrap_or(0);
                    pos += 4;
                    for _ in 0..count.min(1000) {
                        let Some(len_bytes) = packet.get(pos..pos + 4) else {
                            break;
                        };
                        let len = u32::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
                        pos += 4;
                        let Some(bytes) = packet.get(pos..pos.saturating_add(len)) else {
                            break;
                        };
                        pos += len;
                        let comment = String::from_utf8_lossy(bytes);
                        if let Some((key, value)) = comment.split_once('=') {
                            match key.to_ascii_uppercase().as_str() {
                                "TITLE" => meta.title = value.to_string(),
                                "ARTIST" => meta.artist = value.to_string(),
                                "ALBUM" => meta.album = value.to_string(),
                                "TRACKNUMBER" => {
                                    meta.track_number =
                                        value.split('/').next().unwrap_or("0").parse().unwrap_or(0)
                                }
                                "DATE" | "YEAR" => {
                                    meta.year = value
                                        .chars()
                                        .take(4)
                                        .collect::<String>()
                                        .parse()
                                        .unwrap_or(0)
                                }
                                _ => {}
                            }
                        }
                    }
                    return Ok(meta);
                }
                if packet.len() > 4_000_000 {
                    return Ok(meta);
                }
            }
        }
        Ok(meta)
    }

    fn folder_artwork(path: &Path) -> Option<Vec<u8>> {
        let dir = path.parent()?;
        for name in ["cover", "folder", "front", "album", "Cover", "Folder"] {
            for ext in ["jpg", "jpeg", "png", "webp"] {
                let cand = dir.join(format!("{name}.{ext}"));
                if let Ok(data) = std::fs::read(&cand) {
                    if !data.is_empty() {
                        return Some(data);
                    }
                }
            }
        }
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                    if matches!(ext.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp") {
                        if let Ok(data) = std::fs::read(&p) {
                            if !data.is_empty() {
                                return Some(data);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    fn syncsafe(b: &[u8]) -> usize {
        b.iter().fold(0, |acc, &x| (acc << 7) | (x & 0x7F) as usize)
    }

    fn id3_text(data: &[u8]) -> String {
        if data.is_empty() {
            return String::new();
        }
        let (enc, text) = (data[0], &data[1..]);
        // ID3v2: 0 = latin1, 1 = UTF-16 with BOM (endianness follows the BOM),
        // 2 = UTF-16BE, 3 = UTF-8.
        let be = match enc {
            1 => !text.starts_with(&[0xFF, 0xFE]),
            2 => true,
            _ => {
                return String::from_utf8_lossy(text)
                    .trim_matches('\0')
                    .trim()
                    .to_string();
            }
        };
        let text = text
            .strip_prefix(&[0xFF, 0xFE])
            .or_else(|| text.strip_prefix(&[0xFE, 0xFF]))
            .unwrap_or(text);
        let (chunks, _) = text.as_chunks::<2>();
        let u16s: Vec<u16> = chunks
            .iter()
            .map(|c| {
                if be {
                    u16::from_be_bytes(*c)
                } else {
                    u16::from_le_bytes(*c)
                }
            })
            .collect();
        String::from_utf16_lossy(&u16s)
            .trim_matches('\0')
            .trim()
            .to_string()
    }

    fn apic_data(data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 4 {
            return None;
        }
        let enc = data[0];
        let mut pos = 1;
        while pos < data.len() && data[pos] != 0 {
            pos += 1;
        }
        pos += 1; // mime
        if pos >= data.len() {
            return None;
        }
        pos += 1; // picture type
        if enc == 1 || enc == 2 {
            while pos + 1 < data.len() && (data[pos] != 0 || data[pos + 1] != 0) {
                pos += 2;
            }
            pos += 2;
        } else {
            while pos < data.len() && data[pos] != 0 {
                pos += 1;
            }
            pos += 1;
        }
        if pos >= data.len() {
            return None;
        }
        Some(data[pos..].to_vec())
    }

    fn flac_picture(block: &[u8]) -> Option<Vec<u8>> {
        if block.len() < 32 {
            return None;
        }
        let mut idx = 4 + 4;
        if idx + 4 > block.len() {
            return None;
        }
        let mime_len = u32::from_be_bytes(block[idx..idx + 4].try_into().ok()?) as usize;
        idx += 4 + mime_len;
        if idx + 4 > block.len() {
            return None;
        }
        let desc_len = u32::from_be_bytes(block[idx..idx + 4].try_into().ok()?) as usize;
        idx += 4 + desc_len + 4 + 4 + 4 + 4;
        if idx + 4 > block.len() {
            return None;
        }
        let data_len = u32::from_be_bytes(block[idx..idx + 4].try_into().ok()?) as usize;
        idx += 4;
        if idx + data_len > block.len() || data_len == 0 {
            return None;
        }
        Some(block[idx..idx + data_len].to_vec())
    }
}
