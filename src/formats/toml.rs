use anyhow::{Context, Result};
use serde_json::Value;

pub fn parse(input: &str) -> Result<Value> {
    let toml_val: ::toml::Value = input.parse().context("invalid TOML")?;
    toml_to_json(toml_val)
}

pub fn serialize(value: &Value) -> Result<String> {
    let toml_val = json_to_toml(value)?;
    let s = ::toml::to_string_pretty(&toml_val)?;
    // toml::to_string_pretty always ends with \n; the else branch is a safety net
    Ok(if s.ends_with('\n') { s } else { s + "\n" })
}

fn toml_to_json(val: ::toml::Value) -> anyhow::Result<Value> {
    Ok(match val {
        ::toml::Value::String(s) => Value::String(s),
        ::toml::Value::Integer(i) => Value::Number(i.into()),
        ::toml::Value::Float(f) => serde_json::Number::from_f64(f)
            .map(Value::Number)
            .ok_or_else(|| anyhow::anyhow!("TOML float is non-finite (NaN or Infinity): {f}"))?,
        ::toml::Value::Boolean(b) => Value::Bool(b),
        ::toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        ::toml::Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(toml_to_json)
                .collect::<anyhow::Result<_>>()?,
        ),
        ::toml::Value::Table(tbl) => {
            let map = tbl
                .into_iter()
                .map(|(k, v)| Ok((k, toml_to_json(v)?)))
                .collect::<anyhow::Result<_>>()?;
            Value::Object(map)
        }
    })
}

fn json_to_toml(val: &Value) -> Result<::toml::Value> {
    Ok(match val {
        Value::Null => anyhow::bail!("TOML does not support null values"),
        Value::Bool(b) => ::toml::Value::Boolean(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                ::toml::Value::Integer(i)
            } else if n.as_u64().is_some() {
                // u64 that doesn't fit in i64: TOML only supports signed 64-bit integers,
                // and silently converting to f64 would lose precision for large values.
                anyhow::bail!(
                    "TOML integer out of range: {} exceeds i64::MAX ({}). \
                     TOML only supports signed 64-bit integers.",
                    n,
                    i64::MAX
                )
            } else if let Some(f) = n.as_f64() {
                ::toml::Value::Float(f)
            } else {
                anyhow::bail!("unsupported number: {n}")
            }
        }
        Value::String(s) => ::toml::Value::String(s.clone()),
        Value::Array(arr) => {
            ::toml::Value::Array(arr.iter().map(json_to_toml).collect::<Result<_>>()?)
        }
        Value::Object(map) => {
            let tbl = map
                .iter()
                .map(|(k, v)| Ok((k.clone(), json_to_toml(v)?)))
                .collect::<Result<_>>()?;
            ::toml::Value::Table(tbl)
        }
    })
}
