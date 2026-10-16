#version 100
// 判定范围（配置里的「判定范围」，core/note.rs 的 draw_judge_range）片元着色器。
//
// 顶点色直出，但按「噪域方块覆盖度」让位：`u_paint` 是噪域那份掩码纹理，它的 R 通道盛着
// 噪域方块的覆盖度（1 = 这里被噪域盖住了），此时判定范围淡出、把那块让给噪域 ——
// 也就是「噪域可以把判定范围的重叠部分抹去」。噪域本身不被改动。
//
// 采样用 gl_FragCoord 而不是 v_uv：判定范围是一个任意四边形，它的 texcoord 是 0..1 的
// 贴图坐标（贴的是一张纯白 1x1），与掩码画布并不对应；gl_FragCoord 是当前渲染目标
// （chart_target）的像素坐标，减掉 `u_view` 再除以画布尺寸就得到掩码 uv。
//
// `u_view`：掩码画布在帧缓冲里的矩形 `(x, y, w, h)`。掩码是**按视口**烘的（宽高 = 视口
// 尺寸 / cell），画布比整块帧缓冲小得多，且不一定贴在左下角，所以不能直接除以画布尺寸。
// 这里用 gl_FragCoord（原点在左下，和掩码纹理的 v 轴同向），因此不需要再翻 y。
//
// 画布来自上一帧（`Chart::render_block_area` 每帧重铺），绘制顺序没变，因此判定范围仍在
// 音符之下、判定线之上。
precision mediump float;

uniform sampler2D u_paint;
uniform vec4 u_view;

varying lowp vec2 uv;
varying lowp vec4 color;

void main() {
    vec2 suv = (gl_FragCoord.xy - u_view.xy) / u_view.zw;
    float p = texture2D(u_paint, suv).r;
    gl_FragColor = vec4(color.rgb, color.a * (1.0 - p));
}
