# DPRecorder

快 简 轻 便  
Fast Simple Lightweight Convenient

把 **Phigros 谱面离线渲染成视频**的 Windows 桌面程序。本目录 `DPR/` 是它的桌面端：**Vue 3** 前端 + **Tauri 2** 外壳 + Rust 后端，内嵌引擎 **DPS**（解析谱面、跑判定、逐帧渲染）。

- 仓库全貌、目录结构、环境要求：看根目录的 [`README.md`](../README.md)

## Download 下载

安装包不随仓库分发：按下一节自行构建，产物在 `target\release\bundle\`。

## build 构建

在仓库根（= Cargo 工作区根）执行：

```bash
pnpm --dir DPR install
cd DPR
cargo tauri build
```

产物在仓库根 `target\release\bundle\`。开发模式直接跑仓库根的 `run.bat`（等价于在 `DPR\` 里 `cargo tauri dev`）。运行期需要系统里有 **ffmpeg**（在设置里指定它的目录，或把它放进 `PATH`）。

## 注意事项 / Notice

- 请不要伪造游玩成绩、官方内容等，以免造成不好的影响  
  Please do not falsify game results, official content, etc. to avoid causing adverse effects
- 游玩界面与本家有明显区分, 如 COMBO 处文本与 `COMBO` 有明显区分  
  The game interface is clearly distinguished from the main interface, such as the text at COMBO is clearly distinguished from `COMBO`
- 不建议向不使用 `Re: PhiEdit` 的玩家分享本软件
- Sharing this software with players who do not use `Re: PhiEdit` is not recommended

## 出处 / Provenance

本目录的代码谱系：[TeamFlos/phira-render](https://github.com/TeamFlos/phira-render)（Apache-2.0，官方渲染器）→ [2278535805/Phi-Recorder](https://github.com/2278535805/Phi-Recorder)（GPL-3.0，本目录的直接上游）。
