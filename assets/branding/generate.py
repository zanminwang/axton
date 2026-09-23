"""Render the AXTON vector masters and PNG exports. Requires Pillow."""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "assets/branding"
# Geometric uppercase lettering, sheared 10 degrees to the right.
GLYPHS = {
    "A": [[(0,100),(34,0),(57,0),(90,100),(68,100),(61,77),(28,77),(21,100)],
          [(34,59),(55,59),(45,25)]],
    "X": [[(0,0),(25,0),(46,34),(67,0),(92,0),(59,49),(94,100),(69,100),(46,65),(23,100),(0,100),(33,49)]],
    "T": [[(0,0),(86,0),(86,20),(54,20),(54,100),(32,100),(32,20),(0,20)]],
    "O": [[(22,0),(68,0),(88,20),(88,80),(68,100),(22,100),(2,80),(2,20)],
          [(29,20),(22,27),(22,73),(29,80),(61,80),(68,73),(68,27),(61,20)]],
    "N": [[(0,100),(0,0),(21,0),(66,63),(66,0),(87,0),(87,100),(66,100),(21,37),(21,100)]],
}

def shapes(text, x, y, scale):
    for index, letter in enumerate(text):
        yield [[(x + scale * (px + index * 108 + .176327 * (100-py)), y + scale * py)
                for px, py in contour] for contour in GLYPHS[letter]]

def render(name, text, width, height, x, y, scale, background=None):
    letters = list(shapes(text, x, y, scale))
    paths = []
    for contours in letters:
        d = " ".join("M " + " L ".join(f"{px:.3f},{py:.3f}" for px,py in c) + " Z" for c in contours)
        paths.append(f'<path d="{d}"/>')
    bg = f'<path fill="{background}" d="M0 0H{width}V{height}H0Z"/>' if background else ""
    svg = f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {height}" role="img" aria-label="AXTON"><title>AXTON</title>{bg}<g fill="#111111" fill-rule="evenodd">' + "".join(paths) + '</g></svg>\n'
    (OUT / f"{name}.svg").write_text(svg)
    factor = 3
    image = Image.new("RGBA", (width*factor,height*factor), background or (0,0,0,0))
    for contours in letters:
        mask = Image.new("L", image.size)
        draw = ImageDraw.Draw(mask)
        for i, contour in enumerate(contours):
            draw.polygon([(round(px*factor),round(py*factor)) for px,py in contour], fill=255 if i==0 else 0)
        image.paste((17,17,17,255),(0,0),mask)
    image.resize((width,height),Image.Resampling.LANCZOS).save(OUT/f"{name}.png")

render("axton-logo", "AXTON", 1200, 300, 52, 50, 2)
render("axton-icon", "A", 512, 512, 112, 106, 3, "#ffffff")
render("axton-logo-preview", "AXTON", 1200, 400, 52, 100, 2, "#ffffff")
