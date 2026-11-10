//! Validated, comment-preserving edits shared by the visual rule workspace.
use crate::{Action, Config, Rule};
use toml_edit::{Array, DocumentMut, Item, Table, value};
/// Editing a rule preserves its identity even if the user changes its name.
/// A collision is an error, never an implicit replacement of another rule.
pub fn replace(text: &str, original: Option<&str>, rule: &Rule) -> Result<String, String> {
    let config = Config::from_toml(text).map_err(|e| e.to_string())?;
    if let Some(original) = original {
        if config.rules.iter().filter(|r| r.name == original).count() != 1 {
            return Err(
                "The original rule is missing or ambiguous; reload it before editing".into(),
            );
        }
        if original != rule.name && config.rules.iter().any(|r| r.name == rule.name) {
            return Err("Another rule already uses this name".into());
        }
        if original != rule.name {
            return set(&remove(text, original)?, rule);
        }
    } else if config.rules.iter().any(|r| r.name == rule.name) {
        return Err("Another rule already uses this name".into());
    }
    set(text, rule)
}
pub fn set(text: &str, rule: &Rule) -> Result<String, String> {
    if rule.name.trim().is_empty() || rule.name.len() > 128 {
        return Err("Enter a rule name of at most 128 bytes".into());
    }
    rule.validate()?;
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let mut node = Table::new();
    node.insert("name", value(&rule.name));
    let mut matcher = Table::new();
    for (key, pattern) in [
        ("name", &rule.matcher.name),
        ("path", &rule.matcher.path),
        ("cmdline", &rule.matcher.cmdline),
    ] {
        if let Some(pattern) = pattern {
            matcher.insert(key, value(pattern));
        }
    }
    node.insert("match", Item::Table(matcher));
    if let Some(condition) = &rule.when {
        let mut when = Table::new();
        if let Some(v) = condition.cpu_above {
            when.insert("cpu_above", value(f64::from(v)));
        }
        if let Some(v) = condition.memory_above_mb {
            when.insert("memory_above_mb", value(v as i64));
        }
        if let Some(v) = condition.gpu_above {
            when.insert("gpu_above", value(f64::from(v)));
        }
        when.insert("for_secs", value(condition.for_secs as i64));
        node.insert("when", Item::Table(when));
    }
    let mut actions = Array::new();
    for action in &rule.actions {
        actions.push(crate::presets::ladder_item(action));
    }
    node.insert("actions", value(actions));
    let list = doc
        .entry("rule")
        .or_insert_with(|| Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .ok_or("rule must be an array of tables")?;
    let matches: Vec<_> = list
        .iter()
        .enumerate()
        .filter(|(_, t)| t.get("name").and_then(Item::as_str) == Some(rule.name.as_str()))
        .map(|(i, _)| i)
        .collect();
    if matches.len() > 1 {
        return Err("Several rules share this name; use a unique name".into());
    }
    if let Some(index) = matches.first() {
        *list.get_mut(*index).expect("existing rule") = node;
    } else {
        list.push(node);
    }
    let out = doc.to_string();
    Config::from_toml(&out).map_err(|e| e.to_string())?;
    Ok(out)
}
pub fn remove(text: &str, name: &str) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    if let Some(list) = doc.get_mut("rule").and_then(Item::as_array_of_tables_mut) {
        list.retain(|t| t.get("name").and_then(Item::as_str) != Some(name));
    }
    let out = doc.to_string();
    Config::from_toml(&out).map_err(|e| e.to_string())?;
    Ok(out)
}
pub fn destructive(rule: &Rule) -> bool {
    rule.actions
        .iter()
        .any(|a| matches!(a, Action::Kill | Action::Suspend))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rule_edits_validate_and_preserve_unrelated_configuration() {
        let original = "# user notes\ndry_run = true # preserve\n";
        let rule = Rule {
            name: "Editor budget".into(),
            matcher: crate::Matcher {
                name: Some("editor.exe".into()),
                ..Default::default()
            },
            when: Some(crate::Condition {
                cpu_above: Some(20.0),
                for_secs: 10,
                ..Default::default()
            }),
            actions: vec![Action::EcoQos, Action::CpuCapRelative(0.5)],
        };
        let out = set(original, &rule).unwrap();
        assert!(out.starts_with(original));
        assert_eq!(Config::from_toml(&out).unwrap().rules, vec![rule.clone()]);
        let out = set(&out, &rule).unwrap();
        assert_eq!(Config::from_toml(&out).unwrap().rules.len(), 1);
        assert!(
            Config::from_toml(&remove(&out, &rule.name).unwrap())
                .unwrap()
                .rules
                .is_empty()
        );
        let mut bad = rule;
        bad.matcher.name = Some("*".into());
        bad.actions = vec![Action::Kill];
        assert!(set(original, &bad).is_err());
    }

    #[test]
    fn renaming_replaces_the_original_and_never_overwrites_another_rule() {
        let a = Rule {
            name: "A".into(),
            matcher: crate::Matcher {
                name: Some("a.exe".into()),
                ..Default::default()
            },
            when: None,
            actions: vec![Action::EcoQos],
        };
        let b = Rule {
            name: "B".into(),
            ..a.clone()
        };
        let text = set("# keep this\ndry_run=true\n", &a).unwrap();
        let renamed = replace(&text, Some("A"), &b).unwrap();
        assert_eq!(Config::from_toml(&renamed).unwrap().rules, vec![b.clone()]);
        assert!(renamed.starts_with("# keep this"));
        let both = set(&text, &b).unwrap();
        assert!(replace(&both, Some("A"), &b).is_err());
        assert!(replace(&text, None, &a).is_err());
        assert!(replace(&text, Some("missing"), &b).is_err());
    }
}
