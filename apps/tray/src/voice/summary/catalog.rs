//! Which catalog entries a text summary may pick.
//!
//! Explicit capability fields decide first: a populated modality list without `text` means the
//! model cannot take a text chat, and a Google entry that lists generation methods without
//! `generateContent` is not a chat model. Missing or empty fields prove nothing, so those entries
//! fall through to the name check. Names only remove dedicated endpoints that can
//! still pass a capability check, such as speech-to-text models that answer in text.

use serde_json::Value;

/// Whole words in the model's own path segment that name a dedicated non-chat endpoint.
/// `vision` and `audio` stay out on purpose: general multimodal models carry them and can still
/// summarize text.
const DEDICATED: &[&str] = &[
    // Speech synthesis, transcription and real-time voice.
    "tts",
    "speech",
    "whisper",
    "transcribe",
    "transcription",
    "asr",
    "stt",
    "realtime",
    "live",
    // Music, image and video generation.
    "lyria",
    "music",
    "musicgen",
    "imagen",
    "image",
    "imagegen",
    "dall",
    "flux",
    "diffusion",
    "veo",
    "sora",
    // Embeddings and reranking.
    "embedding",
    "embeddings",
    "embed",
    "rerank",
    "reranker",
    "reranking",
    // Dedicated agents and other endpoints.
    "robotics",
    "antigravity",
    "moderation",
    "aqa",
];

/// Two-word markers that are only meaningful together; matching `use` or `research` alone would
/// remove ordinary chat models.
const DEDICATED_PHRASES: &[&[&str]] = &[
    &["nano", "banana"],
    &["video", "generation"],
    &["computer", "use"],
    &["deep", "research"],
    &["native", "audio"],
];

pub(super) fn suitable_for_summary(item: &Value, id: &str) -> bool {
    !lacks_capability(&item["architecture"]["input_modalities"], "text")
        && !lacks_capability(&item["architecture"]["output_modalities"], "text")
        && !lacks_capability(&item["supportedGenerationMethods"], "generateContent")
        && !dedicated_name(id)
}

fn lacks_capability(value: &Value, required: &str) -> bool {
    value.as_array().is_some_and(|values| {
        values.iter().any(Value::is_string)
            && !values.iter().any(|value| {
                value
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(required))
            })
    })
}

/// Only the model's own segment counts, so a provider called `tts` or `live` keeps its models.
/// Matching whole words keeps `livewire` or `imagecraft` from looking dedicated.
fn dedicated_name(id: &str) -> bool {
    let segment = id.rsplit('/').next().unwrap_or(id);
    let tokens: Vec<&str> = segment
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    tokens.iter().any(|token| {
        DEDICATED
            .iter()
            .any(|name| token.eq_ignore_ascii_case(name))
    }) || DEDICATED_PHRASES.iter().any(|phrase| {
        tokens.windows(phrase.len()).any(|window| {
            window
                .iter()
                .zip(phrase.iter())
                .all(|(token, name)| token.eq_ignore_ascii_case(name))
        })
    })
}

#[cfg(test)]
mod tests;
