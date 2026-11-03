# DPE

谱面编辑器（在研）。DPRecorder 只有一个窗口：DPR（录制器）、DPE（编辑器）是同一个窗口里的两个部分，
标题栏左上角的切换器换部分。外壳（窗口、标题栏、路由、导航）在 `DPR/src`（见那里的 `parts.ts`），
DPE 自己的东西都在这个文件夹里：

| 目录 | 是什么 | 怎么跑 |
| --- | --- | --- |
| `src/` | 原生预览进程：链 `dps` 直接出谱面画面，用行 JSON 协议从 stdin 收命令、往 stdout 报时间 | `cargo build -p dpe`（在 `repository` 根） |
| `ui/` | DPE 的界面页面（谱面修改页、Malody 转谱页、噪域播放器），由外壳的路由以 `@dpe/…` 别名挂进来 | 跟外壳一起跑（`pnpm dev` / `run.bat`） |

`ui/` 的源码在 `DPR/` 外面，而前端依赖装在 `DPR/node_modules` 里，所以仓库根要有一个
指向它的 junction，界面才编得动（只需建一次）：

```powershell
cd <仓库根>; cmd /c mklink /J node_modules DPR\node_modules
```

## 预览进程

```powershell
dpe.exe --input E:\DPRecorder\example\pez2\WATER.A39沙包P.0.IN.pez
dpe.exe --input <谱面目录> --play --speed 1.5
```

`--input` 收「谱面目录」或 `.pez` 文件；`--play` 直接播，默认是暂停在第 0 秒（编辑器语义）。
其余选项见 `dpe.exe --help`。

跑起来之后可以从 stdin 逐行喂 JSON 命令：

```json
{"cmd":"seek","t":12.5,"playing":true}
{"cmd":"pause"}
{"cmd":"reload"}
{"cmd":"quit"}
```

`reload` 是原地换 chart（不重启进程、不丢播放位置），改完谱面文件发这一条就能看到新样子。
每帧会往 stdout 回一行 `{"ev":"frame","t":…,"playing":…,"loaded":…,"length":…,"fps":…}`。
从终端直接起（stdin 不是管道）时不读命令，只当预览窗口用。

## 界面

页面挂在 `ui/`：`ChartModView.vue`（谱面修改，读写渲染页那份工作配置）、`MalodyImportView.vue`（Malody 转谱）、
`VideoNoiseView.vue`（噪域播放器）。谱面列表复用外壳的 `BatchView.vue`（`/charts`，带 `listOnly` 与 `scope: 'dpe'`，存储另存一份）。

Malody 转谱的转换逻辑在 `src/malody.rs`（`.mc` / `.mcz` / 谱面文件夹 → PGR v3 + `info.yml`），由 DPR 的命令层
按路径引用编译（`DPR/src-tauri/src/malody_convert.rs`），产物落到 `<程序目录>/dpe/malody/<曲名 (Malody)>.pez`。

还没接上的那一步：把列表里选中的谱面路径交给 `dpe.exe --input`，再照上面的协议推 `seek` / `reload`，
窗口里就能边改边看。
