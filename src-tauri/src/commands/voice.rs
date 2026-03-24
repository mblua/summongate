use base64::Engine;
use tauri::State;

use crate::config::settings::SettingsState;

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    text: Option<String>,
}

#[tauri::command]
pub async fn voice_transcribe(
    settings: State<'_, SettingsState>,
    audio: Vec<u8>,
    mime_type: String,
) -> Result<String, String> {
    let cfg = settings.read().await;
    let api_key = cfg.gemini_api_key.clone();
    let model = cfg.gemini_model.clone();
    drop(cfg);

    let model = if model.is_empty() { "gemini-2.5-flash".to_string() } else { model };

    if api_key.is_empty() {
        return Err("Gemini API key not configured".to_string());
    }

    if audio.is_empty() {
        return Err("No audio data provided".to_string());
    }

    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio);

    let body = serde_json::json!({
        "contents": [{
            "parts": [
                { "text": "Transcribe this audio exactly as spoken. Return only the transcribed text, nothing else." },
                {
                    "inlineData": {
                        "mimeType": mime_type,
                        "data": audio_b64
                    }
                }
            ]
        }]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Gemini API request failed: {}", e))?;

    let status = resp.status();
    if !status.is_success() {
        let error_body = resp.text().await.unwrap_or_default();
        let msg = if status.as_u16() == 429 {
            "Rate limit exceeded - try again in a few seconds".to_string()
        } else if status.as_u16() == 403 {
            "Invalid API key or access denied".to_string()
        } else {
            format!("Gemini API error ({}): {}", status, error_body)
        };
        log::warn!("Gemini API {} - {}", status, error_body);
        return Err(msg);
    }

    let gemini_resp: GeminiResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse Gemini response: {}", e))?;

    let text = gemini_resp
        .candidates
        .and_then(|c| c.into_iter().next())
        .map(|c| c.content)
        .and_then(|content| {
            content
                .parts
                .into_iter()
                .filter_map(|p| p.text)
                .next()
        })
        .unwrap_or_default()
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("Transcription returned empty text".to_string());
    }

    log::info!("Voice transcription: {} chars", text.len());
    Ok(text)
}
