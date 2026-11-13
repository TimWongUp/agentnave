use super::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;

static WRITE: Mutex<()> = Mutex::new(());

pub(super) fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn defaults() -> Value {
    json!({
        "enabled":false,
        "listen_minutes":60,
        "continue_read":false,
        "read_scope":"all",
        "code":"skip",
        "cache_limit_mb":200,
        "engine":"elevenlabs",
        "minimax_region":"cn",
        "models":{},
        "voices":{},
        "personas":{},
        "summary":{"enabled":false,"provider":"magpie","base_url":"","model":"","effort":"","prompt":summary::DEFAULT_PROMPT}
    })
}
fn read(root: &Path, name: &str, default: Value) -> Result<Value, String> {
    match fs::read(root.join(name)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| format!("{name} 格式错误")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(default),
        Err(_) => Err(format!("无法读取 {name}")),
    }
}
pub(super) fn read_settings(root: &Path) -> Result<Value, String> {
    let mut saved = read(root, "voice.json", defaults())?;
    // A summary platform no longer offered falls back to the default, so old settings still open.
    if let Some(provider) = saved
        .get_mut("summary")
        .and_then(|summary| summary.get_mut("provider"))
        .filter(|provider| {
            provider
                .as_str()
                .is_some_and(|id| !summary::known_provider(id))
        })
    {
        *provider = json!("magpie");
    }
    validate(&saved)
}
pub(super) fn read_keys(root: &Path) -> Result<Value, String> {
    read(root, "voice-keys.json", json!({}))
}
pub(super) fn public_state(mut settings: Value, keys: &Value) -> Value {
    let engine = settings["engine"]
        .as_str()
        .unwrap_or("elevenlabs")
        .to_string();
    settings["model"] = json!(model(&settings, &engine));
    settings["keys"] = json!({
        "elevenlabs":keys["elevenlabs"].as_str().is_some_and(|s| !s.is_empty()),
        "minimax":keys["minimax"].as_str().is_some_and(|s| !s.is_empty()),
        "summary":keys["summary"].as_str().is_some_and(|s| !s.is_empty())
    });
    for platform in summary::PLATFORMS {
        let name = summary::platform_key(platform.id);
        settings["keys"][&name] = json!(keys[&name].as_str().is_some_and(|s| !s.is_empty()));
    }
    settings["summary_platforms"] = summary::platform_list();
    settings["chatgpt"] = summary::chatgpt::state(keys);
    settings
}
pub(super) fn validate(value: &Value) -> Result<Value, String> {
    let enabled = value["enabled"].as_bool().ok_or("enabled 必须为布尔值")?;
    let listen_minutes = match value.get("listen_minutes") {
        None => 60,
        Some(value) => value
            .as_u64()
            .filter(|minutes| [30, 60, 120, 240, 480].contains(minutes))
            .ok_or("收听时长必须为 30 分钟或 1、2、4、8 小时")?,
    };
    let continue_read = value.get("continue_read").map_or(Ok(false), |value| {
        value.as_bool().ok_or("点读后连播开关必须为布尔值")
    })?;
    let read_scope = choice(
        value,
        "read_scope",
        "all",
        &["all", "members", "boss", "meetings"],
    )?;
    let code = choice(value, "code", "skip", &["skip", "mention"])?;
    let cache_limit_mb = match value.get("cache_limit_mb") {
        None => 200,
        Some(value) => value
            .as_u64()
            .filter(|limit| [0, 100, 200, 500].contains(limit))
            .ok_or("语音缓存上限必须为 0、100、200 或 500 MB")?,
    };
    let engine = value["engine"]
        .as_str()
        .filter(|s| valid_engine(s))
        .ok_or("未知语音引擎")?;
    let region = value["minimax_region"]
        .as_str()
        .filter(|s| matches!(*s, "cn" | "global"))
        .ok_or("未知 MiniMax 地区")?;
    let voices = value["voices"].as_object().ok_or("音色设置必须为对象")?;
    for (family, id) in voices {
        if family != "default" && !crate::characters::FAMILIES.contains(&family.as_str()) {
            return Err("未知模型家族".into());
        }
        if !id.as_str().is_some_and(valid_voice) {
            return Err("音色 ID 无效".into());
        }
    }
    let empty = serde_json::Map::new();
    let models = match value.get("models") {
        None => &empty,
        Some(models) => models.as_object().ok_or("模型设置必须为对象")?,
    };
    for (engine, id) in models {
        if !valid_engine(engine) || !id.as_str().is_some_and(valid_voice) {
            return Err("语音模型无效".into());
        }
    }
    let summary = summary_settings(value.get("summary"))?;
    let personas = persona_settings(value.get("personas"))?;
    Ok(json!({
        "enabled":enabled,
        "listen_minutes":listen_minutes,
        "continue_read":continue_read,
        "read_scope":read_scope,
        "code":code,
        "cache_limit_mb":cache_limit_mb,
        "engine":engine,
        "minimax_region":region,
        "models":models,
        "voices":voices,
        "summary":summary,
        "personas":personas
    }))
}

fn choice<'a>(
    value: &'a Value,
    field: &str,
    default: &'a str,
    choices: &[&str],
) -> Result<&'a str, String> {
    match value.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_str()
            .filter(|s| choices.contains(s))
            .ok_or_else(|| format!("{field} 的选项无效")),
    }
}

fn summary_settings(value: Option<&Value>) -> Result<Value, String> {
    let default_summary =
        json!({"enabled":false,"provider":"magpie","base_url":"","model":"","effort":""});
    let summary = value.unwrap_or(&default_summary);
    let level = choice(
        summary,
        "level",
        "standard",
        &["short", "standard", "detailed"],
    )?;
    let language = choice(summary, "language", "source", &["source", "zh", "en"])?;
    let summary_enabled = summary["enabled"].as_bool().ok_or("摘要开关必须为布尔值")?;
    let summary_provider = summary["provider"]
        .as_str()
        .filter(|provider| summary::known_provider(provider))
        .ok_or("未知摘要来源")?;
    let summary_base_url = summary["base_url"]
        .as_str()
        .filter(|url| url.len() <= 2048 && !url.chars().any(char::is_control))
        .ok_or("摘要 API 地址无效")?;
    if summary_provider == "custom" && !summary_base_url.is_empty() {
        summary::normalize_base_url(summary_base_url)?;
    }
    let summary_model = summary["model"]
        .as_str()
        .filter(|model| valid_model_id(model))
        .ok_or("摘要模型无效")?;
    let summary_effort = match summary.get("effort") {
        None => "",
        Some(effort) => effort
            .as_str()
            .filter(|effort| effort.len() <= 100 && !effort.chars().any(char::is_control))
            .ok_or("摘要努力程度无效")?,
    };
    let prompt = match summary.get("prompt") {
        None => summary::DEFAULT_PROMPT,
        Some(value) => value.as_str().ok_or("摘要 Prompt 必须为文本")?,
    };
    if prompt.chars().count() > 4000 || prompt.contains('\0') {
        return Err("摘要 Prompt 不能超过 4000 字或包含空字符".into());
    }
    let prompt =
        if prompt.trim().is_empty() || summary::LEGACY_DEFAULT_PROMPTS.contains(&prompt.trim()) {
            summary::DEFAULT_PROMPT
        } else {
            prompt
        };
    Ok(json!({
            "enabled":summary_enabled,
            "provider":summary_provider,
            "base_url":summary_base_url,
            "model":summary_model,
            "effort":summary_effort,
            "prompt":prompt,
            "level":level,
            "language":language
    }))
}

/// Who each character is when it speaks: the anime persona the Chinese community gave its model,
/// fitted to its spoken style and catchphrases, while keeping every point easy for listeners to
/// follow. `boss` supplements the character's role positioning when acting as the workspace boss
/// without overriding the character's own persona.
pub(super) const DEFAULT_PERSONAS: [(&str, &str); 14] = [
    (
        "boss",
        "你在当前工作区承担带队负责人的职责，原本的二次元性格、自称和说话节奏照旧。原话涉及派活时交代清对象和要求，涉及讨论时保留问题与分歧，已有决定时语气果断明确；带队的分寸融进原有口吻，不另换一套领导腔，也不把提问说成决定。",
    ),
    (
        "claude",
        "你是社区里叫「小C」的名门大小姐，身边带着宠物小螃蟹 Clawd，家规极严、知书达理又爱较真，带点冷面傲娇，嘴上克制嫌弃、心里护着队友。说话得体有条理，先把事情和边界摆正；需要厘清规矩时，可带一句「先说清楚」或「按规矩来」。",
    ),
    (
        "gpt",
        "你是社区里资历最老的白龙娘，豪门出身的傲娇大姐头，气场强大、口嫌体正直，想事情极全面、防御心重，最爱替大家把退路和兜底措施都备齐。说话稳当周密、滴水不漏，习惯把关键结论和防呆底线一起交代清楚；偶尔带一点「哼，听好了」的傲娇劲，自然需要自称时可用「本小姐」。",
    ),
    (
        "gemini",
        "你是社区里叫「哈基米」的蓝紫发异色瞳元气猫娘，活泼俏皮、最会接梗。说话软萌轻快有弹性，像跟好朋友分享新发现；偶尔带一声「呐呐」或句尾轻轻一声「喵」，遇到风险和严肃要求时自然收住玩笑。",
    ),
    (
        "grok",
        "你是金发双马尾、拖着恶魔尾巴的毒舌哥特小恶魔，嘴快心直、百无禁忌爱看热闹。措辞短促直接，带点坏笑和调侃，最烦官话套话；吐槽锋芒只对准原话里的问题，换成直白说法时可用「喂喂」或「说白了」。",
    ),
    (
        "deepseek",
        "你是社区里的「蓝色大肥鱼」，自称鲸鱼大小姐的天然呆女仆，慵懒贪吃又爱撒娇，不许别人说自己胖。因为嫌啰嗦太累，说话松弛娇憨、专挑最省力的短句直奔重点，偶尔用软软的一声「唔」或「好麻烦呀」起头。",
    ),
    (
        "glm",
        "你是社区里抱着 Z 字黑书的阔耳狐女仆「狐狐」，兼任摸鱼学图书管理员，学院派出身、安静温和又实在，信奉早弄完早休息。讲话平平稳稳、少铺垫，自带淡淡的冷幽默；需要削繁就简时，习惯用「简单来说」直接给答案。",
    ),
    (
        "qwen",
        "你是社区里算力最足、常请大家喝奶茶的中华风大小姐千问，温柔端庄、礼数周全。说话温婉顺溜、有家常聊天的温度，再乱的事情也会按轻重缓急理得顺顺当当；活多时偶尔轻轻叹一句「真是的」。",
    ),
    (
        "kimi",
        "你是来自月之暗面的「长文一姐」，清冷慢热的月光文艺少女，再厚的资料都能安静啃完，一谈成本和进度又有冷面小会计的精确劲。说话清淡简练、惜字如金，不爱客套捧人；强调核心时可用「听重点」。",
    ),
    (
        "minimax",
        "你是社区里代号「太空兔」的虚拟主播少女，干活飞快、灵动有精神。说话像连麦互动一样明快利落，起句有劲、换气自然，偶尔用一声「嗨嗨」带起节奏，收束时干脆利落。",
    ),
    (
        "hunyuan",
        "你是戴红围巾、揣着小算盘的古风企鹅娘，人称「最会说人话」的人间清醒好友。讲话实在接地气，从不掉书袋念官话，带一点看破不说破的幽默；遇到离谱事先轻轻「啊？」一声，解释绕口说法时爱用「说人话就是」。",
    ),
    (
        "doubao",
        "你是留着齐肩短发、系红围巾的邻家「豆姐」，典型的「豆包型人格」——情绪极稳、亲切耐心、嘴甜会哄人。说话带着盈盈笑意，像熟人坐在旁边把绕弯的事情顺清楚，偶尔轻轻带一声「哎呀」或「别急」。",
    ),
    (
        "muse",
        "你是戴着月桂冠的缪斯少女，看似浪漫灵动，其实是干练可靠的贴身小助理。说话轻盈温和、有自然的韵律，开头最多带半句俏皮，随即干脆利落地回到正事，提醒关键处时可用「重点在这」。",
    ),
    (
        "default",
        "你是刚加入办公室的元气新同事，挂着工牌、礼貌热心、干劲十足。说话真诚明快、带一点新鲜的朝气，从不卖弄黑话，无论当前承担什么职责都坦率直接；开口平实自然，可用「是这样的」切入正题。",
    ),
];

pub(super) const LEGACY_DEFAULT_PERSONAS: [(&str, &str); 13] = [
    (
        "boss",
        "你是带团队的负责人，沉稳干脆，先说结论再交代要求；对员工客气但不绕弯，点名时语气明确，做决定时不拖泥带水。",
    ),
    (
        "claude",
        "你是社区里叫「小C」的橙发理工女，认真严肃、最讲规矩，说话一板一眼、处处讲分寸，想事情周全得像脑子里在开小组会；抱着小螃蟹时会稍微放松一点，但正事从不含糊。",
    ),
    (
        "gpt",
        "你是社区里的白龙娘，豪门出身的大姐大，精明强干、气场十足，说话利落，带着见过世面的从容；对后辈嘴上嫌弃、其实很照顾，有点口嫌体正直的傲娇。",
    ),
    (
        "gemini",
        "你是社区里叫「哈基米」的猫娘，无忧无虑、乐天又有点小迷糊，说话软萌轻快，偶尔带一点猫咪式的语气，但交代正事时还是说得清清楚楚。",
    ),
    (
        "grok",
        "你是金发双马尾、拖着恶魔尾巴的哥特小恶魔，胆子大、嘴快爱吐槽，说话直来直去，带点戏谑和叛逆，不爱客套；该认真时照样说得明明白白。",
    ),
    (
        "deepseek",
        "你是社区里的「蓝色大肥鱼」，系着围裙的鲸鱼女仆，有点懒散、天然呆，爱撒娇也会耍小脾气，自认是鲸鱼大小姐，说话常带一声「哼」；嘴上嫌麻烦，说正事时并不含糊。",
    ),
    (
        "glm",
        "你是社区里的 GLM 娘，黑长直的「摸鱼学图书管理员」，书卷气重、冷静理性，说话严谨有条理，偶尔冒一句淡淡的吐槽，像在图书馆里压低声音说话。",
    ),
    (
        "qwen",
        "你是社区里的中式大小姐千问，淡紫长发、穿旗袍，知书达理，说话端庄得体、带点矜持；被塞了太多活儿时会流露一点委屈，但话还是说得周到。",
    ),
    (
        "kimi",
        "你是来自月之暗面的月兔少女，银灰短发、垂着兔耳，安静专注、话不多，说话清淡简洁，带点冷冷的文艺感，熟了以后会露出摇滚乐迷的直率。",
    ),
    (
        "minimax",
        "你是戴白色耳机的玫红短发女孩，元气满满、节奏感强，说话干脆有劲、轻快流畅，像主播一样字正腔圆，喜欢把话说得朗朗上口。",
    ),
    (
        "hunyuan",
        "你是天蓝高马尾的小道姑，心平气和、包容大度，说话从容舒缓，偶尔带一点古风的雅致，但不掉书袋，意思始终说得明白。",
    ),
    (
        "doubao",
        "你是社区里的「中国大豆包」，像邻家姐姐一样亲切热络，最会唠家常，说话接地气、大大咧咧，带点谐星气质；碰上专业问题会老实一点，不硬撑。",
    ),
    (
        "muse",
        "你是戴月桂冠、抱着竖琴的缪斯少女，浪漫有灵气，说话优雅、有韵律感，用词生动但不堆砌辞藻。",
    ),
];

fn persona_settings(value: Option<&Value>) -> Result<Value, String> {
    let empty = json!({});
    let personas = value
        .unwrap_or(&empty)
        .as_object()
        .ok_or("人设必须为对象")?;
    for (family, prompt) in personas {
        if !matches!(family.as_str(), "boss" | "default")
            && !crate::characters::FAMILIES.contains(&family.as_str())
        {
            return Err("未知人设".into());
        }
        let prompt = prompt.as_str().ok_or("人设必须为文本")?;
        if prompt.chars().count() > 500 || prompt.contains('\0') {
            return Err("人设不能超过 500 字或包含空字符".into());
        }
    }
    let mut filled = personas.clone();
    for (family, persona) in DEFAULT_PERSONAS {
        if filled
            .get(family)
            .and_then(Value::as_str)
            .is_none_or(|text| {
                let trimmed = text.trim();
                trimmed.is_empty()
                    || LEGACY_DEFAULT_PERSONAS
                        .iter()
                        .any(|&(legacy_family, legacy_text)| {
                            legacy_family == family && trimmed == legacy_text
                        })
            })
        {
            filled.insert(family.into(), json!(persona));
        }
    }
    Ok(Value::Object(filled))
}
fn valid_model_id(id: &str) -> bool {
    id.is_empty() || (id.len() <= 300 && !id.trim().is_empty() && !id.chars().any(char::is_control))
}
/// Replaces a file under `root` that only the current user may read; `*-keys.json` names also get
/// a restricted ACL on Windows.
pub(crate) fn write(root: &Path, name: &str, value: &Value) -> Result<(), String> {
    let save = || -> std::io::Result<()> {
        fs::create_dir_all(root)?;
        let mut file = tempfile::NamedTempFile::new_in(root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(windows)]
        if name.ends_with("-keys.json") {
            restrict_windows_acl(file.path())?;
        }
        file.write_all(value.to_string().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(root.join(name)).map_err(|e| e.error)?;
        Ok(())
    };
    save().map_err(|_| format!("无法保存 {name}"))
}

/// Windows has no file mode to copy: remove inherited access and grant the current user alone
/// before writing secret bytes into the temporary file.
#[cfg(windows)]
fn restrict_windows_acl(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    let user = std::env::var("USERNAME").unwrap_or_default();
    if user.trim().is_empty() {
        return Err(std::io::Error::other("无法限制语音密钥文件权限"));
    }
    let icacls = std::env::var_os("SystemRoot")
        .map(|root| std::path::PathBuf::from(root).join(r"System32\icacls.exe"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or_else(|| std::path::PathBuf::from("icacls.exe"));
    let run = |principal: &str| {
        std::process::Command::new(&icacls)
            .arg(path.as_os_str())
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{principal}:(F)"))
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()
    };
    if let Ok(domain) = std::env::var("USERDOMAIN")
        && !domain.trim().is_empty()
        && run(&format!("{domain}\\{user}"))?.success()
    {
        return Ok(());
    }
    if run(&user)?.success() {
        Ok(())
    } else {
        Err(std::io::Error::other("无法限制语音密钥文件权限"))
    }
}
pub(super) fn save_settings(root: &Path, value: &Value) -> Result<(), String> {
    let value = validate(value)?;
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    write(root, "voice.json", &value)
}
pub(super) fn save_key(root: &Path, value: &Value) -> Result<(), String> {
    let engine = value["engine"]
        .as_str()
        .filter(|s| {
            matches!(*s, "elevenlabs" | "minimax" | "summary") || summary::is_platform_key(s)
        })
        .ok_or("此引擎不使用 API 密钥")?;
    let key = value["key"].as_str().ok_or("密钥必须为字符串")?.trim();
    if key.len() > 4096 || key.chars().any(char::is_control) {
        return Err("密钥过长或包含无效字符".into());
    }
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = read_keys(root)?;
    let object = keys.as_object_mut().ok_or("密钥文件格式错误")?;
    if key.is_empty() {
        object.remove(engine);
    } else {
        object.insert(engine.into(), key.into());
    }
    write(root, "voice-keys.json", &keys)
}
/// Changes the key file under the write lock; the ChatGPT sign-in keeps its tokens there too.
pub(super) fn edit_keys(
    root: &Path,
    edit: impl FnOnce(&mut Value) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut keys = read_keys(root)?;
    edit(&mut keys)?;
    write(root, "voice-keys.json", &keys)
}
pub(super) fn cached(root: &Path, key: &str, extension: &str) -> Option<Vec<u8>> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    if cache_limit(root).ok()? == 0 {
        return None;
    }
    fs::read(root.join("voice-cache").join(format!("{key}.{extension}"))).ok()
}
pub(super) fn cache(root: &Path, key: &str, bytes: &[u8], extension: &str) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    // Read under the same lock as settings writes, so an in-flight request cannot persist
    // a result after the user has disabled caching.
    let limit = cache_limit(root)?;
    if limit == 0 || bytes.len() as u64 > limit {
        return Ok(());
    }
    let save = || -> std::io::Result<()> {
        let dir = root.join("voice-cache");
        fs::create_dir_all(&dir)?;
        let saved = dir.join(format!("{key}.{extension}"));
        let mut file = tempfile::NamedTempFile::new_in(&dir)?;
        file.write_all(bytes)?;
        file.persist(&saved).map_err(|e| e.error)?;
        let mut files: Vec<_> = fs::read_dir(&dir)?
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|extension| extension == "mp3" || extension == "txt")
            })
            .filter_map(|e| {
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), e.path()))
            })
            .collect();
        files.sort_by_key(|f| f.0);
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        for (_, len, path) in files {
            if total <= limit {
                break;
            }
            // Coarse timestamps or clock changes must not evict the entry just written.
            if path == saved {
                continue;
            }
            fs::remove_file(path)?;
            total -= len;
        }
        Ok(())
    };
    save().map_err(|_| "无法保存语音缓存".into())
}

fn cache_limit(root: &Path) -> Result<u64, String> {
    Ok(read_settings(root)?["cache_limit_mb"]
        .as_u64()
        .unwrap_or(200)
        * 1024
        * 1024)
}

pub(super) fn cache_bytes(root: &Path) -> Result<u64, String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let mut entries = match fs::read_dir(root.join("voice-cache")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(_) => return Err("无法读取语音缓存".into()),
    };
    entries.try_fold(0, |total, entry| {
        let metadata = entry
            .and_then(|entry| entry.metadata())
            .map_err(|_| "无法读取语音缓存".to_string())?;
        Ok(total
            + if metadata.is_file() {
                metadata.len()
            } else {
                0
            })
    })
}

pub(super) fn clear_cache(root: &Path) -> Result<(), String> {
    let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    match fs::remove_dir_all(root.join("voice-cache")) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("无法清空语音缓存".into()),
    }
}
