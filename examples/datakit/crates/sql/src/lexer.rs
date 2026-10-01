use std::ops::Range;

/// What a [`Token`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lexeme {
    /// A keyword or an unquoted identifier; the lexer does not tell them
    /// apart, because which one a word is depends on where it stands.
    Word,
    /// `"Name"`.
    QuotedIdentifier,
    /// `'text'`, `E'text'`, `B'0101'`, `X'ff'`, `U&'text'`, or a
    /// dollar-quoted `$tag$ text $tag$`.
    String,
    Number,
    /// `$1`, or a named parameter such as `:name`.
    Parameter,
    LineComment,
    BlockComment,
    Whitespace,
    Semicolon,
    Comma,
    Period,
    OpenParen,
    CloseParen,
    /// Any other operator or punctuation, such as `=`, `::` or `->>`.
    Operator,
    /// A string, quoted identifier or comment the text ends inside of.
    Unterminated,
}

impl Lexeme {
    /// Whether the token carries meaning: not whitespace and not a comment.
    pub fn is_significant(self) -> bool {
        !matches!(
            self,
            Lexeme::Whitespace | Lexeme::LineComment | Lexeme::BlockComment
        )
    }
}

/// A run of text with one [`Lexeme`], as a byte range into the lexed text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    lexeme: Lexeme,
    range: Range<usize>,
}

impl Token {
    pub fn lexeme(&self) -> Lexeme {
        self.lexeme
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.range.clone()]
    }

    /// The same token in a text where its source starts `by` bytes later.
    pub(crate) fn offset_by(self, by: usize) -> Token {
        Token {
            lexeme: self.lexeme,
            range: self.range.start + by..self.range.end + by,
        }
    }

    /// Whether the token is the word `keyword`, compared without case.
    pub fn is_keyword(&self, source: &str, keyword: &str) -> bool {
        self.lexeme == Lexeme::Word && self.text(source).eq_ignore_ascii_case(keyword)
    }
}

/// Split `text` into tokens covering every byte, following PostgreSQL's
/// lexical rules. Never fails: text that ends inside a string or comment
/// ends with an [`Lexeme::Unterminated`] token.
pub fn lex(text: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        text,
        bytes: text.as_bytes(),
        position: 0,
        tokens: Vec::new(),
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    position: usize,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        while self.position < self.bytes.len() {
            let start = self.position;
            let lexeme = self.next_lexeme();
            debug_assert!(self.position > start, "the lexer must always advance");
            self.tokens.push(Token {
                lexeme,
                range: start..self.position,
            });
        }
    }

    fn peek(&self, offset: usize) -> Option<u8> {
        self.bytes.get(self.position + offset).copied()
    }

    fn next_lexeme(&mut self) -> Lexeme {
        let byte = self.bytes[self.position];
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' | 0x0c => {
                while self
                    .peek(0)
                    .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c))
                {
                    self.position += 1;
                }
                Lexeme::Whitespace
            }
            b'-' if self.peek(1) == Some(b'-') => {
                while self.peek(0).is_some_and(|b| b != b'\n') {
                    self.position += 1;
                }
                Lexeme::LineComment
            }
            b'/' if self.peek(1) == Some(b'*') => self.block_comment(),
            b'\'' => self.quoted(b'\'', false, Lexeme::String),
            b'"' => self.quoted(b'"', false, Lexeme::QuotedIdentifier),
            // MySQL and ClickHouse quote identifiers with backticks.
            b'`' => self.quoted(b'`', false, Lexeme::QuotedIdentifier),
            b'e' | b'E' if self.peek(1) == Some(b'\'') => {
                self.position += 1;
                self.quoted(b'\'', true, Lexeme::String)
            }
            b'b' | b'B' | b'x' | b'X' | b'n' | b'N' if self.peek(1) == Some(b'\'') => {
                self.position += 1;
                self.quoted(b'\'', false, Lexeme::String)
            }
            b'u' | b'U' if self.peek(1) == Some(b'&') && self.peek(2) == Some(b'\'') => {
                self.position += 2;
                self.quoted(b'\'', false, Lexeme::String)
            }
            b'u' | b'U' if self.peek(1) == Some(b'&') && self.peek(2) == Some(b'"') => {
                self.position += 2;
                self.quoted(b'"', false, Lexeme::QuotedIdentifier)
            }
            b'$' => self.dollar(),
            b'0'..=b'9' => self.number(),
            b'.' if self.peek(1).is_some_and(|b| b.is_ascii_digit()) => self.number(),
            b';' => self.single(Lexeme::Semicolon),
            b',' => self.single(Lexeme::Comma),
            b'.' => self.single(Lexeme::Period),
            b'(' => self.single(Lexeme::OpenParen),
            b')' => self.single(Lexeme::CloseParen),
            b':' if self.peek(1).is_some_and(is_word_start) => {
                self.position += 1;
                self.word();
                Lexeme::Parameter
            }
            b if is_word_start(b) => {
                self.word();
                Lexeme::Word
            }
            _ => self.operator(),
        }
    }

    fn single(&mut self, lexeme: Lexeme) -> Lexeme {
        self.position += 1;
        lexeme
    }

    fn word(&mut self) {
        while self.peek(0).is_some_and(is_word_continue) {
            self.position += 1;
        }
    }

    fn number(&mut self) -> Lexeme {
        while self
            .peek(0)
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
        {
            // `1.e5` and `1e-5` are numbers; `1..2` is not.
            if self.peek(0) == Some(b'.') && self.peek(1) == Some(b'.') {
                break;
            }
            let exponent = matches!(self.peek(0), Some(b'e' | b'E'))
                && matches!(self.peek(1), Some(b'+' | b'-'));
            self.position += if exponent { 2 } else { 1 };
        }
        Lexeme::Number
    }

    fn operator(&mut self) -> Lexeme {
        const OPERATOR: &[u8] = b"+-*/<>=~!@#%^&|`?:[]{}\\";
        let first = self.bytes[self.position];
        self.position += 1;
        if !OPERATOR.contains(&first) {
            // A character outside SQL's alphabet; advance by the whole
            // character so a multi-byte one is never split.
            while !self.text.is_char_boundary(self.position) {
                self.position += 1;
            }
            return Lexeme::Operator;
        }
        // Brackets and a lone `:` stand alone; a run of operator characters
        // is one operator, but never swallows the start of a comment.
        if matches!(first, b'[' | b']' | b'{' | b'}') {
            return Lexeme::Operator;
        }
        while let Some(next) = self.peek(0) {
            let starts_comment = (next == b'-' && self.peek(1) == Some(b'-'))
                || (next == b'/' && self.peek(1) == Some(b'*'));
            if !OPERATOR.contains(&next) || matches!(next, b'[' | b']') || starts_comment {
                break;
            }
            self.position += 1;
        }
        Lexeme::Operator
    }

    fn block_comment(&mut self) -> Lexeme {
        // PostgreSQL block comments nest.
        self.position += 2;
        let mut depth = 1;
        while self.position < self.bytes.len() {
            if self.peek(0) == Some(b'/') && self.peek(1) == Some(b'*') {
                depth += 1;
                self.position += 2;
            } else if self.peek(0) == Some(b'*') && self.peek(1) == Some(b'/') {
                depth -= 1;
                self.position += 2;
                if depth == 0 {
                    return Lexeme::BlockComment;
                }
            } else {
                self.position += 1;
            }
        }
        Lexeme::Unterminated
    }

    /// A run quoted by `quote`, where a doubled quote stands for itself and,
    /// with `backslash`, a backslash escapes the next byte.
    fn quoted(&mut self, quote: u8, backslash: bool, lexeme: Lexeme) -> Lexeme {
        self.position += 1;
        while let Some(byte) = self.peek(0) {
            if backslash && byte == b'\\' {
                self.position += 2.min(self.bytes.len() - self.position);
            } else if byte == quote {
                if self.peek(1) == Some(quote) {
                    self.position += 2;
                } else {
                    self.position += 1;
                    return lexeme;
                }
            } else {
                self.position += 1;
            }
        }
        Lexeme::Unterminated
    }

    /// `$1`, or a dollar-quoted string `$tag$ … $tag$`, or a lone `$`.
    fn dollar(&mut self) -> Lexeme {
        let start = self.position;
        if self.peek(1).is_some_and(|b| b.is_ascii_digit()) {
            self.position += 1;
            while self.peek(0).is_some_and(|b| b.is_ascii_digit()) {
                self.position += 1;
            }
            return Lexeme::Parameter;
        }
        let mut end = start + 1;
        while self
            .bytes
            .get(end)
            .copied()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80)
        {
            end += 1;
        }
        let tag_is_valid = !self
            .bytes
            .get(start + 1)
            .copied()
            .is_some_and(|b| b.is_ascii_digit());
        if self.bytes.get(end) != Some(&b'$') || !tag_is_valid {
            return self.operator();
        }
        let delimiter = &self.text[start..=end];
        self.position = end + 1;
        match self.text[self.position..].find(delimiter) {
            Some(close) => {
                self.position += close + delimiter.len();
                Lexeme::String
            }
            None => {
                self.position = self.bytes.len();
                Lexeme::Unterminated
            }
        }
    }
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_word_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lexemes(text: &str) -> Vec<(Lexeme, &str)> {
        lex(text)
            .into_iter()
            .filter(|token| token.lexeme() != Lexeme::Whitespace)
            .map(|token| (token.lexeme(), &text[token.range()]))
            .collect()
    }

    #[test]
    fn tokens_cover_every_byte() {
        let text = "select 'a;b', \"c\"\"d\" -- x\n/* /* y */ */ $f$ ; $f$ from t;";
        let tokens = lex(text);
        let mut position = 0;
        for token in &tokens {
            assert_eq!(token.range().start, position);
            position = token.range().end;
        }
        assert_eq!(position, text.len());
    }

    #[test]
    fn quoted_text_keeps_its_semicolons() {
        assert_eq!(
            lexemes("'a;b' \"x;y\" E'it\\'s;' $$ ; $$ $body$ $$ ; $body$"),
            vec![
                (Lexeme::String, "'a;b'"),
                (Lexeme::QuotedIdentifier, "\"x;y\""),
                (Lexeme::String, "E'it\\'s;'"),
                (Lexeme::String, "$$ ; $$"),
                (Lexeme::String, "$body$ $$ ; $body$"),
            ]
        );
    }

    #[test]
    fn block_comments_nest() {
        assert_eq!(
            lexemes("/* a /* b */ ; */ x"),
            vec![
                (Lexeme::BlockComment, "/* a /* b */ ; */"),
                (Lexeme::Word, "x")
            ]
        );
    }

    #[test]
    fn parameters_casts_and_numbers() {
        assert_eq!(
            lexemes("$1::int + :limit - 1.5e-3"),
            vec![
                (Lexeme::Parameter, "$1"),
                (Lexeme::Operator, "::"),
                (Lexeme::Word, "int"),
                (Lexeme::Operator, "+"),
                (Lexeme::Parameter, ":limit"),
                (Lexeme::Operator, "-"),
                (Lexeme::Number, "1.5e-3"),
            ]
        );
    }

    #[test]
    fn an_operator_stops_before_a_comment() {
        assert_eq!(
            lexemes("a =-- note"),
            vec![
                (Lexeme::Word, "a"),
                (Lexeme::Operator, "="),
                (Lexeme::LineComment, "-- note"),
            ]
        );
    }

    #[test]
    fn unterminated_text_is_reported_not_rejected() {
        assert_eq!(
            lexemes("select 'abc"),
            vec![(Lexeme::Word, "select"), (Lexeme::Unterminated, "'abc")]
        );
        assert_eq!(lexemes("/* open"), vec![(Lexeme::Unterminated, "/* open")]);
    }

    #[test]
    fn non_ascii_text_is_never_split_mid_character() {
        let text = "select 名前, «x» from 表";
        let tokens = lex(text);
        for token in &tokens {
            assert!(text.is_char_boundary(token.range().start));
            assert!(text.is_char_boundary(token.range().end));
        }
    }
}
