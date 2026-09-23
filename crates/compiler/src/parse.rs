//! Parse: schema text to declarations with token positions. No semantic rule
//! lives here; [`crate::validate`] consumes the result.
use serde_json::{Map, Value, json};
#[derive(Clone, Debug)]
struct Token {
    text: String,
    line: usize,
    col: usize,
}
/// Source position of a declaration, kept beside the parsed value so semantic
/// checks can point at the declaration instead of the end of the input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}
/// A diagnostic at a source position: `line:col: message`.
pub fn at(pos: Pos, msg: impl AsRef<str>) -> String {
    format!("{}:{}: {}", pos.line, pos.col, msg.as_ref())
}
struct Parser {
    tokens: Vec<Token>,
    i: usize,
}
impl Parser {
    fn pos(&self) -> Pos {
        let t = &self.tokens[self.i.min(self.tokens.len() - 1)];
        Pos {
            line: t.line,
            col: t.col,
        }
    }
    fn err(&self, msg: impl AsRef<str>) -> String {
        let t = &self.tokens[self.i.min(self.tokens.len() - 1)];
        format!(
            "{}:{}: {} (found '{}')",
            t.line,
            t.col,
            msg.as_ref(),
            t.text
        )
    }
    fn peek(&self) -> &str {
        &self.tokens[self.i].text
    }
    fn take(&mut self) -> String {
        let s = self.peek().to_string();
        if s != "<eof>" {
            self.i += 1;
        }
        s
    }
    fn eat(&mut self, s: &str) -> bool {
        if self.peek() == s {
            self.take();
            true
        } else {
            false
        }
    }
    fn need(&mut self, s: &str) -> Result<(), String> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.err(format!("expected {s}")))
        }
    }
    fn ident(&mut self) -> Result<String, String> {
        let s = self.take();
        if s.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            Ok(s)
        } else {
            Err(self.err("expected identifier"))
        }
    }
    fn expression(&mut self) -> Result<Value, String> {
        if self.eat("[") {
            let mut items = vec![];
            while !self.eat("]") {
                items.push(self.expression()?);
                if self.peek() != "]" {
                    self.need(",")?;
                }
            }
            return Ok(json!(items));
        }
        if self.peek().starts_with('"') {
            return serde_json::from_str(&self.take()).map_err(|_| self.err("invalid string"));
        }
        let mut name = self.ident()?;
        while self.eat(".") {
            name.push('.');
            name.push_str(&self.ident()?);
        }
        if self.peek() == "(" {
            return Ok(json!({"name":name,"arguments":self.arguments()?}));
        }
        Ok(json!(name))
    }
    fn arguments(&mut self) -> Result<Value, String> {
        self.need("(")?;
        let mut args = serde_json::Map::new();
        let mut positional = 0;
        while !self.eat(")") {
            let named = self.tokens.get(self.i + 1).is_some_and(|t| t.text == ":");
            let key = if named {
                let k = self.ident()?;
                self.need(":")?;
                k
            } else {
                let k = positional.to_string();
                positional += 1;
                k
            };
            let value = self.expression()?;
            if args.insert(key, value).is_some() {
                return Err(self.err("duplicate argument"));
            }
            if self.peek() != ")" {
                self.need(",")?;
            }
        }
        Ok(Value::Object(args))
    }
    /// The `(n)` of a `@@version` directive: a positive integer within the safe range.
    fn version(&mut self, seen: &mut bool) -> Result<u64, String> {
        if *seen {
            return Err(self.err("duplicate version"));
        }
        *seen = true;
        self.need("(")?;
        let version = self
            .take()
            .parse::<u64>()
            .map_err(|_| self.err("expected positive version"))?;
        if version == 0 || version > axton_core::MAX_SAFE_INTEGER {
            return Err(self.err("version must be positive"));
        }
        self.need(")")?;
        Ok(version)
    }
    /// The arguments of a `@deprecated` directive, GraphQL style: nothing, or
    /// `(reason: "text")`.
    fn deprecation(&mut self) -> Result<Option<String>, String> {
        if !self.eat("(") {
            return Ok(None);
        }
        if self.eat(")") {
            return Ok(None);
        }
        if self.ident()? != "reason" {
            return Err(self.err("deprecated accepts only reason"));
        }
        self.need(":")?;
        if !self.peek().starts_with('"') {
            return Err(self.err("deprecated reason must be a string"));
        }
        let reason: String =
            serde_json::from_str(&self.take()).map_err(|_| self.err("invalid string"))?;
        self.need(")")?;
        Ok(Some(reason))
    }
    fn names(&mut self, end: &str) -> Result<Vec<String>, String> {
        let mut n = vec![];
        while !self.eat(end) {
            n.push(self.ident()?);
            if self.peek() != end {
                self.need(",")?;
            }
        }
        Ok(n)
    }
}
fn lex(s: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<_> = s.chars().collect();
    let (mut i, mut line, mut col) = (0, 1, 1);
    let mut out = vec![];
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            col = 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
                col += 1;
            }
            continue;
        }
        let start = i;
        let column = col;
        if c.is_ascii_alphanumeric() || c == '_' {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
                col += 1;
            }
        } else if c == '"' {
            i += 1;
            col += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                    col += 1;
                }
                i += 1;
                col += 1;
            }
            if i >= chars.len() {
                return Err(format!("{line}:{column}: unterminated string"));
            }
            i += 1;
            col += 1;
        } else if "{}()[]?,.@<>:".contains(c) {
            i += 1;
            col += 1;
        } else {
            return Err(format!("{line}:{col}: unsupported character {c}"));
        }
        out.push(Token {
            text: chars[start..i].iter().collect(),
            line,
            col: column,
        });
    }
    out.push(Token {
        text: "<eof>".into(),
        line,
        col,
    });
    Ok(out)
}

/// Everything the source declares, in source order, each with the position of
/// its first token. Expressions (directive arguments, slot bindings, sequence
/// arguments) stay as JSON values; validation gives them meaning.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Declarations {
    pub enums: Vec<EnumDecl>,
    pub models: Vec<ModelDecl>,
    pub mutations: Vec<MutationDecl>,
    pub prerequisites: Vec<PrerequisiteDecl>,
    /// Position of the end of input, for diagnostics that have no declaration.
    pub end: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub values: Vec<String>,
    /// `value @deprecated(reason: "…")`: the value and its optional reason.
    pub deprecated: Vec<(String, Option<String>)>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDecl {
    pub name: String,
    /// The read-contract version declared by `@@version(n)`; 1 when omitted.
    pub version: u64,
    pub identity: Vec<String>,
    pub fields: Vec<FieldDecl>,
    pub unique: Vec<UniqueDecl>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct FieldDecl {
    pub name: String,
    pub type_name: String,
    pub list: bool,
    pub nullable: bool,
    /// `@reference`, `@inverse` and `@requires` arguments, keyed by directive name.
    pub attributes: Map<String, Value>,
    /// `@deprecated(reason: "…")`: `Some(reason)` when present, the reason itself optional.
    pub deprecated: Option<Option<String>>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct UniqueDecl {
    pub fields: Vec<String>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct MutationDecl {
    pub name: String,
    pub version: u64,
    pub slots: Vec<SlotDecl>,
    pub sequence: Option<SequenceDecl>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceDecl {
    pub arguments: Value,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SlotDecl {
    pub name: String,
    pub model: String,
    pub operation: String,
    pub cardinality: String,
    pub allowed_patch_fields: Option<Vec<String>>,
    /// The `(relation: parentSlot, …)` arguments, as a JSON object.
    pub relation_bindings: Value,
    /// `@deprecated(reason: "…")` on the slot.
    pub deprecated: Option<Option<String>>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PrerequisiteDecl {
    pub name: String,
    pub fields: Vec<PrerequisiteField>,
    pub pos: Pos,
}
#[derive(Clone, Debug, PartialEq)]
pub struct PrerequisiteField {
    pub name: String,
    pub type_name: String,
    pub pos: Pos,
}

/// Turn schema text into declarations. Errors are syntax errors only:
/// `line:col: message (found 'token')`.
pub fn parse(source: &str) -> Result<Declarations, String> {
    let mut p = Parser {
        tokens: lex(source)?,
        i: 0,
    };
    let mut d = Declarations::default();
    while p.peek() != "<eof>" {
        let pos = p.pos();
        let kind = p.take();
        let name = p.ident()?;
        if kind == "prerequisite" {
            p.need("(")?;
            let mut fields = vec![];
            while !p.eat(")") {
                let pos = p.pos();
                let name = p.ident()?;
                let type_name = p.ident()?;
                fields.push(PrerequisiteField {
                    name,
                    type_name,
                    pos,
                });
                if p.peek() != ")" {
                    p.need(",")?;
                }
            }
            d.prerequisites.push(PrerequisiteDecl { name, fields, pos });
            continue;
        }
        p.need("{")?;
        match kind.as_str() {
            "enum" => {
                let (mut values, mut deprecated) = (vec![], vec![]);
                while !p.eat("}") {
                    let value = p.ident()?;
                    let mut marked = false;
                    while p.peek() == "@" && p.tokens.get(p.i + 1).is_some_and(|t| t.text != "@") {
                        p.need("@")?;
                        let attr = p.ident()?;
                        if attr != "deprecated" {
                            return Err(p.err(format!("unsupported enum value directive {attr}")));
                        }
                        if marked {
                            return Err(p.err("duplicate deprecated"));
                        }
                        marked = true;
                        deprecated.push((value.clone(), p.deprecation()?));
                    }
                    values.push(value);
                    p.eat(",");
                }
                d.enums.push(EnumDecl {
                    name,
                    values,
                    deprecated,
                    pos,
                });
            }
            "model" => {
                let (mut fields, mut identity, mut unique) = (vec![], vec![], vec![]);
                let (mut version, mut version_seen) = (1, false);
                while !p.eat("}") {
                    let directive_pos = p.pos();
                    if p.eat("@") {
                        p.need("@")?;
                        let attr = p.ident()?;
                        if attr == "version" {
                            version = p.version(&mut version_seen)?;
                            continue;
                        }
                        p.need("(")?;
                        let names = p.names(")")?;
                        match attr.as_str() {
                            "id" => {
                                if !identity.is_empty() {
                                    return Err(p.err("duplicate identity"));
                                }
                                identity = names
                            }
                            "unique" => unique.push(UniqueDecl {
                                fields: names,
                                pos: directive_pos,
                            }),
                            _ => return Err(p.err(format!("unsupported model directive {attr}"))),
                        }
                        continue;
                    }
                    let field = p.ident()?;
                    let type_name = p.ident()?;
                    let list = if p.eat("[") {
                        p.need("]")?;
                        true
                    } else {
                        false
                    };
                    let nullable = p.eat("?");
                    let mut attributes = Map::new();
                    let mut deprecated = None;
                    while p.peek() == "@" && p.tokens.get(p.i + 1).is_some_and(|t| t.text != "@") {
                        p.need("@")?;
                        let attr = p.ident()?;
                        if attr == "deprecated" {
                            if deprecated.is_some() {
                                return Err(p.err("duplicate field directive"));
                            }
                            deprecated = Some(p.deprecation()?);
                            continue;
                        }
                        if !["reference", "inverse", "requires"].contains(&attr.as_str()) {
                            return Err(p.err(format!("unsupported field directive {attr}")));
                        }
                        let args = p.arguments()?;
                        if attributes.insert(attr, args).is_some() {
                            return Err(p.err("duplicate field directive"));
                        }
                    }
                    fields.push(FieldDecl {
                        name: field,
                        type_name,
                        list,
                        nullable,
                        attributes,
                        deprecated,
                        pos: directive_pos,
                    });
                }
                d.models.push(ModelDecl {
                    name,
                    version,
                    identity,
                    fields,
                    unique,
                    pos,
                });
            }
            "mutation" => {
                let (mut slots, mut version) = (vec![], 1);
                let mut sequence = None;
                let mut version_seen = false;
                while !p.eat("}") {
                    let directive_pos = p.pos();
                    if p.eat("@") {
                        p.need("@")?;
                        let attr = p.ident()?;
                        if attr == "sequence" {
                            sequence = Some(SequenceDecl {
                                arguments: p.arguments()?,
                                pos: directive_pos,
                            });
                            continue;
                        }
                        if attr != "version" {
                            return Err(p.err(format!("unsupported mutation directive {attr}")));
                        }
                        version = p.version(&mut version_seen)?;
                        continue;
                    }
                    let slot = p.ident()?;
                    let model = p.ident()?;
                    p.need(".")?;
                    let operation = p.ident()?;
                    if !["create", "update", "delete"].contains(&operation.as_str()) {
                        return Err(p.err("unknown operation"));
                    }
                    let allowed_patch_fields = if p.eat("<") {
                        if operation != "update" {
                            return Err(p.err("field restriction requires update"));
                        }
                        Some(p.names(">")?)
                    } else {
                        None
                    };
                    let relation_bindings = if p.peek() == "(" {
                        p.arguments()?
                    } else {
                        json!({})
                    };
                    let cardinality = if p.eat("[") {
                        p.need("]")?;
                        "list"
                    } else if p.eat("?") {
                        "optional"
                    } else {
                        "single"
                    };
                    let mut deprecated = None;
                    while p.peek() == "@" && p.tokens.get(p.i + 1).is_some_and(|t| t.text != "@") {
                        p.need("@")?;
                        let attr = p.ident()?;
                        if attr != "deprecated" {
                            return Err(p.err(format!("unsupported slot directive {attr}")));
                        }
                        if deprecated.is_some() {
                            return Err(p.err("duplicate deprecated"));
                        }
                        deprecated = Some(p.deprecation()?);
                    }
                    slots.push(SlotDecl {
                        name: slot,
                        model,
                        operation,
                        cardinality: cardinality.into(),
                        allowed_patch_fields,
                        relation_bindings,
                        deprecated,
                        pos: directive_pos,
                    });
                }
                d.mutations.push(MutationDecl {
                    name,
                    version,
                    slots,
                    sequence,
                    pos,
                });
            }
            _ => return Err(p.err(format!("unsupported declaration {kind}"))),
        }
    }
    d.end = p.pos();
    Ok(d)
}
