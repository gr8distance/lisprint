use std::collections::HashMap;

use crate::env::Env;
use crate::value::LispError;

/// stdlibモジュール登録関数の型
pub type ModuleRegisterFn = fn(&mut Env);

/// stdlibレジストリ: モジュール名 → 登録関数
pub fn registry() -> HashMap<&'static str, ModuleRegisterFn> {
    let mut map: HashMap<&'static str, ModuleRegisterFn> = HashMap::new();
    map.insert("math", math::register);
    map.insert("str", str_mod::register);
    map.insert("fs", fs::register);
    map.insert("os", os::register);
    map
}

/// stdlibモジュールをロードする (requireから呼ばれる)
pub fn load_stdlib(name: &str, env: &mut Env) -> Result<bool, LispError> {
    let reg = registry();
    if let Some(register_fn) = reg.get(name) {
        register_fn(env);
        Ok(true)
    } else {
        Ok(false)
    }
}

pub mod fs;
pub mod math;
pub mod os;
pub mod str_mod;
