//! TOML parser — delegates to `toml` crate for standard TOML parsing.
//! TOML's dotted keys like [lower.Icmp.Equal] create nested tables.
//! We flatten them to flat HashMap keys for the model.

use crate::model::IsaModel;

pub fn parse(source: &str) -> Result<IsaModel, String> {
    let mut raw: toml::Value = toml::from_str(source).map_err(|e| format!("TOML: {e}"))?;

    // Flatten [lower.X.Y] → lower["X.Y"] and [lower_term.X] → lower_term["X"]
    if let Some(table) = raw.as_table_mut() {
        flatten_nested(table, "lower");
        flatten_nested(table, "lower_term");
    }

    let model: IsaModel = raw.try_into().map_err(|e| format!("deserialize: {e}"))?;
    Ok(model)
}

/// Flatten nested tables under `key`:
/// { key: { A: { B: value } } } → { key: { "A.B": value } }
/// Only flattens when the leaf values are tables (maps), not arrays.
/// This handles [lower.Icmp.Equal] → lower["Icmp.Equal"] while
/// leaving [lower.Band] → lower["Band"] intact.
fn flatten_nested(table: &mut toml::Table, key: &str) {
    let Some(toml::Value::Table(inner)) = table.get(key).cloned() else { return };
    let mut new_entries: Vec<(String, toml::Value)> = Vec::new();
    let mut keys_to_remove: Vec<String> = Vec::new();

    for (k, v) in &inner {
        if let toml::Value::Table(sub) = v {
            // Only flatten if sub does NOT already contain "insts" (i.e. is not a direct LowerRule)
            // [lower.Icmp.Equal] → Icmp:{Equal:{insts:[]}} → Equal has insts → flatten to Icmp.Equal
            // [lower.Band] → Band:{insts:[]} → Band has insts → DON'T flatten
            if sub.contains_key("insts") {
                let new_key = k.clone();
                new_entries.push((new_key, v.clone()));
                keys_to_remove.push(k.clone());
            } else {
                // Deeper nesting: recurse-style flatten
                for (sk, sv) in sub {
                    let new_key = format!("{k}.{sk}");
                    new_entries.push((new_key, sv.clone()));
                }
                keys_to_remove.push(k.clone());
            }
        }
    }

    let Some(inner_mut) = table.get_mut(key).and_then(|v| v.as_table_mut()) else {
        return;
    };
    for k in &keys_to_remove {
        inner_mut.remove(k);
    }
    for (k, v) in &new_entries {
        inner_mut.insert(k.clone(), v.clone());
    }
}
