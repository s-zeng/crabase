use crate::error::{CrabaseError, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    Str(String),
    Bool(bool),
    Null,
    // Identifiers
    Ident(String),
    // Punctuation
    Dot,
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
    // Arithmetic operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    // Comparison operators
    EqEq,
    BangEq,
    Gt,
    Lt,
    GtEq,
    LtEq,
    // Boolean operators
    AmpAmp,
    PipePipe,
    Bang,
    // End of input
    Eof,
}

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Self {
        Lexer { input, pos: 0 }
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
            self.advance();
        }
    }

    fn read_string(&mut self, quote: char) -> Result<Token> {
        let mut s = String::new();
        loop {
            match self.advance() {
                None => {
                    return Err(CrabaseError::ExprParse(
                        "Unterminated string literal".to_string(),
                    ));
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
                        return Err(CrabaseError::ExprParse(
                            "Unterminated escape sequence".to_string(),
                        ));
                    }
                },
                Some(c) if c == quote => break,
                Some(c) => s.push(c),
            }
        }
        Ok(Token::Str(s))
    }

    fn read_number(&mut self, first: char) -> Token {
        let mut s = String::new();
        s.push(first);
        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() || c == '.' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        let n: f64 = s.parse().unwrap_or(0.0);
        Token::Number(n)
    }

    fn read_ident(&mut self, first: char) -> Token {
        let mut s = String::new();
        s.push(first);
        while let Some(c) = self.peek_char() {
            if c.is_alphanumeric() || c == '_' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        match s.as_str() {
            "true" => Token::Bool(true),
            "false" => Token::Bool(false),
            "null" => Token::Null,
            _ => Token::Ident(s),
        }
    }

    pub fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace();
        match self.peek_char() {
            None => Ok(Token::Eof),
            Some(c) => {
                self.advance();
                match c {
                    '.' => Ok(Token::Dot),
                    '(' => Ok(Token::LParen),
                    ')' => Ok(Token::RParen),
                    '[' => Ok(Token::LBracket),
                    ']' => Ok(Token::RBracket),
                    ',' => Ok(Token::Comma),
                    '+' => Ok(Token::Plus),
                    '-' => Ok(Token::Minus),
                    '*' => Ok(Token::Star),
                    '/' => Ok(Token::Slash),
                    '%' => Ok(Token::Percent),
                    '=' => {
                        if self.peek_char() == Some('=') {
                            self.advance();
                            Ok(Token::EqEq)
                        } else {
                            Err(CrabaseError::ExprParse(format!(
                                "Unexpected character '=' at pos {}",
                                self.pos
                            )))
                        }
                    }
                    '!' => {
                        if self.peek_char() == Some('=') {
                            self.advance();
                            Ok(Token::BangEq)
                        } else {
                            Ok(Token::Bang)
                        }
                    }
                    '>' => {
                        if self.peek_char() == Some('=') {
                            self.advance();
                            Ok(Token::GtEq)
                        } else {
                            Ok(Token::Gt)
                        }
                    }
                    '<' => {
                        if self.peek_char() == Some('=') {
                            self.advance();
                            Ok(Token::LtEq)
                        } else {
                            Ok(Token::Lt)
                        }
                    }
                    '&' => {
                        if self.peek_char() == Some('&') {
                            self.advance();
                            Ok(Token::AmpAmp)
                        } else {
                            Err(CrabaseError::ExprParse(format!(
                                "Expected '&&' at pos {}",
                                self.pos
                            )))
                        }
                    }
                    '|' => {
                        if self.peek_char() == Some('|') {
                            self.advance();
                            Ok(Token::PipePipe)
                        } else {
                            Err(CrabaseError::ExprParse(format!(
                                "Expected '||' at pos {}",
                                self.pos
                            )))
                        }
                    }
                    '\'' => self.read_string('\''),
                    '"' => self.read_string('"'),
                    c if c.is_ascii_digit() => Ok(self.read_number(c)),
                    c if c.is_alphabetic() || c == '_' => Ok(self.read_ident(c)),
                    other => Err(CrabaseError::ExprParse(format!(
                        "Unexpected character '{}' at pos {}",
                        other, self.pos
                    ))),
                }
            }
        }
    }

    /// Tokenize the entire input into a Vec<Token>
    pub fn tokenize(&mut self) -> Result<Vec<Token>> {
        let mut tokens = Vec::new();
        loop {
            let tok = self.next_token()?;
            let is_eof = tok == Token::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }
}
