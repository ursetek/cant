use cant::Component;

#[derive(Component, Debug)]
struct Position(f32);

#[cant::system]
fn bad(_positions: Vec<Position>) {
    let _ = _positions;
}

fn main() {}
