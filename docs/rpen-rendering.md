# RPEN 噪域与渲染改动

| 变更 | 行为 | 主要文件 |
|---|---|---|
| RPEN 自定义噪域皮肤 | 在已有分层和遮挡判定上接入皮肤的 GLSL、多趟渲染目标、外部纹理、历史纹理和低性能通道。加载或绘制失败时回退到原生材质。 | `DPS/dps/src/core/block_area/skin.rs`、`parse/rpe.rs` |
| 忽略谱面噪域皮肤 | 切到 DPRecorder 原生材质，使面板里的颜色等参数可以用于该谱面。 | `core/chart.rs`、`DPR/src-tauri/src/render.rs` |
| 噪域画在着色器上层 | 关时沿用原有合成顺序。开时在当前帧的全局特效生效期间补画噪域，一次在特效前，一次在特效后。 | `scene/game.rs`、`core/effect.rs` |
| 噪域揭秘 | 在谱面特效之后用纯色标出谱面作者布置的噪域。填充不透明度在 0 到 1 之间，默认 0.5。启用后跳过底层原材质绘制。 | `core/block_area/reveal.rs`、`core/chart.rs` |
| 反转区挖空 | 默认开启。按层计算有效覆盖，遵循该格式的反转规则，抵消处透明。关时恢复逐块填色和描边。 | `core/block_area/reveal.rs` |
| 白边框 | 默认开启。可独立关闭，适用于逐块和合成挖空两种模式。 | `core/block_area/reveal.rs` |
| 自定义着色器区域 | 「谱面 → 界面」的「将谱面着色器限制在游玩区」默认关闭，沿用整帧渲染。开启后，谱面引用的自定义着色器与内置预设都在游玩区大小的缓冲中执行，UV、screenSize 和 gl_FragCoord 对应游玩区，越界采样取边缘像素。 | `core/effect.rs`、`parse/extra.rs`、`scene/game.rs` |
| 调试信息 | 界面调试的计数、实时 BPM，以及跟随变换的判定线调试文字移到谱面着色器之后。音符上的调试标签仍在谱面绘制阶段。 | `scene/game.rs`、`core/line.rs`、`core/resource.rs` |

以上 Rust 文件路径除特别标注外都位于 `DPS/dps/src/`。渲染设置通过 `DPR/src/components/ConfigView.vue`、配置字段表和前后端数据模型传递。

原有拖影、强制定位、简化噪域在普通材质模式下沿用自己的渲染路径。揭秘模式显示谱面里的实际几何，填充和白边框指示的是区域位置，不等同于触点判定内缩。

谱面可以随包提供自定义噪域皮肤。缺少对应皮肤资源或加载失败时，使用 DPRecorder 原生噪域材质。
