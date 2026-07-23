"""
mkemacs 图标生成脚本
输入: assets/logo-mini.png（方形透明底 PNG）
输出: assets/logo.ico（多尺寸图标）+ assets/logo.png（256x256 透明底，用于嵌入）
Usage: python logo.py
"""

from PIL import Image

SRC = "assets/logo-mini.png"
OUT_PNG = "assets/logo.png"
OUT_ICO = "assets/logo.ico"

# ICO 需要覆盖的尺寸（Windows 托盘推荐）
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
# 嵌入程序用的清晰尺寸
EMBED_SIZE = 256


def generate_icons() -> None:
    img = Image.open(SRC)

    # 如果已经是 RGBA 就保持，否则转 RGBA（保持透明）
    if img.mode != "RGBA":
        img = img.convert("RGBA")

    # 补成正方形（居中贴到最大边长）
    w, h = img.size
    size = max(w, h)
    square = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    square.paste(img, ((size - w) // 2, (size - h) // 2))

    # 输出清晰版 PNG（256x256）
    png_embed = square.resize((EMBED_SIZE, EMBED_SIZE), Image.LANCZOS)
    png_embed.save(OUT_PNG, "PNG")
    print(f"保存: {OUT_PNG} ({EMBED_SIZE}x{EMBED_SIZE} RGBA)")

    # 输出各尺寸 ICO
    icons = [square.resize((s, s), Image.LANCZOS) for s in ICO_SIZES]
    icons[0].save(
        OUT_ICO,
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=icons[1:],
    )
    print(f"保存: {OUT_ICO} (包含 {ICO_SIZES} 尺寸)")


if __name__ == "__main__":
    generate_icons()
    print("完成！")
