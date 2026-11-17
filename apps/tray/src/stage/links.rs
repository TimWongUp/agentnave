//! Bidirectional workspace handover links projected for the office view (ADR-0099).

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Group, boss_label};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceLink {
    pub source: String,
    pub target_boss: String,
    pub target: Option<String>,
    pub target_host: Option<String>,
    pub claimed_at: String,
}

pub(super) struct ClaimCard {
    pub target_boss: String,
    pub label: String,
    pub claimed_at: String,
}

struct Peer {
    host: Option<String>,
    label: String,
    title: String,
}

impl Peer {
    fn of(group: &Group) -> Self {
        let host = match &group.handover.host {
            Some(host) if group.handover.changed => Some(host.as_str()),
            _ => group.host.as_deref(),
        };
        let host_label = boss_label(host, group.app).to_owned();
        let title = if !group.title.is_empty() {
            group.title.clone()
        } else {
            group
                .project_name
                .clone()
                .unwrap_or_else(|| host_label.clone())
        };
        Self {
            host: host.map(str::to_owned),
            label: host_label,
            title,
        }
    }
}

pub(super) fn attach_links(groups: &mut [Group], links: &[WorkspaceLink]) {
    if links.is_empty() {
        return;
    }
    let peers: HashMap<String, Peer> = groups
        .iter()
        .map(|group| (group.id.clone(), Peer::of(group)))
        .collect();
    for group in groups {
        for link in links {
            if link.source == group.id {
                let outgoing = outgoing_json(link, &peers);
                let target_label = outgoing["label"].as_str().unwrap_or("未知 boss").to_owned();
                group.handover_links.push(ClaimCard {
                    target_boss: link.target_boss.clone(),
                    label: target_label,
                    claimed_at: link.claimed_at.clone(),
                });
                group.handover_to.push(outgoing);
            }
            let targets_group = link.target.as_deref() == Some(group.id.as_str())
                || (link.target.is_none() && link.target_boss == group.id);
            if targets_group {
                group.handover_from.push(incoming_json(link, &peers));
            }
        }
    }
}

fn outgoing_json(link: &WorkspaceLink, peers: &HashMap<String, Peer>) -> Value {
    let fallback_label = boss_label(link.target_host.as_deref(), None);
    match link.target.as_deref() {
        None => json!({
            "status": "claimed",
            "host": link.target_host,
            "label": fallback_label,
            "claimed_at": link.claimed_at,
        }),
        Some(target_id) => match peers.get(target_id) {
            Some(peer) => json!({
                "status": "linked",
                "group": target_id,
                "host": peer.host.as_deref().or(link.target_host.as_deref()),
                "label": peer.label,
                "title": peer.title,
                "claimed_at": link.claimed_at,
            }),
            None => json!({
                "status": "cleaned",
                "host": link.target_host,
                "label": fallback_label,
                "claimed_at": link.claimed_at,
            }),
        },
    }
}

fn incoming_json(link: &WorkspaceLink, peers: &HashMap<String, Peer>) -> Value {
    match peers.get(&link.source) {
        Some(peer) => json!({
            "status": "linked",
            "group": link.source,
            "host": peer.host,
            "label": peer.label,
            "title": peer.title,
            "claimed_at": link.claimed_at,
        }),
        None => json!({
            "status": "cleaned",
            "claimed_at": link.claimed_at,
        }),
    }
}
