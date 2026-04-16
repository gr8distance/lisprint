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
            let (data_ptr, count) = list_data(payload);
            print!("(");
            for i in 0..count {
                if i > 0 { print!(" "); }
                unsafe {
                    let t = *data_ptr.add(i * 2);
                    let p = *data_ptr.add(i * 2 + 1);
                    lsp_print(t, p);
                }
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

// --- List runtime ---
// Lists are stored as heap-allocated arrays: [count, tag0, payload0, tag1, payload1, ...]
// The payload of a TAG_LIST value is a pointer to such an array.

/// Get the count and data pointer from a list payload
fn list_data(payload: i64) -> (*const i64, usize) {
    let ptr = payload as *const i64;
    if ptr.is_null() {
        return (std::ptr::null(), 0);
    }
    unsafe {
        let count = *ptr as usize;
        (ptr.add(1), count)
    }
}

/// Allocate a new list: [count, tag0, payload0, ...]
fn alloc_list(elements: &[(i64, i64)]) -> i64 {
    let count = elements.len();
    let mut data: Vec<i64> = Vec::with_capacity(1 + count * 2);
    data.push(count as i64);
    for &(tag, payload) in elements {
        data.push(tag);
        data.push(payload);
    }
    let ptr = data.as_ptr();
    std::mem::forget(data);
    ptr as i64
}

/// (list elem1 elem2 ...) — create a list from stack-allocated elements
#[no_mangle]
pub extern "C" fn lsp_list_new(count: i64, elements: *const i64) -> TaggedValue {
    let count = count as usize;
    if count == 0 {
        return TaggedValue { tag: TAG_NIL, payload: 0 };
    }
    let mut pairs = Vec::with_capacity(count);
    unsafe {
        for i in 0..count {
            let tag = *elements.add(i * 2);
            let payload = *elements.add(i * 2 + 1);
            pairs.push((tag, payload));
        }
    }
    TaggedValue { tag: TAG_LIST, payload: alloc_list(&pairs) }
}

/// (cons elem list) — prepend element to list
#[no_mangle]
pub extern "C" fn lsp_cons(tag: i64, payload: i64, list_tag: i64, list_payload: i64) -> TaggedValue {
    let mut pairs = vec![(tag, payload)];
    if list_tag == TAG_LIST {
        let (data_ptr, count) = list_data(list_payload);
        unsafe {
            for i in 0..count {
                let t = *data_ptr.add(i * 2);
                let p = *data_ptr.add(i * 2 + 1);
                pairs.push((t, p));
            }
        }
    }
    // If list is nil, just return single-element list
    TaggedValue { tag: TAG_LIST, payload: alloc_list(&pairs) }
}

/// (first list) — get first element
#[no_mangle]
pub extern "C" fn lsp_first(tag: i64, payload: i64) -> TaggedValue {
    if tag == TAG_LIST {
        let (data_ptr, count) = list_data(payload);
        if count > 0 {
            unsafe {
                let t = *data_ptr;
                let p = *data_ptr.add(1);
                return TaggedValue { tag: t, payload: p };
            }
        }
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

/// (rest list) — get all but first element
#[no_mangle]
pub extern "C" fn lsp_rest(tag: i64, payload: i64) -> TaggedValue {
    if tag == TAG_LIST {
        let (data_ptr, count) = list_data(payload);
        if count <= 1 {
            return TaggedValue { tag: TAG_NIL, payload: 0 };
        }
        let mut pairs = Vec::with_capacity(count - 1);
        unsafe {
            for i in 1..count {
                let t = *data_ptr.add(i * 2);
                let p = *data_ptr.add(i * 2 + 1);
                pairs.push((t, p));
            }
        }
        return TaggedValue { tag: TAG_LIST, payload: alloc_list(&pairs) };
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

/// (count list) — get element count
#[no_mangle]
pub extern "C" fn lsp_count(tag: i64, payload: i64) -> TaggedValue {
    if tag == TAG_LIST {
        let (_, count) = list_data(payload);
        return TaggedValue { tag: TAG_INT, payload: count as i64 };
    }
    if tag == TAG_NIL {
        return TaggedValue { tag: TAG_INT, payload: 0 };
    }
    TaggedValue { tag: TAG_INT, payload: 0 }
}

/// (nth list idx) — get element at index
#[no_mangle]
pub extern "C" fn lsp_nth(list_tag: i64, list_payload: i64, _idx_tag: i64, idx_payload: i64) -> TaggedValue {
    if list_tag == TAG_LIST {
        let (data_ptr, count) = list_data(list_payload);
        let idx = idx_payload as usize;
        if idx < count {
            unsafe {
                let t = *data_ptr.add(idx * 2);
                let p = *data_ptr.add(idx * 2 + 1);
                return TaggedValue { tag: t, payload: p };
            }
        }
    }
    TaggedValue { tag: TAG_NIL, payload: 0 }
}

/// (empty? list) — check if list is empty
#[no_mangle]
pub extern "C" fn lsp_empty(tag: i64, payload: i64) -> TaggedValue {
    if tag == TAG_NIL {
        return TaggedValue { tag: TAG_BOOL, payload: 1 };
    }
    if tag == TAG_LIST {
        let (_, count) = list_data(payload);
        return TaggedValue { tag: TAG_BOOL, payload: if count == 0 { 1 } else { 0 } };
    }
    TaggedValue { tag: TAG_BOOL, payload: 1 }
}

/// (concat list1 list2) — concatenate two lists
#[no_mangle]
pub extern "C" fn lsp_concat(tag1: i64, payload1: i64, tag2: i64, payload2: i64) -> TaggedValue {
    let mut pairs = Vec::new();
    for (tag, payload) in [(tag1, payload1), (tag2, payload2)] {
        if tag == TAG_LIST {
            let (data_ptr, count) = list_data(payload);
            unsafe {
                for i in 0..count {
                    let t = *data_ptr.add(i * 2);
                    let p = *data_ptr.add(i * 2 + 1);
                    pairs.push((t, p));
                }
            }
        }
    }
    if pairs.is_empty() {
        return TaggedValue { tag: TAG_NIL, payload: 0 };
    }
    TaggedValue { tag: TAG_LIST, payload: alloc_list(&pairs) }
}
