//! tsort — topological sort
//!
//! Usage: tsort [file]
//!
//! Reads pairs of whitespace-separated tokens from input.
//! Each pair "A B" means A must come before B.
//! Outputs a topologically sorted order, one item per line.

#![no_std]
#![no_main]

mod common;

use common::{exit, stderrln, stdout, stdoutln};
use openos_sdk::fs;

/// Maximum number of unique nodes.
const MAX_NODES: usize = 256;
/// Maximum number of edges.
const MAX_EDGES: usize = 1024;

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let path = "/disk/test.txt";

    let fd = match fs::open(path) {
        Ok(fd) => fd,
        Err(_) => {
            stderrln("tsort: no such file");
            exit(1);
        }
    };

    let mut buf = [0u8; 8192];
    let n = fs::read(fd, &mut buf).unwrap_or(0);
    let _ = fs::close(fd);
    let data = &buf[..n];

    // Parse token pairs (A B) from the input.
    // Tokens are separated by whitespace (spaces, tabs, newlines).
    let (tokens, token_count) = tokenize(data);

    // Build adjacency list using index-based storage.
    let mut node_names: [&str; MAX_NODES] = [""; MAX_NODES];
    let mut node_count = 0usize;

    // edges[a] contains list of b indices where a -> b
    let mut edge_from: [usize; MAX_EDGES] = [0; MAX_EDGES];
    let mut edge_to: [usize; MAX_EDGES] = [0; MAX_EDGES];
    let mut edge_count = 0usize;

    // Process tokens as pairs
    let mut i = 0;
    while i + 1 < token_count {
        let a = tokens[i];
        let b = tokens[i + 1];

        let idx_a = find_or_add_node(&mut node_names, &mut node_count, a);
        let idx_b = find_or_add_node(&mut node_names, &mut node_count, b);

        if edge_count < MAX_EDGES {
            edge_from[edge_count] = idx_a;
            edge_to[edge_count] = idx_b;
            edge_count += 1;
        }

        i += 2;
    }

    // Kahn's algorithm for topological sort.
    // Compute in-degree for each node.
    let mut in_degree: [usize; MAX_NODES] = [0; MAX_NODES];
    for e in 0..edge_count {
        in_degree[edge_to[e]] += 1;
    }

    // Queue of nodes with zero in-degree.
    let mut queue: [usize; MAX_NODES] = [0; MAX_NODES];
    let mut q_head = 0usize;
    let mut q_tail = 0usize;

    for n in 0..node_count {
        if in_degree[n] == 0 {
            queue[q_tail] = n;
            q_tail += 1;
        }
    }

    let mut sorted_count = 0usize;

    while q_head < q_tail {
        let node = queue[q_head];
        q_head += 1;
        sorted_count += 1;

        stdoutln(node_names[node]);

        // Remove edges from this node
        for e in 0..edge_count {
            if edge_from[e] == node {
                let dest = edge_to[e];
                if in_degree[dest] > 0 {
                    in_degree[dest] -= 1;
                    if in_degree[dest] == 0 && q_tail < MAX_NODES {
                        queue[q_tail] = dest;
                        q_tail += 1;
                    }
                }
            }
        }
    }

    if sorted_count < node_count {
        stderrln("tsort: input contains a cycle");
    }

    exit(0);
}

/// Tokenize whitespace-separated data into a list of string slices.
fn tokenize<'a>(data: &'a [u8]) -> ([&'a str; MAX_EDGES], usize) {
    let mut tokens: [&str; MAX_EDGES] = [""; MAX_EDGES];
    let mut count = 0usize;
    let mut i = 0;

    while i < data.len() && count < MAX_EDGES {
        // Skip whitespace
        while i < data.len()
            && (data[i] == b' ' || data[i] == b'\t' || data[i] == b'\n' || data[i] == b'\r')
        {
            i += 1;
        }
        if i >= data.len() {
            break;
        }
        let start = i;
        while i < data.len()
            && data[i] != b' '
            && data[i] != b'\t'
            && data[i] != b'\n'
            && data[i] != b'\r'
        {
            i += 1;
        }
        if let Ok(s) = core::str::from_utf8(&data[start..i]) {
            tokens[count] = s;
            count += 1;
        }
    }
    (tokens, count)
}

/// Find a node by name or add it. Returns the index.
fn find_or_add_node<'a>(
    names: &mut [&'a str; MAX_NODES],
    count: &mut usize,
    name: &'a str,
) -> usize {
    for i in 0..*count {
        if names[i] == name {
            return i;
        }
    }
    if *count < MAX_NODES {
        names[*count] = name;
        let idx = *count;
        *count += 1;
        idx
    } else {
        0
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    openos_sdk::process::exit(1);
}
