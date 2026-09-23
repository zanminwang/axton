//! Identifiers emitted into the Action TypeScript and Dart contract namespaces.
use crate::parse::{Declarations, Pos};
use serde_json::Value;
use std::collections::BTreeMap;

fn values<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn name(value: &Value) -> &str {
    value["name"].as_str().unwrap()
}
fn position(declarations: Option<&Declarations>, owner: &str) -> Option<Pos> {
    let declarations = declarations?;
    if let Some(n) = owner.strip_prefix("model ") {
        return declarations
            .models
            .iter()
            .find(|m| m.name == n)
            .map(|m| m.pos);
    }
    if let Some(n) = owner.strip_prefix("enum ") {
        return declarations
            .enums
            .iter()
            .find(|e| e.name == n)
            .map(|e| e.pos);
    }
    if let Some(n) = owner.strip_prefix("mutation ") {
        let n = n.split(' ').next()?;
        return declarations
            .mutations
            .iter()
            .find(|m| m.name == n)
            .map(|m| m.pos);
    }
    let action = owner.strip_prefix("Action ")?.split(' ').next()?;
    declarations
        .actions
        .iter()
        .find(|a| a.name == action)
        .map(|a| a.pos)
}

/// Called once for current declarations and again after retained histories are reconciled.
/// The second call checks names that only exist in older Action/Model versions.
pub(crate) fn check(config: &Value, declarations: Option<&Declarations>) -> Result<(), String> {
    let actions = values(config, "actions");
    if actions.is_empty() {
        return Ok(());
    }
    let mut names = BTreeMap::<String, String>::new();
    let mut add = |identifier: String, owner: String| -> Result<(), String> {
        if let Some(previous) = names.get(&identifier) {
            if previous != &owner {
                let pos =
                    position(declarations, &owner).or_else(|| position(declarations, previous));
                let message = format!(
                    "generated identifier {identifier} from {owner} collides with {previous}"
                );
                return Err(match pos {
                    Some(pos) => format!("{}:{}: {message}", pos.line, pos.col),
                    None => format!("Action history: {message}"),
                });
            }
        } else {
            names.insert(identifier, owner);
        }
        Ok(())
    };
    let schema = &config["schema"];
    for en in values(schema, "enums") {
        add(name(en).into(), format!("enum {}", name(en)))?;
    }
    for model in values(schema, "models") {
        let n = name(model);
        let owner = format!("model {n}");
        for suffix in [
            "",
            "Identity",
            "Patch",
            "Create",
            "Update",
            "Delete",
            "Filter",
            "Order",
            "OrderField",
            "Model",
            "LiveModel",
            "TxModel",
        ] {
            add(format!("{n}{suffix}"), owner.clone())?;
        }
        for suffix in ["Model", "LiveModel"] {
            add(
                format!("Action{n}{suffix}"),
                format!("Action helper for model {n}"),
            )?;
        }
    }
    for model in values(config, "backendModels") {
        let n = name(model);
        let version = model["version"].as_u64().unwrap_or(1);
        let current = values(schema, "models")
            .iter()
            .find(|m| name(m) == n)
            .and_then(|m| m["version"].as_u64());
        if current != Some(version) {
            add(format!("{n}V{version}"), format!("model {n}"))?;
            add(format!("{n}V{version}Identity"), format!("model {n}"))?;
        }
    }
    for helper in [
        "ActionError",
        "ActionStatus",
        "ActionOutcome",
        "ActionCall",
        "ActionTxModels",
        "ActionModels",
        "ActionTransactionContract",
        "ActionClientContract",
        "ActionActionsContract",
        "ActionDirectCallsContract",
        "ActionHandlerCall",
        "ActionHandlers",
        "ActionSuccess",
        "ActionFailure",
        "ActionBackendContract",
    ] {
        add(helper.into(), "Action helper".into())?;
    }
    let mutations = if config["backendMutations"].is_array() {
        values(config, "backendMutations")
    } else {
        values(config, "mutations")
    };
    let mut mutation_latest = BTreeMap::<&str, u64>::new();
    for mutation in mutations {
        let n = name(mutation);
        let version = mutation["version"].as_u64().unwrap();
        mutation_latest
            .entry(n)
            .and_modify(|v| *v = (*v).max(version))
            .or_insert(version);
    }
    for mutation in mutations {
        let n = name(mutation);
        let version = mutation["version"].as_u64().unwrap();
        let prefix = if version == mutation_latest[n] {
            n.to_owned()
        } else {
            format!("{n}V{version}")
        };
        add(format!("{prefix}Input"), format!("mutation {n} v{version}"))?;
    }
    let mut latest = BTreeMap::<&str, u64>::new();
    for action in actions {
        let n = name(action);
        let version = action["version"].as_u64().unwrap();
        latest
            .entry(n)
            .and_modify(|v| *v = (*v).max(version))
            .or_insert(version);
    }
    for &n in latest.keys() {
        add(format!("Action{n}Handlers"), format!("Action {n} handlers"))?;
    }
    for action in actions {
        let n = name(action);
        let version = action["version"].as_u64().unwrap();
        let retained = version != latest[n];
        let prefix = if retained {
            format!("{n}V{version}")
        } else {
            n.to_owned()
        };
        let owner = format!("Action {n} v{version}");
        for suffix in ["Input", "HandlerOutput"] {
            add(format!("{prefix}{suffix}"), format!("{owner} {suffix}"))?;
        }
        if !retained {
            add(format!("{prefix}Output"), format!("{owner} Output"))?;
        } else {
            for model in values(&action["input"], "models") {
                for suffix in ["Create", "Identity", "Patch", "Update", "Delete"] {
                    add(
                        format!("{prefix}{}{suffix}", name(model)),
                        format!("{owner} model {} {suffix}", name(model)),
                    )?;
                }
            }
            let mut input_enums = std::collections::BTreeSet::new();
            for en in values(&action["input"], "enums") {
                if input_enums.insert(name(en)) {
                    add(
                        format!("{prefix}{}", name(en)),
                        format!("{owner} input enum {}", name(en)),
                    )?;
                }
            }
            let mut output_enums = std::collections::BTreeSet::new();
            for en in values(action, "outputEnums") {
                if output_enums.insert(name(en)) {
                    add(
                        format!("{prefix}Output{}", name(en)),
                        format!("{owner} output enum {}", name(en)),
                    )?;
                }
            }
        }
        for arg in values(action, "inputs") {
            if arg["kind"] == "model"
                && arg["operation"] == "update"
                && arg["allowedPatchFields"].is_array()
            {
                let slot = arg["name"].as_str().unwrap();
                let upper = format!("{}{}", slot[..1].to_ascii_uppercase(), &slot[1..]);
                add(
                    format!(
                        "{prefix}{upper}{}Update",
                        if retained { "Slot" } else { "" }
                    ),
                    format!("{owner} operand {slot} Update"),
                )?;
            }
        }
    }
    Ok(())
}
