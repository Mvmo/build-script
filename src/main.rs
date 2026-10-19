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
                token.span.clone(),
            ))
        }
    }};
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Span {
    start: usize,
    end: usize,
}

impl Span {
    fn new(start: usize, end: usize) -> Self {
        return Span {
            start,
            end
        }
    }

    fn till(&self, end: Span) -> Span {
        return Span::new(
            self.start,
            end.end
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    variant: TokenType,
    span: Span,
}

impl Token {
    fn new(variant: TokenType, span: Span) -> Self {
        return Token {
            variant,
            span
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenType {
    // Keywords
    Use,

    // Operators
    EqEq,
    Eq,
    Minus,
    Plus,
    Bang,

    // Parens
    LeftParen,
    RightParen,

    Ident(String),
    String(String),

    EOF,
}

impl ToString for TokenType {
    fn to_string(&self) -> String {
        return String::from(match self {
            TokenType::Use => "use",
            TokenType::EqEq => "==",
            TokenType::Eq => "=",
            TokenType::Minus => "-",
            TokenType::Plus => "+",
            TokenType::Bang => "!",
            TokenType::LeftParen => "(",
            TokenType::RightParen => ")",
            TokenType::Ident(ident) => ident,
            TokenType::String(str) => str,
            TokenType::EOF => "<<EOF>>",
        })
    }
}

const CONTROL_SYMBOLS: &[(&'static str, TokenType)] = &[
    ("(", TokenType::LeftParen),
    (")", TokenType::RightParen),
    ("==", TokenType::EqEq),
    ("=", TokenType::Eq),
    ("-", TokenType::Minus),
    ("+", TokenType::Plus),
    ("!", TokenType::Bang),
];

const fn control_chars() -> [char;CONTROL_SYMBOLS.len()] {
    let mut out: [char;CONTROL_SYMBOLS.len()] = ['\0';CONTROL_SYMBOLS.len()];

    let mut index: usize = 0;
    while index <= CONTROL_SYMBOLS.len() - 1 {
        out[index] = CONTROL_SYMBOLS[index].0.as_bytes()[0] as char;
        index += 1;
    }

    return out
}

const CONTROL_CHARS: [char; CONTROL_SYMBOLS.len()] = control_chars();

const KEYWORDS: &[(&'static str, TokenType)] = &[
    ("use", TokenType::Use),
];


struct TokenStream<'src> {
    input: &'src str,
    current_pos: usize,
}

impl<'src> TokenStream<'src> {
    pub fn new(input: &'src str) -> Self {
        return TokenStream {
            input: input,
            current_pos: 0
        }
    }

    pub fn lex_token(&self, pos: usize) -> Token {
        let mut current_pos = pos;

        let remaining = &self.input[current_pos..];
        let trimmed = remaining.trim_start();

        let diff = remaining.len() - trimmed.len();
        current_pos += diff;

        let start_pos = current_pos;

        let first_char = if let Some(next_char) = trimmed.chars().next() {
            next_char
        } else {
            return Token { variant: TokenType::EOF, span: Span::new(start_pos, start_pos) }
        };

        if first_char == '"' {
            let closing_index = trimmed[1..].find('"')
                .expect("expected closing \"");

            let str_content = &trimmed[1..closing_index + 1];
            current_pos += str_content.len() + 2; // closing and opening "

            return Token::new(TokenType::String(str_content.to_string()), Span::new(
                start_pos,
                current_pos
            ))
        };

        if let Some((variant, len)) = Self::control_symbol(trimmed) {
            current_pos += len;
            return Token::new(
                variant,
                Span::new(start_pos, current_pos)
            )
        };

        let maybe_ident_len = trimmed
            .find(|char: char| char.is_whitespace() || char.is_ascii_whitespace() || CONTROL_CHARS.contains(&char))
            .unwrap_or_else(|| trimmed.len());

        let maybe_ident = &trimmed[..maybe_ident_len];
        current_pos += maybe_ident_len;

        if let Some(keyword) = Self::keyword(maybe_ident) {
            return Token::new(keyword, Span::new(
                start_pos,
                current_pos
            ))
        };

        return Token::new(
            TokenType::Ident(maybe_ident.to_string()),
            Span::new(
                start_pos,
                current_pos
            ),
        )
    }

    // let x=5
    pub fn next(&mut self) -> Token {
        let token = self.lex_token(self.current_pos);
        self.current_pos = token.span.end;

        return token
    }

    pub fn peek(&mut self) -> Token {
        return self.lex_token(self.current_pos)
    }

    pub fn eat_next(&mut self) {
        let _ = self.next();
    }

    fn control_symbol(remaining: &str) -> Option<(TokenType, usize)> {
        for (symbol, token) in CONTROL_SYMBOLS {
            if remaining.starts_with(*symbol) {
                return Some((token.clone(), symbol.len()));
            }
        }

        return None;
    }

    fn keyword(chain: &'src str) -> Option<TokenType> {
        for (symbol, token) in KEYWORDS {
            if *symbol == chain {
                return Some(token.clone())
            }
        }

        return None
    }
}

mod parser {
    use std::unreachable;

    use crate::{Span, Token, TokenStream, TokenType};

    enum ParseErrorType {
        UnexpectedToken {
            expected: Vec<String>, // or
            got: Option<Token>,
        }
    }

    struct ParseError {
        variant: ParseErrorType,
        span: Span,
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
    }

    struct Parser<'src> {
        token_stream: TokenStream<'src>
    }

    enum UnaryOperator {
        Neg,
        Not,
    }

    enum BinaryOperator {
        Add,
        Sub,
        Mul,
        Div,
        Eq,
        NotEq,
    }

    // examples:
    // ident: use kotlin
    // filepath: use "otherscript"
    enum ExpressionType {
        Ident(String),
        String(String),
        Unary {
            operator: BinaryOperator,
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

    struct Expression {
        variant: ExpressionType,
        span: Span,
    }

    enum TypeExpressionType {
        String,
    }

    struct TypeExpression {
        variant: TypeExpressionType,
        span: Span
    }

    struct Identifier {
        str: String,
        span: String,
    }

    struct Parameters {
        params: Vec<(Identifier, TypeExpression)>,
        span: Span,
    }

    enum StatementType {
        Assignment {
            identifier: Identifier,
        },
        Expression(Expression),
        Function {
            params: Parameters,
        }
    }

    struct Statement {
        variant: StatementType,
        span: Span
    }

    impl Statement {
        fn new(variant: StatementType, span: Span) -> Self {
            return Statement { variant, span }
        }
    }

    struct Script {
        statements: Vec<Statement>
    }

    impl<'src> Parser<'src> {
        fn parse_script(&mut self) -> Script {
            let mut statements = vec![];
            while self.token_stream.peek().variant != TokenType::EOF {
                statements.push(self.parse_statement());
            }

            return Script {
                statements: vec![]
            }
        }

        fn parse_prefix_expression(&mut self) {
            let token = self.token_stream.next();
            let span = token.span;

            match token.variant {
                TokenType::Ident(str) => Ok(Expression {
                    variant: ExpressionType::Ident(str),
                    span
                }),
                TokenType::String(str) => Ok(Expression {
                    variant: ExpressionType::String(str),
                    span
                }),
                TokenType::Minus => self.parse_unary(UnaryOp::)
            }
        }

        fn parse_atom_expression(&mut self) -> Result<Expression<'src>, ParseError> {
            let next_token = self.token_stream.next();
            return Ok(atom);
        }

        fn parse_expression(&mut self, precedence: u8) -> Result<Expression<'src>, ParseError> {
            let mut lhs = match self.token_stream.next().variant {
                TokenType::Use => todo!(),
                TokenType::EqEq => todo!(),
                TokenType::Eq => todo!(),
                TokenType::LeftParen => todo!(),
                TokenType::RightParen => todo!(),
                TokenType::Ident(_) => {
                    self.parse_identifier()
                },
                TokenType::String(_) => todo!(),
                TokenType::EOF => todo!(),
            };

            todo!()
        }

        // statement parsers
        fn parse_statement(&mut self) -> Result<Statement<'src>, ParseError> {
            match self.token_stream.peek().variant {
                TokenType::Use => self.parse_use();
            }
        }

        fn parse_identifier(&mut self) -> Result<Identifier, ParseError> {
            let identifier = expect_token!(self, TokenType::Ident(..))?;
            if let TokenType::Ident(ident) = identifier.variant {
                return Ok(Identifier {
                    str: ident,
                    span: identifier.span
                })
            }

            unreachable!("we expected on type already")
        }

        fn expect_next_token(&mut self, expected_variant: TokenType) -> Result<Token, ParseError> {
            let token = self.token_stream.peek();
            if token.variant == expected_variant {
                return Ok(token);
            } else {
                return Err(ParseError::unexpected_token(vec![expected_variant.to_string()], Some(token.clone()), token.span.clone()));
            }
        }
    }
}


fn main() {
    let mut stream = TokenStream::new("let x = \"hello\"");
    let mut next = stream.next();
    while next.variant != TokenType::EOF {
        println!("{:?}", next.variant);
        next = stream.next();
    }
}
