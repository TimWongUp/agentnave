use super::super::{SummaryModel, parse_models};
use serde_json::{Value, json};

fn parse(items: Vec<Value>) -> Vec<SummaryModel> {
    let items: Vec<&Value> = items.iter().collect();
    parse_models(&items)
}

fn ids(models: &[SummaryModel]) -> Vec<&str> {
    models.iter().map(|model| model.id.as_str()).collect()
}

#[test]
fn capability_fields_remove_models_without_text_or_generate_content() {
    let models = parse(vec![
        json!({"id":"vendor/chat","architecture":{
            "input_modalities":["text","image"],
            "output_modalities":["text"]
        }}),
        json!({"id":"vendor/no-output-text","architecture":{
            "input_modalities":["text"],
            "output_modalities":["image"]
        }}),
        json!({"id":"vendor/no-input-text","architecture":{
            "input_modalities":["audio"],
            "output_modalities":["text"]
        }}),
        json!({"id":"vendor/unknown"}),
        json!({"id":"vendor/empty-capabilities","architecture":{
            "input_modalities":[], "output_modalities":[]
        },"supportedGenerationMethods":[]}),
        json!({"id":"vendor/invalid-capabilities","architecture":{
            "input_modalities":[null], "output_modalities":[42]
        },"supportedGenerationMethods":[false]}),
        json!({"id":"vendor/input-only","architecture":{"input_modalities":["text"]}}),
        json!({"id":"vendor/output-only","architecture":{"output_modalities":["text"]}}),
        json!({"id":"vendor/malformed","architecture":{
            "input_modalities":"text",
            "output_modalities":null
        }}),
        json!({"id":"vendor/generate",
            "supportedGenerationMethods":["generateContent","countTokens"]}),
        json!({"id":"vendor/no-generate","supportedGenerationMethods":["embedContent"]}),
        json!({"id":"vendor/method-not-a-list","supportedGenerationMethods":"generateContent"}),
    ]);
    assert_eq!(
        ids(&models),
        [
            "vendor/chat",
            "vendor/unknown",
            "vendor/empty-capabilities",
            "vendor/invalid-capabilities",
            "vendor/input-only",
            "vendor/output-only",
            "vendor/malformed",
            "vendor/generate",
            "vendor/method-not-a-list",
        ]
    );
}

#[test]
fn dedicated_names_are_removed_but_general_and_unknown_models_survive() {
    let models = parse(vec![
        json!({
            "id":"google/gemini-2.5-pro",
            "display_name":"Gemini 2.5 Pro",
            "owned_by":"google",
            "reasoning_levels":["low","high"],
            "supported_reasoning_levels":[{"effort":"medium"}]
        }),
        json!({"id":"google/gemini-2.5-flash"}),
        json!({"id":"google/gemini-9-ultra-preview-01"}),
        json!({"id":"google/gemma-3-27b-it"}),
        json!({"id":"openai/gpt-4o"}),
        json!({"id":"openai/gpt-4o-audio-preview"}),
        json!({"id":"qwen/qwen2.5-vl-72b-instruct"}),
        json!({"id":"meta-llama/llama-3.2-90b-vision-instruct"}),
        json!({"id":"tts-provider/gemini-2.5-flash"}),
        json!({"id":"live-provider/chat-model"}),
        json!({"id":"google/livewire-chat"}),
        json!({"id":"google/imagecraft-chat"}),
        json!({"id":"vendor/video-understanding","architecture":{
            "input_modalities":["text","video"], "output_modalities":["text"]
        }}),
        json!({"id":"google/gemini-2.5-flash-preview-tts"}),
        json!({"id":"openai/gpt-4o-transcribe"}),
        json!({"id":"openai/gpt-4o-realtime-preview"}),
        json!({"id":"google/gemini-2.0-flash-live-001"}),
        json!({"id":"google/gemini-2.5-flash-native-audio-dialog"}),
        json!({"id":"google/lyria-002"}),
        json!({"id":"google/imagen-4.0-generate-001"}),
        json!({"id":"google/gemini-2.5-flash-image"}),
        json!({"id":"google/models/nano-banana-pro-preview"}),
        json!({"id":"google/models/gemini-nano-banana-2.1"}),
        json!({"id":"openai/dall-e-3"}),
        json!({"id":"google/veo-3.0-generate-001"}),
        json!({"id":"vendor/video-generation-1"}),
        json!({"id":"openai/sora-2"}),
        json!({"id":"openai/text-embedding-3-small"}),
        json!({"id":"BAAI/bge-reranker-v2-m3"}),
        json!({"id":"google/gemini-robotics-er-1.5-preview"}),
        json!({"id":"google/gemini-2.5-computer-use-preview-10-2025"}),
        json!({"id":"google/antigravity-preview-05-2026"}),
        json!({"id":"google/deep-research-pro-preview-12-2025"}),
        json!({"id":"openai/omni-moderation-latest"}),
        json!({"id":"google/models/aqa"}),
        json!({"id":"google/gemini-2.5-pro","display_name":"Duplicate retained"}),
    ]);
    assert_eq!(
        ids(&models),
        [
            "google/gemini-2.5-pro",
            "google/gemini-2.5-flash",
            "google/gemini-9-ultra-preview-01",
            "google/gemma-3-27b-it",
            "openai/gpt-4o",
            "openai/gpt-4o-audio-preview",
            "qwen/qwen2.5-vl-72b-instruct",
            "meta-llama/llama-3.2-90b-vision-instruct",
            "tts-provider/gemini-2.5-flash",
            "live-provider/chat-model",
            "google/livewire-chat",
            "google/imagecraft-chat",
            "vendor/video-understanding",
            "google/gemini-2.5-pro",
        ]
    );
    assert_eq!(models[0].display_name, "Gemini 2.5 Pro");
    assert_eq!(models[0].owned_by.as_deref(), Some("google"));
    assert_eq!(models[0].reasoning_levels, ["low", "high", "medium"]);
    assert_eq!(models.last().unwrap().display_name, "Duplicate retained");
}

#[test]
fn text_capability_does_not_override_a_dedicated_name() {
    let models = parse(vec![
        json!({"id":"google/gemini-2.5-flash-preview-tts","architecture":{
            "input_modalities":["text"],
            "output_modalities":["text"]
        }}),
        json!({"id":"google/gemini-2.5-flash-image","architecture":{
            "input_modalities":["text","image"],
            "output_modalities":["text","image"]
        }}),
        json!({"id":"vendor/general","architecture":{
            "input_modalities":["text"],
            "output_modalities":["text"]
        }}),
    ]);
    assert_eq!(ids(&models), ["vendor/general"]);
}
