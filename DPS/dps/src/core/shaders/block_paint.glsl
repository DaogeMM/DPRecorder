#version 100
// 「噪域着色覆盖度」：1 = 这一像素最终被噪域涂上了颜色。
// 判定范围（`core/note.rs` 的 `draw_judge_range`）要在这些像素上让位，于是先把覆盖度
// 烘成一张单通道画布，交给判定范围那份材质采样（见 `judge_range.glsl`）。
//
// 两种模式：
//   * u_xor = 0（正式管线）—— `block_cov.glsl` 把 Active / Disabled / Ready 三个相位的
//     覆盖度分别写进 cov 的 R / G / B，涂过 = 三个通道取 max。
//   * u_xor = 1（「简化噪域 + 直接镂空」）—— 填充的透明度是分组的，覆盖度不是简单并集：
//       非 Active 相位（Ready / Disabled）照常上色，由 `u_other` 给出；
//       Active 相位（普通块与减算块）按上游 `cov = |普通 − 减算|` 上色，
//       其中 `u_maskA` / `u_maskS` 都是**并集**（不能拿 sub[1] 那张异或累积过的画布，
//       否则「两块减算叠一起」那片在填充里已经抵消成洞、这里却会算成涂过）。
//
// 顶点着色器复用 block_vert.glsl：position 直接是 NDC、v_uv 就是渲染目标 uv
// （v = 0 在底边），与 cov / mask / sub 那几张画布的约定一致。
precision highp float;

uniform sampler2D u_cov;
uniform sampler2D u_other;
uniform sampler2D u_maskA;
uniform sampler2D u_maskS;
uniform float u_xor;

varying vec2 v_uv;

void main() {
    float a;
    if (u_xor > 0.5) {
        float o = texture2D(u_other, v_uv).r;
        float n = texture2D(u_maskA, v_uv).r;
        float s = texture2D(u_maskS, v_uv).r;
        a = max(o, abs(n - s));
    } else {
        vec4 c = texture2D(u_cov, v_uv);
        a = max(max(c.r, c.g), c.b);
    }
    gl_FragColor = vec4(a, a, a, 1.0);
}
