//! The lexer (`formulas.md` section 2.1): text to tokens. It never fails: a lexical error is a
//! [`Tok::Error`] token, so the parser can report the first problem in reading order and the
//! name/id conversions still see the `prop("…")` calls of a text that does not fully parse.
//!
//! Positions count characters (Unicode code points); the byte offsets are kept next to them for the
//! conversions that splice the original text.

/// What a token is.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
    /// A number literal (finite).
    Number(f64),
    /// A string literal, escapes resolved.
    Str(String),
    /// An identifier that is not a keyword.
    Ident(String),
    /// `true`
    True,
    /// `false`
    False,
    /// The word `not`.
    KwNot,
    /// The word `and`.
    KwAnd,
    /// The word `or`.
    KwOr,
    /// `!`
    Bang,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `?`
    Question,
    /// `:`
    Colon,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,
    /// `^`
    Caret,
    /// A lexical error: the message and what was expected.
    Error(String, String),
    /// The end of the text.
    Eof,
}

/// A token with its position in characters and in bytes.
#[derive(Debug, Clone)]
pub(super) struct Token {
    pub tok: Tok,
    /// First character.
    pub start: usize,
    /// One past the last character.
    pub end: usize,
    /// First byte.
    pub byte_start: usize,
    /// One past the last byte.
    pub byte_end: usize,
}

/// A readable name of a token for messages.
pub(super) fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Number(_) => "a number".to_owned(),
        Tok::Str(_) => "a string".to_owned(),
        Tok::Ident(name) => format!("\"{name}\""),
        Tok::True => "\"true\"".to_owned(),
        Tok::False => "\"false\"".to_owned(),
        Tok::KwNot => "\"not\"".to_owned(),
        Tok::KwAnd => "\"and\"".to_owned(),
        Tok::KwOr => "\"or\"".to_owned(),
        Tok::Bang => "\"!\"".to_owned(),
        Tok::AndAnd => "\"&&\"".to_owned(),
        Tok::OrOr => "\"||\"".to_owned(),
        Tok::LParen => "\"(\"".to_owned(),
        Tok::RParen => "\")\"".to_owned(),
        Tok::LBracket => "\"[\"".to_owned(),
        Tok::RBracket => "\"]\"".to_owned(),
        Tok::Comma => "\",\"".to_owned(),
        Tok::Dot => "\".\"".to_owned(),
        Tok::Question => "\"?\"".to_owned(),
        Tok::Colon => "\":\"".to_owned(),
        Tok::EqEq => "\"==\"".to_owned(),
        Tok::NotEq => "\"!=\"".to_owned(),
        Tok::Lt => "\"<\"".to_owned(),
        Tok::Le => "\"<=\"".to_owned(),
        Tok::Gt => "\">\"".to_owned(),
        Tok::Ge => "\">=\"".to_owned(),
        Tok::Plus => "\"+\"".to_owned(),
        Tok::Minus => "\"-\"".to_owned(),
        Tok::Star => "\"*\"".to_owned(),
        Tok::Slash => "\"/\"".to_owned(),
        Tok::Percent => "\"%\"".to_owned(),
        Tok::Caret => "\"^\"".to_owned(),
        Tok::Error(message, _) => message.clone(),
        Tok::Eof => "the end of the formula".to_owned(),
    }
}

struct Lexer<'a> {
    text: &'a str,
    /// `(byte offset, char)` of every character.
    chars: Vec<(usize, char)>,
    pos: usize,
    out: Vec<Token>,
}

/// Splits a text into tokens (comments and white space are dropped). The last token is always
/// [`Tok::Eof`].
pub(super) fn tokenize(text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        text,
        chars: text.char_indices().collect(),
        pos: 0,
        out: Vec::new(),
    };
    lexer.run();
    lexer.out
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).map(|(_, c)| *c)
    }

    fn byte_at(&self, pos: usize) -> usize {
        self.chars
            .get(pos)
            .map_or(self.text.len(), |(byte, _)| *byte)
    }

    fn push(&mut self, tok: Tok, start: usize, end: usize) {
        self.out.push(Token {
            tok,
            start,
            end,
            byte_start: self.byte_at(start),
            byte_end: self.byte_at(end),
        });
    }

    fn error(&mut self, start: usize, end: usize, message: &str, detail: &str) {
        self.push(
            Tok::Error(message.to_owned(), detail.to_owned()),
            start,
            end,
        );
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            let start = self.pos;
            match c {
                ' ' | '\t' | '\r' | '\n' => self.pos += 1,
                '/' if self.peek(1) == Some('*') => self.comment(),
                '0'..='9' => self.number(),
                '.' if self.peek(1).is_some_and(|d| d.is_ascii_digit()) => self.number(),
                '"' => self.string(),
                'A'..='Z' | 'a'..='z' | '_' => self.word(),
                _ => {
                    self.pos += 1;
                    self.punct(c, start);
                }
            }
        }
        let end = self.chars.len();
        self.push(Tok::Eof, end, end);
    }

    fn comment(&mut self) {
        let start = self.pos;
        self.pos += 2;
        while self.pos < self.chars.len() {
            if self.peek(0) == Some('*') && self.peek(1) == Some('/') {
                self.pos += 2;
                return;
            }
            self.pos += 1;
        }
        self.error(start, self.chars.len(), "unclosed comment", "*/");
    }

    fn digits(&mut self) {
        while self.peek(0).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
    }

    fn number(&mut self) {
        let start = self.pos;
        self.digits();
        if self.peek(0) == Some('.') && self.peek(1).is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
            self.digits();
        }
        if matches!(self.peek(0), Some('e' | 'E')) {
            let signed = matches!(self.peek(1), Some('+' | '-'));
            let first = if signed { 2 } else { 1 };
            if self.peek(first).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += first;
                self.digits();
            }
        }
        let from = self.byte_at(start);
        let to = self.byte_at(self.pos);
        let literal = self.text.get(from..to).unwrap_or("0");
        match literal.parse::<f64>() {
            Ok(value) if value.is_finite() => self.push(Tok::Number(value), start, self.pos),
            _ => self.error(start, self.pos, "a number is too large", "number"),
        }
    }

    fn string(&mut self) {
        let start = self.pos;
        self.pos += 1;
        let mut value = String::new();
        let mut bad_escape: Option<(usize, usize)> = None;
        loop {
            let Some(c) = self.peek(0) else {
                self.error(start, self.chars.len(), "unclosed string", "\"");
                return;
            };
            match c {
                '"' => {
                    self.pos += 1;
                    break;
                }
                '\\' => {
                    let escape_start = self.pos;
                    match self.peek(1) {
                        Some('"') => value.push('"'),
                        Some('\\') => value.push('\\'),
                        Some('n') => value.push('\n'),
                        Some('t') => value.push('\t'),
                        Some(_) => {
                            if bad_escape.is_none() {
                                bad_escape = Some((escape_start, escape_start + 2));
                            }
                        }
                        None => {
                            self.error(start, self.chars.len(), "unclosed string", "\"");
                            return;
                        }
                    }
                    self.pos += 2;
                }
                other => {
                    value.push(other);
                    self.pos += 1;
                }
            }
        }
        match bad_escape {
            Some((from, to)) => {
                self.error(from, to, "unknown escape in a string", "\\\" \\\\ \\n \\t");
            }
            None => self.push(Tok::Str(value), start, self.pos),
        }
    }

    fn word(&mut self) {
        let start = self.pos;
        while self
            .peek(0)
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.pos += 1;
        }
        let from = self.byte_at(start);
        let to = self.byte_at(self.pos);
        let word = self.text.get(from..to).unwrap_or("");
        let tok = match word {
            "true" => Tok::True,
            "false" => Tok::False,
            "not" => Tok::KwNot,
            "and" => Tok::KwAnd,
            "or" => Tok::KwOr,
            other => Tok::Ident(other.to_owned()),
        };
        self.push(tok, start, self.pos);
    }

    fn punct(&mut self, c: char, start: usize) {
        let next = self.peek(0);
        let two = |lexer: &mut Self, tok: Tok| {
            lexer.pos += 1;
            lexer.push(tok, start, start + 2);
        };
        match (c, next) {
            ('&', Some('&')) => two(self, Tok::AndAnd),
            ('|', Some('|')) => two(self, Tok::OrOr),
            ('=', Some('=')) => two(self, Tok::EqEq),
            ('!', Some('=')) => two(self, Tok::NotEq),
            ('<', Some('=')) => two(self, Tok::Le),
            ('>', Some('=')) => two(self, Tok::Ge),
            ('!', _) => self.push(Tok::Bang, start, start + 1),
            ('<', _) => self.push(Tok::Lt, start, start + 1),
            ('>', _) => self.push(Tok::Gt, start, start + 1),
            ('(', _) => self.push(Tok::LParen, start, start + 1),
            (')', _) => self.push(Tok::RParen, start, start + 1),
            ('[', _) => self.push(Tok::LBracket, start, start + 1),
            (']', _) => self.push(Tok::RBracket, start, start + 1),
            (',', _) => self.push(Tok::Comma, start, start + 1),
            ('.', _) => self.push(Tok::Dot, start, start + 1),
            ('?', _) => self.push(Tok::Question, start, start + 1),
            (':', _) => self.push(Tok::Colon, start, start + 1),
            ('+', _) => self.push(Tok::Plus, start, start + 1),
            ('-', _) => self.push(Tok::Minus, start, start + 1),
            ('*', _) => self.push(Tok::Star, start, start + 1),
            ('/', _) => self.push(Tok::Slash, start, start + 1),
            ('%', _) => self.push(Tok::Percent, start, start + 1),
            ('^', _) => self.push(Tok::Caret, start, start + 1),
            ('=', _) => self.error(start, start + 1, "unexpected \"=\"", "=="),
            ('&', _) => self.error(start, start + 1, "unexpected \"&\"", "&&"),
            ('|', _) => self.error(start, start + 1, "unexpected \"|\"", "||"),
            (other, _) => {
                let message = format!("unexpected character \"{other}\"");
                self.error(start, start + 1, &message, "");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Tok> {
        tokenize(text).into_iter().map(|token| token.tok).collect()
    }

    #[test]
    fn numbers_strings_and_words() {
        assert_eq!(
            kinds("12 1.5 .5 1e3 2.5E-3"),
            vec![
                Tok::Number(12.0),
                Tok::Number(1.5),
                Tok::Number(0.5),
                Tok::Number(1000.0),
                Tok::Number(0.0025),
                Tok::Eof
            ]
        );
        assert_eq!(
            kinds(r#""a\"b\\c\n" true x_1"#),
            vec![
                Tok::Str("a\"b\\c\n".into()),
                Tok::True,
                Tok::Ident("x_1".into()),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn a_number_followed_by_a_method_dot_keeps_the_dot() {
        assert_eq!(
            kinds("1.abs"),
            vec![
                Tok::Number(1.0),
                Tok::Dot,
                Tok::Ident("abs".into()),
                Tok::Eof
            ]
        );
        // An `e` that starts no exponent begins a word.
        assert_eq!(
            kinds("2e"),
            vec![Tok::Number(2.0), Tok::Ident("e".into()), Tok::Eof]
        );
    }

    #[test]
    fn comments_are_dropped_and_unclosed_ones_are_errors() {
        assert_eq!(
            kinds("1 /* x */ + 2"),
            vec![Tok::Number(1.0), Tok::Plus, Tok::Number(2.0), Tok::Eof]
        );
        let tokens = tokenize("1 /* x");
        assert!(matches!(tokens[1].tok, Tok::Error(..)));
        assert_eq!((tokens[1].start, tokens[1].end), (2, 6));
    }

    #[test]
    fn errors_are_tokens_and_lexing_goes_on() {
        let tokens = tokenize("\"a\\q\" + \"b");
        assert!(matches!(tokens[0].tok, Tok::Error(..)));
        assert_eq!((tokens[0].start, tokens[0].end), (2, 4));
        assert_eq!(tokens[1].tok, Tok::Plus);
        assert!(matches!(tokens[2].tok, Tok::Error(..)));
        assert_eq!(tokens[2].start, 8);
        assert!(matches!(tokenize("a = b")[1].tok, Tok::Error(..)));
        assert!(matches!(tokenize("a # b")[1].tok, Tok::Error(..)));
    }

    #[test]
    fn positions_count_code_points_and_bytes() {
        let tokens = tokenize("\"é😀\" + x");
        assert_eq!((tokens[0].start, tokens[0].end), (0, 4));
        assert_eq!((tokens[0].byte_start, tokens[0].byte_end), (0, 8));
        assert_eq!((tokens[1].start, tokens[2].start), (5, 7));
    }

    #[test]
    fn operators() {
        assert_eq!(
            kinds("&& || ! != == <= >= < > not and or"),
            vec![
                Tok::AndAnd,
                Tok::OrOr,
                Tok::Bang,
                Tok::NotEq,
                Tok::EqEq,
                Tok::Le,
                Tok::Ge,
                Tok::Lt,
                Tok::Gt,
                Tok::KwNot,
                Tok::KwAnd,
                Tok::KwOr,
                Tok::Eof
            ]
        );
    }

    #[test]
    fn an_overflowing_number_is_an_error() {
        assert!(matches!(tokenize("1e999")[0].tok, Tok::Error(..)));
    }
}
