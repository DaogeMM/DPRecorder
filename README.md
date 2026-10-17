# DPRecorder

**Phigros / RPE 谱面渲染器**

> [!WARNING]
> **本项目是 vibe coding 的产物。** 代码、文档、重构与整合大量由 AI 生成 —— 用的是 **DeepSeek V4.1 Flash**（DeepSeek 官方 API，模型 id `deepseek-flash`），人类作者负责提需求、做决定与验收。AI 输出**可能包含错误、过时或不安全的内容**，一切以仓库里的代码和你在本机的实测结果为准；构建、提交、再分发之前请自行审查与测试。完整声明见 [`NOTICE`](NOTICE) 第 1 节。

| 路径 | 作用 |
| --- | --- |
| `DPS/` | 引擎与播放器，5 个 crate：`dps`（核心库）、`dps-ui`、`dps-main`（瘦入口 exe）、`prpr-avc`（音视频编解码）、`prpr-pbc`（谱面转换 CLI） |
| `DPR/` | Tauri 2 桌面端：`DPR/src/` 是 Vue 3 + Vuetify 界面，`DPR/src-tauri/` 是 Rust 后端（crate `dpr`，exe `dpr.exe`） |
| `assets/` | 资源（材质 / 字体 / 音效 / 资源包），两侧共用 |
| `Cargo.toml` | 工作区根，6 个成员：`DPS/` 的 5 个 + `DPR/src-tauri` |

## fork 网络

```text
Phigros（Pigeon Games，闭源游戏）—— 谱面格式与素材被非官方引用
└─ Phira（TeamFlos，GPL-3.0，prpr 引擎）
   ├─ phira-render（TeamFlos，Apache-2.0，官方渲染器）
   │  └─ Phi-Recorder（2278535805，GPL-3.0）
   │     └─ DPR/   ← 本仓库的桌面渲染器，整个目录是它的 fork
   ├─ phire（2278535805，GPL-3.0；crate 原名 phire / phire-ui）
   │  └─ DPS/   ← 本仓库的引擎，整个目录是它的 fork
   └─ Phira-Pro（Phira-Pro，GPL-3.0）
      └─ DPS/dps/src/core/block_area/   ← 噪域（blockArea）实现，逐字移植
```

| 本仓库 | 直接上游 | 许可证 |
| --- | --- | --- |
| `DPR/`（渲染器，crate `dpr`） | [2278535805/Phi-Recorder](https://github.com/2278535805/Phi-Recorder)（旧名 `phigros-recorder`、`Not-Phira-render`，旧地址会重定向），再往上 [TeamFlos/phira-render](https://github.com/TeamFlos/phira-render) | GPL-3.0（含按 Apache-2.0 提供的上游代码，单向兼容） |
| `DPS/`（引擎，crate `dps` / `dps-ui` / `dps-main` / `prpr-avc` / `prpr-pbc`） | [2278535805/phire](https://github.com/2278535805/phire)，再往上 [TeamFlos/phira](https://github.com/TeamFlos/phira) 的 `prpr` 引擎 | GPL-3.0 |
| `DPS/dps/src/core/block_area/`（噪域） | [Phira-Pro/Phira-Pro](https://github.com/Phira-Pro/Phira-Pro) | GPL-3.0 |

上游依赖里还有几个自定义 fork，都在 `github.com/2278535805/` 下：[`prpr-macroquad`](https://github.com/2278535805/prpr-macroquad)、[`prpr-miniquad`](https://github.com/2278535805/prpr-miniquad)、[`sasa`](https://github.com/2278535805/sasa)、[`oboe-rs`](https://github.com/2278535805/oboe-rs) —— 它们对应的上游项目（macroquad / miniquad / sasa / oboe-rs）版权归各自作者。逐项出处与致谢见 [`NOTICE`](NOTICE)。

## 构建方式

需要 Windows 10/11 (x64)、Rust（MSVC target）、Microsoft C++ Build Tools、WebView2 Runtime、Node.js + pnpm；**出片**还需要 `ffmpeg`（在设置里指定它的目录，或放进 `PATH` / exe 同目录）。

```powershell
cd <仓库根>

pnpm --dir DPR install          # 前端依赖，只需一次
.\run.bat                       # 开发模式启动（等价于在 DPR\ 里 cargo tauri dev）

cd DPR
cargo tauri build               # 出安装包 → <仓库根>\target\release\bundle\
```

- 只编 Rust 后端：`cargo build -p dpr` → `target\debug\dpr.exe`；无界面渲染：`target\debug\dpr.exe --render -i <谱面.pez|.json> -o <输出目录>`（另有 `--preview` / `--play` / `--tweakoffset`、`-c <配置.toml>`）。
- 只要引擎：`cargo build -p dps-main` → `target\debug\dps-main.exe`；谱面格式转换：`cargo run -p prpr-pbc -- <输入> <输出.pbc>`。
- `cargo build -p dpr` 默认是 dev 形态（不把前端打进 exe，加载 `devUrl`，要配合 `pnpm dev`）；正式形态带 `--features custom-protocol`，`cargo tauri build` 就是。改过前端要先 `pnpm build` 才会进 exe。
- `prpr-avc` 需要的 FFmpeg 静态库**不在仓库里**（`DPS/prpr-avc/static-lib/<TARGET>/` 被 `.gitignore` 排除），缺了编不过；目录可用 `PRPR_AVC_LIBS` 覆盖（见 `DPS/prpr-avc/build.rs`）。
- `target/` 默认生成在仓库根、已被 `.gitignore` 排除；若用 `[build] target-dir` 指到别处，实际位置看 `cargo metadata` 的 `target_directory`。DPR 的运行时工作目录跟着 exe 走（`assets/`、`config.toml`、`respack/`、`output/`、`tmp/` 都在 exe 旁边）。

## 许可与出处

- 整体以 [GNU GPL-3.0](LICENSE) 发布。`DPR/` 谱系含按 **Apache-2.0** 提供的上游代码，两者**单向兼容**，并入本仓库后整体按 GPL-3.0 分发；再分发时请一并满足 GPL-3.0 的**源码提供**与**许可证保留**要求。
- 逐项出处、版权与致谢见 [`NOTICE`](NOTICE)；第三方组件与依赖的许可证索引见 [`LICENSES/README.md`](LICENSES/README.md)（Rust 依赖见 [`Cargo.lock`](Cargo.lock)，前端依赖见 [`DPR/pnpm-lock.yaml`](DPR/pnpm-lock.yaml)）。
- 第三方素材（`assets/Minecraft/` 的贴图与打击音效、`assets/respack/`、`assets/rank/`、字体等）版权归各自权利人，随仓库分发只为开箱可用；权利人如要求会立即移除。
- 致谢：Phi-Recorder 的作者 **HLMC**、**Mivik**；Phira 与 phira-render 的 **TeamFlos** 及其贡献者；Phira-Pro 的维护者；以及所有被直接或间接引用的开源项目作者。
- **非官方**：与 Pigeon Games（Phigros）、TeamFlos（Phira）、Mojang AB / Microsoft 均无任何隶属、授权或赞助关系；相关游戏名与商标归各自权利人。请勿用它伪造游玩成绩或官方内容。

界面与配置项仍在变动，细节一律以代码与实测为准。问题与建议直接联系作者（DaogeMM）。
