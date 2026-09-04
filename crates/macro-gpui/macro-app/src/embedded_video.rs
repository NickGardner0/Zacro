use editor::{Editor as ZedTextEditor, EditorEvent as ZedEditorEvent};
use gpui::prelude::FluentBuilder;
use gpui::*;
use macro_api::CallRecordTranscriptSegment;
use macro_ui::{InputFrame, h_flex, macro_colors, v_flex};
use std::time::Duration;

#[derive(Clone)]
struct TimedTranscriptSegment {
    sequence_num: i64,
    speaker: String,
    content: String,
    offset_seconds: f64,
    end_seconds: Option<f64>,
}

/// A native video player used by Macro canvas video nodes.
///
/// On macOS the decoder is AVFoundation and each decoded CoreVideo frame is
/// presented directly by GPUI. The entity is deliberately cached by the
/// canvas host so rerenders and canvas gestures never recreate the decoder.
pub(crate) struct EmbeddedVideoView {
    url: String,
    playback: Option<platform::NativeVideoPlayback>,
    latest_frame: Option<platform::VideoFrame>,
    playing: bool,
    muted: bool,
    current_seconds: f64,
    duration_seconds: Option<f64>,
    recording_anchor: Option<String>,
    transcript: Vec<TimedTranscriptSegment>,
    transcript_search_editor: Option<Entity<ZedTextEditor>>,
    transcript_search_query: String,
    transcript_search_match: usize,
    _transcript_search_subscription: Option<Subscription>,
    error: Option<String>,
    _poll_frames: Option<Task<()>>,
}

impl EmbeddedVideoView {
    pub(crate) fn new(url: String, cx: &mut Context<Self>) -> Self {
        Self::new_with_transcript(url, None, Vec::new(), cx)
    }

    pub(crate) fn new_call_recording(
        url: String,
        recording_anchor: Option<&str>,
        transcript: Vec<CallRecordTranscriptSegment>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_transcript(
            url,
            recording_anchor,
            timed_transcript(recording_anchor, transcript),
            cx,
        )
    }

    fn new_with_transcript(
        url: String,
        recording_anchor: Option<&str>,
        transcript: Vec<TimedTranscriptSegment>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (playback, error) = match platform::NativeVideoPlayback::new(&url) {
            Ok(playback) => (Some(playback), None),
            Err(error) => (None, Some(error)),
        };
        let mut this = Self {
            url,
            playback,
            latest_frame: None,
            playing: false,
            muted: false,
            current_seconds: 0.0,
            duration_seconds: None,
            recording_anchor: recording_anchor.map(str::to_string),
            transcript,
            transcript_search_editor: None,
            transcript_search_query: String::new(),
            transcript_search_match: 0,
            _transcript_search_subscription: None,
            error,
            _poll_frames: None,
        };
        if this.playback.is_some() {
            this._poll_frames = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(16))
                        .await;
                    let keep_polling = this
                        .update(cx, |this, cx| {
                            let Some(playback) = &this.playback else {
                                return false;
                            };
                            if let Some(frame) = playback.copy_new_frame() {
                                this.latest_frame = Some(frame);
                                cx.notify();
                            }
                            let current_seconds = playback.current_time_seconds();
                            if (current_seconds - this.current_seconds).abs() >= 0.05 {
                                this.current_seconds = current_seconds;
                                cx.notify();
                            }
                            let duration_seconds = playback.duration_seconds();
                            if duration_seconds != this.duration_seconds {
                                this.duration_seconds = duration_seconds;
                                cx.notify();
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !keep_polling {
                        break;
                    }
                }
            }));
        }
        this
    }

    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn matches_call_recording(
        &self,
        url: &str,
        recording_anchor: Option<&str>,
        transcript: &[CallRecordTranscriptSegment],
    ) -> bool {
        self.url == url
            && self.recording_anchor.as_deref() == recording_anchor
            && self.transcript.len() == transcript.len()
            && self
                .transcript
                .iter()
                .zip(transcript)
                .all(|(cached, segment)| {
                    cached.sequence_num == segment.sequence_num
                        && cached.speaker == segment.speaker_id
                        && cached.content == segment.content
                })
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        let Some(playback) = &self.playback else {
            return;
        };
        if self.playing {
            playback.pause();
        } else {
            playback.play();
        }
        self.playing = !self.playing;
        cx.notify();
    }

    fn toggle_muted(&mut self, cx: &mut Context<Self>) {
        let Some(playback) = &self.playback else {
            return;
        };
        self.muted = !self.muted;
        playback.set_muted(self.muted);
        cx.notify();
    }

    fn seek_to(&mut self, seconds: f64, cx: &mut Context<Self>) {
        let Some(playback) = &self.playback else {
            return;
        };
        let maximum = self.duration_seconds.unwrap_or(seconds).max(0.0);
        let seconds = seconds.clamp(0.0, maximum);
        playback.seek_to(seconds);
        self.current_seconds = seconds;
        cx.notify();
    }

    fn seek_by(&mut self, delta_seconds: f64, cx: &mut Context<Self>) {
        self.seek_to(self.current_seconds + delta_seconds, cx);
    }

    fn transcript_match_indices(&self) -> Vec<usize> {
        transcript_match_indices(&self.transcript, &self.transcript_search_query)
    }

    fn seek_to_transcript_match(&mut self, delta: isize, cx: &mut Context<Self>) {
        let matches = self.transcript_match_indices();
        if matches.is_empty() {
            self.transcript_search_match = 0;
            cx.notify();
            return;
        }
        self.transcript_search_match = if delta.is_negative() {
            self.transcript_search_match
                .checked_sub(delta.unsigned_abs())
                .unwrap_or(matches.len() - 1)
        } else {
            (self.transcript_search_match + delta as usize) % matches.len()
        };
        self.transcript_search_match %= matches.len();
        self.seek_to(
            self.transcript[matches[self.transcript_search_match]].offset_seconds,
            cx,
        );
    }
}

impl Render for EmbeddedVideoView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.transcript.is_empty() && self.transcript_search_editor.is_none() {
            let editor = cx.new(|cx| {
                let mut editor = ZedTextEditor::single_line(window, cx);
                editor.set_placeholder_text("Search transcript…", window, cx);
                editor
            });
            let subscription = cx.subscribe_in(
                &editor,
                window,
                |this, editor, event: &ZedEditorEvent, _, cx| {
                    if !matches!(event, ZedEditorEvent::BufferEdited) {
                        return;
                    }
                    this.transcript_search_query = editor.read(cx).text(cx);
                    this.transcript_search_match = 0;
                    let first = this.transcript_match_indices().first().copied();
                    if let Some(index) = first {
                        this.seek_to(this.transcript[index].offset_seconds, cx);
                    } else {
                        cx.notify();
                    }
                },
            );
            self.transcript_search_editor = Some(editor);
            self._transcript_search_subscription = Some(subscription);
        }
        let colors = macro_colors(cx);
        let playing = self.playing;
        let muted = self.muted;
        let active_sequence = active_transcript_sequence(&self.transcript, self.current_seconds);
        let transcript_matches = self.transcript_match_indices();
        let transcript_match_label = if self.transcript_search_query.trim().is_empty() {
            String::new()
        } else if transcript_matches.is_empty() {
            "No matches".into()
        } else {
            format!(
                "{} of {}",
                self.transcript_search_match
                    .min(transcript_matches.len() - 1)
                    + 1,
                transcript_matches.len()
            )
        };
        let transcript_search_editor = self.transcript_search_editor.clone();
        let current_time = format_media_time(self.current_seconds);
        let duration = self
            .duration_seconds
            .map(format_media_time)
            .unwrap_or_else(|| "--:--".into());
        v_flex()
            .size_full()
            .min_h_0()
            .bg(colors.background)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .when_some(self.latest_frame.clone(), |this, frame| {
                        this.child(
                            platform::surface(frame)
                                .object_fit(ObjectFit::Contain)
                                .size_full(),
                        )
                    })
                    .when(
                        self.latest_frame.is_none() && self.error.is_none(),
                        |this| {
                            this.flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .text_color(colors.ink_subtle)
                                .child("Loading video…")
                        },
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .px_3()
                            .text_xs()
                            .text_color(colors.failure)
                            .child(error)
                    }),
            )
            .child(
                h_flex()
                    .h(gpui::px(28.0))
                    .flex_none()
                    .px_2()
                    .gap_2()
                    .border_t_1()
                    .border_color(colors.border_variant)
                    .bg(colors.surface.opacity(0.94))
                    .child(
                        div()
                            .id("embedded-video-play-pause")
                            .role(Role::Button)
                            .aria_label(if playing { "Pause video" } else { "Play video" })
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|this| this.bg(colors.hover))
                            .cursor_pointer()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.ink)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_playback(cx)))
                            .child(if playing { "Pause" } else { "Play" }),
                    )
                    .child(
                        div()
                            .id("embedded-video-mute")
                            .role(Role::Button)
                            .aria_label(if muted { "Unmute video" } else { "Mute video" })
                            .focusable()
                            .tab_stop(true)
                            .focus_visible(|this| this.bg(colors.hover))
                            .cursor_pointer()
                            .text_xs()
                            .text_color(colors.ink_muted)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_muted(cx)))
                            .child(if muted { "Unmute" } else { "Mute" }),
                    )
                    .when(!self.transcript.is_empty(), |this| {
                        this.child(
                            div()
                                .id("embedded-video-back-15")
                                .role(Role::Button)
                                .aria_label("Seek back 15 seconds")
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|this| this.bg(colors.hover))
                                .cursor_pointer()
                                .text_xs()
                                .text_color(colors.ink_muted)
                                .on_click(cx.listener(|this, _, _, cx| this.seek_by(-15.0, cx)))
                                .child("−15s"),
                        )
                        .child(
                            div()
                                .id("embedded-video-forward-15")
                                .role(Role::Button)
                                .aria_label("Seek forward 15 seconds")
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(|this| this.bg(colors.hover))
                                .cursor_pointer()
                                .text_xs()
                                .text_color(colors.ink_muted)
                                .on_click(cx.listener(|this, _, _, cx| this.seek_by(15.0, cx)))
                                .child("+15s"),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.ink_subtle)
                                .child(format!("{current_time} / {duration}")),
                        )
                    }),
            )
            .when(!self.transcript.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id("call-recording-transcript-scroll")
                        .h(px(220.0))
                        .flex_none()
                        .border_t_1()
                        .border_color(colors.border_variant)
                        .when_some(transcript_search_editor, |this, editor| {
                            let active = editor.read(cx).is_focused(window);
                            this.child(
                                h_flex()
                                    .h(px(38.0))
                                    .flex_none()
                                    .px_2()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(colors.border_variant)
                                    .child(
                                        InputFrame::new().active(active).child(
                                            div()
                                                .id("call-transcript-search")
                                                .role(Role::TextInput)
                                                .aria_label("Search call transcript")
                                                .w(px(240.0))
                                                .h(px(24.0))
                                                .child(editor),
                                        ),
                                    )
                                    .child(
                                        div()
                                            .min_w(px(64.0))
                                            .text_xs()
                                            .text_color(colors.ink_subtle)
                                            .child(transcript_match_label),
                                    )
                                    .child(
                                        div()
                                            .id("call-transcript-previous-match")
                                            .role(Role::Button)
                                            .aria_label("Previous transcript match")
                                            .focusable()
                                            .tab_stop(true)
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(colors.ink_muted)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.seek_to_transcript_match(-1, cx)
                                            }))
                                            .child("Previous"),
                                    )
                                    .child(
                                        div()
                                            .id("call-transcript-next-match")
                                            .role(Role::Button)
                                            .aria_label("Next transcript match")
                                            .focusable()
                                            .tab_stop(true)
                                            .cursor_pointer()
                                            .text_xs()
                                            .text_color(colors.ink_muted)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.seek_to_transcript_match(1, cx)
                                            }))
                                            .child("Next"),
                                    ),
                            )
                        })
                        .child(
                            v_flex()
                                .id("call-transcript-results")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .children(self.transcript.iter().cloned().enumerate().map(
                                    |(index, segment)| {
                                        let active = active_sequence == Some(segment.sequence_num);
                                        let matched = transcript_matches.contains(&index);
                                        let seek_seconds = segment.offset_seconds;
                                        h_flex()
                                            .id(("call-recording-transcript", index))
                                            .min_h(px(42.0))
                                            .px_3()
                                            .py_2()
                                            .gap_3()
                                            .border_b_1()
                                            .border_color(colors.border_variant)
                                            .bg(if active {
                                                colors.selected
                                            } else if matched {
                                                colors.warning.opacity(0.12)
                                            } else {
                                                colors.background.opacity(0.0)
                                            })
                                            .cursor_pointer()
                                            .hover(|this| this.bg(colors.hover))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.seek_to(seek_seconds, cx)
                                            }))
                                            .child(
                                                div()
                                                    .w(px(52.0))
                                                    .flex_none()
                                                    .text_xs()
                                                    .text_color(if active {
                                                        colors.accent
                                                    } else {
                                                        colors.ink_subtle
                                                    })
                                                    .child(format_media_time(
                                                        segment.offset_seconds,
                                                    )),
                                            )
                                            .child(
                                                div()
                                                    .w(px(120.0))
                                                    .flex_none()
                                                    .truncate()
                                                    .text_xs()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(colors.ink_muted)
                                                    .child(segment.speaker),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_sm()
                                                    .text_color(colors.ink)
                                                    .child(segment.content),
                                            )
                                    },
                                )),
                        ),
                )
            })
    }
}

fn timed_transcript(
    recording_anchor: Option<&str>,
    transcript: Vec<CallRecordTranscriptSegment>,
) -> Vec<TimedTranscriptSegment> {
    let anchor = recording_anchor
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.timestamp_millis());
    let mut segments = transcript
        .into_iter()
        .map(|segment| {
            let offset_seconds = anchor
                .and_then(|anchor| {
                    chrono::DateTime::parse_from_rfc3339(&segment.started_at)
                        .ok()
                        .map(|value| (value.timestamp_millis() - anchor).max(0) as f64 / 1000.0)
                })
                .unwrap_or_default();
            let end_seconds = anchor.and_then(|anchor| {
                segment.ended_at.as_deref().and_then(|value| {
                    chrono::DateTime::parse_from_rfc3339(value)
                        .ok()
                        .map(|value| (value.timestamp_millis() - anchor).max(0) as f64 / 1000.0)
                })
            });
            TimedTranscriptSegment {
                sequence_num: segment.sequence_num,
                speaker: segment.speaker_id,
                content: segment.content,
                offset_seconds,
                end_seconds,
            }
        })
        .collect::<Vec<_>>();
    segments.sort_by(|left, right| {
        left.offset_seconds
            .total_cmp(&right.offset_seconds)
            .then_with(|| left.sequence_num.cmp(&right.sequence_num))
    });
    segments
}

fn active_transcript_sequence(
    transcript: &[TimedTranscriptSegment],
    current_seconds: f64,
) -> Option<i64> {
    transcript
        .iter()
        .enumerate()
        .rev()
        .find(|(index, segment)| {
            if segment.offset_seconds > current_seconds + 0.25 {
                return false;
            }
            let end = segment
                .end_seconds
                .or_else(|| transcript.get(index + 1).map(|next| next.offset_seconds));
            end.map_or(true, |end| current_seconds < end)
        })
        .map(|(_, segment)| segment.sequence_num)
}

fn transcript_match_indices(transcript: &[TimedTranscriptSegment], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    transcript
        .iter()
        .enumerate()
        .filter_map(|(index, segment)| {
            let searchable = format!("{} {}", segment.speaker, segment.content).to_lowercase();
            searchable.contains(&query).then_some(index)
        })
        .collect()
}

fn format_media_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod transcript_tests {
    use super::{
        active_transcript_sequence, format_media_time, timed_transcript, transcript_match_indices,
    };
    use macro_api::CallRecordTranscriptSegment;

    fn segment(
        sequence_num: i64,
        started_at: &str,
        ended_at: Option<&str>,
    ) -> CallRecordTranscriptSegment {
        CallRecordTranscriptSegment {
            content: format!("segment {sequence_num}"),
            diarized_speaker_id: None,
            ended_at: ended_at.map(str::to_string),
            segment_id: Some(format!("segment-{sequence_num}")),
            sequence_num,
            speaker_id: format!("speaker-{sequence_num}"),
            started_at: started_at.to_string(),
            transcript_id: "transcript-1".into(),
        }
    }

    #[test]
    fn transcript_offsets_and_active_segment_follow_recording_time() {
        let transcript = timed_transcript(
            Some("2026-08-31T12:00:00Z"),
            vec![
                segment(2, "2026-08-31T12:00:10Z", Some("2026-08-31T12:00:14Z")),
                segment(1, "2026-08-31T12:00:05Z", Some("2026-08-31T12:00:09Z")),
            ],
        );

        assert_eq!(transcript[0].offset_seconds, 5.0);
        assert_eq!(transcript[1].offset_seconds, 10.0);
        assert_eq!(active_transcript_sequence(&transcript, 0.0), None);
        assert_eq!(active_transcript_sequence(&transcript, 5.0), Some(1));
        assert_eq!(active_transcript_sequence(&transcript, 10.0), Some(2));
        assert_eq!(active_transcript_sequence(&transcript, 15.0), None);
    }

    #[test]
    fn media_time_uses_compact_clock_format() {
        assert_eq!(format_media_time(65.0), "1:05");
        assert_eq!(format_media_time(3661.0), "1:01:01");
    }

    #[test]
    fn transcript_search_matches_speaker_and_content_case_insensitively() {
        let transcript = timed_transcript(
            Some("2026-08-31T12:00:00Z"),
            vec![
                segment(1, "2026-08-31T12:00:05Z", None),
                segment(2, "2026-08-31T12:00:10Z", None),
            ],
        );
        assert_eq!(transcript_match_indices(&transcript, "SEGMENT 2"), vec![1]);
        assert_eq!(transcript_match_indices(&transcript, "speaker-1"), vec![0]);
        assert!(transcript_match_indices(&transcript, "   ").is_empty());
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use cocoa::{
        base::{id, nil},
        foundation::NSString,
        quartzcore::current_media_time,
    };
    use core_foundation::base::TCFType;
    use core_video::pixel_buffer::{CVPixelBuffer, CVPixelBufferRef};
    use objc::{Encode, Encoding, msg_send, rc::StrongPtr, runtime::Class, sel, sel_impl};
    use std::ptr;

    pub(super) type VideoFrame = CVPixelBuffer;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CMTime {
        value: i64,
        timescale: i32,
        flags: u32,
        epoch: i64,
    }

    unsafe impl Encode for CMTime {
        fn encode() -> Encoding {
            unsafe { Encoding::from_str("{CMTime=qiIq}") }
        }
    }

    pub(super) struct NativeVideoPlayback {
        player: StrongPtr,
        output: StrongPtr,
    }

    impl NativeVideoPlayback {
        pub(super) fn new(url: &str) -> Result<Self, String> {
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                return Err("Video URL must use HTTP or HTTPS".into());
            }
            unsafe {
                let ns_url_class = Class::get("NSURL")
                    .ok_or_else(|| "Foundation URL support is unavailable".to_string())?;
                let item_class = Class::get("AVPlayerItem")
                    .ok_or_else(|| "AVFoundation player items are unavailable".to_string())?;
                let output_class = Class::get("AVPlayerItemVideoOutput")
                    .ok_or_else(|| "AVFoundation video output is unavailable".to_string())?;
                let player_class = Class::get("AVPlayer")
                    .ok_or_else(|| "AVFoundation playback is unavailable".to_string())?;

                let string = NSString::alloc(nil).init_str(url);
                let _string = StrongPtr::new(string);
                let ns_url: id = msg_send![ns_url_class, URLWithString: string];
                if ns_url == nil {
                    return Err("Video URL is invalid".into());
                }
                let item: id = msg_send![item_class, playerItemWithURL: ns_url];
                if item == nil {
                    return Err("Video item could not be created".into());
                }
                let output: id = msg_send![output_class, alloc];
                let output: id = msg_send![output, initWithPixelBufferAttributes: nil];
                if output == nil {
                    return Err("Video decoder output could not be created".into());
                }
                let output = StrongPtr::new(output);
                let _: () = msg_send![item, addOutput: *output];
                let player: id = msg_send![player_class, playerWithPlayerItem: item];
                if player == nil {
                    return Err("Video player could not be created".into());
                }
                Ok(Self {
                    player: StrongPtr::retain(player),
                    output,
                })
            }
        }

        pub(super) fn play(&self) {
            unsafe {
                let _: () = msg_send![*self.player, play];
            }
        }

        pub(super) fn pause(&self) {
            unsafe {
                let _: () = msg_send![*self.player, pause];
            }
        }

        pub(super) fn set_muted(&self, muted: bool) {
            unsafe {
                let _: () = msg_send![*self.player, setMuted: muted];
            }
        }

        pub(super) fn current_time_seconds(&self) -> f64 {
            unsafe {
                let time: CMTime = msg_send![*self.player, currentTime];
                cm_time_seconds(time).unwrap_or_default()
            }
        }

        pub(super) fn duration_seconds(&self) -> Option<f64> {
            unsafe {
                let item: id = msg_send![*self.player, currentItem];
                if item == nil {
                    return None;
                }
                let duration: CMTime = msg_send![item, duration];
                cm_time_seconds(duration)
            }
        }

        pub(super) fn seek_to(&self, seconds: f64) {
            let time = CMTime {
                value: (seconds.max(0.0) * 600.0).round() as i64,
                timescale: 600,
                flags: 1,
                epoch: 0,
            };
            unsafe {
                let _: () = msg_send![*self.player, seekToTime: time];
            }
        }

        pub(super) fn copy_new_frame(&self) -> Option<VideoFrame> {
            unsafe {
                let item_time: CMTime =
                    msg_send![*self.output, itemTimeForHostTime: current_media_time()];
                let available: bool =
                    msg_send![*self.output, hasNewPixelBufferForItemTime: item_time];
                if !available {
                    return None;
                }
                let buffer: CVPixelBufferRef = msg_send![
                    *self.output,
                    copyPixelBufferForItemTime: item_time
                    itemTimeForDisplay: ptr::null_mut::<CMTime>()
                ];
                if buffer.is_null() {
                    None
                } else {
                    Some(CVPixelBuffer::wrap_under_create_rule(buffer))
                }
            }
        }
    }

    fn cm_time_seconds(time: CMTime) -> Option<f64> {
        (time.timescale > 0)
            .then_some(time.value as f64 / time.timescale as f64)
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
    }

    impl Drop for NativeVideoPlayback {
        fn drop(&mut self) {
            self.pause();
        }
    }

    pub(super) fn surface(frame: VideoFrame) -> gpui::Surface {
        gpui::surface(frame)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::platform::NativeVideoPlayback;

    #[test]
    fn rejects_non_network_video_urls_before_opening_avfoundation() {
        assert!(NativeVideoPlayback::new("file:///tmp/video.mp4").is_err());
        assert!(NativeVideoPlayback::new("javascript:alert(1)").is_err());
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    #[derive(Clone)]
    pub(super) struct VideoFrame;

    pub(super) struct NativeVideoPlayback;

    impl NativeVideoPlayback {
        pub(super) fn new(_url: &str) -> Result<Self, String> {
            Err("Embedded video playback is currently available on macOS".into())
        }

        pub(super) fn play(&self) {}
        pub(super) fn pause(&self) {}
        pub(super) fn set_muted(&self, _muted: bool) {}
        pub(super) fn current_time_seconds(&self) -> f64 {
            0.0
        }
        pub(super) fn duration_seconds(&self) -> Option<f64> {
            None
        }
        pub(super) fn seek_to(&self, _seconds: f64) {}
        pub(super) fn copy_new_frame(&self) -> Option<VideoFrame> {
            None
        }
    }

    pub(super) fn surface(_frame: VideoFrame) -> gpui::Empty {
        gpui::Empty
    }
}
