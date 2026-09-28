#![allow(unused_imports)]

use cant::{Component, Entity};

#[derive(Component, Debug)]
struct Position(f32);

#[cant::system]
fn bad(entities: &mut [Entity]) {
    let _ = entities;
}

fn main() {}
