//! `GStreamer` H.264 encoding pipeline.
//!
//! Pipeline: `appsrc ! videoconvert ! encoder ! h264parse ! appsink`
//!
//! Supports hardware-accelerated encoding via VAAPI (Intel/AMD) and
//! NVENC (NVIDIA), with automatic fallback to x264 software encoding.

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

use crate::{EncodeError, EncodedFrame, EncoderConfig};

/// Hardware encoder backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderType {
    /// Intel/AMD VAAPI hardware encoder (`vaapih264enc`).
    Vaapi,
    /// NVIDIA NVENC hardware encoder (`nvh264enc`).
    Nvenc,
    /// x264 software encoder (`x264enc`).
    Software,
}

impl EncoderType {
    /// `GStreamer` element factory name for this encoder.
    #[must_use]
    pub fn element_name(self) -> &'static str {
        match self {
            Self::Vaapi => "vaapih264enc",
            Self::Nvenc => "nvh264enc",
            Self::Software => "x264enc",
        }
    }
}

impl std::fmt::Display for EncoderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vaapi => write!(f, "VAAPI"),
            Self::Nvenc => write!(f, "NVENC"),
            Self::Software => write!(f, "x264 (software)"),
        }
    }
}

/// Check if a `GStreamer` element factory is available.
#[must_use]
pub fn is_encoder_available(element_name: &str) -> bool {
    gst::ElementFactory::find(element_name).is_some()
}

/// Detect the best available H.264 encoder.
///
/// Checks in priority order: VAAPI (hardware) -> NVENC (hardware) -> x264 (software).
///
/// # Panics
///
/// Does not panic. Returns `Software` as the fallback which is always available
/// when `GStreamer` and the x264 plugin are installed.
#[must_use]
pub fn detect_best_encoder() -> EncoderType {
    if is_encoder_available(EncoderType::Vaapi.element_name()) {
        EncoderType::Vaapi
    } else if is_encoder_available(EncoderType::Nvenc.element_name()) {
        EncoderType::Nvenc
    } else {
        EncoderType::Software
    }
}

/// H.264 encoder using a `GStreamer` pipeline.
///
/// Creates and manages the pipeline:
/// `appsrc ! videoconvert ! encoder ! h264parse ! appsink`
///
/// Push raw BGRx/BGRA frames via [`encode_frame`](GstEncoder::encode_frame)
/// and receive H.264 NAL units in byte-stream format.
pub struct GstEncoder {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
    encoder_type: EncoderType,
    running: bool,
    /// Log negotiated caps once after first successful buffer push.
    caps_logged: bool,
}

impl GstEncoder {
    /// Create a new H.264 encoder with the given configuration.
    ///
    /// Initializes `GStreamer` (if not already done), detects the best
    /// hardware encoder, and builds the encoding pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if `GStreamer` initialization fails or
    /// required elements cannot be created.
    pub fn new(config: &EncoderConfig) -> Result<Self, EncodeError> {
        gst::init().map_err(|e| EncodeError::GstInit(e.to_string()))?;

        let encoder_type = config.encoder_type.unwrap_or_else(detect_best_encoder);
        tracing::info!(%encoder_type, "Selected H.264 encoder");

        let (pipeline, appsrc, appsink) = build_pipeline(config, encoder_type)?;

        Ok(Self {
            pipeline,
            appsrc,
            appsink,
            encoder_type,
            running: false,
            caps_logged: false,
        })
    }

    /// The encoder type in use.
    #[must_use]
    pub fn encoder_type(&self) -> EncoderType {
        self.encoder_type
    }

    /// Start the encoding pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError::StateChange`] if the pipeline cannot transition to Playing.
    pub fn start(&mut self) -> Result<(), EncodeError> {
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| EncodeError::StateChange(e.to_string()))?;
        self.running = true;
        self.caps_logged = false;

        tracing::info!("Encoder pipeline started (caps logged after first frame)");
        Ok(())
    }

    /// Stop the encoding pipeline.
    pub fn stop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
        self.running = false;
        tracing::info!("Encoder pipeline stopped");
    }

    /// Whether the pipeline is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Encode a raw BGRA frame.
    ///
    /// Pushes the frame into the `GStreamer` pipeline and attempts to
    /// pull an encoded H.264 frame. The pipeline starts automatically
    /// on the first call.
    ///
    /// Returns `Ok(None)` if no encoded frame is available yet (the
    /// encoder may buffer a few frames before producing output).
    ///
    /// # Errors
    ///
    /// Returns [`EncodeError`] if pushing the frame or pulling the
    /// result fails.
    pub fn encode_frame(&mut self, frame_data: &[u8]) -> Result<Option<EncodedFrame>, EncodeError> {
        if !self.running {
            self.start()?;
        }

        // Create a GStreamer buffer from the frame data
        let mut buffer = gst::Buffer::with_size(frame_data.len())
            .map_err(|e| EncodeError::PushBuffer(e.to_string()))?;

        {
            let buffer_ref = buffer.get_mut().ok_or(EncodeError::BufferMap)?;
            let mut map = buffer_ref.map_writable().map_err(|_| EncodeError::BufferMap)?;
            map.copy_from_slice(frame_data);
        }

        // Push into the pipeline
        self.appsrc
            .push_buffer(buffer)
            .map_err(|e| EncodeError::PushBuffer(e.to_string()))?;

        // Log negotiated caps once after first buffer push, when GStreamer
        // has actually performed caps negotiation (caps are None at start()).
        if !self.caps_logged {
            self.caps_logged = true;
            if let Some(convert) = self.pipeline.by_name("convert") {
                if let Some(caps) = convert.static_pad("sink").and_then(|p| p.current_caps()) {
                    tracing::info!(%caps, "videoconvert input caps (negotiated)");
                }
                if let Some(caps) = convert.static_pad("src").and_then(|p| p.current_caps()) {
                    tracing::info!(%caps, "videoconvert output caps (negotiated)");
                }
            }
            if let Some(enc) = self.pipeline.by_name("encoder") {
                if let Some(caps) = enc.static_pad("sink").and_then(|p| p.current_caps()) {
                    tracing::info!(%caps, "encoder input caps (negotiated)");
                }
            }
        }

        // Try to pull an encoded frame
        self.pull_encoded_frame()
    }

    /// Force the encoder to produce an IDR keyframe on the next output.
    pub fn force_keyframe(&self) {
        let event = gst_video::UpstreamForceKeyUnitEvent::builder()
            .all_headers(true)
            .build();
        self.appsrc.send_event(event);
        tracing::debug!("Forced keyframe requested");
    }

    /// Adjust the target bitrate at runtime (in bits per second).
    pub fn set_bitrate(&self, bitrate: u32) {
        if let Some(encoder) = self.pipeline.by_name("encoder") {
            encoder.set_property("bitrate", bitrate / 1000);
            tracing::debug!(bitrate, "Encoder bitrate updated");
        }
    }

    /// Try to pull an encoded frame from the appsink.
    fn pull_encoded_frame(&self) -> Result<Option<EncodedFrame>, EncodeError> {
        // Non-blocking pull with 1ms timeout
        let Some(sample) = self
            .appsink
            .try_pull_sample(gst::ClockTime::from_mseconds(1))
        else {
            return Ok(None);
        };

        let buffer = sample
            .buffer()
            .ok_or_else(|| EncodeError::PushBuffer("sample has no buffer".into()))?;

        let map = buffer.map_readable().map_err(|_| EncodeError::BufferMap)?;

        let pts = buffer
            .pts()
            .map_or(0, gst::ClockTime::useconds);

        let duration = buffer
            .duration()
            .map_or(0, gst::ClockTime::useconds);

        // DELTA_UNIT flag means it's NOT a keyframe
        let is_keyframe = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);

        Ok(Some(EncodedFrame {
            data: map.to_vec(),
            pts,
            duration,
            is_keyframe,
        }))
    }
}

impl Drop for GstEncoder {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Build the `GStreamer` encoding pipeline.
///
/// `appsrc ! videoconvert ! encoder ! h264parse ! appsink`
fn build_pipeline(
    config: &EncoderConfig,
    encoder_type: EncoderType,
) -> Result<(gst::Pipeline, gst_app::AppSrc, gst_app::AppSink), EncodeError> {
    #[allow(clippy::cast_possible_wrap)]
    let width = config.width as i32;
    #[allow(clippy::cast_possible_wrap)]
    let height = config.height as i32;
    #[allow(clippy::cast_possible_wrap)]
    let framerate = config.framerate as i32;

    let pipeline = gst::Pipeline::new();

    // AppSrc: raw video input from PipeWire.
    //
    // PipeWire delivers BGRx data (memory: [B, G, R, x]). No explicit
    // colorimetry is set — GStreamer infers BT.709 for HD resolutions
    // (≥720p) and BT.601 for SD, which matches standard conventions.
    //
    // Letting GStreamer auto-negotiate avoids mismatches between the
    // `sRGB` identity matrix (which means "no YUV matrix") and
    // videoconvert's output-only mode, which was causing a blueish
    // color shift in the previous explicit-colorimetry pipeline.
    let appsrc = gst_app::AppSrc::builder()
        .name("source")
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "BGRx")
                .field("width", width)
                .field("height", height)
                .field("framerate", gst::Fraction::new(framerate, 1))
                .build(),
        )
        .format(gst::Format::Time)
        .is_live(true)
        .do_timestamp(true)
        .build();

    // videoconvert: color space conversion (BGRx → I420 YUV for H.264).
    // Default matrix mode lets GStreamer auto-select BT.709/BT.601 based
    // on resolution, matching what most RDP clients expect.
    let videoconvert = make_element("videoconvert", "convert")?;

    // Capsfilter: constrain output to I420 for H.264 encoding.
    // No explicit colorimetry — GStreamer propagates the auto-negotiated
    // colorimetry from videoconvert.
    let capsfilter = make_element("capsfilter", "colorfix")?;
    capsfilter.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .build(),
    );

    // H.264 encoder: hardware or software
    let encoder = make_element(encoder_type.element_name(), "encoder")?;
    configure_encoder(&encoder, encoder_type, config);

    // h264parse: proper NAL unit framing
    let h264parse = make_element("h264parse", "parser")?;

    // AppSink: encoded H.264 output
    let appsink = gst_app::AppSink::builder()
        .name("sink")
        .caps(
            &gst::Caps::builder("video/x-h264")
                .field("stream-format", "byte-stream")
                .field("alignment", "au")
                .build(),
        )
        .build();

    // Pipeline: appsrc(BGRx) ! videoconvert ! capsfilter(I420) ! encoder ! h264parse ! appsink
    pipeline
        .add_many([
            appsrc.upcast_ref(),
            &videoconvert,
            &capsfilter,
            &encoder,
            &h264parse,
            appsink.upcast_ref(),
        ])
        .map_err(|e| EncodeError::PipelineLink(e.to_string()))?;

    gst::Element::link_many([
        appsrc.upcast_ref(),
        &videoconvert,
        &capsfilter,
        &encoder,
        &h264parse,
        appsink.upcast_ref(),
    ])
    .map_err(|e| EncodeError::PipelineLink(e.to_string()))?;

    tracing::info!(
        %encoder_type,
        width = config.width,
        height = config.height,
        bitrate = config.bitrate,
        framerate = config.framerate,
        "GStreamer H.264 pipeline built"
    );

    Ok((pipeline, appsrc, appsink))
}

/// Create a `GStreamer` element by factory name.
fn make_element(factory_name: &str, element_name: &str) -> Result<gst::Element, EncodeError> {
    gst::ElementFactory::make(factory_name)
        .name(element_name)
        .build()
        .map_err(|e| EncodeError::ElementCreate {
            name: factory_name.to_string(),
            reason: e.to_string(),
        })
}

/// Configure encoder-specific properties.
fn configure_encoder(encoder: &gst::Element, encoder_type: EncoderType, config: &EncoderConfig) {
    let bitrate_kbps = config.bitrate / 1000;

    match encoder_type {
        EncoderType::Vaapi => {
            encoder.set_property("rate-control", 2u32); // CBR
            encoder.set_property("bitrate", bitrate_kbps);
            encoder.set_property("keyframe-period", config.keyframe_interval);
            if config.low_latency {
                encoder.set_property("tune", 3u32); // low-latency
            }
        }
        EncoderType::Nvenc => {
            encoder.set_property("bitrate", bitrate_kbps);
            #[allow(clippy::cast_possible_wrap)]
            let gop = config.keyframe_interval as i32;
            encoder.set_property("gop-size", gop);
            if config.low_latency {
                encoder.set_property("preset", 5u32); // low-latency-hq
                encoder.set_property("zerolatency", true);
            }
        }
        EncoderType::Software => {
            encoder.set_property("bitrate", bitrate_kbps);
            encoder.set_property("key-int-max", config.keyframe_interval);
            if config.low_latency {
                encoder.set_property_from_str("tune", "zerolatency");
                encoder.set_property_from_str("speed-preset", "ultrafast");
            }
        }
    }

    tracing::debug!(
        %encoder_type,
        bitrate_kbps,
        keyframe_interval = config.keyframe_interval,
        low_latency = config.low_latency,
        "Encoder configured"
    );
}
