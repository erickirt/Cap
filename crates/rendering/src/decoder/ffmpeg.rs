use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{mpsc, Arc},
};

use ffmpeg::{codec, format, frame, software, Codec};
use ffmpeg_sys_next::{avcodec_find_decoder, AVHWDeviceType};
use log::debug;
use tokio::sync::oneshot;
use tracing::error;

use super::{pts_to_frame, DecodedFrame, VideoDecoderMessage, FRAME_CACHE_SIZE};

#[derive(Clone)]
struct CachedFrame {
    data: CachedFrameData,
}

impl CachedFrame {
    fn process(&mut self, width: u32, height: u32) -> Arc<Vec<u8>> {
        match &mut self.data {
            CachedFrameData::Raw(frame) => {
                let rgb_frame = if frame.format() != format::Pixel::RGBA {
                    // Reinitialize the scaler with the new input format
                    let mut scaler =
                        software::converter((width, height), frame.format(), format::Pixel::RGBA)
                            .unwrap();

                    let mut rgb_frame = frame::Video::empty();
                    scaler.run(&frame, &mut rgb_frame).unwrap();
                    rgb_frame
                } else {
                    std::mem::replace(frame, frame::Video::empty())
                };

                let width = rgb_frame.width() as usize;
                let height = rgb_frame.height() as usize;
                let stride = rgb_frame.stride(0);
                let data = rgb_frame.data(0);

                let expected_size = width * height * 4;

                let mut frame_buffer = Vec::with_capacity(expected_size);

                // account for stride > width
                for line_data in data.chunks_exact(stride) {
                    frame_buffer.extend_from_slice(&line_data[0..width * 4]);
                }

                let data = Arc::new(frame_buffer);

                self.data = CachedFrameData::Processed(data.clone());

                data
            }
            CachedFrameData::Processed(data) => data.clone(),
        }
    }
}

#[derive(Clone)]
enum CachedFrameData {
    Raw(frame::Video),
    Processed(Arc<Vec<u8>>),
}

pub struct FfmpegDecoder;

impl FfmpegDecoder {
    pub fn spawn(
        name: &'static str,
        path: PathBuf,
        fps: u32,
        rx: mpsc::Receiver<VideoDecoderMessage>,
        ready_tx: oneshot::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let mut this = cap_video_decode::FFmpegDecoder::new(
            path,
            Some(if cfg!(target_os = "macos") {
                AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX
            } else {
                AVHWDeviceType::AV_HWDEVICE_TYPE_D3D12VA
            }),
        )?;

        let time_base = this.decoder().time_base();
        let start_time = this.start_time();
        let width = this.decoder().width();
        let height = this.decoder().height();

        std::thread::spawn(move || {
            let mut cache = BTreeMap::<u32, CachedFrame>::new();
            // active frame is a frame that triggered decode.
            // frames that are within render_more_margin of this frame won't trigger decode.
            let mut last_active_frame = None::<u32>;

            let mut last_decoded_frame = None::<u32>;
            let mut last_sent_frame = None::<(u32, DecodedFrame)>;

            let mut peekable_requests = PeekableReceiver { rx, peeked: None };

            let mut frames = this.frames().peekable();

            let _ = ready_tx.send(Ok(()));

            while let Ok(r) = peekable_requests.recv() {
                match r {
                    VideoDecoderMessage::GetFrame(requested_time, sender) => {
                        let requested_frame = (requested_time * fps as f32).floor() as u32;
                        // sender.send(black_frame.clone()).ok();
                        // continue;

                        let mut sender = if let Some(cached) = cache.get_mut(&requested_frame) {
                            let data = cached.process(width, height);

                            sender.send(data.clone()).ok();
                            last_sent_frame = Some((requested_frame, data));
                            continue;
                        } else {
                            Some(sender)
                        };

                        let cache_min = requested_frame.saturating_sub(FRAME_CACHE_SIZE as u32 / 2);
                        let cache_max = requested_frame + FRAME_CACHE_SIZE as u32 / 2;

                        if requested_frame == 0
                            || last_sent_frame
                                .as_ref()
                                .map(|last| {
                                    requested_frame < last.0 ||
                                // seek forward for big jumps. this threshold is arbitrary but should be derived from i-frames in future
                                requested_frame - last.0 > FRAME_CACHE_SIZE as u32
                                })
                                .unwrap_or(true)
                        {
                            debug!("seeking to {}", requested_frame);

                            this.reset(requested_time);
                            frames = this.frames().peekable();

                            last_decoded_frame = None;
                        }

                        last_active_frame = Some(requested_frame);

                        let mut exit = false;

                        loop {
                            if peekable_requests.peek().is_some() {
                                break;
                            }

                            let Some(frame) = frames.next() else {
                                break;
                            };

                            {
                                let frame = match frame {
                                    Ok(f) => f,
                                    Err(e) => {
                                        error!("Error decoding frame: {}", e);
                                        break;
                                    }
                                };

                                let current_frame =
                                    pts_to_frame(frame.pts().unwrap() - start_time, time_base, fps);

                                // Handles frame skips. requested_frame == last_decoded_frame should be handled by the frame cache.
                                if let Some((last_decoded_frame, sender)) = last_decoded_frame
                                    .filter(|last_decoded_frame| {
                                        requested_frame > *last_decoded_frame
                                            && requested_frame < current_frame
                                    })
                                    .and_then(|l| Some((l, sender.take()?)))
                                {
                                    let Some(data) = cache
                                        .get_mut(&last_decoded_frame)
                                        .map(|f| f.process(width, height))
                                    else {
                                        break;
                                    };

                                    last_sent_frame = Some((last_decoded_frame, data.clone()));
                                    sender.send(data).ok();
                                }

                                last_decoded_frame = Some(current_frame);

                                let exceeds_cache_bounds = current_frame > cache_max;
                                let too_small_for_cache_bounds = current_frame < cache_min;

                                if !too_small_for_cache_bounds {
                                    let mut cache_frame = CachedFrame {
                                        data: CachedFrameData::Raw(frame),
                                    };

                                    if current_frame == requested_frame {
                                        if let Some(sender) = sender.take() {
                                            let data = cache_frame.process(width, height);
                                            last_sent_frame = Some((current_frame, data.clone()));
                                            sender.send(data).ok();

                                            break;
                                        }
                                    }

                                    if cache.len() >= FRAME_CACHE_SIZE {
                                        if let Some(last_active_frame) = &last_active_frame {
                                            let frame = if requested_frame > *last_active_frame {
                                                *cache.keys().next().unwrap()
                                            } else if requested_frame < *last_active_frame {
                                                *cache.keys().next_back().unwrap()
                                            } else {
                                                let min = *cache.keys().min().unwrap();
                                                let max = *cache.keys().max().unwrap();

                                                if current_frame > max {
                                                    min
                                                } else {
                                                    max
                                                }
                                            };

                                            cache.remove(&frame);
                                        } else {
                                            cache.clear()
                                        }
                                    }

                                    cache.insert(current_frame, cache_frame);
                                }

                                exit = exit || exceeds_cache_bounds;
                            }
                        }

                        // handles the case where the cache doesn't contain a frame so we fallback to the previously sent one
                        if let Some(last_sent_frame) = &last_sent_frame {
                            if last_sent_frame.0 < requested_frame {
                                sender.take().map(|s| s.send(last_sent_frame.1.clone()));
                            }
                        }

                        if exit {
                            continue;
                        }

                        if let Some((sender, last_sent_frame)) =
                            sender.take().zip(last_sent_frame.clone())
                        {
                            sender.send(last_sent_frame.1).ok();
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

pub fn find_decoder(
    s: &format::context::Input,
    st: &format::stream::Stream,
    codec_id: codec::Id,
) -> Option<Codec> {
    unsafe {
        use ffmpeg::media::Type;
        let codec = match st.parameters().medium() {
            Type::Video => Some((*s.as_ptr()).video_codec),
            Type::Audio => Some((*s.as_ptr()).audio_codec),
            Type::Subtitle => Some((*s.as_ptr()).subtitle_codec),
            _ => None,
        };

        if let Some(codec) = codec {
            if !codec.is_null() {
                return Some(Codec::wrap(codec));
            }
        }

        let found = avcodec_find_decoder(codec_id.into());

        if found.is_null() {
            return None;
        }
        Some(Codec::wrap(found))
    }
}

struct PeekableReceiver<T> {
    rx: mpsc::Receiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_some() {
            self.peeked.as_ref()
        } else {
            match self.rx.try_recv() {
                Ok(value) => {
                    self.peeked = Some(value);
                    self.peeked.as_ref()
                }
                Err(_) => None,
            }
        }
    }

    fn recv(&mut self) -> Result<T, mpsc::RecvError> {
        if let Some(value) = self.peeked.take() {
            Ok(value)
        } else {
            self.rx.recv()
        }
    }
}
