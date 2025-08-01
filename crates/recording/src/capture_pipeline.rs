use std::{
    future::Future,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::SystemTime,
};

use cap_media::{
    data::AudioInfo,
    encoders::{AACEncoder, AudioEncoder, H264Encoder, MP4File},
    feeds::AudioInputFeed,
    pipeline::{builder::PipelineBuilder, task::PipelineSinkTask, RealTimeClock},
    sources::{
        AVFrameCapture, AudioInputSource, AudioMixer, CMSampleBufferCapture, ScreenCaptureFormat,
        ScreenCaptureSource, ScreenCaptureTarget,
    },
    MediaError,
};
use ffmpeg::ffi::AV_TIME_BASE_Q;
use flume::{Receiver, Sender};
use tokio::sync::oneshot;
use tracing::error;

use crate::RecordingError;

pub type CapturePipelineBuilder = PipelineBuilder<RealTimeClock<()>>;

pub trait MakeCapturePipeline: ScreenCaptureFormat + std::fmt::Debug + 'static {
    fn make_studio_mode_pipeline(
        builder: CapturePipelineBuilder,
        source: (
            ScreenCaptureSource<Self>,
            flume::Receiver<(Self::VideoFormat, f64)>,
        ),
        output_path: PathBuf,
    ) -> Result<(CapturePipelineBuilder, flume::Receiver<f64>), MediaError>
    where
        Self: Sized;

    fn make_instant_mode_pipeline(
        builder: CapturePipelineBuilder,
        source: (
            ScreenCaptureSource<Self>,
            flume::Receiver<(Self::VideoFormat, f64)>,
        ),
        audio: Option<&AudioInputFeed>,
        system_audio: Option<(Receiver<(ffmpeg::frame::Audio, f64)>, AudioInfo)>,
        output_path: PathBuf,
        pause_flag: Arc<AtomicBool>,
    ) -> impl Future<Output = Result<CapturePipelineBuilder, MediaError>> + Send
    where
        Self: Sized;
}

#[cfg(target_os = "macos")]
impl MakeCapturePipeline for cap_media::sources::CMSampleBufferCapture {
    fn make_studio_mode_pipeline(
        mut builder: CapturePipelineBuilder,
        source: (
            ScreenCaptureSource<Self>,
            flume::Receiver<(Self::VideoFormat, f64)>,
        ),
        output_path: PathBuf,
    ) -> Result<(CapturePipelineBuilder, flume::Receiver<f64>), MediaError> {
        let screen_config = source.0.info();
        let mut screen_encoder = cap_media::encoders::MP4AVAssetWriterEncoder::init(
            "screen",
            screen_config,
            None,
            output_path.into(),
            None,
        )?;

        let (timestamp_tx, timestamp_rx) = flume::bounded(1);

        builder.spawn_task("screen_capture_encoder", move |ready| {
            use std::time::Duration;

            let mut timestamp_tx = Some(timestamp_tx);
            let _ = ready.send(Ok(()));

            let Ok(frame) = source.1.recv() else {
                return Ok(());
            };

            if let Some(timestamp_tx) = timestamp_tx.take() {
                let _ = timestamp_tx.send(frame.1);
            }

            let result = loop {
                use flume::RecvTimeoutError;

                match source.1.recv() {
                    Ok(frame) => {
                        let _ = screen_encoder.queue_video_frame(frame.0.as_ref());
                    }
                    // Err(RecvTimeoutError::Timeout) => {
                    //     break Err("Frame receive timeout".to_string());
                    // }
                    Err(_) => {
                        break Ok(());
                    }
                }
            };

            screen_encoder.finish();

            result
        });

        builder.spawn_source("screen_capture", source.0);

        Ok((builder, timestamp_rx))
    }

    async fn make_instant_mode_pipeline(
        mut builder: CapturePipelineBuilder,
        source: (
            ScreenCaptureSource<Self>,
            flume::Receiver<(Self::VideoFormat, f64)>,
        ),
        audio: Option<&AudioInputFeed>,
        system_audio: Option<(Receiver<(ffmpeg::frame::Audio, f64)>, AudioInfo)>,
        output_path: PathBuf,
        pause_flag: Arc<AtomicBool>,
    ) -> Result<CapturePipelineBuilder, MediaError> {
        let (audio_tx, audio_rx) = flume::bounded(64);
        let mut audio_mixer = AudioMixer::new(audio_tx);

        if let Some(system_audio) = system_audio {
            audio_mixer.add_source(system_audio.1, system_audio.0);
        }

        if let Some(audio) = audio {
            let sink = audio_mixer.sink(audio.audio_info());
            let source = AudioInputSource::init(audio, sink.tx, SystemTime::now());

            builder.spawn_source("microphone_capture", source);
        }

        let has_audio_sources = audio_mixer.has_sources();

        let mp4 = Arc::new(std::sync::Mutex::new(
            cap_media::encoders::MP4AVAssetWriterEncoder::init(
                "mp4",
                source.0.info(),
                has_audio_sources.then_some(AudioMixer::info()),
                output_path.into(),
                Some(1080),
            )?,
        ));

        use cidre::cm;

        let (first_frame_tx, mut first_frame_rx) = oneshot::channel::<(cm::Time, f64)>();

        if has_audio_sources {
            builder.spawn_source("audio_mixer", audio_mixer);

            let mp4 = mp4.clone();
            builder.spawn_task("audio_encoding", move |ready| {
                let _ = ready.send(Ok(()));
                let mut time = None;

                while let Ok(mut frame) = audio_rx.recv() {
                    let pts = frame.pts().unwrap();

                    if let Ok(first_time) = first_frame_rx.try_recv() {
                        time = Some(first_time);
                    };

                    let Some(time) = time else {
                        continue;
                    };

                    let elapsed = (pts as f64 / AV_TIME_BASE_Q.den as f64) - time.1;

                    let time = time.0.add(cm::Time::new(
                        (elapsed * time.0.scale as f64 + time.1 * time.0.scale as f64) as i64,
                        time.0.scale,
                    ));

                    frame.set_pts(Some(time.value / (time.scale / AV_TIME_BASE_Q.den) as i64));

                    if let Ok(mut mp4) = mp4.lock() {
                        if let Err(e) = mp4.queue_audio_frame(frame) {
                            error!("{e}");
                            return Ok(());
                        }
                    }
                }

                Ok(())
            });
        }

        let mut first_frame_tx = Some(first_frame_tx);
        builder.spawn_task("screen_capture_encoder", move |ready| {
            let _ = ready.send(Ok(()));
            while let Ok((frame, unix_time)) = source.1.recv() {
                if let Ok(mut mp4) = mp4.lock() {
                    if pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
                        mp4.pause();
                    } else {
                        mp4.resume();
                    }

                    if let Some(first_frame_tx) = first_frame_tx.take() {
                        let _ = first_frame_tx.send((frame.pts(), unix_time));
                    }

                    mp4.queue_video_frame(frame.as_ref());
                }
            }
            if let Ok(mut mp4) = mp4.lock() {
                mp4.finish();
            }

            Ok(())
        });

        builder.spawn_source("screen_capture", source.0);

        Ok(builder)
    }
}

impl MakeCapturePipeline for AVFrameCapture {
    fn make_studio_mode_pipeline(
        mut builder: CapturePipelineBuilder,
        source: (
            ScreenCaptureSource<Self>,
            flume::Receiver<(Self::VideoFormat, f64)>,
        ),
        output_path: PathBuf,
    ) -> Result<(CapturePipelineBuilder, flume::Receiver<f64>), MediaError>
    where
        Self: Sized,
    {
        let screen_config = source.0.info();
        let mut screen_encoder = MP4File::init(
            "screen",
            output_path.into(),
            |o| H264Encoder::builder("screen", screen_config).build(o),
            |_| None,
        )?;

        builder.spawn_source("screen_capture", source.0);

        let (timestamp_tx, timestamp_rx) = flume::bounded(1);

        builder.spawn_task("screen_capture_encoder", move |ready| {
            let mut timestamp_tx = Some(timestamp_tx);
            let _ = ready.send(Ok(()));

            while let Ok(frame) = source.1.recv() {
                if let Some(timestamp_tx) = timestamp_tx.take() {
                    timestamp_tx.send(frame.1).unwrap();
                }
                screen_encoder.queue_video_frame(frame.0);
            }
            screen_encoder.finish();
            Ok(())
        });

        Ok((builder, timestamp_rx))
    }

    async fn make_instant_mode_pipeline(
        mut builder: CapturePipelineBuilder,
        source: (
            ScreenCaptureSource<Self>,
            flume::Receiver<(Self::VideoFormat, f64)>,
        ),
        audio: Option<&AudioInputFeed>,
        system_audio: Option<(Receiver<(ffmpeg::frame::Audio, f64)>, AudioInfo)>,
        output_path: PathBuf,
        _pause_flag: Arc<AtomicBool>,
    ) -> Result<CapturePipelineBuilder, MediaError>
    where
        Self: Sized,
    {
        let (audio_tx, audio_rx) = flume::bounded(64);
        let mut audio_mixer = AudioMixer::new(audio_tx);

        if let Some(system_audio) = system_audio {
            audio_mixer.add_source(system_audio.1, system_audio.0);
        }

        if let Some(audio) = audio {
            let sink = audio_mixer.sink(audio.audio_info());
            let source = AudioInputSource::init(audio, sink.tx, SystemTime::now());

            builder.spawn_source("microphone_capture", source);
        }

        let has_audio_sources = audio_mixer.has_sources();

        let screen_config = source.0.info();
        let mp4 = Arc::new(std::sync::Mutex::new(MP4File::init(
            "screen",
            output_path.into(),
            |o| H264Encoder::builder("screen", screen_config).build(o),
            |o| {
                has_audio_sources.then(|| {
                    AACEncoder::init("mic_audio", AudioMixer::info(), o).map(|v| v.boxed())
                })
            },
        )?));

        if has_audio_sources {
            builder.spawn_source("audio_mixer", audio_mixer);

            let mp4 = mp4.clone();
            builder.spawn_task("audio_encoding", move |ready| {
                let _ = ready.send(Ok(()));
                while let Ok(frame) = audio_rx.recv() {
                    if let Ok(mut mp4) = mp4.lock() {
                        mp4.queue_audio_frame(frame);
                    }
                }
                Ok(())
            });
        }

        builder.spawn_source("screen_capture", source.0);

        builder.spawn_task("screen_encoder", move |ready| {
            let _ = ready.send(Ok(()));
            while let Ok((frame, unix_time)) = source.1.recv() {
                if let Ok(mut mp4) = mp4.lock() {
                    // if pause_flag.load(std::sync::atomic::Ordering::Relaxed) {
                    //     mp4.pause();
                    // } else {
                    //     mp4.resume();
                    // }

                    mp4.queue_video_frame(frame);
                }
            }
            if let Ok(mut mp4) = mp4.lock() {
                mp4.finish();
            }
            Ok(())
        });

        Ok(builder)
    }
}

type ScreenCaptureReturn<T> = (
    ScreenCaptureSource<T>,
    Receiver<(<T as ScreenCaptureFormat>::VideoFormat, f64)>,
);

#[cfg(target_os = "macos")]
pub type ScreenCaptureMethod = CMSampleBufferCapture;

#[cfg(not(target_os = "macos"))]
pub type ScreenCaptureMethod = AVFrameCapture;

pub async fn create_screen_capture(
    capture_target: &ScreenCaptureTarget,
    show_camera: bool,
    force_show_cursor: bool,
    max_fps: u32,
    audio_tx: Option<Sender<(ffmpeg::frame::Audio, f64)>>,
    start_time: SystemTime,
) -> Result<ScreenCaptureReturn<ScreenCaptureMethod>, RecordingError> {
    let (video_tx, video_rx) = flume::bounded(16);

    ScreenCaptureSource::<ScreenCaptureMethod>::init(
        capture_target,
        None,
        show_camera,
        force_show_cursor,
        max_fps,
        video_tx,
        audio_tx,
        start_time,
    )
    .await
    .map(|v| (v, video_rx))
    .map_err(|e| RecordingError::Media(MediaError::TaskLaunch(e)))
}
