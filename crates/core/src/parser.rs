use crate::value::{LispError, Value};

/// S式をパースして Value に変換する
pub fn parse(input: &str) -> Result<Vec<Value>, LispError> {
    let tokens = tokenize(input)?;
    let mut pos = 0;
    let mut exprs = Vec::new();
    while pos < tokens.len() {
        let (val, next) = parse_expr(&tokens, pos)?;
        exprs.push(val);
        pos = next;
    }
    Ok(exprs)
}

// --- トークナイザ ---

#[derive(Debug, Clone, PartialEq)]
enum Token {
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Quote,
    Quasiquote,
    Unquote,
    SpliceUnquote,
    Str(String),
    Atom(String),
}

fn tokenize(input: &str) -> Result<Vec<Token>, LispError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            ' ' | '\t' | '\n' | '\r' | ',' => i += 1,

            ';' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }

            '(' => { tokens.push(Token::LParen); i += 1; }
            ')' => { tokens.push(Token::RParen); i += 1; }
            '[' => { tokens.push(Token::LBracket); i += 1; }
            ']' => { tokens.push(Token::RBracket); i += 1; }
            '{' => { tokens.push(Token::LBrace); i += 1; }
            '}' => { tokens.push(Token::RBrace); i += 1; }

            '\'' => { tokens.push(Token::Quote); i += 1; }
            '`' => { tokens.push(Token::Quasiquote); i += 1; }
            '~' => {
                if i + 1 < chars.len() && chars[i + 1] == '@' {
                    tokens.push(Token::SpliceUnquote);
                    i += 2;
                } else {
                    tokens.push(Token::Unquote);
                    i += 1;
                }
            }

            '"' => {
                i += 1;
                let mut s = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                        match chars[i] {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            '\\' => s.push('\\'),
                            '"' => s.push('"'),
                            c => { s.push('\\'); s.push(c); }
                        }
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(LispError::new("unterminated string"));
                }
                i += 1; // closing "
                tokens.push(Token::Str(s));
            }

            _ => {
                let start = i;
                while i < chars.len() && !is_delimiter(chars[i]) {
                    i += 1;
                }
                let atom = chars[start..i].iter().collect::<String>();
                tokens.push(Token::Atom(atom));
            }
        }
    }

    Ok(tokens)
}

fn is_delimiter(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | ',' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';')
}

// --- パーサ ---

fn parse_expr(tokens: &[Token], pos: usize) -> Result<(Value, usize), LispError> {
    if pos >= tokens.len() {
        return Err(LispError::new("unexpected end of input"));
    }

    match &tokens[pos] {
        Token::LParen => parse_list(tokens, pos + 1, Token::RParen)
            .map(|(items, next)| (Value::List(std::sync::Arc::new(items)), next)),

        Token::LBracket => parse_list(tokens, pos + 1, Token::RBracket)
            .map(|(items, next)| (Value::Vec(std::sync::Arc::new(items)), next)),

        Token::LBrace => parse_map(tokens, pos + 1),

        Token::Quote => {
            let (val, next) = parse_expr(tokens, pos + 1)?;
            Ok((Value::list(vec![Value::symbol("quote"), val]), next))
        }

        Token::Quasiquote => {
            let (val, next) = parse_expr(tokens, pos + 1)?;
            Ok((Value::list(vec![Value::symbol("quasiquote"), val]), next))
        }

        Token::Unquote => {
            let (val, next) = parse_expr(tokens, pos + 1)?;
            Ok((Value::list(vec![Value::symbol("unquote"), val]), next))
        }

        Token::SpliceUnquote => {
            let (val, next) = parse_expr(tokens, pos + 1)?;
            Ok((Value::list(vec![Value::symbol("splice-unquote"), val]), next))
        }

        Token::Str(s) => Ok((Value::str(s.clone()), pos + 1)),

        Token::Atom(atom) => Ok((parse_atom(atom), pos + 1)),

        Token::RParen | Token::RBracket | Token::RBrace => {
            Err(LispError::new(format!("unexpected closing delimiter: {:?}", tokens[pos])))
        }
    }
}

fn parse_list(tokens: &[Token], mut pos: usize, end: Token) -> Result<(Vec<Value>, usize), LispError> {
    let mut items = Vec::new();
    while pos < tokens.len() && tokens[pos] != end {
        let (val, next) = parse_expr(tokens, pos)?;
        items.push(val);
        pos = next;
    }
    if pos >= tokens.len() {
        return Err(LispError::new("unterminated list"));
    }
    Ok((items, pos + 1)) // skip closing delimiter
}

fn parse_map(tokens: &[Token], mut pos: usize) -> Result<(Value, usize), LispError> {
    let mut pairs = Vec::new();
    while pos < tokens.len() && tokens[pos] != Token::RBrace {
        let (key, next) = parse_expr(tokens, pos)?;
        pos = next;
        if pos >= tokens.len() || tokens[pos] == Token::RBrace {
            return Err(LispError::new("map requires even number of elements"));
        }
        let (val, next) = parse_expr(tokens, pos)?;
        pairs.push((key, val));
        pos = next;
    }
    if pos >= tokens.len() {
        return Err(LispError::new("unterminated map"));
    }

    let mut map = std::collections::HashMap::new();
    for (k, v) in pairs {
        let key_str = match &k {
            Value::Keyword(s) => s.to_string(),
            Value::Str(s) => s.to_string(),
            _ => return Err(LispError::new("map keys must be keywords or strings")),
        };
        map.insert(key_str, v);
    }

    Ok((Value::Map(std::sync::Arc::new(map)), pos + 1))
}

fn parse_atom(atom: &str) -> Value {
    match atom {
        "nil" => Value::Nil,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => {
            // keyword: starts with :
            if let Some(kw) = atom.strip_prefix(':') {
                return Value::keyword(kw);
            }

            // integer
            if let Ok(n) = atom.parse::<i64>() {
                return Value::Int(n);
            }

            // float
            if let Ok(n) = atom.parse::<f64>() {
                return Value::Float(n);
            }

            // symbol (may contain type annotation like x:i64)
            Value::symbol(atom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_int() {
        let result = parse("42").unwrap();
        assert_eq!(result[0], Value::Int(42));
    }

    #[test]
    fn test_parse_float() {
        let result = parse("3.14").unwrap();
        assert_eq!(result[0], Value::Float(3.14));
    }

    #[test]
    fn test_parse_string() {
        let result = parse("\"hello\"").unwrap();
        assert_eq!(result[0], Value::str("hello"));
    }

    #[test]
    fn test_parse_nil() {
        let result = parse("nil").unwrap();
        assert_eq!(result[0], Value::Nil);
    }

    #[test]
    fn test_parse_bool() {
        let result = parse("true").unwrap();
        assert_eq!(result[0], Value::Bool(true));
    }

    #[test]
    fn test_parse_keyword() {
        let result = parse(":name").unwrap();
        assert_eq!(result[0], Value::keyword("name"));
    }

    #[test]
    fn test_parse_symbol_with_type_annotation() {
        let result = parse("x:i64").unwrap();
        assert_eq!(result[0], Value::symbol("x:i64"));
    }

    #[test]
    fn test_parse_list() {
        let result = parse("(+ 1 2)").unwrap();
        assert_eq!(result[0], Value::list(vec![
            Value::symbol("+"),
            Value::Int(1),
            Value::Int(2),
        ]));
    }

    #[test]
    fn test_parse_vec() {
        let result = parse("[1 2 3]").unwrap();
        assert_eq!(result[0], Value::vec(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]));
    }

    #[test]
    fn test_parse_map() {
        let result = parse("{:name \"alice\"}").unwrap();
        if let Value::Map(m) = &result[0] {
            assert_eq!(m.get("name"), Some(&Value::str("alice")));
        } else {
            panic!("expected map");
        }
    }

    #[test]
    fn test_parse_nested() {
        let result = parse("(defun sum (a b) (+ a b))").unwrap();
        assert_eq!(result.len(), 1);
        if let Value::List(items) = &result[0] {
            assert_eq!(items[0], Value::symbol("defun"));
            assert_eq!(items[1], Value::symbol("sum"));
        } else {
            panic!("expected list");
        }
    }

    #[test]
    fn test_parse_quote() {
        let result = parse("'foo").unwrap();
        assert_eq!(result[0], Value::list(vec![
            Value::symbol("quote"),
            Value::symbol("foo"),
        ]));
    }

    #[test]
    fn test_parse_comment() {
        let result = parse("; comment\n42").unwrap();
        assert_eq!(result[0], Value::Int(42));
    }

    #[test]
    fn test_parse_negative_int() {
        let result = parse("-42").unwrap();
        assert_eq!(result[0], Value::Int(-42));
    }
}
