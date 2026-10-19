"""把 RetriSP 目录打成引擎能用的资源包 zip。

必须用本脚本、不能用 PowerShell 的 `Compress-Archive`：后者写进去的条目名用反斜杠
（`digits\\Result_4.1_0.png`），而引擎按 `/` 拼路径去查条目，会报
`specified file not found in archive`。

用法：python package_zip.py [输出 zip 路径]
默认输出到上一级的 RetriSP.zip（与资源包目录同级）。
"""

import sys
import zipfile
from pathlib import Path

PACK_DIR = Path(__file__).resolve().parent
OUT = Path(sys.argv[1]) if len(sys.argv) > 1 else PACK_DIR.parent / "RetriSP.zip"
# 引擎只认根目录下这 10 个文件，加上可选的结算数字（digits/）与曲目标题（title/）
ROOT_FILES = [
    "info.yml",
    "click.png",
    "click_mh.png",
    "drag.png",
    "drag_mh.png",
    "flick.png",
    "flick_mh.png",
    "hold.png",
    "hold_mh.png",
    "hit_fx.png",
]
EXTRA_DIRS = ["digits", "title"]


def main() -> None:
    entries = [PACK_DIR / name for name in ROOT_FILES]
    for name in EXTRA_DIRS:
        entries.extend(sorted((PACK_DIR / name).glob("*.png")))
    missing = [str(it) for it in entries if not it.is_file()]
    if missing:
        raise SystemExit("missing: " + ", ".join(missing))
    with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as zf:
        for path in entries:
            zf.write(path, path.relative_to(PACK_DIR).as_posix())
    print(f"{OUT} ({len(entries)} entries, {OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
