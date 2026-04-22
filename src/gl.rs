// WebGL2 constants (subset we use) and JS-provided imports.

pub const GL_COLOR_BUFFER_BIT: u32 = 0x4000;
pub const GL_DEPTH_BUFFER_BIT: u32 = 0x100;
pub const GL_DEPTH_TEST: u32 = 0x0B71;
pub const GL_CULL_FACE: u32 = 0x0B44;
pub const GL_TRIANGLES: u32 = 0x0004;
pub const GL_ARRAY_BUFFER: u32 = 0x8892;
pub const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
pub const GL_STATIC_DRAW: u32 = 0x88E4;
pub const GL_FLOAT: u32 = 0x1406;
pub const GL_UNSIGNED_SHORT: u32 = 0x1403;
pub const GL_UNSIGNED_INT: u32 = 0x1405;
pub const GL_VERTEX_SHADER: u32 = 0x8B31;
pub const GL_FRAGMENT_SHADER: u32 = 0x8B30;
pub const GL_LEQUAL: u32 = 0x0203;
pub const GL_TEXTURE_2D: u32 = 0x0DE1;
pub const GL_TEXTURE0: u32 = 0x84C0;

extern "C" {
    pub fn audio_beep(freq: f32, dur: f32, gain: f32);

    pub fn gl_clear_color(r: f32, g: f32, b: f32, a: f32);
    pub fn gl_clear(mask: u32);
    pub fn gl_enable(cap: u32);
    pub fn gl_disable(cap: u32);
    pub fn gl_depth_func(func: u32);
    pub fn gl_viewport(x: i32, y: i32, w: i32, h: i32);
    pub fn gl_create_shader(ty: u32) -> u32;
    pub fn gl_shader_source(shader: u32, ptr: *const u8, len: u32);
    pub fn gl_compile_shader(shader: u32);
    pub fn gl_create_program() -> u32;
    pub fn gl_attach_shader(p: u32, s: u32);
    pub fn gl_link_program(p: u32);
    pub fn gl_use_program(p: u32);
    pub fn gl_get_uniform_location(p: u32, ptr: *const u8, len: u32) -> i32;
    pub fn gl_uniform_matrix4fv(loc: i32, ptr: *const f32);
    pub fn gl_uniform3f(loc: i32, x: f32, y: f32, z: f32);
    pub fn gl_uniform1f(loc: i32, x: f32);
    pub fn gl_uniform1i(loc: i32, x: i32);
    pub fn gl_create_buffer() -> u32;
    pub fn gl_bind_buffer(target: u32, buffer: u32);
    pub fn gl_buffer_data_f32(target: u32, ptr: *const f32, len: u32, usage: u32);
    pub fn gl_buffer_data_u16(target: u32, ptr: *const u16, len: u32, usage: u32);
    pub fn gl_buffer_data_u32(target: u32, ptr: *const u32, len: u32, usage: u32);
    pub fn gl_create_vertex_array() -> u32;
    pub fn gl_bind_vertex_array(vao: u32);
    pub fn gl_vertex_attrib_pointer(idx: u32, size: i32, ty: u32, normalized: u32, stride: i32, offset: i32);
    pub fn gl_enable_vertex_attrib_array(idx: u32);
    pub fn gl_draw_elements(mode: u32, count: i32, ty: u32, offset: i32);
    pub fn gl_bind_texture(target: u32, handle: u32);
    pub fn gl_active_texture(unit: u32);
}
