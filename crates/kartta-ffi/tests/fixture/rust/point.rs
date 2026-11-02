pub struct Point {
    pub x: u32,
}

pub fn shift(point: &mut Point) -> u32 {
    point.x += 1;
    point.x
}
