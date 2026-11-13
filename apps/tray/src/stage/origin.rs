//! Where a workspace's work began: the working directory of its earliest surviving dispatch.
//! It decides the workspace's project and never moves when later calls work elsewhere.

use super::*;

/// (time, conversation, call number, directory); the smallest is the earliest. A call without a
/// readable time sorts last so it never claims the origin from a dated one.
pub(super) type Origin = (i64, String, usize, String);

/// A private chat's calls each have a directory and a time; a meeting has one directory and
/// the time it was created.
pub(super) fn snapshot_origin(snapshot: &Snapshot) -> Option<Origin> {
    let director = &snapshot.view["director"];
    let id = &snapshot.item.id;
    let time = |value: &str| timestamp(value).unwrap_or(i64::MAX);
    if snapshot.item.mode == "task" {
        array(director, "tasks")
            .enumerate()
            .filter_map(|(number, task)| {
                let cwd = opt_text(task, "cwd")?;
                Some((time(text(task, "time")), id.clone(), number, cwd))
            })
            .min()
    } else {
        let cwd = opt_text(director, "cwd")?;
        Some((time(text(director, "created")), id.clone(), 0, cwd))
    }
}

/// The directory each workspace began in; a workspace a boss opened by handing over has the
/// directory its handover record names.
pub(crate) fn origin_cwds(
    snapshots: &[Snapshot],
    handovers: &HashMap<String, Handover>,
) -> BTreeMap<String, String> {
    let mut found: BTreeMap<String, Origin> = BTreeMap::new();
    let chats: BTreeSet<String> = snapshots
        .iter()
        .filter_map(|snapshot| chat_group(&snapshot.item))
        .collect();
    for snapshot in snapshots {
        let (Some(group), Some(origin)) = (chat_group(&snapshot.item), snapshot_origin(snapshot))
        else {
            continue;
        };
        match found.get(&group) {
            Some(known) if *known <= origin => {}
            _ => {
                found.insert(group, origin);
            }
        }
    }
    let mut cwds: BTreeMap<_, _> = found
        .into_iter()
        .map(|(group, origin)| (group, origin.3))
        .collect();
    for (id, handover) in handovers {
        if let Some(cwd) = handover.cwd.as_deref().filter(|cwd| !cwd.is_empty())
            && handover.title.is_some()
            && valid_id(id)
            && !chats.contains(id)
        {
            cwds.insert(id.clone(), cwd.to_owned());
        }
    }
    cwds
}
