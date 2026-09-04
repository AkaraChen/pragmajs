use crate::syntax::*;
use oxc_allocator::Allocator;
use oxc_ast::ast::{BindingPattern, CommentPosition, Expression, Program, Statement};
use oxc_ast_visit::Visit;
use oxc_ast_visit::walk::walk_statement;
use oxc_span::Span;
use pragma_loc::byte_offset_to_line_col;
use pragma_parse::parse;

#[derive(Debug, Clone, PartialEq)]
enum TokenKind {
    Ident,
    Number,
    String,
    Op,
    Colon,
    Pipe,
    Arrow,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    value: String,
    _pos: usize,
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn tokenize(mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        while self.pos < self.src.len() {
            if let Some(t) = self.next()? {
                tokens.push(t);
            }
        }
        tokens.push(Token {
            kind: TokenKind::Eof,
            value: String::new(),
            _pos: self.pos,
        });
        Ok(tokens)
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            if ch.is_whitespace() {
                self.pos += ch.len_utf8();
            } else if ch == '/' && self.src[self.pos + 1..].starts_with('/') {
                while self.pos < self.src.len() && !self.src[self.pos..].starts_with('\n') {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Result<Option<Token>, String> {
        self.skip_ws();
        if self.pos >= self.src.len() {
            return Ok(None);
        }
        let start = self.pos;
        let ch = self.src[start..].chars().next().unwrap();

        if ch == '"' || ch == '\'' {
            return Ok(Some(self.read_string(ch)?));
        }

        if ch.is_ascii_digit()
            || (ch == '.'
                && self.src[start + 1..]
                    .chars()
                    .next()
                    .map(|c| c.is_ascii_digit())
                    .unwrap_or(false))
        {
            return Ok(Some(self.read_number()?));
        }

        if ch == '_' || ch.is_alphabetic() || ch == '$' {
            return Ok(Some(self.read_ident()));
        }

        let two = &self.src[start..self.src.len().min(start + 2)];
        let three = &self.src[start..self.src.len().min(start + 3)];

        if three == "===" {
            self.pos += 3;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "===".into(),
                _pos: start,
            }));
        }
        if three == "!==" {
            self.pos += 3;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "!==".into(),
                _pos: start,
            }));
        }
        if two == "==" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "==".into(),
                _pos: start,
            }));
        }
        if two == "!=" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "!=".into(),
                _pos: start,
            }));
        }
        if two == ">=" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: ">=".into(),
                _pos: start,
            }));
        }
        if two == "<=" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "<=".into(),
                _pos: start,
            }));
        }
        if two == "&&" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "&&".into(),
                _pos: start,
            }));
        }
        if two == "||" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Op,
                value: "||".into(),
                _pos: start,
            }));
        }
        if two == "=>" {
            self.pos += 2;
            return Ok(Some(Token {
                kind: TokenKind::Arrow,
                value: "=>".into(),
                _pos: start,
            }));
        }

        self.pos += 1;
        match ch {
            ':' => Ok(Some(Token {
                kind: TokenKind::Colon,
                value: ch.to_string(),
                _pos: start,
            })),
            '|' => Ok(Some(Token {
                kind: TokenKind::Pipe,
                value: ch.to_string(),
                _pos: start,
            })),
            '(' => Ok(Some(Token {
                kind: TokenKind::LParen,
                value: ch.to_string(),
                _pos: start,
            })),
            ')' => Ok(Some(Token {
                kind: TokenKind::RParen,
                value: ch.to_string(),
                _pos: start,
            })),
            '<' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            '>' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            '{' => Ok(Some(Token {
                kind: TokenKind::LBrace,
                value: ch.to_string(),
                _pos: start,
            })),
            '}' => Ok(Some(Token {
                kind: TokenKind::RBrace,
                value: ch.to_string(),
                _pos: start,
            })),
            '[' => Ok(Some(Token {
                kind: TokenKind::LBracket,
                value: ch.to_string(),
                _pos: start,
            })),
            ']' => Ok(Some(Token {
                kind: TokenKind::RBracket,
                value: ch.to_string(),
                _pos: start,
            })),
            ',' => Ok(Some(Token {
                kind: TokenKind::Comma,
                value: ch.to_string(),
                _pos: start,
            })),
            '.' => Ok(Some(Token {
                kind: TokenKind::Dot,
                value: ch.to_string(),
                _pos: start,
            })),
            '!' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            '+' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            '-' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            '*' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            '/' => Ok(Some(Token {
                kind: TokenKind::Op,
                value: ch.to_string(),
                _pos: start,
            })),
            _ => Err(format!("Unexpected character '{}' at {}", ch, start)),
        }
    }

    fn read_string(&mut self, quote: char) -> Result<Token, String> {
        let start = self.pos;
        self.pos += 1; // opening quote
        let mut value = String::new();
        while self.pos < self.src.len() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            if ch == quote {
                self.pos += 1;
                return Ok(Token {
                    kind: TokenKind::String,
                    value,
                    _pos: start,
                });
            }
            if ch == '\\' {
                self.pos += 1;
            }
            value.push(ch);
            self.pos += ch.len_utf8();
        }
        Err("Unterminated string".into())
    }

    fn read_number(&mut self) -> Result<Token, String> {
        let start = self.pos;
        let rest = &self.src[start..];
        if rest.len() >= 2 {
            let prefix: String = rest
                .chars()
                .take(2)
                .collect::<String>()
                .to_ascii_lowercase();
            if prefix == "0x" || prefix == "0o" || prefix == "0b" {
                self.pos += 2;
                let digits_start = self.pos;
                while self.pos < self.src.len() {
                    let ch = self.src[self.pos..].chars().next().unwrap();
                    let ok = match prefix.as_str() {
                        "0x" => ch.is_ascii_hexdigit(),
                        "0o" => ('0'..='7').contains(&ch),
                        "0b" => ch == '0' || ch == '1',
                        _ => false,
                    };
                    if ok {
                        self.pos += ch.len_utf8();
                    } else {
                        break;
                    }
                }
                if self.pos == digits_start {
                    return Err(format!(
                        "Invalid numeric literal '{}'",
                        &self.src[start..self.pos]
                    ));
                }
                return Ok(Token {
                    kind: TokenKind::Number,
                    value: self.src[start..self.pos].into(),
                    _pos: start,
                });
            }
        }
        while self.pos < self.src.len() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            if ch.is_ascii_digit() || ch == '.' {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        Ok(Token {
            kind: TokenKind::Number,
            value: self.src[start..self.pos].into(),
            _pos: start,
        })
    }

    fn read_ident(&mut self) -> Token {
        let start = self.pos;
        while self.pos < self.src.len() {
            let ch = self.src[self.pos..].chars().next().unwrap();
            if ch.is_alphanumeric() || ch == '_' || ch == '$' {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        Token {
            kind: TokenKind::Ident,
            value: self.src[start..self.pos].into(),
            _pos: start,
        }
    }
}

fn parse_predicate_number(text: &str) -> Result<f64, String> {
    let lower = text.to_ascii_lowercase();
    if let Some(digits) = lower.strip_prefix("0x") {
        i64::from_str_radix(digits, 16)
            .map(|value| value as f64)
            .map_err(|_| format!("Invalid number {text}"))
    } else if let Some(digits) = lower.strip_prefix("0o") {
        i64::from_str_radix(digits, 8)
            .map(|value| value as f64)
            .map_err(|_| format!("Invalid number {text}"))
    } else if let Some(digits) = lower.strip_prefix("0b") {
        i64::from_str_radix(digits, 2)
            .map(|value| value as f64)
            .map_err(|_| format!("Invalid number {text}"))
    } else {
        text.parse().map_err(|_| format!("Invalid number {text}"))
    }
}

struct TypeParser {
    tokens: Vec<Token>,
    pos: usize,
    predicate_params: Vec<String>,
}

impl TypeParser {
    fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            predicate_params: Vec::new(),
        }
    }

    fn parse_annotation(&mut self) -> Result<RefinementType, String> {
        self.eat_optional_type_keyword();
        if self.peek().kind == TokenKind::Ident && self.peek().value == "forall" {
            self.pos += 1;
            loop {
                let name = self.expect_ident_raw()?;
                self.predicate_params.push(name);
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.expect(TokenKind::Dot)?;
        }
        let ty = self.parse_refined_type()?;
        if !self.peek().is_eof() {
            return Err(format!(
                "Unexpected token '{}' after type",
                self.peek().value
            ));
        }
        Ok(ty)
    }

    fn parse_refined_type(&mut self) -> Result<RefinementType, String> {
        let base = if self.omitted_base_start() {
            BaseType::Omitted
        } else {
            self.parse_base_type()?
        };
        let index = if self.peek().kind == TokenKind::LBracket {
            self.pos += 1;
            let index = self.parse_predicate()?;
            self.expect(TokenKind::RBracket)?;
            Some(index)
        } else {
            None
        };
        let predicate = if self.peek().kind == TokenKind::Pipe {
            self.pos += 1;
            Some(self.parse_predicate()?)
        } else {
            None
        };
        if matches!(base, BaseType::Omitted) && index.is_none() && predicate.is_none() {
            return Err("Expected type, index, or predicate".into());
        }
        Ok(RefinementType {
            base,
            index,
            predicate,
        })
    }

    fn eat_optional_type_keyword(&mut self) {
        if self.peek().kind == TokenKind::Ident
            && self.peek().value == "type"
            && self
                .tokens
                .get(self.pos + 1)
                .is_some_and(|token| token.kind == TokenKind::Colon)
        {
            let _ = self.expect_ident("type");
            let _ = self.expect(TokenKind::Colon);
        }
    }

    fn omitted_base_start(&self) -> bool {
        matches!(
            self.peek().kind,
            TokenKind::Pipe
                | TokenKind::LBracket
                | TokenKind::Eof
                | TokenKind::RParen
                | TokenKind::Comma
                | TokenKind::Arrow
                | TokenKind::RBrace
        )
    }

    fn parse_base_type(&mut self) -> Result<BaseType, String> {
        match self.peek().kind {
            TokenKind::LParen => self.parse_function_type(),
            TokenKind::LBrace => self.parse_object_type(),
            TokenKind::Ident => {
                let mut name = self.peek().value.clone();
                self.pos += 1;
                while self.eat(TokenKind::Dot) {
                    name.push('.');
                    name.push_str(&self.expect_ident_raw()?);
                }
                match name.as_str() {
                    "number" | "string" | "boolean" | "unknown" | "any" | "void" => {
                        Ok(BaseType::Primitive(name))
                    }
                    "Array" => {
                        self.expect_op("<")?;
                        let el = self.parse_base_type()?;
                        self.expect_op(">")?;
                        Ok(BaseType::Array(Box::new(el)))
                    }
                    _ if self.peek_op() == Some("<") => {
                        self.pos += 1;
                        let mut arguments = Vec::new();
                        loop {
                            arguments.push(self.parse_base_type()?);
                            if !self.eat(TokenKind::Comma) {
                                break;
                            }
                        }
                        self.expect_op(">")?;
                        Ok(BaseType::Generic(name, arguments))
                    }
                    _ => Ok(BaseType::Named(name)),
                }
            }
            _ => Err(format!("Expected type, got '{}'", self.peek().value)),
        }
    }

    fn parse_function_type(&mut self) -> Result<BaseType, String> {
        self.expect(TokenKind::LParen)?;
        let mut params = Vec::new();
        if self.peek().kind != TokenKind::RParen {
            loop {
                let name = self.expect_ident_raw()?;
                self.expect(TokenKind::Colon)?;
                let ty = self.parse_refined_type()?;
                params.push(RefinedParam { name, ty });
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RParen)?;
        self.expect(TokenKind::Arrow)?;
        let ret = self.parse_refined_type()?;
        Ok(BaseType::Function(params, Box::new(ret)))
    }

    fn parse_object_type(&mut self) -> Result<BaseType, String> {
        self.expect(TokenKind::LBrace)?;
        let mut fields = Vec::new();
        if self.peek().kind != TokenKind::RBrace {
            loop {
                let name = self.expect_ident_raw()?;
                self.expect(TokenKind::Colon)?;
                let ty = self.parse_base_type()?;
                fields.push((name, ty));
                if !self.eat(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RBrace)?;
        Ok(BaseType::Object(fields))
    }

    fn parse_predicate(&mut self) -> Result<PredicateExpr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<PredicateExpr, String> {
        let mut left = self.parse_and()?;
        while self.peek_op() == Some("||") {
            self.pos += 1;
            let right = self.parse_and()?;
            left = PredicateExpr::Logical(LogicalOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<PredicateExpr, String> {
        let mut left = self.parse_not()?;
        while self.peek_op() == Some("&&") {
            self.pos += 1;
            let right = self.parse_not()?;
            left = PredicateExpr::Logical(LogicalOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<PredicateExpr, String> {
        if self.peek_op() == Some("!") {
            self.pos += 1;
            return Ok(PredicateExpr::Not(Box::new(self.parse_not()?)));
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> Result<PredicateExpr, String> {
        let mut left = self.parse_additive()?;
        while let Some(op) = self.peek_comparison_op() {
            self.pos += 1;
            let right = self.parse_additive()?;
            left = PredicateExpr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<PredicateExpr, String> {
        let mut left = self.parse_multiplicative()?;
        while let Some(op) = self.peek_additive_op() {
            self.pos += 1;
            let right = self.parse_multiplicative()?;
            left = PredicateExpr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<PredicateExpr, String> {
        let mut left = self.parse_unary()?;
        while let Some(op) = self.peek_multiplicative_op() {
            self.pos += 1;
            let right = self.parse_unary()?;
            left = PredicateExpr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<PredicateExpr, String> {
        if let Some(op) = self.peek_additive_op()
            && (op == BinaryOp::Add || op == BinaryOp::Sub)
        {
            self.pos += 1;
            let expr = self.parse_unary()?;
            return Ok(PredicateExpr::Binary(
                op,
                Box::new(PredicateExpr::Literal(Literal::Number(0.0))),
                Box::new(expr),
            ));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<PredicateExpr, String> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Number => {
                self.pos += 1;
                let v = parse_predicate_number(&tok.value)?;
                Ok(PredicateExpr::Literal(Literal::Number(v)))
            }
            TokenKind::String => {
                self.pos += 1;
                Ok(PredicateExpr::Literal(Literal::String(tok.value)))
            }
            TokenKind::LParen => {
                self.pos += 1;
                let expr = self.parse_predicate()?;
                self.expect(TokenKind::RParen)?;
                Ok(expr)
            }
            TokenKind::Ident => {
                self.pos += 1;
                let mut expression = match tok.value.as_str() {
                    "true" => PredicateExpr::Literal(Literal::Boolean(true)),
                    "false" => PredicateExpr::Literal(Literal::Boolean(false)),
                    "$" => PredicateExpr::Return,
                    _ => {
                        if self.peek().kind == TokenKind::LParen {
                            self.pos += 1;
                            let arg = self.parse_predicate()?;
                            self.expect(TokenKind::RParen)?;
                            PredicateExpr::PredicateApply(tok.value, Box::new(arg))
                        } else {
                            PredicateExpr::Identifier(tok.value)
                        }
                    }
                };
                while self.peek().kind == TokenKind::Dot {
                    self.pos += 1;
                    let property = self.expect_ident_raw()?;
                    expression = PredicateExpr::Member(Box::new(expression), property);
                }
                Ok(expression)
            }
            _ => Err(format!("Unexpected token '{}' in predicate", tok.value)),
        }
    }

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_op(&self) -> Option<&str> {
        let t = self.peek();
        if t.kind == TokenKind::Op {
            Some(&t.value)
        } else {
            None
        }
    }

    fn peek_comparison_op(&self) -> Option<BinaryOp> {
        self.peek_op().and_then(|v| match v {
            ">" => Some(BinaryOp::Gt),
            "<" => Some(BinaryOp::Lt),
            ">=" => Some(BinaryOp::Gte),
            "<=" => Some(BinaryOp::Lte),
            "===" => Some(BinaryOp::EqEqEq),
            "!==" => Some(BinaryOp::NotEqEq),
            "==" => Some(BinaryOp::EqEq),
            "!=" => Some(BinaryOp::NotEq),
            _ => None,
        })
    }

    fn peek_additive_op(&self) -> Option<BinaryOp> {
        self.peek_op().and_then(|v| match v {
            "+" => Some(BinaryOp::Add),
            "-" => Some(BinaryOp::Sub),
            _ => None,
        })
    }

    fn peek_multiplicative_op(&self) -> Option<BinaryOp> {
        self.peek_op().and_then(|v| match v {
            "*" => Some(BinaryOp::Mul),
            "/" => Some(BinaryOp::Div),
            _ => None,
        })
    }

    fn expect(&mut self, kind: TokenKind) -> Result<(), String> {
        if self.peek().kind != kind {
            return Err(format!(
                "Expected {:?} but got '{}'",
                kind,
                self.peek().value
            ));
        }
        self.pos += 1;
        Ok(())
    }

    fn expect_op(&mut self, value: &str) -> Result<(), String> {
        if self.peek().kind != TokenKind::Op || self.peek().value != value {
            return Err(format!(
                "Expected operator '{}' but got '{}'",
                value,
                self.peek().value
            ));
        }
        self.pos += 1;
        Ok(())
    }

    fn expect_ident(&mut self, value: &str) -> Result<(), String> {
        if self.peek().kind != TokenKind::Ident || self.peek().value != value {
            return Err(format!(
                "Expected '{}' but got '{}'",
                value,
                self.peek().value
            ));
        }
        self.pos += 1;
        Ok(())
    }

    fn expect_ident_raw(&mut self) -> Result<String, String> {
        if self.peek().kind != TokenKind::Ident {
            return Err(format!(
                "Expected identifier but got '{}'",
                self.peek().value
            ));
        }
        let v = self.peek().value.clone();
        self.pos += 1;
        Ok(v)
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.peek().kind == kind {
            self.pos += 1;
            true
        } else {
            false
        }
    }
}

impl Token {
    fn is_eof(&self) -> bool {
        self.kind == TokenKind::Eof
    }
}

fn parse_annotation_payload(text: &str) -> Result<(RefinementType, Vec<String>), String> {
    let tokens = Lexer::new(text).tokenize()?;
    let mut parser = TypeParser::new(tokens);
    let ty = parser.parse_annotation()?;
    Ok((ty, parser.predicate_params))
}

fn clean_comment(text: &str) -> String {
    let trimmed = text.trim();
    let body = if trimmed.starts_with("/*") && trimmed.ends_with("*/") {
        &trimmed[2..trimmed.len() - 2]
    } else {
        trimmed
    };
    body.lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('*') {
                trimmed
                    .trim_start_matches('*')
                    .trim_start_matches(' ')
                    .trim()
                    .to_string()
            } else {
                trimmed.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn extract_rt_payload(text: &str) -> Option<String> {
    let cleaned = clean_comment(text);
    if !cleaned.starts_with("#rt") {
        return None;
    }
    Some(cleaned[3..].trim().to_string())
}

pub struct ParseResult {
    pub annotations: Vec<Annotation>,
}

pub fn parse_file(source: &str, file_name: &str) -> Result<ParseResult, String> {
    let allocator = Allocator::default();
    let parsed = parse(&allocator, file_name, source);

    if !parsed.diagnostics.is_empty() {
        return Err(format!("Parse errors: {:?}", parsed.diagnostics));
    }

    annotations_from_program(source, file_name, &parsed.program)
}

/// Collect `/*#rt` annotations from a program already produced by `pragma_parse`.
pub fn annotations_from_program(
    source: &str,
    file_name: &str,
    program: &Program<'_>,
) -> Result<ParseResult, String> {
    let mut collector = SpanCollector::new();
    collector.visit_program(program);

    let mut annotations = Vec::new();

    for comment in &program.comments {
        if comment.position != CommentPosition::Leading {
            continue;
        }
        let payload = match extract_rt_payload(comment.span.source_text(source)) {
            Some(p) => p,
            None => continue,
        };

        // Find the nearest node that starts after this comment ends.
        let comment_end = comment.span.end;
        let mut best: Option<&SpanInfo> = None;
        for info in &collector.spans {
            if info.span.start >= comment_end
                && best.map(|b| info.span.start < b.span.start).unwrap_or(true)
            {
                best = Some(info);
            }
        }

        let Some(info) = best else { continue };
        let (line, column) = byte_offset_to_line_col(source, comment.span.start);
        let loc = SourceLocation {
            file: Some(file_name.to_string()),
            line,
            column,
        };

        let (ty, predicate_params) = parse_annotation_payload(&payload)?;
        if let Some(target) = &info.target {
            match target {
                NodeTarget::Return {
                    function_name,
                    function_start,
                } => {
                    if let BaseType::Function(params, ret) = &ty.base {
                        for (idx, p) in params.iter().enumerate() {
                            let query_offset = param_query_offset(
                                &collector.spans,
                                *function_start,
                                idx,
                            )
                            .unwrap_or(*function_start);
                            annotations.push(Annotation {
                                target: AnnotationTarget::Param {
                                    function_name: function_name.clone(),
                                    function_start: *function_start,
                                    param_name: p.name.clone(),
                                    index: idx,
                                },
                                ty: p.ty.clone(),
                                predicate_params: predicate_params.clone(),
                                loc: loc.clone(),
                                query_offset,
                            });
                        }
                        annotations.push(Annotation {
                            target: AnnotationTarget::Return {
                                function_name: function_name.clone(),
                                function_start: *function_start,
                            },
                            ty: *ret.clone(),
                            predicate_params: predicate_params.clone(),
                            loc,
                            query_offset: *function_start,
                        });
                    } else {
                        annotations.push(Annotation {
                            target: AnnotationTarget::Return {
                                function_name: function_name.clone(),
                                function_start: *function_start,
                            },
                            ty,
                            predicate_params: predicate_params.clone(),
                            loc,
                            query_offset: *function_start,
                        });
                    }
                }
                NodeTarget::Param {
                    function_name,
                    function_start,
                    param_name,
                    index,
                } => {
                    annotations.push(Annotation {
                        target: AnnotationTarget::Param {
                            function_name: function_name.clone(),
                            function_start: *function_start,
                            param_name: param_name.clone(),
                            index: *index,
                        },
                        ty,
                        predicate_params,
                        loc,
                        query_offset: info.span.start,
                    });
                }
                NodeTarget::Variable {
                    name,
                    declaration_start,
                } => {
                    annotations.push(Annotation {
                        target: AnnotationTarget::Variable {
                            name: name.clone(),
                            declaration_start: *declaration_start,
                        },
                        ty,
                        predicate_params,
                        loc,
                        query_offset: *declaration_start,
                    });
                }
            }
        }
    }

    Ok(ParseResult { annotations })
}

fn param_query_offset(spans: &[SpanInfo], function_start: u32, index: usize) -> Option<u32> {
    spans.iter().find_map(|info| match &info.target {
        Some(NodeTarget::Param {
            function_start: start,
            index: i,
            ..
        }) if *start == function_start && *i == index => Some(info.span.start),
        _ => None,
    })
}

fn refinement_omits_base(ty: &RefinementType) -> bool {
    if matches!(ty.base, BaseType::Omitted) {
        return true;
    }
    if let BaseType::Function(params, ret) = &ty.base {
        return params.iter().any(|p| refinement_omits_base(&p.ty)) || refinement_omits_base(ret);
    }
    false
}

/// Offsets that need a compiler type because a base type was omitted.
pub fn omitted_query_offsets(annotations: &[Annotation]) -> Vec<usize> {
    let mut offsets: Vec<usize> = annotations
        .iter()
        .filter(|a| refinement_omits_base(&a.ty))
        .map(|a| a.query_offset as usize)
        .collect();
    offsets.sort();
    offsets.dedup();
    offsets
}

#[derive(Debug, Clone)]
enum NodeTarget {
    Return {
        function_name: String,
        function_start: u32,
    },
    Param {
        function_name: String,
        function_start: u32,
        param_name: String,
        index: usize,
    },
    Variable {
        name: String,
        declaration_start: u32,
    },
}

#[derive(Debug, Clone)]
struct SpanInfo {
    span: Span,
    target: Option<NodeTarget>,
}

struct SpanCollector {
    spans: Vec<SpanInfo>,
}

impl SpanCollector {
    fn new() -> Self {
        Self { spans: Vec::new() }
    }

    fn add(&mut self, span: Span, target: Option<NodeTarget>) {
        self.spans.push(SpanInfo { span, target });
    }
}

impl<'a> Visit<'a> for SpanCollector {
    fn visit_program(&mut self, program: &Program<'a>) {
        for stmt in &program.body {
            self.visit_statement(stmt);
        }
    }

    fn visit_statement(&mut self, stmt: &Statement<'a>) {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                let name = func
                    .id
                    .as_ref()
                    .map(|id| id.name.to_string())
                    .unwrap_or_else(|| "<anonymous>".into());
                self.add(
                    func.span,
                    Some(NodeTarget::Return {
                        function_name: name.clone(),
                        function_start: func.span.start,
                    }),
                );
                for (idx, param) in func.params.items.iter().enumerate() {
                    let param_name = match &param.pattern {
                        BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                        BindingPattern::AssignmentPattern(ap) => match &ap.left {
                            BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                            _ => continue,
                        },
                        _ => continue,
                    };
                    self.add(
                        param.span,
                        Some(NodeTarget::Param {
                            function_name: name.clone(),
                            function_start: func.span.start,
                            param_name,
                            index: idx,
                        }),
                    );
                }
                if let Some(body) = &func.body {
                    for stmt in &body.statements {
                        self.visit_statement(stmt);
                    }
                }
            }
            Statement::VariableDeclaration(decl) => {
                for d in &decl.declarations {
                    if let Some(name) = match &d.id {
                        BindingPattern::BindingIdentifier(id) => Some(id.name.to_string()),
                        _ => None,
                    } {
                        let target = if let Some(Expression::FunctionExpression(_))
                        | Some(Expression::ArrowFunctionExpression(_)) =
                            d.init.as_ref()
                        {
                            NodeTarget::Return {
                                function_name: name.clone(),
                                function_start: d.span.start,
                            }
                        } else {
                            NodeTarget::Variable {
                                name: name.clone(),
                                declaration_start: d.span.start,
                            }
                        };
                        self.add(d.span, Some(target));
                    }
                    if let Some(init) = &d.init {
                        self.visit_expression(init);
                    }
                }
            }
            _ => walk_statement(self, stmt),
        }
    }
}
