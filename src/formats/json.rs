use anyhow::Result;
use serde_json::Value;

pub fn parse(input: &str) -> Result<Value> {
    Ok(serde_json::from_str(input)?)
}

pub fn serialize(value: &Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)? + "\n")
}
