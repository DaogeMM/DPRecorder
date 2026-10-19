"""把 Phigros SP（Retribution）的原始按键素材拼成引擎要的贴图。

原始素材是 Phigros 内部导出的一堆散图（文件名里带 # 编号），引擎只要
click/drag/flick/hold 各两张（普通 + 双押提示 _mh），Hold 还要求「尾部 /
中间 / 头部」竖着拼在一张图里。这里只做按键贴图，hit_fx 与结算 UI 不动。

用法：python make_retrisp_notes.py <素材目录> <资源包目录>
"""
import sys
from pathlib import Path

from PIL import Image

SRC = Path(sys.argv[1])
OUT = Path(sys.argv[2])

# Hold 「中间」取身体图里靠中、纹理均匀的一段（原图顶部/底部各有一条烧死的端帽）
BODY_ROWS = (300, 700)


def load(name):
    return Image.open(SRC / name).convert("RGBA")


def fit_width(im, width):
    if im.width == width:
        return im
    height = max(1, round(im.height * width / im.width))
    return im.resize((width, height), Image.LANCZOS)


def compose_hold(tail_name, body_name, head_name, body_rows, width=None):
    """尾部在上、中间居中、头部在下，三者宽度统一（默认取尾部宽度）。

    中间段再上下镜像拼成两倍高：info.yml 里打开 holdRepeat（重复式拉伸）时，
    镜像让每处拼接的像素连续，看不出接缝。
    """
    tail = load(tail_name)
    width = width or tail.width
    if tail.width != width:
        tail = tail.resize((width, max(1, round(tail.height * width / tail.width))), Image.LANCZOS)

    body = load(body_name)
    band = body.crop((0, body_rows[0], body.width, min(body_rows[1], body.height)))
    band = fit_width(band, width)
    flipped = band.transpose(Image.FLIP_TOP_BOTTOM)
    doubled = Image.new("RGBA", (width, band.height * 2), (0, 0, 0, 0))
    doubled.paste(band, (0, 0))
    doubled.paste(flipped, (0, band.height))

    head = load(head_name)
    if head.width != width:
        # 只横向拉伸：「头部」高度就是 info.yml 里 holdAtlas 的第二项，必须保持原值
        head = head.resize((width, head.height), Image.LANCZOS)

    out = Image.new("RGBA", (width, tail.height + doubled.height + head.height), (0, 0, 0, 0))
    out.paste(tail, (0, 0))
    out.paste(doubled, (0, tail.height))
    out.paste(head, (0, tail.height + doubled.height))
    return out, (tail.height, head.height)


def main():
    # 单押 Hold：尾 Hold_End + 身体 Hold + 头 Hold_Head（三张都是 50 px 端帽）
    hold, atlas = compose_hold("Hold_End #5599.png", "Hold #5512.png", "Hold_Head #5575.png", BODY_ROWS)
    hold.save(OUT / "hold.png")
    print("hold.png", hold.size, "atlas", atlas)

    # 双押 Hold：尾沿用单押的 Hold_End，头是带黄边的 Hold2HL_0，身体取自 Hold2HL_1；
    # 画布宽度跟头部一致（双押的贴图本来就宽一些）
    mh_head = load("Hold2HL_0 #5532.png")
    mh, atlas_mh = compose_hold(
        "Hold_End #5599.png", "Hold2HL_1 #5472.png", "Hold2HL_0 #5532.png", BODY_ROWS, width=mh_head.width
    )
    mh.save(OUT / "hold_mh.png")
    print("hold_mh.png", mh.size, "atlas", atlas_mh)

    # 点按用专门的 Tap 素材（青条）；双押提示用它的加粗版
    tap = load("Tap (1).png")
    tap.save(OUT / "click.png")
    tap_mh = load("Tap 双押.png")
    tap_mh.save(OUT / "click_mh.png")
    print("click", tap.size, "click_mh", tap_mh.size)

    drag = load("Drag #5558.png")
    drag.save(OUT / "drag.png")
    drag_mh = load("Drag 双押 #5466.png")
    drag_mh.save(OUT / "drag_mh.png")
    print("drag", drag.size, "drag_mh", drag_mh.size)

    flick = load("Flick #5612.png")
    flick.save(OUT / "flick.png")
    flick_mh = load("Flick 双押 #5482.png")
    flick_mh.save(OUT / "flick_mh.png")
    print("flick", flick.size, "flick_mh", flick_mh.size)


if __name__ == "__main__":
    main()
