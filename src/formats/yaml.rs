use anyhow::Result;
use serde_json::Value;

pub fn parse(input: &str) -> Result<Value> {
    // Detect multi-document YAML streams. serde_yml silently parses only
    // the first document, which would silently discard data.
    //
    // A document separator must appear at column 0 (no leading whitespace).
    // Lines indented with spaces are block-scalar content, not separators.
    let trimmed = input.trim_start();
    let body = trimmed.strip_prefix("---").unwrap_or(trimmed);
    let has_second_doc = body.lines().any(|line| {
        // Must be exactly "---" (possibly with trailing whitespace) at column 0
        !line.starts_with(' ') && !line.starts_with('\t') && line.trim() == "---"
    });
    if has_second_doc {
        anyhow::bail!(
            "multi-document YAML is not supported (found multiple '---' document separators); \
             use a single document"
        );
    }
    Ok(serde_yml::from_str(input)?)
}

pub fn serialize(value: &Value) -> Result<String> {
    let s = serde_yml::to_string(value)?;
    let s = s
        .strip_prefix("---\n")
        .or_else(|| s.strip_prefix("---\r\n"))
        .unwrap_or(&s);
    let mut s = s.to_string();
    if !s.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}
