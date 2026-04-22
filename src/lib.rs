// Zero-dependency Rust WASM first-person sandbox.
// Entity-driven renderer with per-object transforms, normals + sun/hemi lighting,
// indexed meshes via VAOs, skybox, particles, and procedural terrain.

#![allow(static_mut_refs)]

mod math;
mod gl;

use core::f32::consts::PI;
use math::*;
use gl::*;

// ============================================================
// SHADERS
// ============================================================

const SCENE_VS: &[u8] = b"#version 300 es
precision highp float;
layout(location=0) in vec3 a_pos;
layout(location=1) in vec3 a_normal;
layout(location=2) in vec3 a_color;
layout(location=3) in vec2 a_uv;
uniform mat4 u_vp;
uniform mat4 u_model;
uniform float u_sway;
uniform float u_time;
out vec3 v_world;
out vec3 v_normal;
out vec3 v_albedo;
out vec2 v_uv;
out float v_fog;
void main() {
    vec4 world = u_model * vec4(a_pos, 1.0);
    float sway_factor = max(a_pos.y, 0.0) * u_sway;
    world.x += sin(u_time * 1.6 + world.x * 0.18 + world.z * 0.11) * sway_factor * 0.35;
    world.z += cos(u_time * 1.3 + world.x * 0.14 - world.z * 0.22) * sway_factor * 0.28;
    gl_Position = u_vp * world;
    v_world = world.xyz;
    v_normal = normalize(mat3(u_model) * a_normal);
    v_albedo = a_color;
    v_uv = a_uv;
    v_fog = clamp(gl_Position.w / 180.0, 0.0, 1.0);
}";

const SCENE_FS: &[u8] = b"#version 300 es
precision highp float;
in vec3 v_world;
in vec3 v_normal;
in vec3 v_albedo;
in vec2 v_uv;
in float v_fog;
uniform vec3 u_tint;
uniform vec3 u_sun_dir;
uniform vec3 u_sun_color;
uniform vec3 u_sky_top;
uniform vec3 u_sky_bot;
uniform vec3 u_fog_color;
uniform vec3 u_camera_pos;
uniform float u_spec;
uniform float u_night;
uniform float u_flash;
uniform float u_rain;
uniform sampler2D u_tex;
uniform int u_has_tex;
out vec4 outColor;

vec3 tonemap(vec3 c) {
    // ACES-ish filmic curve.
    c *= 0.85;
    vec3 a = c * (2.51 * c + 0.03);
    vec3 b = c * (2.43 * c + 0.59) + 0.14;
    return clamp(a / b, 0.0, 1.0);
}

void main() {
    vec3 n = normalize(v_normal);
    vec3 base = v_albedo * u_tint;
    if (u_has_tex != 0) {
        vec4 t = texture(u_tex, v_uv);
        base = base * t.rgb;
    }

    // Sun lambert + soft wrap so backsides aren't pitch black.
    float ndl = dot(n, u_sun_dir);
    float wrap = max((ndl + 0.25) / 1.25, 0.0);
    // Storms mute the sun and desaturate the world slightly.
    float storm_mute = 1.0 - u_rain * 0.55;
    vec3 sun_lit = base * u_sun_color * wrap * storm_mute;

    // Hemisphere ambient (sky from above, ground bounce from below).
    vec3 hemi = mix(u_sky_bot * 0.55, u_sky_top, n.y * 0.5 + 0.5);
    vec3 ambient = base * hemi * 0.55;

    // Blinn-Phong specular, gated by sun visibility. Rain makes things glossier.
    vec3 v = normalize(u_camera_pos - v_world);
    vec3 h = normalize(u_sun_dir + v);
    float spec = pow(max(dot(n, h), 0.0), 36.0) * max(ndl, 0.0) * (u_spec + u_rain * 0.6);
    vec3 specular = u_sun_color * spec * storm_mute;

    vec3 lit = sun_lit + ambient + specular;

    // Lightning flash - nearly-white additive burst on exposed surfaces.
    lit += vec3(u_flash) * (0.35 + 0.65 * max(n.y, 0.0));

    // Height fog: thicker low to the ground, thinner up. Rain thickens fog.
    float h_fog = exp(-max(v_world.y + 1.5, 0.0) * 0.05);
    float fog_t = clamp(v_fog * (0.45 + 0.55 * h_fog) + u_rain * 0.15, 0.0, 1.0);
    lit = mix(lit, u_fog_color, fog_t);

    // Tonemap then gamma.
    vec3 mapped = tonemap(lit);
    outColor = vec4(pow(mapped, vec3(1.0 / 2.2)), 1.0);
}";

const SKY_VS: &[u8] = b"#version 300 es
precision highp float;
layout(location=0) in vec3 a_pos;
uniform mat4 u_sky_vp;
out vec3 v_dir;
void main() {
    v_dir = a_pos;
    vec4 p = u_sky_vp * vec4(a_pos, 1.0);
    gl_Position = p.xyww;
}";

const SKY_FS: &[u8] = b"#version 300 es
precision highp float;
in vec3 v_dir;
uniform vec3 u_sun_dir;
uniform vec3 u_sky_top;
uniform vec3 u_sky_bot;
uniform vec3 u_sun_color;
uniform float u_time;
uniform float u_rain;
uniform float u_flash;
out vec4 outColor;

float hash12(vec2 p) {
    p = fract(p * vec2(123.34, 345.45));
    p += dot(p, p + 34.345);
    return fract(p.x * p.y);
}
float noise2d(vec2 p) {
    vec2 i = floor(p), f = fract(p);
    float a = hash12(i);
    float b = hash12(i + vec2(1.0, 0.0));
    float c = hash12(i + vec2(0.0, 1.0));
    float d = hash12(i + vec2(1.0, 1.0));
    vec2 u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}
float fbm2(vec2 p) {
    float v = 0.0, a = 0.5;
    for (int i = 0; i < 5; i++) { v += a * noise2d(p); p *= 2.0; a *= 0.5; }
    return v;
}

vec3 tonemap(vec3 c) {
    c *= 0.85;
    vec3 a = c * (2.51 * c + 0.03);
    vec3 b = c * (2.43 * c + 0.59) + 0.14;
    return clamp(a / b, 0.0, 1.0);
}

void main() {
    vec3 d = normalize(v_dir);
    float t = smoothstep(-0.15, 0.6, d.y);
    vec3 col = mix(u_sky_bot, u_sky_top, t);
    float sd = max(dot(d, u_sun_dir), 0.0);
    // Sun disc + corona + halo.
    col += u_sun_color * (pow(sd, 1024.0) * 12.0 + pow(sd, 64.0) * 1.5 + pow(sd, 8.0) * 0.35);
    // Soft horizon glow toward the sun.
    float horizon = (1.0 - abs(d.y)) * pow(sd, 2.0);
    col += u_sun_color * horizon * 0.28;

    float night = smoothstep(0.15, -0.10, u_sun_dir.y);
    float day = 1.0 - night;

    // Clouds - project ray onto cloud plane overhead. Storms pump thickness + churn.
    if (d.y > 0.04) {
        vec2 cp = d.xz / max(d.y, 0.05) * 0.12 + vec2(u_time * (0.04 + u_rain * 0.14), u_time * (0.015 + u_rain * 0.08));
        float c = fbm2(cp);
        float lo = mix(0.48, 0.18, u_rain);
        float hi = mix(0.78, 0.55, u_rain);
        c = smoothstep(lo, hi, c);
        float fade = smoothstep(0.05, 0.28, d.y);
        // Sun-lit cloud underbellies during dawn/dusk; storms mute them toward grey.
        vec3 cloud_lit = mix(u_sky_bot * 0.85, vec3(1.0), c);
        cloud_lit = mix(cloud_lit, u_sun_color * 1.2, smoothstep(0.0, 0.5, sd) * 0.45);
        cloud_lit = mix(cloud_lit, vec3(0.22, 0.24, 0.28), u_rain * 0.75);
        col = mix(col, cloud_lit, c * fade * day * mix(0.82, 1.0, u_rain));
    }

    // Overall storm tint pulls sky toward slate grey.
    col = mix(col, vec3(0.22, 0.24, 0.30) * (0.4 + 0.6 * day), u_rain * 0.55);

    // Lightning flash bathes everything briefly.
    col += vec3(u_flash) * (0.35 + smoothstep(0.0, 0.5, d.y));

    // Stars - visible when sun is below the horizon.
    if (night > 0.0 && d.y > 0.02) {
        vec2 ang = vec2(atan(d.z, d.x), asin(clamp(d.y, -0.99, 0.99))) * 38.0;
        vec2 cell = floor(ang);
        vec2 f = fract(ang) - 0.5;
        float h = hash12(cell);
        if (h > 0.965) {
            float r = length(f);
            float tw = 0.45 + 0.55 * sin(h * 621.0 + u_time * 2.4);
            float g = smoothstep(0.38, 0.0, r) * tw;
            col += g * night * vec3(1.0, 0.96, 0.86);
        }
    }

    outColor = vec4(pow(tonemap(col), vec3(1.0 / 2.2)), 1.0);
}";

// ============================================================
// MESH
// ============================================================

#[derive(Copy, Clone)]
pub struct Mesh {
    pub vao: u32,
    pub vbo: u32,
    pub index_count: i32,
    pub index_ty: u32,
    // JS-side texture handle (0 = no texture, sampled color stays vertex-color only).
    pub texture: u32,
}

pub struct MeshBuilder {
    pub verts: Vec<f32>,  // pos(3) + normal(3) + color(3) + uv(2) = 11 floats
    pub indices: Vec<u16>,
}

impl MeshBuilder {
    pub fn new() -> Self { Self { verts: Vec::new(), indices: Vec::new() } }

    pub fn vertex(&mut self, p: V3, n: V3, c: V3) -> u16 {
        self.vertex_uv(p, n, c, 0.0, 0.0)
    }

    pub fn vertex_uv(&mut self, p: V3, n: V3, c: V3, u: f32, v: f32) -> u16 {
        let idx = (self.verts.len() / 11) as u16;
        self.verts.extend_from_slice(&[p.x,p.y,p.z, n.x,n.y,n.z, c.x,c.y,c.z, u, v]);
        idx
    }

    pub fn tri(&mut self, a: u16, b: u16, c: u16) {
        self.indices.extend_from_slice(&[a, b, c]);
    }

    pub fn upload(&self) -> Mesh { upload_mesh(&self.verts, &self.indices) }
}

fn upload_mesh(verts: &[f32], indices: &[u16]) -> Mesh {
    unsafe {
        let vao = gl_create_vertex_array();
        gl_bind_vertex_array(vao);

        let vbo = gl_create_buffer();
        gl_bind_buffer(GL_ARRAY_BUFFER, vbo);
        gl_buffer_data_f32(GL_ARRAY_BUFFER, verts.as_ptr(), verts.len() as u32, GL_STATIC_DRAW);

        let ibo = gl_create_buffer();
        gl_bind_buffer(GL_ELEMENT_ARRAY_BUFFER, ibo);
        gl_buffer_data_u16(GL_ELEMENT_ARRAY_BUFFER, indices.as_ptr(), indices.len() as u32, GL_STATIC_DRAW);

        setup_vertex_attribs();

        Mesh { vao, vbo, index_count: indices.len() as i32, index_ty: GL_UNSIGNED_SHORT, texture: 0 }
    }
}

fn upload_mesh_u32(verts: &[f32], indices: &[u32]) -> Mesh {
    unsafe {
        let vao = gl_create_vertex_array();
        gl_bind_vertex_array(vao);

        let vbo = gl_create_buffer();
        gl_bind_buffer(GL_ARRAY_BUFFER, vbo);
        gl_buffer_data_f32(GL_ARRAY_BUFFER, verts.as_ptr(), verts.len() as u32, GL_STATIC_DRAW);

        let ibo = gl_create_buffer();
        gl_bind_buffer(GL_ELEMENT_ARRAY_BUFFER, ibo);
        gl_buffer_data_u32(GL_ELEMENT_ARRAY_BUFFER, indices.as_ptr(), indices.len() as u32, GL_STATIC_DRAW);

        setup_vertex_attribs();

        Mesh { vao, vbo, index_count: indices.len() as i32, index_ty: GL_UNSIGNED_INT, texture: 0 }
    }
}

unsafe fn setup_vertex_attribs() {
    let stride = 44;  // pos(12) + normal(12) + color(12) + uv(8)
    gl_vertex_attrib_pointer(0, 3, GL_FLOAT, 0, stride, 0);
    gl_enable_vertex_attrib_array(0);
    gl_vertex_attrib_pointer(1, 3, GL_FLOAT, 0, stride, 12);
    gl_enable_vertex_attrib_array(1);
    gl_vertex_attrib_pointer(2, 3, GL_FLOAT, 0, stride, 24);
    gl_enable_vertex_attrib_array(2);
    gl_vertex_attrib_pointer(3, 2, GL_FLOAT, 0, stride, 36);
    gl_enable_vertex_attrib_array(3);
}

// ---- primitives ----

pub fn mesh_cube(half: f32) -> Mesh {
    // 3x3 grid per face with corner darkening for baked AO.
    let mut b = MeshBuilder::new();
    let h = half;
    // Each face: (origin, edge u, edge v, normal). origin is the (-,-) corner of the face.
    let faces: [(V3, V3, V3, V3); 6] = [
        // +X: u along -Z, v along +Y
        (V3::new( h,-h, h), V3::new(0.0, 0.0, -2.0 * h), V3::new(0.0, 2.0 * h, 0.0), V3::new( 1.0, 0.0, 0.0)),
        // -X: u along +Z, v along +Y
        (V3::new(-h,-h,-h), V3::new(0.0, 0.0,  2.0 * h), V3::new(0.0, 2.0 * h, 0.0), V3::new(-1.0, 0.0, 0.0)),
        // +Y: u along +X, v along -Z
        (V3::new(-h, h, h), V3::new(2.0 * h, 0.0, 0.0), V3::new(0.0, 0.0, -2.0 * h), V3::new(0.0, 1.0, 0.0)),
        // -Y: u along +X, v along +Z
        (V3::new(-h,-h,-h), V3::new(2.0 * h, 0.0, 0.0), V3::new(0.0, 0.0,  2.0 * h), V3::new(0.0,-1.0, 0.0)),
        // +Z: u along +X, v along +Y
        (V3::new(-h,-h, h), V3::new(2.0 * h, 0.0, 0.0), V3::new(0.0, 2.0 * h, 0.0), V3::new(0.0, 0.0, 1.0)),
        // -Z: u along -X, v along +Y
        (V3::new( h,-h,-h), V3::new(-2.0 * h, 0.0, 0.0), V3::new(0.0, 2.0 * h, 0.0), V3::new(0.0, 0.0,-1.0)),
    ];
    for (origin, eu, ev, normal) in faces.iter() {
        let base = (b.verts.len() / 11) as u16;
        for j in 0..3 {
            for i in 0..3 {
                let u = i as f32 * 0.5;
                let v = j as f32 * 0.5;
                let p = V3::new(
                    origin.x + eu.x * u + ev.x * v,
                    origin.y + eu.y * u + ev.y * v,
                    origin.z + eu.z * u + ev.z * v,
                );
                // AO: 1.0 at center, 0.62 at corners, ~0.85 at edge midpoints.
                let du = (u - 0.5).abs() * 2.0; // 0..1
                let dv = (v - 0.5).abs() * 2.0;
                let edge = du.max(dv);
                let corner = du * dv;
                let ao = 1.0 - 0.18 * edge - 0.20 * corner;
                b.vertex(p, *normal, V3::new(ao, ao, ao));
            }
        }
        // 8 triangles per face (2 per quad, 4 quads).
        let q = |i: u16, j: u16| base + j * 3 + i;
        for j in 0..2u16 {
            for i in 0..2u16 {
                let a = q(i,     j    );
                let bb = q(i + 1, j    );
                let c = q(i,     j + 1);
                let d = q(i + 1, j + 1);
                b.tri(a, bb, d);
                b.tri(a, d, c);
            }
        }
    }
    b.upload()
}

pub fn mesh_sphere(radius: f32, rings: u32, segments: u32) -> Mesh {
    let mut b = MeshBuilder::new();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * PI;
        let st = theta.sin(); let ct = theta.cos();
        for s in 0..=segments {
            let u = s as f32 / segments as f32;
            let phi = u * 2.0 * PI;
            let sp = phi.sin(); let cp = phi.cos();
            let n = V3::new(cp * st, ct, sp * st);
            let p = n.scale(radius);
            b.vertex(p, n, V3::ONE);
        }
    }
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            let c = (r + 1) * row + s;
            let a = a as u16; let c = c as u16;
            b.tri(a, c, a + 1);
            b.tri(a + 1, c, c + 1);
        }
    }
    b.upload()
}

pub fn mesh_cylinder(radius: f32, height: f32, segments: u32) -> Mesh {
    let mut b = MeshBuilder::new();
    let hh = height * 0.5;
    // Side ring (duplicate ends for sharp normal transition at caps).
    for s in 0..=segments {
        let u = s as f32 / segments as f32;
        let phi = u * 2.0 * PI;
        let cp = phi.cos(); let sp = phi.sin();
        let n = V3::new(cp, 0.0, sp);
        let p_top = V3::new(cp * radius, hh, sp * radius);
        let p_bot = V3::new(cp * radius, -hh, sp * radius);
        b.vertex(p_bot, n, V3::ONE);
        b.vertex(p_top, n, V3::ONE);
    }
    for s in 0..segments {
        let a = (s * 2) as u16;
        let bb = a + 1;
        let c = a + 2;
        let d = a + 3;
        b.tri(a, c, bb);
        b.tri(bb, c, d);
    }
    // Caps.
    let top_center = b.vertex(V3::new(0.0, hh, 0.0), V3::Y, V3::ONE);
    for s in 0..=segments {
        let u = s as f32 / segments as f32;
        let phi = u * 2.0 * PI;
        let cp = phi.cos(); let sp = phi.sin();
        b.vertex(V3::new(cp * radius, hh, sp * radius), V3::Y, V3::ONE);
    }
    for s in 0..segments {
        let a = top_center;
        let bb = top_center + 1 + s as u16;
        let c = top_center + 2 + s as u16;
        b.tri(a, bb, c);
    }
    let bot_center = b.vertex(V3::new(0.0, -hh, 0.0), V3::new(0.0, -1.0, 0.0), V3::ONE);
    for s in 0..=segments {
        let u = s as f32 / segments as f32;
        let phi = u * 2.0 * PI;
        let cp = phi.cos(); let sp = phi.sin();
        b.vertex(V3::new(cp * radius, -hh, sp * radius), V3::new(0.0, -1.0, 0.0), V3::ONE);
    }
    for s in 0..segments {
        let a = bot_center;
        let bb = bot_center + 2 + s as u16;
        let c = bot_center + 1 + s as u16;
        b.tri(a, bb, c);
    }
    b.upload()
}

pub fn mesh_plane(size: f32, color: V3) -> Mesh {
    let mut b = MeshBuilder::new();
    let h = size * 0.5;
    let n = V3::Y;
    let i0 = b.vertex(V3::new(-h, 0.0,  h), n, color);
    let i1 = b.vertex(V3::new( h, 0.0,  h), n, color);
    let i2 = b.vertex(V3::new( h, 0.0, -h), n, color);
    let i3 = b.vertex(V3::new(-h, 0.0, -h), n, color);
    b.tri(i0, i1, i2);
    b.tri(i0, i2, i3);
    b.upload()
}

pub fn mesh_terrain(size: f32, res: u32) -> Mesh {
    // Flat checkerboard grid — each cell gets its own 4 unshared verts so the
    // per-quad color stays sharp (shared verts would blend along edges).
    // A 60×60 grid at 2 m cells = 3600 quads = 14400 verts; u16 index range is
    // 65535 so we stay well under the cap.
    let mut verts: Vec<f32> = Vec::with_capacity((res * res * 4 * 11) as usize);
    let mut indices: Vec<u32> = Vec::with_capacity((res * res * 6) as usize);
    let half = size * 0.5;
    let cell = size / res as f32;
    let n = V3::new(0.0, 1.0, 0.0);
    let light = V3::new(0.42, 0.45, 0.42);
    let dark  = V3::new(0.30, 0.32, 0.30);
    let mut base_idx: u32 = 0;
    for j in 0..res {
        for i in 0..res {
            let x0 = -half + i as f32 * cell;
            let x1 = x0 + cell;
            let z0 = -half + j as f32 * cell;
            let z1 = z0 + cell;
            let checker = ((i + j) & 1) == 0;
            let c = if checker { light } else { dark };
            let pts = [
                V3::new(x0, 0.0, z0),
                V3::new(x1, 0.0, z0),
                V3::new(x0, 0.0, z1),
                V3::new(x1, 0.0, z1),
            ];
            for p in pts.iter() {
                verts.extend_from_slice(&[p.x, p.y, p.z, n.x, n.y, n.z, c.x, c.y, c.z, 0.0, 0.0]);
            }
            indices.extend_from_slice(&[base_idx, base_idx + 2, base_idx + 1,
                                        base_idx + 1, base_idx + 2, base_idx + 3]);
            base_idx += 4;
        }
    }
    upload_mesh_u32(&verts, &indices)
}

// ---- noise + terrain shape ----

fn hash2(x: i32, y: i32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374761393)
        .wrapping_add((y as u32).wrapping_mul(668265263));
    n ^= n >> 13;
    n = n.wrapping_mul(1274126177);
    n ^= n >> 16;
    (n & 0xFFFF) as f32 / 32767.5 - 1.0
}

fn smooth(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }

fn value_noise(x: f32, y: f32) -> f32 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let xf = x - xi as f32;
    let yf = y - yi as f32;
    let a = hash2(xi,     yi    );
    let b = hash2(xi + 1, yi    );
    let c = hash2(xi,     yi + 1);
    let d = hash2(xi + 1, yi + 1);
    let u = smooth(xf);
    let v = smooth(yf);
    let ab = a + u * (b - a);
    let cd = c + u * (d - c);
    ab + v * (cd - ab)
}

fn fbm(mut x: f32, mut y: f32) -> f32 {
    let mut sum = 0.0f32;
    let mut amp = 1.0f32;
    let mut norm = 0.0f32;
    for _ in 0..5 {
        sum += value_noise(x, y) * amp;
        norm += amp;
        x *= 2.0; y *= 2.0;
        amp *= 0.5;
    }
    sum / norm
}

pub fn terrain_height(_x: f32, _z: f32) -> f32 {
    // Flat ground plane — the scene is being rebuilt from scratch, user imports
    // proper assets rather than procedural geometry from here on.
    0.0
}

fn terrain_color(y: f32, n: V3) -> V3 {
    let slope = 1.0 - n.y.max(0.0);
    let sand  = V3::new(0.82, 0.74, 0.52);
    let grass = V3::new(0.34, 0.58, 0.28);
    let rock  = V3::new(0.52, 0.48, 0.44);
    let snow  = V3::new(0.92, 0.94, 0.96);

    let low  = if y < 0.3  { 1.0 } else { smoothstep01(0.9, 0.3, y) };
    let high = smoothstep01(3.0, 5.5, y);
    let very_high = smoothstep01(5.0, 7.0, y);

    let mut c = lerp(grass, sand, low);
    c = lerp(c, rock, high.max(smoothstep01(0.35, 0.75, slope)));
    c = lerp(c, snow, very_high * (1.0 - slope).max(0.0));
    c
}

fn smoothstep01(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: V3, b: V3, t: f32) -> V3 {
    V3::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t)
}

// ============================================================
// SCENE
// ============================================================

pub struct Entity {
    pub mesh: u32,
    pub pos: V3,
    pub yaw: f32,
    pub scale: V3,
    pub tint: V3,
    pub hidden: bool,
    pub sway: f32,
}

#[derive(Copy, Clone)]
pub struct AABB { pub min: V3, pub max: V3 }

pub struct Npc {
    pub pos: V3,
    pub dir_x: f32, pub dir_z: f32,
    pub timer: f32,
    pub phase: f32,
    pub base_scale: f32,
    pub entity_idx: usize,
    pub kind: u8, // 0 = slime, 1 = critter
}

static mut MESHES: Vec<Mesh> = Vec::new();
static mut ENTITIES: Vec<Entity> = Vec::new();
static mut COLLIDERS: Vec<AABB> = Vec::new();
static mut NPCS: Vec<Npc> = Vec::new();

// Tint used for a class's ranged-attack projectile color.
fn class_tint(class_: u8) -> V3 {
    match class_ {
        0 => V3::new(0.78, 0.34, 0.26),  // Warrior (steel-red)
        1 => V3::new(0.42, 0.68, 0.32),  // Hunter (forest-green)
        2 => V3::new(0.48, 0.42, 0.90),  // Mage (arcane-blue)
        _ => V3::new(0.60, 0.60, 0.60),
    }
}

pub struct RainDrop {
    pub pos: V3,
    pub vel: V3,
    pub life: f32,
}
static mut RAIN: Vec<RainDrop> = Vec::new();

pub struct Bird {
    pub pos: V3,
    pub vel: V3,
    pub entity_idx: usize,
    pub phase: f32,
}
static mut BIRDS: Vec<Bird> = Vec::new();

pub struct Particle {
    pub pos: V3,
    pub vel: V3,
    pub life: f32,
    pub max_life: f32,
    pub color: V3,
    pub size: f32,
    pub gravity: f32,
}
static mut PARTICLES: Vec<Particle> = Vec::new();

pub struct Projectile {
    pub pos: V3,
    pub vel: V3,
    pub life: f32,
    pub entity_idx: usize,
    pub tint: V3,
}
static mut PROJECTILES: Vec<Projectile> = Vec::new();

// ---- QUESTS ----

pub struct Objective {
    pub kind: u32,       // 0=kill, 1=chop, 2=reach, 3=place
    pub target: u32,     // kill: NPC kind; place: shape
    pub pos: V3,         // reach
    pub radius: f32,
    pub required: u32,
    pub progress: u32,
    pub desc: String,
}

pub struct Quest {
    pub id: String,
    pub title: String,
    pub desc: String,
    pub giver_pos: V3,
    pub giver_tint: V3,
    pub accepted: bool,
    pub turned_in: bool,
    pub objectives: Vec<Objective>,
    pub giver_entity_idx: usize,
    pub marker_entity_idx: usize,
}

static mut QUESTS: Vec<Quest> = Vec::new();
static mut QUEST_JSON: String = String::new();

// Strings coming from JS go through these staging buffers.
static mut BUF_TITLE: Vec<u8> = Vec::new();
static mut BUF_DESC: Vec<u8> = Vec::new();
static mut BUF_ID: Vec<u8> = Vec::new();
static mut BUF_OBJ_DESC: Vec<u8> = Vec::new();

#[derive(Copy, Clone)]
pub struct TreeRef {
    pub trunk_idx: usize,
    pub foliage_start: usize,
    pub foliage_count: usize,
    pub collider_idx: usize,
}
static mut TREES: Vec<TreeRef> = Vec::new();

fn spawn_burst(origin: V3, count: u32, up_bias: f32, spread: f32,
               color: V3, life: f32, size: f32, gravity: f32) {
    unsafe {
        for _ in 0..count {
            let vx = (rand01() - 0.5) * 2.0 * spread;
            let vz = (rand01() - 0.5) * 2.0 * spread;
            let vy = up_bias + (rand01() - 0.5) * spread;
            let life_j = life * (0.6 + rand01() * 0.6);
            PARTICLES.push(Particle {
                pos: origin,
                vel: V3::new(vx, vy, vz),
                life: life_j,
                max_life: life_j,
                color,
                size: size * (0.7 + rand01() * 0.6),
                gravity,
            });
        }
    }
}

unsafe fn update_particles(dt: f32) {
    let mut i = 0;
    while i < PARTICLES.len() {
        let alive = {
            let p = &mut PARTICLES[i];
            p.life -= dt;
            if p.life <= 0.0 {
                false
            } else {
                p.vel.y -= 18.0 * p.gravity * dt;
                p.vel.x *= 1.0 - dt * 1.2;
                p.vel.z *= 1.0 - dt * 1.2;
                p.pos.x += p.vel.x * dt;
                p.pos.y += p.vel.y * dt;
                p.pos.z += p.vel.z * dt;
                let floor = terrain_height(p.pos.x, p.pos.z);
                if p.pos.y < floor {
                    p.pos.y = floor;
                    p.vel.y *= -0.35;
                    p.vel.x *= 0.6;
                    p.vel.z *= 0.6;
                }
                true
            }
        };
        if !alive { PARTICLES.swap_remove(i); } else { i += 1; }
    }
}

unsafe fn spawn_projectile(origin: V3, dir: V3, tint: V3) {
    let entity_idx = ENTITIES.len();
    ENTITIES.push(Entity {
        mesh: 2, pos: origin, yaw: 0.0,
        scale: V3::new(0.22, 0.22, 0.22),
        tint,
        hidden: false, sway: 0.0,
    });
    PROJECTILES.push(Projectile {
        pos: origin,
        vel: dir.scale(28.0).add(V3::new(0.0, 1.5, 0.0)),
        life: 4.0,
        entity_idx,
        tint,
    });
    audio_beep(880.0, 0.06, 0.12);
}

unsafe fn update_projectiles(dt: f32) {
    let mut i = 0;
    while i < PROJECTILES.len() {
        let (die, hit_collider, npc_hit, final_pos, tint) = {
            let p = &mut PROJECTILES[i];
            p.life -= dt;
            let alive = p.life > 0.0;

            if alive {
                p.vel.y -= 18.0 * dt;
                p.pos.x += p.vel.x * dt;
                p.pos.y += p.vel.y * dt;
                p.pos.z += p.vel.z * dt;
            }

            let ty = terrain_height(p.pos.x, p.pos.z);
            let terrain_hit = p.pos.y < ty;

            let mut hit_idx: Option<usize> = None;
            if alive && !terrain_hit {
                for ci in 0..COLLIDERS.len() {
                    let c = &COLLIDERS[ci];
                    if p.pos.x > c.min.x && p.pos.x < c.max.x
                        && p.pos.y > c.min.y && p.pos.y < c.max.y
                        && p.pos.z > c.min.z && p.pos.z < c.max.z
                    {
                        hit_idx = Some(ci);
                        break;
                    }
                }
            }

            let mut npc_kill: Option<(usize, u8, V3, V3)> = None;
            if alive && !terrain_hit && hit_idx.is_none() {
                for ni in 0..NPCS.len() {
                    let n = &NPCS[ni];
                    let dx = n.pos.x - p.pos.x;
                    let ny_c = n.pos.y + n.base_scale * 0.7;
                    let dy = ny_c - p.pos.y;
                    let dz = n.pos.z - p.pos.z;
                    let d2 = dx * dx + dy * dy + dz * dz;
                    let hr = n.base_scale + 0.28;
                    if d2 < hr * hr {
                        let e_tint = ENTITIES[n.entity_idx].tint;
                        npc_kill = Some((ni, n.kind, V3::new(n.pos.x, ny_c, n.pos.z), e_tint));
                        break;
                    }
                }
            }

            ENTITIES[p.entity_idx].pos = p.pos;

            let die = !alive || terrain_hit || hit_idx.is_some() || npc_kill.is_some();
            (die, hit_idx, npc_kill, p.pos, p.tint)
        };

        if die {
            let p_idx = PROJECTILES[i].entity_idx;
            ENTITIES[p_idx].hidden = true;
            spawn_burst(final_pos, 8, 0.5, 2.0, tint, 0.4, 0.1, 1.0);
            audio_beep(200.0, 0.08, 0.14);

            if let Some(ci) = hit_collider {
                // Tree chop?
                let mut tree_found: Option<usize> = None;
                for ti in 0..TREES.len() {
                    if TREES[ti].collider_idx == ci { tree_found = Some(ti); break; }
                }
                if let Some(ti) = tree_found {
                    let tr = TREES[ti];
                    let pos = ENTITIES[tr.trunk_idx].pos;
                    let e_tint = ENTITIES[tr.trunk_idx].tint;
                    ENTITIES[tr.trunk_idx].hidden = true;
                    for k in 0..tr.foliage_count {
                        ENTITIES[tr.foliage_start + k].hidden = true;
                    }
                    COLLIDERS[tr.collider_idx] = disabled_aabb();
                    TREES.swap_remove(ti);
                    spawn_burst(pos, 24, 2.5, 3.5, e_tint, 0.9, 0.20, 1.0);
                    audio_beep(80.0, 0.5, 0.22);
                    record_chop();
                }
                let _ = ci;
            }

            if let Some((ni, kind, pos, e_tint)) = npc_hit {
                let eidx = NPCS[ni].entity_idx;
                ENTITIES[eidx].hidden = true;
                spawn_burst(pos, 18, 2.0, 3.0, e_tint, 0.6, 0.16, 1.0);
                audio_beep(150.0, 0.2, 0.2);
                NPCS.swap_remove(ni);
                record_kill(kind);
            }

            PROJECTILES.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

unsafe fn record_kill(kind: u8) {
    for q in QUESTS.iter_mut() {
        if !q.accepted || q.turned_in { continue; }
        for o in q.objectives.iter_mut() {
            if o.kind == 0 && o.target == kind as u32 && o.progress < o.required {
                o.progress += 1;
                audio_beep(620.0, 0.08, 0.12);
            }
        }
    }
}

unsafe fn record_chop() {
    for q in QUESTS.iter_mut() {
        if !q.accepted || q.turned_in { continue; }
        for o in q.objectives.iter_mut() {
            if o.kind == 1 && o.progress < o.required {
                o.progress += 1;
                audio_beep(620.0, 0.08, 0.12);
            }
        }
    }
}

unsafe fn update_quests(t: f32) {
    let (px, _py, pz) = if let Some(s) = S.as_ref() {
        (s.cam_x, s.cam_y, s.cam_z)
    } else { return };

    for q in QUESTS.iter_mut() {
        if q.turned_in {
            ENTITIES[q.giver_entity_idx].hidden = true;
            ENTITIES[q.marker_entity_idx].hidden = true;
            continue;
        }

        let dx = q.giver_pos.x - px;
        let dz = q.giver_pos.z - pz;
        let dist2 = dx * dx + dz * dz;
        let near = dist2 < 9.0;

        if !q.accepted && near {
            q.accepted = true;
            audio_beep(680.0, 0.30, 0.18);
        }

        // Reach objectives.
        for obj in q.objectives.iter_mut() {
            if obj.kind == 2 && obj.progress < obj.required {
                let odx = obj.pos.x - px;
                let odz = obj.pos.z - pz;
                let d2 = odx * odx + odz * odz;
                if d2 < obj.radius * obj.radius {
                    obj.progress = obj.required;
                    audio_beep(840.0, 0.18, 0.18);
                }
            }
        }

        let all_done = q.objectives.iter().all(|o| o.progress >= o.required);

        let marker_tint = if !q.accepted {
            V3::new(1.0, 0.85, 0.25)
        } else if all_done {
            V3::new(0.40, 0.95, 0.40)
        } else {
            V3::new(0.55, 0.55, 0.60)
        };
        let bob = (t * 2.5 + q.giver_pos.x * 0.5).sin() * 0.18;
        ENTITIES[q.marker_entity_idx].tint = marker_tint;
        ENTITIES[q.marker_entity_idx].pos.y = q.giver_pos.y + 2.6 + bob;

        if q.accepted && all_done && near {
            q.turned_in = true;
            audio_beep(880.0, 0.45, 0.22);
            spawn_burst(q.giver_pos.add(V3::new(0.0, 1.5, 0.0)), 30, 3.0, 4.0,
                V3::new(1.0, 0.92, 0.42), 1.0, 0.22, 0.25);
        }
    }
}

unsafe fn spawn_birds(n: u32) {
    for i in 0..n {
        let a = (i as f32 / n as f32) * PI * 2.0;
        let r = 18.0 + rand01() * 22.0;
        let pos = V3::new(a.cos() * r, 28.0 + rand01() * 10.0, a.sin() * r);
        let v_a = a + PI * 0.5 + (rand01() - 0.5) * 0.6;
        let vel = V3::new(v_a.cos() * 4.5, (rand01() - 0.5) * 0.4, v_a.sin() * 4.5);
        let tint = V3::new(
            0.10 + rand01() * 0.10,
            0.12 + rand01() * 0.08,
            0.14 + rand01() * 0.08,
        );
        let entity_idx = ENTITIES.len();
        ENTITIES.push(Entity {
            mesh: 1,
            pos, yaw: 0.0,
            scale: V3::new(0.45, 0.12, 0.25),
            tint,
            hidden: false, sway: 0.0,
        });
        BIRDS.push(Bird { pos, vel, entity_idx, phase: rand01() * PI * 2.0 });
    }
}

unsafe fn update_birds(dt: f32, rain: f32) {
    if BIRDS.is_empty() { return; }
    let n = BIRDS.len();
    // Two-pass boids: compute steer forces from snapshot, then integrate.
    const R_COH: f32 = 9.0;
    const R_ALI: f32 = 7.0;
    const R_SEP: f32 = 2.2;
    const MAX_SPEED: f32 = 7.5;
    const MIN_SPEED: f32 = 3.5;

    let mut steers: Vec<V3> = Vec::with_capacity(n);
    for i in 0..n {
        let me = &BIRDS[i];
        let mut coh = V3::ZERO; let mut coh_n = 0.0f32;
        let mut ali = V3::ZERO; let mut ali_n = 0.0f32;
        let mut sep = V3::ZERO;
        for j in 0..n {
            if i == j { continue; }
            let o = &BIRDS[j];
            let d = o.pos.sub(me.pos);
            let d2 = d.x*d.x + d.y*d.y + d.z*d.z;
            if d2 < R_COH * R_COH { coh = coh.add(o.pos); coh_n += 1.0; }
            if d2 < R_ALI * R_ALI { ali = ali.add(o.vel); ali_n += 1.0; }
            if d2 < R_SEP * R_SEP && d2 > 1e-4 {
                sep = sep.sub(d.scale(1.0 / d2));
            }
        }
        let mut steer = V3::ZERO;
        if coh_n > 0.0 {
            let c = V3::new(coh.x / coh_n, coh.y / coh_n, coh.z / coh_n);
            steer = steer.add(c.sub(me.pos).scale(0.9));
        }
        if ali_n > 0.0 {
            let a = V3::new(ali.x / ali_n, ali.y / ali_n, ali.z / ali_n);
            steer = steer.add(a.sub(me.vel).scale(1.2));
        }
        steer = steer.add(sep.scale(3.0));
        // Gentle pull toward a band of sky over the origin.
        let target = V3::new(0.0, 32.0 + (me.phase + me.pos.x * 0.02).sin() * 4.0, 0.0);
        steer = steer.add(target.sub(me.pos).scale(0.08));
        // Rain: sink toward ground, thin out.
        if rain > 0.2 {
            steer.y -= rain * 6.0;
        }
        steers.push(steer);
    }

    for i in 0..n {
        let s = steers[i];
        let b = &mut BIRDS[i];
        b.vel = b.vel.add(s.scale(dt));
        // Clamp speed.
        let sp2 = b.vel.x*b.vel.x + b.vel.y*b.vel.y + b.vel.z*b.vel.z;
        let sp = sp2.sqrt();
        if sp > MAX_SPEED { b.vel = b.vel.scale(MAX_SPEED / sp); }
        else if sp < MIN_SPEED && sp > 1e-3 { b.vel = b.vel.scale(MIN_SPEED / sp); }
        b.pos.x += b.vel.x * dt;
        b.pos.y += b.vel.y * dt;
        b.pos.z += b.vel.z * dt;
        // Wrap to a 120m sky cylinder around origin.
        let r2 = b.pos.x*b.pos.x + b.pos.z*b.pos.z;
        if r2 > 120.0 * 120.0 {
            let l = r2.sqrt();
            b.pos.x *= 100.0 / l;
            b.pos.z *= 100.0 / l;
        }
        b.phase += dt * 9.0;
        let wing = b.phase.sin() * 0.08;
        let yaw = (-b.vel.x).atan2(-b.vel.z);
        let e = &mut ENTITIES[b.entity_idx];
        let hide = rain > 0.55 && b.pos.y < 18.0;
        e.hidden = hide;
        e.pos = V3::new(b.pos.x, b.pos.y + wing, b.pos.z);
        e.yaw = yaw;
        e.scale = V3::new(0.45 + wing.abs() * 0.8, 0.10 + wing.abs() * 0.20, 0.25);
    }
}

unsafe fn update_weather(dt: f32) {
    let (cam_x, cam_y, cam_z) = {
        let ss = match S.as_ref() { Some(s) => s, None => return };
        (ss.cam_x, ss.cam_y, ss.cam_z)
    };
    let s = match S.as_mut() { Some(s) => s, None => return };
    s.storm_t += dt;

    // Auto cycle: clear ~2.5min, ramp up, storm ~1min, ramp down.
    let cycle = 240.0f32;
    let phase = s.storm_t - (s.storm_t / cycle).floor() * cycle;
    let auto = if phase > 140.0 && phase < 220.0 {
        smoothstep01(140.0, 160.0, phase) * (1.0 - smoothstep01(200.0, 220.0, phase))
    } else { 0.0 };
    let target = if s.storm_force { 1.0 } else { auto };
    let ease = (dt * 0.6).min(1.0);
    s.rain += (target - s.rain) * ease;

    // Decay flash.
    s.flash = (s.flash - dt * 7.0).max(0.0);

    // Random lightning strikes during heavy rain.
    if s.rain > 0.55 && s.flash < 0.05 && rand01() < 0.003 {
        s.flash = 1.0;
        s.thunder_pending = 0.25 + rand01() * 1.1;
    }

    // Deferred thunder audio.
    if s.thunder_pending > 0.0 {
        s.thunder_pending -= dt;
        if s.thunder_pending <= 0.0 {
            audio_beep(55.0, 1.6, 0.26);
            audio_beep(90.0, 1.0, 0.18);
        }
    }

    // Rain spawns proportional to intensity; cap total drops.
    let spawn_rate = s.rain * 260.0;
    s.rain_spawn_acc += spawn_rate * dt;
    while s.rain_spawn_acc >= 1.0 && RAIN.len() < 900 {
        s.rain_spawn_acc -= 1.0;
        let a = rand01() * PI * 2.0;
        let r = rand01().sqrt() * 26.0;
        let x = cam_x + a.cos() * r;
        let z = cam_z + a.sin() * r;
        let y = cam_y + 12.0 + rand01() * 8.0;
        RAIN.push(RainDrop {
            pos: V3::new(x, y, z),
            vel: V3::new((rand01() - 0.5) * 1.5, -18.0 - rand01() * 6.0, (rand01() - 0.5) * 1.5),
            life: 2.5,
        });
    }

    // Integrate raindrops.
    let mut i = 0;
    while i < RAIN.len() {
        let drop = &mut RAIN[i];
        drop.life -= dt;
        drop.pos.x += drop.vel.x * dt;
        drop.pos.y += drop.vel.y * dt;
        drop.pos.z += drop.vel.z * dt;
        let floor = terrain_height(drop.pos.x, drop.pos.z);
        let hit_ground = drop.pos.y < floor;
        let too_far = {
            let dx = drop.pos.x - cam_x;
            let dz = drop.pos.z - cam_z;
            (dx*dx + dz*dz) > 40.0 * 40.0
        };
        if drop.life <= 0.0 || hit_ground || too_far { RAIN.swap_remove(i); }
        else { i += 1; }
    }
    let _ = cam_y;
}

unsafe fn render_rain(s: &State) {
    if RAIN.is_empty() { return; }
    let sh = &s.scene_shader;
    gl_uniform1f(sh.u_sway, 0.0);
    gl_uniform1f(sh.u_spec, 0.0);
    gl_uniform1i(sh.u_has_tex, 0);
    let cube = MESHES[1];
    gl_bind_vertex_array(cube.vao);
    let tint = V3::new(0.72, 0.82, 0.95);
    gl_uniform3f(sh.u_tint, tint.x, tint.y, tint.z);
    for d in RAIN.iter() {
        let model = M4::trs(d.pos, 0.0, V3::new(0.03, 0.42, 0.03));
        gl_uniform_matrix4fv(sh.u_model, model.0.as_ptr());
        gl_draw_elements(GL_TRIANGLES, cube.index_count, cube.index_ty, 0);
    }
}

unsafe fn render_particles(s: &State) {
    if PARTICLES.is_empty() { return; }
    let sh = &s.scene_shader;
    gl_uniform1f(sh.u_sway, 0.0);
    gl_uniform1i(sh.u_has_tex, 0);
    let cube = MESHES[1];
    gl_bind_vertex_array(cube.vao);
    for p in PARTICLES.iter() {
        let fade = (p.life / p.max_life).clamp(0.0, 1.0);
        let sz = p.size * (0.4 + 0.6 * fade);
        let model = M4::trs(p.pos, 0.0, V3::new(sz, sz, sz));
        gl_uniform_matrix4fv(sh.u_model, model.0.as_ptr());
        gl_uniform3f(sh.u_tint, p.color.x, p.color.y, p.color.z);
        gl_draw_elements(GL_TRIANGLES, cube.index_count, cube.index_ty, 0);
    }
}

static mut RNG: u32 = 0x9E37_79B9;

// Preview mode: when non-zero, `frame()` renders just the player avatar on a
// dark background with a fixed orbit camera (used by the character-creation
// screen) instead of the live world. Movement input is ignored too so WASD in
// the UI doesn't drag the world player around.
static mut PREVIEW_ACTIVE: bool = false;

#[no_mangle]
pub extern "C" fn preview_mode(on: u32) {
    unsafe { PREVIEW_ACTIVE = on != 0; }
}

unsafe fn render_preview(s: &State, _dt: f32) {
    // Place the rigged avatar at the origin. Camera slowly orbits around it
    // (auto-rotate) so the creation screen has some motion.
    let feet_y = 0.0f32;
    let t = s.t;
    let spin = t * 0.35;
    ENTITIES[s.player_body_idx].pos = V3::new(0.0, feet_y, 0.0);
    // Camera orbits the character at (sin*dist, y, cos*dist). Mesh forward
    // is +Z in its local frame, so yaw = spin puts the mesh front-face toward
    // the orbiting camera at every angle — no more looking at the back of
    // the character on the creation screen.
    ENTITIES[s.player_body_idx].yaw = spin;
    ENTITIES[s.player_body_idx].hidden = false;

    let body_mid_y = 0.95;
    let dist = 3.4;
    let cam = V3::new(spin.sin() * dist, body_mid_y + 0.25, spin.cos() * dist);
    let target = V3::new(0.0, body_mid_y, 0.0);

    let aspect = s.canvas_w as f32 / s.canvas_h as f32;
    let proj = M4::perspective(1.0, aspect, 0.1, 50.0);
    let view = M4::look_at(cam, target, V3::Y);
    let vp = proj.mul(&view);

    gl_clear_color(0.05, 0.06, 0.09, 1.0);
    gl_viewport(0, 0, s.canvas_w as i32, s.canvas_h as i32);
    gl_clear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);

    let sh = &s.scene_shader;
    gl_use_program(sh.program);
    gl_uniform_matrix4fv(sh.u_vp, vp.0.as_ptr());
    let sun_dir = V3::new(0.35, 0.85, 0.40).norm();
    let sun_color = V3::new(1.35, 1.20, 1.00);
    let sky_top = V3::new(0.78, 0.82, 0.92);
    let sky_bot = V3::new(0.62, 0.58, 0.52);
    gl_uniform3f(sh.u_sun_dir, sun_dir.x, sun_dir.y, sun_dir.z);
    gl_uniform3f(sh.u_sun_color, sun_color.x, sun_color.y, sun_color.z);
    gl_uniform3f(sh.u_sky_top, sky_top.x, sky_top.y, sky_top.z);
    gl_uniform3f(sh.u_sky_bot, sky_bot.x, sky_bot.y, sky_bot.z);
    gl_uniform3f(sh.u_fog_color, 0.20, 0.22, 0.28);
    gl_uniform3f(sh.u_camera_pos, cam.x, cam.y, cam.z);
    gl_uniform1f(sh.u_time, s.t);
    gl_uniform1f(sh.u_night, 0.0);
    gl_uniform1f(sh.u_flash, 0.0);
    gl_uniform1f(sh.u_rain, 0.0);
    gl_uniform1i(sh.u_tex, 0);
    gl_active_texture(GL_TEXTURE0);

    let e = &ENTITIES[s.player_body_idx];
    if !e.hidden {
        let m = MESHES[e.mesh as usize];
        let model = M4::trs(e.pos, e.yaw, e.scale);
        gl_uniform_matrix4fv(sh.u_model, model.0.as_ptr());
        gl_uniform3f(sh.u_tint, e.tint.x, e.tint.y, e.tint.z);
        gl_uniform1f(sh.u_sway, 0.0);
        gl_uniform1f(sh.u_spec, 0.15);
        if m.texture != 0 {
            gl_bind_texture(GL_TEXTURE_2D, m.texture);
            gl_uniform1i(sh.u_has_tex, 1);
        } else {
            gl_uniform1i(sh.u_has_tex, 0);
        }
        gl_bind_vertex_array(m.vao);
        gl_draw_elements(GL_TRIANGLES, m.index_count, m.index_ty, 0);
    }
    NAMEPLATE[2] = 0.0;  // hide nameplate during preview
}

// HUD stats read by JS each frame: [x, y, z, yaw, day_t, tps, body_yaw, freecam, rain, flash, grounded, storm_force]
static mut STATS: [f32; 12] = [0.0; 12];

// Character nameplate in screen space: [ndc_x, ndc_y, visible_flag].
static mut NAMEPLATE: [f32; 3] = [0.0; 3];

#[no_mangle]
pub extern "C" fn nameplate_ptr() -> *const f32 {
    unsafe { NAMEPLATE.as_ptr() }
}

unsafe fn project_nameplate(vp: &M4, world: V3) {
    let m = &vp.0;
    let cx = m[0] * world.x + m[4] * world.y + m[8]  * world.z + m[12];
    let cy = m[1] * world.x + m[5] * world.y + m[9]  * world.z + m[13];
    let cw = m[3] * world.x + m[7] * world.y + m[11] * world.z + m[15];
    if cw > 0.05 {
        NAMEPLATE[0] = cx / cw;
        NAMEPLATE[1] = cy / cw;
        NAMEPLATE[2] = 1.0;
    } else {
        NAMEPLATE[2] = 0.0;
    }
}

#[no_mangle]
pub extern "C" fn stats_ptr() -> *const f32 {
    unsafe { STATS.as_ptr() }
}

#[no_mangle]
static mut MESH_VERT_BUF: Vec<f32> = Vec::new();
static mut MESH_IDX_BUF: Vec<u16> = Vec::new();
static mut MESH_IDX_BUF_U32: Vec<u32> = Vec::new();

#[no_mangle]
pub extern "C" fn mesh_vert_buf(n_floats: u32) -> *mut f32 {
    unsafe { MESH_VERT_BUF.resize(n_floats as usize, 0.0); MESH_VERT_BUF.as_mut_ptr() }
}
#[no_mangle]
pub extern "C" fn mesh_idx_buf(n_u16s: u32) -> *mut u16 {
    unsafe { MESH_IDX_BUF.resize(n_u16s as usize, 0); MESH_IDX_BUF.as_mut_ptr() }
}
#[no_mangle]
pub extern "C" fn mesh_idx_buf_u32(n_u32s: u32) -> *mut u32 {
    unsafe { MESH_IDX_BUF_U32.resize(n_u32s as usize, 0); MESH_IDX_BUF_U32.as_mut_ptr() }
}
#[no_mangle]
pub extern "C" fn mesh_upload(vert_count: u32, index_count: u32) -> u32 {
    unsafe {
        let verts = &MESH_VERT_BUF[..(vert_count as usize) * 11];
        let indices = &MESH_IDX_BUF[..index_count as usize];
        let mesh = upload_mesh(verts, indices);
        MESHES.push(mesh);
        (MESHES.len() - 1) as u32
    }
}
#[no_mangle]
pub extern "C" fn mesh_upload_u32(vert_count: u32, index_count: u32) -> u32 {
    unsafe {
        let verts = &MESH_VERT_BUF[..(vert_count as usize) * 11];
        let indices = &MESH_IDX_BUF_U32[..index_count as usize];
        let mesh = upload_mesh_u32(verts, indices);
        MESHES.push(mesh);
        (MESHES.len() - 1) as u32
    }
}

// Re-upload a mesh's vertex buffer in-place (no new VAO/VBO). The new data must
// use the same 11-float layout; the caller has already staged it in MESH_VERT_BUF.
// This is how the CPU skinner pushes each posed frame for the charcreate preview
// without leaking WebGL resources.
#[no_mangle]
pub extern "C" fn mesh_update_verts(mesh_id: u32, vert_count: u32) {
    unsafe {
        let m = match MESHES.get(mesh_id as usize) { Some(m) => *m, None => return };
        let n = (vert_count as usize) * 11;
        if n > MESH_VERT_BUF.len() { return; }
        let verts = &MESH_VERT_BUF[..n];
        gl_bind_buffer(GL_ARRAY_BUFFER, m.vbo);
        gl_buffer_data_f32(GL_ARRAY_BUFFER, verts.as_ptr(), verts.len() as u32, GL_STATIC_DRAW);
    }
}

#[no_mangle]
pub extern "C" fn set_entity_mesh(entity_idx: u32, mesh_id: u32) {
    unsafe {
        if let Some(e) = ENTITIES.get_mut(entity_idx as usize) {
            e.mesh = mesh_id;
        }
    }
}

#[no_mangle]
pub extern "C" fn set_entity_hidden(entity_idx: u32, hidden: u32) {
    unsafe {
        if let Some(e) = ENTITIES.get_mut(entity_idx as usize) {
            e.hidden = hidden != 0;
        }
    }
}

#[no_mangle]
pub extern "C" fn set_entity_tint(entity_idx: u32, r: f32, g: f32, b: f32) {
    unsafe {
        if let Some(e) = ENTITIES.get_mut(entity_idx as usize) {
            e.tint = V3::new(r, g, b);
        }
    }
}

#[no_mangle]
pub extern "C" fn set_entity_scale(entity_idx: u32, x: f32, y: f32, z: f32) {
    unsafe {
        if let Some(e) = ENTITIES.get_mut(entity_idx as usize) {
            e.scale = V3::new(x, y, z);
        }
    }
}

#[no_mangle]
pub extern "C" fn set_entity_pos(entity_idx: u32, x: f32, y: f32, z: f32) {
    unsafe {
        if let Some(e) = ENTITIES.get_mut(entity_idx as usize) {
            e.pos = V3::new(x, y, z);
        }
    }
}

#[no_mangle]
pub extern "C" fn set_entity_yaw(entity_idx: u32, yaw: f32) {
    unsafe {
        if let Some(e) = ENTITIES.get_mut(entity_idx as usize) {
            e.yaw = yaw;
        }
    }
}

// Allocate a fresh entity slot backed by `mesh_id`. Used by the multiplayer
// client to spawn a body for each remote player. Returns the entity index.
#[no_mangle]
pub extern "C" fn spawn_entity(mesh_id: u32) -> u32 {
    unsafe {
        let idx = ENTITIES.len() as u32;
        ENTITIES.push(Entity {
            mesh: mesh_id, pos: V3::ZERO, yaw: 0.0,
            scale: V3::new(1.0, 1.0, 1.0),
            tint: V3::new(1.0, 1.0, 1.0),
            hidden: true, sway: 0.0,
        });
        idx
    }
}

#[no_mangle]
pub extern "C" fn set_mesh_texture(mesh_id: u32, tex_handle: u32) {
    unsafe {
        if let Some(m) = MESHES.get_mut(mesh_id as usize) {
            m.texture = tex_handle;
        }
    }
}

#[no_mangle]
pub extern "C" fn player_body_entity() -> u32 {
    unsafe { S.as_ref().map(|s| s.player_body_idx as u32).unwrap_or(0) }
}

#[no_mangle]
pub extern "C" fn apply_character(class_: u32) {
    unsafe {
        let s = match S.as_mut() { Some(s) => s, None => return };
        s.class_ = class_ as u8;
        // The rigged Meshy mesh bakes its own colors + proportions, so the body
        // entity stays at identity transform and a neutral white tint — shading
        // comes from vertex colors and the base-color texture on the mesh.
        let body_e = &mut ENTITIES[s.player_body_idx];
        body_e.tint = V3::new(1.0, 1.0, 1.0);
        body_e.scale = V3::new(1.0, 1.0, 1.0);
    }
}

#[no_mangle]
pub extern "C" fn buf_title(len: u32) -> *mut u8 {
    unsafe { BUF_TITLE.resize(len as usize, 0); BUF_TITLE.as_mut_ptr() }
}
#[no_mangle]
pub extern "C" fn buf_desc(len: u32) -> *mut u8 {
    unsafe { BUF_DESC.resize(len as usize, 0); BUF_DESC.as_mut_ptr() }
}
#[no_mangle]
pub extern "C" fn buf_id(len: u32) -> *mut u8 {
    unsafe { BUF_ID.resize(len as usize, 0); BUF_ID.as_mut_ptr() }
}
#[no_mangle]
pub extern "C" fn buf_obj_desc(len: u32) -> *mut u8 {
    unsafe { BUF_OBJ_DESC.resize(len as usize, 0); BUF_OBJ_DESC.as_mut_ptr() }
}

fn buf_to_string(buf: &[u8]) -> String {
    match core::str::from_utf8(buf) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(buf).to_string(),
    }
}

#[no_mangle]
pub extern "C" fn quests_clear() {
    unsafe {
        for q in QUESTS.iter() {
            ENTITIES[q.giver_entity_idx].hidden = true;
            ENTITIES[q.marker_entity_idx].hidden = true;
        }
        QUESTS.clear();
    }
}

#[no_mangle]
pub extern "C" fn quest_begin(gx: f32, gy: f32, gz: f32, r: f32, g: f32, b: f32) -> u32 {
    unsafe {
        let id = buf_to_string(&BUF_ID);
        let title = buf_to_string(&BUF_TITLE);
        let desc = buf_to_string(&BUF_DESC);
        let ground_y = terrain_height(gx, gz);
        let giver_y = if gy <= 0.0 { ground_y } else { gy };

        let giver_entity_idx = ENTITIES.len();
        ENTITIES.push(Entity {
            mesh: 3,
            pos: V3::new(gx, giver_y + 0.6, gz), yaw: 0.0,
            scale: V3::new(0.55, 1.2, 0.55),
            tint: V3::new(r, g, b),
            hidden: false, sway: 0.0,
        });
        let marker_entity_idx = ENTITIES.len();
        ENTITIES.push(Entity {
            mesh: 1,
            pos: V3::new(gx, giver_y + 2.6, gz), yaw: 0.0,
            scale: V3::new(0.25, 0.45, 0.25),
            tint: V3::new(1.0, 0.85, 0.25),
            hidden: false, sway: 0.0,
        });

        QUESTS.push(Quest {
            id, title, desc,
            giver_pos: V3::new(gx, giver_y, gz),
            giver_tint: V3::new(r, g, b),
            accepted: false,
            turned_in: false,
            objectives: Vec::new(),
            giver_entity_idx,
            marker_entity_idx,
        });
        (QUESTS.len() - 1) as u32
    }
}

#[no_mangle]
pub extern "C" fn quest_add_objective(qi: u32, kind: u32, target: u32,
                                      px: f32, py: f32, pz: f32, radius: f32, count: u32) {
    unsafe {
        let desc = buf_to_string(&BUF_OBJ_DESC);
        if let Some(q) = QUESTS.get_mut(qi as usize) {
            q.objectives.push(Objective {
                kind, target, pos: V3::new(px, py, pz), radius,
                required: count, progress: 0,
                desc,
            });
        }
    }
}

#[no_mangle]
pub extern "C" fn quest_set_state(qi: u32, accepted: u32, turned_in: u32) {
    unsafe {
        if let Some(q) = QUESTS.get_mut(qi as usize) {
            q.accepted = accepted != 0;
            q.turned_in = turned_in != 0;
        }
    }
}

#[no_mangle]
pub extern "C" fn quest_set_progress(qi: u32, oi: u32, progress: u32) {
    unsafe {
        if let Some(q) = QUESTS.get_mut(qi as usize) {
            if let Some(o) = q.objectives.get_mut(oi as usize) {
                o.progress = progress;
            }
        }
    }
}

fn escape_json(s: &str) -> String {
    let mut r = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => r.push_str("\\\""),
            '\\' => r.push_str("\\\\"),
            '\n' => r.push_str("\\n"),
            '\r' => r.push_str("\\r"),
            '\t' => r.push_str("\\t"),
            c if (c as u32) < 0x20 => r.push_str(&format!("\\u{:04x}", c as u32)),
            c => r.push(c),
        }
    }
    r
}

#[no_mangle]
pub extern "C" fn quest_json_ptr() -> *const u8 {
    unsafe {
        QUEST_JSON.clear();
        QUEST_JSON.push('[');
        for (i, q) in QUESTS.iter().enumerate() {
            if i > 0 { QUEST_JSON.push(','); }
            QUEST_JSON.push_str(&format!(
                "{{\"id\":\"{}\",\"title\":\"{}\",\"desc\":\"{}\",\"giver\":[{:.2},{:.2},{:.2}],\"giverTint\":[{:.3},{:.3},{:.3}],\"accepted\":{},\"turnedIn\":{},\"objectives\":[",
                escape_json(&q.id), escape_json(&q.title), escape_json(&q.desc),
                q.giver_pos.x, q.giver_pos.y, q.giver_pos.z,
                q.giver_tint.x, q.giver_tint.y, q.giver_tint.z,
                q.accepted, q.turned_in,
            ));
            for (j, o) in q.objectives.iter().enumerate() {
                if j > 0 { QUEST_JSON.push(','); }
                QUEST_JSON.push_str(&format!(
                    "{{\"desc\":\"{}\",\"kind\":{},\"target\":{},\"pos\":[{:.2},{:.2},{:.2}],\"radius\":{:.2},\"required\":{},\"progress\":{}}}",
                    escape_json(&o.desc), o.kind, o.target,
                    o.pos.x, o.pos.y, o.pos.z, o.radius,
                    o.required, o.progress,
                ));
            }
            QUEST_JSON.push_str("]}");
        }
        QUEST_JSON.push(']');
        QUEST_JSON.as_ptr()
    }
}

#[no_mangle]
pub extern "C" fn quest_json_len() -> u32 {
    unsafe { QUEST_JSON.len() as u32 }
}

#[no_mangle]
pub extern "C" fn npc_count() -> u32 { unsafe { NPCS.len() as u32 } }

static mut NPC_BUF: Vec<f32> = Vec::new();
#[no_mangle]
pub extern "C" fn npc_data_ptr() -> *const f32 {
    unsafe {
        NPC_BUF.clear();
        for n in NPCS.iter() {
            NPC_BUF.extend_from_slice(&[n.pos.x, n.pos.z]);
        }
        NPC_BUF.as_ptr()
    }
}

fn rand01() -> f32 {
    unsafe {
        RNG = RNG.wrapping_mul(1664525).wrapping_add(1013904223);
        ((RNG >> 8) & 0xFFFF) as f32 / 65536.0
    }
}

// ============================================================
// COLLISION
// ============================================================

fn overlaps(center: V3, ext: V3, c: &AABB) -> bool {
    let pmin = center.sub(ext);
    let pmax = center.add(ext);
    pmax.x > c.min.x && pmin.x < c.max.x
        && pmax.y > c.min.y && pmin.y < c.max.y
        && pmax.z > c.min.z && pmin.z < c.max.z
}

#[derive(Default, Copy, Clone)]
struct Hits { x: bool, y_up: bool, y_dn: bool, z: bool }

fn disabled_aabb() -> AABB {
    AABB { min: V3::new(1e9, 1e9, 1e9), max: V3::new(1e9 + 1.0, 1e9 + 1.0, 1e9 + 1.0) }
}

unsafe fn move_collide(center: V3, delta: V3, ext: V3) -> (V3, Hits) {
    let mut p = center;
    let mut hits = Hits::default();
    const EPS: f32 = 1e-4;

    // X axis.
    p.x += delta.x;
    for c in COLLIDERS.iter() {
        if overlaps(p, ext, c) {
            if delta.x > 0.0 { p.x = c.min.x - ext.x - EPS; }
            else if delta.x < 0.0 { p.x = c.max.x + ext.x + EPS; }
            hits.x = true;
        }
    }

    // Z axis.
    p.z += delta.z;
    for c in COLLIDERS.iter() {
        if overlaps(p, ext, c) {
            if delta.z > 0.0 { p.z = c.min.z - ext.z - EPS; }
            else if delta.z < 0.0 { p.z = c.max.z + ext.z + EPS; }
            hits.z = true;
        }
    }

    // Y axis.
    p.y += delta.y;
    for c in COLLIDERS.iter() {
        if overlaps(p, ext, c) {
            if delta.y > 0.0 { p.y = c.min.y - ext.y - EPS; hits.y_up = true; }
            else if delta.y < 0.0 { p.y = c.max.y + ext.y + EPS; hits.y_dn = true; }
        }
    }

    // Terrain floor.
    let floor = terrain_height(p.x, p.z);
    if p.y - ext.y < floor {
        p.y = floor + ext.y;
        if delta.y < 0.0 { hits.y_dn = true; }
    }

    (p, hits)
}

// ============================================================
// GAME STATE
// ============================================================

const EYE_H: f32 = 1.7;
const PLAYER_R: f32 = 0.35;
const MOVE_SPEED: f32 = 5.5;
const SPRINT_MULT: f32 = 1.8;
const MOUSE_SENS: f32 = 0.0025;

// WoW-style starting tilt: camera sits behind and above the player looking
// down at ~23°, not level with the warrior's eyeline. Negative pitch tilts
// the look vector downward (look.y = sin(pitch)).
const INITIAL_PITCH: f32 = -0.40;
const GRAVITY: f32 = 25.0;
const JUMP_V: f32 = 8.0;

struct Shader {
    program: u32,
    u_vp: i32,
    u_model: i32,
    u_tint: i32,
    u_sun_dir: i32,
    u_sun_color: i32,
    u_sky_top: i32,
    u_sky_bot: i32,
    u_fog_color: i32,
    u_sway: i32,
    u_time: i32,
    u_camera_pos: i32,
    u_spec: i32,
    u_night: i32,
    u_flash: i32,
    u_rain: i32,
    u_tex: i32,
    u_has_tex: i32,
}

struct SkyShader {
    program: u32,
    u_sky_vp: i32,
    u_sun_dir: i32,
    u_sky_top: i32,
    u_sky_bot: i32,
    u_sun_color: i32,
    u_time: i32,
    u_rain: i32,
    u_flash: i32,
}

struct State {
    scene_shader: Shader,
    sky_shader: SkyShader,
    sky_mesh: Mesh,

    cam_x: f32, cam_y: f32, cam_z: f32,
    vy: f32,
    yaw: f32, pitch: f32,
    w: bool, a: bool, s: bool, d: bool, space: bool, shift: bool, ctrl: bool,
    canvas_w: u32, canvas_h: u32,
    t: f32,

    tps: bool,
    tps_dist: f32,
    player_body_idx: usize,

    step_phase: f32,
    shake: f32,
    step_sound_acc: f32,
    freecam: bool,
    zoom_hold: bool,
    fov: f32,
    time_scale: f32,

    // Weather.
    storm_t: f32,
    storm_force: bool,
    rain: f32,
    flash: f32,
    thunder_pending: f32,
    rain_spawn_acc: f32,

    // Character creation — just the class slot (0=Warrior, 1=Hunter, 2=Mage).
    class_: u8,

    // Smoothed body yaw. Follows movement direction while moving; eases back
    // to face the camera when still. Broadcast over multiplayer so remotes
    // see each other facing the way they're actually running, not the way
    // their camera is pointing.
    body_yaw: f32,
}

static mut S: Option<State> = None;

// ============================================================
// EXPORTS
// ============================================================

#[no_mangle]
pub extern "C" fn init(w: u32, h: u32) {
    let scene_shader = compile_scene_shader();
    let sky_shader = compile_sky_shader();
    let sky_mesh = mesh_cube(1.0);

    unsafe {
        S = Some(State {
            scene_shader, sky_shader,
            sky_mesh,
            cam_x: 0.0, cam_y: EYE_H, cam_z: 8.0, vy: 0.0,
            yaw: 0.0, pitch: INITIAL_PITCH,
            w: false, a: false, s: false, d: false, space: false, shift: false, ctrl: false,
            canvas_w: w, canvas_h: h,
            t: 0.0,
            // WoW-style: start in third-person so the player sees their own
            // character by default.
            tps: true,
            tps_dist: 5.5,
            player_body_idx: 0,
            step_phase: 0.0,
            shake: 0.0,
            step_sound_acc: 0.0,
            freecam: false,
            zoom_hold: false,
            fov: 1.2,
            time_scale: 1.0,
            storm_t: 0.0,
            storm_force: false,
            rain: 0.0,
            flash: 0.0,
            thunder_pending: 0.0,
            rain_spawn_acc: 0.0,
            class_: 0,
            body_yaw: PI,
        });

        build_scene();

        // Spawn player avatar entities — visible only in TPS mode.
        let body_idx = ENTITIES.len();
        ENTITIES.push(Entity {
            mesh: 1, // placeholder; JS set_entity_mesh points this at the rigged Meshy GLB
            pos: V3::ZERO, yaw: 0.0,
            scale: V3::new(1.0, 1.0, 1.0),
            tint: V3::new(1.0, 1.0, 1.0),
            hidden: true, sway: 0.0,
        });
        if let Some(s) = S.as_mut() {
            s.player_body_idx = body_idx;
        }

        gl_enable(GL_DEPTH_TEST);
        gl_enable(GL_CULL_FACE);
        gl_clear_color(0.58, 0.78, 0.95, 1.0);
    }
}

#[no_mangle]
pub extern "C" fn on_resize(w: u32, h: u32) {
    if let Some(s) = unsafe { S.as_mut() } { s.canvas_w = w; s.canvas_h = h; }
}

#[no_mangle]
pub extern "C" fn on_key(code: u32, down: u32) {
    let v = down != 0;
    unsafe {
        if let Some(s) = S.as_mut() {
            match code {
                0 => s.w = v,
                1 => s.a = v,
                2 => s.s = v,
                3 => s.d = v,
                4 => s.space = v,
                5 => s.shift = v,
                8 => s.ctrl = v,
                6 => if v {
                    s.tps = !s.tps;
                    ENTITIES[s.player_body_idx].hidden = !s.tps;
                },
                7 => if v { s.freecam = !s.freecam; },
                21 => s.zoom_hold = v,
                22 => if v { s.time_scale = (s.time_scale * 0.5).max(0.125); },
                23 => if v { s.time_scale = (s.time_scale * 2.0).min(16.0); },
                24 => if v { s.time_scale = if s.time_scale == 0.0 { 1.0 } else { 0.0 }; },
                25 => if v { s.time_scale = 1.0; },
                27 => if v {
                    s.cam_x = 0.0; s.cam_y = EYE_H; s.cam_z = 8.0;
                    s.vy = 0.0; s.yaw = 0.0; s.pitch = INITIAL_PITCH;
                },
                _ => {}
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn on_mouse_button(button: u32, down: u32) {
    unsafe {
        let s = match S.as_mut() { Some(s) => s, None => return };

        // Right-click: hold to zoom.
        if button == 2 {
            s.zoom_hold = down != 0;
            return;
        }
        if down == 0 { return; }

        if button == 0 {
            // Left-click is now a melee attack — the swing animation is driven
            // from JS against the rigged hero, and there is no projectile.
            let _ = s;
        }
    }
}

#[no_mangle]
pub extern "C" fn on_wheel(dy: f32) {
    // WoW-style: scroll out from FPS switches to TPS; scroll all the way in from
    // TPS flips back to FPS. Otherwise adjust follow distance.
    unsafe {
        let s = match S.as_mut() { Some(s) => s, None => return };
        if !s.tps && dy > 0.0 {
            s.tps = true;
            s.tps_dist = 3.5;
            ENTITIES[s.player_body_idx].hidden = false;
            return;
        }
        if s.tps && dy < 0.0 && s.tps_dist <= 2.0 {
            s.tps = false;
            s.tps_dist = 3.5;
            ENTITIES[s.player_body_idx].hidden = true;
            return;
        }
        s.tps_dist = (s.tps_dist + dy * 0.01).clamp(1.8, 14.0);
    }
}

#[no_mangle]
pub extern "C" fn on_mouse_move(dx: f32, dy: f32) {
    if let Some(s) = unsafe { S.as_mut() } {
        s.yaw -= dx * MOUSE_SENS;
        s.pitch -= dy * MOUSE_SENS;
        let lim = 1.55;
        if s.pitch >  lim { s.pitch =  lim; }
        if s.pitch < -lim { s.pitch = -lim; }
    }
}

#[no_mangle]
pub extern "C" fn frame(dt: f32) {
    unsafe {
        let s = match S.as_mut() { Some(s) => s, None => return };
        s.t += dt * s.time_scale;

        if PREVIEW_ACTIVE {
            let s_ref = S.as_ref().unwrap();
            render_preview(s_ref, dt);
            return;
        }

        // Smooth FOV zoom.
        let target_fov = if s.zoom_hold { 0.55 } else { 1.2 };
        s.fov += (target_fov - s.fov) * (10.0 * dt).min(1.0);

        // --- movement ---
        let fwd_planar = V3::new(-s.yaw.sin(), 0.0, -s.yaw.cos());
        let right = V3::new(-fwd_planar.z, 0.0, fwd_planar.x);

        if s.freecam {
            // Free-cam: fly along look direction, no gravity/collision.
            let cy = s.yaw.cos(); let sy = s.yaw.sin();
            let cp = s.pitch.cos(); let sp = s.pitch.sin();
            let look = V3::new(-sy * cp, sp, -cy * cp);
            let mut mv = V3::ZERO;
            if s.w { mv = mv.add(look); }
            if s.s { mv = mv.sub(look); }
            if s.d { mv = mv.add(right); }
            if s.a { mv = mv.sub(right); }
            if s.space { mv = mv.add(V3::Y); }
            if s.ctrl  { mv = mv.sub(V3::Y); }
            let mv = mv.norm();
            let speed = 12.0 * if s.shift { 3.0 } else { 1.0 };
            s.cam_x += mv.x * speed * dt;
            s.cam_y += mv.y * speed * dt;
            s.cam_z += mv.z * speed * dt;
            s.vy = 0.0;

            STATS[0] = s.cam_x; STATS[1] = s.cam_y; STATS[2] = s.cam_z;
            STATS[3] = s.yaw; STATS[4] = s.t * 0.05; STATS[5] = if s.tps { 1.0 } else { 0.0 };
            STATS[6] = s.body_yaw; STATS[7] = 1.0;
            STATS[8] = 0.0; STATS[9] = 0.0;
            STATS[10] = 0.0; STATS[11] = 0.0;

            let player_eye = V3::new(s.cam_x, s.cam_y, s.cam_z);
            let up = V3::Y;
            ENTITIES[s.player_body_idx].hidden = true;
            let (eye, target) = (player_eye, player_eye.add(look));
            let aspect = s.canvas_w as f32 / s.canvas_h as f32;
            let proj = M4::perspective(s.fov, aspect, 0.1, 400.0);
            let view = M4::look_at(eye, target, up);
            let vp = proj.mul(&view);
            let mut view_nt = M4(view.0);
            view_nt.0[12] = 0.0; view_nt.0[13] = 0.0; view_nt.0[14] = 0.0;
            let sky_vp = proj.mul(&view_nt);

            let lit = compute_lighting(s.t);
            gl_viewport(0, 0, s.canvas_w as i32, s.canvas_h as i32);
            gl_clear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);
            let s_ref = S.as_ref().unwrap();
            render_scene(s_ref, &vp, &lit, eye);
            render_sky(s_ref, &sky_vp, &lit);
            return;
        }

        let fwd = fwd_planar;
        let mut mv = V3::ZERO;
        if s.w { mv = mv.add(fwd); }
        if s.s { mv = mv.sub(fwd); }
        if s.d { mv = mv.add(right); }
        if s.a { mv = mv.sub(right); }
        let mv = mv.norm();

        let speed = MOVE_SPEED * if s.shift { SPRINT_MULT } else { 1.0 };

        // Detect on_ground before gravity by probing just under the feet.
        let ext = V3::new(PLAYER_R, EYE_H * 0.5, PLAYER_R);
        let center = V3::new(s.cam_x, s.cam_y - EYE_H * 0.5, s.cam_z);
        let (_, probe) = move_collide(center, V3::new(0.0, -0.02, 0.0), ext);
        let grounded = probe.y_dn;

        // Restore player avatar visibility when back from freecam.
        ENTITIES[s.player_body_idx].hidden = !s.tps;

        if s.space && grounded {
            s.vy = JUMP_V;
        }
        s.vy -= GRAVITY * dt;

        let delta = V3::new(mv.x * speed * dt, s.vy * dt, mv.z * speed * dt);
        let (new_center, hits) = move_collide(center, delta, ext);
        s.cam_x = new_center.x;
        s.cam_y = new_center.y + EYE_H * 0.5;
        s.cam_z = new_center.z;
        if hits.y_dn && s.vy < 0.0 { s.vy = 0.0; }
        if hits.y_up && s.vy > 0.0 { s.vy = 0.0; }

        // Head-bob phase still drives FPS camera sway, but no landing shake,
        // no footstep audio, no landing particles. User wants clean movement.
        let moving = (mv.x * mv.x + mv.z * mv.z) > 0.01 && grounded;
        if moving { s.step_phase += speed * dt * 1.6; }

        // --- camera ---
        let cy = s.yaw.cos(); let sy = s.yaw.sin();
        let cp = s.pitch.cos(); let sp = s.pitch.sin();
        let look = V3::new(-sy * cp, sp, -cy * cp);
        let player_eye = V3::new(s.cam_x, s.cam_y, s.cam_z);
        let up = V3::Y;

        // Body faces the direction it's moving (not the camera). This lets
        // the forward walk/run clip read correctly while strafing — left/right
        // input rotates the body into the strafe direction instead of the
        // mesh walking sideways. When the player stops moving the body eases
        // back to face the camera. Rotation is smoothed so the mesh doesn't
        // pop 90° between keypresses.
        let feet_y = s.cam_y - EYE_H;
        ENTITIES[s.player_body_idx].pos = V3::new(s.cam_x, feet_y, s.cam_z);
        let moving_horizontal = (mv.x * mv.x + mv.z * mv.z) > 1e-4;
        // Mesh's default forward is +Z; player yaw=0 faces -Z. The π offset
        // compensates for that when reusing s.yaw to face the camera.
        // M4::trs rotates +Z by yaw into (-sin(yaw), 0, cos(yaw)), so the
        // yaw that points the mesh forward at world direction (mvx, 0, mvz)
        // is atan2(-mvx, mvz). Getting this sign wrong mirrors the body.
        let target_body_yaw = if moving_horizontal {
            (-mv.x).atan2(mv.z)
        } else {
            PI - s.yaw
        };
        // Angular lerp: shortest arc between current and target body yaw.
        let mut d = target_body_yaw - s.body_yaw;
        while d >  PI { d -= 2.0 * PI; }
        while d < -PI { d += 2.0 * PI; }
        // Faster turn while actually moving so strafe flips read instantly.
        let turn_rate = if moving_horizontal { 16.0 } else { 10.0 };
        let k = (1.0 - (-dt * turn_rate).exp()).min(1.0);
        s.body_yaw += d * k;
        ENTITIES[s.player_body_idx].yaw = s.body_yaw;

        let (eye, target) = if s.tps {
            let target = player_eye.add(V3::new(0.0, -0.25, 0.0));
            let eye = target.sub(look.scale(s.tps_dist));
            (eye, target)
        } else {
            let moving = (mv.x * mv.x + mv.z * mv.z) > 0.01 && grounded;
            let bob_y = if moving { s.step_phase.sin() * 0.06 } else { 0.0 };
            let bob_x = if moving { (s.step_phase * 0.5).sin() * 0.04 } else { 0.0 };
            let right = V3::new(-fwd.z, 0.0, fwd.x);
            let adj = V3::new(right.x * bob_x, bob_y, right.z * bob_x);
            let e = player_eye.add(adj);
            (e, e.add(look))
        };

        let aspect = s.canvas_w as f32 / s.canvas_h as f32;
        let proj = M4::perspective(s.fov, aspect, 0.1, 300.0);
        let view = M4::look_at(eye, target, up);
        let vp = proj.mul(&view);

        // Nameplate anchor: ~2m above the avatar's feet (above the head).
        if s.tps {
            let body_pos = ENTITIES[s.player_body_idx].pos;
            let anchor = V3::new(body_pos.x, body_pos.y + 2.0, body_pos.z);
            project_nameplate(&vp, anchor);
        } else {
            NAMEPLATE[2] = 0.0;
        }

        // View without translation → skybox stays around camera.
        let mut view_nt = M4(view.0);
        view_nt.0[12] = 0.0; view_nt.0[13] = 0.0; view_nt.0[14] = 0.0;
        let sky_vp = proj.mul(&view_nt);

        // --- render ---
        gl_viewport(0, 0, s.canvas_w as i32, s.canvas_h as i32);
        gl_clear(GL_COLOR_BUFFER_BIT | GL_DEPTH_BUFFER_BIT);

        STATS[0] = s.cam_x; STATS[1] = s.cam_y; STATS[2] = s.cam_z;
        STATS[3] = s.yaw; STATS[4] = s.t * 0.05; STATS[5] = if s.tps { 1.0 } else { 0.0 };
        STATS[6] = s.body_yaw;
        STATS[7] = if s.freecam { 1.0 } else { 0.0 };
        STATS[8] = 0.0; STATS[9] = 0.0;
        STATS[10] = if grounded { 1.0 } else { 0.0 };
        STATS[11] = 0.0;

        let lit = compute_lighting(s.t);
        let s_ref = S.as_ref().unwrap();
        render_scene(s_ref, &vp, &lit, eye);
        render_sky(s_ref, &sky_vp, &lit);
    }
}

// ============================================================
// RENDERING
// ============================================================

struct Lighting {
    sun_dir: V3,
    sun_color: V3,
    sky_top: V3,
    sky_bot: V3,
    fog: V3,
}

fn compute_lighting(t: f32) -> Lighting {
    let day_t = t * 0.05;
    let sun_dir = V3::new(
        (day_t * 0.4).sin() * 0.45,
        day_t.sin(),
        day_t.cos(),
    ).norm();

    let h = sun_dir.y;
    let day = smoothstep01(-0.05, 0.25, h);
    let dusk = smoothstep01(0.05, 0.35, 0.4 - h.abs());

    let sun_noon   = V3::new(1.25, 1.10, 0.85);
    let sun_dawn   = V3::new(1.45, 0.60, 0.30);
    let sun_night  = V3::new(0.18, 0.22, 0.35);
    let sun_color = lerp(sun_night, lerp(sun_dawn, sun_noon, day), day.max(dusk));

    let sky_top_day   = V3::new(0.30, 0.55, 0.90);
    let sky_top_night = V3::new(0.02, 0.04, 0.09);
    let sky_top = lerp(sky_top_night, sky_top_day, day);

    let sky_bot_day   = V3::new(0.85, 0.88, 0.80);
    let sky_bot_dawn  = V3::new(0.95, 0.55, 0.35);
    let sky_bot_night = V3::new(0.08, 0.10, 0.18);
    let sky_bot_base  = lerp(sky_bot_night, sky_bot_day, day);
    let sky_bot = lerp(sky_bot_base, sky_bot_dawn, dusk * 0.85);

    let fog = lerp(sky_bot, V3::new(0.70, 0.80, 0.92), 0.35);

    Lighting { sun_dir, sun_color, sky_top, sky_bot, fog }
}

unsafe fn render_scene(s: &State, vp: &M4, lit: &Lighting, eye: V3) {
    let sh = &s.scene_shader;
    gl_use_program(sh.program);
    gl_uniform_matrix4fv(sh.u_vp, vp.0.as_ptr());
    gl_uniform3f(sh.u_sun_dir, lit.sun_dir.x, lit.sun_dir.y, lit.sun_dir.z);
    gl_uniform3f(sh.u_sun_color, lit.sun_color.x, lit.sun_color.y, lit.sun_color.z);
    gl_uniform3f(sh.u_sky_top, lit.sky_top.x, lit.sky_top.y, lit.sky_top.z);
    gl_uniform3f(sh.u_sky_bot, lit.sky_bot.x, lit.sky_bot.y, lit.sky_bot.z);
    gl_uniform3f(sh.u_fog_color, lit.fog.x, lit.fog.y, lit.fog.z);
    gl_uniform3f(sh.u_camera_pos, eye.x, eye.y, eye.z);
    gl_uniform1f(sh.u_time, s.t);
    let night = 1.0 - smoothstep01(-0.05, 0.25, lit.sun_dir.y);
    gl_uniform1f(sh.u_night, night);
    gl_uniform1f(sh.u_flash, s.flash);
    gl_uniform1f(sh.u_rain, s.rain);
    // Sampler2D u_tex stays bound to texture unit 0.
    gl_uniform1i(sh.u_tex, 0);
    gl_active_texture(GL_TEXTURE0);

    let mut last_sway: f32 = -1.0;
    let mut last_spec: f32 = -1.0;
    let mut last_tex: u32 = u32::MAX;
    for e in ENTITIES.iter() {
        if e.hidden { continue; }
        let m = MESHES[e.mesh as usize];
        let model = M4::trs(e.pos, e.yaw, e.scale);
        gl_uniform_matrix4fv(sh.u_model, model.0.as_ptr());
        gl_uniform3f(sh.u_tint, e.tint.x, e.tint.y, e.tint.z);
        if e.sway != last_sway { gl_uniform1f(sh.u_sway, e.sway); last_sway = e.sway; }
        // Cubes (mesh 1) get a touch of spec for dampness; cylinders/spheres a bit more (creatures).
        let spec = match e.mesh {
            1 => 0.05,
            2 => 0.25,
            3 => 0.15,
            _ => 0.0,
        };
        if spec != last_spec { gl_uniform1f(sh.u_spec, spec); last_spec = spec; }
        if m.texture != last_tex {
            if m.texture != 0 {
                gl_bind_texture(GL_TEXTURE_2D, m.texture);
                gl_uniform1i(sh.u_has_tex, 1);
            } else {
                gl_uniform1i(sh.u_has_tex, 0);
            }
            last_tex = m.texture;
        }
        gl_bind_vertex_array(m.vao);
        gl_draw_elements(GL_TRIANGLES, m.index_count, m.index_ty, 0);
    }
}

unsafe fn render_sky(s: &State, sky_vp: &M4, lit: &Lighting) {
    let sh = &s.sky_shader;
    gl_depth_func(GL_LEQUAL);
    gl_disable(GL_CULL_FACE);
    gl_use_program(sh.program);
    gl_uniform_matrix4fv(sh.u_sky_vp, sky_vp.0.as_ptr());
    gl_uniform3f(sh.u_sun_dir, lit.sun_dir.x, lit.sun_dir.y, lit.sun_dir.z);
    gl_uniform3f(sh.u_sky_top, lit.sky_top.x, lit.sky_top.y, lit.sky_top.z);
    gl_uniform3f(sh.u_sky_bot, lit.sky_bot.x, lit.sky_bot.y, lit.sky_bot.z);
    gl_uniform3f(sh.u_sun_color, lit.sun_color.x, lit.sun_color.y, lit.sun_color.z);
    gl_uniform1f(sh.u_time, s.t);
    gl_uniform1f(sh.u_rain, s.rain);
    gl_uniform1f(sh.u_flash, s.flash);
    gl_bind_vertex_array(s.sky_mesh.vao);
    gl_draw_elements(GL_TRIANGLES, s.sky_mesh.index_count, s.sky_mesh.index_ty, 0);
    gl_enable(GL_CULL_FACE);
    gl_depth_func(0x0201); // LESS
}

// ============================================================
// SCENE BUILD
// ============================================================

unsafe fn build_scene() {
    let terrain = mesh_terrain(120.0, 96);
    let cube = mesh_cube(0.5);
    let sphere = mesh_sphere(0.5, 12, 20);
    let cylinder = mesh_cylinder(0.5, 1.0, 20);

    MESHES.push(terrain);    // 0
    MESHES.push(cube);       // 1
    MESHES.push(sphere);     // 2
    MESHES.push(cylinder);   // 3

    ENTITIES.push(Entity {
        mesh: 0, pos: V3::ZERO, yaw: 0.0,
        scale: V3::ONE, tint: V3::ONE, hidden: false, sway: 0.0,
    });
}


unsafe fn update_npcs(dt: f32) {
    let (player_x, player_z) = if let Some(s) = S.as_ref() {
        (s.cam_x, s.cam_z)
    } else { (0.0, 0.0) };

    for i in 0..NPCS.len() {
        let (dir_x, dir_z, timer_before, pos_before, kind, base_scale) = {
            let n = &NPCS[i];
            (n.dir_x, n.dir_z, n.timer, n.pos, n.kind, n.base_scale)
        };

        let (speed, flee_range, ext_half) = match kind {
            1 => (4.2, 10.0, base_scale.max(0.25)),
            _ => (1.4, 0.0, 0.35),
        };
        let npc_ext = V3::new(ext_half, ext_half, ext_half);

        // For critters: if player is close, override direction to flee.
        let mut target_dx = dir_x;
        let mut target_dz = dir_z;
        let mut forced_pick = false;
        let dpx = pos_before.x - player_x;
        let dpz = pos_before.z - player_z;
        let pd2 = dpx * dpx + dpz * dpz;
        if kind == 1 && pd2 < flee_range * flee_range && pd2 > 1e-4 {
            let l = pd2.sqrt();
            target_dx = dpx / l;
            target_dz = dpz / l;
            forced_pick = true;
        }

        let center = V3::new(pos_before.x, pos_before.y + npc_ext.y, pos_before.z);
        let delta = V3::new(target_dx * speed * dt, 0.0, target_dz * speed * dt);
        let (new_center, hits) = move_collide(center, delta, npc_ext);

        let n = &mut NPCS[i];
        n.pos.x = new_center.x;
        n.pos.z = new_center.z;
        n.pos.y = terrain_height(n.pos.x, n.pos.z);
        n.dir_x = target_dx;
        n.dir_z = target_dz;
        n.phase += dt * if kind == 1 { 7.0 } else { 3.5 };
        n.timer = timer_before - dt;

        let mut bumped = hits.x || hits.z;
        let r2 = n.pos.x * n.pos.x + n.pos.z * n.pos.z;
        if r2 > 95.0 * 95.0 { bumped = true; }

        if !forced_pick && (n.timer <= 0.0 || bumped) {
            let a = rand01() * PI * 2.0;
            n.dir_x = a.cos();
            n.dir_z = a.sin();
            if r2 > 95.0 * 95.0 {
                let to_center = (-n.pos.x, -n.pos.z);
                let l = (to_center.0 * to_center.0 + to_center.1 * to_center.1).sqrt();
                if l > 1e-3 {
                    n.dir_x = to_center.0 / l;
                    n.dir_z = to_center.1 / l;
                }
            }
            n.timer = if kind == 1 { 0.8 + rand01() * 1.6 } else { 1.5 + rand01() * 3.0 };
        }

        // Render: bob height + squash scale.
        let bob_amp = if kind == 1 { 0.14 } else { 0.18 };
        let bob = n.phase.sin() * bob_amp;
        let squash = 1.0 + (n.phase * 2.0).sin() * 0.12;
        let yaw = (-n.dir_x).atan2(-n.dir_z);
        let e = &mut ENTITIES[n.entity_idx];
        e.pos = V3::new(n.pos.x, n.pos.y + n.base_scale * 0.7 + bob.max(0.0), n.pos.z);
        e.yaw = yaw;
        let sq_y = if kind == 1 { 0.9 } else { 0.75 };
        e.scale = V3::new(n.base_scale, n.base_scale * sq_y / squash, n.base_scale);
    }
}

// ============================================================
// SHADER COMPILATION
// ============================================================

fn uniform_loc(program: u32, name: &[u8]) -> i32 {
    unsafe { gl_get_uniform_location(program, name.as_ptr(), name.len() as u32) }
}

fn compile_program(vs_src: &[u8], fs_src: &[u8]) -> u32 {
    unsafe {
        let vs = gl_create_shader(GL_VERTEX_SHADER);
        gl_shader_source(vs, vs_src.as_ptr(), vs_src.len() as u32);
        gl_compile_shader(vs);
        let fs = gl_create_shader(GL_FRAGMENT_SHADER);
        gl_shader_source(fs, fs_src.as_ptr(), fs_src.len() as u32);
        gl_compile_shader(fs);
        let p = gl_create_program();
        gl_attach_shader(p, vs);
        gl_attach_shader(p, fs);
        gl_link_program(p);
        p
    }
}

fn compile_scene_shader() -> Shader {
    let program = compile_program(SCENE_VS, SCENE_FS);
    Shader {
        program,
        u_vp: uniform_loc(program, b"u_vp"),
        u_model: uniform_loc(program, b"u_model"),
        u_tint: uniform_loc(program, b"u_tint"),
        u_sun_dir: uniform_loc(program, b"u_sun_dir"),
        u_sun_color: uniform_loc(program, b"u_sun_color"),
        u_sky_top: uniform_loc(program, b"u_sky_top"),
        u_sky_bot: uniform_loc(program, b"u_sky_bot"),
        u_fog_color: uniform_loc(program, b"u_fog_color"),
        u_sway: uniform_loc(program, b"u_sway"),
        u_time: uniform_loc(program, b"u_time"),
        u_camera_pos: uniform_loc(program, b"u_camera_pos"),
        u_spec: uniform_loc(program, b"u_spec"),
        u_night: uniform_loc(program, b"u_night"),
        u_flash: uniform_loc(program, b"u_flash"),
        u_rain: uniform_loc(program, b"u_rain"),
        u_tex: uniform_loc(program, b"u_tex"),
        u_has_tex: uniform_loc(program, b"u_has_tex"),
    }
}

fn compile_sky_shader() -> SkyShader {
    let program = compile_program(SKY_VS, SKY_FS);
    SkyShader {
        program,
        u_sky_vp: uniform_loc(program, b"u_sky_vp"),
        u_sun_dir: uniform_loc(program, b"u_sun_dir"),
        u_sky_top: uniform_loc(program, b"u_sky_top"),
        u_sky_bot: uniform_loc(program, b"u_sky_bot"),
        u_sun_color: uniform_loc(program, b"u_sun_color"),
        u_time: uniform_loc(program, b"u_time"),
        u_rain: uniform_loc(program, b"u_rain"),
        u_flash: uniform_loc(program, b"u_flash"),
    }
}
