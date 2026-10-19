use crate::lexer::{Span, Token, TokenStream, TokenType};

macro_rules! expect_token {
    ($parser: expr, $pattern: pat => $out: expr) => {{
        let token = $parser.token_stream.peek();
        match token.variant {
            $pattern => {
                $parser.token_stream.current_pos = token.span.end;
                Ok(($out, token.span))
            },
            _ => Err(ParseError::unexpected_token(
                vec![stringify!($pattern).to_string()],
                Some(token.clone()),
                token.span,
            )),
        }
    }};

    ($parser: expr, $pattern: pat) => {{
        let token = $parser.token_stream.peek();
        if matches!(token.variant, $pattern) {
            $parser.token_stream.current_pos = token.span.end;
            Ok(token)
        } else {
            Err(ParseError::unexpected_token(
                vec![stringify!($pattern).to_string()],
                Some(token.clone()),
                token.span,
            ))
        }
    }};
}

#[derive(Debug)]
pub enum ParseErrorType {
    UnexpectedToken {
        expected: Vec<String>, // or
        got: Option<Token>,
    },
    InvalidAssignmentTarget {
        target: Expression
    },
}

#[derive(Debug)]
pub struct ParseError {
    pub variant: ParseErrorType,
    pub span: Span,
}

impl ParseError {
    fn unexpected_token(
        expected: Vec<String>,
        got: Option<Token>,
        span: Span
    ) -> Self {
        return ParseError {
            variant: ParseErrorType::UnexpectedToken { expected, got },
            span
        }
    }

    fn invalid_assignment_target(target: Expression) -> Self {
        let span = target.span;
        return ParseError {
            variant: ParseErrorType::InvalidAssignmentTarget { target },
            span,
        }
    }
}

pub struct Parser<'src> {
    token_stream: TokenStream<'src>
}

#[derive(Debug, Clone)]
pub enum UnaryOperator {
    Neg,
    Not,
}

#[derive(Debug, Clone)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    NotEq,
}

#[derive(Debug, Clone)]
pub enum ExpressionType {
    Ident(String),
    String(String),
    Unary {
        operator: UnaryOperator,
        target: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>
    },
    Assignment {
        target: Box<Expression>,
        value: Box<Expression>
    },
    FunctionCall {
        target: Box<Expression>,
        args: Vec<Expression>
    },
}

#[derive(Debug, Clone)]
pub struct Expression {
    pub variant: ExpressionType,
    pub span: Span,
}

#[derive(Debug)]
pub enum TypeExpressionType {
    String,
}

#[derive(Debug)]
pub struct TypeExpression {
    pub variant: TypeExpressionType,
    pub span: Span
}

#[derive(Debug)]
pub struct Identifier {
    pub str: String,
    pub span: Span,
}

#[derive(Debug)]
pub struct Parameters {
    pub params: Vec<(Identifier, TypeExpression)>,
    pub span: Span,
}

// examples:
// ident: use kotlin
// filepath: use "otherscript"
#[derive(Debug)]
pub enum UseTarget {
    Ident(Identifier),
    FilePath { path: String, span: Span },
}

#[derive(Debug)]
pub enum StatementType {
    Use(UseTarget),
    Expression(Expression),
    Function {
        params: Parameters,
    },
}

#[derive(Debug)]
pub struct Statement {
    pub variant: StatementType,
    pub span: Span
}

impl Statement {
    fn new(variant: StatementType, span: Span) -> Self {
        return Statement { variant, span }
    }
}

#[derive(Debug)]
pub struct Script {
    pub statements: Vec<Statement>
}

const PREFIX_PRECEDENCE: u8 = 9;  // -x, !x
const CALL_PRECEDENCE: u8 = 11;   // f(x)

const fn infix_precedence(variant: &TokenType) -> Option<(u8, u8)> {
    return Some(match variant {
        TokenType::Eq => (2, 1),
        TokenType::EqEq | TokenType::BangEq => (3, 4),
        TokenType::Plus | TokenType::Minus => (5, 6),
        TokenType::Star | TokenType::Slash => (7, 8),
        _ => return None,
    })
}

impl<'src> Parser<'src> {
    pub fn new(input: &'src str) -> Self {
        return Parser {
            token_stream: TokenStream::new(input)
        }
    }

    pub fn parse_script(&mut self) -> Result<Script, ParseError> {
        let mut statements = vec![];
        while self.token_stream.peek().variant != TokenType::EOF {
            statements.push(self.parse_statement()?);
        }

        return Ok(Script {
            statements
        })
    }

    // statement parsers
    fn parse_statement(&mut self) -> Result<Statement, ParseError> {
        match self.token_stream.peek().variant {
            TokenType::Use => return self.parse_use_statement(),
            _ => {
                let expression = self.parse_expression(0)?;
                let span = expression.span;
                return Ok(Statement::new(StatementType::Expression(expression), span))
            }
        }
    }

    fn parse_use_statement(&mut self) -> Result<Statement, ParseError> {
        let use_token = expect_token!(self, TokenType::Use)?;

        let token = self.token_stream.next();
        let target = match token.variant {
            TokenType::Ident(str) => UseTarget::Ident(Identifier { str, span: token.span }),
            TokenType::String(path) => UseTarget::FilePath { path, span: token.span },
            other => return Err(ParseError::unexpected_token(
                vec!["Ident".to_string(), "String".to_string()],
                Some(Token::new(other, token.span)),
                token.span,
            )),
        };

        return Ok(Statement::new(
            StatementType::Use(target),
            use_token.span.till(token.span)
        ))
    }

    fn parse_expression(&mut self, min_precedence: u8) -> Result<Expression, ParseError> {
        let mut left_expression = self.parse_prefix_expression()?;

        loop {
            let next_token = self.token_stream.peek();

            if next_token.variant == TokenType::LeftParen {
                if min_precedence > CALL_PRECEDENCE {
                    break;
                }

                left_expression = self.parse_function_call(left_expression)?;
                continue;
            }

            let Some((left_precedence, right_precedence)) = infix_precedence(&next_token.variant) else {
                break;
            };

            if left_precedence < min_precedence {
                break;
            }

            self.token_stream.eat_next();
            let right_expression = self.parse_expression(right_precedence)?;

            left_expression = Self::build_infix_expression(next_token, left_expression, right_expression)?;
        }

        return Ok(left_expression)
    }

    fn parse_prefix_expression(&mut self) -> Result<Expression, ParseError> {
        let token = self.token_stream.next();
        let span = token.span;

        match token.variant {
            TokenType::Ident(str) => return Ok(Expression {
                variant: ExpressionType::Ident(str),
                span
            }),
            TokenType::String(str) => return Ok(Expression {
                variant: ExpressionType::String(str),
                span
            }),
            TokenType::Minus => return self.parse_unary_expression(UnaryOperator::Neg, span),
            TokenType::Bang => return self.parse_unary_expression(UnaryOperator::Not, span),
            TokenType::LeftParen => {
                let inner = self.parse_expression(0)?;
                let close = expect_token!(self, TokenType::RightParen)?;
                return Ok(Expression {
                    variant: inner.variant,
                    span: span.till(close.span)
                })
            },
            other => return Err(ParseError::unexpected_token(
                vec!["Expression".to_string()],
                Some(Token::new(other, span)),
                span,
            )),
        }
    }

    fn parse_unary_expression(&mut self, operator: UnaryOperator, operator_span: Span) -> Result<Expression, ParseError> {
        let target = self.parse_expression(PREFIX_PRECEDENCE)?;
        let span = operator_span.till(target.span);

        return Ok(Expression {
            variant: ExpressionType::Unary {
                operator,
                target: Box::new(target)
            },
            span
        })
    }

    fn parse_function_call(&mut self, target: Expression) -> Result<Expression, ParseError> {
        expect_token!(self, TokenType::LeftParen)?;

        let mut args = vec![];
        if self.token_stream.peek().variant != TokenType::RightParen {
            loop {
                args.push(self.parse_expression(0)?);
                if self.token_stream.peek().variant != TokenType::Comma {
                    break;
                }

                self.token_stream.eat_next();
            }
        }

        let close = expect_token!(self, TokenType::RightParen)?;
        let span = target.span.till(close.span);

        return Ok(Expression {
            variant: ExpressionType::FunctionCall {
                target: Box::new(target),
                args
            },
            span
        })
    }

    fn build_infix_expression(
        operator_token: Token,
        left: Expression,
        right: Expression
    ) -> Result<Expression, ParseError> {
        let span = left.span.till(right.span);

        let operator = match operator_token.variant {
            TokenType::Eq => return Ok(Expression {
                variant: ExpressionType::Assignment {
                    target: Box::new(Self::try_into_assignment_target(left)?),
                    value: Box::new(right)
                },
                span
            }),
            TokenType::EqEq => BinaryOperator::Eq,
            TokenType::BangEq => BinaryOperator::NotEq,
            TokenType::Plus => BinaryOperator::Add,
            TokenType::Minus => BinaryOperator::Sub,
            TokenType::Star => BinaryOperator::Mul,
            TokenType::Slash => BinaryOperator::Div,
            other => unreachable!("{other:?} is no binary expression"),
        };

        return Ok(Expression {
            variant: ExpressionType::Binary {
                left: Box::new(left),
                operator,
                right: Box::new(right)
            },
            span
        })
    }

    fn try_into_assignment_target(expression: Expression) -> Result<Expression, ParseError> {
        match expression.variant {
            ExpressionType::Ident(_) => Ok(expression),
            _ => return Err(ParseError::invalid_assignment_target(expression.clone())),
        }
    }

    fn parse_identifier(&mut self) -> Result<Identifier, ParseError> {
        let (str, span) = expect_token!(self, TokenType::Ident(ident) => ident)?;
        return Ok(Identifier { str, span })
    }
}
