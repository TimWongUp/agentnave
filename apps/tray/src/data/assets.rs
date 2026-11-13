use super::*;

const PANEL_HTML: &[u8] = include_bytes!("../../ui/panel.html");
const PANEL_JS: &[u8] = include_bytes!("../../ui/panel.js");
const PANEL_CSS: &[u8] = include_bytes!("../../ui/panel.css");
const MANAGE_CSS: &[u8] = include_bytes!("../../ui/manage.css");
const THEME_CSS: &[u8] = include_bytes!("../../ui/theme.css");
const SETTINGS_HTML: &[u8] = include_bytes!("../../ui/settings.html");
const SETTINGS_JS: &[u8] = include_bytes!("../../ui/settings.js");
const SETTINGS_CSS: &[u8] = include_bytes!("../../ui/settings.css");
const CHAT_HTML: &[u8] = include_bytes!("../../ui/chat.html");
const CHAT_JS: &[u8] = include_bytes!("../../ui/chat.js");
const CHAT_CSS: &[u8] = include_bytes!("../../ui/chat.css");
const APPEARANCE_JS: &[u8] = include_bytes!("../../ui/appearance.js");
const COMMON_JS: &[u8] = concat!(
    "\"use strict\";\nconst AGENTNAVE_ART_VERSION = \"",
    env!("CARGO_PKG_VERSION"),
    "-",
    env!("AGENTNAVE_ART_REVISION"),
    "\";\n",
    include_str!("../../ui/common.js")
)
.as_bytes();
const HOSTS_JS: &[u8] = include_bytes!("../../ui/hosts.js");
const HOSTS_CSS: &[u8] = include_bytes!("../../ui/hosts.css");

impl Reader {
    /// Serve one path of the `agentnave` scheme: the tray assets and saved conversation views.
    pub fn respond(&self, path: &str) -> Response {
        if path == "/app/voice" || path.starts_with("/app/voice/") {
            return crate::voice::get(&self.root, path);
        }
        if path == "/app/video-packs" {
            return Response::json(&crate::video_packs::status(&self.root));
        }
        if let Some(rest) = path.strip_prefix("/art/video/") {
            return crate::video_packs::asset(&self.root, rest, None)
                .unwrap_or_else(Response::not_found);
        }
        if let Some(name) = path.strip_prefix("/art/") {
            if name.contains("..") {
                return Response::not_found();
            }
            return match ART.iter().find(|(key, _)| *key == name) {
                Some((_, bytes)) => {
                    let mime = match name.rsplit('.').next() {
                        Some("webp") => "image/webp",
                        Some("svg") => "image/svg+xml",
                        Some("png") => "image/png",
                        Some("json") => "application/json",
                        Some("mov") => "video/quicktime",
                        Some("webm") => "video/webm",
                        _ => "application/octet-stream",
                    };
                    let mut response = Response::ok(mime, bytes.to_vec());
                    response.cache_control = "public, max-age=31536000";
                    response
                }
                None => Response::not_found(),
            };
        }
        if path == "/app/stage" || path == "/app/chats" || path.starts_with("/app/chats/") {
            return self.route(path);
        }
        if path == "/app/characters" {
            return Response::json(&crate::characters::list(&self.root));
        }
        if let Some(name) = path.strip_prefix("/character/") {
            return crate::characters::frame(&self.root, name).unwrap_or_else(Response::not_found);
        }
        if path == "/avatar-settings" {
            return match crate::avatars::read(&self.root) {
                Ok(value) => Response::json(&value),
                Err(error) => Response::error(500, &error),
            };
        }
        if let Some((mime, bytes)) = crate::avatars::asset(path.trim_start_matches('/')) {
            return Response::ok(mime, bytes.to_vec());
        }
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        if let ["chat", resource] = parts.as_slice() {
            return chat_asset(resource);
        }
        if let ["settings", resource] = parts.as_slice() {
            return settings_asset(resource);
        }
        if let ["manage", resource] = parts.as_slice() {
            return match *resource {
                "manage.css" => Response::ok("text/css; charset=utf-8", MANAGE_CSS.to_vec()),
                "theme.css" => Response::ok("text/css; charset=utf-8", THEME_CSS.to_vec()),
                "state" => Response::json(&crate::manage::list(&self.root, self.overview())),
                _ => Response::not_found(),
            };
        }
        if let ["panel", resource] = parts.as_slice() {
            return panel_asset(resource);
        }
        match parts.as_slice() {
            [""] => page(CHAT_HTML),
            _ => Response::not_found(),
        }
    }
}

fn settings_asset(resource: &str) -> Response {
    match resource {
        "" => page(SETTINGS_HTML),
        "voice-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/voice-settings.js").to_vec(),
        ),
        "alerts-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/alerts-settings.js").to_vec(),
        ),
        "about-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/about-settings.js").to_vec(),
        ),
        "storage-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/storage-settings.js").to_vec(),
        ),
        "general-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/general-settings.js").to_vec(),
        ),
        "language-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/language-settings.js").to_vec(),
        ),
        "avatar-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/avatar-settings.js").to_vec(),
        ),
        "advanced.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/advanced.js").to_vec(),
        ),
        "pane-loader.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/pane-loader.js").to_vec(),
        ),
        "settings-state.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/settings-state.js").to_vec(),
        ),
        "member-models.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/member-models.js").to_vec(),
        ),
        "member-updates.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/member-updates.js").to_vec(),
        ),
        "member-cards.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/member-cards.js").to_vec(),
        ),
        "update-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/update-settings.js").to_vec(),
        ),
        "settings-frame.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/settings-frame.js").to_vec(),
        ),
        "settings.js" => Response::ok("text/javascript; charset=utf-8", SETTINGS_JS.to_vec()),
        "settings.css" => Response::ok("text/css; charset=utf-8", SETTINGS_CSS.to_vec()),
        "hosts.js" => Response::ok("text/javascript; charset=utf-8", HOSTS_JS.to_vec()),
        "hosts.css" => Response::ok("text/css; charset=utf-8", HOSTS_CSS.to_vec()),
        "skills.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/skills.js").to_vec(),
        ),
        _ => Response::not_found(),
    }
}

fn chat_asset(resource: &str) -> Response {
    match resource {
        "voice-menu.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/voice-menu.js").to_vec(),
        ),
        "voice-menu.css" => Response::ok(
            "text/css; charset=utf-8",
            include_bytes!("../../ui/voice-menu.css").to_vec(),
        ),
        "voice.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/voice.js").to_vec(),
        ),
        "office-settings.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/office-settings.js").to_vec(),
        ),
        "chat-state.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/chat-state.js").to_vec(),
        ),
        "laptop.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/laptop.js").to_vec(),
        ),
        "chat-office.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/chat-office.js").to_vec(),
        ),
        "chat-messages.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/chat-messages.js").to_vec(),
        ),
        "chat.js" => Response::ok("text/javascript; charset=utf-8", CHAT_JS.to_vec()),
        "chat-groups.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/chat-groups.js").to_vec(),
        ),
        "chat-topics.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/chat-topics.js").to_vec(),
        ),
        "actions.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/actions.js").to_vec(),
        ),
        "chat.css" => Response::ok("text/css; charset=utf-8", CHAT_CSS.to_vec()),
        "appearance.js" => Response::ok("text/javascript; charset=utf-8", APPEARANCE_JS.to_vec()),
        "common.js" => Response::ok("text/javascript; charset=utf-8", COMMON_JS.to_vec()),
        "common-select.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/common-select.js").to_vec(),
        ),
        "update-dialog.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/update-dialog.js").to_vec(),
        ),
        "common-status.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/common-status.js").to_vec(),
        ),

        "" => page(CHAT_HTML),
        id if valid_id(id) => page(CHAT_HTML),
        _ => Response::not_found(),
    }
}

fn panel_asset(resource: &str) -> Response {
    match resource {
        "" => page(PANEL_HTML),
        "panel.js" => Response::ok("text/javascript; charset=utf-8", PANEL_JS.to_vec()),
        "panel.css" => Response::ok("text/css; charset=utf-8", PANEL_CSS.to_vec()),
        "voice-menu.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/voice-menu.js").to_vec(),
        ),
        "voice-menu.css" => Response::ok(
            "text/css; charset=utf-8",
            include_bytes!("../../ui/voice-menu.css").to_vec(),
        ),
        "appearance.js" => Response::ok("text/javascript; charset=utf-8", APPEARANCE_JS.to_vec()),
        "common.js" => Response::ok("text/javascript; charset=utf-8", COMMON_JS.to_vec()),
        "common-select.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/common-select.js").to_vec(),
        ),
        "update-dialog.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/update-dialog.js").to_vec(),
        ),
        "common-status.js" => Response::ok(
            "text/javascript; charset=utf-8",
            include_bytes!("../../ui/common-status.js").to_vec(),
        ),

        _ => Response::not_found(),
    }
}
