use crate::error::{CrabaseError, Result};
use crate::expr::ast::{BinOp, Expr, UnaryOp};
use crate::expr::lexer::{Lexer, Token};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> &Token {
        let token = self.tokens.get(self.pos).unwrap_or(&Token::Eof);
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn expect(&mut self, expected: &Token) -> Result<()> {
        let actual = self.advance();
        if actual == expected {
            Ok(())
        } else {
            Err(CrabaseError::ExprParse(format!(
                "Expected {expected:?}, got {actual:?}"
            )))
        }
    }

    pub fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_precedence(0)
    }

    fn parse_precedence(&mut self, min_binding_power: u8) -> Result<Expr> {
        let mut left = self.parse_prefix()?;

        while let Some((op, left_bp, right_bp)) = infix_binding_power(self.peek()) {
            if left_bp < min_binding_power {
                break;
            }

            self.advance();
            let right = self.parse_precedence(right_bp)?;
            left = Expr::BinOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }

        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<Expr> {
        let expr = match self.peek() {
            Token::Bang => {
                self.advance();
                Expr::UnaryOp {
                    op: UnaryOp::Not,
                    operand: Box::new(self.parse_precedence(11)?),
                }
            }
            Token::Minus => {
                self.advance();
                Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    operand: Box::new(self.parse_precedence(11)?),
                }
            }
            _ => self.parse_primary()?,
        };

        self.parse_postfix(expr)
    }

    fn parse_postfix(&mut self, expr: Expr) -> Result<Expr> {
        let mut current = expr;

        loop {
            current = match self.peek() {
                Token::LParen => {
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(&Token::RParen)?;
                    Expr::Call {
                        callee: Box::new(current),
                        args,
                    }
                }
                Token::Dot => {
                    self.advance();
                    let field = self.parse_identifier("Expected identifier after '.'")?;
                    Expr::Member {
                        object: Box::new(current),
                        field,
                    }
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    Expr::Index {
                        object: Box::new(current),
                        index: Box::new(index),
                    }
                }
                _ => return Ok(current),
            };
        }
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().clone() {
            Token::Number(number) => {
                self.advance();
                Ok(Expr::Number(number))
            }
            Token::Str(text) => {
                self.advance();
                Ok(Expr::Str(text))
            }
            Token::Bool(value) => {
                self.advance();
                Ok(Expr::Bool(value))
            }
            Token::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            Token::Ident(name) => {
                self.advance();
                Ok(Expr::Ident(name))
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&Token::RParen)?;
                Ok(expr)
            }
            other => Err(CrabaseError::ExprParse(format!(
                "Unexpected token in primary: {other:?}"
            ))),
        }
    }

    fn parse_identifier(&mut self, message: &str) -> Result<String> {
        match self.advance() {
            Token::Ident(name) => Ok(name.clone()),
            other => Err(CrabaseError::ExprParse(format!("{message}, got {other:?}"))),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>> {
        if self.peek() == &Token::RParen {
            return Ok(Vec::new());
        }

        let mut args = vec![self.parse_expr()?];
        while self.peek() == &Token::Comma {
            self.advance();
            args.push(self.parse_expr()?);
        }
        Ok(args)
    }
}

fn infix_binding_power(token: &Token) -> Option<(BinOp, u8, u8)> {
    match token {
        Token::PipePipe => Some((BinOp::Or, 1, 2)),
        Token::AmpAmp => Some((BinOp::And, 3, 4)),
        Token::EqEq => Some((BinOp::Eq, 5, 6)),
        Token::BangEq => Some((BinOp::Ne, 5, 6)),
        Token::Gt => Some((BinOp::Gt, 5, 6)),
        Token::Lt => Some((BinOp::Lt, 5, 6)),
        Token::GtEq => Some((BinOp::Ge, 5, 6)),
        Token::LtEq => Some((BinOp::Le, 5, 6)),
        Token::Plus => Some((BinOp::Add, 7, 8)),
        Token::Minus => Some((BinOp::Sub, 7, 8)),
        Token::Star => Some((BinOp::Mul, 9, 10)),
        Token::Slash => Some((BinOp::Div, 9, 10)),
        Token::Percent => Some((BinOp::Mod, 9, 10)),
        _ => None,
    }
}

/// Parse an expression string into an AST
pub fn parse(input: &str) -> Result<Expr> {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;

    match parser.peek() {
        Token::Eof => Ok(expr),
        other => Err(CrabaseError::ExprParse(format!(
            "Unexpected trailing token: {other:?}"
        ))),
    }
}
