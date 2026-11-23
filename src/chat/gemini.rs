use std::time::Duration;

use reqwest::{StatusCode, blocking::Client, header::HeaderValue, redirect::Policy};
use serde_json::{Value, json};

#[cfg(test)]
use super::Turn;
use super::{Context, credentials};
use crate::{actions, app::PanelAction, usage};

pub const MODEL_NAME: &str = "Gemini 3.5 Flash-Lite";
const ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.5-flash-lite:generateContent";
const CONTEXT_TURNS: usize = 24;
const MAX_TOOL_ROUNDS: usize = 6;

pub struct Gemini(Client);

impl Gemini {
    pub fn new() -> reqwest::Result<Self> {
        Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(Policy::none())
            .build()
            .map(Self)
    }

    #[cfg(test)]
    pub fn reply(
        &self,
        history: &[Turn],
        prompt: &str,
        navigate: impl FnMut(PanelAction),
    ) -> Result<String, &'static str> {
        self.reply_with(
            &Context {
                recent: history.to_vec(),
                ..Context::default()
            },
            prompt,
            navigate,
        )
    }

    pub fn reply_with(
        &self,
        context: &Context,
        prompt: &str,
        mut navigate: impl FnMut(PanelAction),
    ) -> Result<String, &'static str> {
        let key = credentials::load()?;
        let mut authorization =
            HeaderValue::from_str(&key).map_err(|_| "The Gemini API key has an invalid format.")?;
        authorization.set_sensitive(true);

        let db = (!context.private).then(|| actions::open().ok()).flatten();
        let instruction = context.guidance(
            "You are Amica, a personal assistant. Speak Filipino, English, or Taglish naturally. Favor clear plain text.",
            db.as_ref(), Some(prompt),
        );
        let history = &context.recent;
        let mut contents = Vec::new();
        for turn in &history[history.len().saturating_sub(CONTEXT_TURNS)..] {
            contents.push(text_content("user", &turn.user));
            contents.push(text_content("model", &turn.assistant));
        }
        contents.push(text_content("user", prompt));
        let mut body = json!({
            "contents": contents,
            "systemInstruction": { "parts": [{"text": instruction}] },
            "generationConfig": {"maxOutputTokens": 4096},
        });
        if !context.private {
            body["tools"] = actions::tools(false);
        }
        let mut completed = Vec::new();
        let mut calls_by_id = std::collections::HashMap::new();
        for round in 0..=MAX_TOOL_ROUNDS {
            let model = match self.generate(&authorization, &body) {
                Ok(model) => model,
                Err(error) => return local_confirmation(completed, error),
            };
            let Some(parts) = model["parts"].as_array() else {
                return local_confirmation(completed, "Gemini returned an unreadable response.");
            };
            let mut calls = parts
                .iter()
                .filter_map(|part| part.get("functionCall"))
                .peekable();
            if calls.peek().is_none() {
                let text = parts
                    .iter()
                    .filter(|part| part["thought"].as_bool() != Some(true))
                    .filter_map(|part| part["text"].as_str())
                    .collect::<String>();
                return if text.trim().is_empty() {
                    local_confirmation(
                        completed,
                        "Gemini returned no reply. Try rephrasing your message.",
                    )
                } else {
                    Ok(text)
                };
            }
            if context.private || round == MAX_TOOL_ROUNDS {
                break;
            }

            let mut responses = Vec::new();
            for raw in calls {
                let Ok(call) = serde_json::from_value::<actions::Call>(raw.clone()) else {
                    return local_confirmation(
                        completed,
                        "Gemini returned an unreadable local action.",
                    );
                };
                let response =
                    if let Some(previous) = call.id.as_ref().and_then(|id| calls_by_id.get(id)) {
                        previous
                    } else {
                        let response = call.for_request(db.as_ref(), prompt, &mut navigate);
                        if let Some(summary) = call.confirmation(&response) {
                            completed.push(summary);
                        }
                        if let Some(id) = call.id {
                            calls_by_id.insert(id, response.clone());
                        }
                        responses.push(json!({"functionResponse": response}));
                        continue;
                    };
                responses.push(json!({"functionResponse": response}));
            }
            let contents = body["contents"].as_array_mut().unwrap();
            contents.push(model);
            contents.push(json!({"role": "user", "parts": responses}));
        }
        local_confirmation(
            completed,
            "Gemini requested too many local actions to complete this message.",
        )
    }

    fn generate(&self, authorization: &HeaderValue, body: &Value) -> Result<Value, &'static str> {
        let request = self
            .0
            .post(ENDPOINT)
            .header("x-goog-api-key", authorization.clone());
        let response = usage::send_json(request, body).map_err(|error| {
            if error.is_timeout() {
                "Gemini took too long to reply. Send your message again to retry."
            } else {
                "Amica could not reach Gemini. Check your connection and retry."
            }
        })?;

        match response.status() {
            StatusCode::OK => usage::read_json(response)
                .ok_or("Gemini returned an unreadable response.")
                .and_then(|response| {
                    response
                        .pointer("/candidates/0/content")
                        .cloned()
                        .ok_or("Gemini returned no reply. Try rephrasing your message.")
                }),
            StatusCode::BAD_REQUEST => {
                Err("Gemini rejected the request. Check your API key or try a shorter message.")
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(
                "Gemini rejected the API key. Check the key and project access in Google AI Studio.",
            ),
            StatusCode::NOT_FOUND => {
                Err("This Gemini model is currently unavailable to your project.")
            }
            StatusCode::TOO_MANY_REQUESTS => Err(
                "Gemini returned a rate or quota limit. Check your project’s free-tier usage or try again later.",
            ),
            status if status.is_server_error() => {
                Err("Gemini is temporarily unavailable. Send your message again to retry.")
            }
            _ => Err("Gemini could not complete this message. Send it again to retry."),
        }
    }
}

fn text_content(role: &str, text: &str) -> Value {
    json!({ "role": role, "parts": [{ "text": text }] })
}

fn local_confirmation(completed: Vec<String>, error: &'static str) -> Result<String, &'static str> {
    if completed.is_empty() {
        Err(error)
    } else {
        Ok(format!(
            "{}\nGemini couldn’t finish the reply.",
            completed.join("\n")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Panel, local_time, memory, timers};

    #[test]
    #[ignore = "Uses Gemini and an isolated AMICA_DATA_DIR"]
    fn typed_chat_executes_local_actions() {
        assert!(
            std::env::var_os("AMICA_DATA_DIR").is_some(),
            "Set an isolated data directory before running this test"
        );
        let gemini = Gemini::new().unwrap();
        let db = actions::open().unwrap();
        let mut panels = Vec::new();
        let reply = gemini.reply(&[], "Use your local tools. Tell me the system's local time, start a 10-minute timer named QA Tea, save a note named QA Flower with exactly the content 'sampaguita', and save a preference titled QA Style with the value 'short answers'. Handle all four requests.", |panel| panels.push(panel)).unwrap();
        assert!(!reply.trim().is_empty());
        assert!(
            panels.contains(&PanelAction::Show(Panel::TimeDate))
                && panels.contains(&PanelAction::Show(Panel::Saved))
        );
        let now = local_time::now_ms();
        assert!(
            timers::running(&db, now)
                .unwrap()
                .iter()
                .any(|timer| timer.label.eq_ignore_ascii_case("QA Tea"))
        );
        assert!(
            memory::find(&db, Some("note"), "QA Flower", 5)
                .unwrap()
                .iter()
                .any(|item| item.value == "sampaguita")
        );
        assert!(
            memory::find(&db, Some("preference"), "QA Style", 5)
                .unwrap()
                .iter()
                .any(|item| item.value.eq_ignore_ascii_case("short answers"))
        );
        let recalled = gemini
            .reply(
                &[],
                "Look up my saved note titled QA Flower and tell me its exact contents.",
                |_| {},
            )
            .unwrap();
        assert!(
            recalled.to_lowercase().contains("sampaguita"),
            "Gemini did not retrieve the note"
        );
        gemini
            .reply(&[], "Close the Time & Date tab.", |action| {
                panels.push(action)
            })
            .unwrap();
        assert!(panels.contains(&PanelAction::Close(Panel::TimeDate)));
    }
}
