//! Who is talking, where, and to whom: the part of a spoken message the text itself leaves out.
use serde_json::Value;

const NAME_CHARS: usize = 40;

struct Person {
    name: String,
    role: String,
}

pub(super) struct Scene {
    boss: bool,
    me: Person,
    room: bool,
    to: Option<Person>,
}

/// A role or name comes from the conversation, so an odd one is trimmed rather than refused.
fn field(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(NAME_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn person(value: &Value) -> Person {
    Person {
        name: field(value, "name"),
        role: field(value, "role"),
    }
}

impl Person {
    /// "Claude（探索派）", dropping the role when the name already carries it.
    fn called(&self) -> String {
        if self.role.is_empty() || self.name.contains(&self.role) {
            self.name.clone()
        } else {
            format!("{}（{}）", self.name, self.role)
        }
    }
}

impl Scene {
    pub(super) fn of(payload: &Value, boss: bool) -> Self {
        let scene = &payload["scene"];
        Self {
            boss,
            me: person(scene),
            room: scene["room"] == true,
            to: Some(person(&scene["to"])).filter(|to| !to.name.is_empty()),
        }
    }

    /// Tells the model who it is and who hears it, so it speaks as itself and not about itself.
    pub(super) fn identity(&self) -> String {
        let who = if self.boss {
            "你是这个工作区的 boss：给员工派任务、主持会议、做决定。".to_string()
        } else if self.me.name.is_empty() {
            "你是这个工作区的一名员工：接 boss 交代的事，向 boss 汇报。".to_string()
        } else if self.me.role.is_empty() {
            format!(
                "你是员工 {}：接 boss 交代的事，向 boss 汇报。",
                self.me.name
            )
        } else {
            format!(
                "你是员工 {}，工位角色是「{}」：接 boss 交代的事，向 boss 汇报。",
                self.me.name, self.me.role
            )
        };
        let hearing = match (self.boss, self.room, &self.to) {
            (true, _, Some(to)) => format!("现在你私下对员工 {} 说话，别人听不到。", to.called()),
            (true, true, None) => "现在你在会议上对所有参会员工说话。".to_string(),
            (true, false, None) => "现在你在对员工说话。".to_string(),
            (false, true, _) => "现在你在会议上发言，boss 和其他员工都听得到。".to_string(),
            (false, false, _) => "现在你私下向 boss 汇报。".to_string(),
        };
        format!("{who}{hearing}")
    }

    /// A message for one employee opens by calling them, unless it already does.
    pub(super) fn addressed(&self, text: String) -> String {
        let Some(to) = self.to.as_ref().filter(|_| self.boss) else {
            return text;
        };
        let name = to.name.to_lowercase();
        if text.trim_start().to_lowercase().starts_with(&name) {
            return text;
        }
        format!("{}，{text}", to.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn brief() -> Scene {
        let scene = json!({"name":"boss","role":"","room":true,
            "to":{"name":"Claude","role":"探索派"}});
        Scene::of(&json!({ "scene": scene }), true)
    }

    #[test]
    fn a_private_briefing_calls_its_employee_once() {
        let scene = brief();
        assert_eq!(
            scene.addressed("你是探索派。".into()),
            "Claude，你是探索派。"
        );
        assert_eq!(
            scene.addressed("claude，你是探索派。".into()),
            "claude，你是探索派。"
        );
        assert!(
            scene
                .identity()
                .contains("私下对员工 Claude（探索派） 说话")
        );
    }

    #[test]
    fn only_a_boss_addressing_one_employee_is_prefixed() {
        let room = json!({"scene":{"name":"boss","room":true,"to":null}});
        assert_eq!(Scene::of(&room, true).addressed("开会。".into()), "开会。");
        let member = json!({"scene":{"name":"Gemini","role":"探索派","to":{"name":"boss"}}});
        let member = Scene::of(&member, false);
        assert_eq!(member.addressed("结论是甲。".into()), "结论是甲。");
        assert!(
            member
                .identity()
                .starts_with("你是员工 Gemini，工位角色是「探索派」")
        );
        assert!(member.identity().contains("私下向 boss 汇报"));
    }

    #[test]
    fn a_payload_without_a_scene_still_speaks_unchanged() {
        let scene = Scene::of(&json!({}), false);
        assert_eq!(scene.addressed("你好".into()), "你好");
        assert!(scene.identity().starts_with("你是这个工作区的一名员工"));
    }
}
