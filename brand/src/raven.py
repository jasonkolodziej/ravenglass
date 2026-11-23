import numpy as np, colorsys, hashlib
from scipy.spatial import Delaunay
from matplotlib.path import Path
OUT = [ (424,266),(372,212),(316,188),(292,168),(250,154),(206,168),(182,212),(170,286),(148,372),(138,432),
        (372,432),(368,362),(352,334),(358,330),(340,306),(350,300),(330,280),(320,258),(372,264) ]
INNER = [(262,206),(232,264),(256,352),(292,262),(318,332),(300,214),(206,402),(300,402),(214,316)]
def build():
    pts=np.array(OUT+INNER,float)
    d=Delaunay(pts); path=Path(np.array(OUT))
    tris=[t for t in d.simplices if path.contains_point(pts[t].mean(0))]
    return pts,tris
def shade_l(c, base=0.10, span=0.24, seed=''):
    cx,cy=c; k=((cx-140)/290)*0.55+(1-(cy-150)/290)*0.45
    j=(int(hashlib.md5(seed.encode()).hexdigest(),16)%100)/100
    return min(0.9,max(0,base+span*k+0.05*j))
def hexc(h,l,s):
    r,g,b=colorsys.hls_to_rgb(h,l,s); return f'#{int(r*255):02x}{int(g*255):02x}{int(b*255):02x}'
def facets(hue=0.70,sat=0.32,base=0.09,span=0.24,transform=''):
    pts,tris=build(); out=[f'<g {transform}>']
    for t in tris:
        c=pts[t].mean(0); col=hexc(hue,shade_l(c,base,span,str(sorted(t))),sat)
        p=' '.join(f'{x:.1f},{y:.1f}' for x,y in pts[t])
        out.append(f'<polygon points="{p}" fill="{col}" stroke="{col}" stroke-width="0.9" stroke-linejoin="round"/>')
    out.append('</g>'); return '\n'.join(out)
def outline_d():
    return 'M'+' L'.join(f'{x},{y}' for x,y in OUT)+' Z'
def facet_lines():
    pts,tris=build(); seen=set(); segs=[]
    for t in tris:
        for a,b in ((t[0],t[1]),(t[1],t[2]),(t[2],t[0])):
            k=tuple(sorted((a,b)))
            if k in seen: continue
            seen.add(k); segs.append((pts[a],pts[b]))
    return segs
EYE=(296,198)

def dot_field(rows=26, facets=True):
    """Mirror of ravenglass_brand::dot_field (Rust): dots inside the silhouette, minus facet gaps; eye flagged."""
    VB=(126.0,140.0,310.0,304.0); h=rows*4; w=int(round(h*VB[2]/VB[3]/2))*2
    unit=VB[3]/h; gap=unit*0.55; path=Path(np.array(OUT)); segs=facet_lines(); out=[]
    def sd(p,a,b):
        d=b-a; t=max(0,min(1,np.dot(p-a,d)/np.dot(d,d))); return np.linalg.norm(p-(a+t*d))
    for y in range(h):
        for x in range(w):
            p=np.array([VB[0]+(x+.5)*unit, VB[1]+(y+.5)*unit])
            if not path.contains_point(p): continue
            if np.linalg.norm(p-np.array(EYE))<unit*1.2: out.append((x,y,True)); continue
            if facets and any(sd(p,np.array(a),np.array(b))<gap for a,b in segs): continue
            out.append((x,y,False))
    return w,h,out
