use cant::Component;

#[derive(Component, Debug)]
struct Position(f32);

#[cant::system]
fn bad(_positions: &[Position]) -> u32 {
    0
}

fn main() {}
