use super::*;

pub(super) const DEFAULT_MODEL: &str = "eleven_v4";

pub(super) fn models(key: &str) -> Result<Vec<Voice>, String> {
    let bytes = http(
        client()?
            .get("https://api.elevenlabs.io/v1/models")
            .header("xi-api-key", key),
        key,
    )?;
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| "ElevenLabs 模型响应格式错误")?;
    Ok(data
        .as_array()
        .ok_or("ElevenLabs 未返回模型列表")?
        .iter()
        .filter(|m| m["can_do_text_to_speech"] == true)
        .filter_map(|m| {
            Some(Voice {
                id: m["model_id"].as_str().filter(|id| valid_voice(id))?.into(),
                name: m["name"].as_str()?.into(),
            })
        })
        .collect())
}

pub(super) fn voices(key: &str) -> Result<Vec<Voice>, String> {
    let bytes = http(
        client()?
            .get("https://api.elevenlabs.io/v1/voices")
            .header("xi-api-key", key),
        key,
    )?;
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| "ElevenLabs 音色响应格式错误")?;
    Ok(data["voices"]
        .as_array()
        .ok_or("ElevenLabs 未返回音色列表")?
        .iter()
        .filter_map(|v| {
            Some(Voice {
                id: v["voice_id"].as_str().filter(|id| valid_voice(id))?.into(),
                name: v["name"].as_str()?.into(),
            })
        })
        .collect())
}
pub(super) fn synthesize(
    key: &str,
    model: &str,
    voice: &str,
    text: &str,
    speed: f64,
) -> Result<Vec<u8>, String> {
    if key.is_empty() {
        return Err("请先保存 ElevenLabs API 密钥".into());
    }
    http(
        client()?
            .post(format!(
                "https://api.elevenlabs.io/v1/text-to-speech/{voice}?output_format=mp3_44100_128"
            ))
            .header("xi-api-key", key)
            .header("Content-Type", "application/json")
            .body(
                json!({"text":text,"model_id":model,"voice_settings":{"speed":speed}}).to_string(),
            ),
        key,
    )
}
