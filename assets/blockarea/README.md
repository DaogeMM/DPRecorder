# assets/blockarea（仓库根 `assets\`）

官方 BlockArea（噪域）渲染所需的二进制素材。移植来源是 **Phira-Pro**
（上游：<https://github.com/Phira-Pro/Phira-Pro>）
的 `prpr/src/core/{block_shader.rs, block_mask.rs}` + `block_shader_full.{vert,frag}`。

## 文件状态

| 文件 | 状态 | 说明 |
|---|---|---|
| `block_shader_full.vert` | **官方原件** | 从上游 Phira-Pro 的 `prpr/src/core/` 复制 |
| `block_shader_full.frag` | ⚠️ **官方原件 + 本仓库改动** | 同上（559 行 Unity 反编译片元着色器）。本仓库改了两处：`snapshotSample` 里硬编码的场景快照降采样，以及「岩浆噪域」的 `_LavaTex` / `_LavaMode` / `_LavaScale`，见下面两节 |
| `BlockNoise1.png` | **官方原件** | 256x256，位移图。与 `DPS/dps/src/core/block_area/BlockNoise1.png`、`index/res/BlockNoise1.png` 哈希一致 |
| `PointNoise.png` | **官方原件** | 128x128，火花图。同上 |
| `FD_Noise_00000.png` | ⚠️ **替身（非官方）** | 官方文件在 `Phira-Pro-main` 的备份里缺失（`prpr/assets/` 是空的），仓库里找不到。这里是 64x64 值噪声近似 |
| `TouchHover.png` | ⚠️ **替身（非官方）** | 官方应为 `Round10_Blur4`（44x44、PPU=100 的模糊圆点）。这里是 44x44 白色径向衰减近似 |

两个替身由 `gen_blockarea_assets.js` 生成，该脚本不在仓库里。

> TODO(未核实)：`FD_Noise_00000.png` / `TouchHover.png` 这两个替身目前没有可复现的生成方式。

## 本仓库对 full.frag 的改动之一：场景快照降采样

官方 `snapshotSample()`（场景快照采样，完整噪域填充里混的那层背景）把 UV 硬编码
量化到 **1/3 分辨率**：

```glsl
vec2 size = max(floor(_ScreenParams.xy / 3.0), vec2(1.0));
```

它和 `_BackgroundPixelScale`（默认 6.0，`frag:210-236` 先把场景 UV 量化成 6px 块）
叠加后，噪域填充里的画面会明显发虚。本仓库把它换成 uniform：

```glsl
uniform mediump float _SnapshotPixelScale;   // 新增，默认 3.0
vec2 size = max(floor(_ScreenParams.xy / max(_SnapshotPixelScale, 1.0)), vec2(1.0));
```

* 默认值 3.0 ⇒ 与官方**逐位一致**（`cargo test`/像素差分已验证：`YAVG=YMAX=0`）。
* 「噪域扭曲」关闭时 `native/shader.rs::set_fill_style` 把 `_SnapshotPixelScale`
  与 `_BackgroundPixelScale` 一起置 1.0 ⇒ 快照 1:1 采样，噪域后面的字 / 按键不再发虚。
* 官方原件 MD5 `69551487E55CBF156EC589D71D027837`（559 行）；改后
  MD5 `476A8C162AED74D18583DAD60F9A5827`（564 行）。

## 本仓库对 full.frag 的改动之二：「岩浆噪域」

`disabledColor()` 与 `fullActive()` 的填充色各多一层 `if/else`：`_LavaMode < 0.5` 走官方
原式（默认值就是 0 ⇒ **逐位等于官方**），否则把填充换成

```glsl
uniform mediump sampler2D _LavaTex;   // 本仓库新增：下界岩 / 岩浆的当前动画帧
uniform mediump float _LavaMode;      // 0 = 关闭（官方原样），1 = 用 _LavaTex 填
uniform mediump float _LavaScale;     // 每屏高铺多少格，默认 16
uniform mediump sampler2D _LavaUV;    // 逐噪域局部坐标（RG8），只在 _LavaUVMode == 2 时采
uniform mediump float _LavaUVMode;    // 0 = 平铺（官方那支），1 = 拉伸满整屏，2 = 拉伸进每个噪域
vec2 lavaUV(vec2 uv);                 // 按 _LavaUVMode 折算采样坐标
```

* `sceneUV`（`block_shader_full.vert:22`）= 屏幕 UV ⇒ 贴图**锚定屏幕**、噪域移动时从它下面滑过。
* `_LavaUVMode` 是「噪域皮肤」的三档：`0` 与官方那支逐位相同；`1` 直接拿 `sceneUV` 采
  （一张图铺满屏幕，噪域露出自己盖住的那块）；`2` 改采 `_LavaUV`（CPU 在每个噪域自己的矩形里
  把坐标归一化成 0~1，整张图被拉伸进这个噪域）。后两档只由资源包的 `noiseMode` 打开。
* 素材取自 `assets/Minecraft/{netherrock.png, lava.gif}`（仓库根唯一一份资源目录；16×16 / 32×32×16 帧），
  由 `native/shader.rs:109` / `:111` 用 `include_bytes!` 打进二进制，不走 `assets/blockarea`。
* 本仓库改动后的完整 MD5 `785B422FBFC838295ADB5D24A90B1AC3`（580 行）。

## 替身的影响

* `FD_Noise_00000.png` 只被 `block_shader_full.frag:429` 的
  `texture2D(uNoiseTex, u_xlat1.xy).xy` 用到一次，且只在 `uLayer == 3` 的
  **hover 溶解**分支里 —— AUTOPLAY 不参与渲染。
* `TouchHover.png` 只在 `native/touch.rs` 的 `hover_source()` 里解码；而
  `native/mod.rs` 的 `HOVER_ASSET` 目前是 `false`，hover 材质根本不构建，
  所以 AUTOPLAY 下它也不参与渲染。

⇒ 这两个替身**只影响「手指按住时的触摸光影」**，与噪域本体的观感无关。
拿到官方素材后直接覆盖同名文件即可，无需改代码。
