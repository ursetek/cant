use cant::Component;

#[derive(Component, Debug)]
struct Position(f32);

#[cant::system(hint(1000))]
#[resources(dt: f32)]
fn bad(_positions: &[Position]) {
    let _ = dt;
}

fn main() {}
