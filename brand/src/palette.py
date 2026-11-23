import json
def lum(h):
    h=h.lstrip('#'); c=[int(h[i:i+2],16)/255 for i in (0,2,4)]
    c=[x/12.92 if x<=0.03928 else ((x+0.055)/1.055)**2.4 for x in c]
    return 0.2126*c[0]+0.7152*c[1]+0.0722*c[2]
def cr(a,b):
    la,lb=sorted((lum(a),lum(b)),reverse=True); return (la+0.05)/(lb+0.05)
NIGHT={ # dark default
 "bg":"#0E0C1A","content":"#15121F","raised":"#1D1930","sunken":"#0A0913","border":"#2C2742",
 "glass":"rgba(42,35,80,0.55)","text":"#ECEAF6","textMuted":"#A7A2C3","textFaint":"#7A759C",
 "accent":"#A9DCFF","accentStrong":"#6CC2FF","onAccent":"#0B1A2A","brand":"#6C66B5","brandSoft":"#8A87D8",
 "success":"#7FD6A6","warning":"#F2C46D","danger":"#FF7A8A","paper":"#FBFAFF","watermark":"#A9DCFF"}
FROST={ # light
 "bg":"#EEF0FA","content":"#FFFFFF","raised":"#F5F6FC","sunken":"#E4E6F4","border":"#DAD9EA",
 "glass":"rgba(185,196,242,0.50)","text":"#17142A","textMuted":"#545070","textFaint":"#76728F",
 "accent":"#1F6FBF","accentStrong":"#155A9E","onAccent":"#FFFFFF","brand":"#4E48A0","brandSoft":"#8A87D8",
 "success":"#22804F","warning":"#9A6407","danger":"#C8344B","paper":"#FFFFFF","watermark":"#4E48A0"}
for name,p in (("night",NIGHT),("frost",FROST)):
    print(name)
    for k in ("text","textMuted","textFaint","accent","success","warning","danger","brandSoft" if name=="night" else "brand"):
        print(f"  {k:10s} on content {cr(p[k],p['content']):.2f}  on raised {cr(p[k],p['raised']):.2f}")
    print(f"  onAccent on accent {cr(p['onAccent'],p['accent']):.2f}")
