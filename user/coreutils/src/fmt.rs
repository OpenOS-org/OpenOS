//! fmt — simple text formatter / line wrapper
//!
//! Usage: fmt [-w width] [file]
//!
//! Wraps lines to fit within the specified width (default 75 columns).
//! Preserves paragraph breaks (blank lines).

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

const DEFAULT_WIDTH: usize = 75;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";
    let width = DEFAULT_WIDTH;

    match fs::open(path) {
        Ok(fd) => {
            let mut buf = [0u8; 8192];
            match fs::read(fd, &mut buf) {
                Ok(n) => {
                    let data = &buf[..n];
                    fmt_text(data, width);
                }
                Err(_) => {
                    stderrln("fmt: read error");
                }
            }
            let _ = fs::close(fd);
        }
        Err(_) => {
            stderrln("fmt: no such file");
        }
    }
    exit(0);
}

fn fmt_text(data: &[u8], width: usize) {
    // Split into lines, then re-wrap
    let mut line_buf: [u8; 2048] = [0u8; 2048];
    let mut line_len: usize = 0;
    let mut start = 0;

    for i in 0..=data.len() {
        let at_end = i == data.len();
        let is_nl = !at_end && data[i] == b'\n';

        if at_end || is_nl {
            let line = &data[start..i];
            let is_blank = line.iter().all(|b| b.is_ascii_whitespace());

            if is_blank {
                // Flush any pending output, then print blank line
                if line_len > 0 {
                    flush_line(&line_buf[..line_len]);
                    line_len = 0;
                }
                stdout("\n");
            } else {
                // Add words from this line to the current output line
                let mut word_start = 0;
                let mut in_word = false;
                for j in 0..line.len() {
                    if line[j].is_ascii_whitespace() {
                        if in_word {
                            let word = &line[word_start..j];
                            in_word = false;

                            // Check if this word fits on the current line
                            let need = if line_len == 0 {
                                word.len()
                            } else {
                                word.len() + 1
                            };
                            if line_len > 0 && line_len + need > width {
                                flush_line(&line_buf[..line_len]);
                                line_len = 0;
                            }
                            if line_len > 0 {
                                line_buf[line_len] = b' ';
                                line_len += 1;
                            }
                            let end = core::cmp::min(line_len + word.len(), line_buf.len());
                            line_buf[line_len..end].copy_from_slice(&word[..end - line_len]);
                            line_len = end;
                        }
                    } else if !in_word {
                        word_start = j;
                        in_word = true;
                    }
                }
                // Handle last word on the line
                if in_word {
                    let word = &line[word_start..];
                    let need = if line_len == 0 {
                        word.len()
                    } else {
                        word.len() + 1
                    };
                    if line_len > 0 && line_len + need > width {
                        flush_line(&line_buf[..line_len]);
                        line_len = 0;
                    }
                    if line_len > 0 {
                        line_buf[line_len] = b' ';
                        line_len += 1;
                    }
                    let end = core::cmp::min(line_len + word.len(), line_buf.len());
                    line_buf[line_len..end].copy_from_slice(&word[..end - line_len]);
                    line_len = end;
                }
            }
            start = i + 1;
        }
    }
    // Flush remaining
    if line_len > 0 {
        flush_line(&line_buf[..line_len]);
    }
}

fn flush_line(data: &[u8]) {
    if let Ok(s) = core::str::from_utf8(data) {
        stdoutln(s);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
