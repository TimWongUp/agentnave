use super::*;

pub(super) const DEFAULT_MODEL: &str = "speech-2.8-turbo";
const MODELS: [(&str, &str); 6] = [
    ("speech-2.8-turbo", "Speech 2.8 Turbo"),
    ("speech-2.8-hd", "Speech 2.8 HD"),
    ("speech-2.6-turbo", "Speech 2.6 Turbo"),
    ("speech-2.6-hd", "Speech 2.6 HD"),
    ("speech-02-turbo", "Speech 02 Turbo"),
    ("speech-02-hd", "Speech 02 HD"),
];

pub(super) fn models() -> Vec<Voice> {
    MODELS
        .iter()
        .map(|(id, name)| Voice {
            id: (*id).into(),
            name: (*name).into(),
        })
        .collect()
}
fn base(region: &str) -> &'static str {
    if region == "global" {
        "https://api.minimax.io"
    } else {
        "https://api.minimaxi.com"
    }
}
fn call(key: &str, region: &str, path: &str, body: Value) -> Result<Value, String> {
    if key.is_empty() {
        return Err("请先保存 MiniMax API 密钥".into());
    }
    let value = http_json(
        client()?
            .post(format!("{}{path}", base(region)))
            .bearer_auth(key),
        &body,
        key,
    )?;
    check(&value).map_err(|error| error.replace(key, "[密钥]"))?;
    Ok(value)
}
pub(super) fn check(value: &Value) -> Result<(), String> {
    match value["base_resp"]["status_code"].as_i64() {
        Some(0) => Ok(()),
        code => Err(format!(
            "MiniMax 语音错误（{}）：{}",
            code.map_or("未知".into(), |n| n.to_string()),
            value["base_resp"]["status_msg"]
                .as_str()
                .unwrap_or("响应格式错误")
        )),
    }
}
pub(super) fn decode(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
        return Err("MiniMax 音频十六进制格式错误".into());
    }
    hex.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = (pair[0] as char)
                .to_digit(16)
                .ok_or("MiniMax 音频十六进制格式错误")?;
            let lo = (pair[1] as char)
                .to_digit(16)
                .ok_or("MiniMax 音频十六进制格式错误")?;
            Ok((hi * 16 + lo) as u8)
        })
        .collect()
}
pub(super) fn voices(key: &str, region: &str) -> Result<Vec<Voice>, String> {
    let data = call(key, region, "/v1/get_voice", json!({"voice_type":"all"}))?;
    Ok(["system_voice", "voice_cloning", "voice_generation"]
        .iter()
        .flat_map(|kind| data[kind].as_array().into_iter().flatten())
        .filter_map(|v| {
            Some(Voice {
                id: v["voice_id"].as_str().filter(|id| valid_voice(id))?.into(),
                name: v["voice_name"].as_str()?.into(),
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
    region: &str,
) -> Result<Vec<u8>, String> {
    let data = call(
        key,
        region,
        "/v1/t2a_v2",
        json!({"model":model,"text":text,"stream":false,"output_format":"hex","language_boost":"auto","voice_setting":{"voice_id":voice,"speed":speed,"vol":1,"pitch":0},"audio_setting":{"sample_rate":32000,"bitrate":128000,"format":"mp3","channel":1}}),
    )?;
    decode(data["data"]["audio"].as_str().ok_or("MiniMax 未返回音频")?)
}
