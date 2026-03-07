use std::collections::HashMap;
use std::sync::Arc;

use crate::value::{LispError, Value};

/// 環境 (スコープチェーン)
#[derive(Clone)]
pub struct Env {
    bindings: HashMap<String, Value>,
    parent: Option<Arc<Env>>,
}

impl Env {
    pub fn new() -> Self {
        Self {
            bindings: HashMap::new(),
            parent: None,
        }
    }

    pub fn with_parent(parent: Arc<Env>) -> Self {
        Self {
            bindings: HashMap::new(),
            parent: Some(parent),
        }
    }

    pub fn get(&self, name: &str) -> Result<Value, LispError> {
        if let Some(val) = self.bindings.get(name) {
            Ok(val.clone())
        } else if let Some(parent) = &self.parent {
            parent.get(name)
        } else {
            Err(LispError::new(format!("undefined symbol: {}", name)))
        }
    }

    pub fn define(&mut self, name: impl Into<String>, val: Value) {
        self.bindings.insert(name.into(), val);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.bindings.contains_key(name)
    }
}
