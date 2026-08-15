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

use std::collections::BTreeMap;
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
    // Split on whitespace and commas so `@call_reloc target, 0xE8` and
    // `@call_reloc target 0xE8` both parse (other primitives with comma
    // separated args, e.g. @lea_sib, benefit too).
    let parts: Vec<&str> = rest
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|s| !s.is_empty())
        .collect();
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

    // Binary: 0bNNNN or 0BNNNN — parse before hex/decimal to avoid ambiguity
    if text.starts_with("0b") || text.starts_with("0B") {
        let v = u64::from_str_radix(&text[2..], 2).map_err(|_| ParseError::InvalidNumber {
            pos: start,
            text: text.to_string(),
        })?;
        return Ok(BitFieldValue::Hex(v));
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

/// Strip `# ...` comments from encoding pattern strings.
/// Comments are only stripped when `#` is preceded by whitespace or at line start.
/// This preserves `#` in contexts like `0b000101` (bit patterns).
fn strip_comments(input: &str) -> String {
    input
        .lines()
        .map(|line| {
            if let Some(pos) = line.find('#') {
                let prefix = &line[..pos];
                if pos == 0 || prefix.as_bytes()[pos - 1].is_ascii_whitespace() {
                    prefix.trim_end()
                } else {
                    line
                }
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Expand `$NAME` constant references using `[enc_constants]` from the TOML model.
fn expand_constants(input: &str, constants: &BTreeMap<String, String>) -> String {
    let mut result = input.to_string();
    for (name, value) in constants {
        // Replace $NAME references (standalone, not ${param} format)
        let placeholder = format!("${name}");
        result = result.replace(&placeholder, value);
    }
    result
}

/// Expand `$name arg1 arg2 ...` macro calls in the encoding string.
fn expand_macros(input: &str, model: &crate::model::IsaModel) -> Result<String, String> {
    // Pre-process: strip comments and expand constants
    let input = strip_comments(input);
    let input = expand_constants(&input, &model.enc_constants);

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
                    sink.use_label_at(__fixup, Block(*#bt_ident as u32), #reloc_kind);
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

            // E1: Merge consecutive segments with the same condition into one if-block.
            // Buffer accumulates (condition, condition_ts, Vec<pack_bits_stmts>).
            let mut current_cond: Option<String> = None;
            let mut current_cond_ts: Option<TokenStream> = None;
            let mut buffered: Vec<TokenStream> = Vec::new();

            let flush = |_cond_str: Option<String>,
                         cond_ts: Option<TokenStream>,
                         buf: &mut Vec<TokenStream>,
                         stmts: &mut Vec<TokenStream>| {
                if buf.is_empty() {
                    return;
                }
                let drained: Vec<TokenStream> = std::mem::take(buf);
                match cond_ts {
                    Some(ref cts) => {
                        stmts.push(quote! {
                            if #cts {
                                #(#drained)*
                            }
                        });
                    }
                    None => {
                        stmts.extend(drained);
                    }
                }
            };

            for seg in &segments {
                // ── Handle primitive segment ──
                if let Some((ref prim_name, ref prim_args)) = seg.primitive {
                    // Flush current buffer before emitting primitive
                    flush(
                        current_cond.take(),
                        current_cond_ts.take(),
                        &mut buffered,
                        &mut stmts,
                    );
                    let prim_ts = gen_primitive(prim_name, prim_args, &field_map, model)?;
                    stmts.push(prim_ts);
                    continue;
                }

                // ── Handle fixup segment ──
                if let Some(fixup) = seg.fixup {
                    flush(
                        current_cond.take(),
                        current_cond_ts.take(),
                        &mut buffered,
                        &mut stmts,
                    );
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
                        sink.use_label_at(__fixup, Block(*#bt_ident as u32), #reloc_kind);
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
                let pack_stmt = quote! { pack_bits(sink, #seg_width as u8, &[#(#bitfields),*]); };

                let seg_cond: Option<String> = seg
                    .condition
                    .as_deref()
                    .filter(|c| !c.is_empty())
                    .map(String::from);

                // Determine if this segment shares the same condition as the buffered one.
                let same_cond = match (&current_cond, &seg_cond) {
                    (Some(a), Some(b)) => a == b,
                    (None, None) => true,
                    _ => false,
                };

                if same_cond {
                    // Same condition — merge into current buffer
                    buffered.push(pack_stmt);
                } else {
                    // Condition changed: flush old buffer
                    flush(
                        current_cond.take(),
                        current_cond_ts.take(),
                        &mut buffered,
                        &mut stmts,
                    );
                    // Start new buffer with this segment's condition
                    let cond_ts = match seg_cond {
                        Some(ref cond) => Some(resolve_condition(
                            cond,
                            inst,
                            &field_map,
                            &mut preg_vars,
                            &mut seen_vars,
                        )?),
                        None => None,
                    };
                    current_cond = seg_cond;
                    current_cond_ts = cond_ts;
                    buffered.push(pack_stmt);
                }
            }
            // Flush any remaining buffered segments
            flush(
                current_cond.take(),
                current_cond_ts.take(),
                &mut buffered,
                &mut stmts,
            );
            // Prepend preg variable declarations
            let all_stmts = if preg_vars.is_empty() {
                quote! { #(#stmts)* }
            } else {
                quote! { #(#preg_vars)* #(#stmts)* }
            };
            Ok(all_stmts)
        }
        ParsedEncoding::Primitive { name, args } => gen_primitive(&name, &args, &field_map, model),
    }
}

/// Convert a FixupKind to the corresponding RelocKind TokenStream.
fn fixup_to_reloc(fixup: FixupKind) -> proc_macro2::TokenStream {
    use quote::quote;
    match fixup {
        // All in-function branch fixups are PC-relative; the ISA-specific
        // encoding (AArch64 BL imm26, RISC-V B/J immediate reordering) is
        // applied by the backend's RelocPatcher at finish/patch time.
        FixupKind::Rel4 | FixupKind::Isa0 | FixupKind::Isa1 | FixupKind::Isa2 => {
            quote! { crate::RelocKind::REL4 }
        }
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
        let is_vreg = matches!(
            field.field_type,
            crate::model::FieldType::Ireg | crate::model::FieldType::Freg
        );
        let replacement = if is_vreg {
            // 字段为物理 Reg（用户定义寄存器类型）：直接取物理索引
            let var_name = format!("__{}", field.name);
            if seen.insert(var_name.clone()) {
                let vi = syn::Ident::new(&var_name, proc_macro2::Span::call_site());
                let fi = syn::Ident::new(&field.name, proc_macro2::Span::call_site());
                preg_vars.push(quote! { let #vi = #fi.to_index(); });
            }
            format!("__{}", field.name)
        } else {
            format!("(*{0} as i64)", field.name)
        };
        cond = cond.replace(&field.name, &replacement);
    }
    // Wrap condition in parentheses to avoid Rust generics ambiguity
    let cond_with_parens = format!("({cond})");
    cond_with_parens
        .parse::<TokenStream>()
        .map_err(|e| format!("invalid segment condition '{condition}': {e}"))
}

/// Parse a non-negative integer literal (decimal or 0x hex).
fn parse_num_u64(s: &str) -> Option<u64> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    s.parse::<u64>().ok()
}

/// Generate code for encoding primitives like @jmp_rel32, @mov_imm64, etc.
fn gen_primitive(
    name: &str,
    args: &[String],
    field_map: &std::collections::HashMap<String, &crate::model::FieldType>,
    model: &crate::model::IsaModel,
) -> Result<proc_macro2::TokenStream, String> {
    use proc_macro2::TokenStream;
    use quote::quote;

    // Get an argument as an expression TokenStream: if numeric, use literal; otherwise, use identifier.
    let arg_expr = |idx: usize| -> TokenStream {
        let val = args.get(idx).map(|s| s.as_str()).unwrap_or("_");
        // Check if numeric (decimal or hex)
        if let Some(n) = parse_num_u64(val) {
            return quote! { #n };
        }
        if let Ok(n) = val.parse::<i64>() {
            return quote! { #n };
        }
        // Otherwise treat as a field binding — emit functions match on `&Inst`,
        // so non-register fields are references (`&u8`, `&i64`, …) and must be
        // dereferenced.
        let ident = syn::Ident::new(val, proc_macro2::Span::call_site());
        quote! { *#ident }
    };

    let arg_ident = |idx: usize| -> syn::Ident {
        let name = args.get(idx).map(|s| s.as_str()).unwrap_or("_");
        syn::Ident::new(name, proc_macro2::Span::call_site())
    };

    // Register operand: a field name (resolved via `.to_index()`) or a numeric literal.
    let reg_expr = |idx: usize| -> TokenStream {
        let val = args.get(idx).map(|s| s.as_str()).unwrap_or("0");
        if let Some(n) = parse_num_u64(val) {
            return quote! { #n as u32 };
        }
        let ident = syn::Ident::new(val, proc_macro2::Span::call_site());
        quote! { #ident.to_index() as u32 }
    };

    // Numeric literal arg (prefix/escape bytes): must be a non-negative number.
    let num_arg = |idx: usize, prim: &str| -> Result<u64, String> {
        match args.get(idx) {
            None => Ok(0),
            Some(s) => parse_num_u64(s)
                .ok_or_else(|| format!("@{prim} arg {idx} ('{s}') must be a numeric literal")),
        }
    };

    match name {
        // ──────────────────────────────────────────────────────
        // 通用 ModRM/REX 编码原语（参数化，无 ISA 专有常量）。
        // 供使用 ModRM 编码体系的 ISA（如 x86）直接引用；opsize
        // 可为数字字面量或指令的 Opsize 字段名。
        // ──────────────────────────────────────────────────────

        // @modrm opsize opcode reg rm [escape] — 宽度感知 ModRM(mod=11b)。
        //   16-bit → 0x66 前缀；64-bit → REX.W=1；32-bit + 扩展寄存器 → REX.W=0。
        //   可选 escape 字节（如 0x0F）插在 opcode 之前。
        "modrm" => {
            let opsize = arg_expr(0);
            let opcode = arg_expr(1);
            let reg = reg_expr(2);
            let rm = reg_expr(3);
            let escape = num_arg(4, "modrm")?;
            let esc_tok = if escape != 0 {
                quote! { sink.put1(#escape as u8); }
            } else {
                quote! {}
            };
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __reg = #reg;
                let __rm = #rm;
                if __opsize == 16 { sink.put1(0x66u8); }
                let __rex: u8 = if __opsize == 64 { 0x48 } else { 0x40 }
                    | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                    | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__reg & 0x8) != 0 || (__rm & 0x8) != 0 { sink.put1(__rex); }
                #esc_tok
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @modrm_mem opsize opcode reg base disp [prefix] [escape] — ModRM
        // 内存寻址。base 编码为 100 (RSP/R12) 时自动插入 SIB；base 属于
        // [meta].modrm_force_disp_base（如 x86 RBP=5/R13=13）时强制带位移
        // （mod=00+rm=101 在 x86 上是 RIP-relative）。disp 可为 0/字面量/字段。
        "modrm_mem" => {
            let opsize = arg_expr(0);
            let opcode = arg_expr(1);
            let reg = reg_expr(2);
            // rm 参数可以是寄存器字段（现有）或 MemRef 字段（自动展开 base/offset）。
            let is_memref = args
                .get(3)
                .and_then(|a| field_map.get(a.as_str()))
                .is_some_and(|t| **t == crate::model::FieldType::MemRef);
            let (rm_tok, disp_tok) = if is_memref {
                let m = arg_ident(3);
                (quote! { #m.base as u32 }, quote! { #m.offset as i32 })
            } else {
                let d = arg_expr(4);
                (reg_expr(3), quote! { #d as i32 })
            };
            // MemRef 路径无显式 disp 参数（位移来自 mem.offset），因此
            // [prefix] [escape] 从 index 4/5 读；寄存器 rm 路径 disp 占用
            // index 4，prefix/escape 从 index 5/6 读。
            let (prefix_idx, escape_idx) = if is_memref { (4, 5) } else { (5, 6) };
            let prefix = num_arg(prefix_idx, "modrm_mem")?;
            let escape = num_arg(escape_idx, "modrm_mem")?;
            let pre_tok = if prefix != 0 {
                quote! { sink.put1(#prefix as u8); }
            } else {
                quote! {}
            };
            let esc_tok = if escape != 0 {
                quote! { sink.put1(#escape as u8); }
            } else {
                quote! {}
            };
            let force = &model.meta.modrm_force_disp_base;
            let force_toks: Vec<TokenStream> = force.iter().map(|&b| quote! { #b }).collect();
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __reg = #reg;
                let __rm = #rm_tok;
                let __disp = #disp_tok;
                #pre_tok
                let __rex: u8 = if __opsize == 64 { 0x48 } else { 0x40 }
                    | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                    | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__reg & 0x8) != 0 || (__rm & 0x8) != 0 { sink.put1(__rex); }
                #esc_tok
                sink.put1(#opcode as u8);
                const FORCE_DISP: &[u8] = &[#(#force_toks),*];
                let __sib = (__rm & 7) == 4;
                let __force = FORCE_DISP.contains(&(__rm as u8));
                let __mod: u8 = if __disp == 0 && !__force {
                    0
                } else if (-128i32..=127i32).contains(&__disp) {
                    1
                } else {
                    2
                };
                if __sib {
                    // SIB required: rm=100; index=4 (无 index), base=rm
                    sink.put1((__mod << 6) | ((__reg as u8 & 0x7) << 3) | 0x04);
                    sink.put1(0x20u8 | (__rm as u8 & 0x7));
                } else {
                    sink.put1((__mod << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
                }
                if __mod == 1 {
                    sink.put1(__disp as u8);
                } else if __mod == 2 {
                    sink.put4(__disp as u32);
                }
            })
        }

        // @op_rm opsize opcode ext rm — /digit 单操作数（reg 固定为 ext）。
        "op_rm" => {
            let opsize = arg_expr(0);
            let opcode = arg_expr(1);
            let ext = arg_expr(2);
            let rm = reg_expr(3);
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __rm = #rm;
                if __opsize == 16 { sink.put1(0x66u8); }
                let __rex: u8 = if __opsize == 64 { 0x48 } else { 0x40 }
                    | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__rm & 0x8) != 0 { sink.put1(__rex); }
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((#ext as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @modrm_imm32 opsize opcode ext rm imm — 81 /digit + imm32。
        "modrm_imm32" => {
            let opsize = arg_expr(0);
            let opcode = arg_expr(1);
            let ext = arg_expr(2);
            let rm = reg_expr(3);
            let imm = arg_expr(4);
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __rm = #rm;
                if __opsize == 16 { sink.put1(0x66u8); }
                let __rex: u8 = if __opsize == 64 { 0x48 } else { 0x40 }
                    | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__rm & 0x8) != 0 { sink.put1(__rex); }
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((#ext as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
                let __imm = #imm as u32;
                sink.put1(__imm as u8);
                sink.put1((__imm >> 8) as u8);
                sink.put1((__imm >> 16) as u8);
                sink.put1((__imm >> 24) as u8);
            })
        }

        // @op_rm_imm32 opcode ext rm imm — 固定 64 位 /digit + imm32（帧分配）。
        "op_rm_imm32" => {
            let opcode = arg_expr(0);
            let ext = arg_expr(1);
            let rm = reg_expr(2);
            let imm = arg_expr(3);
            Ok(quote! {
                let __rm = #rm;
                let __rex: u8 = 0x48 | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                sink.put1(__rex);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((#ext as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
                let __imm = #imm as u32;
                sink.put1(__imm as u8);
                sink.put1((__imm >> 8) as u8);
                sink.put1((__imm >> 16) as u8);
                sink.put1((__imm >> 24) as u8);
            })
        }

        // @cmovcc opsize cc dest src — 0F 4{cc} /r 条件传送。
        "cmovcc" => {
            let opsize = arg_expr(0);
            let cc = arg_expr(1);
            let dest = reg_expr(2);
            let src = reg_expr(3);
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __dest = #dest;
                let __src = #src;
                if __opsize == 16 { sink.put1(0x66u8); }
                let __rex: u8 = if __opsize == 64 { 0x48 } else { 0x40 }
                    | if (__dest & 0x8) != 0 { 0x04 } else { 0 }
                    | if (__src & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__dest & 0x8) != 0 || (__src & 0x8) != 0 { sink.put1(__rex); }
                sink.put1(0x0Fu8);
                sink.put1(0x40u8 | (#cc as u8 & 0x0F));
                sink.put1((3u8 << 6) | ((__dest as u8 & 0x7) << 3) | (__src as u8 & 0x7));
            })
        }

        // @sse_rr prefix opcode w reg rm — SSE reg-reg（w=REX.W 位值，0/1）。
        "sse_rr" => {
            let prefix = arg_expr(0);
            let opcode = arg_expr(1);
            let w = arg_expr(2);
            let reg = reg_expr(3);
            let rm = reg_expr(4);
            Ok(quote! {
                let __p = #prefix as u8;
                if __p != 0 { sink.put1(__p); }
                let __reg = #reg;
                let __rm = #rm;
                if (__reg & 0x8) != 0 || (__rm & 0x8) != 0 {
                    let __rex: u8 = 0x40 | ((#w as u8 & 0x1) << 3)
                        | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                        | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                    sink.put1(__rex);
                }
                sink.put1(0x0Fu8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @sse_rr_3a prefix opcode reg rm imm — SSE 66 0F 3A /r ib。
        "sse_rr_3a" => {
            let prefix = arg_expr(0);
            let opcode = arg_expr(1);
            let reg = reg_expr(2);
            let rm = reg_expr(3);
            let imm = arg_expr(4);
            Ok(quote! {
                let __p = #prefix as u8;
                if __p != 0 { sink.put1(__p); }
                let __reg = #reg;
                let __rm = #rm;
                if (__reg & 0x8) != 0 || (__rm & 0x8) != 0 {
                    let __rex: u8 = 0x40
                        | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                        | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                    sink.put1(__rex);
                }
                sink.put1(0x0Fu8);
                sink.put1(0x3Au8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
                sink.put1(#imm as u8);
            })
        }

        // @sse_rr_38 prefix opcode reg rm — SSE 66 0F 38 xx /r（SSE4.1，如 PMULLD）。
        "sse_rr_38" => {
            let prefix = arg_expr(0);
            let opcode = arg_expr(1);
            let reg = reg_expr(2);
            let rm = reg_expr(3);
            Ok(quote! {
                let __p = #prefix as u8;
                if __p != 0 { sink.put1(__p); }
                let __reg = #reg;
                let __rm = #rm;
                if (__reg & 0x8) != 0 || (__rm & 0x8) != 0 {
                    let __rex: u8 = 0x40
                        | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                        | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                    sink.put1(__rex);
                }
                sink.put1(0x0Fu8);
                sink.put1(0x38u8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @sse_rr_opsize prefix opcode opsize reg rm — SSE/GPR 0F xx /r，
        // REX.W/66 前缀按运行时 opsize 动态（修复固定 w=1 导致窄宽度也 64 位执行）。
        "sse_rr_opsize" => {
            let prefix = arg_expr(0);
            let opcode = arg_expr(1);
            let opsize = arg_expr(2);
            let reg = reg_expr(3);
            let rm = reg_expr(4);
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __p = #prefix as u8;
                if __p != 0 { sink.put1(__p); }
                if __opsize == 16 { sink.put1(0x66u8); }
                let __reg = #reg;
                let __rm = #rm;
                let __rex: u8 = 0x40
                    | if __opsize == 64 { 0x08 } else { 0 }
                    | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                    | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__reg & 0x8) != 0 || (__rm & 0x8) != 0 {
                    sink.put1(__rex);
                }
                sink.put1(0x0Fu8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @vex_rrvvv map pp w l opcode reg rm vv has_src — 3 字节 VEX（C4）三操作数 AVX
        // （vaddps 等：dest=ModRM.reg、rm=ModRM.r/m、vvvv=~src1）。
        // has_src=0（无 src1，如 vextractf128/vbroadcastss）：vvvv 字段直接编码 1111
        // （Intel 约定：无操作数时 vvvv=1111，不反转）；has_src=1：vvvv = ~vv & 0xF。
        "vex_rrvvv" => {
            let map = arg_expr(0);
            let pp = arg_expr(1);
            let w = arg_expr(2);
            let l = arg_expr(3);
            let opcode = arg_expr(4);
            let reg = reg_expr(5);
            let rm = reg_expr(6);
            let vv = reg_expr(7);
            let has_src = arg_expr(8);
            Ok(quote! {
                assert!(crate::prelude::avx_available(), "AVX instruction on non-AVX CPU");
                let __reg = #reg;
                let __rm = #rm;
                let __vv = #vv;
                sink.put1(0xC4u8);
                // R = ~reg.bit3、B = ~rm.bit3；无 SIB 时 X 位标准编码置 1。
                let __b: u8 = if (__rm & 0x8) != 0 { 0 } else { 1 };
                let __x: u8 = 1;
                let __r: u8 = if (__reg & 0x8) != 0 { 0 } else { 1 };
                sink.put1((__r << 7) | (__x << 6) | (__b << 5) | (#map as u8 & 0x1F));
                let __w = #w as u8;
                let __vvvv: u8 = if #has_src == 0 { 0x0F } else { (!(__vv as u8 & 0x0F)) & 0x0F };
                sink.put1((__w << 7) | (__vvvv << 3) | ((#l as u8 & 0x1) << 2) | (#pp as u8 & 0x3));
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @vex_rrvvv_avx2 map pp w l opcode reg rm vv has_src — 同 @vex_rrvvv 但断言
        // AVX2（整数 256 位指令：VPADDD/VPSUBD/VPXOR/VPMULLD 等，AVX1 无 ymm 整数）。
        "vex_rrvvv_avx2" => {
            let map = arg_expr(0);
            let pp = arg_expr(1);
            let w = arg_expr(2);
            let l = arg_expr(3);
            let opcode = arg_expr(4);
            let reg = reg_expr(5);
            let rm = reg_expr(6);
            let vv = reg_expr(7);
            let has_src = arg_expr(8);
            Ok(quote! {
                assert!(crate::prelude::avx2_available(), "AVX2 instruction on non-AVX2 CPU");
                let __reg = #reg;
                let __rm = #rm;
                let __vv = #vv;
                sink.put1(0xC4u8);
                let __b: u8 = if (__rm & 0x8) != 0 { 0 } else { 1 };
                let __x: u8 = 1;
                let __r: u8 = if (__reg & 0x8) != 0 { 0 } else { 1 };
                sink.put1((__r << 7) | (__x << 6) | (__b << 5) | (#map as u8 & 0x1F));
                let __w = #w as u8;
                let __vvvv: u8 = if #has_src == 0 { 0x0F } else { (!(__vv as u8 & 0x0F)) & 0x0F };
                sink.put1((__w << 7) | (__vvvv << 3) | ((#l as u8 & 0x1) << 2) | (#pp as u8 & 0x3));
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @vex_rrvvv_imm map pp w l opcode reg rm vv has_src imm — VEX + 立即数
        // （vinsertf128/vextractf128 等：66 0F3A xx /r ib）。
        "vex_rrvvv_imm" => {
            let map = arg_expr(0);
            let pp = arg_expr(1);
            let w = arg_expr(2);
            let l = arg_expr(3);
            let opcode = arg_expr(4);
            let reg = reg_expr(5);
            let rm = reg_expr(6);
            let vv = reg_expr(7);
            let has_src = arg_expr(8);
            let imm = arg_expr(9);
            Ok(quote! {
                assert!(crate::prelude::avx_available(), "AVX instruction on non-AVX CPU");
                let __reg = #reg;
                let __rm = #rm;
                let __vv = #vv;
                sink.put1(0xC4u8);
                let __b: u8 = if (__rm & 0x8) != 0 { 0 } else { 1 };
                let __x: u8 = 1;
                let __r: u8 = if (__reg & 0x8) != 0 { 0 } else { 1 };
                sink.put1((__r << 7) | (__x << 6) | (__b << 5) | (#map as u8 & 0x1F));
                let __w = #w as u8;
                let __vvvv: u8 = if #has_src == 0 { 0x0F } else { (!(__vv as u8 & 0x0F)) & 0x0F };
                sink.put1((__w << 7) | (__vvvv << 3) | ((#l as u8 & 0x1) << 2) | (#pp as u8 & 0x3));
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
                sink.put1(#imm as u8);
            })
        }

        // @sse_rr_imm8 prefix opcode reg rm imm — SSE 0F xx /r ib（PSHUFD/SHUFPS 等）。
        "sse_rr_imm8" => {
            let prefix = arg_expr(0);
            let opcode = arg_expr(1);
            let reg = reg_expr(2);
            let rm = reg_expr(3);
            let imm = arg_expr(4);
            Ok(quote! {
                let __p = #prefix as u8;
                if __p != 0 { sink.put1(__p); }
                let __reg = #reg;
                let __rm = #rm;
                if (__reg & 0x8) != 0 || (__rm & 0x8) != 0 {
                    let __rex: u8 = 0x40
                        | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                        | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                    sink.put1(__rex);
                }
                sink.put1(0x0Fu8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
                sink.put1(#imm as u8);
            })
        }

        // @sse_rr_w prefix opcode reg rm — SSE 恒发 REX.W（GPR↔XMM 等）。
        "sse_rr_w" => {
            let prefix = arg_expr(0);
            let opcode = arg_expr(1);
            let reg = reg_expr(2);
            let rm = reg_expr(3);
            Ok(quote! {
                let __p = #prefix as u8;
                if __p != 0 { sink.put1(__p); }
                let __reg = #reg;
                let __rm = #rm;
                let __rex: u8 = 0x48
                    | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                    | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                sink.put1(__rex);
                sink.put1(0x0Fu8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @sse_ps_rr opcode reg rm — SSE packed-single（无强制前缀）。
        "sse_ps_rr" => {
            let opcode = arg_expr(0);
            let reg = reg_expr(1);
            let rm = reg_expr(2);
            Ok(quote! {
                let __reg = #reg;
                let __rm = #rm;
                if (__reg & 0x8) != 0 || (__rm & 0x8) != 0 {
                    let __rex: u8 = 0x40
                        | if (__reg & 0x8) != 0 { 0x04 } else { 0 }
                        | if (__rm & 0x8) != 0 { 0x01 } else { 0 };
                    sink.put1(__rex);
                }
                sink.put1(0x0Fu8);
                sink.put1(#opcode as u8);
                sink.put1((3u8 << 6) | ((__reg as u8 & 0x7) << 3) | (__rm as u8 & 0x7));
            })
        }

        // @push_reg reg / @pop_reg reg — PUSH/POP r64（REX 扩展）。
        "push_reg" => {
            let reg = reg_expr(0);
            Ok(quote! {
                let __reg = #reg;
                if (__reg & 0x8) != 0 { sink.put1(0x41u8); }
                sink.put1(0x50u8 | (__reg as u8 & 0x7));
            })
        }
        "pop_reg" => {
            let reg = reg_expr(0);
            Ok(quote! {
                let __reg = #reg;
                if (__reg & 0x8) != 0 { sink.put1(0x41u8); }
                sink.put1(0x58u8 | (__reg as u8 & 0x7));
            })
        }

        // @mov_imm64 reg imm — MOV r64, imm64（REX.W + B8+r + 8 字节）。
        "mov_imm64" => {
            let reg = reg_expr(0);
            let imm = arg_expr(1);
            Ok(quote! {
                let __reg = #reg;
                if (__reg & 0x8) != 0 { sink.put1(0x49u8); } else { sink.put1(0x48u8); }
                sink.put1(0xB8u8 | (__reg as u8 & 0x7));
                let __imm = #imm as u64;
                sink.put1(__imm as u8);
                sink.put1((__imm >> 8) as u8);
                sink.put1((__imm >> 16) as u8);
                sink.put1((__imm >> 24) as u8);
                sink.put1((__imm >> 32) as u8);
                sink.put1((__imm >> 40) as u8);
                sink.put1((__imm >> 48) as u8);
                sink.put1((__imm >> 56) as u8);
            })
        }

        // @setcc dest cond — SETcc r/m8（0F 90+cc /r）。
        "setcc" => {
            let dest = reg_expr(0);
            let cond = arg_expr(1);
            Ok(quote! {
                let __dest = #dest;
                if (__dest & 0x8) != 0 { sink.put1(0x41u8); }
                else if __dest >= 4 { sink.put1(0x40u8); }
                sink.put1(0x0Fu8);
                sink.put1(0x90u8 | (#cond as u8 & 0x0F));
                sink.put1((3u8 << 6) | (__dest as u8 & 0x7));
            })
        }

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
                let mut __v = #reg.to_index() as u64;
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
        "lea_rbp_disp" => {
            // @lea_rbp_disp dest base disp — REX.W + 8D + ModRM(base) + disp8/disp32。
            // base 为寄存器字段或物理编号字面量（x86 RBP=5）。
            let dest = reg_expr(0);
            let base = reg_expr(1);
            let disp = arg_expr(2);
            Ok(quote! {
                let __rd = #dest;
                let __base = #base;
                let __disp = #disp as i32;
                let __rex: u8 = 0x48 | if (__rd & 0x8) != 0 { 0x04 } else { 0 };
                sink.put1(__rex);
                sink.put1(0x8Du8);
                if (-128i32..=127i32).contains(&__disp) {
                    sink.put1((1u8 << 6) | ((__rd as u8 & 0x7) << 3) | (__base as u8 & 0x7));
                    sink.put1(__disp as u8);
                } else {
                    sink.put1((2u8 << 6) | ((__rd as u8 & 0x7) << 3) | (__base as u8 & 0x7));
                    sink.put4(__disp as u32);
                }
            })
        }
        "lea_sib" => {
            // @lea_sib dest base index scale disp — REX.W + 8D + ModRM/SIB + disp。
            // force-disp base 编号来自 [meta].modrm_force_disp_base。
            let dest = reg_expr(0);
            let base = reg_expr(1);
            let index = reg_expr(2);
            let scale = arg_expr(3);
            let disp = arg_expr(4);
            let force = &model.meta.modrm_force_disp_base;
            let force_toks: Vec<TokenStream> = force.iter().map(|&b| quote! { #b }).collect();
            Ok(quote! {
                let __rd = #dest;
                let __base = #base;
                let __index = #index;
                let __scale = #scale;
                let __disp = #disp as i32;
                let __rex: u8 = 0x48
                    | if (__rd & 0x8) != 0 { 0x04 } else { 0 }
                    | if (__index & 0x8) != 0 { 0x02 } else { 0 }
                    | if (__base & 0x8) != 0 { 0x01 } else { 0 };
                sink.put1(__rex);
                sink.put1(0x8Du8);
                const FORCE_DISP: &[u8] = &[#(#force_toks),*];
                let __scale_bits: u8 = match __scale { 1 => 0, 2 => 1, 4 => 2, 8 => 3, _ => 0 };
                // index=4（RSP）是"无 index"哨兵；base=4（RSP）时 modrm.rm=4
                // 也触发 SIB 期待（mod≠11 且 rm=100 → SIB）。两种情况都必须发
                // SIB，否则 SIB 字节被 CPU 当作 disp8、真正的 disp 被吞进下一条
                // 指令——指令流错位（栈参数 lea [rsp+disp] SEGV/SIGILL）。
                let __no_index = (__index as u8 & 0x7) == 4;
                let __base_is_rsp = (__base as u8 & 0x7) == 4;
                let __has_sib = !__no_index || __base_is_rsp;
                let __rm: u8 = if __has_sib { 4 } else { __base as u8 & 0x7 };
                let __modrm = |m: u8| -> u8 { (m << 6) | ((__rd as u8 & 0x7) << 3) | __rm };
                let __sib = |s: u8| -> u8 { (s << 6) | ((__index as u8 & 0x7) << 3) | (__base as u8 & 0x7) };
                let __need_disp = __disp != 0 || FORCE_DISP.contains(&(__base as u8));
                if !__need_disp {
                    sink.put1(__modrm(0));
                    if __has_sib { sink.put1(__sib(__scale_bits)); }
                } else if (-128i32..=127i32).contains(&__disp) {
                    sink.put1(__modrm(1));
                    if __has_sib { sink.put1(__sib(__scale_bits)); }
                    sink.put1(__disp as u8);
                } else {
                    sink.put1(__modrm(2));
                    if __has_sib { sink.put1(__sib(__scale_bits)); }
                    sink.put4(__disp as u32);
                }
            })
        }
        "bswap_r" => {
            // @bswap_r dest opsize — BSWAP r/m（0F C8+r；64 位加 REX.W + REX.B=dest[3]）
            let reg = reg_expr(0);
            let opsize = arg_expr(1);
            Ok(quote! {
                let __r = #reg;
                let __opsize = #opsize as i64;
                let __rex: u8 = if __opsize == 64 { 0x48 } else { 0x40 }
                    | if (__r & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__r & 0x8) != 0 { sink.put1(__rex); }
                sink.put1(0x0Fu8);
                sink.put1(0xC8u8 | (__r as u8 & 0x7));
            })
        }

        "shift_reg" => {
            // @shift_reg count dest src ext opsize — 可变计数移位：count 为计数
            // 寄存器物理编号（x86 CL=1，架构事实由 TOML 传入），dest 为
            // 被移位寄存器（r/m 字段），ext 为 /digit 扩展码。
            // 计数不在 count 寄存器时先 mov：REX.W + 8B + ModRM。
            // opsize==64 才带 REX.W（32 位移位不得加，否则宽度错）。
            let count = arg_expr(0);
            let dest = reg_expr(1);
            let src = reg_expr(2);
            let op_ext = arg_expr(3);
            let opsize = arg_expr(4);
            Ok(quote! {
                let __opsize = #opsize as i64;
                let __cnt_reg = #count as u32;
                let __cnt = #src;
                if __cnt != __cnt_reg {
                    // mov count-reg, cnt — REX.W + 0x8B + modrm(3, count_reg, cnt)
                    let __rex: u8 = if (__cnt & 0x8) != 0 { 0x49 } else { 0x48 };
                    sink.put1(__rex);
                    sink.put1(0x8Bu8);
                    sink.put1((3u8 << 6) | ((__cnt_reg as u8 & 0x7) << 3) | (__cnt as u8 & 0x7));
                }
                let __d = #dest;
                // 64 位移位需 REX.W=1；32 位不得加；REX.B set if dest ≥ 8
                let __rex: u8 = 0x40
                    | if __opsize == 64 { 0x08 } else { 0 }
                    | if (__d & 0x8) != 0 { 0x01 } else { 0 };
                if __opsize == 64 || (__d & 0x8) != 0 { sink.put1(__rex); }
                sink.put1(0xD3u8);
                sink.put1((3u8 << 6) | ((#op_ext as u8 & 0x7) << 3) | (__d as u8 & 0x7));
            })
        }
        // Cross-function call — 32-bit instruction whose displacement lives in
        // the low bits of the same word (AArch64 BL imm26, RISC-V JAL UJ):
        // emit the instruction template and record a relocation against the
        // "@N" symbol (N = FuncRef number from the `func` lowering variable).
        "call_reloc" => {
            let target = arg_ident(0);
            let tpl = arg_expr(1);
            Ok(quote! {
                let __func = *(#target) as u32;
                let __off = sink.offset();
                sink.put4(#tpl as u32);
                sink.add_reloc(
                    __off,
                    crate::RelocKind::REL4,
                    &format!("@{}", __func),
                    0,
                );
            })
        }
        // Cross-function call — x86 style: opcode byte + 32-bit rel32
        // placeholder, with the relocation recorded at the displacement.
        "call_reloc32" => {
            let target = arg_ident(0);
            let opc = arg_expr(1);
            Ok(quote! {
                let __func = *(#target) as u32;
                sink.put1(#opc as u8);
                let __off = sink.offset();
                sink.put4(0u32);
                sink.add_reloc(
                    __off,
                    crate::RelocKind::REL4,
                    &format!("@{}", __func),
                    0,
                );
            })
        }
        // Global address — absolute 8-byte placeholder + relocation against
        // "G{id}" (module global symbol, resolved by the JIT data segment).
        "abs_reloc" => {
            let target = arg_ident(0);
            Ok(quote! {
                let __g = *(#target) as u32;
                let __off = sink.offset();
                sink.put8(0u64);
                sink.add_reloc(
                    __off,
                    crate::RelocKind::Absolute(8),
                    &format!("G{}", __g),
                    0,
                );
            })
        }
        // lea rd, [rip+disp32] — REX.W + 8D + ModRM(rm=101) + disp32,with a
        // PC-relative reloc (RelocKind::Relative(4,-4) → PE IMAGE_REL_AMD64_REL32).
        // PC-relative offsets are loader-independent (no .reloc entry needed),
        // so global addresses survive ASLR — unlike the absolute movabs
        // placeholder (box_write 0xC000001D root cause: the 5th movabs reloc
        // landed beyond the .reloc VirtualSize and was never fixed up under ASLR).
        "lea_rip_rel" => {
            let dest = reg_expr(0);
            let target = arg_ident(1);
            Ok(quote! {
                let __rd = #dest;
                let __g = *(#target) as u32;
                let __rex: u8 = 0x48 | if (__rd & 0x8) != 0 { 0x04 } else { 0 };
                sink.put1(__rex);
                sink.put1(0x8Du8);
                // mod=00, reg=dest, rm=101 (RIP-relative)
                sink.put1(((__rd as u8 & 0x7) << 3) | 0x05);
                let __off = sink.offset();
                sink.put4(0u32);
                sink.add_reloc(
                    __off,
                    crate::RelocKind::Relative(4, -4),
                    &format!("G{}", __g),
                    0,
                );
            })
        }
        _ => Err(format!(
            "unknown encoding primitive '@{name}'. Available: @leb128, @sleb128, @leb128_reg, @modrm, @modrm_mem, @op_rm, @modrm_imm32, @op_rm_imm32, @cmovcc, @sse_rr, @sse_rr_3a, @sse_rr_w, @sse_ps_rr, @sse_rr_imm8, @push_reg, @pop_reg, @mov_imm64, @setcc, @lea_sib, @lea_rbp_disp, @lea_rip_rel, @shift_reg, @call_reloc, @call_reloc32, @abs_reloc"
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
            Some(FieldType::Ireg | FieldType::Freg) => {
                let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
                quote! { #ident.to_index() as u64 }
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

#[cfg(test)]
mod memref_primitive_tests {
    use super::*;
    use crate::model::{FieldType, IsaModel};

    /// @modrm_mem 的 rm 参数为 MemRef 字段时，生成代码从 mem.base/mem.offset 展开。
    #[test]
    fn test_modrm_mem_memref_field() {
        let src = "[meta]\nname = \"t\"\nmodrm_force_disp_base = [5, 13]\n[reg.gpr64]\ncount = 16\nwidth = 64";
        let model: IsaModel = toml::from_str(src).expect("parse");

        let mut field_map: std::collections::HashMap<String, &FieldType> =
            std::collections::HashMap::new();
        field_map.insert("dest".into(), &FieldType::GprReg);
        field_map.insert("mem".into(), &FieldType::MemRef);

        let args: Vec<String> = ["64", "0x8B", "dest", "mem"]
            .map(|s| s.to_string())
            .to_vec();
        let ts = gen_primitive("modrm_mem", &args, &field_map, &model).expect("gen");
        let out = ts.to_string();
        assert!(
            out.contains("mem . base as u32"),
            "MemRef 字段应展开 base: {out}"
        );
        assert!(
            out.contains("mem . offset as i32"),
            "MemRef 字段应展开 offset: {out}"
        );
        assert!(out.contains("as u8"), "opcode 应编码: {out}");
    }

    /// 普通寄存器 rm 参数保持原有行为（disp 转 i32）。
    #[test]
    fn test_modrm_mem_reg_field() {
        let src = "[meta]\nname = \"t\"\nmodrm_force_disp_base = [5, 13]\n[reg.gpr64]\ncount = 16\nwidth = 64";
        let model: IsaModel = toml::from_str(src).expect("parse");
        let mut field_map: std::collections::HashMap<String, &FieldType> =
            std::collections::HashMap::new();
        field_map.insert("dest".into(), &FieldType::GprReg);
        field_map.insert("src".into(), &FieldType::GprReg);

        let args: Vec<String> = ["64", "0x8B", "dest", "src", "0"]
            .map(|s| s.to_string())
            .to_vec();
        let ts = gen_primitive("modrm_mem", &args, &field_map, &model).expect("gen");
        let out = ts.to_string();
        assert!(out.contains("as i32"), "寄存器路径 disp 应转 i32: {out}");
        assert!(!out.contains("mem . base"), "寄存器路径不应展开 MemRef");
    }
}

#[cfg(test)]
mod modrm_prefix_probe {
    use super::*;
    use crate::model::{FieldType, IsaModel};

    #[test]
    fn probe_f2_prefix() {
        let src = "[meta]\nname = \"t\"\nmodrm_force_disp_base = [5, 13]\n[reg.gpr64]\ncount = 16\nwidth = 64\n[reg.fpr64]\ncount = 16\nwidth = 64";
        let model: IsaModel = toml::from_str(src).expect("parse");
        let mut field_map: std::collections::HashMap<String, &FieldType> =
            std::collections::HashMap::new();
        field_map.insert("dest".into(), &FieldType::XmmReg);
        field_map.insert("mem".into(), &FieldType::MemRef);
        let args: Vec<String> = ["32", "0x10", "dest", "mem", "0xF2", "0x0F"]
            .map(|s| s.to_string())
            .to_vec();
        let ts = gen_primitive("modrm_mem", &args, &field_map, &model).expect("gen");
        let out = ts.to_string();
        eprintln!("QUOTE: {out}");
        assert!(out.contains("242u64"), "prefix 0xF2(242) 缺失: {out}");
        assert!(out.contains("15u64"), "escape 0x0F(15) 缺失: {out}");
    }
}
