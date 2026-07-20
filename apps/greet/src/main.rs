//! Stdin demo: prompt, read a line (console protocol v1), respond.

#![no_std]
#![no_main]

extern crate alloc;

use tinyos_app::{app, print, println, read_line, Env};

fn main(_env: Env) -> i32 {
    print!("What's your name? ");
    let Some(name) = read_line() else { return 1 };
    let name = name.trim();
    if name.is_empty() {
        println!("Hello, whoever you are!");
    } else {
        println!("Hello, {name}!");
    }
    0
}

app!(main);

// Reads a line and prints — console only.
tinyos_app::declare_caps!(b"console");
