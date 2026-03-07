use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// lisprint の全ての値を表す型
#[derive(Clone)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<String>),
    Symbol(Arc<String>),
    Keyword(Arc<String>),
    List(Arc<Vec<Value>>),
    Vec(Arc<Vec<Value>>),
    Map(Arc<HashMap<String, Value>>),
    Fn(Arc<LispFn>),
    NativeFn(Arc<NativeFnData>),
    Macro(Arc<LispFn>),
}

/// Lispで定義された関数
pub struct LispFn {
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body: Vec<Value>,
    pub env: crate::env::Env,
}

/// Rust側で定義されたネイティブ関数
pub struct NativeFnData {
    pub name: String,
    pub func: Box<dyn Fn(&[Value]) -> Result<Value, LispError> + Send + Sync>,
}

/// エラー型
#[derive(Clone, Debug)]
pub struct LispError {
    pub message: String,
}

impl LispError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { message: msg.into() }
    }
}

impl fmt::Display for LispError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for LispError {}

pub type LispResult = Result<Value, LispError>;

// --- Value のヘルパーメソッド (将来のNaN boxing移行に備えて) ---

impl Value {
    pub fn int(n: i64) -> Self {
        Value::Int(n)
    }

    pub fn float(n: f64) -> Self {
        Value::Float(n)
    }

    pub fn bool(b: bool) -> Self {
        Value::Bool(b)
    }

    pub fn str(s: impl Into<String>) -> Self {
        Value::Str(Arc::new(s.into()))
    }

    pub fn symbol(s: impl Into<String>) -> Self {
        Value::Symbol(Arc::new(s.into()))
    }

    pub fn keyword(s: impl Into<String>) -> Self {
        Value::Keyword(Arc::new(s.into()))
    }

    pub fn list(v: Vec<Value>) -> Self {
        Value::List(Arc::new(v))
    }

    pub fn vec(v: Vec<Value>) -> Self {
        Value::Vec(Arc::new(v))
    }

    pub fn nil() -> Self {
        Value::Nil
    }

    pub fn as_int(&self) -> Result<i64, LispError> {
        match self {
            Value::Int(n) => Ok(*n),
            _ => Err(LispError::new(format!("expected int, got {}", self.type_name()))),
        }
    }

    pub fn as_float(&self) -> Result<f64, LispError> {
        match self {
            Value::Float(n) => Ok(*n),
            Value::Int(n) => Ok(*n as f64),
            _ => Err(LispError::new(format!("expected float, got {}", self.type_name()))),
        }
    }

    pub fn as_str(&self) -> Result<&str, LispError> {
        match self {
            Value::Str(s) => Ok(s),
            _ => Err(LispError::new(format!("expected string, got {}", self.type_name()))),
        }
    }

    pub fn as_symbol(&self) -> Result<&str, LispError> {
        match self {
            Value::Symbol(s) => Ok(s),
            _ => Err(LispError::new(format!("expected symbol, got {}", self.type_name()))),
        }
    }

    pub fn as_list(&self) -> Result<&[Value], LispError> {
        match self {
            Value::List(v) => Ok(v),
            _ => Err(LispError::new(format!("expected list, got {}", self.type_name()))),
        }
    }

    pub fn as_vec(&self) -> Result<&[Value], LispError> {
        match self {
            Value::Vec(v) => Ok(v),
            _ => Err(LispError::new(format!("expected vec, got {}", self.type_name()))),
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) => "string",
            Value::Symbol(_) => "symbol",
            Value::Keyword(_) => "keyword",
            Value::List(_) => "list",
            Value::Vec(_) => "vec",
            Value::Map(_) => "map",
            Value::Fn(_) => "fn",
            Value::NativeFn(_) => "fn",
            Value::Macro(_) => "macro",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => write!(f, "nil"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(n) => write!(f, "{}", n),
            Value::Str(s) => write!(f, "\"{}\"", s),
            Value::Symbol(s) => write!(f, "{}", s),
            Value::Keyword(s) => write!(f, ":{}", s),
            Value::List(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { write!(f, " ")?; }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
            Value::Vec(items) => {
                write!(f, "[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { write!(f, " ")?; }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            Value::Map(map) => {
                write!(f, "{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 { write!(f, " ")?; }
                    write!(f, ":{} {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Fn(func) => {
                write!(f, "<fn {}>", func.name.as_deref().unwrap_or("anonymous"))
            }
            Value::NativeFn(func) => {
                write!(f, "<native-fn {}>", func.name)
            }
            Value::Macro(m) => {
                write!(f, "<macro {}>", m.name.as_deref().unwrap_or("anonymous"))
            }
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::Keyword(a), Value::Keyword(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Vec(a), Value::Vec(b)) => a == b,
            _ => false,
        }
    }
}
