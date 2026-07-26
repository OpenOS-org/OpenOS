//! md5sum — compute MD5 hash of files
//!
//! Usage: md5sum [file...]
//!
//! Pure Rust MD5 implementation.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const T: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

struct Md5 {
    state: [u32; 4],
    buf: [u8; 64],
    buf_len: usize,
    total_len: u64,
}

impl Md5 {
    fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            buf: [0u8; 64],
            buf_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.buf[self.buf_len] = byte;
            self.buf_len += 1;
            self.total_len += 1;
            if self.buf_len == 64 {
                self.compress();
                self.buf_len = 0;
            }
        }
    }

    fn compress(&mut self) {
        let mut m = [0u32; 16];
        for i in 0..16 {
            m[i] = u32::from_le_bytes([
                self.buf[i * 4],
                self.buf[i * 4 + 1],
                self.buf[i * 4 + 2],
                self.buf[i * 4 + 3],
            ]);
        }

        let [mut a, mut b, mut c, mut d] = self.state;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | ((!b) & d), i),
                16..=31 => ((d & b) | ((!d) & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | (!d)), (7 * i) % 16),
            };

            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(T[i]).wrapping_add(m[g])).rotate_left(S[i]),
            );
            a = temp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
    }

    fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.total_len * 8;
        // Padding
        self.buf[self.buf_len] = 0x80;
        self.buf_len += 1;
        if self.buf_len > 56 {
            while self.buf_len < 64 {
                self.buf[self.buf_len] = 0;
                self.buf_len += 1;
            }
            self.compress();
            self.buf_len = 0;
        }
        while self.buf_len < 56 {
            self.buf[self.buf_len] = 0;
            self.buf_len += 1;
        }
        // Append length in bits as little-endian u64
        let len_bytes = bit_len.to_le_bytes();
        self.buf[56..64].copy_from_slice(&len_bytes);
        self.compress();

        let mut result = [0u8; 16];
        for i in 0..4 {
            result[i * 4..i * 4 + 4].copy_from_slice(&self.state[i].to_le_bytes());
        }
        result
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    match fs::open(path) {
        Ok(fd) => {
            let mut md5 = Md5::new();
            let mut buf = [0u8; 4096];
            loop {
                match fs::read(fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => md5.update(&buf[..n]),
                    Err(_) => break,
                }
            }
            let _ = fs::close(fd);

            let hash = md5.finalize();
            print_hash(&hash);
            stdout("  ");
            stdoutln(path);
        }
        Err(_) => {
            stderrln("md5sum: no such file");
        }
    }
    exit(0);
}

fn print_hash(hash: &[u8; 16]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in hash {
        let hi = HEX[(byte >> 4) as usize];
        let lo = HEX[(byte & 0x0F) as usize];
        let pair = [hi, lo];
        if let Ok(s) = core::str::from_utf8(&pair) {
            stdout(s);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
