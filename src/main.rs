use std::matches;

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
enum TokenType {
    // Keywords
    Use,
    Let,

    // Operators
    EqEq,
    Eq,

    // Parens
    LeftParen,
    RightParen,

    Ident(String),
    String(String),
}

const CONTROL_SYMBOLS: &[(&'static str, TokenType)] = &[
    ("(", TokenType::LeftParen),
    (")", TokenType::RightParen),
    ("==", TokenType::EqEq),
    ("=", TokenType::Eq),
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
    ("let", TokenType::Let),
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

    // let x=5
    pub fn next(&mut self) -> Option<Token> {
        let remaining = &self.input[self.current_pos..];
        let trimmed = remaining.trim_start();

        let diff = remaining.len() - trimmed.len();
        self.current_pos += diff;

        let start_pos = self.current_pos;

        let first_char = trimmed.chars().next()?;
        if first_char == '"' {
            let closing_index = trimmed[1..].find('"')
                .expect("expected closing \"");

            let str_content = &trimmed[1..closing_index + 1];
            self.current_pos += str_content.len() + 2; // closing and opening "

            return Some(Token::new(TokenType::String(str_content.to_string()), Span::new(
                start_pos,
                self.current_pos
            )))
        };

        if let Some((variant, len)) = Self::control_symbol(trimmed) {
            self.current_pos += len;
            return Some(Token::new(
                variant,
                Span::new(start_pos, self.current_pos)
            ))
        };

        let maybe_ident_len = trimmed
            .find(|char: char| char.is_whitespace() || char.is_ascii_whitespace() || CONTROL_CHARS.contains(&char))
            .unwrap_or_else(|| trimmed.len());

        let maybe_ident = &trimmed[..maybe_ident_len];
        self.current_pos += maybe_ident_len;

        if let Some(keyword) = Self::keyword(maybe_ident) {
            return Some(Token::new(keyword, Span::new(
                start_pos,
                self.current_pos
            )))
        };

        return Some(Token::new(
            TokenType::Ident(maybe_ident.to_string()),
            Span::new(
                start_pos,
                self.current_pos
            )),
        )
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

fn main() {
    let mut stream = TokenStream::new("let x = \"hello\"");
    while let Some(token) = stream.next() {
        println!("{token:?}");
    }
}
