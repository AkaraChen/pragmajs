//! Parser for `/*#own ... */` ownership annotations.

use crate::{Diagnostic, RuleKind};

/// How a value may be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnType {
    Unique(String),
    Affine(String),
    Copy(String),
    RefRead(String),
    RefWrite(String),
    Void,
}

impl OwnType {
    pub fn type_name(&self) -> &str {
        match self {
            OwnType::Unique(s)
            | OwnType::Affine(s)
            | OwnType::Copy(s)
            | OwnType::RefRead(s)
            | OwnType::RefWrite(s) => s,
            OwnType::Void => "void",
        }
    }

    pub fn is_linear(&self) -> bool {
        matches!(self, OwnType::Unique(_) | OwnType::Affine(_))
    }

    pub fn is_unique(&self) -> bool {
        matches!(self, OwnType::Unique(_))
    }

    pub fn is_copy(&self) -> bool {
        matches!(self, OwnType::Copy(_))
    }

    pub fn is_ref(&self) -> bool {
        matches!(self, OwnType::RefRead(_) | OwnType::RefWrite(_))
    }

    pub fn is_mut_ref(&self) -> bool {
        matches!(self, OwnType::RefWrite(_))
    }

    pub fn is_read_ref(&self) -> bool {
        matches!(self, OwnType::RefRead(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowMode {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnSig {
    pub params: Vec<(String, OwnType)>,
    pub ret: OwnType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnDirective {
    Type(FnSig),
    Let { name: String, ty: OwnType },
    Borrow {
        owner: String,
        alias: String,
        mode: BorrowMode,
        ty_name: String,
    },
    Clone { owner: String, alias: String },
    Drop { name: String },
    Shorthand(BorrowMode),
}

#[derive(Debug, Clone)]
pub struct AttachedOwn {
    #[allow(dead_code)]
    pub attached_to: u32,
    #[allow(dead_code)]
    pub span_start: u32,
    #[allow(dead_code)]
    pub span_end: u32,
    pub directives: Vec<OwnDirective>,
}

pub fn parse_own_comment(
    path: &str,
    attached_to: u32,
    span_start: u32,
    span_end: u32,
    content: &str,
    diags: &mut Vec<Diagnostic>,
) -> Option<AttachedOwn> {
    let body = strip_own_prefix(content)?;
    match parse_directives(&body) {
        Ok(directives) if !directives.is_empty() => Some(AttachedOwn {
            attached_to,
            span_start,
            span_end,
            directives,
        }),
        Ok(_) => None,
        Err(msg) => {
            diags.push(Diagnostic {
                path: path.to_string(),
                offset: span_start,
                kind: RuleKind::AnnotParseError,
                message: format!("invalid /*#own comment: {msg}"),
            });
            None
        }
    }
}

fn strip_own_prefix(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("#own")?;
    let mut out = String::new();
    for line in rest.lines() {
        let line = line.trim();
        let line = line.strip_prefix('*').map(str::trim_start).unwrap_or(line);
        if line.is_empty() {
            out.push('\n');
        } else {
            if !out.is_empty() && !out.ends_with('\n') && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    Some(out)
}

struct Lexer<'a> {
    src: &'a str,
    i: usize,
    peeked: Option<Tok>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Colon,
    Comma,
    LParen,
    RParen,
    Arrow,
    Amp,
    Bang,
    Semi,
    Eof,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            i: 0,
            peeked: None,
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.i..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek_char()?;
        self.i += c.len_utf8();
        Some(c)
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn peek_tok(&mut self) -> Result<Tok, String> {
        if self.peeked.is_none() {
            let t = self.next_tok_raw()?;
            self.peeked = Some(t);
        }
        Ok(self.peeked.clone().unwrap())
    }

    fn next_tok(&mut self) -> Result<Tok, String> {
        if let Some(t) = self.peeked.take() {
            return Ok(t);
        }
        self.next_tok_raw()
    }

    fn next_tok_raw(&mut self) -> Result<Tok, String> {
        self.skip_ws();
        let Some(c) = self.peek_char() else {
            return Ok(Tok::Eof);
        };
        match c {
            ':' => {
                self.bump();
                Ok(Tok::Colon)
            }
            ',' => {
                self.bump();
                Ok(Tok::Comma)
            }
            '(' => {
                self.bump();
                Ok(Tok::LParen)
            }
            ')' => {
                self.bump();
                Ok(Tok::RParen)
            }
            '&' => {
                self.bump();
                Ok(Tok::Amp)
            }
            '!' => {
                self.bump();
                Ok(Tok::Bang)
            }
            ';' => {
                self.bump();
                Ok(Tok::Semi)
            }
            '=' => {
                self.bump();
                if self.peek_char() == Some('>') {
                    self.bump();
                    Ok(Tok::Arrow)
                } else {
                    Err("expected '=>'".into())
                }
            }
            c if is_ident_start(c) => {
                let mut s = String::new();
                while let Some(c) = self.peek_char() {
                    if is_ident_continue(c) {
                        s.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                Ok(Tok::Ident(s))
            }
            other => Err(format!("unexpected character `{other}`")),
        }
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn parse_directives(src: &str) -> Result<Vec<OwnDirective>, String> {
    let mut lx = Lexer::new(src);
    let mut dirs = Vec::new();
    loop {
        match lx.next_tok()? {
            Tok::Eof => break,
            Tok::Semi => continue,
            Tok::Amp => {
                let mode = parse_borrow_mode_after_amp(&mut lx)?;
                dirs.push(OwnDirective::Shorthand(mode));
            }
            Tok::Ident(w) => match w.as_str() {
                "type" => {
                    expect(&mut lx, Tok::Colon, ":")?;
                    dirs.push(OwnDirective::Type(parse_fn_sig(&mut lx)?));
                }
                "let" => {
                    let name = expect_ident(&mut lx)?;
                    expect(&mut lx, Tok::Colon, ":")?;
                    let ty = parse_own_type(&mut lx)?;
                    dirs.push(OwnDirective::Let { name, ty });
                }
                "borrow" => {
                    dirs.push(parse_borrow_directive(&mut lx)?);
                }
                "clone" => {
                    let owner = expect_ident(&mut lx)?;
                    expect_ident_is(&mut lx, "as")?;
                    let alias = expect_ident(&mut lx)?;
                    dirs.push(OwnDirective::Clone { owner, alias });
                }
                "drop" => {
                    let name = expect_ident(&mut lx)?;
                    dirs.push(OwnDirective::Drop { name });
                }
                other => return Err(format!("unknown directive `{other}`")),
            },
            other => return Err(format!("unexpected token {other:?}")),
        }
    }
    Ok(dirs)
}

fn parse_borrow_directive(lx: &mut Lexer<'_>) -> Result<OwnDirective, String> {
    let bang = matches!(lx.peek_tok()?, Tok::Bang);
    if bang {
        let _ = lx.next_tok()?;
    }
    let owner = expect_ident(lx)?;
    expect_ident_is(lx, "as")?;
    let alias = expect_ident(lx)?;
    expect(lx, Tok::Colon, ":")?;
    let ty = parse_own_type(lx)?;
    let (mode, ty_name) = match &ty {
        OwnType::RefRead(n) => (BorrowMode::Read, n.clone()),
        OwnType::RefWrite(n) => (BorrowMode::Write, n.clone()),
        OwnType::Unique(n) | OwnType::Affine(n) | OwnType::Copy(n) => {
            (
                if bang {
                    BorrowMode::Write
                } else {
                    BorrowMode::Read
                },
                n.clone(),
            )
        }
        OwnType::Void => return Err("borrow type cannot be void".into()),
    };
    Ok(OwnDirective::Borrow {
        owner,
        alias,
        mode: if bang { BorrowMode::Write } else { mode },
        ty_name,
    })
}

fn parse_fn_sig(lx: &mut Lexer<'_>) -> Result<FnSig, String> {
    expect(lx, Tok::LParen, "(")?;
    let mut params = Vec::new();
    loop {
        match lx.next_tok()? {
            Tok::RParen => break,
            Tok::Ident(name) => {
                expect(lx, Tok::Colon, ":")?;
                let ty = parse_own_type(lx)?;
                params.push((name, ty));
                match lx.next_tok()? {
                    Tok::Comma => continue,
                    Tok::RParen => break,
                    other => return Err(format!("expected `,` or `)` in param list, got {other:?}")),
                }
            }
            Tok::Comma => continue,
            other => return Err(format!("expected parameter name, got {other:?}")),
        }
    }
    expect(lx, Tok::Arrow, "=>")?;
    let ret = parse_own_type(lx)?;
    Ok(FnSig { params, ret })
}

fn parse_own_type(lx: &mut Lexer<'_>) -> Result<OwnType, String> {
    match lx.next_tok()? {
        Tok::Amp => {
            let mode = parse_borrow_mode_after_amp(lx)?;
            let name = expect_ident(lx)?;
            Ok(match mode {
                BorrowMode::Read => OwnType::RefRead(name),
                BorrowMode::Write => OwnType::RefWrite(name),
            })
        }
        Tok::Ident(w) => match w.as_str() {
            "unique" => Ok(OwnType::Unique(expect_ident(lx)?)),
            "affine" => Ok(OwnType::Affine(expect_ident(lx)?)),
            "copy" => Ok(OwnType::Copy(expect_ident(lx)?)),
            "void" | "Unit" | "unit" => Ok(OwnType::Void),
            other => {
                // Bare type name defaults to unique (Austral Linear-ish).
                Ok(OwnType::Unique(other.to_string()))
            }
        },
        other => Err(format!("expected type, got {other:?}")),
    }
}

fn parse_borrow_mode_after_amp(lx: &mut Lexer<'_>) -> Result<BorrowMode, String> {
    match lx.next_tok()? {
        Tok::Ident(s) if s == "readonly" || s == "read" => Ok(BorrowMode::Read),
        Tok::Ident(s) if s == "mut" => Ok(BorrowMode::Write),
        other => Err(format!(
            "expected `&readonly` or `&mut`, got &{other:?}"
        )),
    }
}

fn expect(lx: &mut Lexer<'_>, want: Tok, label: &str) -> Result<(), String> {
    let got = lx.next_tok()?;
    if std::mem::discriminant(&got) == std::mem::discriminant(&want) {
        Ok(())
    } else {
        Err(format!("expected `{label}`, got {got:?}"))
    }
}

fn expect_ident(lx: &mut Lexer<'_>) -> Result<String, String> {
    match lx.next_tok()? {
        Tok::Ident(s) => Ok(s),
        other => Err(format!("expected identifier, got {other:?}")),
    }
}

fn expect_ident_is(lx: &mut Lexer<'_>, word: &str) -> Result<(), String> {
    match lx.next_tok()? {
        Tok::Ident(s) if s == word => Ok(()),
        other => Err(format!("expected `{word}`, got {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_function_type() {
        let mut diags = Vec::new();
        let a = parse_own_comment(
            "t.js",
            0,
            0,
            10,
            "#own\n * type: (buf: unique Buffer) => void\n",
            &mut diags,
        )
        .unwrap();
        assert!(diags.is_empty());
        match &a.directives[0] {
            OwnDirective::Type(sig) => {
                assert_eq!(sig.params[0].0, "buf");
                assert!(matches!(sig.params[0].1, OwnType::Unique(_)));
                assert_eq!(sig.ret, OwnType::Void);
            }
            _ => panic!("expected type"),
        }
    }

    #[test]
    fn parse_shorthand_and_borrow() {
        let mut d = Vec::new();
        let a = parse_own_comment("t.js", 0, 0, 10, "#own &readonly", &mut d).unwrap();
        assert!(matches!(
            a.directives[0],
            OwnDirective::Shorthand(BorrowMode::Read)
        ));
        let a = parse_own_comment(
            "t.js",
            0,
            0,
            10,
            "#own borrow buf as view: &readonly Buffer",
            &mut d,
        )
        .unwrap();
        match &a.directives[0] {
            OwnDirective::Borrow { owner, alias, mode, .. } => {
                assert_eq!(owner, "buf");
                assert_eq!(alias, "view");
                assert_eq!(*mode, BorrowMode::Read);
            }
            _ => panic!(),
        }
    }
}
