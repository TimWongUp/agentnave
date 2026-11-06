//! Native prompt audit events, kept separate from boss dispatch messages (`prompt`).
use super::*;

/// Produce one updatable event per prompt, using its answer or closure time when resolved.
pub(super) fn messages(source: &Value, prefix: &str, template: &Value) -> Vec<Value> {
    array(source, "prompts")
        .enumerate()
        .map(|(index, prompt)| {
            let mut message = template.clone();
            let prompt_id = opt_text(prompt, "prompt_id").unwrap_or_else(|| index.to_string());
            message["id"] = json!(format!(
                "{prefix}:approval:{}:{prompt_id}",
                text(prompt, "invocation_id")
            ));
            let time = [
                text(&prompt["answer"], "answered_at"),
                text(prompt, "closed_at"),
                text(prompt, "created_at"),
                text(template, "time"),
            ]
            .into_iter()
            .find(|time| timestamp(time).is_some())
            .unwrap_or_default();
            message["time"] = json!(time);
            let answer = &prompt["answer"];
            message["approval"] = json!({
                "kind": prompt["kind"], "tool_name": prompt["tool_name"],
                "tool_input": prompt["tool_input"], "questions": prompt["questions"],
                "status": prompt["status"],
                "answer": {
                    "actor": answer["actor"], "decision": answer["decision"],
                    "message": answer["message"], "answers": answer["answers"],
                },
            });
            message
        })
        .collect()
}

impl Group {
    pub(super) fn show_approvals(&mut self, snapshot: &Snapshot) {
        if !self.include_messages {
            return;
        }
        let item = &snapshot.item;
        let view = &snapshot.view["director"];
        // Historical rounds carry audits too; older snapshots may only carry the latest round.
        let mut rounds: Vec<_> = array(view, "rounds").collect();
        if view["round"].is_object() {
            rounds.retain(|round| round["number"] != view["round"]["number"]);
            rounds.push(&view["round"]);
        }
        for round in rounds {
            for run in array(round, "seats") {
                let id = text(run, "seat_id");
                let Some(seat) = array(view, "seats").find(|seat| text(seat, "id") == id) else {
                    continue;
                };
                let desk = seat_desk(&item.id, id);
                let mut template = Self::message(
                    String::new(),
                    &item.updated,
                    &item.id,
                    "approval",
                    member_speaker(&identity_of(seat, id, run)),
                    "",
                );
                template["round"] = round["number"].clone();
                template["focus"] = json!(desk);
                annotate(
                    &mut template,
                    "private",
                    room_thread(&item.id),
                    Some(&desk),
                    None,
                    None,
                );
                let prefix = format!("{}:round:{}:{id}", item.id, round["number"]);
                self.messages.extend(messages(run, &prefix, &template));
            }
        }
    }

    pub(super) fn task_approvals(
        &mut self,
        item: &Item,
        task: &Value,
        prefix: &str,
        desk: &str,
        time: &str,
    ) {
        if !self.include_messages {
            return;
        }
        let mut template = Self::message(
            String::new(),
            time,
            &item.id,
            "approval",
            member_speaker(task),
            "",
        );
        annotate(
            &mut template,
            "private",
            desk_thread(desk),
            Some(desk),
            None,
            None,
        );
        template["reply_to"] = json!(format!("{prefix}:prompt"));
        self.messages.extend(messages(task, prefix, &template));
    }
}
