//! Recipes (taskq.md § 7, § 9 `recipes`): a reusable task list whose run adds ordinary held tasks. Identity is the
//! name and its `version`; both go into each task's op id, so a rerun of the same run returns the first receipts.

use crate::{NewTask, Res};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    version: Value,
    tasks: Vec<Template>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Template {
    title: String,
    goal: String,
    acceptance: Vec<String>,
    #[serde(default)]
    scope: Vec<String>,
    #[serde(default)]
    deps: Vec<usize>,
    #[serde(default)]
    gate: bool,
    #[serde(rename = "type")]
    kind: Option<String>,
    runtime: Option<String>,
    priority: Option<Value>,
    host: Option<String>,
}

/// One task of a run: the fields, its 1-based `deps` on earlier tasks of the run, and whether it carries the gate.
#[derive(Debug, PartialEq)]
pub struct Item {
    pub task: NewTask,
    pub deps: Vec<usize>,
    pub gate: bool,
}

fn word(what: &str, s: &str) -> Res<String> {
    if s.is_empty() || s.chars().any(|c| c.is_whitespace() || c == '.') {
        return Err(format!("{what} `{s}`: one word without dots"));
    }
    Ok(s.to_string())
}

/// The version and items of recipe `name` from `taskq.json`, `{goal}` replaced; checked before any write.
pub fn load(raw: &Value, name: &str, run: &str, goal: Option<&str>) -> Res<(String, Vec<Item>)> {
    let name = word("recipe", name)?;
    word("--run", run)?;
    let v = raw.get("recipes").and_then(|r| r.get(&name)).ok_or(format!("no recipe `{name}` in taskq.json `recipes`"))?;
    let r: Recipe = serde_json::from_value(v.clone()).map_err(|e| format!("recipe `{name}`: {e}"))?;
    let version = word("recipe version", &match &r.version {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Err(format!("recipe `{name}`: version is a string or number")),
    })?;
    if r.tasks.is_empty() {
        return Err(format!("recipe `{name}` has no tasks"));
    }
    let fill = |s: &str| -> Res<String> {
        match goal {
            Some(g) => Ok(s.replace("{goal}", g)),
            None if s.contains("{goal}") => Err(format!("recipe `{name}` needs --goal")),
            None => Ok(s.to_string()),
        }
    };
    let mut out = vec![];
    for (i, t) in r.tasks.into_iter().enumerate() {
        if let Some(d) = t.deps.iter().find(|d| **d == 0 || **d > i) {
            return Err(format!("recipe `{name}` task {}: deps {d} is not an earlier task", i + 1));
        }
        let priority = match t.priority {
            None => None,
            Some(Value::String(s)) => Some(s),
            Some(Value::Number(n)) => Some(n.to_string()),
            Some(p) => return Err(format!("recipe `{name}` task {}: bad priority {p}", i + 1)),
        };
        let task = NewTask {
            title: fill(&t.title)?,
            goal: fill(&t.goal)?,
            acceptance: t.acceptance.iter().map(|s| fill(s)).collect::<Res<_>>()?,
            scope: t.scope.iter().map(|s| fill(s)).collect::<Res<_>>()?,
            kind: t.kind,
            runtime: t.runtime,
            priority,
            host: t.host,
            hold: true,
            labels: if t.gate { vec!["needs-gate".into()] } else { vec![] },
            ..NewTask::default()
        };
        out.push(Item { task, deps: t.deps, gate: t.gate });
    }
    Ok((version, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg() -> Value {
        json!({"recipes": {"feature": {"version": 2, "tasks": [
            {"title": "Spec {goal}", "goal": "Decide {goal}", "acceptance": ["spec merged"], "gate": true, "type": "docs"},
            {"title": "Code {goal}", "goal": "Build {goal}", "acceptance": ["tests pass"], "deps": [1], "priority": 1}
        ]}}})
    }

    // Chain: taskq.json recipe -> load -> held NewTask list. Risk: a template that names a later task, or a run
    // without --goal, adds half a recipe before failing.
    #[test]
    fn load_checks_the_whole_recipe_before_any_write() {
        let (v, items) = load(&cfg(), "feature", "r1", Some("login")).unwrap();
        assert_eq!(v, "2");
        assert_eq!(items[0].task.title, "Spec login");
        assert!(items[0].gate && items[0].task.hold && items[0].task.labels == ["needs-gate"]);
        assert_eq!((items[1].deps.clone(), items[1].task.priority.clone()), (vec![1], Some("1".into())));
        assert_eq!(load(&cfg(), "feature", "r1", None).unwrap_err(), "recipe `feature` needs --goal");
        assert!(load(&cfg(), "nope", "r1", None).unwrap_err().starts_with("no recipe"));
        assert!(load(&cfg(), "feature", "r 1", Some("x")).unwrap_err().starts_with("--run"));
        let bad = json!({"recipes": {"x": {"version": "1", "tasks": [{"title": "A", "goal": "G", "acceptance": ["a"], "deps": [1]}]}}});
        assert!(load(&bad, "x", "r", None).unwrap_err().contains("not an earlier task"));
    }
}
