use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use aec3::{nodes::audio::AudioFormat, pipelines::linear};
use base64::{Engine, engine::general_purpose::STANDARD};
use cpal::Sample;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    net::TcpStream,
    sync::mpsc,
    time::{self, MissedTickBehavior},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Error, Message, client::IntoClientRequest, http::HeaderValue},
};

use super::{activity::SpeechActivity, playback::Playback};
use crate::{
    VoiceState, actions,
    app::PanelAction,
    chat::{Context, Turn, gemini_key},
    microphone::{
        AudioTap,
        pcm::{SPEECH_RATE, SpeechFrame},
    },
};

const ENDPOINT: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
const MODEL: &str = "models/gemini-3.8-live";
pub(super) const MODEL_NAME: &str = "Gemini 3.8 Live";
const VOICE: &str = "Aoede";
const DISCONNECTED: &str = "Gemini Live disconnected. Unmute to reconnect.";
const ECHO_ERROR: &str = "Amica could not process speaker echo. Unmute to retry.";

pub enum Event {
    Phase(VoiceState),
    Caption(String),
    Recovery(String),
    SpeechLevel(f32),
    Turn(Turn),
    Navigate(PanelAction),
}

struct Connection(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Connection {
    #[cfg(test)]
    async fn open(
        history: &[Turn],
        db: Option<&rusqlite::Connection>,
    ) -> Result<Self, &'static str> {
        Self::with_context(
            &Context {
                recent: history.to_vec(),
                ..Context::default()
            },
            db,
        )
        .await
    }

    async fn with_context(
        context: &Context,
        db: Option<&rusqlite::Connection>,
    ) -> Result<Self, &'static str> {
        let mut request = ENDPOINT.into_client_request().map_err(|_| DISCONNECTED)?;
        let mut key = HeaderValue::from_str(&gemini_key()?)
            .map_err(|_| "The Gemini API key has an invalid format.")?;
        key.set_sensitive(true);
        request.headers_mut().insert("x-goog-api-key", key);
        let (socket, _) = time::timeout(
            Duration::from_secs(15),
            connect_async_with_config(request, None, true),
        )
        .await
        .map_err(|_| "Connecting to Gemini Live timed out.")?
        .map_err(|error| match error {
            Error::Http(response) if matches!(response.status().as_u16(), 401 | 403) => {
                "Gemini Live rejected the API key. Check your project in Google AI Studio."
            }
            Error::Http(response) if response.status().as_u16() == 429 => {
                "Gemini Live reached the project’s rate or quota limit. Try again later."
            }
            _ => "Amica could not connect to Gemini Live. Check your connection and try again.",
        })?;
        let mut connection = Self(socket);

        let instruction = context.guidance(
            "You are Amica, a warm, concise personal assistant. Understand natural Filipino (Tagalog), English, and Taglish. Reply in the user's language, including Taglish when they mix both, unless they ask for another language. For Filipino replies, speak clearly and naturally as spoken in the Philippines.",
            db, None,
        );
        let history = &context.recent[context.recent.len().saturating_sub(12)..];
        let mut setup = json!({
            "model": MODEL,
            "generationConfig": {
                "responseModalities": ["AUDIO"],
                "speechConfig": { "voiceConfig": { "prebuiltVoiceConfig": { "voiceName": VOICE } } }
            },
            "systemInstruction": { "parts": [{ "text": instruction }] },
            "inputAudioTranscription": {},
            "outputAudioTranscription": {},
            "realtimeInputConfig": {
                "automaticActivityDetection": { "prefixPaddingMs": 150, "silenceDurationMs": 450 },
                "activityHandling": "START_OF_ACTIVITY_INTERRUPTS"
            }
        });
        if !context.private {
            setup["tools"] = actions::tools(true);
        }
        if !history.is_empty() {
            setup["historyConfig"] = json!({"initialHistoryInClientContent": true});
        }
        connection.send(json!({ "setup": setup })).await?;
        time::timeout(Duration::from_secs(15), async {
            loop {
                if connection.receive().await?.setup_complete.is_some() {
                    return Ok::<_, &'static str>(());
                }
            }
        })
        .await
        .map_err(|_| "Gemini Live did not finish starting. Try again.")??;

        if !history.is_empty() {
            let turns: Vec<_> = history
                .iter()
                .flat_map(|turn| {
                    [
                        json!({ "role": "user", "parts": [{ "text": turn.user }] }),
                        json!({ "role": "model", "parts": [{ "text": turn.assistant }] }),
                    ]
                })
                .collect();
            connection
                .send(json!({ "clientContent": { "turns": turns, "turnComplete": true } }))
                .await?;
        }
        Ok(connection)
    }

    async fn send(&mut self, value: Value) -> Result<(), &'static str> {
        let text = value.to_string();
        let bytes = text.len();
        time::timeout(
            Duration::from_secs(5),
            self.0.send(Message::Text(text.into())),
        )
        .await
        .map_err(|_| "Sending audio to Gemini Live timed out.")?
        .map_err(|_| DISCONNECTED)?;
        crate::usage::sent(bytes);
        Ok(())
    }

    async fn send_audio(&mut self, pcm: &[u8]) -> Result<(), &'static str> {
        self.send(json!({ "realtimeInput": { "audio": { "data": STANDARD.encode(pcm), "mimeType": "audio/pcm;rate=16000" } } })).await
    }

    async fn answer_tools(
        &mut self,
        request: ToolRequest,
        db: Option<&rusqlite::Connection>,
        responses_by_id: &mut HashMap<String, Value>,
        current_request: Option<&str>,
        mut navigate: impl FnMut(PanelAction),
    ) -> Result<(), &'static str> {
        let responses: Vec<_> = request
            .function_calls
            .into_iter()
            .filter_map(|call| {
                let id = call.id.clone()?;
                Some(
                    responses_by_id
                        .entry(id)
                        .or_insert_with(|| {
                            if let Some(request) = current_request {
                                call.for_request(db, request, &mut navigate)
                            } else {
                                call.response(db, &mut navigate)
                            }
                        })
                        .clone(),
                )
            })
            .collect();
        if !responses.is_empty() {
            self.send(json!({"toolResponse": {"functionResponses": responses}}))
                .await?;
        }
        Ok(())
    }

    async fn receive(&mut self) -> Result<ServerMessage, &'static str> {
        loop {
            let frame = self
                .0
                .next()
                .await
                .ok_or(DISCONNECTED)?
                .map_err(|_| DISCONNECTED)?;
            let data = match frame {
                Message::Text(text) => text.into(),
                Message::Binary(data) => data,
                Message::Close(close) => {
                    return Err(close
                        .as_ref()
                        .map_or(DISCONNECTED, |close| error_message(&close.reason)));
                }
                Message::Ping(_) => {
                    self.0.flush().await.map_err(|_| DISCONNECTED)?;
                    continue;
                }
                _ => continue,
            };
            crate::usage::received(data.len());
            let message: ServerMessage = serde_json::from_slice(&data)
                .map_err(|_| "Gemini Live returned an unreadable message.")?;
            if let Some(error) = message.error {
                return Err(error_message(&error.message));
            }
            return Ok(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Panel, microphone::pcm::Resampler, voice::playback::REPLY_RATE};

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Requires a Gemini API key and uses the Live API"]
    async fn system_usage_requests_choose_the_pc_or_amica() {
        let mut socket = Connection::open(&[], None).await.unwrap();
        let mut responses = HashMap::new();
        for (prompt, expected_scope) in [
            ("What's the system usage?", "whole_pc"),
            ("What's your system usage?", "amica_process"),
        ] {
            socket
                .send(json!({"realtimeInput": {"text": prompt}}))
                .await
                .unwrap();
            let report = time::timeout(Duration::from_secs(35), async {
                let mut report = None;
                loop {
                    let packet = socket.receive().await.unwrap();
                    if let Some(request) = packet.tool_call {
                        let id = request
                            .function_calls
                            .iter()
                            .find(|call| call.name == "get_system_usage")
                            .and_then(|call| call.id.clone());
                        socket
                            .answer_tools(request, None, &mut responses, None, |_| {})
                            .await
                            .unwrap();
                        if let Some(id) = id {
                            report = responses.get(&id).map(|reply| reply["response"].clone());
                        }
                    }
                    if packet
                        .server_content
                        .is_some_and(|content| content.turn_complete)
                    {
                        break report;
                    }
                }
            })
            .await
            .expect("Gemini Live did not finish the usage request")
            .expect("Gemini Live did not request usage data");
            assert_eq!(report["scope"], expected_scope);
            assert!(report["cpu_percent_of_pc_capacity"].is_number());
            assert!(report["memory_used_mib"].is_number());
            assert!(report["download_mbps"].is_number());
            assert!(report["upload_mbps"].is_number());
        }
    }

    #[tokio::test]
    #[ignore = "Requires a Gemini API key and uses the Live API"]
    async fn gemini_live_opens_and_closes_tabs_on_natural_requests() {
        let mut socket = Connection::open(&[], None).await.unwrap();
        let mut responses = HashMap::new();
        for (prompt, expected) in [
            (
                "What are the conversations we have had?",
                PanelAction::Show(Panel::Conversation),
            ),
            (
                "Isara mo yung Conversation tab.",
                PanelAction::Close(Panel::Conversation),
            ),
            (
                "Play something in Spotify.",
                PanelAction::PlayMedia(
                    crate::media::PlayRequest::new(crate::media::Service::Spotify, "").unwrap(),
                ),
            ),
        ] {
            socket
                .send(json!({"realtimeInput": {"text": prompt}}))
                .await
                .unwrap();
            let actions = time::timeout(Duration::from_secs(35), async {
                let mut actions = Vec::new();
                loop {
                    let packet = socket.receive().await.unwrap();
                    if let Some(call) = packet.tool_call {
                        socket
                            .answer_tools(call, None, &mut responses, None, |action| {
                                actions.push(action)
                            })
                            .await
                            .unwrap();
                    }
                    if packet
                        .server_content
                        .is_some_and(|content| content.turn_complete)
                    {
                        break actions;
                    }
                }
            })
            .await
            .expect("Gemini Live did not finish the navigation request");
            let local = actions::spoken_panel(prompt)
                .or_else(|| crate::media::spoken_request(prompt).map(PanelAction::PlayMedia));
            assert!(
                actions.contains(&expected) || local == Some(expected),
                "Received {actions:?} for {prompt:?}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "Requires a Gemini API key and uses the Live API"]
    async fn gemini_live_can_read_the_clock_and_start_a_local_timer() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        crate::timers::initialize(&db).unwrap();
        crate::memory::initialize(&db).unwrap();
        let mut connection = Connection::open(&[], Some(&db)).await.unwrap();
        connection.send(json!({"realtimeInput": {"text": "Gamitin ang tools mo para tingnan ang local time ngayon at mag-set ng 10-minute timer na ang eksaktong pangalan ay QA Tea. Sabihin mo ang resulta sa Filipino."}})).await.unwrap();
        let (clock, audio, panels) = time::timeout(Duration::from_secs(45), async {
            let mut responses = HashMap::new();
            let mut clock = false;
            let mut audio = false;
            let mut panels = Vec::new();
            loop {
                let packet = connection.receive().await.unwrap();
                if let Some(call) = packet.tool_call {
                    clock |= call
                        .function_calls
                        .iter()
                        .any(|call| call.name == "get_local_time");
                    connection
                        .answer_tools(call, Some(&db), &mut responses, None, |panel| {
                            panels.push(panel)
                        })
                        .await
                        .unwrap();
                }
                if let Some(content) = packet.server_content {
                    audio |= content.model_turn.is_some_and(|turn| {
                        turn.parts.iter().any(|part| part.inline_data.is_some())
                    });
                    if content.turn_complete {
                        break (clock, audio, panels);
                    }
                }
            }
        })
        .await
        .expect("Gemini Live did not finish the tool test");
        assert!(clock && audio);
        assert!(panels.contains(&PanelAction::Show(Panel::TimeDate)));
        let timers = crate::timers::running(&db, crate::local_time::now_ms()).unwrap();
        assert!(
            timers
                .iter()
                .any(|timer| timer.label.eq_ignore_ascii_case("QA Tea"))
        );
    }

    #[test]
    fn echo_filter_preserves_speech_without_a_speaker_signal() {
        let mut filter = echo_filter().unwrap();
        let mut audible = false;
        for index in 0..30 {
            let mic = std::array::from_fn::<_, 160, _>(|sample| {
                ((index * 160 + sample) as f32 * std::f32::consts::TAU * 440.0 / SPEECH_RATE as f32)
                    .sin()
                    * 0.12
            });
            let mut clean = [0.0; 160];
            filter.handle_render_frame(&[0.0; 160]).unwrap();
            if filter.process_capture_frame(&mic, &mut clean).unwrap() {
                assert!(
                    clean
                        .iter()
                        .all(|value| value.is_finite() && value.abs() <= 1.0)
                );
                audible |= clean.iter().any(|value| value.abs() > 0.015);
            }
        }
        assert!(audible);
    }

    async fn collect_turn(connection: &mut Connection) -> (String, String, Vec<u8>) {
        time::timeout(Duration::from_secs(40), async {
            let mut user = String::new();
            let mut answer = String::new();
            let mut audio = Vec::new();
            loop {
                if let Some(content) = connection.receive().await.unwrap().server_content {
                    if let Some(text) = content.input_transcription {
                        user.push_str(&text.text);
                    }
                    if let Some(text) = content.output_transcription {
                        answer.push_str(&text.text);
                    }
                    if let Some(turn) = content.model_turn {
                        for part in turn.parts {
                            if let Some(data) = part.inline_data {
                                audio.extend(STANDARD.decode(data.data).unwrap());
                            }
                        }
                    }
                    if content.turn_complete {
                        break (user, answer, audio);
                    }
                }
            }
        })
        .await
        .expect("Gemini Live did not complete the test turn")
    }

    #[tokio::test]
    #[ignore = "Requires a Gemini API key and uses the Live API"]
    async fn gemini_live_filipino_round_trip() {
        let mut speech_source = Connection::open(&[], None).await.unwrap();
        speech_source.send(json!({ "realtimeInput": { "text": "Basahin lang ang pangungusap na ito nang eksakto, huwag itong sagutin: Magkano ang lima dagdag sa pito? Sagutin mo sa Filipino." } })).await.unwrap();
        let (_, spoken_input, audio) = collect_turn(&mut speech_source).await;
        assert!(!audio.is_empty(), "Gemini Live did not return audio");
        assert!(
            spoken_input.to_lowercase().contains("pito"),
            "Expected a spoken Filipino question, got {spoken_input:?}"
        );
        drop(speech_source);

        let mut mic = Vec::new();
        let mut converter = Resampler::new(REPLY_RATE, SPEECH_RATE);
        for pair in audio.chunks_exact(2) {
            let sample = i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0;
            converter.push(sample, |sample| {
                mic.extend(sample.to_sample::<i16>().to_le_bytes())
            });
        }
        let mut conversation = Connection::open(
            &[Turn {
                user: "Mag-usap tayo sa Filipino.".into(),
                assistant: "Sige, mag-usap tayo sa Filipino.".into(),
            }],
            None,
        )
        .await
        .unwrap();
        for chunk in mic.chunks(640) {
            conversation.send_audio(chunk).await.unwrap();
            time::sleep(Duration::from_millis(20)).await;
        }
        conversation
            .send(json!({ "realtimeInput": { "audioStreamEnd": true } }))
            .await
            .unwrap();
        let (heard, reply, reply_audio) = collect_turn(&mut conversation).await;
        assert!(!reply_audio.is_empty(), "No spoken reply");
        let heard = heard.to_lowercase();
        let reply = reply.to_lowercase();
        assert!(
            heard.contains("pito") || heard.contains('7'),
            "Input transcript was {heard:?}"
        );
        assert!(
            reply.contains("labindalawa")
                || reply.contains("labing dalawa")
                || reply.contains("12"),
            "Reply was {reply:?}"
        );
        println!("Live Filipino input {heard:?}. Spoken reply transcript {reply:?}.");
    }

    #[tokio::test]
    #[ignore = "Uses Gemini and an isolated AMICA_DATA_DIR"]
    async fn spoken_preference_is_recalled_in_a_new_typed_chat() {
        assert!(std::env::var_os("AMICA_DATA_DIR").is_some());
        let mut reading = Connection::open(&[], None).await.unwrap();
        reading.send(json!({"realtimeInput": {"text": "Basahin nang eksakto ang pangungusap na ito lamang: Mas gusto kong tahimik kapag nag-aaral ako."}})).await.unwrap();
        let (_, _, audio) = collect_turn(&mut reading).await;
        assert!(!audio.is_empty());
        drop(reading);

        let mut samples = Vec::new();
        let mut converter = Resampler::new(REPLY_RATE, SPEECH_RATE);
        for pair in audio.chunks_exact(2) {
            let sample = i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0;
            converter.push(sample, |sample| {
                samples.extend(sample.to_sample::<i16>().to_le_bytes())
            });
        }
        let mut listening = Connection::open(&[], None).await.unwrap();
        for chunk in samples.chunks(640) {
            listening.send_audio(chunk).await.unwrap();
            time::sleep(Duration::from_millis(20)).await;
        }
        listening
            .send(json!({"realtimeInput": {"audioStreamEnd": true}}))
            .await
            .unwrap();
        let (heard, assistant, audio) = collect_turn(&mut listening).await;
        assert!(!audio.is_empty());
        drop(listening);

        let history = crate::chat::History::open().unwrap();
        let id = history.create("Spoken preference fixture").unwrap();
        let turn = history
            .append(
                id,
                &Turn {
                    user: heard.clone(),
                    assistant,
                },
                "voice",
                Some("qa-study-preference"),
            )
            .unwrap()
            .unwrap();
        let captured = crate::memory::learned::capture_epoch(history.db()).unwrap();
        assert!(
            crate::memory::learned::process(history.db(), turn, captured).unwrap(),
            "Transcript {heard:?}"
        );
        let node = crate::memory::learned::list(history.db(), "studying")
            .unwrap()
            .into_iter()
            .find(|node| node.category == "Preference")
            .unwrap();
        assert_eq!(node.sources[0].turn, turn);

        let chat = history.create("New typed study plan").unwrap();
        let node_id = node.id;
        drop(history);
        tokio::task::spawn_blocking(move || {
            let history = crate::chat::History::open().unwrap();
            assert!(history.turns(chat, 24).unwrap().is_empty());
            let prompt =
                "Make a short personalized study plan. Which study environment do I prefer?";
            let gemini = crate::chat::TestGemini::new().unwrap();
            let reply = gemini
                .reply_with(&Context::default(), prompt, |_| {})
                .unwrap()
                .to_lowercase();
            assert!(
                reply.contains("quiet") || reply.contains("silence") || reply.contains("tahimik"),
                "{reply}"
            );
            crate::memory::learned::edit(
                history.db(),
                node_id,
                "I prefer instrumental music when studying.",
            )
            .unwrap();
            let revised = gemini
                .reply_with(&Context::default(), prompt, |_| {})
                .unwrap()
                .to_lowercase();
            assert!(revised.contains("instrumental"), "{revised}");
            crate::memory::learned::forget(history.db(), node_id, false).unwrap();
            assert!(
                crate::memory::learned::recall(history.db(), prompt)
                    .unwrap()
                    .is_empty()
            );
        })
        .await
        .unwrap();
    }
}

fn echo_filter() -> Result<linear::LinearPipeline, &'static str> {
    let format = AudioFormat::ten_ms(SPEECH_RATE, 1);
    linear::builder(format, format)
        .initial_delay_ms(75)
        .enable_noise_suppression(false)
        .enable_gain_controller2(false)
        .enable_post_filter(false)
        .build()
        .map_err(|_| ECHO_ERROR)
}

fn error_message(reason: &str) -> &'static str {
    let reason = reason.to_ascii_lowercase();
    if reason.contains("quota")
        || reason.contains("resource exhausted")
        || reason.contains("rate limit")
    {
        "Gemini Live reached the project’s rate or quota limit. Try again later."
    } else if reason.contains("api key")
        || reason.contains("credentials")
        || reason.contains("permission")
    {
        "Gemini Live rejected the API key or project access. Check Google AI Studio."
    } else if reason.contains("model")
        && (reason.contains("not found") || reason.contains("not supported"))
    {
        "Gemini 3.8 Live is unavailable to this project. Check model access in Google AI Studio."
    } else {
        DISCONNECTED
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerMessage {
    setup_complete: Option<Value>,
    voice_activity: Option<VoiceActivity>,
    server_content: Option<ServerContent>,
    go_away: Option<Value>,
    tool_call: Option<ToolRequest>,
    error: Option<ServerError>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolRequest {
    function_calls: Vec<actions::Call>,
}

#[derive(Deserialize)]
struct ServerError {
    message: String,
}

#[derive(Deserialize)]
struct VoiceActivity {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ServerContent {
    input_transcription: Option<Transcript>,
    output_transcription: Option<Transcript>,
    model_turn: Option<ModelTurn>,
    interrupted: bool,
    turn_complete: bool,
    waiting_for_input: bool,
}

#[derive(Deserialize)]
struct Transcript {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ModelTurn {
    #[serde(default)]
    parts: Vec<Part>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Part {
    inline_data: Option<AudioData>,
}

#[derive(Deserialize)]
struct AudioData {
    data: String,
}

struct Conversation {
    phase: VoiceState,
    user_text: String,
    reply: String,
    generated: bool,
    interrupted: bool,
    tool_navigation: Option<PanelAction>,
    pending_actions: Vec<PanelAction>,
    ready_actions: Vec<PanelAction>,
    spoke: bool,
}

impl Conversation {
    fn navigate(&mut self, action: PanelAction, emit: &mut impl FnMut(Event)) {
        if action.after_reply() {
            if !self.pending_actions.contains(&action) {
                self.pending_actions.push(action);
            }
        } else {
            emit(Event::Navigate(action));
        }
    }

    fn release_actions(&mut self, playing: bool, emit: &mut impl FnMut(Event)) {
        if !playing {
            for action in self.ready_actions.drain(..) {
                emit(Event::Navigate(action));
            }
        }
    }

    fn set_phase(&mut self, phase: VoiceState, emit: &mut impl FnMut(Event)) {
        if self.phase != phase {
            self.phase = phase;
            emit(Event::Phase(phase));
        }
    }

    fn complete_turn(&mut self, emit: &mut impl FnMut(Event)) {
        let user = std::mem::take(&mut self.user_text);
        let assistant = std::mem::take(&mut self.reply);
        let tool_navigation = self.tool_navigation.take();
        if self.interrupted {
            self.pending_actions.clear();
        } else {
            if actions::exit_requested(&user) {
                self.pending_actions.clear();
                self.navigate(PanelAction::ExitAmica, emit);
            } else if let Some(command) = actions::catalog::parse(&user)
                .filter(|command| *command != actions::command::Command::Companion)
            {
                self.pending_actions.clear();
                let stop = command == actions::command::Command::Stop;
                let action = PanelAction::Local(command, Instant::now());
                if stop {
                    emit(Event::Navigate(action));
                } else {
                    self.navigate(action, emit);
                }
            } else {
                if let Some(action) = actions::spoken_panel(&user)
                    && tool_navigation.as_ref() != Some(&action)
                {
                    self.navigate(action, emit);
                }
                if !self
                    .pending_actions
                    .iter()
                    .any(|action| matches!(action, PanelAction::PlayMedia(_)))
                    && let Some(request) = crate::media::spoken_request(&user)
                {
                    self.navigate(PanelAction::PlayMedia(request), emit);
                }
                if self.spoke
                    && let Some(action) = actions::power_request(&user)
                {
                    self.pending_actions.push(action);
                }
            }
            self.ready_actions.append(&mut self.pending_actions);
            if !user.trim().is_empty() && !assistant.trim().is_empty() {
                emit(Event::Turn(Turn { user, assistant }));
            }
        }
        self.interrupted = false;
        self.spoke = false;
        self.generated = true;
    }

    fn receive(
        &mut self,
        message: ServerMessage,
        playback: &mut Playback,
        emit: &mut impl FnMut(Event),
    ) -> Result<(), &'static str> {
        if message
            .voice_activity
            .as_ref()
            .is_some_and(|activity| activity.kind == "ACTIVITY_END")
            && self.phase != VoiceState::Speaking
        {
            self.generated = false;
            self.set_phase(VoiceState::Thinking, emit);
        }
        let Some(content) = message.server_content else {
            return Ok(());
        };
        if content.interrupted {
            self.interrupted = true;
            self.pending_actions.clear();
            self.ready_actions.clear();
            playback.clear();
            emit(Event::Caption(String::new()));
            self.set_phase(VoiceState::Listening, emit);
        }
        if let Some(transcript) = content.input_transcription {
            self.user_text.push_str(&transcript.text);
            if !self.user_text.trim().is_empty() {
                emit(Event::Recovery(self.user_text.clone()));
            }
            if self.phase != VoiceState::Speaking && !transcript.text.trim().is_empty() {
                emit(Event::Caption(self.user_text.clone()));
            }
        }
        if let Some(transcript) = content.output_transcription {
            self.reply.push_str(&transcript.text);
            if !self.interrupted {
                emit(Event::Caption(self.reply.clone()));
            }
        }
        if let Some(turn) = content.model_turn
            && !self.interrupted
        {
            for part in turn.parts {
                if let Some(audio) = part.inline_data {
                    let data = STANDARD
                        .decode(audio.data)
                        .map_err(|_| "Gemini Live returned invalid audio.")?;
                    playback.enqueue(&data)?;
                    self.spoke |= !data.is_empty();
                    self.generated = false;
                    self.set_phase(VoiceState::Speaking, emit);
                }
            }
        }
        if content.turn_complete {
            self.complete_turn(emit);
        }
        if content.waiting_for_input && !playback.playing() {
            self.set_phase(VoiceState::Listening, emit);
        }
        Ok(())
    }
}

pub async fn session(
    tap: Arc<AudioTap>,
    ready: Arc<AtomicBool>,
    mut input: mpsc::Receiver<SpeechFrame>,
    speaker: &str,
    context: Context,
    mut emit: impl FnMut(Event),
) -> Result<(), &'static str> {
    let (render_tx, mut render_rx) = mpsc::channel(24);
    let mut playback = Playback::open(render_tx, speaker)?;
    let mut echo = echo_filter()?;
    let mut speech = SpeechActivity::new();
    let db = (!context.private).then(|| actions::open().ok()).flatten();
    let mut socket = Connection::with_context(&context, db.as_ref()).await?;

    let mut conversation = Conversation {
        phase: VoiceState::Listening,
        user_text: String::new(),
        reply: String::new(),
        generated: false,
        interrupted: false,
        tool_navigation: None,
        pending_actions: Vec::new(),
        ready_actions: Vec::new(),
        spoke: false,
    };
    let mut tick = time::interval(Duration::from_millis(40));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut audio = Vec::with_capacity(640);
    let mut go_away = false;
    let mut responses_by_id = HashMap::new();
    ready.store(true, Ordering::Release);
    emit(Event::Phase(VoiceState::Listening));

    loop {
        tokio::select! {
            packet = socket.receive() => {
                let mut packet = packet?;
                go_away |= packet.go_away.is_some();
                let tool_call = packet.tool_call.take();
                conversation.receive(packet, &mut playback, &mut emit)?;
                if let Some(request) = tool_call.filter(|_| !context.private) {
                    conversation.generated = false;
                    conversation.set_phase(VoiceState::Thinking, &mut emit);
                    let user_request = conversation.user_text.clone();
                    socket.answer_tools(request, db.as_ref(), &mut responses_by_id, Some(&user_request), |action| {
                        conversation.tool_navigation = Some(action.clone());
                        conversation.navigate(action, &mut emit);
                    }).await?;
                }
            }
            Some(frame) = render_rx.recv() => {
                echo.handle_render_frame(&frame).map_err(|_| ECHO_ERROR)?;
                speech.render(&frame);
            }
            Some(frame) = input.recv() => {
                if !tap.is_active() { continue; }
                while let Ok(render) = render_rx.try_recv() {
                    echo.handle_render_frame(&render).map_err(|_| ECHO_ERROR)?;
                    speech.render(&render);
                }
                let mut clean = [0.0; 160];
                let produced = echo.process_capture_frame(&frame, &mut clean).map_err(|_| ECHO_ERROR)?;
                let samples = if produced { clean } else { frame };
                speech.capture(&samples);
                audio.extend(samples.into_iter().flat_map(|sample| sample.to_sample::<i16>().to_le_bytes()));
                if audio.len() >= 640 {
                    socket.send_audio(&audio).await?;
                    audio.clear();
                }
            }
            _ = tick.tick() => {
                if playback.failed() { return Err("Amica lost the audio output device. Unmute to reconnect."); }
                let level = speech.take_level();
                if level > 0.0 { emit(Event::SpeechLevel(level)); }
                let playing = playback.playing();
                if conversation.generated && !playing {
                    conversation.generated = false;
                    conversation.set_phase(VoiceState::Listening, &mut emit);
                    emit(Event::Caption(String::new()));
                }
                conversation.release_actions(playing, &mut emit);
                if go_away && conversation.phase == VoiceState::Listening && !playing { return Err("Gemini Live ended this session. Unmute to reconnect."); }
            }
        }
    }
}

#[cfg(test)]
mod reply_action_tests {
    use super::*;
    use crate::{Panel, desktop::PowerAction};

    #[test]
    fn desktop_actions_wait_for_end_of_turn_and_playback() {
        let mut conversation = Conversation {
            phase: VoiceState::Speaking,
            user_text: String::new(),
            reply: String::new(),
            generated: false,
            interrupted: false,
            tool_navigation: None,
            pending_actions: vec![],
            ready_actions: vec![],
            spoke: true,
        };
        let commands = [
            PanelAction::Show(Panel::Youtube),
            PanelAction::Power(PowerAction::Restart),
        ];
        let mut events = Vec::new();
        let mut capture = |event| {
            if let Event::Navigate(action) = event {
                events.push(action)
            }
        };
        for command in &commands {
            conversation.navigate(command.clone(), &mut capture);
        }
        conversation.release_actions(false, &mut capture);
        conversation
            .ready_actions
            .append(&mut conversation.pending_actions);
        conversation.release_actions(true, &mut capture);
        assert!(events.is_empty());
        conversation.release_actions(false, &mut |event| {
            if let Event::Navigate(action) = event {
                events.push(action);
            }
        });
        assert_eq!(events, commands);
    }

    #[test]
    fn goodbye_commands_exit_only_after_the_farewell_drains() {
        for command in [
            "Close Amica",
            "Goodbye",
            "Terminate",
            "Exit Amica",
            "Isara ang Amica.",
        ] {
            let mut conversation = Conversation {
                phase: VoiceState::Speaking,
                user_text: command.into(),
                reply: "Goodbye.".into(),
                generated: false,
                interrupted: false,
                tool_navigation: None,
                pending_actions: vec![],
                ready_actions: vec![],
                spoke: true,
            };
            let mut events = Vec::new();
            conversation.complete_turn(&mut |event| events.push(event));
            conversation.release_actions(true, &mut |event| events.push(event));
            assert!(matches!(events.as_slice(), [Event::Turn(_)]));
            conversation.release_actions(false, &mut |event| events.push(event));
            assert!(matches!(
                events.as_slice(),
                [Event::Turn(_), Event::Navigate(PanelAction::ExitAmica)]
            ));
        }
        for phrase in [
            "Shut down this PC",
            "Don't terminate",
            "What does goodbye mean?",
        ] {
            assert!(!actions::exit_requested(phrase));
        }
    }
}
