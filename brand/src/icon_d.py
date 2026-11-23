from build import *
import subprocess, numpy as np
from PIL import Image
# rasterize faceted silhouette (with facet gaps) to a dot grid
rows=8; H=rows*4; VBx,VBy,VBw,VBh=126,140,310,304; W=int(round(H*VBw/VBh))
segs=facet_lines()
lines=''.join(f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}"/>' for a,b in segs)
svg=f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{VBx} {VBy} {VBw} {VBh}" width="{W}" height="{H}"><rect x="{VBx}" y="{VBy}" width="{VBw}" height="{VBh}" fill="#fff"/><path d="{outline_d()}" fill="#000"/><g stroke="#fff" stroke-width="{VBw/W*1.1}">{lines}</g></svg>'
open('/tmp/d.svg','w').write(svg); subprocess.run(['rsvg-convert','/tmp/d.svg','-o','/tmp/d.png'],check=True)
a=np.array(Image.open('/tmp/d.png').convert('L'))
pitch=300/max(W,H); ox=256-W*pitch/2+8; oy=256-H*pitch/2+10
import sys
LIGHT='--light' in sys.argv
DOT='#2F2A80' if LIGHT else ICE
dots=''
for y in range(H):
    for x in range(W):
        v=1-a[y,x]/255
        if v>0.45:
            r=pitch*(0.43 if LIGHT else 0.36)
            # light from top-right: brighter dots there
            k=(0.55+0.45*((x/W)*0.5+(1-y/H)*0.5)) if not LIGHT else (0.80+0.20*((1-x/W)*0.5+(y/H)*0.5))
            dots+=f'<circle cx="{ox+x*pitch:.1f}" cy="{oy+y*pitch:.1f}" r="{r:.2f}" fill="{DOT}" fill-opacity="{k:.2f}"/>'
ex=ox+(EYE[0]-VBx)/VBw*W*pitch; ey=oy+(EYE[1]-VBy)/VBh*H*pitch
defs=bg_defs()+'<filter id="g2" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="3" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>'
defs+='<linearGradient id="frost" x1="0.2" y1="0" x2="0.6" y2="1"><stop offset="0" stop-color="#FFFFFF"/><stop offset="1" stop-color="#DCE0F4"/></linearGradient><radialGradient id="mist" cx="0.74" cy="0.24" r="0.6"><stop offset="0" stop-color="#A9DCFF" stop-opacity="0.45"/><stop offset="1" stop-color="#A9DCFF" stop-opacity="0"/></radialGradient>'
if LIGHT:
    bgfill='<rect width="512" height="512" fill="url(#frost)"/><rect width="512" height="512" fill="url(#mist)"/>'
    eyec='#1F6FBF'; glow=''; rim='<path d="'+SQ+'" fill="none" stroke="#4E48A0" stroke-opacity="0.18" stroke-width="2"/>'
else:
    bgfill='<rect width="512" height="512" fill="#0E0C1A"/><rect width="512" height="512" fill="url(#moon)" opacity="0.6"/>'
    eyec='#fff'; glow=' filter="url(#g2)"'; rim='<path d="'+SQ+'" fill="none" stroke="#fff" stroke-opacity="0.18" stroke-width="2"/>'
body=f'''<g clip-path="url(#sq)">{bgfill}
<g{glow}>{dots}</g><circle cx="{ex:.1f}" cy="{ey:.1f}" r="{pitch*0.7:.1f}" fill="{eyec}"/>
<rect width="512" height="512" fill="url(#spec)" opacity="{0.2 if LIGHT else 0.35}"/></g>{rim}'''
open('D_braille_light.svg' if LIGHT else 'D_braille.svg','w').write(wrap(body,defs))
