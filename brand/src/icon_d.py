from build import *
import subprocess, sys, tempfile, os, numpy as np
from PIL import Image
# rasterize faceted silhouette (with facet gaps) to a dot grid
#
# icon_svg() with no arguments is the master icon (assets/icon/ravenglass-icon-D_braille*.svg).
# For small sizes (plan §4: "at <= 24 px regenerate with a coarser dot grid"; 32 px too, where
# the master turns to mush) pass `size`, the
# target pixel size: the grid is then coarse (`dots` rows), dots sit on whole pixels, the mark
# fills more of the tile, facet gaps and the glow are dropped, and the dots are larger
# (`pitch_px` whole pixels apart with a one-pixel gap). brand/src/export_icons.py picks the grids.
VBx,VBy,VBw,VBh=126,140,310,304

def _mask(W,H,facets=True):
    segs=facet_lines()
    lines=''.join(f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}"/>' for a,b in segs) if facets else ''
    svg=f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{VBx} {VBy} {VBw} {VBh}" width="{W}" height="{H}"><rect x="{VBx}" y="{VBy}" width="{VBw}" height="{VBh}" fill="#fff"/><path d="{outline_d()}" fill="#000"/><g stroke="#fff" stroke-width="{VBw/W*1.1}">{lines}</g></svg>'
    with tempfile.TemporaryDirectory() as d:
        open(f'{d}/d.svg','w').write(svg); subprocess.run(['rsvg-convert',f'{d}/d.svg','-o',f'{d}/d.png'],check=True)
        return np.array(Image.open(f'{d}/d.png').convert('L'))

def icon_svg(light=False, rows=8, size=None, dots=None, pitch_px=None, thresh=0.45):
    H=dots if dots else rows*4; W=int(round(H*VBw/VBh))
    small=size is not None
    a=_mask(W,H,facets=not small)
    if small:
        px=512/size                                   # one target pixel in icon units
        pp=pitch_px or max(2,round(420/max(W,H)/px))  # whole pixels per dot; the mark fills ~82%
        pitch=pp*px; dot_px=pp-1                      # a one-pixel gap between dots
        oy=(int((size-(H-1)*pp-dot_px)/2)+dot_px/2)*px
        ox=(-int(-(size-(W-1)*pp-dot_px)/2)+dot_px/2)*px  # odd slack on the left keeps the hackles off the corner
    else:
        pitch=300/max(W,H); ox=256-W*pitch/2+8; oy=256-H*pitch/2+10
    DOT='#2F2A80' if light else ICE
    ex=ox+(EYE[0]-VBx)/VBw*W*pitch; ey=oy+(EYE[1]-VBy)/VBh*H*pitch
    if small:  # the eye takes the place of the dot nearest to it
        exi=int(round((EYE[0]-VBx)/VBw*W-0.5)); eyi=int(round((EYE[1]-VBy)/VBh*H-0.5)); ex=ox+exi*pitch; ey=oy+eyi*pitch
    dots=''
    for y in range(H):
        for x in range(W):
            v=1-a[y,x]/255
            if v>thresh:
                if small and (x,y)==(exi,eyi): continue
                r=pitch*(0.43 if light else 0.36) if not small else dot_px*px*0.56
                # light from top-right: brighter dots there
                k=(0.55+0.45*((x/W)*0.5+(1-y/H)*0.5)) if not light else (0.80+0.20*((1-x/W)*0.5+(y/H)*0.5))
                if small: k=max(k,0.85)
                dots+=f'<circle cx="{ox+x*pitch:.1f}" cy="{oy+y*pitch:.1f}" r="{r:.2f}" fill="{DOT}" fill-opacity="{k:.2f}"/>'
    defs=bg_defs()+'<filter id="g2" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="3" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>'
    defs+='<linearGradient id="frost" x1="0.2" y1="0" x2="0.6" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#DCE0F4"/></linearGradient><radialGradient id="mist" cx="0.74" cy="0.24" r="0.6"><stop offset="0" stop-color="#A9DCFF" stop-opacity="0.45"/><stop offset="1" stop-color="#A9DCFF" stop-opacity="0"/></radialGradient>'
    if light:
        bgfill='<rect width="512" height="512" fill="url(#frost)"/><rect width="512" height="512" fill="url(#mist)"/>'
        eyec='#1F6FBF'; glow=''; rim='<path d="'+SQ+'" fill="none" stroke="#4E48A0" stroke-opacity="0.18" stroke-width="2"/>'
    else:
        bgfill='<rect width="512" height="512" fill="#0E0C1A"/><rect width="512" height="512" fill="url(#moon)" opacity="0.6"/>'
        eyec='#fff'; glow=' filter="url(#g2)"'; rim='<path d="'+SQ+'" fill="none" stroke="#fff" stroke-opacity="0.18" stroke-width="2"/>'
    if small: glow=''; rim=''
    er=pitch*0.7 if not small else dot_px*px*0.6
    body=f'''<g clip-path="url(#sq)">{bgfill}
<g{glow}>{dots}</g><circle cx="{ex:.1f}" cy="{ey:.1f}" r="{er:.1f}" fill="{eyec}"/>
<rect width="512" height="512" fill="url(#spec)" opacity="{0.2 if light else 0.35}"/></g>{rim}'''
    return wrap(body,defs)

if __name__=='__main__':
    LIGHT='--light' in sys.argv
    open('D_braille_light.svg' if LIGHT else 'D_braille.svg','w').write(icon_svg(LIGHT))
