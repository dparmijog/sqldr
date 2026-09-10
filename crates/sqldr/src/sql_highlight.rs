//! Dialect-aware SQL tokenizing shared by the editor's syntax highlighting
//! and its autocomplete popup.
//!
//! Modular by design: the lexical shape of SQL (strings, numbers, `--`
//! comments, punctuation) is the same across every engine sqldr will ever
//! speak, so the tokenizer itself is dialect-agnostic. Only the *keyword
//! vocabulary* differs per engine — that lives on `Dialect::keywords()` in
//! `sqldr-core`, where MySQL is implemented today and Postgres/SQLite can
//! add their own later without touching this file.

use ratatui::style::{Modifier, Style};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Number,
    Comment,
    /// Identifiers, punctuation, whitespace — anything left unstyled.
    Other,
}

pub struct Token<'a> {
    pub text: &'a str,
    pub kind: TokenKind,
}

/// Splits one line of SQL into tokens, classifying identifiers against
/// `keywords` (case-insensitive). A single-line lexer: `--` comments and
/// `'...'` strings never span lines in the editor's per-line rendering, so
/// there's no cross-line state to track — the worst an unterminated quote
/// at a line boundary does is color the rest of that one line as a string,
/// which is purely cosmetic (never affects what's sent to the server).
pub fn tokenize<'a>(line: &'a str, keywords: &[&str]) -> Vec<Token<'a>> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let len = line.len();
    let mut tokens = Vec::new();
    let mut idx = 0; // index into `chars`, not a byte offset

    while idx < chars.len() {
        let (start, c) = chars[idx];

        if c.is_whitespace() {
            while idx < chars.len() && chars[idx].1.is_whitespace() {
                idx += 1;
            }
            let end = chars.get(idx).map_or(len, |&(b, _)| b);
            tokens.push(Token { text: &line[start..end], kind: TokenKind::Other });
        } else if c == '-' && chars.get(idx + 1).map(|&(_, c2)| c2) == Some('-') {
            tokens.push(Token { text: &line[start..], kind: TokenKind::Comment });
            break;
        } else if c == '\'' || c == '"' {
            let quote = c;
            idx += 1;
            while idx < chars.len() {
                let cc = chars[idx].1;
                if cc == quote {
                    idx += 1;
                    // A doubled quote (`''`) is an escaped quote inside
                    // the literal, not its end.
                    if chars.get(idx).map(|&(_, c2)| c2) == Some(quote) {
                        idx += 1;
                        continue;
                    }
                    break;
                }
                if cc == '\\' && idx + 1 < chars.len() {
                    idx += 2;
                } else {
                    idx += 1;
                }
            }
            let end = chars.get(idx).map_or(len, |&(b, _)| b);
            tokens.push(Token { text: &line[start..end], kind: TokenKind::String });
        } else if c.is_ascii_digit() {
            while idx < chars.len() && (chars[idx].1.is_ascii_digit() || chars[idx].1 == '.') {
                idx += 1;
            }
            let end = chars.get(idx).map_or(len, |&(b, _)| b);
            tokens.push(Token { text: &line[start..end], kind: TokenKind::Number });
        } else if c.is_alphabetic() || c == '_' {
            while idx < chars.len() && (chars[idx].1.is_alphanumeric() || chars[idx].1 == '_') {
                idx += 1;
            }
            let end = chars.get(idx).map_or(len, |&(b, _)| b);
            let word = &line[start..end];
            let kind = if keywords.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                TokenKind::Keyword
            } else {
                TokenKind::Other
            };
            tokens.push(Token { text: word, kind });
        } else {
            // Punctuation/operators: one char at a time, always unstyled.
            idx += 1;
            let end = chars.get(idx).map_or(len, |&(b, _)| b);
            tokens.push(Token { text: &line[start..end], kind: TokenKind::Other });
        }
    }

    tokens
}

pub fn style_for(kind: TokenKind, theme: Theme) -> Option<Style> {
    match kind {
        TokenKind::Keyword => Some(Style::default().fg(theme.sql_keyword).add_modifier(Modifier::BOLD)),
        TokenKind::String => Some(Style::default().fg(theme.sql_string)),
        TokenKind::Number => Some(Style::default().fg(theme.sql_number)),
        TokenKind::Comment => Some(Style::default().fg(theme.sql_comment).add_modifier(Modifier::ITALIC)),
        TokenKind::Other => None,
    }
}

/// Mirrors `tui_textarea`'s internal keep-cursor-in-viewport scroll
/// algorithm exactly (its own viewport state isn't public). Kept in sync
/// by calling this with the same inputs tui_textarea uses — the previous
/// offset `sqldr` tracked last render, the cursor position, and the
/// viewport length — so highlighted token ranges land on the same visible
/// cells tui_textarea already drew.
pub fn next_scroll_top(prev_top: u16, cursor: u16, len: u16) -> u16 {
    if cursor < prev_top {
        cursor
    } else if prev_top + len <= cursor {
        cursor + 1 - len
    } else {
        prev_top
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYWORDS: &[&str] = &["SELECT", "FROM", "WHERE", "AND"];

    fn kinds(line: &str) -> Vec<TokenKind> {
        tokenize(line, KEYWORDS).into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn classifies_keywords_case_insensitively() {
        let tokens = tokenize("select * from widgets", KEYWORDS);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[0].text, "select");
        let from = tokens.iter().find(|t| t.text.eq_ignore_ascii_case("from")).unwrap();
        assert_eq!(from.kind, TokenKind::Keyword);
    }

    #[test]
    fn does_not_classify_identifiers_as_keywords() {
        let tokens = tokenize("select selection from widgets", KEYWORDS);
        let selection = tokens.iter().find(|t| t.text == "selection").unwrap();
        assert_eq!(selection.kind, TokenKind::Other, "'selection' must not match the keyword 'select'");
    }

    #[test]
    fn tokenizes_single_quoted_strings_with_escaped_quotes() {
        let tokens = tokenize("WHERE name = 'it''s here'", KEYWORDS);
        let string_tok = tokens.iter().find(|t| t.kind == TokenKind::String).unwrap();
        assert_eq!(string_tok.text, "'it''s here'", "doubled quote must stay inside the string literal");
    }

    #[test]
    fn line_comment_consumes_rest_of_line() {
        let tokens = tokenize("SELECT 1 -- trailing comment", KEYWORDS);
        let comment = tokens.iter().find(|t| t.kind == TokenKind::Comment).unwrap();
        assert_eq!(comment.text, "-- trailing comment");
    }

    #[test]
    fn classifies_numeric_literals() {
        assert_eq!(kinds("WHERE 10.5"), vec![
            TokenKind::Keyword,
            TokenKind::Other,
            TokenKind::Number,
        ]);
    }

    #[test]
    fn tokens_reconstruct_the_original_line_exactly() {
        let line = "SELECT * FROM widgets WHERE id = 1 AND name = 'bolt' -- ok";
        let tokens = tokenize(line, KEYWORDS);
        let rebuilt: String = tokens.iter().map(|t| t.text).collect();
        assert_eq!(rebuilt, line, "concatenated token text must exactly reconstruct the source line");
    }

    #[test]
    fn does_not_panic_on_non_ascii_characters() {
        // Regression: byte-indexed scanning previously cast individual
        // UTF-8 continuation bytes to `char` and sliced on non-boundary
        // offsets, panicking on any accented/multi-byte character.
        let line = "SELECT * FROM niños WHERE nombre = 'José' -- año";
        let tokens = tokenize(line, KEYWORDS);
        let rebuilt: String = tokens.iter().map(|t| t.text).collect();
        assert_eq!(rebuilt, line);
        let string_tok = tokens.iter().find(|t| t.kind == TokenKind::String).unwrap();
        assert_eq!(string_tok.text, "'José'");
    }
}
