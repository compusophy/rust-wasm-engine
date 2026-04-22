#[derive(Copy, Clone)]
pub struct V3 { pub x: f32, pub y: f32, pub z: f32 }

impl V3 {
    pub const ZERO: V3 = V3::new(0.0, 0.0, 0.0);
    pub const ONE: V3 = V3::new(1.0, 1.0, 1.0);
    pub const Y: V3 = V3::new(0.0, 1.0, 0.0);
    pub const fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
    pub fn add(self, o: Self) -> Self { Self::new(self.x + o.x, self.y + o.y, self.z + o.z) }
    pub fn sub(self, o: Self) -> Self { Self::new(self.x - o.x, self.y - o.y, self.z - o.z) }
    pub fn scale(self, s: f32) -> Self { Self::new(self.x * s, self.y * s, self.z * s) }
    pub fn dot(self, o: Self) -> f32 { self.x * o.x + self.y * o.y + self.z * o.z }
    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn len(self) -> f32 { self.dot(self).sqrt() }
    pub fn norm(self) -> Self {
        let l = self.len();
        if l > 1e-6 { self.scale(1.0 / l) } else { self }
    }
}

pub struct M4(pub [f32; 16]);

impl M4 {
    pub fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> Self {
        let f = 1.0 / (fovy * 0.5).tan();
        let nf = 1.0 / (near - far);
        M4([
            f / aspect, 0.0, 0.0, 0.0,
            0.0, f, 0.0, 0.0,
            0.0, 0.0, (far + near) * nf, -1.0,
            0.0, 0.0, 2.0 * far * near * nf, 0.0,
        ])
    }

    pub fn look_at(eye: V3, target: V3, up: V3) -> Self {
        let f = target.sub(eye).norm();
        let s = f.cross(up).norm();
        let u = s.cross(f);
        M4([
            s.x, u.x, -f.x, 0.0,
            s.y, u.y, -f.y, 0.0,
            s.z, u.z, -f.z, 0.0,
            -s.dot(eye), -u.dot(eye), f.dot(eye), 1.0,
        ])
    }

    // Translate * RotateY(yaw) * Scale (column-major).
    pub fn trs(t: V3, yaw: f32, s: V3) -> Self {
        let c = yaw.cos(); let n = yaw.sin();
        M4([
             c * s.x, 0.0,  n * s.x, 0.0,
             0.0,     s.y,  0.0,     0.0,
            -n * s.z, 0.0,  c * s.z, 0.0,
             t.x,     t.y,  t.z,     1.0,
        ])
    }

    pub fn mul(&self, o: &M4) -> M4 {
        let a = &self.0; let b = &o.0;
        let mut r = [0.0f32; 16];
        for i in 0..4 {
            for j in 0..4 {
                let mut s = 0.0;
                for k in 0..4 { s += a[k * 4 + j] * b[i * 4 + k]; }
                r[i * 4 + j] = s;
            }
        }
        M4(r)
    }
}
