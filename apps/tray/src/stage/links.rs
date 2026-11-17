//! Bidirectional workspace handover links projected for the office view (ADR-0099).

use std::collections::HashMap;

use serde_json::{Value, json};

use super::{Group, boss_name, label};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceLink {
    pub source: String,
    pub target_boss: String,
    pub target: Option<String>,
    pub target_host: Option<String>,
    pub claimed_at: String,
}

struct Peer {
    host: Option<String>,
    label: String,
    title: String,
}

impl Peer {
    fn of(group: &Group) -> Self {
        let (host, base) = match &group.handover.host {
            Some(host) if group.handover.changed => (Some(host.as_str()), label(Some(host))),
            _ => (group.host.as_deref(), group.manager_label()),
        };
        let host_label = boss_name(host, group.app).1.unwrap_or(base).to_owned();
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
                group.handover_to.push(outgoing_json(link, &peers));
            }
            let targets_group = link.target.as_deref() == Some(group.id.as_str())
                || (link.target.is_none() && link.target_boss == group.id);
            if targets_group {
                group.handover_from.push(incoming_json(link, &peers));
            }
        }
    }
}

pub(super) fn claimed_hosts(handover_to: &[Value]) -> String {
    let mut hosts = Vec::new();
    for entry in handover_to {
        let name = entry["label"].as_str().unwrap_or("未知 boss");
        if !hosts.contains(&name) {
            hosts.push(name);
        }
    }
    hosts.join("、")
}

fn outgoing_json(link: &WorkspaceLink, peers: &HashMap<String, Peer>) -> Value {
    let fallback_label = label(link.target_host.as_deref());
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
