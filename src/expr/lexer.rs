use crate::error::{CrabaseError, Result};
use crate::expr::ast::{Ident, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Number(f64),
    Str(String),
    /// A `/pattern/[flags]` regex literal. The expression language uses these
    /// in `replace()` calls (e.g. `replace(/\d+/, "")`). Flags are ignored —
    /// callers downstream only consume the pattern.
    Regex(String),
    Bool(bool),
    Null,
    Ident(Ident),
    Dot,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    EqEq,
    BangEq,
    Gt,
    Lt,
    GtEq,
    LtEq,
    AmpAmp,
    PipePipe,
    Bang,
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
    /// Tracks the most recently emitted non-whitespace token kind. Used to
    /// decide whether a `/` starts a regex literal (true after operators, `(`,
    /// `[`, `,`, or at the start of input) or is a plain division operator
    /// (true after an identifier, number, `)`, `]`).
    prev_kind: Option<TokenKind>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            pos: 0,
            prev_kind: None,
        }
    }

    /// Decide whether a `/` at the current position should be parsed as the
    /// start of a regex literal. Mirrors the lookbehind rule JavaScript uses
    /// for the same syntactic ambiguity.
    fn slash_starts_regex(&self) -> bool {
        match &self.prev_kind {
            None => true,
            Some(kind) => matches!(
                kind,
                TokenKind::LParen
                    | TokenKind::LBracket
                    | TokenKind::Comma
                    | TokenKind::Plus
                    | TokenKind::Minus
                    | TokenKind::Star
                    | TokenKind::Slash
                    | TokenKind::Percent
                    | TokenKind::EqEq
                    | TokenKind::BangEq
                    | TokenKind::Gt
                    | TokenKind::Lt
                    | TokenKind::GtEq
                    | TokenKind::LtEq
                    | TokenKind::AmpAmp
                    | TokenKind::PipePipe
                    | TokenKind::Bang
            ),
        }
    }

    fn read_regex(&mut self, start: usize) -> Result<Token> {
        let mut pattern = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(CrabaseError::ExprParse(format!(
                        "Unterminated regex literal at {start}"
                    )));
                }
                Some('\\') => {
                    pattern.push('\\');
                    if let Some(next) = self.advance() {
                        pattern.push(next);
                    }
                }
                Some('/') => {
                    // Consume optional flags (e.g. /pat/gi) and discard.
                    while let Some(c) = self.peek_char() {
                        if c.is_ascii_alphabetic() {
                            let _ = self.advance();
                        } else {
                            break;
                        }
                    }
                    return Ok(self.token(start, TokenKind::Regex(pattern)));
                }
                Some(c) => pattern.push(c),
            }
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek_char()?;
        self.pos += ch.len_utf8();
        Some(ch)
    }

    fn skip_whitespace(&mut self) {
        while self.peek_char().is_some_and(|c| c.is_whitespace()) {
            let _ = self.advance();
        }
    }

    fn token(&self, start: usize, kind: TokenKind) -> Token {
        Token {
            kind,
            span: Span {
                start,
                end: self.pos,
            },
        }
    }

    fn read_string(&mut self, quote: char, start: usize) -> Result<Token> {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(CrabaseError::ExprParse(format!(
                        "Unterminated string literal at {start}"
                    )));
                }
                Some('\\') => match self.advance() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
                    Some('\\') => s.push('\\'),
                    Some(c) if c == quote => s.push(c),
                    Some(c) => {
                        s.push('\\');
                        s.push(c);
                    }
                    None => {
                        return Err(CrabaseError::ExprParse(format!(
                            "Unterminated escape sequence at {}",
                            self.pos
                        )));
                    }
                },
                Some(c) if c == quote => return Ok(self.token(start, TokenKind::Str(s))),
                Some(c) => s.push(c),
            }
        }
    }

    fn read_number(&mut self, first: char, start: usize) -> Result<Token> {
        let mut raw = String::from(first);
        let mut seen_decimal = first == '.';

        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() {
                raw.push(c);
                let _ = self.advance();
                continue;
            }
            if c == '.' && !seen_decimal {
                seen_decimal = true;
                raw.push(c);
                let _ = self.advance();
                continue;
            }
            break;
        }

        let number = raw.parse::<f64>().map_err(|error| {
            CrabaseError::ExprParse(format!(
                "Invalid numeric literal '{raw}' at {start}: {error}"
            ))
        })?;
        Ok(self.token(start, TokenKind::Number(number)))
    }

    fn read_ident(&mut self, first: char, start: usize) -> Token {
        let mut s = String::from(first);
        while let Some(c) = self.peek_char() {
            if c.is_alphanumeric() || c == '_' {
                s.push(c);
                let _ = self.advance();
            } else {
                break;
            }
        }

        let kind = match s.as_str() {
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "null" => TokenKind::Null,
            _ => TokenKind::Ident(Ident::new(s)),
        };
        self.token(start, kind)
    }

    pub fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace();
        let start = self.pos;

        match self.peek_char() {
            None => Ok(self.token(start, TokenKind::Eof)),
            Some(c) => {
                let _ = self.advance();
                match c {
                    '.' => Ok(self.token(start, TokenKind::Dot)),
                    '(' => Ok(self.token(start, TokenKind::LParen)),
                    ')' => Ok(self.token(start, TokenKind::RParen)),
                    '[' => Ok(self.token(start, TokenKind::LBracket)),
                    ']' => Ok(self.token(start, TokenKind::RBracket)),
                    ',' => Ok(self.token(start, TokenKind::Comma)),
                    '+' => Ok(self.token(start, TokenKind::Plus)),
                    '-' => Ok(self.token(start, TokenKind::Minus)),
                    '*' => Ok(self.token(start, TokenKind::Star)),
                    '/' => {
                        if self.slash_starts_regex() {
                            self.read_regex(start)
                        } else {
                            Ok(self.token(start, TokenKind::Slash))
                        }
                    }
                    '%' => Ok(self.token(start, TokenKind::Percent)),
                    '=' => {
                        if self.peek_char() == Some('=') {
                            let _ = self.advance();
                            Ok(self.token(start, TokenKind::EqEq))
                        } else {
                            Err(CrabaseError::ExprParse(format!(
                                "Unexpected character '=' at {start}"
                            )))
                        }
                    }
                    '!' => {
                        if self.peek_char() == Some('=') {
                            let _ = self.advance();
                            Ok(self.token(start, TokenKind::BangEq))
                        } else {
                            Ok(self.token(start, TokenKind::Bang))
                        }
                    }
                    '>' => {
                        if self.peek_char() == Some('=') {
                            let _ = self.advance();
                            Ok(self.token(start, TokenKind::GtEq))
                        } else {
                            Ok(self.token(start, TokenKind::Gt))
                        }
                    }
                    '<' => {
                        if self.peek_char() == Some('=') {
                            let _ = self.advance();
                            Ok(self.token(start, TokenKind::LtEq))
                        } else {
                            Ok(self.token(start, TokenKind::Lt))
                        }
                    }
                    '&' => {
                        if self.peek_char() == Some('&') {
                            let _ = self.advance();
                            Ok(self.token(start, TokenKind::AmpAmp))
                        } else {
                            Err(CrabaseError::ExprParse(format!("Expected '&&' at {start}")))
                        }
                    }
                    '|' => {
                        if self.peek_char() == Some('|') {
                            let _ = self.advance();
                            Ok(self.token(start, TokenKind::PipePipe))
                        } else {
                            Err(CrabaseError::ExprParse(format!("Expected '||' at {start}")))
                        }
                    }
                    '\'' => self.read_string('\'', start),
                    '"' => self.read_string('"', start),
                    c if c.is_ascii_digit() => self.read_number(c, start),
                    c if c.is_alphabetic() || c == '_' => Ok(self.read_ident(c, start)),
                    other => Err(CrabaseError::ExprParse(format!(
                        "Unexpected character '{other}' at {start}"
                    ))),
                }
            }
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            let token = self.next_token()?;
            let is_eof = token.kind == TokenKind::Eof;
            self.prev_kind = Some(token.kind.clone());
            tokens.push(token);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }
}
