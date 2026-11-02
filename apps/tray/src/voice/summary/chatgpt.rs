//! A ChatGPT plan used through OpenAI's API, by OpenAI's "Sign in with ChatGPT" for apps running
//! on the user's machine: a browser sign-in registers this app, and the account's own token then
//! asks `api.openai.com`. Tokens stay in `voice-keys.json`; the page only hears who signed in.
use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::Digest;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) const API: &str = "https://api.openai.com/v1";
const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const AGENT_NAME: &str = "AgentNave";
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// OpenAI's own apps return here first; only the port of a loopback address may vary.
const CALLBACK: &str = "127.0.0.1:1455";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// Refresh this long before a token expires, so no request starts with one about to.
const MARGIN_SECONDS: u64 = 180;
/// Refusals of a refresh token that will never work again.
const DEAD: [&str; 6] = [
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];

#[derive(Default)]
struct Flow {
    generation: u64,
    signing_in: bool,
    error: Option<String>,
}
static FLOW: Mutex<Flow> = Mutex::new(Flow {
    generation: 0,
    signing_in: false,
    error: None,
});
static REFRESH: Mutex<()> = Mutex::new(());

fn flow() -> std::sync::MutexGuard<'static, Flow> {
    FLOW.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn random(bytes: usize) -> Result<String, String> {
    let mut buffer = vec![0; bytes];
    getrandom::fill(&mut buffer).map_err(|_| "无法生成登录随机数")?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

fn query(pairs: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").expect("a valid address");
    url.query_pairs_mut().extend_pairs(pairs);
    url.query().unwrap_or("").to_string()
}

/// What the page may know: never a token.
pub(in crate::voice) fn state(keys: &Value) -> Value {
    let flow = flow();
    let account = &keys["chatgpt"];
    json!({
        "signed_in": account["refresh"].as_str().is_some_and(|token| !token.is_empty()),
        "user": account["user"],
        "plan": account["plan"],
        "signing_in": flow.signing_in,
        "error": flow.error,
    })
}

fn authorize_url(redirect: &str, state: &str, nonce: &str, verifier: &str, host: &str) -> String {
    let challenge = URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()));
    format!(
        "{AUTHORIZE}?{}",
        query(&[
            ("client_id", DYNAMIC_CLIENT),
            ("agent_name_hint", AGENT_NAME),
            ("ext_agent_host_id", host),
            ("response_type", "code"),
            ("redirect_uri", redirect),
            ("resource", API),
            ("scope", SCOPES),
            ("state", state),
            ("nonce", nonce),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ])
    )
}

/// One machine, one id: OpenAI issues its client for it.
fn host_id(root: &Path) -> Result<String, String> {
    if let Some(host) = storage::read_keys(root)?["chatgpt_host"].as_str() {
        return Ok(host.to_string());
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "无法生成登录随机数")?;
    bytes[6] = bytes[6] & 0x0f | 0x40;
    bytes[8] = bytes[8] & 0x3f | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let host = format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    );
    storage::edit_keys(root, |keys| {
        keys["chatgpt_host"] = json!(host);
        Ok(())
    })?;
    Ok(host)
}

/// Opens the browser and waits for it in the background; a newer sign-in replaces this one.
pub(in crate::voice) fn start(root: &Path) -> Result<(), String> {
    let host = host_id(root)?;
    let listener = TcpListener::bind(CALLBACK)
        .or_else(|_| TcpListener::bind("127.0.0.1:0"))
        .map_err(|_| "无法开始登录：本机端口不可用")?;
    let port = listener.local_addr().map_err(|_| "无法开始登录")?.port();
    let sign_in = SignIn {
        redirect: format!("http://127.0.0.1:{port}/auth/callback"),
        state: random(24)?,
        nonce: random(24)?,
        verifier: random(48)?,
        host,
        root: root.to_path_buf(),
    };
    let url = authorize_url(
        &sign_in.redirect,
        &sign_in.state,
        &sign_in.nonce,
        &sign_in.verifier,
        &sign_in.host,
    );
    let generation = {
        let mut flow = flow();
        flow.generation += 1;
        flow.signing_in = true;
        flow.error = None;
        flow.generation
    };
    std::thread::spawn(move || {
        let result = sign_in.wait(&listener, generation);
        let mut flow = flow();
        if flow.generation == generation {
            flow.signing_in = false;
            flow.error = result.err();
        }
    });
    crate::browse_address(&url);
    Ok(())
}

struct SignIn {
    redirect: String,
    state: String,
    nonce: String,
    verifier: String,
    host: String,
    root: PathBuf,
}

impl SignIn {
    fn wait(&self, listener: &TcpListener, generation: u64) -> Result<(), String> {
        listener
            .set_nonblocking(true)
            .map_err(|_| "无法等待登录结果")?;
        let deadline = std::time::Instant::now() + SIGN_IN_TIMEOUT;
        loop {
            if flow().generation != generation {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err("登录已超时，请重新开始".into());
            }
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            };
            let Some(query) = callback_query(&mut stream) else {
                answer(&mut stream, "404 Not Found", "");
                continue;
            };
            // Anything on this port can knock: only the state this sign-in sent counts.
            if value(&query, "state").as_deref() != Some(&self.state) {
                answer(
                    &mut stream,
                    "400 Bad Request",
                    "登录未完成，请回到 AgentNave 重试。",
                );
                continue;
            }
            let result = self.finish(&query, generation);
            let text = if result.is_ok() {
                "已登录，可以关闭这个页面。"
            } else {
                "登录未完成，请回到 AgentNave 重试。"
            };
            answer(&mut stream, "200 OK", text);
            return result;
        }
    }

    fn finish(&self, query: &str, generation: u64) -> Result<(), String> {
        if let Some(error) = value(query, "error") {
            let detail = value(query, "error_description").unwrap_or(error);
            return Err(format!("OpenAI 未完成登录：{detail}"));
        }
        let code = value(query, "code").ok_or("OpenAI 没有返回登录结果")?;
        let client = value(query, "client_id")
            .filter(|client| !client.trim().is_empty())
            .ok_or("OpenAI 没有完成注册，请重新登录")?;
        let reply = post_token(
            TOKEN,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &client),
                ("code", &code),
                ("code_verifier", &self.verifier),
                ("redirect_uri", &self.redirect),
                ("resource", API),
            ],
        )?;
        let id = reply["id_token"]
            .as_str()
            .ok_or("OpenAI 没有返回账号信息")?;
        let user = checked_user(id, &client, &self.nonce)?;
        let mut account = account_from(&reply, &client, &self.host, &user)?;
        account["plan"] = json!(plan(&[id, account["access"].as_str().unwrap_or("")]));
        save_login(&self.root, generation, account)
    }
}

fn save_login(root: &Path, generation: u64, account: Value) -> Result<(), String> {
    storage::edit_keys(root, |keys| {
        let flow = flow();
        if flow.generation != generation {
            return Err("请先用 ChatGPT 账号登录".into());
        }
        keys["chatgpt"] = account;
        Ok(())
    })
}

/// The query of a `GET /auth/callback`, anything else being none of this sign-in's business.
fn callback_query(stream: &mut TcpStream) -> Option<String> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut buffer = [0; 8192];
    let read = stream.read(&mut buffer).ok()?;
    let head = String::from_utf8_lossy(&buffer[..read]);
    let target = head
        .lines()
        .next()?
        .strip_prefix("GET ")?
        .split(' ')
        .next()?;
    target.strip_prefix("/auth/callback?").map(str::to_string)
}

fn value(query: &str, name: &str) -> Option<String> {
    let url = reqwest::Url::parse(&format!("http://localhost/?{query}")).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn answer(stream: &mut TcpStream, status: &str, text: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>AgentNave</title><body style=\"font:15px system-ui;padding:40px\">{text}"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn post_token(url: &str, form: &[(&str, &str)]) -> Result<Value, String> {
    response_value(
        summary_client()?
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(reqwest::header::ACCEPT, "application/json")
            .body(query(form)),
        "",
    )
}

/// The account in an ID token, once it proves to be this sign-in's, from OpenAI, for this app.
fn checked_user(id_token: &str, client: &str, nonce: &str) -> Result<String, String> {
    let claims = id_token
        .split('.')
        .nth(1)
        .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .ok_or("OpenAI 返回的账号信息无法读取")?;
    let issuer = claims["iss"].as_str().unwrap_or("").trim_end_matches('/');
    let audience = claims["aud"].as_str() == Some(client)
        || claims["aud"]
            .as_array()
            .is_some_and(|all| all.iter().any(|one| one.as_str() == Some(client)));
    if issuer != ISSUER || !audience || claims["nonce"].as_str() != Some(nonce) {
        return Err("OpenAI 返回的账号信息与这次登录不符".into());
    }
    if claims["exp"].as_u64().is_some_and(|exp| exp + 60 < now()) {
        return Err("OpenAI 返回的账号信息已过期".into());
    }
    claims["email"]
        .as_str()
        .or(claims["sub"].as_str())
        .map(str::to_string)
        .ok_or("OpenAI 没有说明登录的账号".into())
}

/// The ChatGPT plan a token names, "" when none does.
fn plan(tokens: &[&str]) -> String {
    tokens
        .iter()
        .filter_map(|token| token.split('.').nth(1))
        .filter_map(|part| URL_SAFE_NO_PAD.decode(part).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .find_map(|claims| {
            claims["https://api.openai.com/auth"]["chatgpt_plan_type"]
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn account_from(reply: &Value, client: &str, host: &str, user: &str) -> Result<Value, String> {
    let access = reply["access_token"].as_str().filter(|t| !t.is_empty());
    let refresh = reply["refresh_token"].as_str().filter(|t| !t.is_empty());
    let (Some(access), Some(refresh)) = (access, refresh) else {
        return Err("OpenAI 没有返回令牌".into());
    };
    let lifetime = reply["expires_in"]
        .as_f64()
        .filter(|seconds| *seconds > 0.0);
    let lifetime = lifetime.ok_or("OpenAI 的令牌没有有效期")?;
    let granted = reply["scope"].as_str().unwrap_or("");
    if !granted
        .split_whitespace()
        .any(|scope| scope == DIRECT_SCOPE)
    {
        return Err("这个 ChatGPT 账号不能通过 API 使用订阅".into());
    }
    Ok(json!({
        "client": client,
        "host": host,
        "user": user,
        "access": access,
        "refresh": refresh,
        "expires": now() + lifetime as u64,
    }))
}

/// A token that isn't about to expire. The refresh token rotates, so only one refresh runs.
pub(super) fn access_token(root: &Path) -> Result<String, String> {
    let _guard = REFRESH.lock().unwrap_or_else(|e| e.into_inner());
    let account = storage::read_keys(root)?["chatgpt"].clone();
    let (Some(access), Some(refresh), Some(client)) = (
        account["access"].as_str(),
        account["refresh"].as_str(),
        account["client"].as_str(),
    ) else {
        return Err("请先用 ChatGPT 账号登录".into());
    };
    if account["expires"].as_u64().unwrap_or(0) > now() + MARGIN_SECONDS {
        return Ok(access.to_string());
    }
    let reply = post_token(
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", refresh),
            ("resource", API),
        ],
    );
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) if DEAD.iter().any(|code| error.contains(code)) => {
            return Err("ChatGPT 登录已失效，请重新登录".into());
        }
        Err(error) if error.contains("invalid_client") => {
            return Err("OpenAI 已不认这次登录注册的应用，请重新登录".into());
        }
        // A hiccup: the token in hand still works until it expires.
        Err(_) if account["expires"].as_u64().unwrap_or(0) > now() => {
            return Ok(access.to_string());
        }
        Err(error) => return Err(format!("ChatGPT 令牌刷新失败：{error}")),
    };
    let host = account["host"].as_str().unwrap_or("");
    let user = account["user"].as_str().unwrap_or("");
    let mut fresh = account_from(&reply, client, host, user)?;
    let tokens = [
        reply["id_token"].as_str().unwrap_or(""),
        fresh["access"].as_str().unwrap_or(""),
    ];
    let renewed = plan(&tokens);
    fresh["plan"] = if renewed.is_empty() {
        account["plan"].clone()
    } else {
        json!(renewed)
    };
    save_refresh(root, refresh, &fresh)?;
    Ok(fresh["access"].as_str().unwrap_or("").to_string())
}

fn save_refresh(root: &Path, refresh: &str, fresh: &Value) -> Result<(), String> {
    storage::edit_keys(root, |keys| {
        if keys["chatgpt"]["refresh"].as_str() != Some(refresh) {
            return Err("请先用 ChatGPT 账号登录".into());
        }
        keys["chatgpt"] = fresh.clone();
        Ok(())
    })
}

/// Signed out here at once; OpenAI revoking the token is best effort.
pub(in crate::voice) fn sign_out(root: &Path) -> Result<(), String> {
    let mut account = Value::Null;
    storage::edit_keys(root, |keys| {
        // Match sign-in's lock order: storage WRITE, then FLOW. No callback can restore
        // the account between its removal and invalidating this sign-in generation.
        let mut flow = flow();
        flow.generation += 1;
        flow.signing_in = false;
        flow.error = None;
        if let Some(keys) = keys.as_object_mut() {
            account = keys.remove("chatgpt").unwrap_or(Value::Null);
        }
        Ok(())
    })?;
    std::thread::spawn(move || {
        let (Some(refresh), Some(client)) =
            (account["refresh"].as_str(), account["client"].as_str())
        else {
            return;
        };
        let _ = revoke(refresh, client);
    });
    Ok(())
}

fn revoke(refresh: &str, client: &str) -> Result<(), String> {
    let found = response_value(
        summary_client()?.get(format!("{ISSUER}/.well-known/openid-configuration")),
        "",
    )?;
    let endpoint = found["revocation_endpoint"].as_str().ok_or("no endpoint")?;
    http(
        summary_client()?
            .post(endpoint)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body(query(&[
                ("token", refresh),
                ("token_type_hint", "refresh_token"),
                ("client_id", client),
            ])),
        "",
    )
    .map(|_| ())
}

/// Sign in with ChatGPT takes Responses only: stored nowhere, streamed, with no token limit.
pub(super) fn respond(
    token: &str,
    model: &str,
    text: &str,
    reasoning_effort: Option<&str>,
    system: &str,
) -> Result<String, String> {
    let input = json!([{"type":"message","role":"user",
        "content":[{"type":"input_text","text":user_message(text)}]}]);
    let key = conversation_key(system, &input);
    let mut body = json!({
        "model":model,
        "instructions":system,
        "store":false,
        "stream":true,
        "input":input,
        "prompt_cache_key":key
    });
    if let Some(effort) = reasoning_effort {
        body["reasoning"] = json!({"effort":effort});
    }
    // OpenAI finds a conversation's cached prompt by these headers, not by the body's key.
    let bytes = http(
        client()?
            .post(format!("{API}/responses"))
            .bearer_auth(token)
            .header("session-id", &key)
            .header("thread-id", &key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string()),
        token,
    )
    .map_err(explain)?;
    clean_output(&stream_text(&String::from_utf8_lossy(&bytes))?)
}

/// A conversation's name from how it starts, as magpie gives one to a client that sends none.
fn conversation_key(instructions: &str, input: &Value) -> String {
    let mut hash = sha2::Sha256::new();
    hash.update(instructions.as_bytes());
    for item in input.as_array().into_iter().flatten() {
        hash.update(item.to_string().as_bytes());
        if item["role"] == "user" {
            break;
        }
    }
    let hex: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// What OpenAI's refusal of a ChatGPT token means for the user.
fn explain(error: String) -> String {
    let said = |code: &str| error.contains(code);
    if said("subscription_sharing_usage_limit_exceeded") {
        "ChatGPT 订阅额度已用完（与 ChatGPT 及其他使用它的应用共用）".into()
    } else if said("subscription_sharing_usage_unavailable")
        || said("subscription_sharing_user_unavailable")
    {
        "OpenAI 暂时无法确认这个账号的额度，请稍后再试".into()
    } else if said("subscription_sharing_user_not_eligible") {
        "这个 ChatGPT 账号的套餐不能通过 API 使用".into()
    } else if said("subscription_sharing_unsupported_capability")
        || said("subscription_sharing_route_not_supported")
    {
        "ChatGPT 登录不能使用这项请求".into()
    } else if said("subscription_sharing_invalid_user") || said("401") {
        "ChatGPT 登录已失效，请重新登录".into()
    } else if said("chatpass_v2_") {
        "ChatGPT 拒绝了这个账号".into()
    } else {
        error
    }
}

/// The account's list for pickers, then the models Codex lists for a ChatGPT plan that it leaves
/// out: the token is served as Codex on the plan and runs Codex's newest models too.
pub(super) fn models(token: &str) -> Result<Vec<SummaryModel>, String> {
    let value = response_value(
        request(summary_client()?.get(format!("{API}/models")), token, API),
        token,
    )
    .map_err(explain)?;
    let hidden: Vec<&str> = value["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["visibility"].as_str().is_some_and(|v| v != "list"))
        .filter_map(|item| item["slug"].as_str())
        .collect();
    let mut models = parse_models(&catalog_items(&value).unwrap_or_default());
    for model in codex_models() {
        let known = models.iter().any(|m| m.id == model.id);
        if !known && !model.id.contains('/') && !hidden.contains(&model.id.as_str()) {
            models.push(model);
        }
    }
    if models.is_empty() {
        return Err("ChatGPT 没有为这个账号列出模型".into());
    }
    Ok(models)
}

/// The models Codex CLI last cached, in its order.
fn codex_models() -> Vec<SummaryModel> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
    let Some(cache) = home
        .and_then(|home| std::fs::read(home.join("models_cache.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return Vec::new();
    };
    let mut items: Vec<&Value> = cache["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["visibility"] != "hide")
        .collect();
    items.sort_by_key(|item| item["priority"].as_i64().unwrap_or(0));
    parse_models(&items)
}

/// The effort to ask for: the user's pick when the model has it, else its lowest.
pub(super) fn effort(token: &str, model: &str, selected: &str) -> Result<Option<String>, String> {
    let levels = models(token)
        .ok()
        .and_then(|models| models.into_iter().find(|m| m.id == model))
        .map(|m| m.reasoning_levels);
    if selected.is_empty() {
        return Ok(levels.as_deref().and_then(lowest));
    }
    match levels {
        Some(levels) if levels.iter().any(|level| level == selected) => Ok(Some(selected.into())),
        _ => Err(format!("所选摘要模型不支持努力程度“{selected}”")),
    }
}

/// The text of a Responses event stream.
fn stream_text(stream: &str) -> Result<String, String> {
    let mut text = String::new();
    for data in stream.lines().filter_map(|line| line.strip_prefix("data:")) {
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        match event["type"].as_str().unwrap_or("") {
            "response.output_text.delta" => text.push_str(event["delta"].as_str().unwrap_or("")),
            "response.failed" | "error" => {
                let reason = event["response"]["error"]["message"]
                    .as_str()
                    .or(event["message"].as_str())
                    .unwrap_or("摘要失败");
                return Err(format!("ChatGPT 返回错误：{reason}"));
            }
            _ => {}
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_out_rejects_in_flight_refreshes_and_login_callbacks() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let generation = flow().generation;
        // No client: sign_out cannot send a revocation request in this test.
        save_login(root, generation, json!({"refresh":"old"})).unwrap();
        let fresh = json!({"refresh":"new","access":"new-access"});
        save_refresh(root, "old", &fresh).unwrap();
        assert!(save_refresh(root, "old", &fresh).is_err());
        sign_out(root).unwrap();
        let signed_out = std::fs::read(root.join("voice-keys.json")).unwrap();
        assert_eq!(
            save_refresh(root, "new", &fresh).unwrap_err(),
            "请先用 ChatGPT 账号登录"
        );
        assert!(save_login(root, generation, fresh).is_err());
        assert!(storage::read_keys(root).unwrap()["chatgpt"].is_null());
        assert_eq!(
            std::fs::read(root.join("voice-keys.json")).unwrap(),
            signed_out
        );
    }

    fn id_token(claims: &Value) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(claims.to_string()))
    }

    #[test]
    fn an_id_token_must_be_this_sign_ins() {
        let good = json!({"iss":format!("{ISSUER}/"),"aud":["app"],"nonce":"n","email":"a@b.c"});
        assert_eq!(checked_user(&id_token(&good), "app", "n").unwrap(), "a@b.c");
        for (key, wrong) in [("iss", "https://evil"), ("aud", "other"), ("nonce", "x")] {
            let mut claims = good.clone();
            claims[key] = json!(wrong);
            assert!(checked_user(&id_token(&claims), "app", "n").is_err());
        }
    }

    #[test]
    fn the_subscription_scope_is_required() {
        let mut reply =
            json!({"access_token":"a","refresh_token":"r","expires_in":60,"scope":"openid"});
        assert!(account_from(&reply, "c", "h", "u").is_err());
        reply["scope"] = json!(SCOPES);
        assert_eq!(account_from(&reply, "c", "h", "u").unwrap()["refresh"], "r");
    }

    #[test]
    fn the_authorize_address_carries_pkce_and_registers_this_app() {
        let url = authorize_url("http://127.0.0.1:1455/auth/callback", "s", "n", "v", "h");
        for (name, expected) in [
            ("client_id", DYNAMIC_CLIENT),
            ("agent_name_hint", AGENT_NAME),
            ("code_challenge_method", "S256"),
            ("resource", API),
        ] {
            assert_eq!(
                value(url.split_once('?').unwrap().1, name).unwrap(),
                expected
            );
        }
        assert!(!url.contains("code_verifier"));
    }

    #[test]
    fn a_stream_yields_its_text_or_its_failure() {
        let good = "event: x\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"你\"}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"好\"}\n\ndata: {\"type\":\"response.completed\"}\n";
        assert_eq!(stream_text(good).unwrap(), "你好");
        let bad =
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"no\"}}}\n";
        assert!(stream_text(bad).unwrap_err().contains("no"));
    }

    #[test]
    fn a_plan_and_a_conversation_key_come_out_as_magpie_reads_them() {
        let claims = json!({"https://api.openai.com/auth":{"chatgpt_plan_type":"plus"}});
        assert_eq!(plan(&["", &id_token(&claims)]), "plus");
        let input = json!([{"role":"developer"},{"role":"user","content":"a"},{"role":"user"}]);
        let key = conversation_key("s", &input);
        assert_eq!(key.len(), 36);
        let shorter = json!([{"role":"developer"},{"role":"user","content":"a"}]);
        assert_eq!(
            key,
            conversation_key("s", &shorter),
            "only the start names a conversation"
        );
    }

    #[test]
    fn the_page_never_hears_a_token() {
        let keys = json!({"chatgpt":{"access":"secret-a","refresh":"secret-r","user":"a@b.c"}});
        let shown = state(&keys).to_string();
        assert!(shown.contains("a@b.c") && !shown.contains("secret"));
    }
}
