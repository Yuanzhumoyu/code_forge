//! Bitfield interpolation string parser for ISA instruction encoding.
//!
//! # Syntax
//!
//! - **Fixed-width**: `"32 {dest:7:5}{0x33:0:7}"` — 32-bit instruction
//! - **Segmented**: `"[REX]{...} {...}"` or `"?cond {...}"` — variable-length with conditional segments
//! - **Fixup**: `"!rel4"`, `"!isa1"` — label fixup placeholder (branch/jump targets)
//! - **Primitive**: `"@name arg1 arg2"` — named encoding primitive
//!
//! # Field syntax within `{...}`
//!
//! ```text
//! {VALUE:OFFSET:WIDTH}           — basic: value at bit range [offset, offset+width)
//! {VALUE:[OFFSET;WIDTH]}          — bracket form (same meaning)
//! {VALUE:[OFFSET;WIDTH;SHIFT]}    — bracket form with bit-extraction shift
//! ```
//!
//! | Element  | Meaning                                      | Example    |
//! |----------|----------------------------------------------|------------|
//! | `VALUE`  | field name, hex literal (`0xNN`), or decimal | `dest`     |
//! | `OFFSET` | starting bit position (0 = LSB)              | `7`        |
//! | `WIDTH`  | number of bits                               | `5`        |
//! | `SHIFT`  | optional: right-shift before placing         | `3`        |
//!
//! # Segment delimiters
//!
//! - `?condition` inline condition, e.g., `?dest>=8||src>=8`
//! - Bare `{...}` groups start unnamed (always-emitted) segments
//! - `!fixup_kind` marks a label fixup placeholder segment
//! - Whitespace separates segments
//!
//! # Fixup kinds
//!
//! | Syntax   | Bytes | RelocKind    | Use case                  |
//! |----------|-------|--------------|---------------------------|
//! | `!rel4`  | 4     | REL4         | x86 JMP/JCC rel32         |
//! | `!isa1`  | 4     | Isa(1)       | RISC-V B-type branch      |
//! | `!isa2`  | 4     | Isa(2)       | RISC-V J-type jump        |
//!
//! # Examples
//!
//! RISC-V ADD (R-type, 32-bit fixed):
//! ```text
//! 32 {dest:7:5}{0:12:3}{src1:15:5}{src2:20:5}{0:25:7}{0x33:0:7}
//! ```
//!
//! x86 ADD (REX prefix + opcode + ModRM):
//! ```text
//! [REX]{0x4:[0;4]}{1:4:1}{dest:[5;1;3]}{src:[7;1;3]} {0x01:[0;8]} {dest:[0;3]}{src:[3;3]}{3:[6;2]}
//! ```
//!
//! x86 ADD with inline condition:
//! ```text
//! ?dest>=8||src>=8 {0x4:[0;4]}{1:[3;1]}{dest:[2;1;3]}{0:[1;1]}{src:[0;1;3]} {0x01:[0;8]} {dest:[0;3]}{src:[3;3]}{3:[6;2]}
//! ```
//!
//! x86 JMP rel32 (fixup):
//! ```text
//! {0xE9:[0;8]} !rel4
//! ```
//!
//! AArch64 ADD (32-bit fixed):
//! ```text
//! 32 {dest:0:5}{src1:5:5}{src2:16:5}{0x22C:21:10}{1:31:1}
//! ```

use std::fmt;

// ============================================================
// Parsed types
// ============================================================

/// Parsed encoding string representation.
#[derive(Debug, Clone)]
pub enum ParsedEncoding {
    /// Fixed-width: `"32 {dest:7:5}{0x33:0:7}"` or `"32 {...} !isa1"`
    Fixed {
        width: u32,
        fields: Vec<ParsedBitField>,
        fixup: Option<FixupKind>,
    },
    /// Variable-length: `"[REX]{...} {...}"`
    Segmented { segments: Vec<ParsedSegment> },
    /// Named primitive: `"@lea_sib"` or `"@leb128 imm"`
    Primitive { name: String, args: Vec<String> },
}

/// Label fixup kind for branch/jump instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixupKind {
    /// 4-byte relative offset (x86 JMP/JCC rel32)
    Rel4,
    /// ISA-specific fixup type 0 (AArch64 B/BL, 26-bit offset)
    Isa0,
    /// ISA-specific fixup type 1 (RISC-V B-type)
    Isa1,
    /// ISA-specific fixup type 2 (RISC-V J-type)
    Isa2,
}

impl FixupKind {
    /// Number of placeholder bytes to emit before recording the fixup.
    pub fn width(&self) -> u32 {
        match self {
            FixupKind::Rel4 | FixupKind::Isa0 | FixupKind::Isa1 | FixupKind::Isa2 => 32,
        }
    }

    /// Parse from the `!kind` suffix string.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rel4" => Some(FixupKind::Rel4),
            "isa0" => Some(FixupKind::Isa0),
            "isa1" => Some(FixupKind::Isa1),
            "isa2" => Some(FixupKind::Isa2),
            _ => None,
        }
    }
}

impl fmt::Display for FixupKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixupKind::Rel4 => write!(f, "rel4"),
            FixupKind::Isa0 => write!(f, "isa0"),
            FixupKind::Isa1 => write!(f, "isa1"),
            FixupKind::Isa2 => write!(f, "isa2"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedSegment {
    /// Inline condition expression, e.g., `"dest>=8 || src>=8"`. Parsed from `?...` prefix.
    pub condition: Option<String>,
    /// Width of this segment in bits (8, 16, 32, or 64). Zero for primitive/fixup-only segments.
    pub width: u32,
    /// Bit fields within this segment (empty for primitive/fixup-only segments).
    pub fields: Vec<ParsedBitField>,
    /// Label fixup kind. Parsed from `!kind` suffix.
    pub fixup: Option<FixupKind>,
    /// Inline primitive call: `@name arg1 arg2` within a segmented encoding.
    pub primitive: Option<(String, Vec<String>)>,
}

#[derive(Debug, Clone)]
pub struct ParsedBitField {
    pub value: BitFieldValue,
    pub offset: u8,
    pub width: u8,
    /// Optional right-shift applied to the value before placing at `offset`.
    pub shift: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BitFieldValue {
    /// Reference to an instruction field (e.g., "dest", "src1")
    Field(String),
    /// Hex literal (e.g., 0x33 → Hex(51))
    Hex(u64),
    /// Decimal literal (e.g., 1, 0, 3)
    Dec(i64),
}

impl fmt::Display for BitFieldValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Field(name) => write!(f, "{name}"),
            Self::Hex(v) => write!(f, "0x{v:X}"),
            Self::Dec(v) => write!(f, "{v}"),
        }
    }
}

// ============================================================
// Parse errors
// ============================================================

#[derive(Debug)]
pub enum ParseError {
    UnexpectedChar { pos: usize, ch: char },
    UnclosedBrace { pos: usize },
    InvalidHex { pos: usize, text: String },
    InvalidNumber { pos: usize, text: String },
    MissingBitSpec { pos: usize },
    InvalidBitSpec { pos: usize, text: String },
    EmptyEncoding,
    EmptySegment { pos: usize },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedChar { pos, ch } => {
                write!(f, "unexpected char '{ch}' at position {pos}")
            }
            Self::UnclosedBrace { pos } => {
                write!(f, "unclosed '{{' at position {pos}")
            }
            Self::InvalidHex { pos, text } => {
                write!(f, "invalid hex literal '{text}' at position {pos}")
            }
            Self::InvalidNumber { pos, text } => {
                write!(f, "invalid number '{text}' at position {pos}")
            }
            Self::MissingBitSpec { pos } => {
                write!(f, "expected ':' after bitfield value at position {pos}")
            }
            Self::InvalidBitSpec { pos, text } => {
                write!(f, "invalid bit specification '{text}' at position {pos}")
            }
            Self::EmptyEncoding => write!(f, "empty encoding string"),
            Self::EmptySegment { pos } => write!(f, "empty segment at position {pos}"),
        }
    }
}

// ============================================================
// Parser
// ============================================================

/// Parse an encoding string into [`ParsedEncoding`].
pub fn parse_encoding(input: &str) -> Result<ParsedEncoding, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError::EmptyEncoding);
    }

    // Primitive: "@name" or "@name arg1 arg2"
    if input.starts_with('@') {
        return parse_primitive(input);
    }

    // Fixed-width: "32 {...}" or "16 {...}"
    if let Some(first_char) = input.chars().next()
        && first_char.is_ascii_digit()
    {
        return parse_fixed(input);
    }

    // Segmented: "[NAME]{...} {...}" or bare "{...}{...}"
    parse_segmented(input)
}

fn parse_primitive(input: &str) -> Result<ParsedEncoding, ParseError> {
    let rest = &input[1..]; // skip '@'
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.is_empty() {
        return Err(ParseError::EmptyEncoding);
    }
    Ok(ParsedEncoding::Primitive {
        name: parts[0].to_string(),
        args: parts[1..].iter().map(|s| s.to_string()).collect(),
    })
}

fn parse_fixed(input: &str) -> Result<ParsedEncoding, ParseError> {
    // Extract leading number
    let width_str: String = input.chars().take_while(|c| c.is_ascii_digit()).collect();
    let width: u32 = width_str.parse().map_err(|_| ParseError::InvalidNumber {
        pos: 0,
        text: width_str.clone(),
    })?;
    let rest = &input[width_str.len()..].trim_start();

    // Check for trailing `!fixup` after the bitfields
    let fixup = if let Some(excl_pos) = rest.rfind('!') {
        let after = rest[excl_pos + 1..].trim();
        let kind = FixupKind::parse(after).ok_or_else(|| ParseError::UnexpectedChar {
            pos: excl_pos + 1,
            ch: after.chars().next().unwrap_or('?'),
        })?;
        Some(kind)
    } else {
        None
    };

    // Trim trailing `!fixup` for field parsing
    let fields_part = if let Some(excl_pos) = rest.rfind('!') {
        rest[..excl_pos].trim_end()
    } else {
        rest
    };
    let fields = parse_bit_fields(fields_part, 0)?;
    Ok(ParsedEncoding::Fixed {
        width,
        fields,
        fixup,
    })
}

fn parse_segmented(input: &str) -> Result<ParsedEncoding, ParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut segments = Vec::new();
    let mut pos = 0;

    while pos < chars.len() {
        // Skip whitespace (segment separator)
        pos = skip_ws(&chars, pos);
        if pos >= chars.len() {
            break;
        }

        // ── Check for `!fixup` (fixup-only segment) ──
        if chars[pos] == '!' {
            pos += 1; // skip '!'
            let start = pos;
            while pos < chars.len() && chars[pos].is_alphanumeric() {
                pos += 1;
            }
            let kind_str: String = chars[start..pos].iter().collect();
            let fixup = FixupKind::parse(&kind_str).ok_or_else(|| ParseError::UnexpectedChar {
                pos: start,
                ch: chars.get(start).copied().unwrap_or('?'),
            })?;
            let fw = fixup.width();
            segments.push(ParsedSegment {
                condition: None,
                width: fw,
                fields: Vec::new(),
                fixup: Some(fixup),
                primitive: None,
            });
            continue;
        }

        // ── Check for `@name` (inline primitive call) ──
        if chars[pos] == '@' {
            pos += 1; // skip '@'
            let name_start = pos;
            while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                pos += 1;
            }
            let name: String = chars[name_start..pos].iter().collect();
            if name.is_empty() {
                return Err(ParseError::UnexpectedChar {
                    pos: name_start,
                    ch: '@',
                });
            }
            // Parse arguments
            let mut args: Vec<String> = Vec::new();
            loop {
                pos = skip_ws(&chars, pos);
                if pos >= chars.len() {
                    break;
                }
                if chars[pos] == '{'
                    || chars[pos] == '!'
                    || chars[pos] == '?'
                    || chars[pos] == '$'
                    || chars[pos] == '['
                    || chars[pos] == '@'
                {
                    break;
                }
                let arg_start = pos;
                while pos < chars.len()
                    && !chars[pos].is_whitespace()
                    && chars[pos] != '{'
                    && chars[pos] != '!'
                    && chars[pos] != '?'
                    && chars[pos] != '$'
                    && chars[pos] != '['
                    && chars[pos] != '@'
                {
                    pos += 1;
                }
                let arg: String = chars[arg_start..pos].iter().collect();
                if !arg.is_empty() {
                    args.push(arg);
                }
            }
            segments.push(ParsedSegment {
                condition: None,
                width: 0,
                fields: Vec::new(),
                fixup: None,
                primitive: Some((name, args)),
            });
            continue;
        }

        // ── Check for `?condition` (inline condition prefix) ──
        let inline_cond = if chars[pos] == '?' {
            pos += 1; // skip '?'
            let start = pos;
            // Scan until '{' (start of segment body) — need to handle possible whitespace
            while pos < chars.len() && chars[pos] != '{' {
                pos += 1;
            }
            let cond_str: String = chars[start..pos].iter().collect();
            let cond_str = cond_str.trim().to_string();
            if cond_str.is_empty() {
                return Err(ParseError::UnexpectedChar {
                    pos: start,
                    ch: if pos < chars.len() { chars[pos] } else { '\0' },
                });
            }
            Some(cond_str)
        } else {
            None
        };

        // Parse one segment: contiguous {group}{group}... (no whitespace between groups)
        let seg_start = pos;
        if pos >= chars.len() || chars[pos] != '{' {
            return Err(ParseError::UnexpectedChar {
                pos,
                ch: if pos < chars.len() { chars[pos] } else { '\0' },
            });
        }

        let mut fields = Vec::new();
        loop {
            if pos >= chars.len() || chars[pos] != '{' {
                break;
            }
            pos += 1; // skip '{'

            let value = parse_value(&chars, &mut pos)?;
            let (offset, width, shift) = parse_bit_spec(&chars, &mut pos)?;

            if pos >= chars.len() || chars[pos] != '}' {
                return Err(ParseError::UnclosedBrace { pos });
            }
            pos += 1; // skip '}'

            fields.push(ParsedBitField {
                value,
                offset,
                width,
                shift,
            });
        }

        if fields.is_empty() {
            return Err(ParseError::EmptySegment { pos: seg_start });
        }

        // Compute segment width from fields' max bit extent, rounded up
        let max_bit = fields
            .iter()
            .map(|f| (f.offset as u32).saturating_add(f.width as u32))
            .max()
            .unwrap_or(8);
        let seg_width = round_up_width(max_bit);

        // ── Check for trailing `!fixup` suffix on this segment ──
        let fixup = if pos < chars.len() && chars[pos] == '!' {
            pos += 1; // skip '!'
            let start = pos;
            while pos < chars.len() && chars[pos].is_alphanumeric() {
                pos += 1;
            }
            let kind_str: String = chars[start..pos].iter().collect();
            Some(
                FixupKind::parse(&kind_str).ok_or_else(|| ParseError::UnexpectedChar {
                    pos: start,
                    ch: chars.get(start).copied().unwrap_or('?'),
                })?,
            )
        } else {
            None
        };

        segments.push(ParsedSegment {
            condition: inline_cond,
            width: seg_width,
            fields,
            fixup,
            primitive: None,
        });
    }

    if segments.is_empty() {
        return Err(ParseError::EmptyEncoding);
    }
    Ok(ParsedEncoding::Segmented { segments })
}

fn parse_bit_fields(input: &str, _base_pos: usize) -> Result<Vec<ParsedBitField>, ParseError> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;
    parse_bit_fields_at(&chars, &mut pos)
}

fn parse_bit_fields_at(chars: &[char], pos: &mut usize) -> Result<Vec<ParsedBitField>, ParseError> {
    let mut fields = Vec::new();

    loop {
        *pos = skip_ws(chars, *pos);
        if *pos >= chars.len() || chars[*pos] != '{' {
            break;
        }
        *pos += 1; // skip '{'

        let value = parse_value(chars, pos)?;
        let (offset, width, shift) = parse_bit_spec(chars, pos)?;

        if *pos >= chars.len() || chars[*pos] != '}' {
            return Err(ParseError::UnclosedBrace { pos: *pos });
        }
        *pos += 1; // skip '}'

        fields.push(ParsedBitField {
            value,
            offset,
            width,
            shift,
        });
    }

    Ok(fields)
}

/// Parse value inside `{VALUE...` — field name, hex, or decimal.
fn parse_value(chars: &[char], pos: &mut usize) -> Result<BitFieldValue, ParseError> {
    let start = *pos;

    // Collect until ':' or '[' or '}'
    while *pos < chars.len() && chars[*pos] != ':' && chars[*pos] != '[' && chars[*pos] != '}' {
        *pos += 1;
    }

    let text: String = chars[start..*pos].iter().collect();
    let text = text.trim();

    if text.is_empty() {
        return Err(ParseError::MissingBitSpec { pos: start });
    }

    // Hex: 0xNN or 0XNN
    if text.starts_with("0x") || text.starts_with("0X") {
        let v = u64::from_str_radix(&text[2..], 16).map_err(|_| ParseError::InvalidHex {
            pos: start,
            text: text.to_string(),
        })?;
        return Ok(BitFieldValue::Hex(v));
    }

    // Decimal or negative decimal
    if text.starts_with(|c: char| c.is_ascii_digit()) || text.starts_with('-') {
        let v: i64 = text.parse().map_err(|_| ParseError::InvalidNumber {
            pos: start,
            text: text.to_string(),
        })?;
        return Ok(BitFieldValue::Dec(v));
    }

    // Field name
    Ok(BitFieldValue::Field(text.to_string()))
}

/// Parse bit spec after the value: `:OFFSET:WIDTH` / `:[OFFSET;WIDTH]` / `:[OFFSET;WIDTH;SHIFT]`
///
/// The first character may be `:` (then optionally `[` for bracket range) or directly `[`.
fn parse_bit_spec(chars: &[char], pos: &mut usize) -> Result<(u8, u8, Option<u8>), ParseError> {
    // Skip leading ':' if present (separator between value and bit spec)
    if *pos < chars.len() && chars[*pos] == ':' {
        *pos += 1;
    }

    // Check for bracket format: `[OFFSET;WIDTH]` or `[OFFSET;WIDTH;SHIFT]`
    let use_bracket = *pos < chars.len() && chars[*pos] == '[';
    if use_bracket {
        *pos += 1; // skip '['
    }

    let offset = parse_u8(chars, pos)?;

    // Separator: ':' or ';'
    if *pos >= chars.len() || (chars[*pos] != ':' && chars[*pos] != ';') {
        return Err(ParseError::InvalidBitSpec {
            pos: *pos,
            text: "expected ':' or ';'".into(),
        });
    }
    *pos += 1;

    let width = parse_u8(chars, pos)?;

    // Optional shift (only in bracket form)
    let shift = if use_bracket && *pos < chars.len() && chars[*pos] == ';' {
        *pos += 1;
        Some(parse_u8(chars, pos)?)
    } else {
        None
    };

    if use_bracket {
        if *pos >= chars.len() || chars[*pos] != ']' {
            return Err(ParseError::InvalidBitSpec {
                pos: *pos,
                text: "expected ']'".into(),
            });
        }
        *pos += 1;
    }

    Ok((offset, width, shift))
}

fn parse_u8(chars: &[char], pos: &mut usize) -> Result<u8, ParseError> {
    let start = *pos;
    while *pos < chars.len() && (chars[*pos].is_ascii_digit() || chars[*pos] == '-') {
        *pos += 1;
    }
    let text: String = chars[start..*pos].iter().collect();
    if text.is_empty() {
        return Err(ParseError::InvalidNumber {
            pos: start,
            text: "<empty>".into(),
        });
    }
    text.parse::<u8>()
        .map_err(|_| ParseError::InvalidNumber { pos: start, text })
}

fn skip_ws(chars: &[char], mut pos: usize) -> usize {
    while pos < chars.len() && chars[pos].is_whitespace() {
        pos += 1;
    }
    pos
}

fn round_up_width(max_bit: u32) -> u32 {
    match max_bit {
        0 => 8,
        1..=8 => 8,
        9..=16 => 16,
        17..=32 => 32,
        _ => 64,
    }
}

// ============================================================
// Macro/Scatter expansion (text pre-processing)
// ============================================================

/// Expand `$macro` calls and `{field:scatter}` references in an encoding string.
/// Runs iteratively until no more expansions or max depth reached.
pub fn expand_encoding(enc_str: &str, model: &crate::model::IsaModel) -> Result<String, String> {
    let mut result = enc_str.to_string();
    let max_iter = 16; // prevent infinite recursion
    for _ in 0..max_iter {
        let expanded = expand_macros(&result, model)?;
        let expanded = expand_scatters(&expanded, model)?;
        if expanded == result {
            return Ok(result);
        }
        result = expanded;
    }
    Err(format!(
        "macro/scatter expansion exceeded max depth ({max_iter}) in '{enc_str}'"
    ))
}

/// Expand `$name arg1 arg2 ...` macro calls in the encoding string.
fn expand_macros(input: &str, model: &crate::model::IsaModel) -> Result<String, String> {
    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;

    while pos < chars.len() {
        if chars[pos] == '$' {
            pos += 1; // skip '$'
            // Parse macro name
            let name_start = pos;
            while pos < chars.len() && (chars[pos].is_alphanumeric() || chars[pos] == '_') {
                pos += 1;
            }
            let name: String = chars[name_start..pos].iter().collect();
            if name.is_empty() {
                return Err(format!(
                    "empty macro name after '$' at position {name_start}"
                ));
            }

            // Parse arguments (space-separated tokens until end or structural delimiter)
            let mut args: Vec<String> = Vec::new();
            loop {
                pos = skip_ws(&chars, pos);
                if pos >= chars.len() {
                    break;
                }
                // Stop at structural delimiters
                if chars[pos] == '{'
                    || chars[pos] == '!'
                    || chars[pos] == '?'
                    || chars[pos] == '$'
                    || chars[pos] == '['
                {
                    break;
                }
                // Parse one arg token
                let arg_start = pos;
                while pos < chars.len()
                    && !chars[pos].is_whitespace()
                    && chars[pos] != '{'
                    && chars[pos] != '!'
                    && chars[pos] != '?'
                    && chars[pos] != '$'
                    && chars[pos] != '['
                {
                    pos += 1;
                }
                let arg: String = chars[arg_start..pos].iter().collect();
                if !arg.is_empty() {
                    args.push(arg);
                }
            }

            // Look up macro
            let mac = model.enc_macros.get(&name).ok_or_else(|| {
                format!(
                    "unknown macro '${name}'. Available: {:?}",
                    model.enc_macros.keys().collect::<Vec<_>>()
                )
            })?;

            if args.len() != mac.params.len() {
                return Err(format!(
                    "macro '${name}' expects {} arguments ({:?}), got {} ({:?})",
                    mac.params.len(),
                    mac.params,
                    args.len(),
                    args,
                ));
            }

            // Expand: replace ${param} with arg value
            let mut expanded = mac.pattern.clone();
            for (param, arg) in mac.params.iter().zip(args.iter()) {
                let placeholder = format!("${{{param}}}");
                expanded = expanded.replace(&placeholder, arg);
            }
            result.push_str(&expanded);
        } else {
            result.push(chars[pos]);
            pos += 1;
        }
    }

    Ok(result)
}

/// Expand `{field_name:scatter_name}` scatter references.
fn expand_scatters(input: &str, model: &crate::model::IsaModel) -> Result<String, String> {
    if model.enc_scatters.is_empty() {
        return Ok(input.to_string());
    }

    let mut result = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut pos = 0;

    while pos < chars.len() {
        if chars[pos] == '{' {
            // Find the closing '}'
            let brace_start = pos;
            pos += 1; // skip '{'
            let content_start = pos;
            while pos < chars.len() && chars[pos] != '}' {
                pos += 1;
            }
            if pos >= chars.len() {
                // Unclosed brace — pass through as-is (parser will error later)
                result.push_str(&chars[brace_start..].iter().collect::<String>());
                break;
            }
            let content: String = chars[content_start..pos].iter().collect();
            pos += 1; // skip '}'

            // Check for scatter reference: field_name:scatter_name
            // The bit spec (after the first ':' or '[') comes after the value part
            if let Some(colon_pos) = content.find(':') {
                let field_or_val = &content[..colon_pos];
                let rest = &content[colon_pos + 1..];

                // Check if 'rest' looks like a scatter name (alphabetic start, no digits/semicolons like a bit spec)
                let scatter_name = if let Some(_next_colon) = rest.find(':') {
                    // There's another ':' → this is offset:width, not a scatter
                    None
                } else if rest.starts_with('[') {
                    // Bracket bit spec → not a scatter
                    None
                } else if rest.contains(';') {
                    // Semicolons → bit spec, not a scatter
                    None
                } else if rest.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    // Pure alpha/underscore → scatter name
                    Some(rest.to_string())
                } else {
                    None
                };

                if let Some(sn) = scatter_name
                    && let Some(scat) = model.enc_scatters.get(&sn)
                {
                    // Expand scatter: replace '_' with field name
                    let expanded = scat.pattern.replace('_', field_or_val);
                    result.push_str(&expanded);
                    continue;
                }
            }

            // Not a scatter reference — pass through
            result.push('{');
            result.push_str(&content);
            result.push('}');
        } else {
            result.push(chars[pos]);
            pos += 1;
        }
    }

    Ok(result)
}

// ============================================================
// Validation
// ============================================================

/// Check for overlapping bit fields within a segment. Returns an error if any two
/// fields overlap (i.e., their bit ranges intersect).
fn check_overlap(fields: &[ParsedBitField], context: &str) -> Result<(), String> {
    let mut bits: u128 = 0;
    for f in fields {
        let mask = if f.width >= 128 {
            u128::MAX
        } else {
            (1u128 << f.width) - 1
        };
        let field_bits = mask << f.offset;
        if bits & field_bits != 0 {
            return Err(format!(
                "bit overlap in {context}: field '{}' at offset {} width {} overlaps with another field",
                f.value, f.offset, f.width,
            ));
        }
        bits |= field_bits;
    }
    Ok(())
}

// ============================================================
// Code generation (used by codegen.rs)
// ============================================================

/// Generate Rust `TokenStream` from a parsed encoding, for use by `codegen.rs`.
/// Requires the `field_types` map to resolve VReg vs non-VReg field references.
pub fn gen_bitstring_emit(
    enc_str: &str,
    inst: &crate::model::Instruction,
    model: &crate::model::IsaModel,
) -> Result<proc_macro2::TokenStream, String> {
    use proc_macro2::TokenStream;
    use quote::quote;

    // Expand $macro calls and {field:scatter} references before parsing
    let expanded = expand_encoding(enc_str, model)?;

    let parsed = parse_encoding(&expanded)
        .map_err(|e| format!("encoding parse error in '{enc_str}': {e}"))?;

    let field_map: std::collections::HashMap<String, &crate::model::FieldType> = inst
        .fields
        .iter()
        .map(|f| (f.name.clone(), &f.field_type))
        .collect();

    match parsed {
        ParsedEncoding::Fixed {
            width,
            fields,
            fixup,
        } => {
            check_overlap(&fields, &format!("fixed-width encoding '{enc_str}'"))?;
            let bitfields: Vec<TokenStream> = fields
                .iter()
                .map(|bf| bitfield_to_tokens(bf, &field_map))
                .collect::<Result<_, _>>()?;
            let pack_call = quote! { pack_bits(sink, #width as u8, &[#(#bitfields),*]); };
            if let Some(fk) = fixup {
                let block_target_field: Option<&str> = inst
                    .fields
                    .iter()
                    .find(|f| matches!(f.field_type, crate::model::FieldType::BlockTarget))
                    .map(|f| f.name.as_str());
                let bt = block_target_field.ok_or_else(|| {
                    format!("fixup '!{fk}' requires a BlockTarget field, but instruction has none")
                })?;
                let bt_ident = syn::Ident::new(bt, proc_macro2::Span::call_site());
                let reloc_kind = fixup_to_reloc(fk);
                Ok(quote! {
                    let __fixup = sink.offset();
                    #pack_call
                    sink.use_label_at(__fixup, crate::ir::BlockId(*#bt_ident as u32), #reloc_kind);
                })
            } else {
                Ok(pack_call)
            }
        }
        ParsedEncoding::Segmented { segments } => {
            let mut stmts: Vec<TokenStream> = Vec::new();
            // Collect all VReg fields used in conditions across segments
            let mut preg_vars: Vec<TokenStream> = Vec::new();
            let mut seen_vars: std::collections::HashSet<String> = std::collections::HashSet::new();

            // Find the single BlockTarget field for fixup, if any
            let block_target_field: Option<&str> = inst
                .fields
                .iter()
                .find(|f| matches!(f.field_type, crate::model::FieldType::BlockTarget))
                .map(|f| f.name.as_str());

            for seg in &segments {
                // ── Handle primitive segment ──
                if let Some((ref prim_name, ref prim_args)) = seg.primitive {
                    let prim_ts = gen_primitive(prim_name, prim_args, &field_map)?;
                    stmts.push(prim_ts);
                    continue;
                }

                // ── Handle fixup segment ──
                if let Some(fixup) = seg.fixup {
                    let bt = block_target_field.ok_or_else(|| {
                        format!(
                            "fixup '!{fixup}' requires a BlockTarget field, but instruction has none"
                        )
                    })?;
                    let bt_ident = syn::Ident::new(bt, proc_macro2::Span::call_site());
                    let reloc_kind = fixup_to_reloc(fixup);
                    let zero_width = seg.width;
                    stmts.push(quote! {
                        let __fixup = sink.offset();
                        pack_bits(sink, #zero_width as u8, &[]);
                        sink.use_label_at(__fixup, crate::ir::BlockId(*#bt_ident as u32), #reloc_kind);
                    });
                    continue;
                }

                check_overlap(&seg.fields, &format!("segment in '{enc_str}'"))?;
                let bitfields: Vec<TokenStream> = seg
                    .fields
                    .iter()
                    .map(|bf| bitfield_to_tokens(bf, &field_map))
                    .collect::<Result<_, _>>()?;
                let seg_width = seg.width;

                // ── Determine the condition for this segment ──
                let cond_str: Option<&str> = seg.condition.as_deref();

                match cond_str {
                    Some(cond) if !cond.is_empty() => {
                        let cond_ts = resolve_condition(
                            cond,
                            inst,
                            &field_map,
                            &mut preg_vars,
                            &mut seen_vars,
                        )?;
                        stmts.push(quote! {
                            if #cond_ts {
                                pack_bits(sink, #seg_width as u8, &[#(#bitfields),*]);
                            }
                        });
                    }
                    _ => {
                        stmts.push(
                            quote! { pack_bits(sink, #seg_width as u8, &[#(#bitfields),*]); },
                        );
                    }
                }
            }
            // Prepend preg variable declarations
            let all_stmts = if preg_vars.is_empty() {
                quote! { #(#stmts)* }
            } else {
                quote! { #(#preg_vars)* #(#stmts)* }
            };
            Ok(all_stmts)
        }
        ParsedEncoding::Primitive { name, args } => gen_primitive(&name, &args, &field_map),
    }
}

/// Convert a FixupKind to the corresponding RelocKind TokenStream.
fn fixup_to_reloc(fixup: FixupKind) -> proc_macro2::TokenStream {
    use quote::quote;
    match fixup {
        FixupKind::Rel4 => quote! { crate::RelocKind::REL4 },
        FixupKind::Isa0 => quote! { crate::RelocKind::Isa(0) },
        FixupKind::Isa1 => quote! { crate::RelocKind::Isa(1) },
        FixupKind::Isa2 => quote! { crate::RelocKind::Isa(2) },
    }
}

/// Resolve a condition string like "dest >= 8 || src >= 8" to a Rust TokenStream.
/// Pre-computed `let __field = preg(*field, rm)?;` vars are accumulated in preg_vars.
fn resolve_condition(
    condition: &str,
    inst: &crate::model::Instruction,
    _field_map: &std::collections::HashMap<String, &crate::model::FieldType>,
    preg_vars: &mut Vec<proc_macro2::TokenStream>,
    seen: &mut std::collections::HashSet<String>,
) -> Result<proc_macro2::TokenStream, String> {
    use proc_macro2::TokenStream;
    use quote::quote;

    let mut cond = condition.to_string();
    let mut sorted: Vec<_> = inst.fields.iter().collect();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.name.len()));
    for field in &sorted {
        if !cond.contains(&field.name) {
            continue;
        }
        let is_vreg = matches!(field.field_type, crate::model::FieldType::VReg);
        let replacement = if is_vreg {
            let var_name = format!("__{}", field.name);
            if seen.insert(var_name.clone()) {
                let vi = syn::Ident::new(&var_name, proc_macro2::Span::call_site());
                let fi = syn::Ident::new(&field.name, proc_macro2::Span::call_site());
                preg_vars.push(quote! { let #vi = preg(*#fi, rm)?; });
            }
            format!("__{}", field.name)
        } else {
            format!("(*{0} as i64)", field.name)
        };
        cond = cond.replace(&field.name, &replacement);
    }
    // Wrap condition in parentheses to avoid Rust generics ambiguity
    let cond_with_parens = format!("({cond})");
    cond_with_parens.parse::<TokenStream>()
        .map_err(|e| format!("invalid segment condition '{condition}': {e}"))
}

/// Generate code for encoding primitives like @jmp_rel32, @mov_imm64, etc.
fn gen_primitive(
    name: &str,
    args: &[String],
    _field_map: &std::collections::HashMap<String, &crate::model::FieldType>,
) -> Result<proc_macro2::TokenStream, String> {
    use proc_macro2::TokenStream;
    use quote::quote;

    // Get an argument as an expression TokenStream: if numeric, use literal; otherwise, use identifier.
    let arg_expr = |idx: usize| -> TokenStream {
        let val = args.get(idx).map(|s| s.as_str()).unwrap_or("_");
        // Check if numeric (decimal or hex)
        if (val.starts_with("0x") || val.starts_with("0X"))
            && let Ok(n) = u64::from_str_radix(&val[2..], 16)
        {
            return quote! { #n };
        }
        if let Ok(n) = val.parse::<i64>() {
            return quote! { #n };
        }
        // Otherwise treat as identifier
        let ident = syn::Ident::new(val, proc_macro2::Span::call_site());
        quote! { #ident }
    };

    let arg_ident = |idx: usize| -> syn::Ident {
        let name = args.get(idx).map(|s| s.as_str()).unwrap_or("_");
        syn::Ident::new(name, proc_macro2::Span::call_site())
    };

    match name {
        "leb128" => {
            let a = arg_ident(0);
            Ok(quote! {
                let mut __v = *#a as u64;
                loop {
                    let mut __byte = (__v & 0x7F) as u8;
                    __v >>= 7;
                    if __v != 0 { __byte |= 0x80; }
                    sink.put1(__byte);
                    if __v == 0 { break; }
                }
            })
        }
        "leb128_reg" => {
            let prefix = arg_expr(0);
            let reg = arg_ident(1);
            Ok(quote! {
                sink.put1(#prefix as u8);
                let mut __v = preg(*#reg, rm)? as u64;
                loop {
                    let mut __byte = (__v & 0x7F) as u8;
                    __v >>= 7;
                    if __v != 0 { __byte |= 0x80; }
                    sink.put1(__byte);
                    if __v == 0 { break; }
                }
            })
        }
        "sleb128" => {
            let a = arg_ident(0);
            Ok(quote! {
                let mut __v = *#a as i64;
                loop {
                    let mut __byte = (__v as u8) & 0x7F;
                    __v >>= 7;
                    if (__v == 0 && (__byte & 0x40) == 0) || (__v == -1 && (__byte & 0x40) != 0) {
                        sink.put1(__byte);
                        break;
                    }
                    __byte |= 0x80;
                    sink.put1(__byte);
                }
            })
        }
        "lea_sib" => {
            let dest = arg_ident(0);
            let base = arg_ident(1);
            let index = arg_ident(2);
            let scale = arg_ident(3);
            let disp = arg_ident(4);
            Ok(quote! {
                enc_lea_sib(sink, preg(*#dest, rm)?, preg(*#base, rm)?, preg(*#index, rm)?, *#scale, *#disp as i32);
            })
        }
        "shift_cl" => {
            let dest = arg_ident(0);
            let src = arg_ident(1);
            let op_ext = arg_expr(2);
            Ok(quote! {
                let cnt = preg(*#src, rm)?;
                if cnt != 1 {
                    if cnt >= 8 { sink.put1(0x49u8); } else { sink.put1(0x48u8); }
                    sink.put1(0x8Bu8);
                    sink.put1(modrm(3, 1, cnt));
                }
                let __d = preg(*#dest, rm)?;
                // REX.W=1 required for 64-bit shift; REX.B set if dest ≥ 8
                if __d >= 8 { sink.put1(0x49u8); } else { sink.put1(0x48u8); }
                sink.put1(0xD3u8);
                sink.put1(modrm(3, #op_ext as u8, __d & 7));
            })
        }
        _ => Err(format!(
            "unknown encoding primitive '@{name}'. Available: @leb128, @sleb128, @leb128_reg, @lea_sib, @shift_cl"
        )),
    }
}

fn bitfield_to_tokens(
    bf: &ParsedBitField,
    field_map: &std::collections::HashMap<String, &crate::model::FieldType>,
) -> Result<proc_macro2::TokenStream, String> {
    use crate::model::FieldType;
    use proc_macro2::TokenStream;
    use quote::quote;

    let offset = bf.offset;
    let width = bf.width;

    let value_expr: TokenStream = match &bf.value {
        BitFieldValue::Field(name) => match field_map.get(name.as_str()) {
            Some(FieldType::VReg) => {
                let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
                quote! { preg(*#ident, rm)? as u64 }
            }
            Some(_) => {
                let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
                quote! { *#ident as u64 }
            }
            None => return Err(format!("bitstring references undeclared field '{name}'")),
        },
        BitFieldValue::Hex(v) => {
            let v = *v;
            quote! { #v as u64 }
        }
        BitFieldValue::Dec(v) => {
            let v = *v;
            quote! { #v as u64 }
        }
    };

    if let Some(shift) = bf.shift {
        Ok(quote! { BitField::new((#value_expr) >> #shift, #offset, #width) })
    } else {
        Ok(quote! { BitField::new(#value_expr, #offset, #width) })
    }
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_width_rv() {
        let enc = parse_encoding("32 {dest:7:5}{0:12:3}{src1:15:5}{src2:20:5}{0:25:7}{0x33:0:7}")
            .unwrap();
        let ParsedEncoding::Fixed { width, fields, .. } = enc else {
            panic!("expected Fixed");
        };
        assert_eq!(width, 32);
        assert_eq!(fields.len(), 6);
        assert_eq!(fields[0].value, BitFieldValue::Field("dest".into()));
        assert_eq!(fields[0].offset, 7);
        assert_eq!(fields[0].width, 5);
        assert_eq!(fields[5].value, BitFieldValue::Hex(0x33));
        assert_eq!(fields[5].offset, 0);
        assert_eq!(fields[5].width, 7);
    }

    #[test]
    fn test_segmented_x86_add() {
        // Inline condition with REX segment
        let enc = parse_encoding(
            "?dest>=8||src>=8 {0x4:[0;4]}{1:4:1}{dest:[5;1;3]}{src:[7;1;3]} {0x01:[0;8]} {dest:[0;3]}{src:[3;3]}{3:[6;2]}",
        )
        .unwrap();
        let ParsedEncoding::Segmented { segments } = enc else {
            panic!("expected Segmented");
        };
        assert_eq!(segments.len(), 3);

        // REX segment with inline condition
        assert_eq!(segments[0].condition.as_deref(), Some("dest>=8||src>=8"));
        assert_eq!(segments[0].width, 8);
        assert_eq!(segments[0].fields.len(), 4);
        assert_eq!(
            segments[0].fields[2].value,
            BitFieldValue::Field("dest".into())
        );
        assert_eq!(segments[0].fields[2].offset, 5);
        assert_eq!(segments[0].fields[2].width, 1);
        assert_eq!(segments[0].fields[2].shift, Some(3));

        // Opcode segment
        assert!(segments[1].condition.is_none());
        assert_eq!(segments[1].width, 8);
        assert_eq!(segments[1].fields.len(), 1);
        assert_eq!(segments[1].fields[0].value, BitFieldValue::Hex(0x01));

        // ModRM segment
        assert!(segments[2].condition.is_none());
        assert_eq!(segments[2].width, 8);
        assert_eq!(segments[2].fields.len(), 3);
    }

    #[test]
    fn test_bare_segments() {
        // Instruction with no conditional prefix — just bytes
        let enc = parse_encoding("{0x39:[0;8]} {dest:[0;3]}{src:[3;3]}{3:[6;2]}").unwrap();
        let ParsedEncoding::Segmented { segments } = enc else {
            panic!("expected Segmented");
        };
        assert_eq!(segments.len(), 2);
        assert!(segments[0].condition.is_none());
        assert!(segments[1].condition.is_none());
        assert_eq!(segments[1].fields.len(), 3);
    }

    #[test]
    fn test_primitive() {
        let enc = parse_encoding("@lea_sib").unwrap();
        let ParsedEncoding::Primitive { name, args } = enc else {
            panic!("expected Primitive");
        };
        assert_eq!(name, "lea_sib");
        assert!(args.is_empty());
    }

    #[test]
    fn test_primitive_with_args() {
        let enc = parse_encoding("@leb128 imm").unwrap();
        let ParsedEncoding::Primitive { name, args } = enc else {
            panic!("expected Primitive");
        };
        assert_eq!(name, "leb128");
        assert_eq!(args, vec!["imm"]);
    }

    #[test]
    fn test_colon_format() {
        let enc = parse_encoding("32 {dest:7:5}{0x33:0:7}").unwrap();
        let ParsedEncoding::Fixed { width, fields, .. } = enc else {
            panic!("expected Fixed");
        };
        assert_eq!(width, 32);
        assert_eq!(fields[0].offset, 7);
        assert_eq!(fields[0].width, 5);
        assert_eq!(fields[0].shift, None);
    }

    #[test]
    fn test_hex_and_decimal() {
        let enc = parse_encoding("8 {0xFF:[0;8]}").unwrap();
        let ParsedEncoding::Fixed { fields, .. } = enc else {
            panic!("expected Fixed");
        };
        assert_eq!(fields[0].value, BitFieldValue::Hex(0xFF));

        let enc = parse_encoding("8 {42:[0;8]}").unwrap();
        let ParsedEncoding::Fixed { fields, .. } = enc else {
            panic!("expected Fixed");
        };
        assert_eq!(fields[0].value, BitFieldValue::Dec(42));
    }

    #[test]
    fn test_aarch64_add() {
        let enc =
            parse_encoding("32 {dest:0:5}{src1:5:5}{src2:16:5}{0x22C:21:10}{1:31:1}").unwrap();
        let ParsedEncoding::Fixed { width, fields, .. } = enc else {
            panic!("expected Fixed");
        };
        assert_eq!(width, 32);
        assert_eq!(fields.len(), 5);
        assert_eq!(fields[3].value, BitFieldValue::Hex(0x22C));
    }

    #[test]
    fn test_empty_encoding() {
        assert!(parse_encoding("").is_err());
    }

    #[test]
    fn test_unclosed_brace() {
        assert!(parse_encoding("32 {dest:7:5").is_err());
    }

    // ── New v11 features ──

    #[test]
    fn test_inline_condition() {
        let enc = parse_encoding(
            "?dest>=8||src>=8 {0x4:[0;4]}{1:[3;1]}{dest:[2;1;3]}{0:[1;1]}{src:[0;1;3]} {0x01:[0;8]} {dest:[0;3]}{src:[3;3]}{3:[6;2]}",
        )
        .unwrap();
        let ParsedEncoding::Segmented { segments } = enc else {
            panic!("expected Segmented");
        };
        assert_eq!(segments.len(), 3);
        // First segment has inline condition
        assert_eq!(segments[0].condition.as_deref(), Some("dest>=8||src>=8"));
        assert_eq!(segments[0].width, 8);
        assert_eq!(segments[0].fields.len(), 5); // 5 bitfields in REX byte
        // Remaining segments are unconditional
        assert!(segments[1].condition.is_none());
        assert!(segments[2].condition.is_none());
    }

    #[test]
    fn test_fixup_x86_jmp() {
        let enc = parse_encoding("{0xE9:[0;8]} !rel4").unwrap();
        let ParsedEncoding::Segmented { segments } = enc else {
            panic!("expected Segmented");
        };
        assert_eq!(segments.len(), 2);
        // First segment: opcode byte
        assert_eq!(segments[0].fields.len(), 1);
        assert!(segments[0].fixup.is_none());
        // Second segment: fixup placeholder
        assert_eq!(segments[1].fixup, Some(FixupKind::Rel4));
        assert_eq!(segments[1].width, 32);
        assert!(segments[1].fields.is_empty());
    }

    #[test]
    fn test_fixup_isa1() {
        let enc = parse_encoding("32 {src2:20:5}{src1:15:5}{0:12:3}{0x63:0:7} !isa1").unwrap();
        let ParsedEncoding::Fixed {
            width,
            fields,
            fixup,
        } = enc
        else {
            panic!("expected Fixed");
        };
        assert_eq!(width, 32);
        assert_eq!(fields.len(), 4);
        assert_eq!(fixup, Some(FixupKind::Isa1));
    }

    #[test]
    fn test_empty_inline_condition_is_error() {
        // '?' followed by nothing before '{' is an error
        assert!(parse_encoding("? {0x90:[0;8]}").is_err());
    }

    #[test]
    fn test_unknown_fixup_is_error() {
        assert!(parse_encoding("{0xE9:[0;8]} !bad").is_err());
    }
}
