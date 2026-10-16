# DPRecorder

把 **Phigros 谱面离线渲染成视频**的 Windows 桌面程序：**Tauri 2** 外壳 + **Vue 3** 前端 + 内嵌的 Rust 节奏游戏引擎 **DPS**（解析谱面、跑判定、把每一帧画出来）。

> 这个项目 = **DPRecorder**（桌面渲染器）+ **DPS**（它内嵌的引擎）。
> 本页是**上手页**：先讲清这是什么、怎么跑起来，再告诉你想改什么该去读哪一篇。面向使用者的简短说明见 [`DPR/README.md`](DPR/README.md)。

## 怎么跑起来

先看下面的 [环境要求](#环境要求)：Windows 10/11 (x64)、Rust（MSVC target）、Microsoft C++ Build Tools、WebView2 Runtime、Node.js + pnpm；**要渲染出片**还需要 `ffmpeg`（在设置里指定它的目录，或放进 `PATH` / exe 同目录）。

```powershell
cd E:\path\to\DPRecorder          # 仓库根 = Cargo 工作区根，所有命令都在仓库根或其子目录里跑

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

## 想改什么，去读哪一篇

| 你的目标 | 先读 |
| --- | --- |
| **在本机干活的智能体**：工作区约定、快照流程、沙箱坑 | [`AGENTS.md`](AGENTS.md)（先读这一页）；动工前再看一眼 `backsupVersion/README.md` 最上面一行，确认进度 |
| 项目全貌：目录地图、环境、常用命令、打快照 | [`README.dev.md`](README.dev.md) |
| 想知道软件**怎么用**：界面、渲染流程、文件名格式、批量、任务队列、命令行 | [`docs/quick-start.md`](docs/quick-start.md) |
| 改**记录器**：界面、配置接线、IPC、任务队列、打包 | [`docs/dpr/overview.md`](docs/dpr/overview.md) |
| 改**画面本身**：音符、判定线、判定范围、粒子、场景、谱面解析、资源 | [`docs/dps/overview.md`](docs/dps/overview.md) |
| 不知道改哪里 / 撞上报错 | [`docs/workflows/04-common-changes.md`](docs/workflows/04-common-changes.md) |
| 新增或改一个渲染配置项（要动哪六处 + 自检） | [`docs/workflows/01-add-render-option.md`](docs/workflows/01-add-render-option.md) |
| 无界面渲染、时间窗口、抽帧与像素差分验证 | [`docs/workflows/02-render-and-verify.md`](docs/workflows/02-render-and-verify.md) |
| 打快照、前端 / 后端构建、打包 | [`docs/workflows/03-backup-and-release.md`](docs/workflows/03-backup-and-release.md) |
| 文档索引 | [`docs/README.md`](docs/README.md) |

> `docs/`、`AGENTS*.md`、`README.dev.md`、`backup_code.py`、`backsupVersion/` 这些**物理上在仓库根里，但只属于本机、不随仓库分发**：本项目不使用 git，留痕靠 `backup_code.py` 打快照。文档只负责带你找到入口，**细节一律以代码为准**。

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
| `target/` | 构建产物与运行期工作目录（本机专用，不随仓库分发）。**位置不能挪**：DPR 运行时的工作目录跟着 exe 走，`assets/`、`config.toml`、`respack/`、`output/`、`tmp/` 都在 `target\debug\` 里 |

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

原生静态库不用另外下载：`prpr-avc` 需要的 FFmpeg 静态库已随仓库分发（`DPS/prpr-avc/static-lib/`）。

## 构建与运行

```powershell
# 所有 cargo 命令都在仓库根（= Cargo 工作区根）里跑，例如：
cd E:\path\to\DPRecorder

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

## 出处与致谢

- **代码出处**：

  | 本仓库 | 直接上游 | 许可证 |
  | --- | --- | --- |
  | `DPR/`（渲染器，exe `dpr.exe`） | [2278535805/Phi-Recorder](https://github.com/2278535805/Phi-Recorder) | GPL-3.0 |
  | ↑ 谱系源头 | [TeamFlos/phira-render](https://github.com/TeamFlos/phira-render)（官方渲染器，Phi-Recorder 是它的 fork） | Apache-2.0（与 GPL-3.0 单向兼容，并入后按 GPL-3.0 分发） |
  | `DPS/`（引擎） | [2278535805/phire](https://github.com/2278535805/phire) | GPL-3.0 |
  | ↑ 谱系源头 | [TeamFlos/phira](https://github.com/TeamFlos/phira) 的 `prpr` 引擎 | GPL-3.0 |
  | `DPS/dps/src/core/block_area/`（噪域 / blockArea） | [Phira-Pro/Phira-Pro](https://github.com/Phira-Pro/Phira-Pro)，逐字移植 | GPL-3.0 |

- **第三方素材的版权归各自权利人**：`assets/Minecraft/` 的贴图与打击音效 © Mojang AB；`assets/respack/`、`assets/rank/`、字体等来自 Phira 及其社区资源包。它们随仓库分发只是为了让程序开箱可用，权利人如要求会立即移除。
- **致谢**：Phi-Recorder 的作者 HLMC、Mivik；Phira 与 phira-render 的 TeamFlos 及其贡献者；Phira-Pro 的维护者；以及所有被直接或间接引用的开源项目作者。

## 反馈

构建问题、渲染异常、素材权利问题，直接联系作者（DaogeMM）说明即可。
