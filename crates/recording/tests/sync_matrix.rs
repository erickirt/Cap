//! Synthetic device matrix for A/V sync.
//!
//! Drives the real recording pipeline (sources -> mux loop -> encoders ->
//! containers) with synthetic video and audio across sample rates, channel
//! counts, frame rates and delivery pathologies (jitter, drops, gaps), then
//! verifies the muxed output preserves real time. No capture hardware is
//! required, so this runs identically on macOS, Windows and Linux CI.
//!
//! Frames are emitted in real time because the pipeline pins video
//! timestamps to the wall clock; each case therefore costs its content
//! duration. Keep cases short.
//!
//! Set `CAP_SYNC_MATRIX_REPORT` to write a JSON report of every case.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use cap_media_info::{AudioInfo, Sample, Type, VideoInfo};
use cap_recording::{
    AudioFrame, ChannelAudioSource, ChannelAudioSourceConfig, ChannelVideoSource,
    ChannelVideoSourceConfig, OutputPipeline,
    ffmpeg::{
        FFmpegVideoFrame, Mp4Muxer, OggMuxer, SegmentedVideoMuxer, SegmentedVideoMuxerConfig,
    },
};
use cap_timestamp::{Timestamp, Timestamps};
use serde::Serialize;

const CONTENT_SECS: f64 = 4.0;
/// Absolute tolerance for a muxed pts vs the sent capture timestamp. Covers
/// warmup anchoring, emission jitter and encoder rounding.
const ABS_TOLERANCE_SECS: f64 = 0.20;
/// Tolerance for the relative structure (pts deltas vs sent deltas), which is
/// what actually determines sync drift.
const REL_TOLERANCE_SECS: f64 = 0.10;
/// Tolerance for decoded audio duration vs generated duration.
const AUDIO_DURATION_TOLERANCE_SECS: f64 = 0.15;

#[derive(Debug, Clone, Copy)]
enum VideoScenario {
    Steady,
    Jitter,
    Drops,
    Gap,
}

impl VideoScenario {
    fn name(self) -> &'static str {
        match self {
            Self::Steady => "steady",
            Self::Jitter => "jitter",
            Self::Drops => "drops",
            Self::Gap => "gap",
        }
    }

    /// Deterministic capture timestamps (seconds) for the scenario.
    fn timestamps(self, fps: u32) -> Vec<f64> {
        let period = 1.0 / f64::from(fps);
        let total = (CONTENT_SECS * f64::from(fps)) as u64;
        let mut out = Vec::new();
        for k in 0..total {
            let base = k as f64 * period;
            match self {
                Self::Steady => out.push(base),
                Self::Jitter => {
                    // Deterministic pseudo-jitter, +-40% of the period, kept
                    // monotonic (and non-negative) by construction.
                    let phase = (k as f64 * 0.7368).fract() - 0.5;
                    out.push((base + phase * period * 0.8).max(0.0));
                }
                Self::Drops => {
                    // Drop every 4th and 7th frame: the capture stream simply
                    // never delivers them.
                    if k % 4 != 3 && k % 7 != 6 {
                        out.push(base);
                    }
                }
                Self::Gap => {
                    // 1.5s of frames, a 2s static-screen gap, then the rest.
                    let t = base;
                    if t < 1.5 {
                        out.push(t);
                    } else {
                        out.push(t + 2.0);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.partial_cmp(b).unwrap());
        out.dedup_by(|a, b| (*a - *b).abs() < period * 0.25);
        out
    }
}

#[derive(Serialize)]
struct CaseResult {
    name: String,
    pass: bool,
    detail: String,
}

fn make_video_frame(width: u32, height: u32, shade: u8) -> ffmpeg::frame::Video {
    let mut frame = ffmpeg::frame::Video::new(ffmpeg::format::Pixel::BGRA, width, height);
    for byte in frame.data_mut(0).iter_mut() {
        *byte = shade;
    }
    frame
}

async fn run_video_case(
    fps: u32,
    scenario: VideoScenario,
    fragmented: bool,
) -> Result<String, String> {
    let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let out_path = if fragmented {
        temp.path().join("display")
    } else {
        temp.path().join("display.mp4")
    };

    let info = VideoInfo::from_raw(cap_media_info::RawVideoFormat::Bgra, 160, 120, fps);
    let (tx, rx) = flume::bounded::<FFmpegVideoFrame>(32);
    let timestamps = Timestamps::now();

    let sent = scenario.timestamps(fps);
    let emit = {
        let sent = sent.clone();
        let base = timestamps.instant();
        tokio::spawn(async move {
            for &ts in &sent {
                tokio::time::sleep_until((base + Duration::from_secs_f64(ts)).into()).await;
                let frame = FFmpegVideoFrame {
                    inner: make_video_frame(160, 120, ((ts * 40.0) as u8).wrapping_mul(3)),
                    timestamp: Timestamp::Instant(base + Duration::from_secs_f64(ts)),
                };
                if tx.send_async(frame).await.is_err() {
                    break;
                }
            }
            // Sender drops here, ending the stream.
        })
    };

    let builder = OutputPipeline::builder(out_path.clone())
        .with_video::<ChannelVideoSource<FFmpegVideoFrame>>(ChannelVideoSourceConfig::new(info, rx))
        .with_timestamps(timestamps);

    let pipeline = if fragmented {
        builder
            .build::<SegmentedVideoMuxer>(SegmentedVideoMuxerConfig {
                segment_duration: Duration::from_secs(2),
                ..Default::default()
            })
            .await
    } else {
        builder.build::<Mp4Muxer>(()).await
    }
    .map_err(|e| format!("pipeline build: {e}"))?;

    emit.await.map_err(|e| format!("emit join: {e}"))?;
    // Allow the tail of the stream to flush through the encoder.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let finished = pipeline.stop().await.map_err(|e| format!("stop: {e}"))?;

    // Read back the muxed pts.
    let playable = if fragmented {
        concat_fmp4(&out_path, temp.path())?
    } else {
        out_path.clone()
    };
    let pts = read_video_pts(&playable)?;

    if pts.len() != sent.len() {
        return Err(format!(
            "frame count mismatch: sent {} frames, container has {}",
            sent.len(),
            pts.len()
        ));
    }

    let mut max_abs: f64 = 0.0;
    let mut max_rel: f64 = 0.0;
    for (i, (&p, &s)) in pts.iter().zip(&sent).enumerate() {
        max_abs = max_abs.max((p - s).abs());
        let rel = ((p - pts[0]) - (s - sent[0])).abs();
        max_rel = max_rel.max(rel);
        if rel > REL_TOLERANCE_SECS {
            return Err(format!(
                "frame {i}: relative pts error {rel:.3}s (pts {p:.3}s vs sent {s:.3}s)"
            ));
        }
    }
    if max_abs > ABS_TOLERANCE_SECS {
        return Err(format!(
            "absolute pts error {max_abs:.3}s exceeds tolerance"
        ));
    }

    // The span the recorder would persist must match the sent span.
    if let Some((first, last)) = finished.video_timestamp_span {
        let span = (last - first).as_secs_f64();
        let expected = sent.last().unwrap() - sent[0];
        if (span - expected).abs() > 0.25 {
            return Err(format!(
                "video_timestamp_span {span:.3}s does not match sent span {expected:.3}s"
            ));
        }
    } else {
        return Err("video_timestamp_span missing".to_string());
    }

    // Gap preservation is the regression that desynced 0.5.4.
    if matches!(scenario, VideoScenario::Gap) {
        let mut max_gap: f64 = 0.0;
        for pair in pts.windows(2) {
            max_gap = max_gap.max(pair[1] - pair[0]);
        }
        if max_gap < 1.8 {
            return Err(format!(
                "2s capture gap collapsed to {max_gap:.3}s in the container"
            ));
        }
    }

    Ok(format!(
        "{} frames, max abs err {:.0} ms, max rel err {:.0} ms",
        pts.len(),
        max_abs * 1000.0,
        max_rel * 1000.0
    ))
}

async fn run_audio_case(rate: u32, channels: u16) -> Result<String, String> {
    let temp = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let out_path = temp.path().join("audio.ogg");

    let info = AudioInfo::new(Sample::F32(Type::Packed), rate, channels)
        .map_err(|e| format!("audio info: {e:?}"))?;
    let (tx, rx) = futures::channel::mpsc::channel::<AudioFrame>(32);
    let timestamps = Timestamps::now();

    let chunk_frames = (rate / 50).max(1) as usize; // 20ms chunks
    let total_chunks = (CONTENT_SECS * 50.0) as usize;

    let emit = {
        let base = timestamps.instant();
        let mut tx = tx;
        let info = info;
        tokio::spawn(async move {
            use futures::SinkExt;
            for k in 0..total_chunks {
                let ts = k as f64 * 0.02;
                tokio::time::sleep_until((base + Duration::from_secs_f64(ts)).into()).await;
                let mut frame = ffmpeg::frame::Audio::new(
                    ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Packed),
                    chunk_frames,
                    info.channel_layout(),
                );
                frame.set_rate(rate);
                let data = frame.data_mut(0);
                for (i, sample) in bytemuck_cast_f32(data).iter_mut().enumerate() {
                    let n = (k * chunk_frames + i / channels as usize) as f32;
                    *sample = (n * 440.0 * 2.0 * std::f32::consts::PI / rate as f32).sin() * 0.4;
                }
                let frame = AudioFrame::new(
                    frame,
                    Timestamp::Instant(base + Duration::from_secs_f64(ts)),
                );
                if tx.send(frame).await.is_err() {
                    break;
                }
            }
        })
    };

    let pipeline = OutputPipeline::builder(out_path.clone())
        .with_audio_source::<ChannelAudioSource>(ChannelAudioSourceConfig::new(info, rx))
        .with_timestamps(timestamps)
        .build::<OggMuxer>(())
        .await
        .map_err(|e| format!("pipeline build: {e}"))?;

    emit.await.map_err(|e| format!("emit join: {e}"))?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    pipeline.stop().await.map_err(|e| format!("stop: {e}"))?;

    let (duration, decoded_channels, energy) = read_audio_stats(&out_path)?;
    if (duration - CONTENT_SECS).abs() > AUDIO_DURATION_TOLERANCE_SECS {
        return Err(format!(
            "decoded duration {duration:.3}s vs expected {CONTENT_SECS:.3}s \
             (rate handling error: content plays at the wrong speed)"
        ));
    }
    if energy < 0.01 {
        return Err(format!(
            "decoded audio is nearly silent (rms {energy:.4}); samples were lost or zeroed"
        ));
    }

    Ok(format!(
        "duration {duration:.3}s, {decoded_channels} ch decoded, rms {energy:.3}"
    ))
}

fn bytemuck_cast_f32(data: &mut [u8]) -> &mut [f32] {
    let len = data.len() / 4;
    unsafe { std::slice::from_raw_parts_mut(data.as_mut_ptr().cast::<f32>(), len) }
}

/// Concatenates a fragmented-mp4 segment directory (init.mp4 + *.m4s) into a
/// single playable file.
fn concat_fmp4(dir: &Path, scratch: &Path) -> Result<PathBuf, String> {
    let init = dir.join("init.mp4");
    let mut bytes = std::fs::read(&init).map_err(|e| format!("read init.mp4: {e}"))?;
    let mut segments: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("read segment dir: {e}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "m4s"))
        .collect();
    segments.sort();
    if segments.is_empty() {
        return Err("no media segments produced".to_string());
    }
    for segment in &segments {
        bytes.extend(std::fs::read(segment).map_err(|e| format!("read segment: {e}"))?);
    }
    let out = scratch.join("concat.mp4");
    std::fs::write(&out, bytes).map_err(|e| format!("write concat: {e}"))?;
    Ok(out)
}

fn read_video_pts(path: &Path) -> Result<Vec<f64>, String> {
    let mut ictx = ffmpeg::format::input(&path).map_err(|e| format!("open {e}"))?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or("no video stream")?;
    let index = stream.index();
    let tb = stream.time_base();
    let tb = f64::from(tb.numerator()) / f64::from(tb.denominator());
    let mut pts: Vec<f64> = ictx
        .packets()
        .filter_map(|(s, p)| (s.index() == index).then_some(p.pts()).flatten())
        .map(|p| p as f64 * tb)
        .collect();
    pts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(pts)
}

fn read_audio_stats(path: &Path) -> Result<(f64, u16, f64), String> {
    let mut ictx = ffmpeg::format::input(&path).map_err(|e| format!("open {e}"))?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Audio)
        .ok_or("no audio stream")?;
    let index = stream.index();
    let ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
        .map_err(|e| format!("params: {e}"))?;
    let mut decoder = ctx.decoder().audio().map_err(|e| format!("decoder: {e}"))?;

    let mut samples = 0u64;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut sum_sq = 0.0f64;
    let mut counted = 0u64;
    let mut frame = ffmpeg::frame::Audio::empty();
    for (s, packet) in ictx.packets() {
        if s.index() != index {
            continue;
        }
        if decoder.send_packet(&packet).is_ok() {
            while decoder.receive_frame(&mut frame).is_ok() {
                samples += frame.samples() as u64;
                rate = frame.rate();
                channels = frame.channels();
                if let ffmpeg::format::Sample::F32(ffmpeg::format::sample::Type::Planar) =
                    frame.format()
                {
                    for &v in &frame.plane::<f32>(0)[..frame.samples()] {
                        sum_sq += f64::from(v) * f64::from(v);
                        counted += 1;
                    }
                }
            }
        }
    }
    let _ = decoder.send_eof();
    while decoder.receive_frame(&mut frame).is_ok() {
        samples += frame.samples() as u64;
    }

    if rate == 0 {
        return Err("no audio decoded".to_string());
    }
    let rms = if counted > 0 {
        (sum_sq / counted as f64).sqrt()
    } else {
        0.0
    };
    Ok((samples as f64 / f64::from(rate), channels, rms))
}

#[tokio::test(flavor = "multi_thread")]
async fn synthetic_device_matrix_preserves_sync() {
    let mut results: Vec<CaseResult> = Vec::new();

    let video_cases: Vec<(u32, VideoScenario, bool)> = vec![
        (15, VideoScenario::Steady, true),
        (30, VideoScenario::Steady, true),
        (60, VideoScenario::Steady, true),
        (120, VideoScenario::Steady, true),
        (30, VideoScenario::Jitter, true),
        (60, VideoScenario::Jitter, true),
        (30, VideoScenario::Drops, true),
        (60, VideoScenario::Drops, true),
        (30, VideoScenario::Gap, true),
        (60, VideoScenario::Gap, true),
        (30, VideoScenario::Steady, false),
        (30, VideoScenario::Gap, false),
    ];

    for (fps, scenario, fragmented) in video_cases {
        let name = format!(
            "video/{}fps/{}/{}",
            fps,
            scenario.name(),
            if fragmented { "fragmented" } else { "mp4" }
        );
        let outcome = run_video_case(fps, scenario, fragmented).await;
        eprintln!(
            "{name}: {}",
            match &outcome {
                Ok(d) => format!("ok ({d})"),
                Err(e) => format!("FAIL ({e})"),
            }
        );
        results.push(CaseResult {
            name,
            pass: outcome.is_ok(),
            detail: outcome.unwrap_or_else(|e| e),
        });
    }

    let audio_cases: Vec<(u32, u16)> = vec![
        (8_000, 2),
        (16_000, 2),
        (22_050, 2),
        (44_100, 2),
        (48_000, 2),
        (96_000, 2),
        (48_000, 1),
        (44_100, 1),
        (48_000, 6),
    ];

    for (rate, channels) in audio_cases {
        let name = format!("audio/{rate}hz/{channels}ch");
        let outcome = run_audio_case(rate, channels).await;
        eprintln!(
            "{name}: {}",
            match &outcome {
                Ok(d) => format!("ok ({d})"),
                Err(e) => format!("FAIL ({e})"),
            }
        );
        results.push(CaseResult {
            name,
            pass: outcome.is_ok(),
            detail: outcome.unwrap_or_else(|e| e),
        });
    }

    if let Ok(report_path) = std::env::var("CAP_SYNC_MATRIX_REPORT") {
        let json = serde_json::to_string_pretty(&results).expect("serialize report");
        std::fs::write(&report_path, json).expect("write report");
        eprintln!("report written to {report_path}");
    }

    let failures: Vec<&CaseResult> = results.iter().filter(|r| !r.pass).collect();
    assert!(
        failures.is_empty(),
        "{} of {} matrix cases failed:\n{}",
        failures.len(),
        results.len(),
        failures
            .iter()
            .map(|r| format!("  {} — {}", r.name, r.detail))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
