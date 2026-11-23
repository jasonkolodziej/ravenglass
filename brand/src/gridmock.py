from ansi import NIGHT, FROST
from braille import braille
from xml.sax.saxutils import escape as E
FS=14; CW=FS*0.6; LH=19
def T(col,row,s,fill,y0=0,x0=0,weight=400,bg=None,under=False):
    x=x0+col*CW; y=y0+row*LH
    out=''
    if bg: out+=f'<rect x="{x:.1f}" y="{y:.1f}" width="{len(s)*CW:.1f}" height="{LH}" fill="{bg}"/>'
    out+=f'<text x="{x:.1f}" y="{y+LH*0.74:.1f}" fill="{fill}" font-weight="{weight}"{" text-decoration=\"underline\"" if under else ""} xml:space="preserve">{E(s)}</text>'
    return out
def icon(kind,x,y,c):
    p={'compose':'M5 19h4l10-10-4-4L5 15z M13 7l4 4','search':'M10.5 17a6.5 6.5 0 1 0 0-13 6.5 6.5 0 0 0 0 13z M15.5 15.5 20 20',
       'inbox':'M4 13l3-8h10l3 8v6H4z M4 13h5l1 2h4l1-2h5','archive':'M4 5h16v4H4z M6 9v10h12V9 M10 13h4','flag':'M6 21V4 M6 4h11l-2 4 2 4H6',
       'split':'M4 5h16v14H4z M12 5v14','palette':'M5 7h14 M5 12h14 M5 17h9','gear':'M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z M12 3v3 M12 18v3 M3 12h3 M18 12h3 M5.6 5.6l2.1 2.1 M16.3 16.3l2.1 2.1 M5.6 18.4l2.1-2.1 M16.3 7.7l2.1-2.1',
       'help':'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6V14 M12 17.5v.5'}[kind]
    return f'<g transform="translate({x},{y}) scale(0.8)" fill="none" stroke="{c}" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="{p}"/></g>'
def frame(p,W,H,tabs,body,ws_active=1,rail=True):
    bg=p['background'];fg=p['foreground'];mu=p['silver'];ch=p['gray']
    s=f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" width="{W}" height="{H}" font-family="IBM Plex Mono" font-size="{FS}">'
    s+=f'<rect width="{W}" height="{H}" fill="{bg}"/>'
    s+=f'<circle cx="22" cy="18" r="6" fill="#ff5f57"/><circle cx="42" cy="18" r="6" fill="#febc2e"/><circle cx="62" cy="18" r="6" fill="#28c840"/>'
    x=96
    for i,(t,active) in enumerate(tabs):
        s+=f'<text x="{x}" y="23" fill="{fg if active else mu}" xml:space="preserve">•  {E(t)}</text>'; x+= (len(t)+6)*CW
    s+=body
    if rail:
        rx=W-40
        s+=f'<line x1="{rx}" y1="36" x2="{rx}" y2="{H-34}" stroke="{ch}"/>'
        for i,k in enumerate(['compose','search','inbox','archive','flag','split','palette','gear','help']):
            s+=icon(k,rx+10,52+i*36,mu)
    # workspace bar
    wy=H-26
    ws=[(1,'iCloud'),(2,'Gmail'),(3,'Work'),(5,'receipts'),(6,'from:github')]
    x=14
    for n,name in ws:
        a=(n==ws_active)
        s+=f'<text x="{x}" y="{wy+12}" fill="{p["aqua"] if a else mu}" xml:space="preserve">{n} </text><text x="{x+2*CW}" y="{wy+12}" fill="{fg if a else mu}" xml:space="preserve">{E(name)}</text>'
        x+=(len(name)+5)*CW
    s+=f'<text x="{W-14}" y="{wy+12}" fill="{mu}" text-anchor="end">⟳ synced 10:44 · IDLE</text>'
    return s+'</svg>'

def triage(p,W=1400,H=880):
    fg=p['foreground'];mu=p['silver'];ch=p['gray'];sel=p['navy'];acc=p['aqua']
    b=''; y0=44; LW=700
    rows=[('Odin Allfather','Re: Morning dispatch','Huginn reports the northern pass is clear','10:40',True,False,True),
          ('Muninn','Archive digest — week 46','Everything you asked me to remember','10:21',True,False,False),
          ('GitHub','[anomalyco/rune] dev: new rev','rune-kit Combobox gains animated filtering','09:58',False,False,False),
          ('Fastmail','Your invoice for November','Thanks for using Fastmail. Your receipt','09:12',False,True,True),
          ('Kolodziej Lab','Gnomon scheduler sync','Agenda: backfill policy, preemption','Thu',False,True,False),
          ('Arc HPC','Job 48211 finished (COMPLETED)','Elapsed 03:12:44 on gpu-a100-07','Thu',False,False,False),
          ('Dioxus','Blitz 0.3 release notes','Incremental layout, custom elements','Wed',False,False,False),
          ('UTSA','Spring registration opens','Your enrollment window for the M.S.','Wed',False,False,True),
          ('Mom','Thanksgiving plans','Are you bringing the green chile','Tue',False,True,False),
          ('Servo','This month in Servo','Monthly releases, embedding API','Mon',False,False,False),
          ('Linear','3 issues assigned to you','RG-14 Blitz spike: Parley version','Mon',False,False,False),
          ('Cloudflare','Tunnel healthy','prefect.example.dev is serving','Sun',False,False,False)]
    b+=T(0,0,' INBOX ',p['black'],y0,10,600,bg=acc)+T(8,0,'iCloud · 248 · 12 unread · threaded',mu,y0,10)
    for i,(f,sbj,pv,t,u,flag,att) in enumerate(rows):
        r=2+i*2; isSel=(i==0)
        if isSel: b+=f'<rect x="0" y="{y0+r*LH-2}" width="{LW}" height="{LH*2}" fill="{sel}"/><rect x="0" y="{y0+r*LH-2}" width="3" height="{LH*2}" fill="{acc}"/>'
        num=str(i if not isSel else 1) if not isSel else '1'
        rel=str(abs(i)) if i else '1'
        b+=T(0,r,f'{(i if i else 1):>3}',fg if isSel else ch if False else mu,y0,6, 600 if isSel else 400)
        b+=T(4,r,'●' if u else ' ',acc,y0,6)
        b+=T(6,r,f'{f:<16.16}',fg,y0,6,700 if u else 400)
        b+=T(23,r,f'{sbj:<36.36}',fg,y0,6,600 if u else 400)
        flags=('⚑' if flag else ' ')+('⎘' if att else ' ')
        b+=T(73,r,flags[0],p['yellow'],y0,6)+T(75,r,flags[1],p['blue'],y0,6)
        b+=T(77,r,f'{t:>5}',mu,y0,6)
        b+=T(23,r+1,f'{pv:<50.50}',mu,y0,6)
    # statusline for left pane
    sy=H-34-LH-6
    b+=f'<rect x="0" y="{sy}" width="{LW}" height="{LH}" fill="{ch}"/>'
    b+=T(0,0,' NORMAL ',p['black'],sy,0,600,bg=acc)+T(9,0,'iCloud/Inbox',fg,sy,0)+T(23,0,'⎇ threaded',mu,sy,0)+T(35,0,'● 12',acc,sy,0)+T(41,0,'⚑ 3',p['yellow'],sy,0)+T(68,0,'1/248',mu,sy,0)+T(75,0,' mail ',p['black'],sy,0,600,bg=p['blue'])
    # divider + focus bar
    b+=f'<line x1="{LW+2}" y1="36" x2="{LW+2}" y2="{H-34}" stroke="{ch}" stroke-width="2"/><rect x="{LW}" y="40" width="4" height="{(H-80)*0.45}" fill="{acc}"/>'
    # reader
    rx=LW+16; ry=y0; RWc=int((W-40-rx-12)/CW)
    R=[('From','Odin Allfather <odin@asgard.example>'),('To','me <jason@…>'),('Date','Fri 13 Nov 2026 10:40'),('Subject','Re: Morning dispatch')]
    for i,(k,v) in enumerate(R):
        b+=T(0,i,f'{k+":":<9}',p['blue'],ry,rx,600)+T(9,i,v,fg,ry,rx,600 if k=='Subject' else 400)
    b+=T(0,4,'⎘ route-north.pdf · 2.1 MB    ⎘ ravens.jpg · 840 KB',p['blue'],ry,rx)
    b+=T(RWc-24,0,'view: html │ t text',mu,ry,rx)
    b+=f'<line x1="{rx}" y1="{ry+5.6*LH}" x2="{W-52}" y2="{ry+5.6*LH}" stroke="{ch}"/>'
    body=['Morning,','','Huginn reports the northern pass is clear and the','road to Uppsala is open again. Muninn has the full','archive if you need last winter\'s routes.','','Maps are attached. The ferry schedule is here:']
    for i,l in enumerate(body): b+=T(0,7+i,l,fg,ry,rx)
    b+=T(0,14,'https://ferries.example/norrland/winter',acc,ry,rx,under=True)
    b+=T(0,16,'— O.',fg,ry,rx)
    q=['> On Thu, Nov 12, Jason wrote:','> Is the pass open yet? I want to send the','> Gnomon build north before the snow.']
    for i,l in enumerate(q): b+=T(0,18+i,l,mu,ry,rx)
    # compose pane title bar + box (like reference's lower pane)
    cy=ry+23*LH
    b+=f'<rect x="{rx-14}" y="{cy}" width="{W-40-(rx-14)}" height="{LH}" fill="{ch}"/>'
    b+=T(0,0,'⠿',mu,cy,rx-8)+T(int(RWc/2)-8,0,'reply ▸ Odin Allfather',fg,cy,rx,600)+T(RWc-8,0,'draft ✓',p['lime'],cy,rx)
    by=cy+LH*1.6
    b+=f'<rect x="{rx}" y="{by}" width="{W-52-rx}" height="{LH*5.5}" fill="none" stroke="{mu}" stroke-opacity="0.6"/>'
    b+=T(1,0.4,'Thanks — sending the build north today. Tell Huginn',fg,by,rx)+T(1,1.4,'to watch the ferry dock.',fg,by,rx)
    b+=f'<rect x="{rx+25*CW}" y="{by+1.4*LH+2}" width="{CW}" height="{LH-4}" fill="{p["cursor"]}"/>'
    b+=T(1,4.3,'⇧⌘D send · ⌘⇧A attach · esc normal',mu,by,rx)
    tabs=[('Inbox — iCloud',True),('Re: Morning dispatch',True),('Draft: Gnomon sync',False),('receipts',False)]
    return frame(p,W,H,tabs,b)

def empty(p,W=1400,H=880,title='Night'):
    fg=p['foreground'];mu=p['silver']
    from raven import dot_field
    w,h,dots=dot_field(26)
    px,py=CW/2,LH/4; d=min(px,py)*0.62
    aw=w*px; bh=26
    x0=(W-40-aw)/2; y0=(H-bh*LH)/2-60
    # one dashed, round-capped stroke per horizontal run of dots: same look, tiny file
    rows_={}
    for x,y,e in dots:
        if not e: rows_.setdefault(y,[]).append(x)
    segs=[]
    for y,xs in rows_.items():
        xs.sort(); start=prev=xs[0]
        for x in xs[1:]+[None]:
            if x is not None and x==prev+1: prev=x; continue
            segs.append(f'M{x0+start*px+px/2:.1f} {y0+y*py+py/2:.1f}h{(prev-start)*px:.1f}')
            if x is not None: start=prev=x
    eye=[(x,y) for x,y,e in dots if e]
    eyes=''.join(f'<circle cx="{x0+x*px+px/2:.1f}" cy="{y0+y*py+py/2:.1f}" r="{d/2:.2f}" fill="{p["aqua"]}"/>' for x,y in eye)
    b=f'<g opacity="0.9"><path d="{"".join(segs)}" fill="none" stroke="{p["blue"]}" stroke-width="{d:.2f}" stroke-linecap="round" stroke-dasharray="0 {px:.2f}"/>{eyes}</g>'
    hint=['RAVENGLASS','No message open','/ search    ⇧⌘P commands    1–9 workspaces    c compose']
    cx=(W-40)/2
    b+=f'<text x="{cx}" y="{y0+bh*LH+44}" text-anchor="middle" fill="{fg}" font-weight="600" letter-spacing="6">{hint[0]}</text>'
    b+=f'<text x="{cx}" y="{y0+bh*LH+70}" text-anchor="middle" fill="{mu}">{hint[1]}</text>'
    b+=f'<text x="{cx}" y="{y0+bh*LH+96}" text-anchor="middle" fill="{mu}" xml:space="preserve">{E(hint[2])}</text>'
    return frame(p,W,H,[('Inbox — iCloud',True)],b)
if __name__=='__main__':
    import subprocess
    B='/home/claude/ravenglass-brand/preview/'
    for name,svg in (('ui-triage-night',triage(NIGHT)),('ui-triage-frost',triage(FROST)),('ui-wallpaper-night',empty(NIGHT)),('ui-wallpaper-frost',empty(FROST))):
        open(B+name+'.svg','w').write(svg); subprocess.run(['rsvg-convert',B+name+'.svg','-o',B+name+'.png'],check=True)
