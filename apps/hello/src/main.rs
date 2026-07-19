#![no_std]
#![no_main]

extern crate alloc;

use tinyos_app::{app, entry::Env, println};

fn main(env: Env) -> i32 {
    println!("hello from a third-party tinyOS app!");
    if env.args.is_empty() {
        println!("no arguments given");
    } else {
        println!("got {} argument(s):", env.args.len());
        for (i, a) in env.args.iter().enumerate() {
            println!("  [{i}] {a}");
        }
    }
    // Exit code = argument count, so `run hello a b c` shows "exited (code 3)".
    env.args.len() as i32
}

app!(main);
