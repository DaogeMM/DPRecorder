#version 100
// 判定范围「按噪域覆盖度让位」那份材质用的顶点着色器。
//
// 与 block_vert.glsl 的区别：判定范围的顶点是**世界坐标**（`res.world_to_screen` 之后的
// 屏幕坐标，见 core/note.rs 的 draw_judge_range），必须走 macroquad 的 Model / Projection
// 才能落到正确位置；而 block area 那几个 pass 是自己铺 NDC 的全屏四边形，反过来不吃矩阵。
//
// 内容与 macroquad 默认顶点着色器一字不差（phire 侧见 ext.rs 的 `shader::VERTEX`），
// 单独放一份是因为要用 load_material 换掉片元着色器、又不想改全局默认材质。
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;

varying lowp vec2 uv;
varying lowp vec4 color;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
}
