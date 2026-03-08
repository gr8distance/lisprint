//! Runtime support functions for compiled lisprint code.
//! These are linked into the final binary and called from generated code.

use super::compiler::{TAG_NIL, TAG_BOOL, TAG_INT, TAG_FLOAT, TAG_STR, TAG_LIST};

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
        TAG_LIST => {
            let items = get_list(payload);
            print!("(");
            for (i, (t, p)) in items.iter().enumerate() {
                if i > 0 { print!(" "); }
                lsp_print(*t, *p);
            }
            print!(")");
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
        TAG_LIST => {
            let items = get_list(payload);
            let parts: Vec<String> = items.iter().map(|(t, p)| {
                let tv = lsp_to_string(*t, *p);
                read_str(tv.payload).to_string()
            }).collect();
            format!("({})", parts.join(" "))
        }
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

// --- Data structure runtime ---

/// A runtime list is a leaked Vec of (tag, payload) pairs.
/// The payload of TAG_LIST is a pointer to a RuntimeList.
#[repr(C)]
struct RuntimeList {
    len: usize,
    data: *const (i64, i64),
}

fn make_list(items: Vec<(i64, i64)>) -> TaggedValue {
    let len = items.len();
    let boxed = items.into_boxed_slice();
    let data = Box::into_raw(boxed) as *const (i64, i64);
    let rl = Box::new(RuntimeList { len, data });
    let ptr = Box::into_raw(rl);
    TaggedValue { tag: TAG_LIST, payload: ptr as i64 }
}

fn get_list(payload: i64) -> &'static [(i64, i64)] {
    let ptr = payload as *const RuntimeList;
    if ptr.is_null() {
        return &[];
    }
    unsafe {
        let rl = &*ptr;
        std::slice::from_raw_parts(rl.data, rl.len)
    }
}

/// (cons elem list) → new list with elem prepended
#[no_mangle]
pub extern "C" fn lsp_cons(elem_tag: i64, elem_payload: i64, list_tag: i64, list_payload: i64) -> TaggedValue {
    let mut items = vec![(elem_tag, elem_payload)];
    if list_tag == TAG_LIST {
        items.extend_from_slice(get_list(list_payload));
    } else if list_tag == TAG_NIL {
        // cons onto nil → single-element list
    } else {
        // cons onto non-list: make a pair
        items.push((list_tag, list_payload));
    }
    make_list(items)
}

/// (first coll) → first element or nil
#[no_mangle]
pub extern "C" fn lsp_first(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    if coll_tag == TAG_LIST {
        let items = get_list(coll_payload);
        if let Some(&(tag, payload)) = items.first() {
            return TaggedValue { tag, payload };
        }
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

/// (rest coll) → list of remaining elements or empty list
#[no_mangle]
pub extern "C" fn lsp_rest(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    if coll_tag == TAG_LIST {
        let items = get_list(coll_payload);
        if items.len() > 1 {
            return make_list(items[1..].to_vec());
        }
    }
    make_list(vec![])
}

/// (nth coll idx) → element at index or nil
#[no_mangle]
pub extern "C" fn lsp_nth(coll_tag: i64, coll_payload: i64, _idx_tag: i64, idx_payload: i64) -> TaggedValue {
    if coll_tag == TAG_LIST {
        let items = get_list(coll_payload);
        let idx = idx_payload as usize;
        if idx < items.len() {
            let (tag, payload) = items[idx];
            return TaggedValue { tag, payload };
        }
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

/// (count coll) → integer length
#[no_mangle]
pub extern "C" fn lsp_count(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    let len = if coll_tag == TAG_LIST {
        get_list(coll_payload).len()
    } else if coll_tag == TAG_NIL {
        0
    } else if coll_tag == TAG_STR {
        let s = read_str(coll_payload);
        s.len()
    } else {
        0
    };
    TaggedValue { tag: TAG_INT, payload: len as i64 }
}

/// (empty? coll) → bool
#[no_mangle]
pub extern "C" fn lsp_empty_q(coll_tag: i64, coll_payload: i64) -> TaggedValue {
    let empty = if coll_tag == TAG_LIST {
        get_list(coll_payload).is_empty()
    } else if coll_tag == TAG_NIL {
        true
    } else {
        false
    };
    TaggedValue { tag: TAG_BOOL, payload: if empty { 1 } else { 0 } }
}

/// (concat a b) → combined list
#[no_mangle]
pub extern "C" fn lsp_concat(a_tag: i64, a_payload: i64, b_tag: i64, b_payload: i64) -> TaggedValue {
    let mut items = Vec::new();
    if a_tag == TAG_LIST {
        items.extend_from_slice(get_list(a_payload));
    } else if a_tag != TAG_NIL {
        items.push((a_tag, a_payload));
    }
    if b_tag == TAG_LIST {
        items.extend_from_slice(get_list(b_payload));
    } else if b_tag != TAG_NIL {
        items.push((b_tag, b_payload));
    }
    make_list(items)
}
