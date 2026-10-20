// SPDX-License-Identifier: AGPL-3.0-only
//! Structural check of a Studio application document, done on the server so an application's
//! *design status* never rests on what a browser claims. It checks shape and references (the
//! document is well formed, ids are unique, every cable joins an existing output to an existing
//! input); the Studio's richer rules (protocol compatibility, configuration values) still run in
//! the browser and are not repeated here.

use serde_json::Value;

pub const API_VERSION: &str = "fabric.apelora/v1alpha1";
pub const KIND: &str = "ApplicationAssembly";
pub const MAX_BYTES: usize = 2_000_000;
const MAX_MODULES: usize = 200;
const MAX_CONNECTIONS: usize = 1000;

fn id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Problems found, empty when the document is structurally valid.
pub fn validate(doc: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(root) = doc.as_object() else {
        return vec!["the document must be a JSON object".into()];
    };
    if root.get("apiVersion").and_then(Value::as_str) != Some(API_VERSION) {
        errors.push(format!("apiVersion must be {API_VERSION}"));
    }
    if root.get("kind").and_then(Value::as_str) != Some(KIND) {
        errors.push(format!("kind must be {KIND}"));
    }
    let Some(spec) = root.get("spec").and_then(Value::as_object) else {
        errors.push("spec is missing".into());
        return errors;
    };
    let modules = spec.get("modules").and_then(Value::as_array);
    let Some(modules) = modules else {
        errors.push("spec.modules must be a list".into());
        return errors;
    };
    if modules.len() > MAX_MODULES {
        errors.push(format!("at most {MAX_MODULES} modules"));
    }
    // id -> (inputs, outputs) port ids
    let mut ports: std::collections::BTreeMap<&str, (Vec<&str>, Vec<&str>)> = Default::default();
    for (i, a) in modules.iter().enumerate() {
        let Some(id) = a.get("id").and_then(Value::as_str).filter(|s| id_ok(s)) else {
            errors.push(format!("module {} has no valid id", i + 1));
            continue;
        };
        if ports.contains_key(id) {
            errors.push(format!("module {id} appears twice"));
            continue;
        }
        let mut entry = (Vec::new(), Vec::new());
        let mut seen = std::collections::BTreeSet::new();
        for p in a
            .get("ports")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let (Some(pid), Some(dir)) = (
                p.get("id").and_then(Value::as_str),
                p.get("direction").and_then(Value::as_str),
            ) else {
                errors.push(format!("{id}: a port lacks an id or direction"));
                continue;
            };
            if !seen.insert(pid) {
                errors.push(format!("{id}: port {pid} appears twice"));
            }
            match dir {
                "input" => entry.0.push(pid),
                "output" => entry.1.push(pid),
                _ => errors.push(format!("{id}.{pid}: direction must be input or output")),
            }
        }
        ports.insert(id, entry);
    }
    let conns = spec
        .get("connections")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    if conns.len() > MAX_CONNECTIONS {
        errors.push(format!("at most {MAX_CONNECTIONS} connections"));
    }
    let mut cable_ids = std::collections::BTreeSet::new();
    for c in conns {
        let end = |side: &str| {
            c.get(side).map(|e| {
                (
                    e.get("moduleId").and_then(Value::as_str),
                    e.get("portId").and_then(Value::as_str),
                )
            })
        };
        let (Some((Some(sa), Some(sp))), Some((Some(ta), Some(tp)))) =
            (end("source"), end("target"))
        else {
            errors.push("a connection lacks its source or target".into());
            continue;
        };
        if let Some(cid) = c.get("id").and_then(Value::as_str)
            && !cable_ids.insert(cid)
        {
            errors.push(format!("connection {cid} appears twice"));
        }
        if sa == ta {
            errors.push(format!("{sa} is connected to itself"));
        }
        match (ports.get(sa), ports.get(ta)) {
            (Some(s), Some(t)) => {
                if !s.1.contains(&sp) {
                    errors.push(format!("{sa}.{sp} is not an output port"));
                }
                if !t.0.contains(&tp) {
                    errors.push(format!("{ta}.{tp} is not an input port"));
                }
            }
            _ => errors.push(format!(
                "a connection refers to an unknown module ({sa} → {ta})"
            )),
        }
    }
    errors.truncate(50);
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc() -> Value {
        json!({ "apiVersion": API_VERSION, "kind": KIND, "metadata": { "name": "x" },
            "spec": { "modules": [
                { "id": "web-01", "ports": [{ "id": "http", "direction": "input" }, { "id": "db", "direction": "output" }] },
                { "id": "db-01", "ports": [{ "id": "sql", "direction": "input" }] } ],
              "connections": [{ "id": "conn-01", "source": { "moduleId": "web-01", "portId": "db" }, "target": { "moduleId": "db-01", "portId": "sql" } }] } })
    }

    #[test]
    fn a_well_formed_document_is_valid() {
        assert_eq!(validate(&doc()), Vec::<String>::new());
    }

    #[test]
    fn problems_are_named() {
        assert!(
            validate(&json!([1]))
                .iter()
                .any(|e| e.contains("JSON object"))
        );
        let mut d = doc();
        d["apiVersion"] = json!("v0");
        d["spec"]["modules"][1]["id"] = json!("web-01");
        d["spec"]["connections"][0]["source"]["portId"] = json!("http");
        d["spec"]["connections"][0]["target"]["moduleId"] = json!("web-01");
        let e = validate(&d);
        assert!(e.iter().any(|m| m.contains("apiVersion")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("appears twice")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("not an output port")), "{e:?}");
        assert!(e.iter().any(|m| m.contains("connected to itself")), "{e:?}");
        let mut d = doc();
        d["spec"]["connections"][0]["target"]["moduleId"] = json!("ghost");
        assert!(validate(&d).iter().any(|m| m.contains("unknown module")));
        let mut d = doc();
        d["spec"]["modules"][0]["ports"][0]["direction"] = json!("sideways");
        assert!(validate(&d).iter().any(|m| m.contains("direction")));
        let mut d = doc();
        d["spec"].as_object_mut().expect("spec").remove("modules");
        assert!(validate(&d).iter().any(|m| m.contains("spec.modules")));
    }
}
