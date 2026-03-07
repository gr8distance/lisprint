//! Runtime support functions for compiled lisprint code.
//! These are linked into the final binary and called from generated code.

use super::compiler::{TAG_NIL, TAG_BOOL, TAG_INT, TAG_FLOAT, TAG_STR};

/// FFI-safe return type for functions that return (tag, payload)
#[repr(C)]
pub struct TaggedValue {
    pub tag: i64,
    pub payload: i64,
}

/// Print a value followed by newline
#[no_mangle]
pub extern "C" fn lsp_println(tag: i64, payload: i64) {
    lsp_print(tag, payload);
    println!();
}

/// Print a value (no newline)
#[no_mangle]
pub extern "C" fn lsp_print(tag: i64, payload: i64) {
    match tag {
        TAG_NIL => print!("nil"),
        TAG_BOOL => print!("{}", if payload != 0 { "true" } else { "false" }),
        TAG_INT => print!("{}", payload),
        TAG_FLOAT => {
            let f = f64::from_bits(payload as u64);
            print!("{}", f);
        }
        TAG_STR => {
            let ptr = payload as *const u8;
            if !ptr.is_null() {
                unsafe {
                    let mut len = 0;
                    while *ptr.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(ptr, len);
                    if let Ok(s) = std::str::from_utf8(slice) {
                        print!("{}", s);
                    }
                }
            }
        }
        _ => print!("<unknown:{}>", tag),
    }
}

/// String concatenation: returns (TAG_STR, pointer to new string)
/// Caller must ensure both arguments are strings.
/// The returned string is leaked (no GC in compiled mode).
#[no_mangle]
pub extern "C" fn lsp_str_concat(_tag1: i64, payload1: i64, _tag2: i64, payload2: i64) -> TaggedValue {
    let s1 = read_str(payload1);
    let s2 = read_str(payload2);
    let result = format!("{}{}", s1, s2);
    let ptr = leak_string(result);
    TaggedValue { tag: TAG_STR, payload: ptr as i64 }
}

/// Convert a value to string representation
#[no_mangle]
pub extern "C" fn lsp_to_string(tag: i64, payload: i64) -> TaggedValue {
    let s = match tag {
        TAG_NIL => "nil".to_string(),
        TAG_BOOL => if payload != 0 { "true" } else { "false" }.to_string(),
        TAG_INT => payload.to_string(),
        TAG_FLOAT => f64::from_bits(payload as u64).to_string(),
        TAG_STR => return TaggedValue { tag: TAG_STR, payload },
        _ => format!("<unknown:{}>", tag),
    };
    let ptr = leak_string(s);
    TaggedValue { tag: TAG_STR, payload: ptr as i64 }
}

fn read_str(payload: i64) -> &'static str {
    let ptr = payload as *const u8;
    if ptr.is_null() {
        return "";
    }
    unsafe {
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        std::str::from_utf8_unchecked(slice)
    }
}

fn leak_string(s: String) -> *const u8 {
    let mut bytes = s.into_bytes();
    bytes.push(0); // null-terminate
    let ptr = bytes.as_ptr();
    std::mem::forget(bytes);
    ptr
}
