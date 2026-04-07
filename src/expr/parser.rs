use crate::error::{CrabaseError, Result};
use crate::expr::ast::{BinOp, Expr, ExprKind, Ident, Literal, Span, UnaryOp};
use crate::expr::lexer::{Lexer, Token, TokenKind};

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> &Token {
        let last_index = self.tokens.len().saturating_sub(1);
        &self.tokens[self.pos.min(last_index)]
    }

    fn advance(&mut self) -> Token {
        let token = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        token
    }

    fn matches(&self, kind: &TokenKind) -> bool {
        &self.peek().kind == kind
    }

    fn expect(&mut self, expected: TokenKind) -> Result<Token> {
        let token = self.advance();
        if token.kind == expected {
            Ok(token)
        } else {
            Err(CrabaseError::ExprParse(format!(
                "Expected {:?} at {}, got {:?}",
                expected, token.span.start, token.kind
            )))
        }
    }

    pub fn parse_expr(&mut self) -> Result<Expr> {
        self.parse_precedence(0)
    }

    fn parse_precedence(&mut self, min_binding_power: u8) -> Result<Expr> {
        let mut left = self.parse_prefix()?;

        while let Some((op, left_bp, right_bp)) = infix_binding_power(&self.peek().kind) {
            if left_bp < min_binding_power {
                break;
            }

            let _operator = self.advance();
            let right = self.parse_precedence(right_bp)?;
            let span = left.span.merge(right.span);
            left = Expr::new(
                ExprKind::Binary {
                    op,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                span,
            );
        }

        Ok(left)
    }

    fn parse_prefix(&mut self) -> Result<Expr> {
        let expr = match self.peek().kind.clone() {
            TokenKind::Bang => {
                let operator = self.advance();
                let operand = self.parse_precedence(11)?;
                let span = operator.span.merge(operand.span);
                Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(operand),
                    },
                    span,
                )
            }
            TokenKind::Minus => {
                let operator = self.advance();
                let operand = self.parse_precedence(11)?;
                let span = operator.span.merge(operand.span);
                Expr::new(
                    ExprKind::Unary {
                        op: UnaryOp::Neg,
                        operand: Box::new(operand),
                    },
                    span,
                )
            }
            _ => self.parse_primary()?,
        };

        self.parse_postfix(expr)
    }

    fn parse_postfix(&mut self, expr: Expr) -> Result<Expr> {
        let mut current = expr;

        loop {
            current = match self.peek().kind.clone() {
                TokenKind::LParen => {
                    let open = self.advance();
                    let args = self.parse_args()?;
                    let close = self.expect(TokenKind::RParen)?;
                    let span = current.span.merge(open.span).merge(close.span);
                    Expr::new(
                        ExprKind::Call {
                            callee: Box::new(current),
                            args,
                        },
                        span,
                    )
                }
                TokenKind::Dot => {
                    let _dot = self.advance();
                    let field = self.parse_identifier("Expected identifier after '.'")?;
                    let span = current.span.merge(field.1);
                    Expr::new(
                        ExprKind::Member {
                            object: Box::new(current),
                            field: field.0,
                        },
                        span,
                    )
                }
                TokenKind::LBracket => {
                    let open = self.advance();
                    let index = self.parse_expr()?;
                    let close = self.expect(TokenKind::RBracket)?;
                    let span = current
                        .span
                        .merge(open.span)
                        .merge(index.span)
                        .merge(close.span);
                    Expr::new(
                        ExprKind::Index {
                            object: Box::new(current),
                            index: Box::new(index),
                        },
                        span,
                    )
                }
                _ => return Ok(current),
            };
        }
    }

    fn parse_primary(&mut self) -> Result<Expr> {
        match self.peek().kind.clone() {
            TokenKind::Number(number) => {
                let token = self.advance();
                Ok(Expr::new(
                    ExprKind::Literal(Literal::Number(number)),
                    token.span,
                ))
            }
            TokenKind::Str(text) => {
                let token = self.advance();
                Ok(Expr::new(ExprKind::Literal(Literal::Str(text)), token.span))
            }
            TokenKind::Bool(value) => {
                let token = self.advance();
                Ok(Expr::new(
                    ExprKind::Literal(Literal::Bool(value)),
                    token.span,
                ))
            }
            TokenKind::Null => {
                let token = self.advance();
                Ok(Expr::new(ExprKind::Literal(Literal::Null), token.span))
            }
            TokenKind::Ident(name) => {
                let token = self.advance();
                Ok(Expr::new(ExprKind::Variable(name), token.span))
            }
            TokenKind::LParen => {
                let open = self.advance();
                let expr = self.parse_expr()?;
                let close = self.expect(TokenKind::RParen)?;
                Ok(Expr::new(
                    expr.kind,
                    open.span.merge(expr.span).merge(close.span),
                ))
            }
            TokenKind::LBracket => {
                let open = self.advance();
                if self.matches(&TokenKind::RBracket) {
                    let close = self.advance();
                    return Ok(Expr::new(
                        ExprKind::Array(Vec::new()),
                        open.span.merge(close.span),
                    ));
                }

                let mut items = vec![self.parse_expr()?];
                while self.matches(&TokenKind::Comma) {
                    let _ = self.advance();
                    items.push(self.parse_expr()?);
                }
                let close = self.expect(TokenKind::RBracket)?;
                let items_span = items
                    .iter()
                    .fold(open.span, |span, item| span.merge(item.span))
                    .merge(close.span);
                Ok(Expr::new(ExprKind::Array(items), items_span))
            }
            other => Err(CrabaseError::ExprParse(format!(
                "Unexpected token {:?} at {}",
                other,
                self.peek().span.start
            ))),
        }
    }

    fn parse_identifier(&mut self, message: &str) -> Result<(Ident, Span)> {
        let token = self.advance();
        match token.kind {
            TokenKind::Ident(name) => Ok((name, token.span)),
            other => Err(CrabaseError::ExprParse(format!(
                "{message} at {}, got {:?}",
                token.span.start, other
            ))),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<Expr>> {
        if self.matches(&TokenKind::RParen) {
            return Ok(Vec::new());
        }

        let mut args = vec![self.parse_expr()?];
        while self.matches(&TokenKind::Comma) {
            let _ = self.advance();
            args.push(self.parse_expr()?);
        }
        Ok(args)
    }
}

fn infix_binding_power(token: &TokenKind) -> Option<(BinOp, u8, u8)> {
    match token {
        TokenKind::PipePipe => Some((BinOp::Or, 1, 2)),
        TokenKind::AmpAmp => Some((BinOp::And, 3, 4)),
        TokenKind::EqEq => Some((BinOp::Eq, 5, 6)),
        TokenKind::BangEq => Some((BinOp::Ne, 5, 6)),
        TokenKind::Gt => Some((BinOp::Gt, 5, 6)),
        TokenKind::Lt => Some((BinOp::Lt, 5, 6)),
        TokenKind::GtEq => Some((BinOp::Ge, 5, 6)),
        TokenKind::LtEq => Some((BinOp::Le, 5, 6)),
        TokenKind::Plus => Some((BinOp::Add, 7, 8)),
        TokenKind::Minus => Some((BinOp::Sub, 7, 8)),
        TokenKind::Star => Some((BinOp::Mul, 9, 10)),
        TokenKind::Slash => Some((BinOp::Div, 9, 10)),
        TokenKind::Percent => Some((BinOp::Mod, 9, 10)),
        _ => None,
    }
}

pub fn parse(input: &str) -> Result<Expr> {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let expr = parser.parse_expr()?;

    match &parser.peek().kind {
        TokenKind::Eof => Ok(expr),
        other => Err(CrabaseError::ExprParse(format!(
            "Unexpected trailing token {:?} at {}",
            other,
            parser.peek().span.start
        ))),
    }
}
