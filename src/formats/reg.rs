/// Wine/Windows Registry (.reg) format support.
///
/// JSON representation:
/// ```json
/// {
///   "HKEY_CURRENT_USER\\Software\\Wine": {
///     "StringVal": {"type": "sz", "value": "hello"},
///     "DwordVal":  {"type": "dword", "value": 255},
///     "BinaryVal": {"type": "hex", "value": "01,02,03"},
///     "ExpandVal": {"type": "expand_sz", "value": "%SystemRoot%\\foo"},
///     "MultiVal":  {"type": "multi_sz", "value": ["line1", "line2"]},
///     "(default)": {"type": "sz", "value": "default string"}
///   }
/// }
/// ```
///
/// A `null` value for a key in the JSON patch means "delete this value" (emits `-"ValueName"`).
/// A `null` for a section key means "delete this key" (emits `[-HKEY_...]`).
///
/// Value type tags:
/// - `"sz"`        → `"value"` (plain string)
/// - `"expand_sz"` → `"value"` (expandable string, e.g. `%PATH%`)
/// - `"multi_sz"`  → `"value"` (array of strings)
/// - `"dword"`     → `"value"` (integer 0..=0xFFFF_FFFF)
/// - `"qword"`     → `"value"` (integer, serialised as hex:8 bytes LE)
/// - `"hex"`       → `"value"` (raw hex string `"01,02,ff"`)
/// - `"hex(N)"`    → `"value"` (raw hex for REG_* type N, pass-through)
use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Escape a string for emission inside `"…"` in a .reg file.
fn escape_reg_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out
}

/// Strip a BOM (U+FEFF) from the front of a string if present.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

/// Normalize a registry section key to always have a full hive prefix.
///
/// Wine's `user.reg` omits `HKEY_CURRENT_USER\\` from section headers,
/// writing e.g. `[Software\\Foo]` instead of `[HKEY_CURRENT_USER\\Software\\Foo]`.
/// Normalizing ensures patch keys (which use the full path) match parsed keys.
/// Also expands abbreviations (HKCU, HKLM, etc.) to full hive names.
fn normalize_section_key(key: &str) -> String {
    // Fix 4: Expand common abbreviations to full hive names for consistent key matching
    let key = match key.split_once('\\') {
        Some(("HKCU", rest)) => format!("HKEY_CURRENT_USER\\{rest}"),
        Some(("HKLM", rest)) => format!("HKEY_LOCAL_MACHINE\\{rest}"),
        Some(("HKCR", rest)) => format!("HKEY_CLASSES_ROOT\\{rest}"),
        Some(("HKU", rest)) => format!("HKEY_USERS\\{rest}"),
        Some(("HKCC", rest)) => format!("HKEY_CURRENT_CONFIG\\{rest}"),
        _ => key.to_string(),
    };
    // If the key starts with a known full hive prefix, return as-is.
    // Otherwise, add HKEY_CURRENT_USER\ prefix (Wine short-path format).
    const HIVE_PREFIXES: &[&str] = &[
        "HKEY_CURRENT_USER",
        "HKEY_LOCAL_MACHINE",
        "HKEY_CLASSES_ROOT",
        "HKEY_USERS",
        "HKEY_CURRENT_CONFIG",
    ];
    if HIVE_PREFIXES
        .iter()
        .any(|h| key == *h || key.starts_with(&format!("{h}\\")))
    {
        key
    } else {
        format!("HKEY_CURRENT_USER\\{key}")
    }
}

// ── parse ─────────────────────────────────────────────────────────────────────

/// Fix 6: Free function for flushing a logical line into a section map.
fn flush_logical(
    logical: &mut String,
    current_key: &Option<String>,
    root: &mut Map<String, Value>,
) -> Result<()> {
    let trimmed = logical.trim().to_string();
    // Fix 6: Use clear() instead of *logical = String::new()
    logical.clear();
    if trimmed.is_empty() {
        return Ok(());
    }
    let key = match current_key {
        Some(k) => k,
        None => return Ok(()), // value line before any section header — skip
    };
    let section = root
        .entry(key.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    let section_map = section
        .as_object_mut()
        .context("section must be an object")?;
    parse_value_line(&trimmed, section_map)
}

pub fn parse(input: &str) -> Result<Value> {
    let input = strip_bom(input);
    let mut lines = input.lines().peekable();

    // First non-empty line must be the header
    let header = lines
        .find(|l| !l.trim().is_empty())
        .context("empty .reg file")?
        .trim();

    if header != "Windows Registry Editor Version 5.00"
        && header != "REGEDIT4"
        && header != "WINE REGISTRY Version 2"
    {
        bail!(
            "unrecognised .reg header: '{}'; expected \
             'Windows Registry Editor Version 5.00', 'REGEDIT4', or 'WINE REGISTRY Version 2'",
            header
        );
    }

    let mut root: Map<String, Value> = Map::new();
    // Store the original header so serialize can round-trip it exactly.
    root.insert("__header__".to_string(), Value::String(header.to_string()));
    let mut current_key: Option<String> = None;
    // Preamble lines (comments/metadata before first section header).
    let mut preamble: Vec<Value> = Vec::new();

    // Collect logical lines (backslash-continuation)
    let mut logical = String::new();

    for raw_line in lines {
        // Strip inline comments only outside of quoted contexts — but .reg
        // comments are full-line (;), so we just check the start.
        let line = raw_line.trim_end();

        // Comment line or Wine metadata line (#time=..., #arch=..., etc.)
        if line.trim_start().starts_with(';') || line.trim_start().starts_with('#') {
            // flush any pending logical line first
            flush_logical(&mut logical, &current_key, &mut root)?;
            let trimmed = line.trim();
            if let Some(ref key) = current_key {
                // Per-section metadata: store as __time__ inside the section
                if let Some(val) = trimmed.strip_prefix("#time=") {
                    let section = root
                        .entry(key.clone())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Some(obj) = section.as_object_mut() {
                        obj.insert("__time__".to_string(), Value::String(val.to_string()));
                    }
                }
                // Other per-section # lines are silently dropped (rare)
            } else {
                // Pre-section metadata: accumulate into preamble
                preamble.push(Value::String(trimmed.to_string()));
            }
            continue;
        }

        // Empty line: flush pending; also accumulate into preamble if before first section
        if line.trim().is_empty() {
            flush_logical(&mut logical, &current_key, &mut root)?;
            if current_key.is_none() {
                preamble.push(Value::String(String::new()));
            }
            continue;
        }

        // Continuation line
        if logical.ends_with('\\') {
            logical.pop(); // remove trailing backslash
            logical.push_str(line.trim_start());
            continue;
        }

        // Flush any previously accumulated logical line
        flush_logical(&mut logical, &current_key, &mut root)?;

        let trimmed = line.trim();

        // Section header: [KEY] or [-KEY] (delete key)
        // Wine format may append a timestamp: [KEY] 1234567890
        if trimmed.starts_with('[') {
            // Find the closing bracket
            if let Some(close) = trimmed.find(']') {
                let inner = &trimmed[1..close];
                // Section header may have a trailing mtime: [Key] 1234567890
                let mtime = trimmed[close + 1..].trim();
                if let Some(deleted) = inner.strip_prefix('-') {
                    // Key deletion marker — store as null
                    let key = normalize_section_key(&deleted.replace("\\\\", "\\"));
                    root.insert(key, Value::Null);
                    current_key = None;
                } else {
                    let key = normalize_section_key(&inner.replace("\\\\", "\\"));
                    let section = root
                        .entry(key.clone())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if !mtime.is_empty() {
                        if let Some(obj) = section.as_object_mut() {
                            obj.entry("__mtime__".to_string())
                                .or_insert_with(|| Value::String(mtime.to_string()));
                        }
                    }
                    current_key = Some(key);
                }
            } else {
                bail!("malformed section header: '{}'", trimmed);
            }
            // Flush preamble into root now that we've seen the first section.
            // Strip trailing empty lines — the \r\n before each section in serialize
            // already provides the blank line separator.
            if !preamble.is_empty() {
                while preamble.last() == Some(&Value::String(String::new())) {
                    preamble.pop();
                }
                // Fix 7: Use std::mem::take instead of clone() + clear()
                root.insert(
                    "__preamble__".to_string(),
                    Value::Array(std::mem::take(&mut preamble)),
                );
            }
            continue;
        }

        // Otherwise accumulate as a logical line
        logical.push_str(trimmed);
    }

    // Flush final logical line
    flush_logical(&mut logical, &current_key, &mut root)?;

    // Any remaining preamble (file with no sections)
    if !preamble.is_empty() {
        root.insert("__preamble__".to_string(), Value::Array(preamble));
    }

    Ok(Value::Object(root))
}

/// Parse a single (fully-joined) value assignment line into `section_map`.
fn parse_value_line(line: &str, section_map: &mut Map<String, Value>) -> Result<()> {
    // Value deletion: -"ValueName"
    if let Some(rest) = line.strip_prefix('-') {
        let name = parse_value_name(rest)
            .with_context(|| format!("malformed value deletion: '{line}'"))?;
        section_map.insert(name, Value::Null);
        return Ok(());
    }

    // Normal: "ValueName"=<data>  or  @=<data>  (default value)
    let (name, data) =
        split_name_data(line).with_context(|| format!("malformed value line: '{line}'"))?;

    let entry =
        parse_data(data).with_context(|| format!("malformed data for value '{name}': '{data}'"))?;

    section_map.insert(name, entry);
    Ok(())
}

/// Extract the value name from a quoted string (or `@` for default).
fn parse_value_name(s: &str) -> Result<String> {
    let s = s.trim();
    if s == "@" {
        return Ok("(default)".to_string());
    }
    if !s.starts_with('"') {
        bail!("value name must be quoted or '@', got: '{s}'");
    }
    let (name, _rest) = extract_quoted(s)?;
    Ok(name)
}

/// Split `"ValueName"=data` → `(name, data)`.
fn split_name_data(line: &str) -> Result<(String, &str)> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('@') {
        // @=data  (default value)
        let data = rest.strip_prefix('=').context("expected '=' after '@'")?;
        return Ok(("(default)".to_string(), data.trim_start()));
    }

    if !line.starts_with('"') {
        bail!("expected quoted value name or '@'");
    }
    let (name, rest) = extract_quoted(line)?;
    let rest = rest.trim_start();
    let data = rest
        .strip_prefix('=')
        .context("expected '=' after value name")?;
    Ok((name, data.trim_start()))
}

/// Extract a `"…"`-quoted string from the start of `s`, returning
/// `(unescaped_content, remainder_after_closing_quote)`.
fn extract_quoted(s: &str) -> Result<(String, &str)> {
    debug_assert!(s.starts_with('"'));
    let s = &s[1..]; // skip opening quote
    let mut out = String::new();
    let mut chars = s.char_indices().peekable();
    loop {
        match chars.next() {
            None => bail!("unterminated quoted string"),
            Some((_, '\\')) => match chars.next() {
                Some((_, '\\')) => out.push('\\'),
                Some((_, '"')) => out.push('"'),
                // Fix 8: Handle \0 escape → NUL byte (Wine str(7) format)
                Some((_, '0')) => out.push('\0'),
                Some((_, c)) => {
                    out.push('\\');
                    out.push(c);
                }
                None => bail!("unterminated escape sequence"),
            },
            Some((i, '"')) => {
                // closing quote — return remainder
                let remainder = &s[i + 1..];
                return Ok((out, remainder));
            }
            Some((_, c)) => out.push(c),
        }
    }
}

/// Parse the data portion of a value assignment (everything after `=`).
fn parse_data(data: &str) -> Result<Value> {
    let data = data.trim();

    if data.starts_with('"') {
        // REG_SZ
        let (s, _) = extract_quoted(data)?;
        return Ok(make_entry("sz", Value::String(s)));
    }

    if let Some(hex) = data.strip_prefix("dword:") {
        let hex = hex.trim();
        let n = u32::from_str_radix(hex, 16)
            .with_context(|| format!("invalid dword value: '{hex}'"))?;
        return Ok(make_entry("dword", Value::Number(n.into())));
    }

    if data.starts_with("qword:") || data.starts_with("hex(b):") {
        // QWORD stored as 8-byte little-endian hex
        let hex_data = if let Some(hex_data) = data.strip_prefix("qword:") {
            hex_data.trim()
        } else {
            data["hex(b):".len()..].trim()
        };
        // Store as raw hex string — round-trip safe
        return Ok(make_entry("qword", Value::String(hex_data.to_string())));
    }

    if let Some(hex_data) = data.strip_prefix("hex(2):") {
        // REG_EXPAND_SZ — NUL-terminated UTF-16LE
        let hex_data = hex_data.trim();
        let s = decode_utf16le_hex(hex_data).context("decoding REG_EXPAND_SZ (hex(2))")?;
        // Fix 2: Strip trailing NUL terminator
        let s = s.trim_end_matches('\0').to_string();
        return Ok(make_entry("expand_sz", Value::String(s)));
    }

    if let Some(hex_data) = data.strip_prefix("hex(7):") {
        // REG_MULTI_SZ — double-NUL-terminated UTF-16LE
        let hex_data = hex_data.trim();
        let s = decode_utf16le_hex(hex_data).context("decoding REG_MULTI_SZ (hex(7))")?;
        // Split on inner NUL; drop trailing empty strings
        let parts: Vec<Value> = s
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(|p| Value::String(p.to_string()))
            .collect();
        return Ok(make_entry("multi_sz", Value::Array(parts)));
    }

    // Generic hex(N) or plain hex:
    if data.starts_with("hex") {
        let colon = data.find(':').context("expected ':' after hex type")?;
        let type_tag = &data[..colon]; // "hex" or "hex(N)"
        let hex_data = data[colon + 1..].trim();
        let tag = match type_tag {
            "hex" => "hex".to_string(),
            other => other.to_string(), // "hex(3)", "hex(4)", …
        };
        // Fix 5: Remove make_typed_entry, call make_entry directly
        return Ok(make_entry(&tag, Value::String(hex_data.to_string())));
    }

    // Wine-specific: str(N):"..." — human-readable encoding for string types
    // str(2) = REG_EXPAND_SZ, str(7) = REG_MULTI_SZ, etc.
    if let Some(rest) = data.strip_prefix("str(") {
        if let Some(close) = rest.find("):") {
            let n: u32 = rest[..close]
                .parse()
                .with_context(|| format!("invalid str(N) type: '{data}'"))?;
            let quoted = &rest[close + 2..];
            let (s, _) = extract_quoted(quoted)
                .with_context(|| format!("malformed str(N) value: '{data}'"))?;
            let type_tag = match n {
                1 => "sz",
                2 => "expand_sz",
                7 => "multi_sz",
                _ => "sz", // fallback: treat unknown str(N) as plain string
            };
            if n == 7 {
                // Multi-string: split on \0
                let parts: Vec<Value> = s
                    .split('\0')
                    .filter(|p| !p.is_empty())
                    .map(|p| Value::String(p.to_string()))
                    .collect();
                return Ok(make_entry(type_tag, Value::Array(parts)));
            }
            return Ok(make_entry(type_tag, Value::String(s)));
        }
    }

    bail!("unrecognised data format: '{data}'");
}

fn make_entry(type_tag: &str, value: Value) -> Value {
    let mut m = Map::new();
    m.insert("type".to_string(), Value::String(type_tag.to_string()));
    m.insert("value".to_string(), value);
    Value::Object(m)
}

// Fix 5: make_typed_entry removed — was a one-line wrapper around make_entry.

/// Decode a hex string like `"68,00,65,00,6c,00,6c,00,6f,00,00,00"` as UTF-16LE.
fn decode_utf16le_hex(hex: &str) -> Result<String> {
    let bytes = parse_hex_bytes(hex)?;
    if bytes.len() % 2 != 0 {
        bail!("UTF-16LE hex byte count must be even, got {}", bytes.len());
    }
    let u16s: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    String::from_utf16(&u16s).context("invalid UTF-16LE sequence")
}

/// Parse a comma-separated hex byte string (with optional line-continuation whitespace).
fn parse_hex_bytes(hex: &str) -> Result<Vec<u8>> {
    hex.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, s)| {
            u8::from_str_radix(s, 16).with_context(|| format!("invalid hex byte #{i}: '{s}'"))
        })
        .collect()
}

// ── serialize ─────────────────────────────────────────────────────────────────

pub fn serialize(value: &Value) -> Result<String> {
    let root = value.as_object().context("REG root must be an object")?;

    let header = root
        .get("__header__")
        .and_then(Value::as_str)
        .unwrap_or("Windows Registry Editor Version 5.00");
    let wine_format = header == "WINE REGISTRY Version 2";
    // Wine's user.reg uses LF line endings; standard .reg uses CRLF.
    let eol = if wine_format { "\n" } else { "\r\n" };
    let mut out = format!("{header}{eol}");

    // Emit preamble lines (comments/metadata before first section).
    // Each preamble line already includes blank lines as empty strings.
    if let Some(Value::Array(preamble)) = root.get("__preamble__") {
        for line in preamble {
            if let Some(s) = line.as_str() {
                out.push_str(s);
                out.push_str(eol);
            }
        }
    }

    for (section, section_val) in root {
        if section == "__header__" || section == "__preamble__" {
            continue;
        }
        out.push_str(eol);

        // Wine format uses short paths (without HKEY_CURRENT_USER\ prefix)
        // and escapes \ as \\ in section header names.
        let section_short = if wine_format {
            section
                .strip_prefix("HKEY_CURRENT_USER\\")
                .unwrap_or(section)
        } else {
            section.as_str()
        };

        // Fix 1: Wine format and REGEDIT4 use single backslashes in section headers;
        // standard format (v5) uses double backslashes.
        let section_name_escaped = if wine_format || header == "REGEDIT4" {
            section_short.to_string()
        } else {
            section_short.replace('\\', "\\\\")
        };
        let section_name = section_name_escaped.as_str();

        if section_val.is_null() {
            // Key deletion
            out.push('[');
            out.push('-');
            out.push_str(section_name);
            out.push(']');
            out.push_str(eol);
            continue;
        }

        let props = section_val
            .as_object()
            .with_context(|| format!("REG section '{section}' must be an object or null"))?;

        // Section header — emit with optional Wine mtime suffix
        out.push('[');
        out.push_str(section_name);
        out.push(']');
        if let Some(mtime) = props.get("__mtime__").and_then(Value::as_str) {
            out.push(' ');
            out.push_str(mtime);
        }
        out.push_str(eol);

        // Emit Wine #time= line if present
        if let Some(time) = props.get("__time__").and_then(Value::as_str) {
            out.push_str("#time=");
            out.push_str(time);
            out.push_str(eol);
        }

        for (name, entry) in props {
            if name == "__mtime__" || name == "__time__" {
                continue;
            }
            if entry.is_null() {
                // Value deletion
                out.push('-');
                out.push_str(&quote_value_name(name));
                out.push_str(eol);
                continue;
            }

            let entry_obj = entry.as_object().with_context(|| {
                format!("entry '{name}' must be an object {{type, value}} or null")
            })?;

            let type_tag = entry_obj
                .get("type")
                .and_then(Value::as_str)
                .with_context(|| format!("entry '{name}' missing 'type' field"))?;

            let val = entry_obj
                .get("value")
                .with_context(|| format!("entry '{name}' missing 'value' field"))?;

            let name_part = quote_value_name(name);
            // Fix 3: Pass eol through to serialize_data for fold_hex
            let data_part = serialize_data(type_tag, val, name, eol)
                .with_context(|| format!("serialising entry '{name}'"))?;

            out.push_str(&name_part);
            out.push('=');
            out.push_str(&data_part);
            out.push_str(eol);
        }
    }

    // Ensure trailing newline
    if !out.ends_with(eol) {
        out.push_str(eol);
    }

    Ok(out)
}

fn quote_value_name(name: &str) -> String {
    if name == "(default)" {
        return "@".to_string();
    }
    format!("\"{}\"", escape_reg_string(name))
}

// Fix 3: serialize_data now takes eol parameter and passes it to fold_hex
fn serialize_data(type_tag: &str, val: &Value, name: &str, eol: &str) -> Result<String> {
    match type_tag {
        "sz" => {
            let s = val
                .as_str()
                .with_context(|| format!("sz value for '{name}' must be a string"))?;
            Ok(format!("\"{}\"", escape_reg_string(s)))
        }

        "expand_sz" => {
            let s = val
                .as_str()
                .with_context(|| format!("expand_sz value for '{name}' must be a string"))?;
            // Fix 2: Add NUL terminator before encoding
            let with_nul = format!("{s}\0");
            let hex = encode_utf16le_hex(&with_nul);
            Ok(format!("hex(2):{}", fold_hex(&hex, eol)))
        }

        "multi_sz" => {
            let arr = val
                .as_array()
                .with_context(|| format!("multi_sz value for '{name}' must be an array"))?;
            let mut combined = String::new();
            for item in arr {
                let s = item
                    .as_str()
                    .with_context(|| format!("multi_sz items for '{name}' must be strings"))?;
                combined.push_str(s);
                combined.push('\0');
            }
            combined.push('\0'); // double-NUL terminator
            let hex = encode_utf16le_hex(&combined);
            Ok(format!("hex(7):{}", fold_hex(&hex, eol)))
        }

        "dword" => {
            let n = val
                .as_u64()
                .with_context(|| format!("dword value for '{name}' must be an integer"))?;
            if n > u32::MAX as u64 {
                bail!("dword value for '{name}' is out of range: {n}");
            }
            Ok(format!("dword:{:08x}", n as u32))
        }

        "qword" => {
            // Stored as raw hex string from parse, or as u64 integer
            match val {
                Value::String(s) => Ok(format!("hex(b):{}", fold_hex(s, eol))),
                Value::Number(n) => {
                    let v = n
                        .as_u64()
                        .with_context(|| format!("qword value for '{name}' must fit in u64"))?;
                    let bytes = v.to_le_bytes();
                    let hex = bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(",");
                    Ok(format!("hex(b):{}", fold_hex(&hex, eol)))
                }
                _ => bail!("qword value for '{name}' must be a string or number"),
            }
        }

        "hex" => {
            let s = val
                .as_str()
                .with_context(|| format!("hex value for '{name}' must be a string"))?;
            Ok(format!("hex:{}", fold_hex(s, eol)))
        }

        other if other.starts_with("hex(") => {
            let s = val
                .as_str()
                .with_context(|| format!("{other} value for '{name}' must be a string"))?;
            Ok(format!("{}:{}", other, fold_hex(s, eol)))
        }

        other => bail!("unknown value type '{other}' for '{name}'"),
    }
}

/// Encode a Rust `&str` as a UTF-16LE comma-separated hex byte string.
fn encode_utf16le_hex(s: &str) -> String {
    s.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Fold a long hex string to 76-char line width (regedit style).
/// Lines beyond the first are indented with two spaces.
/// Fix 3: Takes eol parameter so Wine files use LF and standard files use CRLF.
fn fold_hex(hex: &str, eol: &str) -> String {
    // Each byte is "xx," = 3 chars; 76 chars per line ≈ 25 bytes/line
    // We split on commas to avoid breaking a byte in the middle.
    if hex.is_empty() {
        return String::new();
    }
    let bytes: Vec<&str> = hex.split(',').collect();
    // ~25 bytes per line keeps lines under 80 chars when indented
    const BYTES_PER_LINE: usize = 25;
    let mut lines: Vec<String> = Vec::new();
    for chunk in bytes.chunks(BYTES_PER_LINE) {
        lines.push(chunk.join(","));
    }
    if lines.len() == 1 {
        return lines.remove(0);
    }
    // Join with ",\<eol>  " continuation
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(&format!(",\\{eol}  "));
        }
        out.push_str(line);
    }
    out
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Round-trip helper: parse → serialize → parse, check both intermediate
    /// and final values equal the expected JSON.
    fn round_trip(reg: &str) -> Result<Value> {
        let parsed = parse(reg)?;
        let serialised = serialize(&parsed)?;
        let reparsed = parse(&serialised)?;
        assert_eq!(parsed, reparsed, "round-trip mismatch");
        Ok(parsed)
    }

    fn make_sz(v: &str) -> Value {
        json!({"type": "sz", "value": v})
    }
    fn make_dword(v: u32) -> Value {
        json!({"type": "dword", "value": v})
    }
    fn make_hex(v: &str) -> Value {
        json!({"type": "hex", "value": v})
    }
    fn make_multi(v: Vec<&str>) -> Value {
        json!({"type": "multi_sz", "value": v})
    }

    // ── header ────────────────────────────────────────────────────────────────

    #[test]
    fn accepts_v5_header() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_CURRENT_USER\\Test]\r\n";
        let v = parse(reg).unwrap();
        assert!(v["HKEY_CURRENT_USER\\Test"].is_object());
    }

    #[test]
    fn accepts_regedit4_header() {
        let reg = "REGEDIT4\r\n\r\n[HKEY_CURRENT_USER\\Test]\r\n";
        let v = parse(reg).unwrap();
        assert!(v["HKEY_CURRENT_USER\\Test"].is_object());
    }

    #[test]
    fn rejects_unknown_header() {
        let reg = "Some Random Header\r\n\r\n[HKEY_CURRENT_USER\\Test]\r\n";
        assert!(parse(reg).is_err());
    }

    #[test]
    fn rejects_empty_file() {
        assert!(parse("").is_err());
        assert!(parse("   \r\n  ").is_err());
    }

    // ── string values ─────────────────────────────────────────────────────────

    #[test]
    fn parses_sz_value() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Wine]\r\n\
                   \"UseGLSL\"=\"enabled\"\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Wine"]["UseGLSL"], make_sz("enabled"));
    }

    #[test]
    fn parses_sz_with_escaped_backslash() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Path\"=\"C:\\\\Windows\\\\System32\"\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Test"]["Path"],
            make_sz("C:\\Windows\\System32")
        );
    }

    #[test]
    fn parses_sz_with_escaped_quote() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Msg\"=\"say \\\"hello\\\"\"\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Test"]["Msg"],
            make_sz("say \"hello\"")
        );
    }

    #[test]
    fn parses_empty_sz() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Empty\"=\"\"\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Empty"], make_sz(""));
    }

    #[test]
    fn parses_default_value() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   @=\"default string\"\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Test"]["(default)"],
            make_sz("default string")
        );
    }

    // ── dword ─────────────────────────────────────────────────────────────────

    #[test]
    fn parses_dword() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Version\"=dword:00000001\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Version"], make_dword(1));
    }

    #[test]
    fn parses_dword_max() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Max\"=dword:ffffffff\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Max"], make_dword(0xFFFF_FFFF));
    }

    #[test]
    fn parses_dword_zero() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Flag\"=dword:00000000\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Flag"], make_dword(0));
    }

    #[test]
    fn serializes_dword_zero_padded() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "Flag": {"type": "dword", "value": 255}
            }
        });
        let s = serialize(&v).unwrap();
        assert!(s.contains("\"Flag\"=dword:000000ff"), "got: {s}");
    }

    // ── hex ───────────────────────────────────────────────────────────────────

    #[test]
    fn parses_hex_binary() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Data\"=hex:01,02,03,ff\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Test"]["Data"],
            make_hex("01,02,03,ff")
        );
    }

    #[test]
    fn parses_hex_empty() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   \"Empty\"=hex:\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Empty"], make_hex(""));
    }

    // ── expand_sz ─────────────────────────────────────────────────────────────

    #[test]
    fn parses_expand_sz() {
        // "%SystemRoot%\foo" encoded as UTF-16LE
        let s = "%SystemRoot%\\foo";
        let hex = encode_utf16le_hex(&format!("{s}\0")); // NUL-terminated
        let reg = format!(
            "Windows Registry Editor Version 5.00\r\n\r\n\
             [HKEY_CURRENT_USER\\Test]\r\n\
             \"ExpandPath\"=hex(2):{hex}\r\n"
        );
        let v = parse(&reg).unwrap();
        // Fix 9: After Fix 2, trailing NUL is stripped; use precise equality check.
        let parsed_val = v["HKEY_CURRENT_USER\\Test"]["ExpandPath"]["value"]
            .as_str()
            .unwrap();
        assert_eq!(parsed_val, s, "expand_sz value mismatch");
    }

    #[test]
    fn round_trips_expand_sz_via_serialize() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "EP": {"type": "expand_sz", "value": "%TEMP%\\wine"}
            }
        });
        let s = serialize(&v).unwrap();
        assert!(s.contains("hex(2):"), "expected hex(2) in output: {s}");
        let reparsed = parse(&s).unwrap();
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["EP"]["value"],
            "%TEMP%\\wine"
        );
    }

    // ── multi_sz ──────────────────────────────────────────────────────────────

    #[test]
    fn round_trips_multi_sz() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "Fonts": {"type": "multi_sz", "value": ["Arial", "Times New Roman", "Courier"]}
            }
        });
        let s = serialize(&v).unwrap();
        assert!(s.contains("hex(7):"), "expected hex(7) in output: {s}");
        let reparsed = parse(&s).unwrap();
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["Fonts"],
            make_multi(vec!["Arial", "Times New Roman", "Courier"])
        );
    }

    #[test]
    fn round_trips_multi_sz_empty() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "Empty": {"type": "multi_sz", "value": []}
            }
        });
        let s = serialize(&v).unwrap();
        let reparsed = parse(&s).unwrap();
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["Empty"]["value"],
            json!([])
        );
    }

    // ── qword ─────────────────────────────────────────────────────────────────

    #[test]
    fn round_trips_qword_integer() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "Big": {"type": "qword", "value": 0x0102030405060708u64}
            }
        });
        let s = serialize(&v).unwrap();
        assert!(s.contains("hex(b):"), "expected hex(b): {s}");
        // Parse back — comes back as string
        let reparsed = parse(&s).unwrap();
        let hex_str = reparsed["HKEY_CURRENT_USER\\Test"]["Big"]["value"]
            .as_str()
            .unwrap();
        // Fix 10: Precise assertion for LE bytes of 0x0102030405060708
        assert_eq!(
            hex_str, "08,07,06,05,04,03,02,01",
            "qword LE bytes mismatch"
        );
    }

    #[test]
    fn round_trips_qword_string() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "Q": {"type": "qword", "value": "08,07,06,05,04,03,02,01"}
            }
        });
        let s = serialize(&v).unwrap();
        let reparsed = parse(&s).unwrap();
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["Q"]["value"],
            "08,07,06,05,04,03,02,01"
        );
    }

    // ── deletions ─────────────────────────────────────────────────────────────

    #[test]
    fn parses_value_deletion() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Test]\r\n\
                   -\"OldValue\"\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["OldValue"], Value::Null);
    }

    #[test]
    fn parses_key_deletion() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [-HKEY_CURRENT_USER\\ObsoleteKey]\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\ObsoleteKey"], Value::Null);
    }

    #[test]
    fn serializes_value_deletion() {
        let v = json!({
            "HKEY_CURRENT_USER\\Test": {
                "OldValue": null
            }
        });
        let s = serialize(&v).unwrap();
        assert!(s.contains("-\"OldValue\""), "got: {s}");
    }

    #[test]
    fn serializes_key_deletion() {
        let v = json!({
            "HKEY_CURRENT_USER\\ObsoleteKey": null
        });
        let s = serialize(&v).unwrap();
        assert!(
            s.contains("[-HKEY_CURRENT_USER\\\\ObsoleteKey]"),
            "got: {s}"
        );
    }

    // ── multiple sections / keys ───────────────────────────────────────────────

    #[test]
    fn parses_multiple_sections() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\A]\r\n\
                   \"x\"=\"1\"\r\n\
                   \r\n\
                   [HKCU\\B]\r\n\
                   \"y\"=dword:00000002\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\A"]["x"], make_sz("1"));
        assert_eq!(v["HKEY_CURRENT_USER\\B"]["y"], make_dword(2));
    }

    #[test]
    fn parses_multiple_values_in_section() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"a\"=\"alpha\"\r\n\
                   \"b\"=dword:0000000a\r\n\
                   \"c\"=hex:de,ad,be,ef\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["a"], make_sz("alpha"));
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["b"], make_dword(10));
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["c"], make_hex("de,ad,be,ef"));
    }

    // ── line continuation ─────────────────────────────────────────────────────

    #[test]
    fn parses_hex_with_line_continuation() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Long\"=hex:01,02,03,\\\r\n  04,05,06\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(
            v["HKEY_CURRENT_USER\\Test"]["Long"],
            make_hex("01,02,03,04,05,06")
        );
    }

    // ── comments ──────────────────────────────────────────────────────────────

    #[test]
    fn ignores_comments() {
        let reg = "Windows Registry Editor Version 5.00\r\n\
                   ; This is a comment\r\n\
                   \r\n\
                   [HKCU\\Test]\r\n\
                   ; another comment\r\n\
                   \"val\"=\"ok\"\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["val"], make_sz("ok"));
    }

    // ── BOM ───────────────────────────────────────────────────────────────────

    #[test]
    fn strips_utf8_bom() {
        let reg = "\u{FEFF}Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"x\"=\"1\"\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["x"], make_sz("1"));
    }

    // ── round-trips ───────────────────────────────────────────────────────────

    #[test]
    fn round_trip_sz() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Hello\"=\"world\"\r\n";
        round_trip(reg).unwrap();
    }

    #[test]
    fn round_trip_dword() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Num\"=dword:0000002a\r\n";
        round_trip(reg).unwrap();
    }

    #[test]
    fn round_trip_hex() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Bin\"=hex:ca,fe,ba,be\r\n";
        round_trip(reg).unwrap();
    }

    #[test]
    fn round_trip_deletions() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   -\"Dead\"\r\n\
                   \"Alive\"=\"yes\"\r\n";
        round_trip(reg).unwrap();
    }

    #[test]
    fn round_trip_key_deletion() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [-HKCU\\Gone]\r\n";
        round_trip(reg).unwrap();
    }

    // ── header preservation ───────────────────────────────────────────────────

    #[test]
    fn wine_registry_header_preserved_on_round_trip() {
        let reg = "WINE REGISTRY Version 2\r\n\r\n\
                   [HKEY_CURRENT_USER\\Software\\Wine]\r\n\
                   \"UseGLSL\"=\"enabled\"\r\n";
        let v = parse(reg).unwrap();
        let s = serialize(&v).unwrap();
        // Wine format uses LF line endings
        assert!(
            s.starts_with("WINE REGISTRY Version 2\n"),
            "header not preserved, got: {}",
            &s[..50.min(s.len())]
        );
        assert!(!s.contains("__header__"), "internal key leaked into output");
    }

    #[test]
    fn wine_header_preserved_after_merge() {
        use crate::merge::{merge, ArrayStrategy, MergeConfig};
        use std::collections::HashMap;

        let existing_reg = "WINE REGISTRY Version 2\r\n\r\n\
                            [HKEY_CURRENT_USER\\Software\\Wine]\r\n\
                            \"UseGLSL\"=\"disabled\"\r\n";
        let patch_reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                         [HKEY_CURRENT_USER\\Software\\Wine]\r\n\
                         \"UseGLSL\"=\"enabled\"\r\n";

        let existing_val = parse(existing_reg).unwrap();
        let mut patch_val = parse(patch_reg).unwrap();
        // Strip __header__ from patch (as main.rs does) so existing header wins
        if let Some(obj) = patch_val.as_object_mut() {
            obj.remove("__header__");
        }

        let config = MergeConfig {
            default_array: ArrayStrategy::Replace,
            path_strategies: HashMap::new(),
            clobber: true,
        };

        let merged = merge(existing_val, patch_val, &config, "");
        let output = serialize(&merged).unwrap();

        // Existing file's header wins (it's in the base, not the patch)
        // Wine format uses LF line endings
        assert!(
            output.starts_with("WINE REGISTRY Version 2\n"),
            "Wine header not preserved after merge, got: {}",
            &output[..60.min(output.len())]
        );
        assert!(!output.contains("__header__"), "internal key leaked");
    }

    // ── Wine metadata preservation ────────────────────────────────────────────

    #[test]
    fn wine_preamble_preserved_on_round_trip() {
        let reg = "WINE REGISTRY Version 2\r\n\
                   ;; All keys relative to REGISTRY\\\\User\\\\S-1-5\r\n\
                   \r\n\
                   #arch=win64\r\n\
                   \r\n\
                   [HKEY_CURRENT_USER\\Software\\Wine]\r\n\
                   #time=1dcba208437b3be\r\n\
                   \"UseGLSL\"=\"enabled\"\r\n";
        let v = parse(reg).unwrap();
        let s = serialize(&v).unwrap();
        assert!(s.contains("#arch=win64"), "preamble #arch missing: {s}");
        assert!(
            s.contains(";; All keys"),
            "preamble ;; comment missing: {s}"
        );
        assert!(s.contains("#time=1dcba208437b3be"), "#time missing: {s}");
        assert!(!s.contains("__preamble__"), "internal key leaked");
        assert!(!s.contains("__time__"), "internal key leaked");
    }

    #[test]
    fn wine_section_mtime_preserved_on_round_trip() {
        let reg = "WINE REGISTRY Version 2\r\n\r\n\
                   [HKEY_CURRENT_USER\\Console] 1774202272\r\n\
                   #time=1dcba25671a37f2\r\n\
                   \"ColorTable00\"=dword:00000000\r\n";
        let v = parse(reg).unwrap();
        let s = serialize(&v).unwrap();
        // Wine format strips HKEY_CURRENT_USER\ prefix on output
        assert!(
            s.contains("[Console] 1774202272"),
            "section mtime missing: {s}"
        );
        assert!(s.contains("#time=1dcba25671a37f2"), "#time missing: {s}");
        assert!(!s.contains("__mtime__"), "internal key leaked");
    }

    // ── Wine-specific real-world scenarios ────────────────────────────────────

    #[test]
    fn wine_direct3d_section() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_CURRENT_USER\\Software\\Wine\\Direct3D]\r\n\
                   \"MaxVersionGL\"=\"opengl32,3.3\"\r\n\
                   \"UseGLSL\"=\"enabled\"\r\n\
                   \"VideoMemorySize\"=\"512\"\r\n";
        let v = parse(reg).unwrap();
        let sec = &v["HKEY_CURRENT_USER\\Software\\Wine\\Direct3D"];
        assert_eq!(sec["MaxVersionGL"], make_sz("opengl32,3.3"));
        assert_eq!(sec["UseGLSL"], make_sz("enabled"));
        assert_eq!(sec["VideoMemorySize"], make_sz("512"));
    }

    #[test]
    fn wine_drives_section_with_dword() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_LOCAL_MACHINE\\System\\CurrentControlSet\\Services\\WineD3D]\r\n\
                   \"Version\"=dword:00000001\r\n\
                   \"Flags\"=dword:00000003\r\n";
        let v = parse(reg).unwrap();
        let sec = &v["HKEY_LOCAL_MACHINE\\System\\CurrentControlSet\\Services\\WineD3D"];
        assert_eq!(sec["Version"], make_dword(1));
        assert_eq!(sec["Flags"], make_dword(3));
    }

    #[test]
    fn wine_font_substitutes() {
        // Font substitutions are common in Wine prefix setups
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\FontSubstitutes]\r\n\
                   \"MS Shell Dlg\"=\"Tahoma\"\r\n\
                   \"MS Shell Dlg 2\"=\"Tahoma\"\r\n";
        let v = parse(reg).unwrap();
        let sec = &v["HKEY_LOCAL_MACHINE\\Software\\Microsoft\\Windows NT\\CurrentVersion\\FontSubstitutes"];
        assert_eq!(sec["MS Shell Dlg"], make_sz("Tahoma"));
        assert_eq!(sec["MS Shell Dlg 2"], make_sz("Tahoma"));
    }

    #[test]
    fn wine_full_patch_scenario() {
        // Simulate patching a Wine prefix: add D3D settings to existing minimal reg
        use crate::merge::{merge, ArrayStrategy, MergeConfig};
        use std::collections::HashMap;

        let existing_reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                            [HKEY_CURRENT_USER\\Software\\Wine]\r\n\
                            \"Version\"=\"6.0\"\r\n";

        let patch_reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                         [HKEY_CURRENT_USER\\Software\\Wine\\Direct3D]\r\n\
                         \"UseGLSL\"=\"enabled\"\r\n\
                         \"MaxVersionGL\"=\"opengl32,4.6\"\r\n";

        let existing_val = parse(existing_reg).unwrap();
        let patch_val = parse(patch_reg).unwrap();

        let config = MergeConfig {
            default_array: ArrayStrategy::Replace,
            path_strategies: HashMap::new(),
            clobber: true,
        };

        let merged = merge(existing_val, patch_val, &config, "");
        let output = serialize(&merged).unwrap();
        let reparsed = parse(&output).unwrap();

        // Original key preserved
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Software\\Wine"]["Version"],
            make_sz("6.0")
        );
        // Patch applied
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Software\\Wine\\Direct3D"]["UseGLSL"],
            make_sz("enabled")
        );
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Software\\Wine\\Direct3D"]["MaxVersionGL"],
            make_sz("opengl32,4.6")
        );
    }

    #[test]
    fn wine_no_clobber_preserves_user_override() {
        // If a user has manually tweaked a wine setting, no-clobber must preserve it
        use crate::merge::{merge, ArrayStrategy, MergeConfig};
        use std::collections::HashMap;

        let existing_reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                            [HKEY_CURRENT_USER\\Software\\Wine\\Direct3D]\r\n\
                            \"UseGLSL\"=\"disabled\"\r\n";

        let patch_reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                         [HKEY_CURRENT_USER\\Software\\Wine\\Direct3D]\r\n\
                         \"UseGLSL\"=\"enabled\"\r\n\
                         \"NewSetting\"=\"value\"\r\n";

        let existing_val = parse(existing_reg).unwrap();
        let patch_val = parse(patch_reg).unwrap();

        let config = MergeConfig {
            default_array: ArrayStrategy::Replace,
            path_strategies: HashMap::new(),
            clobber: false,
        };

        let merged = merge(existing_val, patch_val, &config, "");
        let output = serialize(&merged).unwrap();
        let reparsed = parse(&output).unwrap();

        // User's override preserved
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Software\\Wine\\Direct3D"]["UseGLSL"],
            make_sz("disabled")
        );
        // New key from patch still added
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Software\\Wine\\Direct3D"]["NewSetting"],
            make_sz("value")
        );
    }

    // Fix 11: Wine short-path normalization tests
    #[test]
    fn wine_short_path_normalized_on_parse() {
        // Real user.reg uses [Software\Wine] without HKEY_CURRENT_USER\ prefix.
        // normalize_section_key must expand it so patch keys (full form) match.
        let reg = "WINE REGISTRY Version 2\n\n\
                   [Software\\\\Wine]\n\
                   \"UseGLSL\"=\"enabled\"\n";
        let v = parse(reg).unwrap();
        // key must be stored under the full path
        assert_eq!(
            v["HKEY_CURRENT_USER\\Software\\Wine"]["UseGLSL"],
            make_sz("enabled"),
            "Wine short-path was not normalized to full HKEY_CURRENT_USER\\ prefix"
        );
    }

    #[test]
    fn wine_short_path_round_trips() {
        let reg = "WINE REGISTRY Version 2\n\n\
                   [Software\\\\Wine\\\\Direct3D]\n\
                   \"UseGLSL\"=\"enabled\"\n\
                   \"VideoMemorySize\"=dword:00000200\n";
        let v = parse(reg).unwrap();
        let s = serialize(&v).unwrap();
        // Wine output must use the short path (no HKEY_CURRENT_USER\ prefix)
        assert!(
            s.contains("[Software\\Wine\\Direct3D]"),
            "Wine serialize must emit short paths without HKEY_CURRENT_USER: {s}"
        );
        assert!(
            !s.contains("HKEY_CURRENT_USER"),
            "full prefix must not appear in Wine output: {s}"
        );
        // And must be parseable again
        let reparsed = parse(&s).unwrap();
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Software\\Wine\\Direct3D"]["UseGLSL"],
            make_sz("enabled")
        );
    }

    // Fix 12: str(N) Wine format tests
    #[test]
    fn wine_str1_plain_string() {
        // str(1):"..." is Wine's human-readable REG_SZ
        let reg = "WINE REGISTRY Version 2\n\n\
                   [Software\\\\Test]\n\
                   \"Plain\"=str(1):\"hello world\"\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Software\\Test"]["Plain"]["type"],
            "sz"
        );
        assert_eq!(
            v["HKEY_CURRENT_USER\\Software\\Test"]["Plain"]["value"],
            "hello world"
        );
    }

    #[test]
    fn wine_str2_expand_sz() {
        // str(2):"..." is Wine's human-readable REG_EXPAND_SZ
        let reg = "WINE REGISTRY Version 2\n\n\
                   [Software\\\\Test]\n\
                   \"Path\"=str(2):\"%SystemRoot%\\\\system32\"\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Software\\Test"]["Path"]["type"],
            "expand_sz"
        );
        assert_eq!(
            v["HKEY_CURRENT_USER\\Software\\Test"]["Path"]["value"],
            "%SystemRoot%\\system32"
        );
    }

    #[test]
    fn wine_str7_multi_sz() {
        // str(7):"..." is Wine's human-readable REG_MULTI_SZ; NUL separates strings
        let reg = "WINE REGISTRY Version 2\n\n\
                   [Software\\\\Test]\n\
                   \"Fonts\"=str(7):\"Arial\\0Times New Roman\\0\"\n";
        let v = parse(reg).unwrap();
        assert_eq!(
            v["HKEY_CURRENT_USER\\Software\\Test"]["Fonts"]["type"],
            "multi_sz"
        );
        let parts = v["HKEY_CURRENT_USER\\Software\\Test"]["Fonts"]["value"]
            .as_array()
            .unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "Arial");
        assert_eq!(parts[1], "Times New Roman");
    }

    // Fix 13: hex(b): QWORD alternate form test
    #[test]
    fn parses_qword_hex_b_form() {
        // hex(b): is an alternate representation for QWORD (same as qword: but as raw hex)
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Q\"=hex(b):08,07,06,05,04,03,02,01\r\n";
        let v = parse(reg).unwrap();
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Q"]["type"], "qword");
        assert_eq!(
            v["HKEY_CURRENT_USER\\Test"]["Q"]["value"],
            "08,07,06,05,04,03,02,01"
        );
    }

    // ── error cases ───────────────────────────────────────────────────────────

    #[test]
    fn rejects_malformed_dword() {
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Bad\"=dword:ZZZZZZZZ\r\n";
        assert!(parse(reg).is_err());
    }

    #[test]
    fn hex_stored_as_raw_passthrough() {
        // hex: values are stored verbatim — we don't validate the byte content
        // (they are opaque binary blobs). Round-trip must preserve the raw string.
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Good\"=hex:01,ab,ff\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Good"], make_hex("01,ab,ff"));
    }

    #[test]
    fn rejects_dword_out_of_range_on_serialize() {
        let v = json!({
            "HKCU\\Test": {
                "Bad": {"type": "dword", "value": 0x1_0000_0000u64}
            }
        });
        assert!(serialize(&v).is_err());
    }

    #[test]
    fn rejects_non_object_root_on_serialize() {
        assert!(serialize(&Value::String("nope".into())).is_err());
    }

    #[test]
    fn rejects_non_object_section_on_serialize() {
        let v = json!({ "HKCU\\Test": "not an object" });
        assert!(serialize(&v).is_err());
    }

    #[test]
    fn rejects_missing_type_field() {
        let v = json!({ "HKCU\\Test": { "val": {"value": "x"} } });
        assert!(serialize(&v).is_err());
    }

    #[test]
    fn rejects_missing_value_field() {
        let v = json!({ "HKCU\\Test": { "val": {"type": "sz"} } });
        assert!(serialize(&v).is_err());
    }

    #[test]
    fn rejects_unknown_value_type() {
        let v = json!({ "HKCU\\Test": { "val": {"type": "bogus", "value": "x"} } });
        assert!(serialize(&v).is_err());
    }

    // ── edge cases ────────────────────────────────────────────────────────────

    #[test]
    fn empty_section_serializes_and_reparses() {
        let v = json!({ "HKCU\\Empty": {} });
        let s = serialize(&v).unwrap();
        assert!(s.contains("[HKCU\\\\Empty]"), "got: {s}");
        let reparsed = parse(&s).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER on parse
        assert_eq!(reparsed["HKEY_CURRENT_USER\\Empty"], json!({}));
    }

    #[test]
    fn value_name_with_spaces_and_backslash() {
        let v = json!({
            "HKCU\\Test": {
                "My Value\\Path": {"type": "sz", "value": "test"}
            }
        });
        let s = serialize(&v).unwrap();
        let reparsed = parse(&s).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER on parse
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["My Value\\Path"],
            make_sz("test")
        );
    }

    #[test]
    fn value_with_unicode_string() {
        let v = json!({
            "HKCU\\Test": {
                "Unicode": {"type": "sz", "value": "日本語テスト"}
            }
        });
        let s = serialize(&v).unwrap();
        let reparsed = parse(&s).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER on parse
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["Unicode"],
            make_sz("日本語テスト")
        );
    }

    #[test]
    fn lf_only_line_endings_accepted() {
        // Some tools write \n instead of \r\n; we should handle both
        let reg = "Windows Registry Editor Version 5.00\n\n[HKCU\\Test]\n\"x\"=\"ok\"\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["x"], make_sz("ok"));
    }

    #[test]
    fn hex_passthrough_type() {
        // hex(3) = REG_BINARY variant — should pass through
        let reg = "Windows Registry Editor Version 5.00\r\n\r\n\
                   [HKCU\\Test]\r\n\
                   \"Custom\"=hex(3):01,02\r\n";
        let v = parse(reg).unwrap();
        // Fix 4: HKCU is now expanded to HKEY_CURRENT_USER
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Custom"]["type"], "hex(3)");
        assert_eq!(v["HKEY_CURRENT_USER\\Test"]["Custom"]["value"], "01,02");
        // Round-trip
        let s = serialize(&v).unwrap();
        let reparsed = parse(&s).unwrap();
        assert_eq!(
            reparsed["HKEY_CURRENT_USER\\Test"]["Custom"]["type"],
            "hex(3)"
        );
    }
}
