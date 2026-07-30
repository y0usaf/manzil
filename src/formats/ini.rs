use anyhow::{Context, Result};
use ini::Ini;
use serde_json::{Map, Value};

const GLOBAL_SECTION: &str = "__global__";

/// Parse INI into JSON structure:
/// { "section": { "key": "value" }, "__global__": { "key": "value" } }
pub fn parse(input: &str) -> Result<Value> {
    let ini = Ini::load_from_str(input).context("invalid INI")?;
    let mut root = Map::new();

    for (section, props) in &ini {
        let section_name = section.unwrap_or(GLOBAL_SECTION);
        // Use entry API to handle duplicate section headers by merging their keys
        let section_obj = root
            .entry(section_name.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(section_map) = section_obj {
            for (key, value) in props.iter() {
                section_map.insert(key.to_string(), Value::String(value.to_string()));
            }
        }
    }

    Ok(Value::Object(root))
}

pub fn serialize(value: &Value) -> Result<String> {
    let obj = value.as_object().context("INI root must be an object")?;

    let mut ini = Ini::new();

    // Write sectionless (__global__) keys first so they appear before any [section] headers
    if let Some(props) = obj.get(GLOBAL_SECTION) {
        let props = props
            .as_object()
            .with_context(|| format!("INI section '{GLOBAL_SECTION}' must be an object"))?;
        for (key, value) in props {
            let str_val = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => continue,
                _ => anyhow::bail!(
                    "INI values must be scalars, got nested structure at [{GLOBAL_SECTION}].{key}"
                ),
            };
            ini.with_section(None::<String>).set(key, str_val);
        }
    }

    for (section, props) in obj {
        if section == GLOBAL_SECTION {
            continue;
        }
        let props = props
            .as_object()
            .with_context(|| format!("INI section '{section}' must be an object"))?;

        for (key, value) in props {
            let str_val = match value {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => continue,
                _ => anyhow::bail!(
                    "INI values must be scalars, got nested structure at [{section}].{key}"
                ),
            };
            ini.with_section(Some(section.as_str())).set(key, str_val);
        }
    }

    let mut buf = Vec::new();
    ini.write_to(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}
