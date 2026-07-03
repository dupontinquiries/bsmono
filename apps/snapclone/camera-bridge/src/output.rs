//! Virtual camera output: appsrc → videoconvert → pipewiresink.
//!
//! # postmarketOS / Alpine compatibility
//!
//! ## props= must be in the pipeline string
//! On Alpine gst-pipewire, pw_stream_connect() fires at READY→PAUSED.
//! Properties set via set_property() after that transition are ignored.
//! Embedding props= in the pipeline string guarantees they're applied first.
//!
//! ## Format negotiation
//! appsrc delivers RGB24 (from our processing pipeline).
//! videoconvert converts to whatever the consumer needs.
//! We do NOT hardcode the output format — let pipewiresink negotiate.
//! Hardcoding NV12 caused "could not link" errors when consumers expected BGRA.
//!
//! ## Staged state machine
//! READY → (200ms) → PLAYING
//! The 200ms pause lets wireplumber register the PipeWire node before
//! the appsrc starts pushing buffers.

use anyhow::{anyhow, Context, Result};
use gstreamer::prelude::*;
use gstreamer_app::AppSrc;
use log::{debug, info, warn};

use crate::capture::Frame;

pub struct VirtualCamera {
    pipeline:    gstreamer::Pipeline,
    appsrc:      AppSrc,
    width:       u32,
    height:      u32,
    frame_bytes: usize,
    node_name:   String,
}

impl VirtualCamera {
    pub fn new(
        node_name:        &str,
        node_description: &str,
        width:            u32,
        height:           u32,
    ) -> Result<Self> {
        gstreamer::init()?;

        let frame_bytes = (width * height * 3) as usize; // RGB24

        // IMPORTANT: do NOT quote the prop values inside props=
        // Some versions of gst-pipewire parse the props= string strictly
        // and reject quoted values. Use bare key=value pairs.
        //
        // Do NOT hardcode the output format after videoconvert.
        // Let pipewiresink negotiate with the consumer.
        // This avoids "could not link" errors on consumers that want BGRA or MJPEG.
        // Note: stream-properties is NOT set in the pipeline string here.
        // gst-launch syntax can't reliably express a comma-separated GstStructure
        // value (the parser splits on commas without quoting), causing
        // "Failed to parse output pipeline" errors at runtime.
        // We set stream-properties programmatically below via the "pw" name.
        let pipeline_str = format!(
            "appsrc name=src \
               format=time \
               is-live=true \
               block=false \
               max-bytes={frame_bytes} \
               caps=video/x-raw,format=RGB,width={width},height={height},framerate=30/1 \
             ! queue max-size-buffers=2 leaky=upstream \
             ! videoconvert \
             ! pipewiresink name=pw sync=false"
        );

        info!("[{node_name}] Pipeline: {pipeline_str}");

        let pipeline = gstreamer::parse::launch(&pipeline_str)
            .context(format!("Failed to parse output pipeline for {node_name}"))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow!("Not a Pipeline"))?;

        let appsrc = pipeline
            .by_name("src")
            .ok_or_else(|| anyhow!("No appsrc in pipeline"))?
            .downcast::<AppSrc>()
            .map_err(|_| anyhow!("src is not AppSrc"))?;

        appsrc.set_format(gstreamer::Format::Time);
        appsrc.set_is_live(true);
        appsrc.set_do_timestamp(true);
        appsrc.set_max_bytes(frame_bytes as u64 * 3); // allow 3 frames queued
        appsrc.set_block(false);

        // ── Set pipewiresink stream-properties programmatically ──────────────
        // pipewiresink advertises this struct to PipeWire. Setting media.class
        // = "Video/Source" makes the node discoverable by camera-consuming
        // apps (gst-launch pipewiresrc, GStreamer QCamera, libcamera-apps).
        // Without it, the node is created but tagged as a generic stream and
        // wouldn't show up in `pw-cli ls Node | grep Video/Source`.
        let pwsink = pipeline
            .by_name("pw")
            .ok_or_else(|| anyhow!("No pipewiresink named 'pw' in pipeline"))?;
        let stream_props = gstreamer::Structure::builder("properties")
            .field("media.class",       "Video/Source")
            .field("node.name",         node_name)
            .field("node.description",  node_description)
            .field("media.role",        "Camera")
            .build();
        pwsink.set_property("stream-properties", &stream_props);

        // READY → wait → PLAYING (not PAUSED in between)
        // Going through PAUSED causes the sink to try to preroll which fails
        // when there's no consumer yet.
        pipeline
            .set_state(gstreamer::State::Ready)
            .context("Failed to set pipeline READY")?;

        std::thread::sleep(std::time::Duration::from_millis(250));

        pipeline
            .set_state(gstreamer::State::Playing)
            .context("Failed to set pipeline PLAYING")?;

        // Wait up to 8s for PLAYING — but accept PAUSED as well.
        //
        // Why we accept PAUSED:
        // `appsrc ! videoconvert ! pipewiresink` with no downstream consumer
        // commonly gets stuck transitioning PAUSED → PLAYING because the
        // sink waits for caps negotiation with a consumer that doesn't
        // exist yet. The node IS advertised to PipeWire from PAUSED — any
        // app connecting will see it and trigger the negotiation that
        // finishes the transition.
        //
        // Treating PAUSED as success here means the bridge stops looping
        // forever trying to reach PLAYING. If a real consumer never
        // connects, the pipeline just sits idle (no buffers consumed),
        // which is correct behavior for a publish-only virtual source.
        match pipeline.state(gstreamer::ClockTime::from_seconds(8)) {
            (Ok(_), gstreamer::State::Playing, _) => {}
            (Ok(_), gstreamer::State::Paused, _) => {
                info!("[{node_name}] Pipeline reached PAUSED (waiting for first consumer) — accepting as ready");
            }
            (_, state, _) => {
                // Collect errors from bus before bailing
                if let Some(bus) = pipeline.bus() {
                    while let Some(msg) = bus.timed_pop(gstreamer::ClockTime::ZERO) {
                        if let gstreamer::MessageView::Error(e) = msg.view() {
                            warn!("[{node_name}] GST error: {} — {:?}", e.error(), e.debug());
                        }
                    }
                }
                let _ = pipeline.set_state(gstreamer::State::Null);
                anyhow::bail!("[{node_name}] Pipeline stuck in {state:?} after 8s");
            }
        }

        info!("[{node_name}] Virtual camera ready ({}×{})", width, height);
        Ok(Self { pipeline, appsrc, width, height, frame_bytes, node_name: node_name.to_string() })
    }

    pub fn write_frame(&mut self, frame: &Frame) -> Result<()> {
        if frame.data.len() != self.frame_bytes {
            return Err(anyhow!("[{}] frame {} bytes ≠ {} expected",
                self.node_name, frame.data.len(), self.frame_bytes));
        }

        let mut buf = gstreamer::Buffer::with_size(self.frame_bytes)
            .map_err(|_| anyhow!("[{}] Buffer alloc failed", self.node_name))?;
        {
            let b = buf.get_mut().ok_or_else(|| anyhow!("buffer not mutable"))?;
            let mut map = b.map_writable().context("map_writable failed")?;
            // Resize frame if needed (sensor may deliver different resolution)
            if frame.width == self.width && frame.height == self.height {
                map.as_mut_slice().copy_from_slice(&frame.data);
            } else {
                // Simple nearest-neighbour resize for size mismatch
                let (fw, fh) = (frame.width as usize, frame.height as usize);
                let (ow, oh) = (self.width as usize, self.height as usize);
                let dst = map.as_mut_slice();
                for oy in 0..oh {
                    for ox in 0..ow {
                        let sx = ox * fw / ow;
                        let sy = oy * fh / oh;
                        let si = (sy * fw + sx) * 3;
                        let di = (oy * ow + ox) * 3;
                        if si + 2 < frame.data.len() && di + 2 < dst.len() {
                            dst[di]   = frame.data[si];
                            dst[di+1] = frame.data[si+1];
                            dst[di+2] = frame.data[si+2];
                        }
                    }
                }
            }
        }

        match self.appsrc.push_buffer(buf) {
            Ok(_)  => { debug!("[{}] frame pushed", self.node_name); Ok(()) }
            Err(e) => Err(anyhow!("[{}] push_buffer: {e:?}", self.node_name)),
        }
    }

    pub fn node_name(&self) -> &str { &self.node_name }
    pub fn width(&self)     -> u32  { self.width }
    pub fn height(&self)    -> u32  { self.height }
}

impl Drop for VirtualCamera {
    fn drop(&mut self) {
        info!("[{}] Shutting down virtual camera", self.node_name);
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }
}
