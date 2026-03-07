use crate::env::Env;
use crate::eval;
use crate::parser;
use crate::value::LispError;

const PRELUDE_SOURCE: &str = include_str!("../../../lib/prelude.lisp");

pub fn load(env: &mut Env) -> Result<(), LispError> {
    let exprs = parser::parse(PRELUDE_SOURCE)?;
    for expr in &exprs {
        eval::eval(expr, env)?;
    }
    Ok(())
}
