use anyhow::{Context as _, Result, bail};
use block::ConcreteBlock;
use cocoa::{
    base::{BOOL, YES, id, nil},
    foundation::NSString,
};
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef, TCFType};
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use futures::{
    StreamExt as _,
    channel::{mpsc, oneshot},
};
use gpui::{AsyncApp, Context, Empty, IntoElement, Render, ScreenCaptureFrame, Task, Window};
use livekit::webrtc::{
    video_frame::{VideoFrame, VideoRotation},
    video_source::{RtcVideoSource, VideoResolution, native::NativeVideoSource},
};
use media::core_media::{CMSampleBuffer, CMSampleBufferRef};
use objc::{
    declare::ClassDecl,
    msg_send,
    rc::StrongPtr,
    runtime::{Class, Object, Sel},
    sel, sel_impl,
};
use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use super::{
    LocalVideoTrack,
    playback::{RemoteVideoFrame, video_frame_buffer_to_webrtc},
};
use crate::camera_health::CameraActivity;

const FRAME_CALLBACK_IVAR: &str = "frame_callback";
const AV_AUTHORIZATION_NOT_DETERMINED: isize = 0;
const AV_AUTHORIZATION_AUTHORIZED: isize = 3;

/// Owns an active AVCaptureSession. Dropping it synchronously stops capture,
/// detaches the sample callback, and releases the serial capture queue.
pub struct CameraCaptureStream {
    device_id: String,
    activity: Arc<CameraActivity>,
    session: StrongPtr,
    output: StrongPtr,
    delegate: StrongPtr,
    _queue: DispatchRetained<DispatchQueue>,
}

pub struct CameraPreviewView {
    latest_frame: Option<RemoteVideoFrame>,
    _maintain_frame: Task<()>,
}

pub(crate) struct SendablePixelBuffer(usize);

// CoreVideo buffers are reference-counted and may be retained on the capture
// queue, then reconstructed on GPUI's foreground executor. The wrapper owns
// exactly one retain and releases it if a queued frame is dropped unread.
unsafe impl Send for SendablePixelBuffer {}

impl SendablePixelBuffer {
    fn retained(frame: &ScreenCaptureFrame) -> Self {
        let reference = frame.0.as_concrete_TypeRef() as CFTypeRef;
        unsafe {
            CFRetain(reference);
        }
        Self(reference as usize)
    }

    fn into_pixel_buffer(self) -> core_video::pixel_buffer::CVPixelBuffer {
        let reference = self.0 as core_video::pixel_buffer::CVPixelBufferRef;
        std::mem::forget(self);
        unsafe { core_video::pixel_buffer::CVPixelBuffer::wrap_under_create_rule(reference) }
    }
}

impl Drop for SendablePixelBuffer {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0 as CFTypeRef);
        }
    }
}

impl CameraPreviewView {
    pub(crate) fn new(
        mut frames: mpsc::Receiver<SendablePixelBuffer>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            latest_frame: None,
            _maintain_frame: cx.spawn(async move |this, cx| {
                while let Some(frame) = frames.next().await {
                    this.update(cx, |this, cx| {
                        this.latest_frame = Some(frame.into_pixel_buffer());
                        cx.notify();
                    })
                    .ok();
                }
            }),
        }
    }
}

impl Render for CameraPreviewView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(frame) = &self.latest_frame {
            use gpui::Styled as _;
            return gpui::surface(frame.clone()).size_full().into_any_element();
        }
        Empty.into_any_element()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraDevice {
    pub id: String,
    pub label: String,
}

pub fn available_camera_devices() -> Result<Vec<CameraDevice>> {
    unsafe {
        let device_class =
            Class::get("AVCaptureDevice").context("AVFoundation camera support is unavailable")?;
        let media_type = NSString::alloc(nil).init_str("vide");
        let media_type = StrongPtr::new(media_type);
        let devices: id = msg_send![device_class, devicesWithMediaType: *media_type];
        if devices == nil {
            return Ok(Vec::new());
        }
        let count: usize = msg_send![devices, count];
        let mut result = Vec::with_capacity(count);
        for index in 0..count {
            let device: id = msg_send![devices, objectAtIndex: index];
            if device == nil {
                continue;
            }
            let unique_id: id = msg_send![device, uniqueID];
            let label: id = msg_send![device, localizedName];
            let id = ns_string(unique_id);
            if id.is_empty() {
                continue;
            }
            result.push(CameraDevice {
                id,
                label: ns_string(label),
            });
        }
        result.sort_by(|left, right| {
            left.label
                .to_lowercase()
                .cmp(&right.label.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(result)
    }
}

pub(crate) async fn capture_local_camera_track(
    device_id: Option<String>,
    cx: &mut AsyncApp,
) -> Result<(
    LocalVideoTrack,
    CameraCaptureStream,
    mpsc::Receiver<SendablePixelBuffer>,
)> {
    ensure_camera_permission().await?;
    let track_source = gpui_tokio::Tokio::spawn(cx, async move {
        NativeVideoSource::new(
            VideoResolution {
                width: 1280,
                height: 720,
            },
            true,
        )
    })
    .await?;
    let (frame_tx, frame_rx) = mpsc::channel(1);
    let frame_tx = Mutex::new(frame_tx);
    let stream = CameraCaptureStream::start(device_id.as_deref(), {
        let track_source = track_source.clone();
        Box::new(move |frame| {
            let preview = SendablePixelBuffer::retained(&frame);
            if let Ok(mut frame_tx) = frame_tx.lock() {
                let _ = frame_tx.try_send(preview);
            }
            if let Some(buffer) = video_frame_buffer_to_webrtc(frame) {
                track_source.capture_frame(&VideoFrame {
                    rotation: VideoRotation::VideoRotation0,
                    timestamp_us: 0,
                    buffer,
                });
            }
        })
    })?;
    Ok((
        LocalVideoTrack(livekit::track::LocalVideoTrack::create_video_track(
            "camera",
            RtcVideoSource::Native(track_source),
        )),
        stream,
        frame_rx,
    ))
}

async fn ensure_camera_permission() -> Result<()> {
    unsafe {
        let device_class =
            Class::get("AVCaptureDevice").context("AVFoundation camera support is unavailable")?;
        let media_type = NSString::alloc(nil).init_str("vide");
        let media_type = StrongPtr::new(media_type);
        let status: isize = msg_send![device_class, authorizationStatusForMediaType: *media_type];
        match status {
            AV_AUTHORIZATION_AUTHORIZED => return Ok(()),
            AV_AUTHORIZATION_NOT_DETERMINED => {}
            _ => bail!("Camera access is disabled in macOS Privacy & Security settings"),
        }

        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(Mutex::new(Some(tx)));
        let completion = ConcreteBlock::new(move |granted: BOOL| {
            if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
                let _ = tx.send(granted == YES);
            }
        })
        .copy();
        let _: () = msg_send![
            device_class,
            requestAccessForMediaType: *media_type
            completionHandler: completion
        ];
        if rx
            .await
            .context("camera permission request was cancelled")?
        {
            Ok(())
        } else {
            bail!("Camera permission was denied")
        }
    }
}

impl CameraCaptureStream {
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Returns true when AVFoundation has stopped delivering frames for the
    /// supplied interval. Device enumeration alone cannot detect session
    /// interruptions (for example, another application taking exclusive
    /// access), so callers use this as a recovery signal.
    pub fn is_stalled(&self, timeout: Duration) -> bool {
        self.activity.is_stalled(timeout)
    }

    fn start(
        device_id: Option<&str>,
        frame_callback: Box<dyn Fn(ScreenCaptureFrame) + Send>,
    ) -> Result<Self> {
        unsafe {
            let activity = Arc::new(CameraActivity::new());
            let device_class = Class::get("AVCaptureDevice")
                .context("AVFoundation camera support is unavailable")?;
            let input_class = Class::get("AVCaptureDeviceInput")
                .context("AVFoundation camera input is unavailable")?;
            let output_class = Class::get("AVCaptureVideoDataOutput")
                .context("AVFoundation camera output is unavailable")?;
            let session_class = Class::get("AVCaptureSession")
                .context("AVFoundation capture sessions are unavailable")?;
            let media_type = NSString::alloc(nil).init_str("vide");
            let media_type = StrongPtr::new(media_type);
            let device = if let Some(device_id) = device_id {
                let devices: id = msg_send![device_class, devicesWithMediaType: *media_type];
                let count: usize = if devices == nil {
                    0
                } else {
                    msg_send![devices, count]
                };
                let mut selected = nil;
                for index in 0..count {
                    let candidate: id = msg_send![devices, objectAtIndex: index];
                    let unique_id: id = msg_send![candidate, uniqueID];
                    if ns_string(unique_id) == device_id {
                        selected = candidate;
                        break;
                    }
                }
                if selected == nil {
                    bail!("The selected camera is no longer available")
                }
                selected
            } else {
                msg_send![device_class, defaultDeviceWithMediaType: *media_type]
            };
            if device == nil {
                bail!("No camera is available")
            }
            let active_device_id: id = msg_send![device, uniqueID];
            let active_device_id = ns_string(active_device_id);

            let mut error: id = nil;
            let input: id = msg_send![input_class, deviceInputWithDevice: device error: &mut error];
            if input == nil {
                bail!("Could not open the camera: {}", ns_error_message(error))
            }
            let input = StrongPtr::retain(input);

            let output: id = msg_send![output_class, alloc];
            let output: id = msg_send![output, init];
            if output == nil {
                bail!("Could not create the camera video output")
            }
            let output = StrongPtr::new(output);
            let _: () = msg_send![*output, setAlwaysDiscardsLateVideoFrames: YES];

            let delegate_class = camera_output_class();
            let delegate: id = msg_send![delegate_class, alloc];
            let delegate: id = msg_send![delegate, init];
            if delegate == nil {
                bail!("Could not create the camera frame receiver")
            }
            let delegate = StrongPtr::new(delegate);
            let activity_for_callback = Arc::clone(&activity);
            let callback: Box<Box<dyn Fn(ScreenCaptureFrame) + Send>> =
                Box::new(Box::new(move |frame| {
                    activity_for_callback.mark_frame();
                    frame_callback(frame);
                }));
            delegate
                .as_mut()
                .context("camera frame receiver was released")?
                .set_ivar(FRAME_CALLBACK_IVAR, Box::into_raw(callback) as *mut c_void);

            let queue =
                DispatchQueue::new("dev.zed.livekit.camera-capture", DispatchQueueAttr::SERIAL);
            let queue_ptr = (&*queue as *const DispatchQueue)
                .cast_mut()
                .cast::<Object>();
            let _: () = msg_send![
                *output,
                setSampleBufferDelegate: *delegate
                queue: queue_ptr
            ];

            let session: id = msg_send![session_class, alloc];
            let session: id = msg_send![session, init];
            if session == nil {
                bail!("Could not create the camera capture session")
            }
            let session = StrongPtr::new(session);
            let _: () = msg_send![*session, beginConfiguration];
            let preset = NSString::alloc(nil).init_str("AVCaptureSessionPreset1280x720");
            let preset = StrongPtr::new(preset);
            let can_preset: BOOL = msg_send![*session, canSetSessionPreset: *preset];
            if can_preset == YES {
                let _: () = msg_send![*session, setSessionPreset: *preset];
            }
            let can_add_input: BOOL = msg_send![*session, canAddInput: *input];
            let can_add_output: BOOL = msg_send![*session, canAddOutput: *output];
            if can_add_input != YES || can_add_output != YES {
                let _: () = msg_send![*session, commitConfiguration];
                bail!("The selected camera cannot provide a video stream")
            }
            let _: () = msg_send![*session, addInput: *input];
            let _: () = msg_send![*session, addOutput: *output];
            let _: () = msg_send![*session, commitConfiguration];
            let _: () = msg_send![*session, startRunning];

            Ok(Self {
                device_id: active_device_id,
                activity,
                session,
                output,
                delegate,
                _queue: queue,
            })
        }
    }
}

impl Drop for CameraCaptureStream {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![*self.session, stopRunning];
            let _: () = msg_send![
                *self.output,
                setSampleBufferDelegate: nil
                queue: nil
            ];
            if let Some(delegate) = self.delegate.as_mut() {
                let callback = *delegate.get_ivar::<*mut c_void>(FRAME_CALLBACK_IVAR);
                if !callback.is_null() {
                    delegate.set_ivar(FRAME_CALLBACK_IVAR, ptr::null_mut::<c_void>());
                    drop(Box::from_raw(
                        callback as *mut Box<dyn Fn(ScreenCaptureFrame) + Send>,
                    ));
                }
            }
        }
    }
}

fn camera_output_class() -> &'static Class {
    static CLASS: OnceLock<&'static Class> = OnceLock::new();
    CLASS.get_or_init(|| unsafe {
        if let Some(class) = Class::get("ZedLiveKitCameraOutput") {
            return class;
        }
        let mut declaration =
            ClassDecl::new("ZedLiveKitCameraOutput", Class::get("NSObject").unwrap()).unwrap();
        declaration.add_ivar::<*mut c_void>(FRAME_CALLBACK_IVAR);
        declaration.add_method(
            sel!(captureOutput:didOutputSampleBuffer:fromConnection:),
            did_output_sample_buffer as extern "C" fn(&Object, Sel, id, id, id),
        );
        declaration.register()
    })
}

extern "C" fn did_output_sample_buffer(
    this: &Object,
    _: Sel,
    _output: id,
    sample_buffer: id,
    _connection: id,
) {
    unsafe {
        let callback = *this.get_ivar::<*mut c_void>(FRAME_CALLBACK_IVAR);
        if callback.is_null() || sample_buffer == nil {
            return;
        }
        let sample = CMSampleBuffer::wrap_under_get_rule(sample_buffer as CMSampleBufferRef);
        if let Some(buffer) = sample.image_buffer() {
            let callback = &*(callback as *const Box<dyn Fn(ScreenCaptureFrame) + Send>);
            callback(ScreenCaptureFrame(buffer));
        }
    }
}

fn ns_error_message(error: id) -> String {
    if error == nil {
        return "unknown AVFoundation error".into();
    }
    unsafe {
        let description: id = msg_send![error, localizedDescription];
        if description == nil {
            return "unknown AVFoundation error".into();
        }
        let bytes: *const std::os::raw::c_char = msg_send![description, UTF8String];
        if bytes.is_null() {
            "unknown AVFoundation error".into()
        } else {
            CStr::from_ptr(bytes).to_string_lossy().into_owned()
        }
    }
}

unsafe fn ns_string(value: id) -> String {
    if value == nil {
        return String::new();
    }
    let bytes: *const std::os::raw::c_char = msg_send![value, UTF8String];
    if bytes.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(bytes) }
            .to_string_lossy()
            .into_owned()
    }
}
