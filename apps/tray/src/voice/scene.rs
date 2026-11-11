//! Who is talking, where, and to whom: what the text itself leaves out, told to the summary
//! model so it can speak the message as its speaker would, naming whom it needs to.
use serde_json::Value;

const NAME_CHARS: usize = 40;
const LISTENERS: usize = 8;

struct Person {
    name: String,
    role: String,
    /// Another seat here goes by the same name.
    twin: bool,
}

pub(super) struct Scene {
    boss: bool,
    me: Person,
    room: bool,
    private: bool,
    to: Vec<Person>,
    forward: Option<Person>,
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
        twin: value["twin"] == true,
    }
}

impl Person {
    /// "Claude（职责：探索派）", with a reminder when another seat shares the name.
    fn called(&self) -> String {
        match (self.role.is_empty(), self.twin) {
            (true, _) => self.name.clone(),
            (false, false) => format!("{}（职责：{}）", self.name, self.role),
            (false, true) => format!(
                "{}（职责：{}；这里还有另一位 {}，称呼时带上职责区分）",
                self.name, self.role, self.name
            ),
        }
    }
}

fn list(people: &[Person]) -> String {
    people
        .iter()
        .map(Person::called)
        .collect::<Vec<_>>()
        .join("、")
}

impl Scene {
    pub(super) fn of(payload: &Value, boss: bool) -> Self {
        let scene = &payload["scene"];
        let to = match &scene["to"] {
            Value::Array(items) => items.iter().take(LISTENERS).map(person).collect(),
            Value::Object(_) => vec![person(&scene["to"])],
            _ => Vec::new(),
        };
        Self {
            boss,
            me: person(scene),
            room: scene["room"] == true,
            private: scene["private"] == true,
            to: to.into_iter().filter(|to| !to.name.is_empty()).collect(),
            forward: Some(person(&scene["forward"])).filter(|from| !from.name.is_empty()),
        }
    }

    /// Who the speaker is and who hears it, so the model speaks as itself, to its listeners.
    pub(super) fn identity(&self) -> String {
        let who = if self.boss {
            "你是这个工作区的 boss。".to_string()
        } else if self.me.name.is_empty() {
            "你是这个工作区的一名员工。".to_string()
        } else if self.me.role.is_empty() {
            format!("你是员工 {}。", self.me.name)
        } else {
            format!("你是员工 {}，职责是「{}」。", self.me.name, self.me.role)
        };
        let hearing = match (self.boss, self.room, self.private, self.to.is_empty()) {
            (true, _, true, false) => {
                format!("现在你私下对员工 {} 说话，别人听不到。", list(&self.to))
            }
            (true, true, false, false) => format!(
                "现在你在会议上点名对 {} 说话，其他参会员工也听得到。",
                list(&self.to)
            ),
            (true, true, _, true) => "现在你在会议上对所有参会员工说话。".to_string(),
            (true, false, _, _) => "现在你在对员工说话。".to_string(),
            (false, true, false, _) => "现在你在会议上发言，boss 和其他员工都听得到。".to_string(),
            (false, _, _, _) => "现在你私下回复 boss。".to_string(),
        };
        let relay = self.forward.as_ref().map_or(String::new(), |from| {
            format!(
                "原话是 {} 说的，你在把它转给大家：要说清楚是在转述谁，那些结论和做法是对方的，不是你的。",
                from.called()
            )
        });
        format!("{who}{hearing}{relay}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_private_briefing_names_its_employee_for_the_model_to_call() {
        let scene = json!({"name":"boss","room":true,"private":true,
            "to":[{"name":"Claude","role":"探索派","twin":true}]});
        let identity = Scene::of(&json!({ "scene": scene }), true).identity();
        assert!(identity.contains("私下对员工 Claude（职责：探索派；这里还有另一位 Claude"));
    }

    #[test]
    fn a_meeting_message_says_who_is_called_on_and_who_listens() {
        let named = json!({"scene":{"name":"boss","room":true,
            "to":[{"name":"Claude"},{"name":"Gemini","role":"评审"}]}});
        assert!(
            Scene::of(&named, true)
                .identity()
                .contains("点名对 Claude、Gemini（职责：评审） 说话，其他参会员工也听得到")
        );
        let everyone = json!({"scene":{"name":"boss","room":true,"to":[]}});
        assert!(
            Scene::of(&everyone, true)
                .identity()
                .contains("对所有参会员工说话")
        );
        let member = json!({"scene":{"name":"Gemini","role":"探索派","room":true}});
        let member = Scene::of(&member, false).identity();
        assert!(member.starts_with("你是员工 Gemini，职责是「探索派」"));
        assert!(member.contains("boss 和其他员工都听得到"));
        let private = json!({"scene":{"name":"Gemini","private":true}});
        assert!(
            Scene::of(&private, false)
                .identity()
                .contains("私下回复 boss")
        );
    }

    #[test]
    fn a_forward_says_whose_words_are_relayed() {
        let scene =
            json!({"scene":{"name":"boss","room":true,"forward":{"name":"GPT","role":"实现"}}});
        assert!(
            Scene::of(&scene, true)
                .identity()
                .contains("原话是 GPT（职责：实现） 说的")
        );
        assert!(
            Scene::of(&json!({}), false)
                .identity()
                .starts_with("你是这个工作区的一名员工")
        );
    }
}
