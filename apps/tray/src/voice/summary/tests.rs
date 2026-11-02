use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

#[derive(Clone)]
struct Reply {
    status: u16,
    body: Value,
}

#[derive(Clone)]
struct Recorded {
    path: String,
    headers: String,
    body: Vec<u8>,
}

struct FakeServer {
    root: String,
    worker: Option<thread::JoinHandle<Vec<Recorded>>>,
}

fn header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
        .unwrap_or(0)
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Recorded> {
    let mut bytes = Vec::new();
    let end = loop {
        if let Some(end) = header_end(&bytes) {
            break end;
        }
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let length = content_length(&headers);
    while bytes.len() < end + 4 + length {
        let mut chunk = [0; 4096];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let path = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_string();
    Ok(Recorded {
        path,
        headers,
        body: bytes[end + 4..end + 4 + length].to_vec(),
    })
}

fn error_of<T>(result: Result<T, String>) -> String {
    result.err().expect("expected an error")
}

fn error(result: Result<Response, String>) -> String {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => error,
    }
}

impl FakeServer {
    fn start<F>(count: usize, mut respond: F) -> Self
    where
        F: FnMut(&Recorded) -> Reply + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = Vec::new();
            while requests.len() < count && Instant::now() < deadline {
                let Ok((mut stream, _)) = listener.accept() else {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let request = match read_request(&mut stream) {
                    Ok(request) => request,
                    Err(_) => continue,
                };
                let reply = respond(&request);
                let body = reply.body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    body.len(),
                    body
                );
                requests.push(request);
            }
            requests
        });
        Self {
            root: format!("http://{address}"),
            worker: Some(worker),
        }
    }

    fn finish(mut self) -> Vec<Recorded> {
        self.worker.take().unwrap().join().unwrap()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = TcpStream::connect(
                self.root
                    .trim_start_matches("http://")
                    .trim_end_matches("/v1"),
            );
            let _ = worker.join();
        }
    }
}

fn summary_settings(root: &str, provider: &str, model: &str) -> Value {
    json!({"enabled":true,"provider":provider,"base_url":root,"model":model})
}

fn catalog() -> Reply {
    Reply {
        status: 200,
        body: json!({"data":[
            {
                "id":"opencode-go/deepseek-v4.1-flash",
                "display_name":"DeepSeek V4.1 Flash",
                "owned_by":"opencode-go",
                "reasoning_levels":["low","max","low"],
                "supported_reasoning_levels":[{"effort":"medium"},{"effort":"low"},{"effort":"high"}]
            },
            {"id":"anthropic/haiku","display_name":"Claude Haiku","owned_by":"anthropic"},
            {"id":"opencode-go/deepseek-v4.1-flash","display_name":"Duplicate retained","owned_by":"opencode-go"}
        ]}),
    }
}

fn chat(summary: &str) -> Reply {
    Reply {
        status: 200,
        body: json!({"choices":[{"finish_reason":"stop","message":{"content":summary}}]}),
    }
}

#[test]
fn magpie_identity_catalog_display_names_and_low_reasoning_are_preserved() {
    let server = FakeServer::start(5, |request| match request.path.as_str() {
        "/api/hello" => Reply {
            status: 200,
            body: json!({"name":"magpie","version":"test"}),
        },
        "/v1/models" => catalog(),
        "/v1/chat/completions" => chat("先说结论。理由是改动直接。主要分歧在费用。"),
        _ => Reply {
            status: 404,
            body: json!({}),
        },
    });
    let settings = summary_settings("", "magpie", "opencode-go/deepseek-v4.1-flash");
    let (base, key, magpie) = endpoint_at(&settings, "", Some(&server.root)).unwrap();
    assert!(magpie);
    assert_eq!(key, MAGPIE_KEY);
    let models = models_at(&base, &key).unwrap();
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, "opencode-go/deepseek-v4.1-flash");
    assert_eq!(models[0].display_name, "DeepSeek V4.1 Flash");
    assert_eq!(models[0].owned_by.as_deref(), Some("opencode-go"));
    assert_eq!(models[0].reasoning_levels, ["low", "max", "medium", "high"]);
    assert_eq!(models[1].id, "anthropic/haiku");
    assert_eq!(listed(&models)[0]["display_name"], "DeepSeek V4.1 Flash");
    assert_eq!(listed(&models)[0]["owned_by"], "opencode-go");
    assert_eq!(
        listed(&models)[0]["reasoning_levels"],
        json!(["low", "max", "medium", "high"])
    );

    let directory = tempfile::tempdir().unwrap();
    assert!(
        summarize_at(
            directory.path(),
            "这是一段需要摘要的长发言。",
            &settings,
            "",
            Some(&server.root)
        )
        .unwrap()
        .contains("先说结论")
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 5);
    assert!(
        requests[0]
            .headers
            .contains(&format!("authorization: Bearer {MAGPIE_KEY}"))
            || requests[0]
                .headers
                .contains(&format!("Authorization: Bearer {MAGPIE_KEY}"))
    );
    assert!(requests[0].headers.contains(USER_AGENT));
    let body: Value = serde_json::from_slice(&requests[4].body).unwrap();
    assert_eq!(body["model"], "opencode-go/deepseek-v4.1-flash");
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(body["stream"], false);
    assert_eq!(body["max_tokens"], 350);
}

#[test]
fn explicit_effort_is_sent_exactly_and_unsupported_effort_is_not_sent() {
    let server = FakeServer::start(3, |request| match request.path.as_str() {
        "/api/hello" => Reply {
            status: 200,
            body: json!({"name":"magpie"}),
        },
        "/v1/models" => catalog(),
        "/v1/chat/completions" => chat("结论不变。理由清楚。分歧保留。"),
        _ => Reply {
            status: 404,
            body: json!({}),
        },
    });
    let mut settings = summary_settings("", "magpie", "opencode-go/deepseek-v4.1-flash");
    settings["effort"] = json!("max");
    let directory = tempfile::tempdir().unwrap();
    summarize_at(
        directory.path(),
        "这是一段需要摘要的长发言。",
        &settings,
        "",
        Some(&server.root),
    )
    .unwrap();
    let requests = server.finish();
    let body: Value = serde_json::from_slice(&requests[2].body).unwrap();
    assert_eq!(body["reasoning_effort"], "max");

    let unsupported = FakeServer::start(2, |request| match request.path.as_str() {
        "/api/hello" => Reply {
            status: 200,
            body: json!({"name":"magpie"}),
        },
        "/v1/models" => catalog(),
        _ => Reply {
            status: 200,
            body: json!({"choices":[{"finish_reason":"stop","message":{"content":"不应发送"}}]}),
        },
    });
    settings["effort"] = json!("ultra");
    let error = summarize_at(
        directory.path(),
        "另一段需要摘要的长发言。",
        &settings,
        "",
        Some(&unsupported.root),
    )
    .unwrap_err();
    assert!(error.contains("不支持"));
    assert_eq!(unsupported.finish().len(), 2);
}

#[test]
fn summary_model_route_returns_complete_ordered_metadata() {
    let server = FakeServer::start(1, |_| catalog());
    let directory = tempfile::tempdir().unwrap();
    let mut settings = json!({
        "enabled":false,
        "engine":"elevenlabs",
        "rate":"normal",
        "minimax_region":"cn",
        "models":{},
        "voices":{},
        "summary":summary_settings(&server.root, "custom", "opencode-go/deepseek-v4.1-flash")
    });
    storage::save_settings(directory.path(), &settings).unwrap();
    let path = "/app/voice/summary/models";
    assert_eq!(get(directory.path(), path).status, 405);
    assert_eq!(
        super::super::request(directory.path(), path, "POST", None, b"{}").status,
        403
    );
    let disabled = super::super::request(directory.path(), path, "POST", Some("1"), b"{}");
    assert_eq!(disabled.status, 400);
    assert!(String::from_utf8_lossy(&disabled.body).contains("请先开启会议语音"));
    settings["enabled"] = json!(true);
    storage::save_settings(directory.path(), &settings).unwrap();
    let response = super::super::request(directory.path(), path, "POST", Some("1"), b"{}");
    assert_eq!(response.status, 200);
    let models: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(models.as_array().unwrap().len(), 3);
    assert_eq!(models[0]["id"], "opencode-go/deepseek-v4.1-flash");
    assert_eq!(models[0]["display_name"], "DeepSeek V4.1 Flash");
    assert_eq!(models[0]["owned_by"], "opencode-go");
    assert_eq!(
        models[0]["reasoning_levels"],
        json!(["low", "max", "medium", "high"])
    );
    assert_eq!(models[2]["id"], "opencode-go/deepseek-v4.1-flash");
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn anthropic_headers_follow_the_endpoint_instead_of_the_key_prefix() {
    for (base, key, anthropic) in [
        ("https://api.anthropic.com/v1", "rotated-key", true),
        ("https://api.openai.com/v1", "sk-ant-other", false),
    ] {
        let request = request(
            summary_client().unwrap().get(format!("{base}/models")),
            key,
            base,
        )
        .build()
        .unwrap();
        let headers = request.headers();
        assert_eq!(headers.contains_key("x-api-key"), anthropic);
        assert_eq!(headers.contains_key("anthropic-version"), anthropic);
        assert_eq!(headers["authorization"], format!("Bearer {key}"));
        if anthropic {
            assert_eq!(headers["x-api-key"], key);
        }
    }
}

#[test]
fn custom_urls_are_https_or_loopback_without_credentials_query_or_fragment() {
    assert_eq!(
        normalize_base_url("https://api.example.com/v1/").unwrap(),
        "https://api.example.com/v1"
    );
    assert!(normalize_base_url("http://127.0.0.1:8080/v1").is_ok());
    assert!(normalize_base_url("http://[::1]:8080/v1").is_ok());
    for invalid in [
        "http://api.example.com/v1",
        "https://user@api.example.com/v1",
        "https://@api.example.com/v1",
        "https:user:pass@api.example.com/v1",
        "https://api.example.com/v1?key=bad",
        "https://api.example.com/v1#bad",
    ] {
        assert!(normalize_base_url(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn summary_output_rejects_empty_symbols_and_length_and_caps_long_text() {
    for choice in [
        json!({"finish_reason":"stop","message":{"content":""}}),
        json!({"finish_reason":"stop","message":{"content":"……！？"}}),
        json!({"finish_reason":"length","message":{"content":"有内容"}}),
    ] {
        assert!(output(&json!({"choices":[choice]})).is_err());
    }
    let content = format!("{}。{}。", "甲".repeat(100), "乙".repeat(100));
    let value = json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]});
    let generated = output(&value).unwrap();
    assert!(generated.chars().count() <= 140);
    assert!(generated.ends_with('。'));
}

#[test]
fn summary_cache_deduplicates_and_model_or_effort_changes_invalidate() {
    let server = FakeServer::start(7, |request| {
        if request.path.ends_with("/models") {
            return Reply {
                status: 200,
                body: json!({"data":[
                    {"id":"model-a","reasoning_levels":["low","high"]},
                    {"id":"model-b","reasoning_levels":["low","high"]}
                ]}),
            };
        }
        let _: Value = serde_json::from_slice(&request.body).unwrap();
        thread::sleep(Duration::from_millis(150));
        chat("摘要完成：相同长发言。理由清楚。分歧保留。")
    });
    let directory = Arc::new(tempfile::tempdir().unwrap());
    let mut configured = summary_settings(&server.root, "custom", "model-a");
    configured["effort"] = json!("low");
    let settings = Arc::new(configured);
    let base = normalize_base_url(&server.root).unwrap();
    // Hold other cache inputs fixed so the version change alone rejects an old valid summary.
    for (version, cached) in [
        (PROMPT_VERSION, "……"),
        ("message-compression-v2", "旧的第三人称总结。"),
    ] {
        let cache_key = storage::hash(
            &json!([
                version,
                base,
                "model-a",
                storage::hash(""),
                "low",
                "low",
                DEFAULT_PROMPT,
                "",
                "",
                "相同长发言"
            ])
            .to_string(),
        );
        storage::cache(directory.path(), &cache_key, cached.as_bytes(), "txt").unwrap();
    }
    // One catalog read serves every later summary of the same model.
    assert!(catalog_levels(&base, "", "model-a").unwrap().is_some());
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let directory = directory.clone();
            let settings = settings.clone();
            thread::spawn(move || summarize(directory.path(), "相同长发言", &settings, "").unwrap())
        })
        .collect();
    for worker in workers {
        assert_eq!(
            worker.join().unwrap(),
            "摘要完成：相同长发言。理由清楚。分歧保留。"
        );
    }
    assert_eq!(
        summarize(directory.path(), "相同长发言", &settings, "").unwrap(),
        "摘要完成：相同长发言。理由清楚。分歧保留。"
    );
    let mut high = summary_settings(&server.root, "custom", "model-a");
    high["effort"] = json!("high");
    summarize(directory.path(), "相同长发言", &high, "").unwrap();
    let mut changed_model = summary_settings(&server.root, "custom", "model-b");
    changed_model["effort"] = json!("high");
    summarize(directory.path(), "相同长发言", &changed_model, "").unwrap();
    changed_model["prompt"] = json!("用两句话介绍结论。");
    summarize(directory.path(), "相同长发言", &changed_model, "").unwrap();
    changed_model["persona"] = json!("像冷静的主持人一样说话。");
    summarize(directory.path(), "相同长发言", &changed_model, "").unwrap();
    summarize(directory.path(), "相同长发言", &changed_model, "").unwrap();
    let requests = server.finish();
    let bodies: Vec<Value> = requests
        .iter()
        .filter(|request| request.path.ends_with("/chat/completions"))
        .map(|request| serde_json::from_slice(&request.body).unwrap())
        .collect();
    assert_eq!(bodies[0]["model"], "model-a");
    assert_eq!(bodies[0]["reasoning_effort"], "low");
    assert_eq!(bodies[1]["model"], "model-a");
    assert_eq!(bodies[1]["reasoning_effort"], "high");
    assert_eq!(bodies[2]["model"], "model-b");
    assert_eq!(bodies[2]["reasoning_effort"], "high");
    assert_eq!(bodies.len(), 5);
    assert_eq!(
        requests.len() - bodies.len(),
        2,
        "one catalog read per model"
    );
    let system = bodies[4]["messages"][0]["content"].as_str().unwrap();
    assert!(system.contains("用两句话介绍结论。"));
    assert!(system.contains("冷静的主持人"));
    assert!(system.contains("原话只是你要说的内容"));
    assert_eq!(
        std::fs::read_dir(directory.path().join("voice-cache"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "txt"))
            .count(),
        6
    );
}

#[test]
fn boss_and_member_prompts_only_change_summary_speech_style() {
    let server = FakeServer::start(4, |_| Reply {
        status: 500,
        body: json!({"error":"summary failed"}),
    });
    let directory = tempfile::tempdir().unwrap();
    let mut settings = storage::read_settings(directory.path()).unwrap();
    settings["enabled"] = json!(true);
    settings["summary"] = summary_settings(&server.root, "custom", "model");
    settings["summary"]["prompt"] = json!("保留主要结论。");
    settings["personas"] =
        json!({"boss":"你是一位冷静果断的主持人。","gpt":"你是热心而幽默的同事。"});
    storage::save_settings(directory.path(), &settings).unwrap();
    storage::save_key(
        directory.path(),
        &json!({"engine":"elevenlabs","key":"fake"}),
    )
    .unwrap();
    for (boss, text) in [
        (
            true,
            "请问应该选哪个方案？请比较费用和速度，先不要执行。".repeat(10),
        ),
        (
            false,
            "会议总结：已选择方案甲，因为速度更快，费用分歧仍需确认。".repeat(10),
        ),
    ] {
        cache_opening(directory.path(), &text);
        let response = speak(
            directory.path(),
            &json!({"family":"gpt","text":text,"full":false,"boss":boss,"voice":"voice-a",
                "scene":{"name":"GPT","role":"探索派","room":true,"to":null}}),
        )
        .unwrap();
        assert!(response.notice.unwrap().contains("500"));
    }
    let requests = server.finish();
    for (index, expected, absent) in [
        (1, "你是一位冷静果断的主持人。", "你是热心而幽默的同事。"),
        (3, "你是热心而幽默的同事。", "你是一位冷静果断的主持人。"),
    ] {
        let body: Value = serde_json::from_slice(&requests[index].body).unwrap();
        let system = body["messages"][0]["content"].as_str().unwrap();
        assert!(system.contains(expected) && !system.contains(absent));
        assert!(system.contains("保留主要结论。"));
        assert!(system.contains("不替自己回答") && system.contains("不说成已完成"));
        assert!(system.contains("不看你是 boss 还是员工"));
        assert!(system.contains("你的人设（性格、措辞与口吻）"));
        assert!(system.contains(if index == 1 {
            "你是这个工作区的 boss"
        } else {
            "你是员工 GPT，工位角色是「探索派」"
        }));
        let original = body["messages"][1]["content"].as_str().unwrap();
        assert!(original.starts_with("下面是你要说的原话"));
        assert!(original.contains(if index == 1 {
            "请问应该选哪个方案？"
        } else {
            "会议总结：已选择方案甲"
        }));
    }
}

/// The audio a failed summary falls back to, so a test never reaches a real voice service.
fn cache_opening(root: &Path, raw: &str) {
    let settings = storage::read_settings(root).unwrap();
    let opening = text::excerpt(&text::clean(raw, true));
    let key = super::super::audio_key(&settings, "elevenlabs", "voice-a", &opening);
    storage::cache(root, &key, b"opening", "mp3").unwrap();
}

#[test]
fn short_text_skips_summary_and_failed_summary_reads_the_opening() {
    let directory = tempfile::tempdir().unwrap();
    let mut settings = json!({
        "enabled":true,
        "engine":"elevenlabs",
        "rate":"normal",
        "minimax_region":"cn",
        "models":{},
        "voices":{},
        "summary":summary_settings("http://127.0.0.1:9/v1", "custom", "model")
    });
    storage::save_settings(directory.path(), &settings).unwrap();
    let short = error(speak(
        directory.path(),
        &json!({"family":null,"text":"这是一句短话。","full":true}),
    ));
    assert_eq!(short, "请先保存所选引擎的 API 密钥");
    let long = error(speak(
        directory.path(),
        &json!({"family":null,"text":"字".repeat(51),"full":true}),
    ));
    assert_eq!(long, "请先保存所选引擎的 API 密钥");

    let server = FakeServer::start(2, |_| Reply {
        status: 500,
        body: json!({"error":"summary failed"}),
    });
    settings["summary"] = summary_settings(&server.root, "custom", "model");
    storage::save_settings(directory.path(), &settings).unwrap();
    storage::save_key(
        directory.path(),
        &json!({"engine":"elevenlabs","key":"fake-tts-key"}),
    )
    .unwrap();
    let raw = "这是一段超过一百二十字的发言开头，用来确保进入摘要失败路径。".repeat(5);
    cache_opening(directory.path(), &raw);
    let failed = speak(
        directory.path(),
        &json!({"family":null,"text":raw,"full":true,"voice":"voice-a"}),
    )
    .unwrap();
    assert_eq!(failed.body, b"opening");
    assert!(failed.notice.unwrap().contains("500"));
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].path.ends_with("/models"));
    assert!(requests[1].path.ends_with("/chat/completions"));
    let body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert!(body["reasoning_effort"].is_null());
}

#[test]
fn summary_limits_and_magpie_config_are_explicit() {
    let directory = tempfile::tempdir().unwrap();
    let settings = json!({
        "enabled":true,
        "engine":"elevenlabs",
        "rate":"normal",
        "minimax_region":"cn",
        "models":{},
        "voices":{},
        "summary":summary_settings("http://127.0.0.1:9/v1", "custom", "model")
    });
    storage::save_settings(directory.path(), &settings).unwrap();
    // A very long message is no longer refused; it still needs the engine's key.
    assert_eq!(
        error(speak(
            directory.path(),
            &json!({"family":null,"text":"字".repeat(30_001),"full":true}),
        )),
        "请先保存所选引擎的 API 密钥"
    );
    assert_eq!(
        error(speak(
            directory.path(),
            &json!({"family":null,"text":"字".repeat(51),"full":true,"preview":true}),
        )),
        "请先保存所选引擎的 API 密钥"
    );
    assert_eq!(
        super::super::request(
            directory.path(),
            "/app/voice/speak",
            "POST",
            Some("1"),
            &vec![b' '; 200_001],
        )
        .status,
        400
    );

    let config = tempfile::tempdir().unwrap();
    let magpie = config.path().join("magpie");
    std::fs::create_dir_all(&magpie).unwrap();
    let path = magpie.join("settings.json");
    std::fs::write(&path, r#"{"port":4312,"private":"not read"}"#).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(magpie_port_in(Some(config.path()), None), 4312);
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(
        magpie_port_in(Some(&config.path().join("missing")), None),
        3425
    );
}

#[test]
fn each_platform_keeps_its_own_key_and_needs_one() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let settings = json!({"provider":"deepseek"});
    assert!(error_of(endpoint_at(&settings, "", None)).contains("DeepSeek"));
    let (base, key, magpie) = endpoint_at(&settings, "k", None).unwrap();
    assert_eq!(
        (base.as_str(), key.as_str(), magpie),
        ("https://api.deepseek.com/v1", "k", false)
    );
    storage::save_key(root, &json!({"engine":"summary:deepseek","key":"ds"})).unwrap();
    storage::save_key(root, &json!({"engine":"summary","key":"mine"})).unwrap();
    let keys = storage::read_keys(root).unwrap();
    assert_eq!(key_of(&keys, &settings), "ds");
    assert_eq!(key_of(&keys, &json!({"provider":"groq"})), "");
    assert_eq!(key_of(&keys, &json!({"provider":"custom"})), "mine");
    for refused in ["chatgpt", "chatgpt_host", "summary:unknown"] {
        assert!(storage::save_key(root, &json!({"engine":refused,"key":"x"})).is_err());
    }
    let mut saved = storage::read_settings(root).unwrap();
    saved["summary"]["provider"] = json!("nobody");
    assert!(storage::validate(&saved).is_err());
    for platform in platform_list().as_array().unwrap() {
        saved["summary"]["provider"] = platform["id"].clone();
        assert!(storage::validate(&saved).is_ok());
    }
}

#[test]
fn a_chatgpt_catalog_lists_its_slugs_for_pickers_and_their_levels() {
    let value = json!({"models":[
        {"slug":"gpt-a","visibility":"list","supported_reasoning_levels":[{"effort":"low"}]},
        {"slug":"gpt-hidden","visibility":"hide"}
    ]});
    let items = catalog_items(&value).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(model_id(items[0]), Some("gpt-a"));
    assert_eq!(levels(items[0]), ["low"]);
}

#[test]
fn openai_chat_gets_its_own_token_limit_field() {
    let body = for_openai(request_prompt("m", "t", None, "s"));
    assert!(body.get("temperature").is_none() && body.get("max_tokens").is_none());
    assert_eq!(body["max_completion_tokens"], 350);
}
