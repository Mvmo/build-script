#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn till(&self, end: Span) -> Span {
        return Span::new(self.start, end.end);
    }

    fn new(start: usize, end: usize) -> Self {
        return Span { start, end };
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub variant: TokenType,
    pub span: Span,
}

impl Token {
    pub fn new(variant: TokenType, span: Span) -> Self {
        return Token { variant, span };
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenType {
    // Keywords
    Use,
    Fn,

    // Operators
    EqEq,
    BangEq,
    Eq,
    Minus,
    Plus,
    Star,
    Slash,
    Bang,

    // Parens
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    Comma,

    // Types
    Colon,
    Arrow,
    Less,
    Greater,
    Pipe,

    Ident(String),
    String(String),

    EOF,
}

impl ToString for TokenType {
    fn to_string(&self) -> String {
        return String::from(match self {
            TokenType::Use => "use",
            TokenType::Fn => "fn",
            TokenType::EqEq => "==",
            TokenType::BangEq => "!=",
            TokenType::Eq => "=",
            TokenType::Minus => "-",
            TokenType::Plus => "+",
            TokenType::Star => "*",
            TokenType::Slash => "/",
            TokenType::Bang => "!",
            TokenType::LeftParen => "(",
            TokenType::RightParen => ")",
            TokenType::LeftBrace => "{",
            TokenType::RightBrace => "}",
            TokenType::Comma => ",",
            TokenType::Colon => ":",
            TokenType::Arrow => "->",
            TokenType::Less => "<",
            TokenType::Greater => ">",
            TokenType::Pipe => "|",
            TokenType::Ident(ident) => ident,
            TokenType::String(str) => str,
            TokenType::EOF => "<<EOF>>",
        });
    }
}

// longer symbols first: "->" before "-"
const CONTROL_SYMBOLS: &[(&'static str, TokenType)] = &[
    ("(", TokenType::LeftParen),
    (")", TokenType::RightParen),
    ("{", TokenType::LeftBrace),
    ("}", TokenType::RightBrace),
    (",", TokenType::Comma),
    (":", TokenType::Colon),
    ("<", TokenType::Less),
    (">", TokenType::Greater),
    ("|", TokenType::Pipe),
    ("==", TokenType::EqEq),
    ("!=", TokenType::BangEq),
    ("=", TokenType::Eq),
    ("->", TokenType::Arrow),
    ("-", TokenType::Minus),
    ("+", TokenType::Plus),
    ("*", TokenType::Star),
    ("/", TokenType::Slash),
    ("!", TokenType::Bang),
];

const fn control_chars() -> [char; CONTROL_SYMBOLS.len()] {
    let mut out: [char; CONTROL_SYMBOLS.len()] = ['\0'; CONTROL_SYMBOLS.len()];

    let mut index: usize = 0;
    while index < CONTROL_SYMBOLS.len() {
        out[index] = CONTROL_SYMBOLS[index].0.as_bytes()[0] as char;
        index += 1;
    }

    return out;
}

const CONTROL_CHARS: [char; CONTROL_SYMBOLS.len()] = control_chars();

const KEYWORDS: &[(&'static str, TokenType)] = &[("use", TokenType::Use), ("fn", TokenType::Fn)];

pub struct TokenStream<'src> {
    pub input: &'src str,
    pub current_pos: usize,
}

impl<'src> TokenStream<'src> {
    pub fn new(input: &'src str) -> Self {
        return TokenStream {
            input: input,
            current_pos: 0,
        };
    }

    pub fn lex_token(&self, pos: usize) -> Token {
        let mut current_pos = self.skip_whitespace_and_comments(pos);
        let trimmed = &self.input[current_pos..];

        let start_pos = current_pos;

        let first_char = if let Some(next_char) = trimmed.chars().next() {
            next_char
        } else {
            return Token {
                variant: TokenType::EOF,
                span: Span::new(start_pos, start_pos),
            };
        };

        if first_char == '"' {
            let closing_index = trimmed[1..].find('"').expect("expected closing \"");

            let str_content = &trimmed[1..closing_index + 1];
            current_pos += str_content.len() + 2; // closing and opening "

            return Token::new(
                TokenType::String(str_content.to_string()),
                Span::new(start_pos, current_pos),
            );
        };

        if let Some((variant, len)) = Self::control_symbol(trimmed) {
            current_pos += len;
            return Token::new(variant, Span::new(start_pos, current_pos));
        };

        let maybe_ident_len = trimmed
            .find(|char: char| char.is_whitespace() || char == '"' || CONTROL_CHARS.contains(&char))
            .unwrap_or_else(|| trimmed.len());

        let maybe_ident = &trimmed[..maybe_ident_len];
        current_pos += maybe_ident_len;

        if let Some(keyword) = Self::keyword(maybe_ident) {
            return Token::new(keyword, Span::new(start_pos, current_pos));
        };

        return Token::new(
            TokenType::Ident(maybe_ident.to_string()),
            Span::new(start_pos, current_pos),
        );
    }

    pub fn next(&mut self) -> Token {
        let token = self.lex_token(self.current_pos);
        self.current_pos = token.span.end;

        return token;
    }

    pub fn peek(&self) -> Token {
        return self.lex_token(self.current_pos);
    }

    pub fn eat_next(&mut self) {
        let _ = self.next();
    }

    /// Position of the next token: whitespace, `// …` up to the end of the line
    /// and `/* … */` are skipped
    fn skip_whitespace_and_comments(&self, pos: usize) -> usize {
        let mut current_pos = pos;
        loop {
            let remaining = &self.input[current_pos..];
            let trimmed = remaining.trim_start();
            current_pos += remaining.len() - trimmed.len();

            if trimmed.starts_with("//") {
                current_pos += trimmed.find('\n').unwrap_or(trimmed.len());
            } else if trimmed.starts_with("/*") {
                current_pos += trimmed
                    .find("*/")
                    .map(|end| end + 2)
                    .unwrap_or(trimmed.len());
            } else {
                return current_pos;
            }
        }
    }

    fn control_symbol(remaining: &str) -> Option<(TokenType, usize)> {
        for (symbol, token) in CONTROL_SYMBOLS {
            if remaining.starts_with(*symbol) {
                return Some((token.clone(), symbol.len()));
            }
        }

        return None;
    }

    fn keyword(chain: &str) -> Option<TokenType> {
        for (symbol, token) in KEYWORDS {
            if *symbol == chain {
                return Some(token.clone());
            }
        }

        return None;
    }
}
