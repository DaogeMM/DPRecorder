# 第三方许可证索引

本仓库整体以 **GNU GPL-3.0** 发布，全文见根目录的 [`../LICENSE`](../LICENSE)；代码谱系与第三方素材归属见 [`../NOTICE`](../NOTICE)。
这份文件只做**索引**：本仓库引用的上游代码、字体与素材，以及各组件的许可证原文。

## 1. 直接引入本仓库的上游代码

| 本仓库路径 | 来源 | 许可证 | 许可证原文 |
| --- | --- | --- | --- |
| `DPR/`（渲染器） | [2278535805/Phi-Recorder](https://github.com/2278535805/Phi-Recorder) | GPL-3.0 | [`../LICENSE`](../LICENSE) |
| ↑ 谱系源头 | [TeamFlos/phira-render](https://github.com/TeamFlos/phira-render) | Apache-2.0（并入后按 GPL-3.0 分发） | [`Apache-2.0.txt`](Apache-2.0.txt) |
| `DPS/`（引擎） | [2278535805/phire](https://github.com/2278535805/phire) | GPL-3.0 | [`../LICENSE`](../LICENSE) |
| ↑ 谱系源头 | [TeamFlos/phira](https://github.com/TeamFlos/phira) 的 `prpr` 引擎 | GPL-3.0 | [`../LICENSE`](../LICENSE) |
| `DPS/dps/src/core/block_area/` | [Phira-Pro/Phira-Pro](https://github.com/Phira-Pro/Phira-Pro)（逐字移植） | GPL-3.0 | [`../LICENSE`](../LICENSE) |

## 2. 依赖

- **Rust**：完整依赖树与版本见根目录 [`Cargo.lock`](../Cargo.lock)。每个包的许可证可以在自己的机器上离线读出来：

  ```powershell
  cargo metadata --format-version 1 | ConvertFrom-Json |
    Select-Object -ExpandProperty packages |
    Select-Object name, version, license, license_file |
    Sort-Object name | Format-Table -AutoSize
  ```

- **前端**：完整依赖树与版本见 [`DPR/pnpm-lock.yaml`](../DPR/pnpm-lock.yaml)；也可以 `pnpm --dir DPR licenses list` 生成逐包清单。
- 几个被 fork 的自定义依赖（`2278535805/prpr-macroquad`、`prpr-miniquad`、`sasa`、`oboe-rs`）许可证以其各自上游仓库为准，见 [`../NOTICE`](../NOTICE) 第 1 节。
- **`ffmpeg` 不在本仓库内**：程序只是在运行时调用你本机或 PATH 里的 `ffmpeg`。它的构建方（例如 Windows 上的 BtbN 构建）决定它是 LGPL 还是 GPL 版本，与本仓库的 GPL-3.0 无关；分发安装包时请自行确认所用 `ffmpeg` 构建的许可证。

## 3. 不在 GPL-3.0 覆盖范围内的素材

字体（Exo / halva / bold / Sarasa UI SC 等）、`assets/rank/` 里的社区资源包 —— 版权归各自权利人，各依其自身许可条款（多为字体自身的 SIL OFL / 商用许可或「仅供非商业使用」声明），**不适用本仓库的 GPL-3.0**。皮肤包（`appendant.official.zip` / `appendant.old.zip` / `appendant.retri.zip`）与材质包（`appendant.lava.zip` / `appendant.water.zip`）**不在仓库里、也不进安装包**：作者把它们放在网盘 <https://daogemm.lanzouc.com/b0018qnkxi>（密码 `dgmm`），由用户下载后放进程序目录的 `appendant/`。详见 [`../NOTICE`](../NOTICE) 第 4 节。

## 4. 要补的东西

这份索引只覆盖**直接引入**的代码与已知的素材；逐包清单请按上面第 2 节的命令在本地生成。若将来把某个依赖的许可证原文也放进这个目录，命名沿用 `Apache-2.0.txt` 这种「`SPDX 标识符.txt`」的写法。
