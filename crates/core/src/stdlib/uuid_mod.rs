use std::sync::Arc;

use crate::env::Env;
use crate::value::{LispError, NativeFnData, Value};

pub fn register(env: &mut Env) {
    let name = "uuid/v4".to_string();
    env.define(
        "uuid/v4",
        Value::NativeFn(Arc::new(NativeFnData {
            name,
            func: Box::new(|args| {
                if !args.is_empty() {
                    return Err(LispError::new("uuid/v4 takes no arguments"));
                }
                Ok(Value::str(uuid::Uuid::new_v4().to_string()))
            }),
        })),
    );
}
