//! Load-time repair for recordings whose camera track was stamped at the
//! wrong rate.
//!
//! A camera that delivered frames faster than the pipeline's nominal rate
//! (e.g. an AVFoundation device free-running at 60fps while Cap recorded it
//! as 30fps) produced a camera.mp4 whose duration is a clean multiple of the
//! real recording span — the video plays in slow motion and stretches the
//! editor timeline. The recorder-side defects are fixed, but affected
//! recordings on disk stay broken; this module detects the signature on
//! editor open and losslessly rewrites the camera track's timestamps.

use std::path::{Path, PathBuf};

use cap_enc_ffmpeg::remux::{get_media_duration, get_video_fps, rescale_video_timestamps};
use cap_project::{RecordingMeta, RecordingMetaInner, StudioRecordingMeta};
use tracing::{info, warn};

/// Minimum reference span for a heal to be considered; very short recordings
/// have too much probe noise to trust a ratio.
const MIN_REFERENCE_SECS: f64 = 2.0;
/// Camera must exceed the reference span by at least this factor…
const MIN_STRETCH_RATIO: f64 = 1.5;
/// …and by at least this many seconds, so long recordings with slightly
/// off-by-probe durations never trigger.
const MIN_STRETCH_SECS: f64 = 1.0;
/// When only the display track is available as a reference (no audio), the
/// ratio must sit in the rate-doubling family (60/30, 50/25, 60/24) to rule
/// out a display track that legitimately stopped early.
const DISPLAY_ONLY_RATIO_RANGE: (f64, f64) = (1.6, 2.6);
/// References must agree with each other within this fraction of the larger
/// one before the camera is judged against them.
const REFERENCE_AGREEMENT_RATIO: f64 = 0.15;
/// The healed file must land within this fraction of the expected span or the
/// original is kept untouched.
const HEAL_VERIFY_TOLERANCE_RATIO: f64 = 0.05;

const BACKUP_FILE_NAME: &str = "camera.original.mp4";

/// Detects and repairs stretched camera tracks in a studio recording.
/// Returns `true` if any segment was healed. Never modifies media unless the
/// stretch signature is unambiguous, and keeps the original file alongside
/// the healed one.
pub fn heal_stretched_camera(project_path: &Path) -> anyhow::Result<bool> {
    let mut meta = RecordingMeta::load_for_project(project_path)
        .map_err(|e| anyhow::anyhow!("failed to load recording meta: {e}"))?;

    let RecordingMetaInner::Studio(studio_meta) = &mut meta.inner else {
        return Ok(false);
    };

    let StudioRecordingMeta::MultipleSegments { inner } = studio_meta.as_mut() else {
        return Ok(false);
    };

    let project_path = project_path.to_path_buf();
    let mut healed_any = false;

    for (segment_index, segment) in inner.segments.iter_mut().enumerate() {
        let Some(camera) = segment.camera.as_mut() else {
            continue;
        };

        let camera_path = camera.path.to_path(&project_path);
        let display_path = segment.display.path.to_path(&project_path);

        let backup_path = backup_path_for(&camera_path);
        if backup_path.exists() {
            // Healed before — never rescale the same track twice.
            continue;
        }

        let Some(camera_duration) = get_media_duration(&camera_path) else {
            continue;
        };

        // Each reference predicts the camera's span from its own duration
        // plus the difference in track start times (a track that started
        // later ran for less wall time).
        let camera_start = camera.start_time.unwrap_or(0.0);
        let mut references: Vec<(&'static str, f64)> = Vec::new();

        if let Some(display_duration) = get_media_duration(&display_path) {
            let display_start = segment.display.start_time.unwrap_or(0.0);
            references.push((
                "display",
                display_duration.as_secs_f64() + (display_start - camera_start),
            ));
        }

        for (name, audio) in [("mic", segment.mic.as_ref()), ("system-audio", segment.system_audio.as_ref())] {
            let Some(audio) = audio else { continue };
            let audio_path = audio.path.to_path(&project_path);
            if let Some(audio_duration) = get_media_duration(&audio_path) {
                let audio_start = audio.start_time.unwrap_or(0.0);
                references.push((
                    name,
                    audio_duration.as_secs_f64() + (audio_start - camera_start),
                ));
            }
        }

        let Some(decision) = evaluate_stretch(camera_duration.as_secs_f64(), &references) else {
            continue;
        };

        info!(
            segment_index,
            camera_secs = camera_duration.as_secs_f64(),
            expected_secs = decision.expected_span_secs,
            ratio = decision.ratio,
            references = ?references,
            "Detected stretched camera track, rewriting timestamps"
        );

        let healed_path = camera_path.with_extension("healed.mp4");
        let _ = std::fs::remove_file(&healed_path);

        if let Err(e) = rescale_video_timestamps(&camera_path, &healed_path, decision.scale) {
            warn!(segment_index, "Camera heal remux failed, keeping original: {e}");
            let _ = std::fs::remove_file(&healed_path);
            continue;
        }

        let healed_ok = get_media_duration(&healed_path).is_some_and(|healed| {
            let error = (healed.as_secs_f64() - decision.expected_span_secs).abs();
            error <= decision.expected_span_secs * HEAL_VERIFY_TOLERANCE_RATIO
        });

        if !healed_ok {
            warn!(
                segment_index,
                "Healed camera track failed duration verification, keeping original"
            );
            let _ = std::fs::remove_file(&healed_path);
            continue;
        }

        if let Err(e) = std::fs::rename(&camera_path, &backup_path) {
            warn!(segment_index, "Failed to back up original camera track: {e}");
            let _ = std::fs::remove_file(&healed_path);
            continue;
        }

        if let Err(e) = std::fs::rename(&healed_path, &camera_path) {
            warn!(segment_index, "Failed to install healed camera track: {e}");
            // Put the original back so the recording stays playable.
            let _ = std::fs::rename(&backup_path, &camera_path);
            let _ = std::fs::remove_file(&healed_path);
            continue;
        }

        if let Some(fps) = get_video_fps(&camera_path).filter(|fps| *fps > 0) {
            camera.fps = fps;
        }

        info!(
            segment_index,
            healed_secs = get_media_duration(&camera_path).map(|d| d.as_secs_f64()),
            fps = camera.fps,
            "Camera track healed (original kept as {BACKUP_FILE_NAME})"
        );
        healed_any = true;
    }

    if healed_any {
        meta.save_for_project()
            .map_err(|e| anyhow::anyhow!("failed to save healed recording meta: {e:?}"))?;
    }

    Ok(healed_any)
}

fn backup_path_for(camera_path: &Path) -> PathBuf {
    camera_path.with_file_name(BACKUP_FILE_NAME)
}

#[derive(Debug, PartialEq)]
struct StretchDecision {
    expected_span_secs: f64,
    ratio: f64,
    scale: f64,
}

/// Decides whether `camera_secs` is a stretched track given the expected
/// spans predicted by the other tracks. Returns `None` unless the signature
/// is unambiguous.
fn evaluate_stretch(camera_secs: f64, references: &[(&str, f64)]) -> Option<StretchDecision> {
    let spans: Vec<f64> = references.iter().map(|(_, span)| *span).collect();
    if spans.is_empty() {
        return None;
    }

    let max_span = spans.iter().cloned().fold(f64::MIN, f64::max);
    let min_span = spans.iter().cloned().fold(f64::MAX, f64::min);

    if max_span < MIN_REFERENCE_SECS {
        return None;
    }

    // All references must tell the same story; a mic that ran much longer
    // than the display means the display died early, not that the camera is
    // stretched.
    if max_span - min_span > (max_span * REFERENCE_AGREEMENT_RATIO).max(MIN_STRETCH_SECS) {
        return None;
    }

    let expected_span_secs = max_span;
    let ratio = camera_secs / expected_span_secs;

    if ratio < MIN_STRETCH_RATIO || camera_secs - expected_span_secs < MIN_STRETCH_SECS {
        return None;
    }

    let has_audio_reference = references.iter().any(|(name, _)| *name != "display");
    if !has_audio_reference
        && !(DISPLAY_ONLY_RATIO_RANGE.0..=DISPLAY_ONLY_RATIO_RANGE.1).contains(&ratio)
    {
        return None;
    }

    Some(StretchDecision {
        expected_span_secs,
        ratio,
        scale: expected_span_secs / camera_secs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER_REPORT_CAMERA_SECS: f64 = 13.666;

    fn user_report_references() -> Vec<(&'static str, f64)> {
        // From the original report: display 6.75s starting at 0.302s, mic
        // 6.91s starting at 0.102s, camera starting at 0.149s.
        vec![
            ("display", 6.75 + (0.302 - 0.149)),
            ("mic", 6.91 + (0.102 - 0.149)),
        ]
    }

    #[test]
    fn detects_the_reported_double_length_camera() {
        let decision = evaluate_stretch(USER_REPORT_CAMERA_SECS, &user_report_references())
            .expect("the reported recording must be detected");
        assert!((decision.ratio - 2.0).abs() < 0.05, "ratio {}", decision.ratio);
        assert!((decision.scale - 0.5).abs() < 0.02, "scale {}", decision.scale);
    }

    #[test]
    fn healthy_recording_is_left_alone() {
        assert_eq!(
            evaluate_stretch(6.9, &[("display", 6.75), ("mic", 6.91)]),
            None
        );
    }

    #[test]
    fn camera_slightly_longer_than_references_is_left_alone() {
        // Cameras legitimately run a little past the other tracks.
        assert_eq!(
            evaluate_stretch(7.4, &[("display", 6.75), ("mic", 6.91)]),
            None
        );
    }

    #[test]
    fn display_dying_early_does_not_trigger_heal() {
        // Display stopped at half-time but the mic kept going: references
        // disagree, so the long camera is trusted.
        assert_eq!(
            evaluate_stretch(60.0, &[("display", 30.0), ("mic", 59.5)]),
            None
        );
    }

    #[test]
    fn display_only_requires_doubling_ratio() {
        // Without audio, a 2x camera is healed…
        assert!(evaluate_stretch(13.6, &[("display", 6.8)]).is_some());
        // …but a 3x mismatch (not a rate-doubling signature) is not.
        assert_eq!(evaluate_stretch(20.4, &[("display", 6.8)]), None);
        // …nor is a display that died at two-thirds.
        assert_eq!(evaluate_stretch(90.0, &[("display", 60.0)]), None);
    }

    #[test]
    fn short_recordings_are_not_healed() {
        assert_eq!(
            evaluate_stretch(3.0, &[("display", 1.5), ("mic", 1.5)]),
            None
        );
    }

    #[test]
    fn below_ratio_threshold_is_left_alone() {
        assert_eq!(
            evaluate_stretch(3.1, &[("display", 2.1), ("mic", 2.1)]),
            None
        );
    }

    /// Manual harness for healing a real .cap bundle:
    /// `CAP_HEAL_PROJECT_PATH=/path/to/recording.cap cargo test -p cap-recording \
    ///   --lib camera_heal::tests::heal_project_from_env -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn heal_project_from_env() {
        let path =
            std::env::var("CAP_HEAL_PROJECT_PATH").expect("CAP_HEAL_PROJECT_PATH must be set");
        let healed = heal_stretched_camera(Path::new(&path)).unwrap();
        println!("healed: {healed}");
    }
}
