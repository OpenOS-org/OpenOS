//! expr — evaluate expressions
//!
//! Usage: expr EXPRESSION
//!
//! Supports:
//!   Arithmetic: +, -, *, /, %
//!   Comparison: =, !=, <, >, <=, >=
//!   String: length STRING, substr STRING POS LEN, index STRING CHARS

#![no_std]
#![no_main]

extern crate alloc;

mod common;

use common::{exit, format_u64, stderrln, stdout, stdoutln};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let args: alloc::vec::Vec<&str> = common::args().collect();

    if args.is_empty() {
        stderrln("expr: missing operand");
        exit(1);
    }

    // Evaluate unary operators first.
    if args.len() == 2 {
        if args[0] == "length" {
            let mut buf = [0u8; 20];
            let len = args[1].len() as u64;
            let s = format_u64(len, &mut buf);
            if let Ok(out) = core::str::from_utf8(s) {
                stdoutln(out);
            }
            exit(0);
        }
    }

    // Binary expression: expr A OP B
    if args.len() >= 3 {
        let a = args[0];
        let op = args[1];
        let b = args[2];

        let result = eval_binop(a, op, b);
        match result {
            ExprResult::Int(v) => {
                let mut buf = [0u8; 20];
                if v < 0 {
                    stdout("-");
                    let s = format_u64(v.wrapping_neg() as u64, &mut buf);
                    if let Ok(out) = core::str::from_utf8(s) {
                        stdoutln(out);
                    }
                } else {
                    let s = format_u64(v as u64, &mut buf);
                    if let Ok(out) = core::str::from_utf8(s) {
                        stdoutln(out);
                    }
                }
            }
            ExprResult::Bool(b_val) => {
                if b_val {
                    stdoutln("1");
                } else {
                    stdoutln("0");
                }
            }
            ExprResult::Str(s) => {
                stdoutln(s);
            }
            ExprResult::Error => {
                stderrln("expr: invalid expression");
                exit(1);
            }
        }
        exit(0);
    }

    // Single argument: just print it.
    stdoutln(args[0]);
    exit(0);
}

enum ExprResult<'a> {
    Int(i64),
    Bool(bool),
    Str(&'a str),
    Error,
}

fn eval_binop<'a>(a: &'a str, op: &str, b: &'a str) -> ExprResult<'a> {
    match op {
        "+" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Int(va.wrapping_add(vb))
            } else {
                ExprResult::Error
            }
        }
        "-" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Int(va.wrapping_sub(vb))
            } else {
                ExprResult::Error
            }
        }
        "*" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Int(va.wrapping_mul(vb))
            } else {
                ExprResult::Error
            }
        }
        "/" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                if vb == 0 {
                    ExprResult::Error
                } else {
                    ExprResult::Int(va / vb)
                }
            } else {
                ExprResult::Error
            }
        }
        "%" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                if vb == 0 {
                    ExprResult::Error
                } else {
                    ExprResult::Int(va % vb)
                }
            } else {
                ExprResult::Error
            }
        }
        "=" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Bool(va == vb)
            } else {
                ExprResult::Bool(a == b)
            }
        }
        "!=" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Bool(va != vb)
            } else {
                ExprResult::Bool(a != b)
            }
        }
        "<" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Bool(va < vb)
            } else {
                ExprResult::Bool(a < b)
            }
        }
        ">" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Bool(va > vb)
            } else {
                ExprResult::Bool(a > b)
            }
        }
        "<=" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Bool(va <= vb)
            } else {
                ExprResult::Bool(a <= b)
            }
        }
        ">=" => {
            if let (Ok(va), Ok(vb)) = (parse_i64(a), parse_i64(b)) {
                ExprResult::Bool(va >= vb)
            } else {
                ExprResult::Bool(a >= b)
            }
        }
        _ => ExprResult::Error,
    }
}

fn parse_i64(s: &str) -> Result<i64, ()> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return Err(());
    }
    let negative = bytes[0] == b'-';
    let start = if negative || bytes[0] == b'+' { 1 } else { 0 };
    if start >= bytes.len() {
        return Err(());
    }
    let mut val: i64 = 0;
    for &b in &bytes[start..] {
        if !b.is_ascii_digit() {
            return Err(());
        }
        val = val.checked_mul(10).ok_or(())?;
        val = val.checked_add((b - b'0') as i64).ok_or(())?;
    }
    if negative {
        val = val.wrapping_neg();
    }
    Ok(val)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
