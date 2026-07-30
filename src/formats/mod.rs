mod ini;
mod json;
mod reg;
mod toml;
mod yaml;

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Json,
    Toml,
    Yaml,
    Ini,
    Reg,
}

pub fn parse(input: &str, format: Format) -> Result<Value> {
    match format {
        Format::Json => json::parse(input),
        Format::Toml => toml::parse(input),
        Format::Yaml => yaml::parse(input),
        Format::Ini => ini::parse(input),
        Format::Reg => reg::parse(input),
    }
}

pub fn serialize(value: &Value, format: Format) -> Result<String> {
    match format {
        Format::Json => json::serialize(value),
        Format::Toml => toml::serialize(value),
        Format::Yaml => yaml::serialize(value),
        Format::Ini => ini::serialize(value),
        Format::Reg => reg::serialize(value),
    }
}
