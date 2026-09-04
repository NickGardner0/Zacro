use crate::test;

pub(crate) mod participant;
pub(crate) mod publication;
pub(crate) mod track;

pub type RemoteVideoTrack = track::RemoteVideoTrack;
pub type RemoteAudioTrack = track::RemoteAudioTrack;
pub type RemoteTrackPublication = publication::RemoteTrackPublication;
pub type RemoteParticipant = participant::RemoteParticipant;

pub type LocalVideoTrack = track::LocalVideoTrack;
pub type LocalAudioTrack = track::LocalAudioTrack;
pub type LocalTrackPublication = publication::LocalTrackPublication;
pub type LocalParticipant = participant::LocalParticipant;

pub type Room = test::Room;
pub use test::{ConnectionState, ParticipantIdentity, RtcStats, SessionStats, TrackSid};

pub struct AudioStream {}

#[cfg(target_os = "macos")]
pub struct CameraCaptureStream {
    pub(crate) device_id: String,
}

#[cfg(target_os = "macos")]
impl CameraCaptureStream {
    pub fn device_id(&self) -> &str {
        &self.device_id
    }
}

#[cfg(target_os = "macos")]
pub struct CameraPreviewView {}

#[cfg(target_os = "macos")]
impl gpui::Render for CameraPreviewView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::Empty
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraDevice {
    pub id: String,
    pub label: String,
}

#[cfg(target_os = "macos")]
pub fn available_camera_devices() -> anyhow::Result<Vec<CameraDevice>> {
    Ok(vec![CameraDevice {
        id: "test-camera".into(),
        label: "Test Camera".into(),
    }])
}

impl AudioStream {
    pub fn remote_playback_stats(&self) -> Option<crate::RemoteAudioPlaybackStats> {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub type RemoteVideoFrame = std::sync::Arc<gpui::RenderImage>;

#[cfg(target_os = "macos")]
#[derive(Clone)]
pub(crate) struct RemoteVideoFrame {}
#[cfg(target_os = "macos")]
impl Into<gpui::SurfaceSource> for RemoteVideoFrame {
    fn into(self) -> gpui::SurfaceSource {
        unimplemented!()
    }
}
pub(crate) fn play_remote_video_track(
    _track: &crate::RemoteVideoTrack,
    _: &gpui::BackgroundExecutor,
) -> impl futures::Stream<Item = RemoteVideoFrame> + use<> {
    futures::stream::pending()
}
