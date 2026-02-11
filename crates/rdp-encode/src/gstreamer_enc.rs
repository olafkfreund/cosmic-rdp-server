//! `GStreamer` H.264 encoding pipeline.
//!
//! Pipeline: `appsrc ! videoconvert ! capsfilter(I420,BT.601-limited) ! encoder ! h264parse ! appsink`
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
/// `appsrc ! videoconvert ! capsfilter(I420,BT.601 limited) ! encoder ! h264parse ! appsink`
///
/// Push raw BGRx/BGRA frames via [`encode_frame`](GstEncoder::encode_frame)
/// and receive H.264 NAL units in byte-stream format.
pub struct GstEncoder {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    appsink: gst_app::AppSink,
    encoder_type: EncoderType,
    width: u32,
    height: u32,
    running: bool,
    /// Log negotiated caps once after first successful buffer push.
    caps_logged: bool,
    /// Dump first raw frame + keyframe to /tmp for color diagnostics.
    frame_dumped: bool,
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
            width: config.width,
            height: config.height,
            running: false,
            caps_logged: false,
            frame_dumped: false,
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

        // Dump the first raw frame for color diagnostics.
        if !self.frame_dumped {
            if let Err(e) = std::fs::write("/tmp/rdp_raw_frame.bgra", frame_data) {
                tracing::warn!("Frame dump: failed to write raw frame: {e}");
            } else {
                tracing::info!(
                    width = self.width,
                    height = self.height,
                    size = frame_data.len(),
                    "Frame dump: raw BGRx -> /tmp/rdp_raw_frame.bgra \
                     (view: ffplay -f rawvideo -pixel_format bgra -video_size {}x{} /tmp/rdp_raw_frame.bgra)",
                    self.width, self.height
                );
            }
        }

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
        let result = self.pull_encoded_frame()?;

        // Dump first H.264 keyframe for external analysis.
        if let Some(ref frame) = result {
            if frame.is_keyframe && !self.frame_dumped {
                self.frame_dumped = true;
                if let Err(e) = std::fs::write("/tmp/rdp_h264_keyframe.h264", &frame.data) {
                    tracing::warn!("Frame dump: failed to write H.264 keyframe: {e}");
                } else {
                    tracing::info!(
                        size = frame.data.len(),
                        "Frame dump: H.264 keyframe -> /tmp/rdp_h264_keyframe.h264 \
                         (decode: ffmpeg -i /tmp/rdp_h264_keyframe.h264 -vframes 1 /tmp/rdp_decoded.png)"
                    );
                }
            }
        }

        Ok(result)
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
/// `appsrc ! videoconvert ! capsfilter(I420,BT.601-limited) ! encoder ! h264parse ! appsink`
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

    // AppSrc: raw video input from PipeWire (BGRx = memory [B, G, R, x]).
    //
    // No colorimetry is set — GStreamer defaults to BT.709 for HD
    // resolution, which is correct for desktop content.
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

    // videoconvert: RGB→YUV color space conversion.
    let videoconvert = make_element("videoconvert", "convert")?;

    // Capsfilter: force I420 with BT.601 limited-range colorimetry.
    //
    // FreeRDP hardcodes BT.601 limited-range coefficients for AVC420
    // H.264 decode (prim_YUV.c) and ignores H.264 VUI flags entirely.
    // XRDP also uses BT.601 limited-range with x264.  We MUST match:
    //   "bt601" = 2:1:4:6 = limited : BT601 matrix : BT601 transfer : SMPTE170M primaries
    let capsfilter = make_element("capsfilter", "filter")?;
    capsfilter.set_property(
        "caps",
        gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("colorimetry", "bt601")
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

    // Pipeline: appsrc(BGRx) ! videoconvert ! capsfilter(I420 BT.601-limited) ! encoder ! h264parse ! appsink
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
            // Signal BT.601 limited-range in H.264 SPS VUI.
            // FreeRDP ignores VUI but this keeps metadata consistent
            // with the actual BT.601 limited-range encoding.
            encoder.set_property_from_str(
                "option-string",
                "colorprim=bt601:transfer=bt601:colormatrix=bt601",
            );
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
