//! Shared board catalog; label spellings do not change the fold.
use crate::board::Result;

pub const FIXED: &[&str] = &["taskq", "taskq-host", "q-held", "q-ready", "q-blocked", "q-doing", "q-review", "q-ask", "q-later", "q-failed", "p0", "p1", "p2", "code", "docs", "research", "asset", "run-codex", "run-claude", "assignee-only"];

pub fn canonical(name: &str) -> &str {
    match name { "q-waiting" => "q-blocked", "priority-1" => "p1", "priority-2" => "p2", _ => name }
}

pub fn matches(labels: &[String], filter: &str) -> bool {
    labels.iter().any(|l| canonical(l) == canonical(filter))
}

pub fn definition(name: &str) -> Option<(&'static str, String)> {
    let (color, meaning, change) = match name {
        "q-doing" => ("368477", "Worker active; attempt owns this task", "taskq commands/log"),
        "q-ask" => ("D99536", "Owner answer needed; no admission", "taskq answer"),
        "q-failed" => ("C65365", "Task failed; no admission", "taskq requeue"),
        "q-ready" => ("B7BEC7", "Ready for an eligible host", "taskq commands/log"),
        "q-blocked" => ("AEB5BF", "Open dependency; no admission", "close dependencies"),
        "q-review" => ("8C99A5", "Result awaiting review; not success", "taskq commands/log"),
        "q-held" => ("D5D9DE", "Held; listed, no admission until start", "taskq start"),
        "q-later" => ("D5D9DE", "Parked; no admission", "taskq requeue"),
        "taskq" => ("D5D9DE", "Enables trusted issue admission", "edit issue labels"),
        "taskq-host" => ("D5D9DE", "Host heartbeat; not a task", "service manages this label"),
        "p0" => ("D1495B", "Urgent; passes any focus, admitted first", "edit issue labels"),
        "p1" => ("D5D9DE", "Priority 1; admitted before priority 2", "edit issue labels"),
        "p2" => ("D5D9DE", "Priority 2; also the absent default", "edit issue labels"),
        "code" | "docs" | "research" | "asset" => ("D5D9DE", "Result type; no admission restriction", "edit issue labels"),
        "assignee-only" => ("BAC7D3", "Board account must be an assignee", "edit assignees/labels"),
        "run-codex" | "run-claude" => ("BAC7D3", "Requires this enabled runtime", "edit issue labels"),
        n if n.starts_with("host-") && n.len() > 5 => ("BAC7D3", "Requires this host", "edit issue labels"),
        n if n.starts_with("needs-") && n.len() > 6 => ("BAC7D3", "Requires this capability; all needs must match", "edit issue labels/host has"),
        n if n.starts_with("area-") && n.len() > 5 => ("D5D9DE", "Subsystem; used by focus/status filters", "edit issue labels"),
        _ => return None,
    };
    Some((color, format!("{meaning}; {change}.")))
}

pub fn validate(labels: &[String]) -> Result<()> {
    for (group, values) in [
        ("type", vec!["code", "docs", "research", "asset"]),
        ("priority", vec!["p0", "p1", "p2"]),
    ] {
        let mut found: Vec<_> = labels.iter().map(|l| canonical(l)).filter(|l| values.contains(l)).collect();
        found.sort(); found.dedup();
        if found.len() > 1 { return Err(format!("conflicting {group} labels: {}", found.join(", "))); }
    }
    for prefix in ["run-", "host-"] {
        let mut found: Vec<_> = labels.iter().filter(|l| l.starts_with(prefix)).collect();
        found.sort(); found.dedup();
        if found.len() > 1 { return Err(format!("conflicting {prefix} labels: {found:?}")); }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // Chain: § 1 task labels on add. Risk: conflicting priority, runtime or host labels accepted.
    #[test]
    fn catalog_and_aliases() {
        for n in FIXED { let (hex, desc) = definition(n).unwrap(); assert_eq!(hex.len(), 6); assert!(desc.contains(';')); }
        assert!(definition("run-any").is_none());
        assert!(definition("bug").is_none());
        assert!(matches(&["priority-1".into()], "p1"));
        assert!(matches(&["q-blocked".into()], "q-waiting"));
        assert!(validate(&["p1".into(), "priority-1".into()]).is_ok());
        for pair in [["p1", "priority-2"], ["run-codex", "run-claude"], ["host-win", "host-mac"], ["code", "docs"]] {
            assert!(validate(&pair.map(String::from)).is_err());
        }
        assert!(validate(&["needs-a".into(), "needs-b".into()]).is_ok());
    }
}
