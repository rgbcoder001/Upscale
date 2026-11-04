//! The y4m frame pump between the last video producer (vspipe or Topaz) and the encoder
//!. It counts frames for progress and pauses by not reading: backpressure then
//! blocks every upstream process, and the encoder just waits for input. Bytes pass through
//! untouched; nothing here decodes text except the header and `FRAME` lines.

use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::watch;

/// Longest header or FRAME line accepted (they are a few dozen bytes).
const MAX_LINE: usize = 4096;
const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Y4mHeader {
    pub width: u32,
    pub height: u32,
    /// The `C` tag, e.g. `420p10`; `420jpeg` when absent (the y4m default).
    pub colorspace: String,
    /// Bytes of picture data per frame.
    pub frame_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PumpError {
    #[error("not a y4m stream (starts with {0:?})")]
    NotY4m(String),
    #[error("unsupported y4m colourspace '{0}'")]
    Colorspace(String),
    #[error("bad y4m frame header {0:?}")]
    BadFrame(String),
    #[error("y4m stream ended inside frame {0}")]
    Truncated(u64),
    #[error("reading video: {0}")]
    Read(std::io::Error),
    #[error("writing to the encoder: {0}")]
    Write(std::io::Error),
}

/// Bytes per frame for a y4m colourspace tag.
pub fn frame_bytes(width: u32, height: u32, colorspace: &str) -> Option<usize> {
    let (w, h) = (width as usize, height as usize);
    let (layout, depth) = match colorspace.find('p') {
        Some(i) if colorspace[i + 1..].parse::<u32>().is_ok() => {
            (&colorspace[..i], colorspace[i + 1..].parse::<u32>().ok()?)
        }
        _ => (colorspace, 8),
    };
    let sample = if depth > 8 { 2 } else { 1 };
    let (cw, ch) = match layout {
        "420" | "420jpeg" | "420paldv" | "420mpeg2" => (w.div_ceil(2), h.div_ceil(2)),
        "422" => (w.div_ceil(2), h),
        "411" => (w.div_ceil(4), h),
        "444" => (w, h),
        "mono" => (0, 0),
        "444alpha" => return Some(w * h * 4 * sample),
        _ => return None,
    };
    Some((w * h + 2 * cw * ch) * sample)
}

pub fn parse_header(line: &str) -> Result<Y4mHeader, PumpError> {
    let mut fields = line.trim_end().split(' ');
    if fields.next() != Some("YUV4MPEG2") {
        return Err(PumpError::NotY4m(line.chars().take(16).collect()));
    }
    let (mut width, mut height, mut colorspace) = (0, 0, "420jpeg".to_string());
    for f in fields {
        match f.split_at_checked(1) {
            Some(("W", v)) => width = v.parse().unwrap_or(0),
            Some(("H", v)) => height = v.parse().unwrap_or(0),
            Some(("C", v)) => colorspace = v.to_string(),
            _ => {}
        }
    }
    let frame_bytes = frame_bytes(width, height, &colorspace)
        .ok_or_else(|| PumpError::Colorspace(colorspace.clone()))?;
    Ok(Y4mHeader {
        width,
        height,
        colorspace,
        frame_bytes,
    })
}

/// Reads one `\n`-terminated line byte by byte (the stream must not be over-read).
async fn read_line<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> Result<bool, PumpError> {
    buf.clear();
    loop {
        let mut b = [0u8; 1];
        match r.read(&mut b).await.map_err(PumpError::Read)? {
            0 if buf.is_empty() => return Ok(false),
            0 => return Ok(true),
            _ if b[0] == b'\n' => return Ok(true),
            _ => {
                buf.push(b[0]);
                if buf.len() > MAX_LINE {
                    return Ok(true);
                }
            }
        }
    }
}

/// Waits while `paused` is true.
pub async fn wait_unpaused(paused: &mut watch::Receiver<bool>) {
    // A dropped sender means nobody can pause any more.
    let _ = paused.wait_for(|p| !*p).await;
}

/// Copies a y4m stream from `r` to `w`, adding one to `frames` per frame forwarded. Returns
/// the header at the end of the stream. `w` is shut down at the end so the encoder sees EOF.
pub async fn pump<R, W>(
    r: R,
    mut w: W,
    mut paused: watch::Receiver<bool>,
    frames: &AtomicU64,
) -> Result<Y4mHeader, PumpError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    // Buffered so FRAME lines don't cost a read per byte; the pump owns the stream, so
    // reading ahead is harmless (and while paused nothing is read at all).
    let mut r = tokio::io::BufReader::with_capacity(CHUNK, r);
    let mut line = Vec::with_capacity(128);
    if !read_line(&mut r, &mut line).await? {
        return Err(PumpError::NotY4m(String::new()));
    }
    let header = parse_header(&String::from_utf8_lossy(&line))?;
    line.push(b'\n');
    w.write_all(&line).await.map_err(PumpError::Write)?;

    let mut buf = vec![0u8; CHUNK.min(header.frame_bytes.max(1))];
    let mut n: u64 = 0;
    loop {
        wait_unpaused(&mut paused).await;
        if !read_line(&mut r, &mut line).await? {
            break;
        }
        if !line.starts_with(b"FRAME") {
            return Err(PumpError::BadFrame(
                String::from_utf8_lossy(&line[..line.len().min(32)]).into_owned(),
            ));
        }
        line.push(b'\n');
        w.write_all(&line).await.map_err(PumpError::Write)?;
        let mut left = header.frame_bytes;
        while left > 0 {
            let want = left.min(buf.len());
            let got = r.read(&mut buf[..want]).await.map_err(PumpError::Read)?;
            if got == 0 {
                return Err(PumpError::Truncated(n + 1));
            }
            w.write_all(&buf[..got]).await.map_err(PumpError::Write)?;
            left -= got;
        }
        n += 1;
        frames.store(n, Ordering::Relaxed);
    }
    w.shutdown().await.map_err(PumpError::Write)?;
    Ok(header)
}

/// fps over a sliding window of `(seconds, frames)` samples.
#[derive(Debug, Default)]
pub struct Rate {
    samples: std::collections::VecDeque<(f64, u64)>,
}

impl Rate {
    pub const WINDOW_SECS: f64 = 10.0;

    pub fn add(&mut self, t: f64, frames: u64) -> f64 {
        self.samples.push_back((t, frames));
        while self.samples.len() > 2 && t - self.samples[1].0 >= Self::WINDOW_SECS {
            self.samples.pop_front();
        }
        let (t0, f0) = self.samples[0];
        if t > t0 {
            (frames - f0) as f64 / (t - t0)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn frame_sizes() {
        assert_eq!(frame_bytes(3840, 2160, "420p10"), Some(3840 * 2160 * 3));
        assert_eq!(
            frame_bytes(1920, 1080, "420jpeg"),
            Some(1920 * 1080 * 3 / 2)
        );
        assert_eq!(
            frame_bytes(5, 3, "420"),
            Some(15 + 2 * 3 * 2),
            "odd sizes round chroma up"
        );
        assert_eq!(frame_bytes(4, 2, "444p16"), Some(4 * 2 * 3 * 2));
        assert_eq!(frame_bytes(4, 2, "422"), Some(8 + 2 * 2 * 2));
        assert_eq!(frame_bytes(4, 2, "mono"), Some(8));
        assert_eq!(frame_bytes(4, 2, "xyz"), None);
    }

    #[test]
    fn headers() {
        let h = parse_header("YUV4MPEG2 W3840 H2160 F24000:1001 Ip A1:1 C420p10 XYSCSS=420P10")
            .unwrap();
        assert_eq!(
            (h.width, h.height, h.colorspace.as_str(), h.frame_bytes),
            (3840, 2160, "420p10", 3840 * 2160 * 3)
        );
        assert_eq!(
            parse_header("YUV4MPEG2 W4 H2").unwrap().colorspace,
            "420jpeg"
        );
        assert!(matches!(
            parse_header("[tvai] hello"),
            Err(PumpError::NotY4m(_))
        ));
    }

    /// A 4x2 10-bit 4:2:0 stream of `n` frames (24 bytes each).
    fn stream(n: usize) -> Vec<u8> {
        let mut s = b"YUV4MPEG2 W4 H2 F24:1 Ip C420p10\n".to_vec();
        for i in 0..n {
            s.extend_from_slice(if i % 2 == 0 {
                b"FRAME\n".as_slice()
            } else {
                b"FRAME Ixyz\n".as_slice()
            });
            s.extend(std::iter::repeat_n(i as u8, 24));
        }
        s
    }

    #[tokio::test]
    async fn passes_bytes_through_and_counts_frames() {
        let input = stream(5);
        let mut out = Vec::new();
        let (_tx, rx) = watch::channel(false);
        let frames = AtomicU64::new(0);
        let h = pump(input.as_slice(), &mut out, rx, &frames).await.unwrap();
        assert_eq!(h.frame_bytes, 24);
        assert_eq!(frames.load(Ordering::Relaxed), 5);
        assert_eq!(out, input);
    }

    #[tokio::test]
    async fn truncated_and_corrupt_streams_fail() {
        let mut s = stream(2);
        s.truncate(s.len() - 3);
        let (_tx, rx) = watch::channel(false);
        let err = pump(
            s.as_slice(),
            tokio::io::sink(),
            rx.clone(),
            &AtomicU64::new(0),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PumpError::Truncated(2)));
        let mut s = stream(1);
        s.extend_from_slice(b"Svt[info]: oops\n");
        let err = pump(s.as_slice(), tokio::io::sink(), rx, &AtomicU64::new(0))
            .await
            .unwrap_err();
        assert!(matches!(err, PumpError::BadFrame(_)));
    }

    #[tokio::test]
    async fn pause_blocks_the_producer() {
        // A small pipe, so a producer that isn't read from blocks quickly
        let (mut producer, pipe_out) = tokio::io::duplex(64);
        let (tx, rx) = watch::channel(true);
        let frames = Arc::new(AtomicU64::new(0));
        let counter = frames.clone();
        let pumping =
            tokio::spawn(async move { pump(pipe_out, tokio::io::sink(), rx, &counter).await });

        let data = stream(50);
        let write = producer.write_all(&data);
        tokio::pin!(write);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut write)
                .await
                .is_err(),
            "the producer must block while paused"
        );
        assert_eq!(frames.load(Ordering::Relaxed), 0);

        tx.send(false).unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut write)
            .await
            .unwrap()
            .unwrap();
        drop(producer);
        pumping.await.unwrap().unwrap();
        assert_eq!(frames.load(Ordering::Relaxed), 50);
    }

    #[test]
    fn sliding_fps() {
        let mut r = Rate::default();
        assert_eq!(r.add(0.0, 0), 0.0);
        assert_eq!(r.add(1.0, 10), 10.0);
        assert_eq!(r.add(2.0, 30), 15.0);
        for t in 3..=30 {
            r.add(t as f64, 30 + (t as u64 - 2) * 12);
        }
        assert!(
            (r.add(31.0, 30 + 29 * 12) - 12.0).abs() < 1e-9,
            "old samples drop out of the window"
        );
    }
}
