use cant::Component;

#[derive(Component, Debug)]
struct Position(f32);

#[cant::system]
#[resources(x: f32)]
#[locals(x: f32)]
fn bad(_positions: &[Position]) {
    let _ = x;
}

fn main() {}
