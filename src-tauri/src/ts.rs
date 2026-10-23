//! A small MPEG-TS writer: the encoded frames with their own timestamps.
//!
//! A raw H.264 stream carries no time; ffmpeg had to spread it at one rate, so
//! a stall in the middle of a clip was smeared over all of it and pulled the
//! picture away from its sound (BUF-6). Wrapped in a transport stream, each
//! frame keeps the time it was captured at, and a hole stays a hole: the frame
//! before it is held.
//!
//! Only what ffmpeg's demuxer needs: one program, one H.264 stream that also
//! carries the clock, one PES per frame with its PTS. No DTS — only frames in
//! presentation order come this way ([`crate::muxer::keeps_timestamps`] for a
//! clip, [`crate::buffer::ReplayBuffer::reorders`] for a recording).

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

/// The PID the video and its clock (PCR) travel on.
pub const VIDEO_PID: u16 = 0x100;
/// The PID of the program map table.
const PMT_PID: u16 = 0x1000;
/// The first frame's PTS: 0.7 s at 90 kHz. The PCR runs this far behind the
/// PTS, the same gap ffmpeg's own TS muxer leaves.
pub const PTS_OFFSET_90K: u64 = 63_000;

pub const PACKET: usize = 188;
/// What follows the 4-byte packet header.
const BODY: usize = PACKET - 4;
const SYNC: u8 = 0x47;
const PAT_PID: u16 = 0;
const STREAM_TYPE_H264: u8 = 0x1B;
/// A PTS and a PCR base are 33 bits.
const WRAP_90K: u64 = (1 << 33) - 1;

pub struct TsWriter<W: Write> {
    out: W,
    /// The continuity counter of PAT, PMT and video, each mod 16.
    pat_cc: u8,
    pmt_cc: u8,
    video_cc: u8,
    started: bool,
}

impl<W: Write> TsWriter<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            pat_cc: 0,
            pmt_cc: 0,
            video_cc: 0,
            started: false,
        }
    }

    /// One access unit, Annex-B as the encoder gave it. `pts_us` counts from
    /// the first frame and only ever grows.
    pub fn write_frame(&mut self, pts_us: i64, keyframe: bool, data: &[u8]) -> std::io::Result<()> {
        if !self.started || keyframe {
            self.write_tables()?;
            self.started = true;
        }
        let pts_90k = (pts_us.max(0) as u64 * 9 + 50) / 100 + PTS_OFFSET_90K;

        let mut pes = Vec::with_capacity(14 + data.len());
        // Stream id 0xE0 (video), length 0: unbounded, allowed for video.
        pes.extend_from_slice(&[0x00, 0x00, 0x01, 0xE0, 0x00, 0x00]);
        // Marker bits, then "PTS only", then a header of 5 bytes.
        pes.extend_from_slice(&[0x80, 0x80, 0x05]);
        pes.extend_from_slice(&pts_bytes(pts_90k));
        pes.extend_from_slice(data);

        // A keyframe is a place to start reading, and carries the clock.
        let clock = keyframe.then(|| pts_90k.wrapping_sub(PTS_OFFSET_90K) & WRAP_90K);
        let mut rest = &pes[..];
        let mut first = true;
        while !rest.is_empty() {
            let pcr = if first { clock } else { None };
            let used = self.write_video_packet(first, pcr, rest)?;
            rest = &rest[used..];
            first = false;
        }
        Ok(())
    }

    /// Everything written so far, handed back.
    pub fn finish(mut self) -> std::io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }

    /// One packet of a video PES: as much of `rest` as fits, the room left
    /// over filled with adaptation-field stuffing. Returns how much it took.
    fn write_video_packet(&mut self, start: bool, pcr: Option<u64>, rest: &[u8]) -> std::io::Result<usize> {
        // The adaptation field's fixed part: its length byte, the flags and
        // the PCR, when there is a clock to carry.
        let fixed = if pcr.is_some() { 8 } else { 0 };
        let take = rest.len().min(BODY - fixed);
        let field = BODY - take;

        let mut packet = [0xFFu8; PACKET];
        packet[0] = SYNC;
        packet[1] = (u8::from(start) << 6) | (VIDEO_PID >> 8) as u8;
        packet[2] = VIDEO_PID as u8;
        let control = if field > 0 { 0b11 } else { 0b01 };
        packet[3] = (control << 4) | self.video_cc;
        self.video_cc = (self.video_cc + 1) & 0x0F;

        if field > 0 {
            // The length byte does not count itself.
            packet[4] = (field - 1) as u8;
            if field > 1 {
                packet[5] = match pcr {
                    // random_access_indicator and PCR_flag.
                    Some(_) => 0x50,
                    None => 0x00,
                };
                if let Some(base) = pcr {
                    // 33 bits of base, 6 reserved, 9 of extension (0).
                    packet[6] = (base >> 25) as u8;
                    packet[7] = (base >> 17) as u8;
                    packet[8] = (base >> 9) as u8;
                    packet[9] = (base >> 1) as u8;
                    packet[10] = ((base & 1) << 7) as u8 | 0x7E;
                    packet[11] = 0x00;
                }
                // The stuffing after it is the 0xFF already there.
            }
        }
        packet[4 + field..].copy_from_slice(&rest[..take]);
        self.out.write_all(&packet)?;
        Ok(take)
    }

    /// PAT and PMT, one packet each.
    fn write_tables(&mut self) -> std::io::Result<()> {
        // Program 1 is described on PMT_PID.
        let pat = section(0x00, 0x0001, &[0x00, 0x01, 0xE0 | (PMT_PID >> 8) as u8, PMT_PID as u8]);
        // The clock rides on the video PID; no program descriptors; one
        // H.264 stream on VIDEO_PID with no descriptors either.
        let pmt = section(
            0x02,
            0x0001,
            &[
                0xE0 | (VIDEO_PID >> 8) as u8,
                VIDEO_PID as u8,
                0xF0,
                0x00,
                STREAM_TYPE_H264,
                0xE0 | (VIDEO_PID >> 8) as u8,
                VIDEO_PID as u8,
                0xF0,
                0x00,
            ],
        );
        let pat_cc = self.pat_cc;
        self.pat_cc = (pat_cc + 1) & 0x0F;
        self.write_table(PAT_PID, pat_cc, &pat)?;
        let pmt_cc = self.pmt_cc;
        self.pmt_cc = (pmt_cc + 1) & 0x0F;
        self.write_table(PMT_PID, pmt_cc, &pmt)
    }

    fn write_table(&mut self, pid: u16, cc: u8, section: &[u8]) -> std::io::Result<()> {
        let mut packet = [0xFFu8; PACKET];
        packet[0] = SYNC;
        // Payload unit start: the section begins in this packet.
        packet[1] = 0x40 | (pid >> 8) as u8;
        packet[2] = pid as u8;
        packet[3] = 0x10 | cc;
        // Pointer field: the section follows at once.
        packet[4] = 0x00;
        packet[5..5 + section.len()].copy_from_slice(section);
        self.out.write_all(&packet)
    }
}

/// A PSI section with its CRC: `table_id`, the id after the length
/// (transport stream or program number), version 0, current, one section.
fn section(table_id: u8, id: u16, body: &[u8]) -> Vec<u8> {
    // From after the length field to the end of the CRC.
    let length = 5 + body.len() + 4;
    let mut out = vec![
        table_id,
        // section_syntax_indicator, '0', reserved bits, then the length.
        0xB0 | (length >> 8) as u8,
        length as u8,
        (id >> 8) as u8,
        id as u8,
        // Reserved bits, version 0, current_next_indicator.
        0xC1,
        0x00,
        0x00,
    ];
    out.extend_from_slice(body);
    let crc = crc32(&out);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

/// A 33-bit PTS in its 5-byte form: prefix `0010`, marker bits between.
fn pts_bytes(pts: u64) -> [u8; 5] {
    let pts = pts & WRAP_90K;
    [
        0x20 | ((pts >> 29) as u8 & 0x0E) | 0x01,
        (pts >> 22) as u8,
        ((pts >> 14) as u8 & 0xFE) | 0x01,
        (pts >> 7) as u8,
        ((pts << 1) as u8 & 0xFE) | 0x01,
    ]
}

/// The PTS of each PES on [`VIDEO_PID`], in the order they come.
pub fn read_pts(bytes: &[u8]) -> Vec<u64> {
    let mut found = Vec::new();
    for packet in bytes.chunks_exact(PACKET) {
        let pid = u16::from(packet[1] & 0x1F) << 8 | u16::from(packet[2]);
        if packet[0] != SYNC || pid != VIDEO_PID || packet[1] & 0x40 == 0 {
            continue;
        }
        let start = match packet[3] >> 4 & 0b11 {
            0b01 => 4,
            0b11 => 5 + packet[4] as usize,
            _ => continue,
        };
        let Some(pes) = packet.get(start..) else {
            continue;
        };
        if pes.len() < 14 || pes[..3] != [0, 0, 1] || pes[7] & 0x80 == 0 {
            continue;
        }
        let b = &pes[9..14];
        found.push(
            u64::from(b[0] >> 1 & 0x07) << 30
                | u64::from(b[1]) << 22
                | u64::from(b[2] >> 1) << 15
                | u64::from(b[3]) << 7
                | u64::from(b[4] >> 1),
        );
    }
    found
}

/// How long the picture in a TS file lasts, from its first frame to the end
/// of its last, in microseconds — for a recording that never stopped cleanly
/// and so never wrote its span down (REC-6).
///
/// Only the two ends are read: the first 64 KiB and the last 1 MiB, the
/// latter from a packet boundary counted from the start of the file. A packet
/// cut off at the end, as a crash leaves it, is not read.
pub fn span_us(path: &Path, fps: u32) -> Option<u64> {
    const HEAD: u64 = 64 << 10;
    const TAIL: u64 = 1 << 20;
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut read = |from: u64, max: u64| -> Option<Vec<u64>> {
        file.seek(SeekFrom::Start(from)).ok()?;
        let mut bytes = Vec::new();
        (&mut file).take(max).read_to_end(&mut bytes).ok()?;
        Some(read_pts(&bytes))
    };
    let first = *read(0, HEAD)?.first()?;
    // Windows going down can leave the end of the file zero-filled, and one
    // frame can be larger than the window: without a frame in it, the window
    // steps back until it finds one.
    let mut end = len;
    let last = loop {
        let start = end.saturating_sub(TAIL);
        let start = start - start % PACKET as u64;
        if let Some(&last) = read(start, end - start)?.last() {
            break last;
        }
        if start == 0 {
            return None;
        }
        end = start;
    };
    // Past 26.5 hours the 33-bit clock has wrapped once.
    let ticks = (last + WRAP_90K + 1 - first) & WRAP_90K;
    Some(ticks * 100 / 9 + 1_000_000 / fps.max(1) as u64)
}

/// CRC-32/MPEG-2: polynomial 0x04C11DB7, starting at all ones, unreflected,
/// with no final XOR. Over a whole section, its CRC included, it comes to 0.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `len` bytes that starts like an access unit.
    fn frame(len: usize) -> Vec<u8> {
        let mut data = vec![0, 0, 0, 1, 0x41];
        data.extend((0..len.saturating_sub(5)).map(|at| (at % 251) as u8));
        data.truncate(len);
        data
    }

    fn packets(bytes: &[u8]) -> Vec<&[u8]> {
        bytes.chunks(188).collect()
    }

    fn pid(packet: &[u8]) -> u16 {
        u16::from(packet[1] & 0x1F) << 8 | u16::from(packet[2])
    }

    /// The payload of a packet: after its header and adaptation field.
    fn payload(packet: &[u8]) -> &[u8] {
        match packet[3] >> 4 & 0b11 {
            0b01 => &packet[4..],
            0b11 => &packet[5 + packet[4] as usize..],
            _ => &[],
        }
    }

    /// Every PES on `VIDEO_PID`, its header taken off.
    fn frames(bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out: Vec<Vec<u8>> = Vec::new();
        for packet in packets(bytes).into_iter().filter(|p| pid(p) == VIDEO_PID) {
            if packet[1] & 0x40 != 0 {
                let pes = payload(packet);
                assert_eq!(&pes[..4], &[0, 0, 1, 0xE0]);
                out.push(pes[9 + pes[8] as usize..].to_vec());
            } else {
                out.last_mut().unwrap().extend_from_slice(payload(packet));
            }
        }
        out
    }

    /// BUF-6: each frame's own time reaches ffmpeg, at 90 kHz and 0.7 s in.
    #[test]
    fn a_frame_carries_its_time_at_90_khz() {
        let mut ts = TsWriter::new(Vec::new());
        for (pts, key) in [(0, true), (16_666, false), (1_516_666, false)] {
            ts.write_frame(pts, key, &[0, 0, 0, 1, 0x65, 0x88]).unwrap();
        }
        assert_eq!(read_pts(&ts.finish().unwrap()), vec![63_000, 64_500, 199_500]);
    }

    /// BUF-6: a 1 s hole stays a 1 s hole, instead of being spread over the clip.
    #[test]
    fn a_hole_stays_a_hole() {
        let mut ts = TsWriter::new(Vec::new());
        for tick in (0..60).chain(120..180) {
            ts.write_frame(tick * 1_000_000 / 60, tick % 120 == 0, &[0, 0, 0, 1, 0x41]).unwrap();
        }
        let pts = read_pts(&ts.finish().unwrap());
        let steps: Vec<u64> = pts.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(steps.iter().filter(|s| (1_499..=1_501).contains(*s)).count(), 118);
        assert!((91_490..=91_510).contains(&steps[59]), "the hole is {} ticks", steps[59]);
    }

    /// REC-6: a recording that never stopped cleanly takes its length from
    /// its own first and last timestamp. A crash leaves a cut-off packet at
    /// the end, which is not read as one.
    #[test]
    fn the_span_of_a_ts_file_is_read_from_its_ends() {
        let mut ts = TsWriter::new(Vec::new());
        for k in 0..120 {
            ts.write_frame(k * 16_666, k % 60 == 0, &frame(12_000)).unwrap();
        }
        let mut bytes = ts.finish().unwrap();
        bytes.extend([0x47u8; 100]);
        let dir = std::env::temp_dir().join("clippiboy-ts-span");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("video.ts");
        std::fs::write(&path, &bytes).unwrap();
        let span = span_us(&path, 60).expect("a span");
        assert!((1_999_900..=1_999_940).contains(&span), "{span}");
        // Long enough that the two ends are read apart, the second from a
        // start that has to be put back on a packet boundary.
        assert!(bytes.len() > (64 << 10) + (1 << 20));
        std::fs::write(&path, b"").unwrap();
        assert_eq!(span_us(&path, 60), None, "an empty file has no span");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REC-6: Windows going down can leave more than the last window of a
    /// recording zero-filled. The span is still read, from the last frame
    /// found further back; a file with no frame at all has none.
    #[test]
    fn a_zero_filled_end_does_not_hide_the_last_frame() {
        let mut ts = TsWriter::new(Vec::new());
        for k in 0..120 {
            ts.write_frame(k * 16_666, k % 60 == 0, &frame(12_000)).unwrap();
        }
        let mut bytes = ts.finish().unwrap();
        bytes.extend(std::iter::repeat_n(0u8, (1 << 20) + 5_000));
        let dir = std::env::temp_dir().join("clippiboy-ts-zero-tail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("video.ts");
        std::fs::write(&path, &bytes).unwrap();
        let span = span_us(&path, 60).expect("a span");
        assert!((1_999_900..=1_999_940).contains(&span), "{span}");
        std::fs::write(&path, vec![0u8; 3 << 20]).unwrap();
        assert_eq!(span_us(&path, 60), None, "no frame, no span");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The check value of CRC-32/MPEG-2.
    #[test]
    fn the_tables_carry_the_mpeg2_crc() {
        assert_eq!(crc32(b"123456789"), 0x0376_E6E7);
    }

    /// BUF-6: a demuxer finds each packet by its length and sync byte.
    #[test]
    fn every_packet_is_188_bytes_with_its_sync_byte() {
        let mut ts = TsWriter::new(Vec::new());
        for (at, len) in [10, 500, 5_000].into_iter().enumerate() {
            ts.write_frame(at as i64 * 16_666, at == 0, &frame(len)).unwrap();
        }
        let bytes = ts.finish().unwrap();
        assert_eq!(bytes.len() % 188, 0);
        assert!(packets(&bytes).iter().all(|p| p[0] == 0x47));
    }

    /// BUF-6: a gap in a PID's counter reads as a lost packet, so each PID
    /// counts its own, on packets with payload.
    #[test]
    fn continuity_counters_count_per_pid() {
        let mut ts = TsWriter::new(Vec::new());
        ts.write_frame(0, true, &frame(5_000)).unwrap();
        ts.write_frame(16_666, true, &frame(5_000)).unwrap();
        let bytes = ts.finish().unwrap();
        let counters = |wanted: u16| -> Vec<u8> {
            packets(&bytes)
                .into_iter()
                .filter(|p| pid(p) == wanted)
                .map(|p| p[3] & 0x0F)
                .collect()
        };
        assert_eq!(counters(0), vec![0, 1]);
        assert_eq!(counters(PMT_PID), vec![0, 1]);
        let video = counters(VIDEO_PID);
        assert!(video.len() > 16, "{}", video.len());
        for (at, counter) in video.into_iter().enumerate() {
            assert_eq!(counter, (at % 16) as u8, "packet {at}");
        }
    }

    /// BUF-6: ffmpeg can start reading at any keyframe, and the tables say
    /// what it is reading.
    #[test]
    fn tables_come_before_every_keyframe() {
        let mut ts = TsWriter::new(Vec::new());
        for (at, key) in [true, false, true].into_iter().enumerate() {
            ts.write_frame(at as i64 * 16_666, key, &frame(400)).unwrap();
        }
        let bytes = ts.finish().unwrap();
        let mut pids: Vec<u16> = packets(&bytes).into_iter().map(pid).collect();
        pids.dedup();
        assert_eq!(pids, vec![0, PMT_PID, VIDEO_PID, 0, PMT_PID, VIDEO_PID]);
        // Three frames of 400 bytes are three packets each.
        assert_eq!(bytes.len(), 188 * (2 + 3 + 3 + 2 + 3));
    }

    /// BUF-6: the tables say one H.264 stream on `VIDEO_PID`, which also
    /// carries the clock, and each section's CRC checks.
    #[test]
    fn the_tables_describe_one_h264_stream() {
        let mut ts = TsWriter::new(Vec::new());
        ts.write_frame(0, true, &frame(10)).unwrap();
        let bytes = ts.finish().unwrap();
        let all = packets(&bytes);
        for packet in &all[..2] {
            let body = payload(packet);
            assert_eq!(body[0], 0, "pointer field");
            let length = (usize::from(body[2] & 0x0F) << 8) | usize::from(body[3]);
            let section = &body[1..4 + length];
            assert_eq!(crc32(section), 0, "a whole section checks to zero");
            assert!(body[4 + length..].iter().all(|b| *b == 0xFF));
        }
        let pat = &payload(all[0])[1..];
        assert_eq!(pat[0], 0x00);
        assert_eq!(u16::from(pat[10] & 0x1F) << 8 | u16::from(pat[11]), PMT_PID);
        let pmt = &payload(all[1])[1..];
        assert_eq!(pmt[0], 0x02);
        assert_eq!(u16::from(pmt[8] & 0x1F) << 8 | u16::from(pmt[9]), VIDEO_PID, "PCR PID");
        assert_eq!(pmt[12], 0x1B, "stream type H.264");
        assert_eq!(u16::from(pmt[13] & 0x1F) << 8 | u16::from(pmt[14]), VIDEO_PID);
    }

    /// BUF-6: a keyframe is marked as a place to start, with the clock 0.7 s
    /// before its picture; other frames carry neither.
    #[test]
    fn a_keyframe_carries_its_clock_and_the_random_access_flag() {
        let mut ts = TsWriter::new(Vec::new());
        ts.write_frame(0, true, &frame(400)).unwrap();
        ts.write_frame(1_000_000, true, &frame(400)).unwrap();
        ts.write_frame(1_016_666, false, &frame(400)).unwrap();
        let bytes = ts.finish().unwrap();
        let starts: Vec<&[u8]> = packets(&bytes)
            .into_iter()
            .filter(|p| pid(p) == VIDEO_PID && p[1] & 0x40 != 0)
            .collect();
        let pcr = |packet: &[u8]| -> Option<u64> {
            if packet[3] & 0x20 == 0 || packet[4] == 0 || packet[5] & 0x10 == 0 {
                return None;
            }
            let b = &packet[6..12];
            let base = u64::from(b[0]) << 25
                | u64::from(b[1]) << 17
                | u64::from(b[2]) << 9
                | u64::from(b[3]) << 1
                | u64::from(b[4]) >> 7;
            assert_eq!((u16::from(b[4] & 1) << 8) | u16::from(b[5]), 0, "extension");
            Some(base)
        };
        assert_eq!(starts[0][5] & 0x40, 0x40, "random access");
        assert_eq!(pcr(starts[0]), Some(0));
        assert_eq!(pcr(starts[1]), Some(90_000));
        assert_eq!(pcr(starts[2]), None);
        assert!(starts[2][3] & 0x20 == 0 || starts[2][5] & 0x40 == 0);
    }

    /// BUF-6: the frames reach ffmpeg exactly as the encoder gave them — a
    /// short last packet is filled in its adaptation field, never with bytes
    /// the decoder would read as picture.
    #[test]
    fn the_payload_comes_back_byte_for_byte() {
        let lengths = [1, 5, 175, 176, 177, 182, 183, 184, 185, 360, 367, 368, 5_000];
        for key in [true, false] {
            let mut ts = TsWriter::new(Vec::new());
            let sent: Vec<Vec<u8>> = lengths.iter().map(|len| frame(*len)).collect();
            for (at, data) in sent.iter().enumerate() {
                ts.write_frame(at as i64 * 16_666, key && at % 2 == 0, data).unwrap();
            }
            let bytes = ts.finish().unwrap();
            assert_eq!(bytes.len() % 188, 0);
            assert_eq!(frames(&bytes), sent, "keyframes: {key}");
        }
    }
}
