# DPRecorder

> [!WARNING]
> **本项目大量使用 AI 生成代码。** 源码、文档、重构与整合有相当一部分由 **DeepSeek** 模型（DeepSeek 官方 API，模型 id `deepseek-flash`）生成，人类作者负责提出需求、做决定与验收。**AI 输出可能包含错误、过时或不安全的内容**，一切以仓库里的代码与你在本机的实测结果为准；构建、再分发之前请自行审查与测试。完整声明见 [`NOTICE`](NOTICE) 第 1 节。

把 **Phigros 谱面离线渲染成视频**的 Windows 桌面程序：**Tauri 2** 外壳 + **Vue 3** 前端 + 内嵌的 Rust 节奏游戏引擎 **DPS**（解析谱面、跑判定、把每一帧画出来）。

> 这个项目 = **DPRecorder**（桌面渲染器）+ **DPS**（它内嵌的引擎）。
> **命名**：两侧子系统在正式使用前就定了名——引擎 **DPS**、桌面渲染器 **DPR**；早期工作名 `DP-Simulator`（引擎侧）、`DP-Recorder` / `DP-Renderer`（渲染器侧）**从未发布、也从未正式使用过**，只存在于项目早期的改名过程里。整个项目名是 **DPRecorder**。
> 本页面向**开发者 / 二次开发**：谱系与出处、怎么跑起来、想改什么该去读哪里。面向使用者的简短说明见 [`DPR/README.md`](DPR/README.md)。

## 谱系：fork 网络

DPRecorder 不是从零写的：它是**两个上游项目被并进同一个 Cargo 工作区**，再**逐字移植**了第三个项目的一段实现。谁从谁那儿来、各自什么许可证：

```text
Phigros（Pigeon Games，闭源游戏）—— 谱面格式与素材被非官方引用
└─ Phira（TeamFlos，GPL-3.0，prpr 引擎）
   ├─ phira-render（TeamFlos，Apache-2.0，官方渲染器）
   │  └─ Phi-Recorder（2278535805，GPL-3.0）
   │     └─ DPR/  ← 本仓库的桌面渲染器（整个目录是它的 fork）
   ├─ phire（2278535805，GPL-3.0；crate 原名 phire / phire-ui）
   │  └─ DPS/  ← 本仓库的引擎（整个目录是它的 fork）
   └─ Phira-Pro（Phira-Pro，GPL-3.0，自述「基于 Phira v0.8.2 的改版」）
      └─ DPS/dps/src/core/block_area/  ← 噪域（blockArea）实现，逐字移植
```

| 本仓库 | 直接上游（点击打开） | 再往上 | 许可证 |
| --- | --- | --- | --- |
| `DPR/`（渲染器，crate `dpr`，exe `dpr.exe`） | [2278535805/Phi-Recorder](https://github.com/2278535805/Phi-Recorder)（旧名 `phigros-recorder`、`Not-Phira-render`，旧地址会重定向） | [TeamFlos/phira-render](https://github.com/TeamFlos/phira-render) —— 官方渲染器，Phi-Recorder 是它的 fork | GPL-3.0（含按 Apache-2.0 提供的上游代码，单向兼容） |
| `DPS/`（引擎，crate `dps` / `dps-ui` / `dps-main` / `prpr-avc` / `prpr-pbc`） | [2278535805/phire](https://github.com/2278535805/phire)（上游 crate 原名 `phire` / `phire-ui`；`DP-Simulator` 是本项目这一侧的早期工作名，从未正式使用） | [TeamFlos/phira](https://github.com/TeamFlos/phira) 的 `prpr` 引擎 | GPL-3.0 |
| `DPS/dps/src/core/block_area/`（噪域 / blockArea） | [Phira-Pro/Phira-Pro](https://github.com/Phira-Pro/Phira-Pro)，逐字移植 | [TeamFlos/phira](https://github.com/TeamFlos/phira) | GPL-3.0 |

上游依赖里还有几个**自定义 fork**，都在 `github.com/2278535805/` 下：[`prpr-macroquad`](https://github.com/2278535805/prpr-macroquad)、[`prpr-miniquad`](https://github.com/2278535805/prpr-miniquad)、[`sasa`](https://github.com/2278535805/sasa)、[`oboe-rs`](https://github.com/2278535805/oboe-rs)（对应的上游项目 macroquad / miniquad / sasa / oboe-rs 的版权归各自作者）。

逐项出处、版权与致谢见 [`NOTICE`](NOTICE)；第三方组件与素材的许可证索引见 [`LICENSES/README.md`](LICENSES/README.md)；GPL-3.0 全文见 [`LICENSE`](LICENSE)。

## 怎么跑起来

先看下面的 [环境要求](#环境要求)：Windows 10/11 (x64)、Rust（MSVC target）、Microsoft C++ Build Tools、WebView2 Runtime、Node.js + pnpm；**要渲染出片**还需要 `ffmpeg`（在设置里指定它的目录，或放进 `PATH` / exe 同目录）。

```powershell
cd <仓库根——你 clone / 解压出来的那个目录>

# 1) 前端依赖（只需一次）
cd DPR
pnpm install
cd ..

# 2) 开发模式启动完整应用（改前端、改后端都用它）
.\run.bat                         # 等价于在 DPR\ 里执行 cargo tauri dev

# 3) 无界面渲染一条谱面
.\target\debug\dpr.exe --render -i <谱面.pez|.json> -o <输出目录>
```

- 只要引擎 / 谱面转换：`cargo build -p dps-main` → `target\debug\dps-main.exe`；`cargo run -p prpr-pbc -- <输入> <输出.pbc>`。
- 出安装包：`cd DPR` 后 `cargo tauri build`，产物在仓库根 `target\release\bundle\`。
- 完整命令、dev 形态与 `custom-protocol` 的区别、输出与临时目录，见下面的 [构建与运行](#构建与运行)。
- **`target/` 生成在哪**：**别人 clone 本仓库后，`cargo build` / `cargo tauri dev` 生成的 `target/` 就在仓库根**——也就是 `<仓库根>/target`（Windows 写法 `<仓库根>\target`），与 `Cargo.toml` 同级，已被 [`.gitignore`](.gitignore) 排除，不会进 git。只有在本机开发时它才可能被 `[build] target-dir` 指到仓库之外（那样产物就跟着那个目录走）。想知道实际位置，在仓库根跑 `cargo metadata` 看 `target_directory`。

## 想改什么，去读哪里

本仓库不带单独的开发者文档目录，**入口就是代码本身**：

| 你的目标 | 先读 |
| --- | --- |
| 渲染器怎么用、怎么打包、注意事项、出处 | [`DPR/README.md`](DPR/README.md) |
| 前端界面（Vue 3 + Vuetify）：设置页、任务队列、日志视图 | `DPR/src/`（入口 `DPR/src/main.ts`，设置页 `DPR/src/SettingsView.vue`，任务 `DPR/src/tasks.ts`） |
| Tauri 外壳与 Rust 后端：命令行参数、ffmpeg 调用、IPC、任务队列 | `DPR/src-tauri/src/`（入口 `DPR/src-tauri/src/main.rs`，接线 `lib.rs` / `ipc.rs`，渲染 `render.rs`） |
| 引擎：谱面解析、判定、判定线、音符、粒子、场景、资源 | `DPS/dps/src/`（`core/`、`parse/`、`scene/`、`ui/`） |
| 无界面播放器 / 谱面格式转换 CLI | `DPS/dps-main/src/main.rs`、`DPS/prpr-pbc/src/main.rs` |
| 工作区成员划分与编译 profile | 仓库根的 [`Cargo.toml`](Cargo.toml) |
| 许可、逐项出处、第三方许可证 | [`LICENSE`](LICENSE) · [`NOTICE`](NOTICE) · [`LICENSES/README.md`](LICENSES/README.md) |

> 本页只负责带你找到入口，**细节一律以代码为准**（界面与配置项仍在变动）。

## 项目状态与来源

| 项 | 说明 |
| --- | --- |
| 状态 | 仍在开发中：界面与配置项会继续变动，构建方式与行为一律以仓库当前代码、实测结果为准 |
| 性质 | **第三方非官方工具**，与 Pigeon Games（Phigros）、TeamFlos（Phira）、Mojang AB / Microsoft 均无任何隶属、授权或赞助关系；相关名称与商标归各自权利人 |
| 反馈 | 问题与建议请直接联系作者（DaogeMM）。请不要用它伪造游玩成绩或官方内容 |

🤖 **开发方式**：本项目**使用了 DeepSeek 大模型辅助制作**，具体用的是 **DeepSeek V4.1 Flash**（DeepSeek 官方 API，模型 id `deepseek-flash`）——代码、文档、重构与整合均为人机协作，人类作者负责提出需求、做决定与验收。模型输出可能包含错误或过时信息，**一切以仓库里的代码与实测结果为准**。

## 仓库结构

| 路径 | 作用 |
| --- | --- |
| `DPS/` | Rust 节奏游戏引擎与播放器（5 个 crate）：`dps`（核心库）、`dps-ui`、`dps-main`（瘦入口 exe）、`prpr-avc`（音视频编解码）、`prpr-pbc`（谱面格式转换 CLI） |
| `DPR/` | Tauri 2 桌面渲染器：`DPR/src/`（Vue 3 前端）、`DPR/src-tauri/`（Rust 后端，crate `dpr`、exe `dpr.exe`） |
| `assets/` | **唯一一份运行时资源**（材质 / 字体 / 音效 / 资源包），DPS 与 DPR 共用 |
| `Cargo.toml` / `Cargo.lock` | **整个工作区唯一的一份**，6 个成员：`DPS/` 的 5 个 + `DPR/src-tauri` |
| `run.bat` | 一键开发启动：切到 `DPR/` 后执行 `cargo tauri dev` |
| `target/` | 构建产物与运行期工作目录（**不进 git 仓库**；默认就在仓库根，也可以用 `[build] target-dir` 指到仓库之外）。**位置不能挪**：DPR 运行时的工作目录跟着 exe 走，`assets/`、`config.toml`、`respack/`、`output/`、`tmp/` 都在 `target\debug\` 里 |

**仓库根就是 Cargo 工作区根**：下面所有命令都在仓库根或它的子目录里跑。

## 环境要求

| 需要 | 说明 |
| --- | --- |
| 系统 | 主要面向 **Windows 10/11 (x64)**；`DPR/` 基于 Tauri 2，代码本身是跨平台的，但本项目只在 Windows 上验证过 |
| Rust | 稳定版工具链；Windows 上使用 **MSVC** target（本项目用 `stable-x86_64-msvc`） |
| C/C++ 构建工具 | Windows 需 **Microsoft C++ Build Tools / Visual Studio 生成工具**（含 MSVC 与 Windows SDK）——Rust 依赖里有带 C 源码的 crate |
| WebView2 | Tauri 2 需要 **WebView2 Runtime**（Windows 10/11 一般已预装） |
| Node.js | 配 **pnpm**（本项目用 pnpm 8）；前端与 Tauri CLI 都在 `DPR/` |
| ffmpeg | **运行期**需要：渲染时由程序调用 ffmpeg 编码。查找顺序是「设置里指定的目录 → `PATH` 里的 `ffmpeg` → exe 同目录的 `ffmpeg.exe`」 |

`prpr-avc` 需要的 FFmpeg **静态库不在 git 仓库里**（`DPS/prpr-avc/static-lib/<TARGET>/` 被 `.gitignore` 排除，体积太大）：缺了它 `prpr-avc` 会编不过。目录位置可以用 `PRPR_AVC_LIBS` 环境变量覆盖（见 `DPS/prpr-avc/build.rs`）。

## 构建与运行

```powershell
# 所有 cargo 命令都在仓库根（= Cargo 工作区根）里跑，例如：
cd <仓库根>

# —— 引擎（DPS）——
cargo build -p dps-main                     # 引擎 + 播放器 → target\debug\dps-main.exe
cargo run -p prpr-pbc -- <输入> <输出.pbc>   # 谱面格式转换

# —— 桌面渲染器（DPR）——
cd DPR
pnpm install                                # 前端依赖 + Tauri CLI
pnpm dev                                    # 只跑前端开发服务器 (http://localhost:5173)
cd ..

cargo build -p dpr                          # 只编 Rust 后端 → target\debug\dpr.exe（dev 形态，见下）

# —— 运行 ——
.\run.bat                                   # 开发模式启动完整应用（等价于在 DPR\ 里 cargo tauri dev）
.\target\debug\dpr.exe --render -i <谱面.pez|.json> -o <输出目录>   # 无界面渲染

# —— 出安装包 ——
cd DPR
cargo tauri build                           # 产物在仓库根 target\release\bundle\
```

- `cargo tauri …` 由 Tauri CLI 提供（它也列在 `DPR/package.json` 的依赖里，等价写法是 `pnpm exec tauri …`）。
- **默认的 `cargo build -p dpr` 是 dev 形态**：Tauri 在没有 `custom-protocol` feature 时不会把前端打进 exe，而是加载 `tauri.conf.json` 的 `devUrl`（`http://localhost:5173`）——所以单独跑它要先 `pnpm dev`。要出「前端嵌进 exe」的正式形态，加 feature：`cargo build -p dpr --features custom-protocol`（`cargo tauri build` 就是带这个 feature 跑的）；改过前端要先 `pnpm build`（含 `vue-tsc` 类型检查）再编一次，新的 `dist/` 才会进 exe。
- 渲染相关命令行参数还有 `--preview` / `--play` / `--tweakoffset`，以及 `-c <配置.toml>` 指定一份一次性配置。
- 渲染输出默认落在 exe 旁的 `output\`，临时文件在 `tmp\`，配置与资源包分别是 `config\`、`respack\` —— 都在 `target\debug\` 里。

## 许可证与出处

- **本仓库整体以 [GNU GPL-3.0](LICENSE) 发布**；其中 `DPR/` 谱系包含按 **Apache-2.0** 提供的上游代码（`phira-render`）。两者**单向兼容**：这部分代码并入本仓库后，整体按 GPL-3.0 分发。再分发本程序或其源码时，请一并满足 GPL-3.0 的**源码提供**与**许可证保留**要求。
- **代码谱系**（谁从谁那儿来、各自的许可证）：见上面的「谱系：fork 网络」一节。
- **逐项出处、版权与致谢**：[`NOTICE`](NOTICE)。
- **第三方组件与依赖的许可证**：[`LICENSES/README.md`](LICENSES/README.md)；Rust 依赖见 [`Cargo.lock`](Cargo.lock)，前端依赖见 [`DPR/pnpm-lock.yaml`](DPR/pnpm-lock.yaml)。
- **第三方素材的版权归各自权利人**：`assets/Minecraft/` 的贴图与打击音效 © Mojang AB；`assets/respack/`、`assets/rank/`、字体等来自 Phira 及其社区资源包。它们随仓库分发只是为了让程序开箱可用，权利人如要求会立即移除；这些素材**不在 GPL-3.0 覆盖范围内**。
- **致谢**：Phi-Recorder 的作者 HLMC、Mivik；Phira 与 phira-render 的 TeamFlos 及其贡献者；Phira-Pro 的维护者；以及所有被直接或间接引用的开源项目作者。
- **非官方声明**：与 Pigeon Games（Phigros）、TeamFlos（Phira）、Mojang AB / Microsoft 均无任何隶属、授权或赞助关系；相关游戏名与商标归各自权利人。

## 反馈

构建问题、渲染异常、素材权利问题，直接联系作者（DaogeMM）说明即可。
