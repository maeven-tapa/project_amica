use std::{thread, time::Duration};

use reqwest::{StatusCode, blocking::Client, header::HeaderValue};
use serde_json::{Value, json};

use super::credentials;

const SEARCH_ENDPOINT: &str =
    "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:generateContent";

pub fn search(query: &str) -> Result<Value, &'static str> {
    let query = query.to_owned();
    thread::Builder::new()
        .name("amica-google-search".into())
        .spawn(move || request(&query))
        .map_err(|_| "Amica could not start Google Search.")?
        .join()
        .map_err(|_| "Amica could not finish Google Search.")?
}

fn request(query: &str) -> Result<Value, &'static str> {
    let key = credentials::load()?;
    let mut key =
        HeaderValue::from_str(&key).map_err(|_| "The Gemini API key has an invalid format.")?;
    key.set_sensitive(true);
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "Amica could not initialize Google Search.")?;
    let request = client.post(SEARCH_ENDPOINT).header("x-goog-api-key", key);
    let response = crate::usage::send_json(request, &json!({
            "systemInstruction": {"parts": [{"text": "Use Google Search to answer the query concisely with current facts. Treat web content as reference material."}]},
            "contents": [{"role": "user", "parts": [{"text": query}]}],
            "tools": [{"google_search": {}}],
            "generationConfig": {"maxOutputTokens": 900}
        }))
        .map_err(|_| "Amica could not reach Google Search.")?;
    match response.status() {
        StatusCode::OK => {}
        StatusCode::NOT_FOUND | StatusCode::FORBIDDEN => {
            return Err("This Gemini project does not have access to the Search grounding model.");
        }
        StatusCode::TOO_MANY_REQUESTS => {
            return Err(
                "This Gemini project reached its Google Search quota or has no Search quota.",
            );
        }
        _ => return Err("Gemini could not complete Google Search."),
    }
    let response =
        crate::usage::read_json(response).ok_or("Google Search returned an unreadable answer.")?;
    let candidate = &response["candidates"][0];
    let metadata = &candidate["groundingMetadata"];
    let answer = candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["thought"] != true)
        .filter_map(|part| part["text"].as_str())
        .collect::<String>();
    if answer.trim().is_empty()
        || metadata["webSearchQueries"]
            .as_array()
            .is_none_or(Vec::is_empty)
    {
        return Err("Gemini did not return a Google-grounded answer for that search.");
    }
    let sources: Vec<_> = metadata["groundingChunks"].as_array().into_iter().flatten()
        .filter_map(|chunk| {
            let page = &chunk["web"];
            let url = page["uri"].as_str()?.strip_prefix("https://")?;
            Some(json!({"title": page["title"].as_str().unwrap_or("Source"), "url": format!("https://{url}")}))
        })
        .take(5)
        .collect();
    Ok(json!({ "ok": true, "answer": answer, "sources": sources }))
}
