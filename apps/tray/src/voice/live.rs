//! Explicit, opt-in cloud acceptance checks. Normal tests never contact speech providers.
use super::*;

const PHRASE: &str = "大家好，我先说一下我的看法。";

fn save(engine: &str, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err(format!("{engine} 未返回音频"));
    }
    let directory = std::env::var_os("AGENTNAVE_VOICE_LIVE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/voice-live"));
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join(format!("{engine}.mp3"));
    std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
    eprintln!("{engine}: {} ({} bytes)", path.display(), bytes.len());
    Ok(())
}

#[test]
#[ignore = "真实 ElevenLabs 合成，消耗额度，需要显式授权"]
fn elevenlabs_live() -> Result<(), String> {
    let Ok(key) = std::env::var("ELEVENLABS_API_KEY") else {
        eprintln!("ElevenLabs: 跳过（ELEVENLABS_API_KEY 不存在）");
        return Ok(());
    };
    if key.trim().is_empty() {
        eprintln!("ElevenLabs: 跳过（ELEVENLABS_API_KEY 为空）");
        return Ok(());
    }
    let voices = elevenlabs::voices(&key)?;
    eprintln!("ElevenLabs: {} 个音色", voices.len());
    let voice = voices.first().ok_or("ElevenLabs 未返回可用音色")?;
    let audio = elevenlabs::synthesize(&key, elevenlabs::DEFAULT_MODEL, &voice.id, PHRASE)?;
    save("elevenlabs", &audio)
}

#[test]
#[ignore = "真实 MiniMax 合成，消耗额度，需要显式授权"]
fn minimax_live() -> Result<(), String> {
    let Ok(key) = std::env::var("MINIMAX_API_KEY") else {
        eprintln!("MiniMax: 跳过（MINIMAX_API_KEY 不存在）");
        return Ok(());
    };
    if key.trim().is_empty() {
        eprintln!("MiniMax: 跳过（MINIMAX_API_KEY 为空）");
        return Ok(());
    }
    let region = std::env::var("MINIMAX_REGION").unwrap_or_else(|_| "cn".into());
    if !matches!(region.as_str(), "cn" | "global") {
        return Err("MINIMAX_REGION 必须为 cn 或 global".into());
    }
    let voices = minimax::voices(&key, &region)?;
    eprintln!("MiniMax: {} 个音色（{region}）", voices.len());
    let voice = voices.first().ok_or("MiniMax 未返回可用音色")?;
    let audio = minimax::synthesize(&key, minimax::DEFAULT_MODEL, &voice.id, PHRASE, &region)?;
    save("minimax", &audio)
}
